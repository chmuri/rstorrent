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

    // 5. Test SystemService::Shutdown
    let shutdown_resp = system_client
        .shutdown(rt_grpc::ShutdownRequest { force: false })
        .await
        .expect("Shutdown failed")
        .into_inner();

    assert!(shutdown_resp.acknowledged);

    let _ = server_handle.await;
}
