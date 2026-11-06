use rt_core::session::SessionManager;
use rt_core::{DownloadList, ViewManager};
use rt_grpc::command_service_client::CommandServiceClient;
use rt_grpc::system_service_client::SystemServiceClient;
use rt_grpc::torrent_service_client::TorrentServiceClient;
use rt_grpc::{
    object_value, CommandRequest, Empty, GrpcContext, GrpcServer, ListTorrentsRequest, ObjectValue,
};
use rt_rpc::command::flags::{FLAG_PUBLIC_RPC, FLAG_UNTRUSTED_SAFE};
use rt_rpc::CommandMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::watch;

#[tokio::test]
async fn test_grpc_command_and_system_service() {
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
        Arc::clone(&command_map),
        Arc::clone(&download_list),
        Arc::clone(&view_manager),
        Arc::clone(&session_manager),
        shutdown_tx,
    );

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    let server = GrpcServer::new(ctx);

    let server_handle = tokio::spawn(async move {
        let _ = server.run_tcp(addr, shutdown_rx).await;
    });

    // Wait for server to bind
    tokio::time::sleep(Duration::from_millis(100)).await;

    let channel = tonic::transport::Channel::from_shared(format!("http://127.0.0.1:{}", port))
        .unwrap()
        .connect()
        .await
        .expect("Failed to connect to gRPC server");

    // 1. Test SystemService::GetVersion
    let mut system_client = SystemServiceClient::new(channel.clone());
    let ver_resp = system_client
        .get_version(Empty {})
        .await
        .expect("GetVersion failed")
        .into_inner();

    assert!(!ver_resp.version.is_empty());
    assert_eq!(ver_resp.api_version, lt_common::version::VERSION);

    // 2. Test CommandService::Execute
    let mut command_client = CommandServiceClient::new(channel.clone());
    let req = CommandRequest {
        command: "test.echo".to_string(),
        args: vec![ObjectValue {
            kind: Some(object_value::Kind::StringValue("hello-grpc".to_string())),
        }],
        target_hash: String::new(),
        trusted: true,
    };

    let resp = command_client
        .execute(req)
        .await
        .expect("Execute failed")
        .into_inner();

    assert!(resp.success);
    match resp.result.unwrap().kind.unwrap() {
        object_value::Kind::StringValue(s) => assert_eq!(s, "hello-grpc"),
        other => panic!("Unexpected result: {:?}", other),
    }

    // 3. Test CommandService::ListCommands
    let list_resp = command_client
        .list_commands(Empty {})
        .await
        .expect("ListCommands failed")
        .into_inner();

    assert!(list_resp.commands.contains(&"test.echo".to_string()));

    // 4. Test TorrentService::ListTorrents
    let mut torrent_client = TorrentServiceClient::new(channel.clone());
    let torrents_resp = torrent_client
        .list_torrents(ListTorrentsRequest {
            view: String::new(),
            hashes: Vec::new(),
        })
        .await
        .expect("ListTorrents failed")
        .into_inner();

    assert_eq!(torrents_resp.torrents.len(), 0);

    // 5. Test TorrentService bulk operations
    let bulk_ctrl_resp = torrent_client
        .bulk_control_torrents(rt_grpc::BulkControlTorrentsRequest {
            hashes: vec!["abc".to_string(), "def".to_string()],
            action: rt_grpc::TorrentAction::Start as i32,
        })
        .await
        .expect("BulkControlTorrents failed")
        .into_inner();
    assert_eq!(bulk_ctrl_resp.failure_count, 2);

    let bulk_prio_resp = torrent_client
        .bulk_set_torrent_priority(rt_grpc::BulkSetTorrentPriorityRequest {
            hashes: vec!["abc".to_string()],
            priority: 3,
        })
        .await
        .expect("BulkSetTorrentPriority failed")
        .into_inner();
    assert_eq!(bulk_prio_resp.failure_count, 1);

    let bulk_rem_resp = torrent_client
        .bulk_remove_torrents(rt_grpc::BulkRemoveTorrentsRequest {
            hashes: vec!["abc".to_string()],
            delete_data: false,
        })
        .await
        .expect("BulkRemoveTorrents failed")
        .into_inner();
    assert_eq!(bulk_rem_resp.failure_count, 1);

    // 6. Test SystemService::Shutdown
    let shutdown_resp = system_client
        .shutdown(rt_grpc::ShutdownRequest { force: false })
        .await
        .expect("Shutdown failed")
        .into_inner();

    assert!(shutdown_resp.acknowledged);

    let _ = server_handle.await;
}

#[test]
fn summary_exposes_numbered_custom1_as_category() {
    use lt_common::hash_string::HashString;
    use lt_common::path::Path as TorrentPath;
    use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
    use rt_core::download::Download;

    let mut file_path = TorrentPath::new();
    file_path.push("payload.bin");

    let metadata = TorrentMetadata {
        info_hash: HashString::from_hex("5152535455565758595a5b5c5d5e5f6061626364").unwrap(),
        name: "category-summary".to_string(),
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
    };

    let mut download = Download::from_metadata(metadata);
    download.set_custom_field(1, "tv-sonarr");

    let summary = rt_grpc::conversion::download_to_summary(&download);

    assert_eq!(summary.category, "tv-sonarr");
    assert_eq!(summary.name, "category-summary");
}
