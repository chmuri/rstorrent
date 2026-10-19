use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use lt_net::http::{HttpClient, HttpConfig};

#[tokio::test]
async fn test_http_get_basic() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let response = "HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\nHello, World!";
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });

    let client = HttpClient::new().unwrap();
    let url = format!("http://{}/test", addr);
    let bytes = client.download(&url).await.unwrap();
    assert_eq!(&bytes[..], b"Hello, World!");
}

#[tokio::test]
async fn test_http_max_file_size_exceeded() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            // Send 100 bytes
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{}",
                "A".repeat(100)
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });

    let mut config = HttpConfig::default();
    config.max_file_size = 50; // Limit to 50 bytes
    let client = HttpClient::with_config(config).unwrap();

    let url = format!("http://{}/bigfile", addr);
    let result = client.download(&url).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("exceeds max limit"));
}

#[tokio::test]
async fn test_http_cancellation() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let header = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            let _ = socket.write_all(header.as_bytes()).await;
            // Write first chunk
            let chunk1 = "5\r\nhello\r\n";
            let _ = socket.write_all(chunk1.as_bytes()).await;
            // Delay second chunk
            tokio::time::sleep(Duration::from_millis(200)).await;
            let chunk2 = "5\r\nworld\r\n0\r\n\r\n";
            let _ = socket.write_all(chunk2.as_bytes()).await;
        }
    });

    let client = HttpClient::new().unwrap();
    let mut get = client.get(format!("http://{}/chunked", addr));
    let cancel = get.cancel_handle();

    // Cancel shortly after starting
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = cancel.send(());
    });

    let result = get.execute().await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("cancelled"));
}
