use std::time::{Duration, Instant};

use lt_protocol::request_list::{BlockTransfer, RequestList};

#[test]
fn test_request_list_buckets() {
    let mut rl = RequestList::new();
    assert_eq!(rl.queued_size(), 0);
    assert_eq!(rl.unordered_size(), 0);
    assert_eq!(rl.stalled_size(), 0);
    assert_eq!(rl.choked_size(), 0);

    // Delegate 3 pieces
    let pieces = rl.delegate(3);
    assert_eq!(pieces.len(), 3);
    assert_eq!(rl.queued_size(), 3);

    // Peer choked
    rl.choked();
    assert_eq!(rl.queued_size(), 0);
    assert_eq!(rl.choked_size(), 3);

    // Peer unchoked
    rl.unchoked();
    assert_eq!(rl.queued_size(), 3);
    assert_eq!(rl.choked_size(), 0);

    // Pop one piece for downloading
    let current = rl.pop_queued();
    assert!(current.is_some());
    assert!(rl.is_downloading());
    assert_eq!(rl.queued_size(), 2);

    // Initial stall
    rl.stall_initial();
    assert!(!rl.is_downloading());
    assert_eq!(rl.stalled_size(), 1);

    // Prolonged stall
    rl.stall_prolonged();
    assert_eq!(rl.queued_size(), 0);
    assert_eq!(rl.stalled_size(), 3);

    // Pipe size calculation
    assert_eq!(RequestList::calculate_pipe_size(0, 100), 4);
    assert_eq!(RequestList::calculate_pipe_size(16384 * 10, 100), 10);
    assert_eq!(RequestList::calculate_pipe_size(16384 * 200, 100), 100);

    rl.clear();
    assert_eq!(rl.stalled_size(), 0);
}

#[test]
fn remove_tracks_all_buckets_and_current_transfer() {
    let mut list = RequestList::new();
    let queued = BlockTransfer::new(1, 0, 16384);
    let choked = BlockTransfer::new(2, 0, 16384);
    let current = BlockTransfer::new(3, 0, 16384);

    list.push_queued(queued);
    list.push_queued(choked);
    list.push_queued(current);
    let _ = list.pop_queued(); // first item becomes current
    list.choked();

    assert_eq!(list.len(), 3);
    assert!(list.contains(queued));
    assert!(list.remove(queued));
    assert!(!list.contains(queued));
    assert_eq!(list.len(), 2);

    assert!(list.remove(current));
    assert_eq!(list.len(), 1);
    assert!(!list.remove(BlockTransfer::new(99, 0, 16384)));
}

#[test]
fn default_queue_limit_and_clamping() {
    let mut list = RequestList::new();
    assert_eq!(list.queue_limit(), 250);
    assert_eq!(list.block_size(), 16384);

    list.set_queue_limit(0);
    assert_eq!(list.queue_limit(), 1, "a peer queue is never zero");
    list.set_queue_limit(10_000);
    assert_eq!(list.queue_limit(), 2048, "protocol maximum is enforced");

    list.set_block_size(0);
    assert_eq!(list.block_size(), 16384, "zero block size is ignored");
    list.set_block_size(32768);
    assert_eq!(list.block_size(), 32768);
}

#[test]
fn outstanding_pipeline_respects_queue_limit_and_duplicates() {
    let mut list = RequestList::with_queue_limit(2);
    let now = Instant::now();
    let first = BlockTransfer::new(0, 0, 16384);
    let second = BlockTransfer::new(0, 16384, 16384);
    let third = BlockTransfer::new(1, 0, 16384);

    assert!(list.register(first, now));
    assert!(!list.register(first, now), "non-endgame duplicate refused");
    assert!(list.register(second, now));
    assert_eq!(list.queue_depth(), 2);
    assert_eq!(list.available_slots(), 0);
    assert!(list.is_full());
    assert!(!list.register(third, now), "queue limit enforced");

    assert_eq!(list.take_block_at(0, 0).len(), 1);
    assert_eq!(list.queue_depth(), 1);
    assert_eq!(list.available_slots(), 1);
}

#[test]
fn endgame_duplicates_are_capped_per_block() {
    let mut list = RequestList::with_queue_limit(16);
    list.set_max_endgame_duplicates(2);
    let now = Instant::now();
    let block = BlockTransfer::new(3, 0, 16384);

    assert!(list.register(block, now));
    assert!(list.register_endgame(block, now));
    assert!(
        !list.register_endgame(block, now),
        "third copy exceeds the endgame cap"
    );
    assert_eq!(list.outstanding_count(block), 2);
    assert_eq!(list.endgame_count(), 1);

    let removed = list.take_block(block);
    assert_eq!(removed.len(), 2);
    assert_eq!(list.queue_depth(), 0);
}

#[test]
fn expired_requests_are_removed_and_report_attempts() {
    let mut list = RequestList::with_queue_limit(4);
    list.set_request_timeout(Duration::from_millis(20));

    let stale = BlockTransfer::new(0, 0, 16384);
    let fresh = BlockTransfer::new(0, 16384, 16384);
    let old = Instant::now() - Duration::from_millis(100);

    assert!(list.register(stale, old));
    assert!(list.register(fresh, Instant::now()));

    let expired = list.expire(Instant::now());
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].transfer, stale);
    assert_eq!(expired[0].attempts, 1);
    assert_eq!(list.queue_depth(), 1);
    assert!(list.contains_outstanding(fresh));
}

#[test]
fn choke_drains_outstanding_requests_for_requeue() {
    let mut list = RequestList::with_queue_limit(4);
    let now = Instant::now();

    assert!(list.register(BlockTransfer::new(0, 0, 16384), now));
    assert!(list.register(BlockTransfer::new(0, 16384, 16384), now));
    assert_eq!(list.queue_depth(), 2);

    let drained = list.take_outstanding();
    assert_eq!(drained.len(), 2);
    assert_eq!(list.queue_depth(), 0);
    assert_eq!(list.available_slots(), 4);

    // After unchoke the blocks can be registered again.
    assert!(list.register(drained[0].transfer, Instant::now()));
    assert_eq!(list.queue_depth(), 1);
}

#[test]
fn dynamic_bdp_pipelining_and_snubbing() {
    let mut list = RequestList::new();
    let now = Instant::now();
    let sent_at = now - Duration::from_millis(20);

    // 16KB received in 20ms -> 800 KB/s rate -> target ~ 73 blocks
    list.on_block_received(16384, Some(sent_at), now);
    assert!(list.rate_ewma() > 100_000.0);
    assert!(list.queue_limit() >= 50);

    // Register an outstanding request
    assert!(list.register(BlockTransfer::new(1, 0, 16384), now));
    assert!(!list.check_snubbing(now + Duration::from_secs(5)));

    // After 16 seconds of inactivity: snubbing triggers
    assert!(list.check_snubbing(now + Duration::from_secs(16)));
    assert!(list.is_snubbed());
    assert_eq!(list.queue_limit(), 2);
}
