//! BEP 29: uTP Socket multiplexer over UDP.

use std::collections::HashMap;
use std::io::{self, ErrorKind};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::{trace, warn};

use super::packet::{PacketType, UtpHeader, UtpPacket, UTP_VERSION};
use super::stream::{StreamState, UtpStream};

type StreamMap = Arc<Mutex<HashMap<(SocketAddr, u16), mpsc::UnboundedSender<UtpPacket>>>>;

/// uTP Socket multiplexing streams over a single UDP socket.
pub struct UtpSocket {
    udp_socket: Arc<UdpSocket>,
    local_addr: SocketAddr,
    streams: StreamMap,
    accept_rx: mpsc::Receiver<UtpStream>,
    _shutdown_tx: mpsc::Sender<()>,
}

impl UtpSocket {
    /// Bind uTP socket to the specified local address.
    pub async fn bind(addr: SocketAddr) -> io::Result<Self> {
        let socket = UdpSocket::bind(addr).await?;
        let local_addr = socket.local_addr()?;
        let udp_socket = Arc::new(socket);
        let streams: StreamMap = Arc::new(Mutex::new(HashMap::new()));

        let (accept_tx, accept_rx) = mpsc::channel(64);
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);

        // Spawn background datagram receiver loop
        let recv_udp = Arc::clone(&udp_socket);
        let recv_streams = Arc::clone(&streams);

