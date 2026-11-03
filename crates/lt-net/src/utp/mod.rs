//! BEP 29: Micro Transport Protocol (uTP)
//!
//! A congestion-controlled, reliable streaming transport protocol layered over UDP.
//! Implements LEDBAT congestion control, selective ACKs, out-of-order packet reassembly,
//! and standard Tokio `AsyncRead` / `AsyncWrite` stream semantics.

pub mod congestion;
pub mod packet;
pub mod socket;
pub mod stream;

pub use congestion::{LedbatCongestion, MSS, TARGET_DELAY_MICROS};
pub use packet::{PacketType, UtpExtension, UtpHeader, UtpPacket, UTP_HEADER_LEN, UTP_VERSION};
pub use socket::UtpSocket;
pub use stream::{StreamState, UtpStream};

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn test_utp_loopback_transfer() {
        let server_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let client_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

        let mut server = UtpSocket::bind(server_addr).await.unwrap();
        let server_bound = server.local_addr();

        let client = UtpSocket::bind(client_addr).await.unwrap();

        // Server accept task
        let server_task = tokio::spawn(async move {
            let mut stream = server.accept().await.unwrap();
            let mut buf = vec![0u8; 11];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"hello uTP!!");
            stream.write_all(b"pong response!").await.unwrap();
        });

        // Client connect
        let mut client_stream = client.connect(server_bound).await.unwrap();
        client_stream.write_all(b"hello uTP!!").await.unwrap();

        let mut response = vec![0u8; 14];
        client_stream.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"pong response!");

        server_task.await.unwrap();
    }
}
