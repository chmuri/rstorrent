//! BEP 29: Micro Transport Protocol (uTP) stream implementation.

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, ErrorKind};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use super::congestion::{LedbatCongestion, MSS};
use super::packet::{PacketType, UtpExtension, UtpHeader, UtpPacket, UTP_VERSION};

const MAX_REORDER_BUFFER_SIZE: usize = 128;
const DEFAULT_RCV_WND: u32 = 1024 * 1024; // 1MB receive buffer

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    SynSent,
    Connected,
    FinSent,
    FinReceived,
    Closed,
    Reset,
}

#[derive(Debug)]
struct InFlightPacket {
    seq_nr: u16,
    sent_at: Instant,
    packet: UtpPacket,
    retransmitted: bool,
}

/// A bidirectional uTP stream connection implementing `AsyncRead` and `AsyncWrite`.
pub struct UtpStream {
    peer_addr: SocketAddr,
    udp_socket: Arc<UdpSocket>,
    recv_id: u16,
    send_id: u16,
    state: StreamState,

    // Sequence numbers
    seq_nr: u16,
    ack_nr: u16,
    last_acked_seq: u16,

    // Timestamps
    last_received_micros: u32,
    last_received_at: Instant,

    // Congestion and window
    congestion: LedbatCongestion,
    remote_wnd: u32,

    // Queues
    in_flight: VecDeque<InFlightPacket>,
    reorder_buffer: BTreeMap<u16, Bytes>,
    read_buffer: BytesMut,

    // Channel from socket multiplexer
    packet_rx: mpsc::UnboundedReceiver<UtpPacket>,

    // Async wakers
    read_waker: Option<Waker>,
    write_waker: Option<Waker>,
    flush_waker: Option<Waker>,

    // Timer for RTO
    last_rto_check: Instant,
}

