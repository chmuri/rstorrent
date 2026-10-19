use lt_storage::hash_check_queue::{
    HashCheckQueue, DEFAULT_STARVATION_LIMIT, FLAG_NEED, FLAG_WILL_NEED,
};

#[test]
fn duplicate_enqueue_does_not_schedule_twice() {
    let mut queue = HashCheckQueue::new();

    assert!(queue.insert(9, FLAG_NEED));
    assert!(!queue.insert(9, FLAG_NEED));
    assert_eq!(queue.size(), 1);
    assert_eq!(queue.stats().duplicates, 1);

    // A chunk already in flight must not be enqueued a second time either.
    let job = queue.pop_next_job().expect("job");
    assert_eq!(job.index, 9);
    assert!(!queue.insert(9, FLAG_NEED));
    assert_eq!(queue.size(), 0);
    assert_eq!(queue.in_flight_size(), 1);

    assert!(queue.complete_token(9, job.token));
    assert!(queue.insert(9, FLAG_NEED));
    assert_eq!(queue.stats().enqueued, 2);
}

#[test]
fn need_is_served_before_will_need_and_fifo_within_tier() {
    let mut queue = HashCheckQueue::new();
    queue.set_starvation_limit(None);

    assert!(queue.insert(30, FLAG_WILL_NEED));
    assert!(queue.insert(10, FLAG_NEED));
    assert!(queue.insert(31, FLAG_WILL_NEED));
    assert!(queue.insert(11, FLAG_NEED));

    assert_eq!(queue.pop_next(), Some(10));
    assert_eq!(queue.pop_next(), Some(11));
    assert_eq!(queue.pop_next(), Some(30));
    assert_eq!(queue.pop_next(), Some(31));
}

#[test]
fn promotion_moves_will_need_into_the_need_fifo() {
    let mut queue = HashCheckQueue::new();
    queue.insert(5, FLAG_WILL_NEED);
    queue.insert(6, FLAG_NEED);
    assert!(queue.insert(5, FLAG_NEED));

    assert_eq!(queue.flags(5), Some(FLAG_NEED | FLAG_WILL_NEED));
    assert_eq!(queue.stats().promotions, 1);

    // Promotion is ordered at the point it happened, after 6.
    assert_eq!(queue.pop_next(), Some(6));
    assert_eq!(queue.pop_next(), Some(5));
}

#[test]
fn removing_in_flight_job_discards_its_result() {
    let mut queue = HashCheckQueue::new();
    queue.insert(3, FLAG_NEED);
    let job = queue.pop_next_job().unwrap();

    assert!(queue.remove(3));
    assert!(!queue.has(3));
    // The worker may still finish, so the entry remains visible until then.
    assert!(queue.is_in_flight(3));
    assert!(!queue.complete_token(3, job.token));
    assert!(!queue.is_in_flight(3));

    let stats = queue.stats();
    assert_eq!(stats.completed, 0);
    assert_eq!(stats.cancelled_in_flight, 1);
    assert_eq!(stats.stale_results, 1);
}

#[test]
fn result_from_before_reenqueue_is_stale() {
    let mut queue = HashCheckQueue::new();
    queue.insert(2, FLAG_NEED);
    let old = queue.pop_next_job().unwrap();
    queue.remove(2);

    // Re-enqueue before the old result arrives: this is a fresh job.
    assert!(queue.insert(2, FLAG_NEED));
    let new = queue.pop_next_job().unwrap();
    assert_ne!(old.token, new.token);

    assert!(!queue.complete_token(2, old.token));
    // The stale reply must not remove the newer in-flight job.
    assert!(queue.is_in_flight(2));
    assert!(queue.complete_token(2, new.token));
    assert_eq!(queue.stats().completed, 1);
    assert_eq!(queue.stats().stale_results, 1);
}

#[test]
fn queued_removal_updates_depths_and_counters() {
    let mut queue = HashCheckQueue::new();
    queue.insert(1, FLAG_NEED);
    queue.insert(2, FLAG_WILL_NEED);

    assert_eq!(queue.need_size(), 1);
    assert_eq!(queue.will_need_size(), 1);
    assert!(queue.remove(2));
    assert!(!queue.remove(2));
    assert_eq!(queue.size(), 1);
    assert_eq!(queue.will_need_size(), 0);

    queue.remove(1);
    assert_eq!(queue.size(), 0);
    assert_eq!(queue.pop_next(), None);
    assert_eq!(queue.stats().removed, 2);
}

#[test]
fn fairness_limit_prevents_will_need_starvation() {
    let mut queue = HashCheckQueue::new();
    queue.set_starvation_limit(Some(3));

    queue.insert(1000, FLAG_WILL_NEED);
    for index in 0..10 {
        queue.insert(index, FLAG_NEED);
    }

    let mut order = Vec::new();
    while let Some(index) = queue.pop_next() {
        order.push(index);
    }

    let will_position = order.iter().position(|&i| i == 1000).unwrap();
    assert!(
        will_position <= 3,
        "will-need chunk must be served after the fairness burst, got {order:?}"
    );
    // Every need chunk is still served exactly once.
    for index in 0..10 {
        assert_eq!(order.iter().filter(|&&i| i == index).count(), 1);
    }
}

#[test]
fn default_limit_is_configured_and_need_still_wins_first() {
    let mut queue = HashCheckQueue::new();
    assert_eq!(queue.starvation_limit(), Some(DEFAULT_STARVATION_LIMIT));

    for index in 0..10 {
        queue.insert(index, FLAG_WILL_NEED);
    }
    queue.insert(5000, FLAG_NEED);

    assert_eq!(queue.pop_next(), Some(5000));
}

#[test]
fn fairness_streak_resets_after_the_queue_drains() {
    let mut queue = HashCheckQueue::new();
    queue.set_starvation_limit(Some(2));

    queue.insert(1, FLAG_NEED);
    queue.insert(2, FLAG_NEED);
    assert_eq!(queue.pop_next(), Some(1));
    assert_eq!(queue.pop_next(), Some(2));
    assert_eq!(queue.pop_next(), None);

    // The previous burst must not make a freshly queued will-need chunk
    // preempt a newly arrived need chunk.
    queue.insert(9, FLAG_WILL_NEED);
    queue.insert(3, FLAG_NEED);
    assert_eq!(queue.pop_next(), Some(3));
}

#[test]
fn clear_cancels_queued_and_in_flight_work() {
    let mut queue = HashCheckQueue::new();
    queue.insert(1, FLAG_NEED);
    queue.insert(2, FLAG_WILL_NEED);
    let job = queue.pop_next_job().unwrap();
    assert_eq!(job.index, 1);

    queue.clear();

    assert_eq!(queue.size(), 0);
    assert_eq!(queue.in_flight_size(), 0);
    assert_eq!(queue.pop_next(), None);
    assert_eq!(queue.stats().cancelled_in_flight, 1);
}

#[test]
fn requeue_preserves_priority_and_order() {
    let mut queue = HashCheckQueue::new();
    queue.set_starvation_limit(None);
    queue.insert(1, FLAG_NEED);
    queue.insert(2, FLAG_NEED);
    queue.insert(9, FLAG_WILL_NEED);

    let job = queue.pop_next_job().unwrap();
    assert_eq!(job.index, 1);
    assert!(queue.requeue(job));

    assert_eq!(queue.pop_next(), Some(1));
    assert_eq!(queue.pop_next(), Some(2));
    assert_eq!(queue.pop_next(), Some(9));
}
