use std::net::TcpListener;
use std::time::Duration;
use rstorrent::cli::Cli;
use rstorrent::control::Control;
use rt_grpc::command_service_client::CommandServiceClient;
use rt_grpc::object_value;
use rt_grpc::system_service_client::SystemServiceClient;
use rt_grpc::{CommandRequest, Empty, ObjectValue};

fn get_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind ephemeral port");
    listener.local_addr().unwrap().port()
}

#[tokio::test]
async fn test_grpc_daemon_integration() {
    let port = get_free_port();

    let mut control = Control::new();
    let download_directory = "/tmp/rstorrent-grpc-integration-downloads".to_string();
    let cli = Cli {
        daemon: true,
        no_rc: true,
        grpc_port: Some(port),
        directory: Some(download_directory.clone()),
        ..Default::default()
    };

    let init = control.initialize(&cli);
    assert!(init.is_ok(), "initialize failed: {:?}", init.err());

    // Allow gRPC server task to bind
    tokio::time::sleep(Duration::from_millis(100)).await;

    let channel_url = format!("http://127.0.0.1:{}", port);

    // Test SystemService
    let mut system_client = SystemServiceClient::connect(channel_url.clone())
        .await
        .expect("Failed to connect to gRPC SystemService");

    let version_resp = system_client
        .get_version(Empty {})
        .await
        .expect("GetVersion failed")
        .into_inner();

    assert!(version_resp.version.contains("0.10.0"));
    assert_eq!(version_resp.api_version, "0.10.0");

    // Test CommandService
    let mut command_client = CommandServiceClient::connect(channel_url)
        .await
        .expect("Failed to connect to gRPC CommandService");

    let list_resp = command_client
        .list_commands(Empty {})
        .await
        .expect("ListCommands failed")
        .into_inner();

    assert!(!list_resp.commands.is_empty());
    assert!(list_resp.commands.iter().any(|cmd| cmd == "network.grpc.open_port"));

    // Flood compatibility surface: expressions, sentinels and download getters.
    for command in [
        "cat",
        "true",
        "false",
        "d.down.sequential",
        "d.down.sequential.set",
        "d.creation_date",
        "d.timestamp.last_active",
    ] {
        assert!(list_resp.commands.iter().any(|cmd| cmd == command), "missing {command}");
    }

    let string_arg = |value: &str| ObjectValue {
        kind: Some(object_value::Kind::StringValue(value.to_string())),
    };

    // `-d/--directory` must reach `directory.default` (used by Flood as the
    // default add destination).
    let default_dir = command_client
        .execute(CommandRequest {
            command: "directory.default".to_string(),
            args: vec![],
            target_hash: String::new(),
            trusted: true,
        })
        .await
        .expect("directory.default failed")
        .into_inner();
    let default_dir = match default_dir.result.and_then(|value| value.kind) {
        Some(object_value::Kind::StringValue(value)) => value,
        other => panic!("unexpected directory.default result: {other:?}"),
    };
    assert!(
        default_dir.starts_with(&download_directory),
        "directory.default = {default_dir}"
    );

    let magnet_uri = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Magnet%20Test";
    let add = command_client
        .execute(CommandRequest {
            command: "load.start_throw".to_string(),
            args: vec![string_arg(magnet_uri)],
            target_hash: String::new(),
            trusted: true,
        })
        .await
        .expect("magnet add failed")
        .into_inner();

    assert!(add.success, "magnet add failed: {}", add.error_message);

    // The magnet placeholder must be visible immediately, before metadata arrives.
    let list = command_client
        .execute(CommandRequest {
            command: "d.multicall2".to_string(),
            args: vec![
                string_arg("main"),
                string_arg("d.hash="),
                string_arg("d.name="),
                string_arg("d.size_bytes="),
            ],
            target_hash: String::new(),
            trusted: true,
        })
        .await
        .expect("d.multicall2 failed")
        .into_inner();

    let rows = match list.result.and_then(|value| value.kind) {
        Some(object_value::Kind::ListValue(list)) => list.values,
        _ => panic!("multicall did not return a list"),
    };
    fn output_string(value: &ObjectValue) -> Option<&str> {
        match &value.kind {
            Some(object_value::Kind::StringValue(text)) => Some(text.as_str()),
            _ => None,
        }
    }
    let output_int = |value: &ObjectValue| match &value.kind {
        Some(object_value::Kind::IntValue(number)) => Some(*number),
        _ => None,
    };

    let magnet_row = rows
        .iter()
        .find_map(|row| {
            let values = match &row.kind {
                Some(object_value::Kind::ListValue(list)) => list.values.clone(),
                _ => return None,
            };
            if output_string(&values[1]) == Some("Magnet Test") {
                Some(values)
            } else {
                None
            }
        })
        .expect("magnet download not listed");

    assert_eq!(
        output_string(&magnet_row[0]),
        Some("0123456789abcdef0123456789abcdef01234567")
    );

    // Commands backed by the new fields.
    let hash_target = "0123456789abcdef0123456789abcdef01234567".to_string();
    let sequential = command_client
        .execute(CommandRequest {
            command: "d.down.sequential.set".to_string(),
            args: vec![ObjectValue {
                kind: Some(object_value::Kind::IntValue(1)),
            }],
            target_hash: hash_target.clone(),
            trusted: true,
        })
        .await
        .expect("sequential set failed")
        .into_inner();
    assert!(sequential.success, "sequential set failed: {}", sequential.error_message);

    let sequential_get = command_client
        .execute(CommandRequest {
            command: "d.down.sequential".to_string(),
            args: vec![],
            target_hash: hash_target.clone(),
            trusted: true,
        })
        .await
        .expect("sequential get failed")
        .into_inner();
    assert_eq!(
        sequential_get.result.as_ref().and_then(&output_int),
        Some(1)
    );

    // Erasing must also drop the view entry (no phantom rows in d.multicall2).
    let erase = command_client
        .execute(CommandRequest {
            command: "d.erase".to_string(),
            args: vec![],
            target_hash: hash_target.clone(),
            trusted: true,
        })
        .await
        .expect("erase failed")
        .into_inner();
    assert!(erase.success, "erase failed: {}", erase.error_message);

    let after_erase = command_client
        .execute(CommandRequest {
            command: "d.multicall2".to_string(),
            args: vec![string_arg("main"), string_arg("d.hash=")],
            target_hash: String::new(),
            trusted: true,
        })
        .await
        .expect("d.multicall2 after erase failed")
        .into_inner();
    let remaining = match after_erase.result.and_then(|value| value.kind) {
        Some(object_value::Kind::ListValue(list)) => list.values,
        _ => panic!("multicall did not return a list"),
    };
    assert!(remaining.is_empty(), "erased download still listed: {}", remaining.len());

    let false_cmd = command_client
        .execute(CommandRequest {
            command: "false".to_string(),
            args: vec![],
            target_hash: String::new(),
            trusted: true,
        })
        .await
        .expect("false failed")
        .into_inner();
    assert_eq!(false_cmd.result.as_ref().and_then(&output_int), Some(0));

    // Cleanup
    let _ = control.shutdown().await;
}
