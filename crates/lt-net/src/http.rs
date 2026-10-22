use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use reqwest::{Client, Proxy};
use tokio::sync::oneshot;

use crate::error::NetError;

pub const DEFAULT_MAX_FILE_SIZE: usize = 15 * 1024 * 1024; // 15 MiB
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_USER_AGENT: &str = lt_common::version::USER_AGENT_FULL;

#[derive(Clone, Debug)]
pub struct HttpConfig {
    pub user_agent: String,
    pub proxy: Option<String>,
    pub max_file_size: usize,
    pub timeout: Duration,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            user_agent: DEFAULT_USER_AGENT.to_string(),
            proxy: None,
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        }
    }
}

#[derive(Clone)]
pub struct HttpClient {
    client: Client,
    config: HttpConfig,
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new().expect("Failed to initialize default HttpClient")
    }
}

impl HttpClient {
    pub fn new() -> Result<Self, NetError> {
        Self::with_config(HttpConfig::default())
    }

    pub fn with_config(config: HttpConfig) -> Result<Self, NetError> {
        let mut builder = Client::builder()
            .user_agent(&config.user_agent)
            .timeout(config.timeout);

        if let Some(ref proxy_url) = config.proxy {
            let proxy = Proxy::all(proxy_url).map_err(|e| {
                NetError::Input(format!("Invalid proxy configuration: {}: {}", proxy_url, e))
            })?;
            builder = builder.proxy(proxy);
        }

        let client = builder
            .build()
            .map_err(|e| NetError::Internal(format!("Failed to build HTTP client: {}", e)))?;

        Ok(Self { client, config })
    }

    pub fn config(&self) -> &HttpConfig {
        &self.config
    }

    pub fn get(&self, url: impl Into<String>) -> HttpGet {
        HttpGet::new(self.client.clone(), url.into(), &self.config)
    }

    pub async fn download(&self, url: &str) -> Result<Bytes, NetError> {
        self.get(url).execute().await
    }
}

pub struct HttpGet {
    client: Client,
    url: String,
    max_file_size: usize,
    timeout: Duration,
    headers: Vec<(String, String)>,
    size_done: Arc<AtomicU64>,
    is_cancelled: Arc<AtomicBool>,
}

impl HttpGet {
    pub fn new(client: Client, url: String, config: &HttpConfig) -> Self {
        Self {
            client,
            url,
            max_file_size: config.max_file_size,
            timeout: config.timeout,
            headers: Vec::new(),
            size_done: Arc::new(AtomicU64::new(0)),
            is_cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set_timeout(&mut self, timeout: Duration) -> &mut Self {
        self.timeout = timeout;
        self
    }

    pub fn set_max_file_size(&mut self, max_size: usize) -> &mut Self {
        self.max_file_size = max_size;
        self
    }

    pub fn header(&mut self, key: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn size_done(&self) -> u64 {
        self.size_done.load(Ordering::Relaxed)
    }

    pub fn is_cancelled(&self) -> bool {
        self.is_cancelled.load(Ordering::Relaxed)
    }

    pub fn cancel_handle(&mut self) -> oneshot::Sender<()> {
        let (tx, rx) = oneshot::channel();
        let is_cancelled = self.is_cancelled.clone();
        tokio::spawn(async move {
            if rx.await.is_ok() {
                is_cancelled.store(true, Ordering::Relaxed);
            }
        });
        tx
    }

    pub async fn execute(&self) -> Result<Bytes, NetError> {
        let mut request = self.client.get(&self.url).timeout(self.timeout);

        for (k, v) in &self.headers {
            request = request.header(k, v);
        }

        let mut response = request
            .send()
            .await
            .map_err(|e| NetError::Input(format!("HTTP request failed for {}: {}", self.url, e)))?;

        if !response.status().is_success() {
            return Err(NetError::Input(format!(
                "HTTP request returned non-success status: {}",
                response.status()
            )));
        }

        if let Some(content_length) = response.content_length() {
            if content_length > self.max_file_size as u64 {
                return Err(NetError::Input(format!(
                    "HTTP Content-Length ({} bytes) exceeds max limit of {} bytes",
                    content_length, self.max_file_size
                )));
            }
        }

        let mut buffer = BytesMut::new();
        self.size_done.store(0, Ordering::Relaxed);

        while let Some(chunk) = response.chunk().await.map_err(|e| {
            NetError::Input(format!(
                "Failed reading response chunk from {}: {}",
                self.url, e
            ))
        })? {
            if self.is_cancelled.load(Ordering::Relaxed) {
                return Err(NetError::Input("HTTP download cancelled".into()));
            }

            let new_len = buffer.len() + chunk.len();
            if new_len > self.max_file_size {
                return Err(NetError::Input(format!(
                    "HTTP response size exceeded max limit of {} bytes",
                    self.max_file_size
                )));
            }

            buffer.extend_from_slice(&chunk);
            self.size_done.store(buffer.len() as u64, Ordering::Relaxed);
        }

        Ok(buffer.freeze())
    }
}