        tokio::spawn(async move {
            let mut buf = vec![0u8; 65535];

            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        break;
                    }
                    recv_res = recv_udp.recv_from(&mut buf) => {
                        let (len, remote_addr) = match recv_res {
                            Ok(res) => res,
                            Err(e) => {
                                warn!(error = %e, "uTP socket recv error");
                                break;
                            }
                        };

                        let packet = match UtpPacket::decode(&buf[..len]) {
                            Ok(p) => p,
                            Err(e) => {
                                trace!(peer = %remote_addr, error = %e, "Invalid uTP packet ignored");
                                continue;
                            }
                        };

                        let conn_id = packet.header.connection_id;

                        // Check if this is an incoming SYN initiation
                        if packet.header.packet_type == PacketType::Syn {
                            let (ack_packet_to_send, new_stream) = {
                                let mut map = recv_streams.lock().unwrap();
                                // In BEP 29: initiator sends connection_id = recv_id.
                                // We respond with send_id = recv_id and recv_id = recv_id + 1.
                                let responder_recv_id = conn_id.wrapping_add(1);
                                let responder_send_id = conn_id;

                                if !map.contains_key(&(remote_addr, responder_recv_id)) {
                                    let (pkt_tx, pkt_rx) = mpsc::unbounded_channel();
                                    map.insert((remote_addr, responder_recv_id), pkt_tx.clone());
                                    map.insert((remote_addr, responder_send_id), pkt_tx);

                                    // Send ST_STATE immediately acknowledging the SYN
                                    let ack_header = UtpHeader {
                                        packet_type: PacketType::State,
                                        version: UTP_VERSION,
                                        extension: 0,
                                        connection_id: responder_send_id,
                                        timestamp_micros: 0,
                                        timestamp_diff_micros: 0,
                                        wnd_size: 1024 * 1024,
                                        seq_nr: 1,
                                        ack_nr: packet.header.seq_nr,
                                    };
                                    let ack_packet = UtpPacket::new(ack_header, Bytes::new());

                                    let stream = UtpStream::new(
                                        remote_addr,
                                        Arc::clone(&recv_udp),
                                        responder_recv_id,
                                        responder_send_id,
                                        2, // First data packet we send will be seq_nr 2
                                        packet.header.seq_nr,
                                        StreamState::Connected,
                                        pkt_rx,
                                    );

                                    (Some(ack_packet), Some(stream))
                                } else {
                                    (None, None)
                                }
                            };

                            if let Some(ack) = ack_packet_to_send {
                                let _ = recv_udp.send_to(&ack.encode(), remote_addr).await;
                            }
                            if let Some(stream) = new_stream {
                                let _ = accept_tx.try_send(stream);
                            }
                            continue;
                        }

                        // Dispatch to existing stream
                        let stream_sender = {
                            let map = recv_streams.lock().unwrap();
                            map.get(&(remote_addr, conn_id)).cloned()
                        };

                        if let Some(sender) = stream_sender {
                            let _ = sender.send(packet);
                        } else if packet.header.packet_type != PacketType::Reset {
                            // Send ST_RESET for unknown incoming connection
                            let reset_header = UtpHeader {
                                packet_type: PacketType::Reset,
                                version: UTP_VERSION,
                                extension: 0,
                                connection_id: conn_id,
                                timestamp_micros: 0,
                                timestamp_diff_micros: 0,
                                wnd_size: 0,
                                seq_nr: 1,
                                ack_nr: packet.header.seq_nr,
                            };
                            let reset_packet = UtpPacket::new(reset_header, Bytes::new());
                            let _ = recv_udp.send_to(&reset_packet.encode(), remote_addr).await;
                        }
                    }
                }
            }
        });

        Ok(Self {
            udp_socket,
            local_addr,
            streams,
            accept_rx,
            _shutdown_tx: shutdown_tx,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Accept an incoming uTP stream.
    pub async fn accept(&mut self) -> io::Result<UtpStream> {
        self.accept_rx
            .recv()
            .await
            .ok_or_else(|| io::Error::new(ErrorKind::BrokenPipe, "uTP socket listener closed"))
    }

    /// Connect to a remote uTP peer.
    pub async fn connect(&self, peer_addr: SocketAddr) -> io::Result<UtpStream> {
        let (recv_id, send_id) = {
            let mut rand_bytes = [0u8; 2];
            getrandom::fill(&mut rand_bytes).map_err(io::Error::other)?;
            let id = u16::from_ne_bytes(rand_bytes);
            (id, id.wrapping_add(1))
        };

        let (pkt_tx, mut pkt_rx) = mpsc::unbounded_channel();
        {
            let mut map = self.streams.lock().unwrap();
            map.insert((peer_addr, recv_id), pkt_tx.clone());
            map.insert((peer_addr, send_id), pkt_tx);
        }

        // Send ST_SYN
        let syn_header = UtpHeader {
            packet_type: PacketType::Syn,
            version: UTP_VERSION,
            extension: 0,
            connection_id: recv_id,
            timestamp_micros: 0,
            timestamp_diff_micros: 0,
            wnd_size: 1024 * 1024,
            seq_nr: 1,
            ack_nr: 0,
        };
        let syn_packet = UtpPacket::new(syn_header, Bytes::new());
        let syn_bytes = syn_packet.encode();

        let mut retries = 0;
        let ack_seq = loop {
            self.udp_socket.send_to(&syn_bytes, peer_addr).await?;

            match timeout(Duration::from_millis(500), pkt_rx.recv()).await {
                Ok(Some(pkt)) => {
                    if pkt.header.packet_type == PacketType::State {
                        break pkt.header.seq_nr;
                    } else if pkt.header.packet_type == PacketType::Reset {
                        return Err(io::Error::new(
                            ErrorKind::ConnectionRefused,
                            "Connection refused (ST_RESET)",
                        ));
                    }
                }
                Ok(None) => {
                    return Err(io::Error::new(ErrorKind::BrokenPipe, "uTP channel closed"));
                }
                Err(_) => {
                    retries += 1;
                    if retries >= 5 {
                        let mut map = self.streams.lock().unwrap();
                        map.remove(&(peer_addr, recv_id));
                        map.remove(&(peer_addr, send_id));
                        return Err(io::Error::new(ErrorKind::TimedOut, "uTP connect timed out"));
                    }
                }
            }
        };

        let stream = UtpStream::new(
            peer_addr,
            Arc::clone(&self.udp_socket),
            recv_id,
            send_id,
            2,
            ack_seq,
            StreamState::Connected,
            pkt_rx,
        );

        Ok(stream)
    }
}
