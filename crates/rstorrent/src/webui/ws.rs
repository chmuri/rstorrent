//! WebSocket event stream for the WebUI.
//!
//! Mirrors the gRPC `StreamEvents` surface: a subscriber receives a JSON message
//! for every download-list event (start/stop/metadata/finished/...) carrying the
//! same [`TorrentSummary`] as REST, plus a periodic session-stats snapshot so a
//! browser can refresh totals without polling. The handshake is gated by the
//! same bearer-token middleware as REST; browsers pass `?token=<token>`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::broadcast;

use super::convert::{self, SessionStats, TorrentSummary};
use super::WebuiState;

/// A download-list event, equivalent to the gRPC `TorrentEvent`.
#[derive(Debug, Clone, Serialize)]
pub struct WebuiTorrentEvent {
    pub event_name: String,
    pub hash: String,
    pub summary: TorrentSummary,
    pub timestamp: u64,
}

/// Wire envelope for the WebSocket stream. Internally tagged so a browser can
/// dispatch on `type` without guessing the payload shape.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    Torrent(Box<WebuiTorrentEvent>),
    Stats { stats: SessionStats, timestamp: u64 },
}

type WsSender = SplitSink<WebSocket, Message>;

/// Upgrade an authenticated request to the event stream.
pub async fn events_ws(ws: WebSocketUpgrade, State(state): State<WebuiState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

async fn handle_socket(socket: WebSocket, state: WebuiState) {
    let (mut sender, mut receiver) = socket.split();
    let mut events = state.event_broadcaster.subscribe();
    let mut stats_tick = tokio::time::interval(Duration::from_secs(1));
    stats_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Send an immediate snapshot so a fresh client has state before the first
    // periodic tick lands.
    if send_stats(&mut sender, &state).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) => {
                    if send_json(&mut sender, &WsMessage::Torrent(Box::new(event)))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                // A slow client that fell behind skips the missed events rather
                // than stalling the stream; the next stats tick resyncs totals.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = stats_tick.tick() => {
                if send_stats(&mut sender, &state).await.is_err() {
                    break;
                }
            }
            incoming = receiver.next() => match incoming {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
}

async fn send_stats(sender: &mut WsSender, state: &WebuiState) -> Result<(), axum::Error> {
    let stats = {
        let list = state
            .download_list
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let map = state.command_map.read().unwrap_or_else(|e| e.into_inner());
        convert::stats_from(&list, &map)
    };
    send_json(
        sender,
        &WsMessage::Stats {
            stats,
            timestamp: now_secs(),
        },
    )
    .await
}

async fn send_json(sender: &mut WsSender, msg: &WsMessage) -> Result<(), axum::Error> {
    let text = serde_json::to_string(msg).map_err(axum::Error::new)?;
    sender.send(Message::text(text)).await
}
