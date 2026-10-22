use flate2::read::GzDecoder;
use lt_common::Object;
use rt_rpc::command_flags::*;
use rt_rpc::{CommandMap, JsonRpc, XmlRpc};
use rt_scgi::{ScgiConfig, ScgiHandler, ScgiServer};
use std::io::Read;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixStream};
use tokio::time::timeout;

fn setup_commands() -> Arc<RwLock<CommandMap>> {
    let mut map = CommandMap::new();

    map.insert_func(
        "test_echo",
        |_target, obj| Ok(obj.clone()),
        FLAG_PUBLIC_RPC,
        "",
        "",
    )
    .unwrap();

    map.insert_func(
        "system.pid",
        |_target, _obj| Ok(Object::from(12345i64)),
        FLAG_PUBLIC_RPC,
        "",
        "",
    )
    .unwrap();

    map.insert_func(
        "system.untrusted_only",
        |_target, _obj| Ok(Object::from("private")),
        0, // not untrusted safe
        "",
        "",
    )
    .unwrap();

    map.insert_func(
        "large_data",
        |_target, _obj| Ok(Object::from("a".repeat(2048))),
        FLAG_PUBLIC_RPC,
        "",
        "",
    )
    .unwrap();

    Arc::new(RwLock::new(map))
}

fn make_handler(config: ScgiConfig) -> Arc<ScgiHandler> {
    Arc::new(ScgiHandler::new(
        setup_commands(),
        Arc::new(XmlRpc::new()),
        Arc::new(JsonRpc::new()),
        config,
    ))
}

fn encode_scgi_frame(headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut header_data = Vec::new();
    for (k, v) in headers {
        header_data.extend_from_slice(k.as_bytes());
        header_data.push(0);
        header_data.extend_from_slice(v.as_bytes());
        header_data.push(0);
    }

    let mut frame = Vec::new();
    let len_str = format!("{}:", header_data.len());
    frame.extend_from_slice(len_str.as_bytes());
    frame.extend_from_slice(&header_data);
    frame.push(b',');
    frame.extend_from_slice(body);
    frame
}

fn json_request(id: u64) -> Vec<u8> {
    let body = format!(
        r#"{{"jsonrpc":"2.0","method":"system.pid","params":[],"id":{}}}"#,
        id
    );
    encode_scgi_frame(
        &[
            ("CONTENT_LENGTH", &body.len().to_string()),
            ("SCGI", "1"),
            ("CONTENT_TYPE", "application/json"),
        ],
        body.as_bytes(),
    )
}

/// Read exactly one SCGI response, using Content-Length to find its end so
/// that pipelined responses do not bleed into each other.
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

async fn expect_eof<S>(stream: &mut S)
where
    S: AsyncRead + Unpin,
{
    let mut rest = Vec::new();
    let read = timeout(Duration::from_secs(2), stream.read_to_end(&mut rest))
        .await
        .expect("connection did not close in time")
        .unwrap();
    assert_eq!(read, 0, "unexpected trailing bytes: {:?}", rest);
}

