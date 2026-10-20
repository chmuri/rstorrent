use rstorrent::cli::Cli;
use rstorrent::control::Control;
use std::io::Write;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

fn encode_scgi_frame(headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut header_data = Vec::new();
    for (key, val) in headers {
        header_data.extend_from_slice(key.as_bytes());
        header_data.push(0);
        header_data.extend_from_slice(val.as_bytes());
        header_data.push(0);
    }
    let mut frame = Vec::new();
    write!(frame, "{}:", header_data.len()).unwrap();
    frame.extend_from_slice(&header_data);
    frame.push(b',');
    frame.extend_from_slice(body);
    frame
}

/// Read exactly one SCGI response, using Content-Length so that pipelined
/// responses can be framed without waiting for the connection to close.
async fn read_response<S>(stream: &mut S) -> Vec<u8>
where
    S: AsyncRead + Unpin,
{
    let mut header = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).await.unwrap();
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
    }

    let header_str = String::from_utf8_lossy(&header);
    let content_length = header_str
        .lines()
        .find_map(|line| {
            line.strip_prefix("Content-Length:")
                .map(|value| value.trim().parse::<usize>().unwrap())
        })
        .expect("response is missing Content-Length");

    let mut body = vec![0u8; content_length];
    stream.read_exact(&mut body).await.unwrap();

    header.extend_from_slice(&body);
    header
}

#[tokio::test]
async fn test_scgi_socket_flood_xmlrpc() {
    let sock_path = format!("/tmp/test_rtorrent_{}.sock", std::process::id());
    let session_dir = format!(
        "/tmp/test_rtorrent_scgi_session_flood_{}",
        std::process::id()
    );
    let _ = std::fs::remove_file(&sock_path);
    let _ = std::fs::remove_dir_all(&session_dir);
    std::fs::create_dir_all(&session_dir).unwrap();

    let mut control = Control::new();
    let cli = Cli {
        bind_address: None,
        directory: None,
        local_address: None,
        port_range: None,
        session: Some(session_dir.clone()),
        no_rc: true,
        config_file: None,
        daemon: true,
        grpc_port: None,
        grpc_socket: None,
        options: Vec::new(),
        command_options: vec![format!("network.scgi.open_local = {}", sock_path)],
        deprecated: false,
        disable_intermediate: false,
        intermediate_no_xmlrpc: false,
        files: Vec::new(),
    };

    assert!(control.initialize(&cli).is_ok());

    // Give the listener 50ms to spawn and bind
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Connect to the SCGI Unix socket
    let mut stream = UnixStream::connect(&sock_path)
        .await
        .expect("Failed to connect to SCGI socket");

    // Flood sends two pipelined XML-RPC methodCalls on the same connection.
    let xml_payload = r#"<?xml version="1.0"?><methodCall><methodName>system.client_version</methodName><params></params></methodCall>"#;
    let frame = encode_scgi_frame(
        &[
            ("CONTENT_LENGTH", &xml_payload.len().to_string()),
            ("SCGI", "1"),
        ],
        xml_payload.as_bytes(),
    );

    stream.write_all(&frame).await.unwrap();
    stream.write_all(&frame).await.unwrap();

    // Both responses must arrive, in order, on the same connection.
    for _ in 0..2 {
        let response = read_response(&mut stream).await;
        let resp_str = String::from_utf8_lossy(&response);
        assert!(resp_str.contains("Status: 200 OK"));
        assert!(resp_str.contains("Content-Type: text/xml"));
        assert!(resp_str.contains("<methodResponse>"));
        assert!(resp_str.contains("<string>0.10.0</string>"));
    }

    // A normal close after the pipelined requests must be clean.
    stream.shutdown().await.unwrap();
    let mut remaining = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut remaining))
        .await
        .expect("server did not close the connection")
        .unwrap();
    assert_eq!(read, 0);
    assert!(remaining.is_empty());

    let _ = std::fs::remove_file(&sock_path);
    control.shutdown().await;
    let _ = std::fs::remove_dir_all(&session_dir);
}

#[tokio::test]
async fn test_scgi_socket_load_raw_start() {
    let sock_path = format!("/tmp/test_rtorrent_load_{}.sock", std::process::id());
    let session_dir = format!(
        "/tmp/test_rtorrent_scgi_session_load_{}",
        std::process::id()
    );
    let _ = std::fs::remove_file(&sock_path);
    let _ = std::fs::remove_dir_all(&session_dir);
    std::fs::create_dir_all(&session_dir).unwrap();

    let mut control = Control::new();
    let cli = Cli {
        bind_address: None,
        directory: None,
        local_address: None,
        port_range: None,
        session: Some(session_dir.clone()),
        no_rc: true,
        config_file: None,
        daemon: true,
        grpc_port: None,
        grpc_socket: None,
        options: Vec::new(),
        command_options: vec![format!("network.scgi.open_local = {}", sock_path)],
        deprecated: false,
        disable_intermediate: false,
        intermediate_no_xmlrpc: false,
        files: Vec::new(),
    };

    assert!(control.initialize(&cli).is_ok());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut stream = UnixStream::connect(&sock_path)
        .await
        .expect("Failed to connect to SCGI socket");

    let bencoded_torrent = {
        let mut info = lt_common::Object::create_map();
        info.insert_key("name", lt_common::Object::from("scgi_load_torrent"))
            .unwrap();
        info.insert_key("length", lt_common::Object::from(1024i64))
            .unwrap();
        info.insert_key("piece length", lt_common::Object::from(262144i64))
            .unwrap();
        info.insert_key("pieces", lt_common::Object::from(vec![0xAAu8; 20]))
            .unwrap();

        let mut object = lt_common::Object::create_map();
        object.insert_key("info", info).unwrap();
        lt_common::object::bencode::encode(&object).unwrap()
    };

    let b64 = lt_common::encoding::encode_base64(&bencoded_torrent);

    // Mimic Flood sending load.raw_start with base64 payload and d.directory.set
    let xml_payload = format!(
        r#"<?xml version="1.0"?><methodCall><methodName>load.raw_start</methodName><params><param><value><string></string></value></param><param><value><base64>{}</base64></value></param><param><value><string>d.directory.set="/tmp/scgi_dir"</string></value></param></params></methodCall>"#,
        b64
    );

    let frame = encode_scgi_frame(
        &[
            ("CONTENT_LENGTH", &xml_payload.len().to_string()),
            ("SCGI", "1"),
        ],
        xml_payload.as_bytes(),
    );

    stream.write_all(&frame).await.unwrap();

    let response = read_response(&mut stream).await;
    let resp_str = String::from_utf8_lossy(&response);
    assert!(resp_str.contains("Status: 200 OK"));
    assert!(resp_str.contains("<methodResponse>"));
    assert!(!resp_str.contains("<fault>"));

    stream.shutdown().await.unwrap();
    let _ = std::fs::remove_file(&sock_path);
    control.shutdown().await;
    let _ = std::fs::remove_dir_all(&session_dir);
}
