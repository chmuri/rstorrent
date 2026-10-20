use std::str::FromStr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use lt_net::addr::SocketAddress;
use lt_net::proxy::{HttpProxy, Socks5Proxy};
use lt_net::throttle::Throttle;

#[test]
fn test_throttle_basic() {
    let throttle = Throttle::new(0);
    assert!(!throttle.is_throttled());

    let node = throttle.create_node();
    assert_eq!(node.quota(), 0);

    // In unthrottled mode, node_used returns used amount directly
    let used = throttle.node_used(&node, 4096);
    assert_eq!(used, 4096);
    assert_eq!(node.quota(), 0);

    // Enable throttling
    throttle.set_max_rate(100_000);
    assert!(throttle.is_throttled());

    // Allocate 10,000 bytes
    let allocated = throttle.update_quota(10_000);
    assert_eq!(allocated, 10_000);
    // Node should receive up to max_chunk_size (10_000 < 16384)
    assert_eq!(node.quota(), 10_000);

    // Use 4000 bytes from node
    let used = throttle.node_used(&node, 4000);
    assert_eq!(used, 4000);
    assert_eq!(node.quota(), 6000);
}

#[tokio::test]
async fn test_http_proxy_handshake_success() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let n = socket.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]);
            assert!(req.contains("CONNECT target.host:8080 HTTP/1.1"));
            let response = "HTTP/1.1 200 Connection established\r\n\r\n";
            socket.write_all(response.as_bytes()).await.unwrap();
        }
    });

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let res = HttpProxy::handshake(&mut stream, "target.host", 8080).await;
    assert!(res.is_ok());
}

#[tokio::test]
async fn test_http_proxy_handshake_rejected() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await.unwrap();
            let response = "HTTP/1.1 403 Forbidden\r\n\r\n";
            socket.write_all(response.as_bytes()).await.unwrap();
        }
    });

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let res = HttpProxy::handshake(&mut stream, "target.host", 8080).await;
    assert!(res.is_err());
    let err = res.unwrap_err().to_string();
    assert!(err.contains("rejected"));
}

#[tokio::test]
async fn test_socks5_proxy_handshake_no_auth() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            // 1. Read greeting
            let mut greeting = [0u8; 3];
            socket.read_exact(&mut greeting).await.unwrap();
            assert_eq!(greeting[0], 0x05); // SOCKS5
                                           // Reply no-auth
            socket.write_all(&[0x05, 0x00]).await.unwrap();

            // 2. Read connect request
            let mut req = [0u8; 10]; // 05 01 00 01 (ipv4: 4 bytes) (port: 2 bytes)
            socket.read_exact(&mut req).await.unwrap();
            assert_eq!(req[0], 0x05); // VER
            assert_eq!(req[1], 0x01); // CONNECT
            assert_eq!(req[3], 0x01); // IPv4
            assert_eq!(&req[4..8], &[127, 0, 0, 1]); // 127.0.0.1

            // Reply success
            // 05 00 (success) 00 01 (ipv4 0.0.0.0:0)
            socket
                .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await
                .unwrap();
        }
    });

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let target = SocketAddress::from_str("127.0.0.1:6881").unwrap();
    let res = Socks5Proxy::handshake(&mut stream, &target, None, None).await;
    assert!(res.is_ok());
}

#[tokio::test]
async fn test_socks5_proxy_handshake_with_auth() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            // 1. Read greeting
            let mut greeting = [0u8; 4];
            socket.read_exact(&mut greeting).await.unwrap();
            assert_eq!(greeting[0], 0x05);
            // Choose auth (0x02)
            socket.write_all(&[0x05, 0x02]).await.unwrap();

            // 2. Read auth
            let mut ver_ulen = [0u8; 2];
            socket.read_exact(&mut ver_ulen).await.unwrap();
            assert_eq!(ver_ulen[0], 0x01);
            let ulen = ver_ulen[1] as usize;
            let mut user = vec![0u8; ulen];
            socket.read_exact(&mut user).await.unwrap();
            assert_eq!(&user[..], b"testuser");

            let mut plen = [0u8; 1];
            socket.read_exact(&mut plen).await.unwrap();
            let mut pass = vec![0u8; plen[0] as usize];
            socket.read_exact(&mut pass).await.unwrap();
            assert_eq!(&pass[..], b"secret");

            // Reply auth success: 01 00
            socket.write_all(&[0x01, 0x00]).await.unwrap();

            // 3. Read connect
            let mut req = [0u8; 10];
            socket.read_exact(&mut req).await.unwrap();
            assert_eq!(req[0], 0x05);
            assert_eq!(req[1], 0x01);

            // Reply success
            socket
                .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await
                .unwrap();
        }
    });

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let target = SocketAddress::from_str("127.0.0.1:6881").unwrap();
    let res = Socks5Proxy::handshake(&mut stream, &target, Some("testuser"), Some("secret")).await;
    assert!(res.is_ok());
}
