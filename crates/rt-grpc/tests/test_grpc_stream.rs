//! Live gRPC service behaviour: server-streaming events/stats, bulk success
//! paths on real torrents, and Multicall. No security is installed, so the TCP
//! peer is untrusted; the registered command is UNTRUSTED_SAFE.

use rt_core::{Download, DownloadList, SessionManager, ViewManager};
use rt_grpc::command_service_client::CommandServiceClient;
use rt_grpc::system_service_client::SystemServiceClient;
use rt_grpc::torrent_service_client::TorrentServiceClient;
use rt_grpc::{
    object_value, BulkControlTorrentsRequest, BulkRemoveTorrentsRequest,
    BulkSetTorrentPriorityRequest, CommandRequest, GrpcContext, GrpcServer, ListTorrentsRequest,
    MulticallRequest, ObjectValue, StreamEventsRequest, StreamStatsRequest, TorrentAction,
};
use rt_rpc::command::flags::{FLAG_PUBLIC_RPC, FLAG_UNTRUSTED_SAFE};
use rt_rpc::CommandMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;
use tonic::transport::Channel;

const HASH_A: &str = "5152535455565758595a5b5c5d5e5f6061626364";
const HASH_B: &str = "6162636465666768696a6b6c6d6e6f7071727374";

struct TestServer {
    channel: Channel,
    download_list: Arc<RwLock<DownloadList>>,
    shutdown_tx: watch::Sender<bool>,
    handle: JoinHandle<()>,
}

impl TestServer {
    async fn shutdown(self) {
        let _ = self.shutdown_tx.send(true);
        // Abort: infinite server-streams would block tonic's graceful drain.
        self.handle.abort();
        let _ = self.handle.await;
    }
}