impl UtpStream {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        peer_addr: SocketAddr,
        udp_socket: Arc<UdpSocket>,
        recv_id: u16,
        send_id: u16,
        initial_seq_nr: u16,
        initial_ack_nr: u16,
        state: StreamState,
        packet_rx: mpsc::UnboundedReceiver<UtpPacket>,
    ) -> Self {
        Self {
            peer_addr,
            udp_socket,
            recv_id,
            send_id,
            state,
            seq_nr: initial_seq_nr,
            ack_nr: initial_ack_nr,
            last_acked_seq: initial_seq_nr.wrapping_sub(1),
            last_received_micros: 0,
            last_received_at: Instant::now(),
            congestion: LedbatCongestion::new(),
            remote_wnd: DEFAULT_RCV_WND,
            in_flight: VecDeque::new(),
            reorder_buffer: BTreeMap::new(),
            read_buffer: BytesMut::new(),
            packet_rx,
            read_waker: None,
            write_waker: None,
            flush_waker: None,
            last_rto_check: Instant::now(),
        }
    }

    pub fn peer_addr(&self) -> SocketAddr {
        self.peer_addr
    }

    pub fn state(&self) -> StreamState {
        self.state
    }

    pub fn recv_id(&self) -> u16 {
        self.recv_id
    }

    pub fn send_id(&self) -> u16 {
        self.send_id
    }

    fn now_micros(&self) -> u32 {
        let elapsed = self.last_received_at.elapsed().as_micros();
        (elapsed & 0xFFFF_FFFF) as u32
    }

    fn timestamp_diff(&self) -> u32 {
        if self.last_received_micros == 0 {
            0
        } else {
            self.now_micros().saturating_sub(self.last_received_micros)
        }
    }

    /// Process all pending packets queued from the socket.
    fn drain_incoming_packets(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        let mut wake_write = false;
        let mut send_ack = false;

        loop {
            match self.packet_rx.poll_recv(cx) {
                Poll::Ready(Some(packet)) => {
                    self.last_received_micros = packet.header.timestamp_micros;
                    self.last_received_at = Instant::now();
                    self.remote_wnd = packet.header.wnd_size;

                    match packet.header.packet_type {
                        PacketType::Reset => {
                            self.state = StreamState::Reset;
                            return Err(io::Error::new(
                                ErrorKind::ConnectionReset,
                                "uTP connection reset by peer",
                            ));
                        }
                        PacketType::Fin => {
                            if self.state == StreamState::FinSent {
                                self.state = StreamState::Closed;
                            } else {
                                self.state = StreamState::FinReceived;
                            }
                            self.handle_ack(
                                packet.header.ack_nr,
                                packet.header.timestamp_diff_micros,
                                &packet.extensions,
                            );
                            self.ack_nr = packet.header.seq_nr;
                            send_ack = true;
                            wake_write = true;
                        }
                        PacketType::State => {
                            self.handle_ack(
                                packet.header.ack_nr,
                                packet.header.timestamp_diff_micros,
                                &packet.extensions,
                            );
                            wake_write = true;
                        }
                        PacketType::Data => {
                            self.handle_ack(
                                packet.header.ack_nr,
                                packet.header.timestamp_diff_micros,
                                &packet.extensions,
                            );
                            wake_write = true;

                            let seq = packet.header.seq_nr;
                            let expected = self.ack_nr.wrapping_add(1);

                            if seq == expected {
                                // In-order packet
                                self.ack_nr = seq;
                                self.read_buffer.extend_from_slice(&packet.payload);

                                // Check if any queued reorder packets can now be drained
                                while let Some(first_key) =
                                    self.reorder_buffer.keys().next().copied()
                                {
                                    if first_key == self.ack_nr.wrapping_add(1) {
                                        let data = self.reorder_buffer.remove(&first_key).unwrap();
                                        self.ack_nr = first_key;
                                        self.read_buffer.extend_from_slice(&data);
                                    } else {
                                        break;
                                    }
                                }
                            } else if Self::seq_less(self.ack_nr, seq) {
                                // Out-of-order ahead packet
                                if self.reorder_buffer.len() < MAX_REORDER_BUFFER_SIZE {
                                    self.reorder_buffer.insert(seq, packet.payload);
                                }
                            }
                            // Trigger immediate ACK for received data
                            send_ack = true;
                        }
                        PacketType::Syn => {
                            // Duplicate SYN, ignore or re-ack
                            send_ack = true;
                        }
                    }
                }
                Poll::Ready(None) => {
                    // Socket multiplexer closed channel
                    if self.state != StreamState::Closed {
                        self.state = StreamState::Closed;
                    }
                    break;
                }
                Poll::Pending => break,
            }
        }

        if send_ack
            && (self.state == StreamState::Connected
                || self.state == StreamState::FinReceived
                || self.state == StreamState::FinSent)
        {
            let _ = self.send_state_packet();
        }

        if wake_write {
            if let Some(waker) = self.write_waker.take() {
                waker.wake();
            }
            if self.in_flight.is_empty() {
                if let Some(waker) = self.flush_waker.take() {
                    waker.wake();
                }
            }
        }

        Ok(())
    }

    /// Process acknowledgment and selective ACKs.
    fn handle_ack(&mut self, ack_nr: u16, peer_diff_micros: u32, extensions: &[UtpExtension]) {
        let now = Instant::now();
        let mut bytes_acked = 0;
        let mut latest_rtt = None;

        // Collect packets acknowledged up to ack_nr
        while let Some(front) = self.in_flight.front() {
            if Self::seq_less_equal(front.seq_nr, ack_nr) {
                let packet = self.in_flight.pop_front().unwrap();
                bytes_acked += packet.packet.payload.len() as u32;
                if !packet.retransmitted {
                    latest_rtt = Some(now.saturating_duration_since(packet.sent_at));
                }
                self.last_acked_seq = packet.seq_nr;
            } else {
                break;
            }
        }

        // Process Selective ACK extension if present
        for ext in extensions {
            if let UtpExtension::SelectiveAck(mask) = ext {
                for (byte_idx, &byte) in mask.iter().enumerate() {
                    for bit in 0..8 {
                        if (byte & (1 << bit)) != 0 {
                            let sack_seq = ack_nr.wrapping_add(2 + (byte_idx * 8 + bit) as u16);
                            if let Some(pos) =
                                self.in_flight.iter().position(|p| p.seq_nr == sack_seq)
                            {
                                let packet = self.in_flight.remove(pos).unwrap();
                                bytes_acked += packet.packet.payload.len() as u32;
                                if !packet.retransmitted {
                                    latest_rtt =
                                        Some(now.saturating_duration_since(packet.sent_at));
                                }
                            }
                        }
                    }
                }
            }
        }

        if bytes_acked > 0 {
            self.congestion
                .on_ack(bytes_acked, peer_diff_micros, latest_rtt, now);
        }
    }

    /// Check for timeouts and retransmit unacknowledged packets.
    fn check_retransmissions(&mut self) {
        let now = Instant::now();
        let rto = self.congestion.rto();
        let now_micros = self.now_micros();
        let diff = self.timestamp_diff();

        let mut lost_any = false;
        for packet in self.in_flight.iter_mut() {
            if now.saturating_duration_since(packet.sent_at) > rto {
                lost_any = true;
                packet.sent_at = now;
                packet.retransmitted = true;
                packet.packet.header.timestamp_micros = now_micros;
                packet.packet.header.timestamp_diff_micros = diff;

                let raw = packet.packet.encode();
                let _ = self.udp_socket.try_send_to(&raw, self.peer_addr);
            }
        }

        if lost_any {
            self.congestion.on_loss();
        }
    }

    fn send_state_packet(&self) -> io::Result<()> {
        let header = UtpHeader {
            packet_type: PacketType::State,
            version: UTP_VERSION,
            extension: 0,
            connection_id: self.send_id,
            timestamp_micros: self.now_micros(),
            timestamp_diff_micros: self.timestamp_diff(),
            wnd_size: DEFAULT_RCV_WND,
            seq_nr: self.seq_nr,
            ack_nr: self.ack_nr,
        };
        let packet = UtpPacket::new(header, Bytes::new());
        let encoded = packet.encode();
        match self.udp_socket.try_send_to(&encoded, self.peer_addr) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Comparison for 16-bit sequence numbers with wrapping arithmetic.
    pub fn seq_less(a: u16, b: u16) -> bool {
        b.wrapping_sub(a) < 0x8000 && a != b
    }

    pub fn seq_less_equal(a: u16, b: u16) -> bool {
        a == b || Self::seq_less(a, b)
    }
}