#[tokio::test]
async fn test_scgi_tcp_xmlrpc_and_jsonrpc() {
    let handler = make_handler(ScgiConfig::default());

    // Bind on random local port
    let server = ScgiServer::bind_tcp("127.0.0.1:0", Arc::clone(&handler))
        .await
        .unwrap();
    // Retrieve actual bound addr
    let port = {
        // Let's spawn server task
        let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let listener = match server {
                rt_scgi::ScgiServer {
                    listener: rt_scgi::ScgiListener::Tcp(l),
                    ..
                } => l,
                _ => unreachable!(),
            };
            let addr = listener.local_addr().unwrap();
            addr_tx.send(addr).unwrap();
            loop {
                if let Ok((stream, _)) = listener.accept().await {
                    let h = Arc::clone(&handler);
                    tokio::spawn(async move {
                        let _ = h.handle_stream(stream).await;
                    });
                }
            }
        });
        addr_rx.await.unwrap()
    };

    // 1. XML-RPC over TCP
    let xml_body = r#"<?xml version="1.0"?><methodCall><methodName>system.pid</methodName><params></params></methodCall>"#;
    let xml_req = encode_scgi_frame(
        &[
            ("CONTENT_LENGTH", &xml_body.len().to_string()),
            ("SCGI", "1"),
            ("CONTENT_TYPE", "text/xml"),
        ],
        xml_body.as_bytes(),
    );

    let mut stream = TcpStream::connect(port).await.unwrap();
    stream.write_all(&xml_req).await.unwrap();
    // Legacy clients close their write side after sending a single request.
    stream.shutdown().await.unwrap();

    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).await.unwrap();
    let resp_str = String::from_utf8_lossy(&resp);

    assert!(resp_str.starts_with("Status: 200 OK\r\n"));
    assert!(resp_str.contains("Content-Type: text/xml"));
    assert!(resp_str.contains("<i8>12345</i8>"));

    // 2. JSON-RPC over TCP
    let json_body = r#"{"jsonrpc":"2.0","method":"system.pid","params":[],"id":1}"#;
    let json_req = encode_scgi_frame(
        &[
            ("CONTENT_LENGTH", &json_body.len().to_string()),
            ("SCGI", "1"),
            ("CONTENT_TYPE", "application/json"),
        ],
        json_body.as_bytes(),
    );

    let mut stream = TcpStream::connect(port).await.unwrap();
    stream.write_all(&json_req).await.unwrap();
    stream.shutdown().await.unwrap();

    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).await.unwrap();
    let resp_str = String::from_utf8_lossy(&resp);

    assert!(resp_str.starts_with("Status: 200 OK\r\n"));
    assert!(resp_str.contains("Content-Type: application/json"));
    assert!(resp_str.contains(r#""result":12345"#));
}

#[tokio::test]
async fn test_scgi_unix_socket() {
    let handler = make_handler(ScgiConfig::default());

    let socket_path =
        std::env::temp_dir().join(format!("rtorrent_test_{}.sock", std::process::id()));
    let server = ScgiServer::bind_unix(&socket_path, Arc::clone(&handler))
        .await
        .unwrap();

    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let sock_clone = socket_path.clone();

    tokio::spawn(async move {
        let listener = match server {
            rt_scgi::ScgiServer {
                listener: rt_scgi::ScgiListener::Unix(l, _),
                ..
            } => l,
            _ => unreachable!(),
        };
        ready_tx.send(()).unwrap();
        if let Ok((stream, _)) = listener.accept().await {
            let _ = handler.handle_stream(stream).await;
        }
    });

    ready_rx.await.unwrap();

    let json_body = r#"{"jsonrpc":"2.0","method":"system.pid","params":[],"id":42}"#;
    let json_req = encode_scgi_frame(
        &[
            ("CONTENT_LENGTH", &json_body.len().to_string()),
            ("SCGI", "1"),
        ],
        json_body.as_bytes(),
    );

    let mut stream = UnixStream::connect(&sock_clone).await.unwrap();
    stream.write_all(&json_req).await.unwrap();
    stream.shutdown().await.unwrap();

    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).await.unwrap();
    let resp_str = String::from_utf8_lossy(&resp);

    assert!(resp_str.starts_with("Status: 200 OK\r\n"));
    assert!(resp_str.contains("Content-Type: application/json"));
    assert!(resp_str.contains(r#""id":42"#));
    assert!(resp_str.contains(r#""result":12345"#));

    let _ = std::fs::remove_file(&sock_clone);
}

#[tokio::test]
async fn test_scgi_gzip_and_untrusted() {
    let handler = make_handler(ScgiConfig {
        min_compress_size: 50,
        ..Default::default()
    });

    // 1. Gzip test
    let json_body = r#"{"jsonrpc":"2.0","method":"large_data","params":[],"id":99}"#;
    let req = encode_scgi_frame(
        &[
            ("CONTENT_LENGTH", &json_body.len().to_string()),
            ("SCGI", "1"),
            ("ACCEPT_ENCODING", "gzip, deflate"),
        ],
        json_body.as_bytes(),
    );

    let (client_stream, server_stream) = tokio::io::duplex(8192);
    let (mut client_read, mut client_write) = tokio::io::split(client_stream);

    let h = Arc::clone(&handler);
    tokio::spawn(async move {
        let _ = h.handle_stream(server_stream).await;
    });

    client_write.write_all(&req).await.unwrap();
    client_write.shutdown().await.unwrap();

    let mut resp = Vec::new();
    client_read.read_to_end(&mut resp).await.unwrap();

    // Check header
    let header_end = resp.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let header_str = String::from_utf8_lossy(&resp[..header_end]);
    assert!(header_str.contains("Content-Encoding: gzip"));

    let body_compressed = &resp[header_end + 4..];
    let mut decoder = GzDecoder::new(body_compressed);
    let mut decompressed = String::new();
    decoder.read_to_string(&mut decompressed).unwrap();

    assert!(decompressed.contains(r#""id":99"#));
    assert!(decompressed.contains("aaaaa"));

    // 2. Untrusted connection test
    let untrusted_body = r#"{"jsonrpc":"2.0","method":"system.untrusted_only","params":[],"id":1}"#;
    let untrusted_req = encode_scgi_frame(
        &[
            ("CONTENT_LENGTH", &untrusted_body.len().to_string()),
            ("SCGI", "1"),
            ("UNTRUSTED_CONNECTION", "1"),
        ],
        untrusted_body.as_bytes(),
    );

    let (client_stream2, server_stream2) = tokio::io::duplex(8192);
    let (mut c2_read, mut c2_write) = tokio::io::split(client_stream2);
    let h2 = Arc::clone(&handler);
    tokio::spawn(async move {
        let _ = h2.handle_stream(server_stream2).await;
    });

    c2_write.write_all(&untrusted_req).await.unwrap();
    c2_write.shutdown().await.unwrap();

    let mut resp2 = Vec::new();
    c2_read.read_to_end(&mut resp2).await.unwrap();
    let resp2_str = String::from_utf8_lossy(&resp2);
    assert!(resp2_str.contains("not allowed for untrusted connections"));
}

#[tokio::test]
async fn test_scgi_pipelining_two_requests_in_order() {
    let handler = make_handler(ScgiConfig::default());
    let (mut client, server) = tokio::io::duplex(64 * 1024);

    let h = Arc::clone(&handler);
    let server_task = tokio::spawn(async move { h.handle_stream(server).await });

    // Both requests are written before either response is read. The handler
    // must process them in order and leave the pipelined bytes untouched.
    client.write_all(&json_request(11)).await.unwrap();
    client.write_all(&json_request(22)).await.unwrap();
    client.shutdown().await.unwrap();

    let first = String::from_utf8_lossy(&read_response(&mut client).await).to_string();
    let second = String::from_utf8_lossy(&read_response(&mut client).await).to_string();

    assert!(first.contains(r#""id":11"#), "first response: {first}");
    assert!(!first.contains(r#""id":22"#), "first response: {first}");
    assert!(second.contains(r#""id":22"#), "second response: {second}");
    assert!(!second.contains(r#""id":11"#), "second response: {second}");

    expect_eof(&mut client).await;
    server_task.await.unwrap().unwrap();

    let metrics = handler.metrics().snapshot();
    assert_eq!(metrics.connections_accepted, 1);
    assert_eq!(metrics.connections_closed, 1);
    assert_eq!(metrics.active_connections, 0);
    assert_eq!(metrics.requests_received, 2);
    assert_eq!(metrics.requests_completed, 2);
    assert_eq!(metrics.requests_failed, 0);
}

#[tokio::test]
async fn test_scgi_keep_alive_sequential_requests() {
    let handler = make_handler(ScgiConfig::default());
    let (mut client, server) = tokio::io::duplex(8192);

    let h = Arc::clone(&handler);
    let server_task = tokio::spawn(async move { h.handle_stream(server).await });

    client.write_all(&json_request(1)).await.unwrap();
    let first = String::from_utf8_lossy(&read_response(&mut client).await).to_string();
    assert!(first.contains(r#""id":1"#));

    // The connection is still open; send a second request on it.
    client.write_all(&json_request(2)).await.unwrap();
    let second = String::from_utf8_lossy(&read_response(&mut client).await).to_string();
    assert!(second.contains(r#""id":2"#));

    client.shutdown().await.unwrap();
    expect_eof(&mut client).await;
    server_task.await.unwrap().unwrap();

    let metrics = handler.metrics().snapshot();
    assert_eq!(metrics.requests_received, 2);
    assert_eq!(metrics.requests_completed, 2);
    assert_eq!(metrics.connections_accepted, 1);
}

#[tokio::test]
async fn test_scgi_idle_timeout_closes_connection() {
    let handler = make_handler(ScgiConfig {
        idle_timeout: Duration::from_millis(50),
        read_timeout: Duration::from_millis(500),
        ..Default::default()
    });
    let (mut client, server) = tokio::io::duplex(1024);

    let h = Arc::clone(&handler);
    let server_task = tokio::spawn(async move { h.handle_stream(server).await });

    let started = tokio::time::Instant::now();
    expect_eof(&mut client).await;
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_millis(30),
        "idle timeout fired too early: {elapsed:?}"
    );

    let result = server_task.await.unwrap();
    assert!(
        matches!(result, Err(rt_scgi::ScgiError::Timeout("idle"))),
        "unexpected result: {result:?}"
    );

    let metrics = handler.metrics().snapshot();
    assert_eq!(metrics.timeouts, 1);
    assert_eq!(metrics.requests_received, 0);
    assert_eq!(metrics.connections_closed, 1);
}

#[tokio::test]
async fn test_scgi_read_timeout_closes_half_read_request() {
    let handler = make_handler(ScgiConfig {
        idle_timeout: Duration::from_secs(1),
        read_timeout: Duration::from_millis(50),
        ..Default::default()
    });
    let (mut client, server) = tokio::io::duplex(1024);

    let h = Arc::clone(&handler);
    let server_task = tokio::spawn(async move { h.handle_stream(server).await });

    // A header length prefix that never terminates with ':'.
    client.write_all(b"12").await.unwrap();

    let started = tokio::time::Instant::now();
    expect_eof(&mut client).await;
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_millis(30),
        "read timeout fired too early: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "read timeout did not fire promptly: {elapsed:?}"
    );

    let result = server_task.await.unwrap();
    assert!(
        matches!(result, Err(rt_scgi::ScgiError::Timeout("header length"))),
        "unexpected result: {result:?}"
    );

    let metrics = handler.metrics().snapshot();
    assert_eq!(metrics.timeouts, 1);
    assert_eq!(metrics.requests_received, 0);
}

#[tokio::test]
async fn test_scgi_body_read_timeout_closes_half_read_body() {
    let handler = make_handler(ScgiConfig {
        idle_timeout: Duration::from_secs(1),
        read_timeout: Duration::from_millis(50),
        ..Default::default()
    });
    let (mut client, server) = tokio::io::duplex(1024);

    let h = Arc::clone(&handler);
    let server_task = tokio::spawn(async move { h.handle_stream(server).await });

    // Header declares a 10 byte body but only 3 bytes are sent before the
    // peer stalls. The handler must time out and close, not wait forever.
    let frame = encode_scgi_frame(&[("CONTENT_LENGTH", "10")], b"abc");
    client.write_all(&frame).await.unwrap();

    let started = tokio::time::Instant::now();
    expect_eof(&mut client).await;
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_millis(30),
        "read timeout fired too early: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "read timeout did not fire promptly: {elapsed:?}"
    );

    let result = server_task.await.unwrap();
    assert!(
        matches!(result, Err(rt_scgi::ScgiError::Timeout("body"))),
        "unexpected result: {result:?}"
    );

    let metrics = handler.metrics().snapshot();
    assert_eq!(metrics.timeouts, 1);
    assert_eq!(metrics.requests_received, 0);
}

#[tokio::test]
async fn test_scgi_oversized_frames_are_rejected() {
    let handler = make_handler(ScgiConfig {
        max_header_size: 32,
        max_content_size: 8,
        idle_timeout: Duration::from_millis(500),
        read_timeout: Duration::from_millis(500),
        ..Default::default()
    });

    // 1. Header length exceeds max_header_size.
    let (mut client, server) = tokio::io::duplex(1024);
    let h = Arc::clone(&handler);
    let server_task = tokio::spawn(async move { h.handle_stream(server).await });
    client.write_all(b"999:").await.unwrap();
    expect_eof(&mut client).await;
    let result = server_task.await.unwrap();
    assert!(
        matches!(result, Err(rt_scgi::ScgiError::HeaderTooLarge(999, 32))),
        "unexpected result: {result:?}"
    );

    // 2. CONTENT_LENGTH exceeds max_content_size. The server must reject the
    // frame without waiting for (or buffering) the declared body.
    let (mut client, server) = tokio::io::duplex(1024);
    let h = Arc::clone(&handler);
    let server_task = tokio::spawn(async move { h.handle_stream(server).await });
    let frame = encode_scgi_frame(&[("CONTENT_LENGTH", "100")], b"");
    client.write_all(&frame).await.unwrap();
    expect_eof(&mut client).await;
    let result = server_task.await.unwrap();
    assert!(
        matches!(result, Err(rt_scgi::ScgiError::ContentTooLarge(100, 8))),
        "unexpected result: {result:?}"
    );

    let metrics = handler.metrics().snapshot();
    assert_eq!(metrics.oversized_frames, 2);
    assert_eq!(metrics.requests_received, 0);
    assert_eq!(metrics.connections_accepted, 2);
    assert_eq!(metrics.connections_closed, 2);
    assert_eq!(metrics.active_connections, 0);
}

#[tokio::test]
async fn test_scgi_graceful_shutdown_closes_connections() {
    let handler = make_handler(ScgiConfig {
        shutdown_grace_period: Duration::from_millis(50),
        ..Default::default()
    });
    let server = ScgiServer::bind_tcp("127.0.0.1:0", Arc::clone(&handler))
        .await
        .unwrap();
    let addr = match &server {
        ScgiServer {
            listener: rt_scgi::ScgiListener::Tcp(listener),
            ..
        } => listener.local_addr().unwrap(),
        _ => unreachable!(),
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(async move {
        server
            .run_with_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let mut client = TcpStream::connect(addr).await.unwrap();
    timeout(Duration::from_secs(1), async {
        while handler.metrics().snapshot().connections_accepted == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("server did not accept the connection");

    shutdown_tx.send(()).unwrap();
    let result = timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server did not shut down")
        .unwrap();
    assert!(result.is_ok());

    // The idle connection must have been closed by the shutdown drain/abort.
    let mut buf = [0u8; 1];
    match timeout(Duration::from_secs(1), client.read(&mut buf))
        .await
        .expect("connection was not closed")
    {
        Ok(0) => {}
        Err(err) if err.kind() == std::io::ErrorKind::ConnectionReset => {}
        other => panic!("unexpected read result after shutdown: {other:?}"),
    }

    let metrics = handler.metrics().snapshot();
    assert_eq!(metrics.active_connections, 0);
    assert_eq!(metrics.connections_closed, 1);
}

#[tokio::test]
async fn test_scgi_normal_close_without_request() {
    let handler = make_handler(ScgiConfig::default());
    let (mut client, server) = tokio::io::duplex(1024);

    let h = Arc::clone(&handler);
    let server_task = tokio::spawn(async move { h.handle_stream(server).await });

    // A peer that connects and disconnects without sending anything is a
    // normal close, not a protocol error.
    client.shutdown().await.unwrap();
    expect_eof(&mut client).await;
    server_task.await.unwrap().unwrap();

    let metrics = handler.metrics().snapshot();
    assert_eq!(metrics.connections_accepted, 1);
    assert_eq!(metrics.connections_closed, 1);
    assert_eq!(metrics.active_connections, 0);
    assert_eq!(metrics.requests_received, 0);
    assert_eq!(metrics.requests_failed, 0);
    assert_eq!(metrics.protocol_errors, 0);
}

#[tokio::test]
async fn test_scgi_keep_alive_disabled_closes_after_response() {
    let handler = make_handler(ScgiConfig {
        keep_alive: false,
        ..Default::default()
    });
    let (mut client, server) = tokio::io::duplex(8192);

    let h = Arc::clone(&handler);
    let server_task = tokio::spawn(async move { h.handle_stream(server).await });

    client.write_all(&json_request(7)).await.unwrap();
    let response = String::from_utf8_lossy(&read_response(&mut client).await).to_string();
    assert!(response.contains(r#""id":7"#));

    // No shutdown from the client: the handler itself must close the
    // connection after the response when keep-alive is disabled.
    expect_eof(&mut client).await;
    server_task.await.unwrap().unwrap();

    let metrics = handler.metrics().snapshot();
    assert_eq!(metrics.requests_completed, 1);
    assert_eq!(metrics.connections_closed, 1);
}
