//! Background gRPC stream subscribers with bounded buffering and reconnection.

use std::time::Duration;

use rt_grpc::{SessionStatsResponse, TorrentEvent};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::bridge::GrpcBridge;
use crate::connection::ConnectionManager;

/// Capacity of the channel between the stream tasks and the event loop.
pub const STREAM_CAPACITY: usize = 256;

/// One update pushed from a stream task.
#[derive(Debug)]
pub enum StreamUpdate {
    /// A torrent state change.
    Event(TorrentEvent),
    /// Session-wide statistics.
    Stats(SessionStatsResponse),
}

/// Handle to the two background stream tasks.
pub struct Streams {
    receiver: mpsc::Receiver<StreamUpdate>,
    tasks: Vec<JoinHandle<()>>,
}

impl Streams {
    /// Starts the event and stats subscribers for `endpoint`.
    pub fn spawn(endpoint: &str, timeout: Duration) -> Self {
        let (sender, receiver) = mpsc::channel(STREAM_CAPACITY);
        let endpoint = endpoint.to_string();

        let tasks = vec![
            tokio::spawn(subscribe_events(endpoint.clone(), timeout, sender.clone())),
            tokio::spawn(subscribe_stats(endpoint, timeout, sender)),
        ];

        Self { receiver, tasks }
    }

    /// Non-blocking read of the next update.
    pub fn try_recv(&mut self) -> Option<StreamUpdate> {
        self.receiver.try_recv().ok()
    }
}

impl Drop for Streams {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Reconnects and forwards torrent events until the receiver goes away.
async fn subscribe_events(endpoint: String, timeout: Duration, sender: mpsc::Sender<StreamUpdate>) {
    let mut connection = ConnectionManager::new();

    loop {
        if sender.is_closed() {
            return;
        }

        match GrpcBridge::connect(&endpoint, timeout).await {
            Ok(mut bridge) => match bridge.stream_events().await {
                Ok(mut stream) => {
                    connection.record_success();
                    while let Ok(Some(event)) = stream.message().await {
                        if sender.send(StreamUpdate::Event(event)).await.is_err() {
                            return;
                        }
                    }
                }
                Err(_) => connection.record_failure(),
            },
            Err(_) => connection.record_failure(),
        }

        tokio::time::sleep(connection.next_backoff()).await;
    }
}

/// Reconnects and forwards session stats until the receiver goes away.
async fn subscribe_stats(endpoint: String, timeout: Duration, sender: mpsc::Sender<StreamUpdate>) {
    let mut connection = ConnectionManager::new();

    loop {
        if sender.is_closed() {
            return;
        }

        match GrpcBridge::connect(&endpoint, timeout).await {
            Ok(mut bridge) => match bridge.stream_stats().await {
                Ok(mut stream) => {
                    connection.record_success();
                    while let Ok(Some(stats)) = stream.message().await {
                        if sender.send(StreamUpdate::Stats(stats)).await.is_err() {
                            return;
                        }
                    }
                }
                Err(_) => connection.record_failure(),
            },
            Err(_) => connection.record_failure(),
        }

        tokio::time::sleep(connection.next_backoff()).await;
    }
}