async fn spawn_server() -> TestServer {
    let mut command_map = CommandMap::new();
    command_map
        .insert_func(
            "test.echo",
            |_target, obj| Ok(obj.clone()),
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "",
            "Echo argument back",
        )
        .unwrap();

    let command_map = Arc::new(RwLock::new(command_map));
    let download_list = Arc::new(RwLock::new(DownloadList::new()));
    let view_manager = Arc::new(RwLock::new(ViewManager::new()));
    let session_manager = Arc::new(Mutex::new(SessionManager::new()));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let ctx = GrpcContext::new(
        command_map,
        Arc::clone(&download_list),
        view_manager,
        session_manager,
        shutdown_tx.clone(),
    );

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    let server = GrpcServer::new(ctx);
    let handle = tokio::spawn(async move {
        let _ = server.run_tcp(addr, shutdown_rx).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    let channel = Channel::from_shared(format!("http://127.0.0.1:{port}"))
        .unwrap()
        .connect()
        .await
        .expect("connect to gRPC server");

    TestServer {
        channel,
        download_list,
        shutdown_tx,
        handle,
    }
}

fn make_download(name: &str, hex: &str) -> Arc<RwLock<Download>> {
    use lt_common::hash_string::HashString;
    use lt_common::path::Path as TorrentPath;
    use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};

    let mut file_path = TorrentPath::new();
    file_path.push("payload.bin");
    let metadata = TorrentMetadata {
        info_hash: HashString::from_hex(hex).unwrap(),
        name: name.to_string(),
        piece_length: 16384,
        pieces: vec![0u8; 20],
        num_pieces: 1,
        total_size: 16384,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path: file_path,
            length: 16384,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
        v2: None,
    };
    Arc::new(RwLock::new(Download::from_metadata(metadata)))
}

fn string_value(s: &str) -> ObjectValue {
    ObjectValue {
        kind: Some(object_value::Kind::StringValue(s.to_string())),
    }
}

#[tokio::test]
async fn stream_events_emits_lifecycle_on_insert() {
    let server = spawn_server().await;
    let mut torrent = TorrentServiceClient::new(server.channel.clone());

    let mut stream = torrent
        .stream_events(StreamEventsRequest {
            event_filter: Vec::new(),
        })
        .await
        .unwrap()
        .into_inner();

    let dl = make_download("streamed", HASH_A);
    assert!(server.download_list.write().unwrap().insert(dl));

    let event = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("event stream timed out")
        .expect("event stream ended")
        .expect("event stream error");

    assert_eq!(event.event_name, "event.download.inserted");
    assert_eq!(event.hash, HASH_A);
    assert_eq!(event.summary.unwrap().name, "streamed");
    assert!(event.timestamp > 0);

    server.shutdown().await;
}

#[tokio::test]
async fn stream_events_filter_excludes_unmatched() {
    let server = spawn_server().await;
    let mut torrent = TorrentServiceClient::new(server.channel.clone());

    let mut stream = torrent
        .stream_events(StreamEventsRequest {
            event_filter: vec!["event.download.finished".to_string()],
        })
        .await
        .unwrap()
        .into_inner();

    // Inserting emits `event.download.inserted`, which the filter excludes.
    let dl = make_download("filtered", HASH_A);
    assert!(server.download_list.write().unwrap().insert(dl));

    let nothing = tokio::time::timeout(Duration::from_millis(300), stream.next()).await;
    assert!(
        nothing.is_err(),
        "a filtered-out event must not be delivered"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn stream_stats_reports_torrent_totals() {
    let server = spawn_server().await;
    let mut system = SystemServiceClient::new(server.channel.clone());

    let dl = make_download("counted", HASH_A);
    assert!(server.download_list.write().unwrap().insert(dl));

    let mut stream = system
        .stream_stats(StreamStatsRequest { interval_ms: 50 })
        .await
        .unwrap()
        .into_inner();

    for _ in 0..2 {
        let stats = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("stats stream timed out")
            .expect("stats stream ended")
            .expect("stats stream error");
        assert_eq!(stats.total_torrents, 1);
    }

    server.shutdown().await;
}

#[tokio::test]
async fn bulk_operations_succeed_on_real_torrents() {
    let server = spawn_server().await;
    {
        let mut list = server.download_list.write().unwrap();
        assert!(list.insert(make_download("bulk-one", HASH_A)));
        assert!(list.insert(make_download("bulk-two", HASH_B)));
    }
    let mut torrent = TorrentServiceClient::new(server.channel.clone());

    let ctrl = torrent
        .bulk_control_torrents(BulkControlTorrentsRequest {
            hashes: vec![HASH_A.to_string(), HASH_B.to_string()],
            action: TorrentAction::Stop as i32,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(ctrl.success_count, 2);
    assert_eq!(ctrl.failure_count, 0);
    assert_eq!(ctrl.results.len(), 2);

    let prio = torrent
        .bulk_set_torrent_priority(BulkSetTorrentPriorityRequest {
            hashes: vec![HASH_A.to_string(), HASH_B.to_string()],
            priority: 3,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(prio.success_count, 2);
    assert_eq!(prio.failure_count, 0);

    let listed = torrent
        .list_torrents(ListTorrentsRequest {
            view: String::new(),
            hashes: Vec::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(listed.torrents.len(), 2);
    assert!(listed.torrents.iter().all(|t| t.priority == 3));

    let rem = torrent
        .bulk_remove_torrents(BulkRemoveTorrentsRequest {
            hashes: vec![HASH_A.to_string(), HASH_B.to_string()],
            delete_data: false,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(rem.success_count, 2);
    assert_eq!(rem.failure_count, 0);
    assert_eq!(server.download_list.read().unwrap().len(), 0);

    server.shutdown().await;
}

#[tokio::test]
async fn multicall_returns_per_call_results() {
    let server = spawn_server().await;
    let mut command = CommandServiceClient::new(server.channel.clone());

    let resp = command
        .multicall(MulticallRequest {
            calls: vec![
                CommandRequest {
                    command: "test.echo".to_string(),
                    args: vec![string_value("one")],
                    target_hash: String::new(),
                    trusted: true,
                },
                CommandRequest {
                    command: "test.echo".to_string(),
                    args: vec![string_value("two")],
                    target_hash: String::new(),
                    trusted: true,
                },
                CommandRequest {
                    command: "test.does_not_exist".to_string(),
                    args: Vec::new(),
                    target_hash: String::new(),
                    trusted: true,
                },
            ],
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(resp.results.len(), 3);
    let mut results = resp.results.into_iter();
    let first = results.next().unwrap();
    let second = results.next().unwrap();
    let third = results.next().unwrap();

    assert!(first.success);
    assert!(second.success);
    assert!(!third.success);
    assert!(!third.error_message.is_empty());

    match first.result.unwrap().kind.unwrap() {
        object_value::Kind::StringValue(s) => assert_eq!(s, "one"),
        other => panic!("unexpected first result: {other:?}"),
    }
    match second.result.unwrap().kind.unwrap() {
        object_value::Kind::StringValue(s) => assert_eq!(s, "two"),
        other => panic!("unexpected second result: {other:?}"),
    }

    server.shutdown().await;
}