impl AsyncRead for UtpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.as_mut().get_mut();

        // 1. Drain incoming packets from the UDP receiver
        if let Err(e) = this.drain_incoming_packets(cx) {
            return Poll::Ready(Err(e));
        }

        // 2. Check retransmissions
        if this.last_rto_check.elapsed() >= Duration::from_millis(50) {
            this.check_retransmissions();
            this.last_rto_check = Instant::now();
        }

        // 3. Serve available read buffer
        if !this.read_buffer.is_empty() {
            let to_read = this.read_buffer.len().min(buf.remaining());
            let chunk = this.read_buffer.split_to(to_read);
            buf.put_slice(&chunk);
            return Poll::Ready(Ok(()));
        }

        // 4. EOF conditions
        if this.state == StreamState::FinReceived || this.state == StreamState::Closed {
            return Poll::Ready(Ok(()));
        }

        if this.state == StreamState::Reset {
            return Poll::Ready(Err(io::Error::new(
                ErrorKind::ConnectionReset,
                "uTP stream reset",
            )));
        }

        // Register waker for incoming packets
        this.read_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl AsyncWrite for UtpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.as_mut().get_mut();

        if let Err(e) = this.drain_incoming_packets(cx) {
            return Poll::Ready(Err(e));
        }

        if this.state == StreamState::Reset || this.state == StreamState::Closed {
            return Poll::Ready(Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "uTP stream closed",
            )));
        }

        // Check send window
        let available = this.congestion.available_window(this.remote_wnd);
        if available == 0 || !this.congestion.can_send(this.remote_wnd) {
            this.write_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }

        let send_len = buf.len().min(MSS as usize).min(available as usize);
        if send_len == 0 {
            this.write_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }

        let payload = Bytes::copy_from_slice(&buf[..send_len]);
        let now_micros = this.now_micros();
        let diff = this.timestamp_diff();

        let header = UtpHeader {
            packet_type: PacketType::Data,
            version: UTP_VERSION,
            extension: 0,
            connection_id: this.send_id,
            timestamp_micros: now_micros,
            timestamp_diff_micros: diff,
            wnd_size: DEFAULT_RCV_WND,
            seq_nr: this.seq_nr,
            ack_nr: this.ack_nr,
        };

        let packet = UtpPacket::new(header, payload);
        let encoded = packet.encode();

        match this.udp_socket.try_send_to(&encoded, this.peer_addr) {
            Ok(_) => {
                this.congestion.on_packet_sent(send_len as u32);
                this.in_flight.push_back(InFlightPacket {
                    seq_nr: this.seq_nr,
                    sent_at: Instant::now(),
                    packet,
                    retransmitted: false,
                });
                this.seq_nr = this.seq_nr.wrapping_add(1);
                Poll::Ready(Ok(send_len))
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                this.write_waker = Some(cx.waker().clone());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.as_mut().get_mut();

        if let Err(e) = this.drain_incoming_packets(cx) {
            return Poll::Ready(Err(e));
        }

        if this.in_flight.is_empty() {
            Poll::Ready(Ok(()))
        } else {
            this.flush_waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.as_mut().get_mut();

        if let Err(e) = this.drain_incoming_packets(cx) {
            return Poll::Ready(Err(e));
        }

        match this.state {
            StreamState::Closed | StreamState::Reset => Poll::Ready(Ok(())),
            StreamState::FinReceived => {
                // Peer already FIN'd, send our FIN and close
                let header = UtpHeader {
                    packet_type: PacketType::Fin,
                    version: UTP_VERSION,
                    extension: 0,
                    connection_id: this.send_id,
                    timestamp_micros: this.now_micros(),
                    timestamp_diff_micros: this.timestamp_diff(),
                    wnd_size: DEFAULT_RCV_WND,
                    seq_nr: this.seq_nr,
                    ack_nr: this.ack_nr,
                };
                let packet = UtpPacket::new(header, Bytes::new());
                let _ = this
                    .udp_socket
                    .try_send_to(&packet.encode(), this.peer_addr);
                this.seq_nr = this.seq_nr.wrapping_add(1);
                this.state = StreamState::Closed;
                Poll::Ready(Ok(()))
            }
            StreamState::Connected => {
                let header = UtpHeader {
                    packet_type: PacketType::Fin,
                    version: UTP_VERSION,
                    extension: 0,
                    connection_id: this.send_id,
                    timestamp_micros: this.now_micros(),
                    timestamp_diff_micros: this.timestamp_diff(),
                    wnd_size: DEFAULT_RCV_WND,
                    seq_nr: this.seq_nr,
                    ack_nr: this.ack_nr,
                };
                let packet = UtpPacket::new(header, Bytes::new());
                let _ = this
                    .udp_socket
                    .try_send_to(&packet.encode(), this.peer_addr);
                this.seq_nr = this.seq_nr.wrapping_add(1);
                this.state = StreamState::FinSent;
                this.flush_waker = Some(cx.waker().clone());
                Poll::Pending
            }
            StreamState::FinSent => {
                if this.in_flight.is_empty() {
                    Poll::Ready(Ok(()))
                } else {
                    this.flush_waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
            StreamState::SynSent => {
                this.state = StreamState::Closed;
                Poll::Ready(Ok(()))
            }
        }
    }
}
