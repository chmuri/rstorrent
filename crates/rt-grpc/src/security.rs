//! Server-side security for the gRPC transport.
//!
//! Trust is decided by the listener, never by the client payload: the local
//! Unix socket (permission-guarded `0o600`) is trusted, while a TCP peer is
//! trusted only after presenting the configured bearer token. TLS and the
//! token are installed once by the daemon from `rstorrent.toml`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use tonic::service::Interceptor;
use tonic::{Request, Status};

/// TLS identity and bearer-token settings for the TCP listener.
#[derive(Debug, Clone, Default)]
pub struct GrpcSecurity {
    pub token: Option<String>,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
}

static SECURITY: OnceLock<GrpcSecurity> = OnceLock::new();
static FORCE_SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Install the gRPC security configuration. Called once before the listeners
/// are opened; later calls are ignored.
pub fn install_security(cfg: GrpcSecurity) {
    let _ = SECURITY.set(cfg);
}

pub fn security() -> Option<&'static GrpcSecurity> {
    SECURITY.get()
}

/// Mark that an RPC caller asked for a forced (fast) shutdown.
pub fn request_force_shutdown() {
    FORCE_SHUTDOWN.store(true, Ordering::SeqCst);
}

pub fn force_shutdown_requested() -> bool {
    FORCE_SHUTDOWN.load(Ordering::SeqCst)
}

/// Server-decided trust level for a connection, carried to handlers as a
/// request extension.
#[derive(Clone, Copy, Debug)]
pub struct CallerTrusted(pub bool);

/// TCP interceptor: enforce the bearer token when configured and record the
/// resulting trust level. Without a token the peer stays untrusted (restricted
/// to `UNTRUSTED_SAFE` commands).
pub fn tcp_interceptor(token: Option<String>) -> impl Interceptor + Clone {
    move |mut req: Request<()>| {
        let trusted = match token.as_deref() {
            Some(expected) if !expected.is_empty() => {
                let header = req
                    .metadata()
                    .get("authorization")
                    .and_then(|value| value.to_str().ok());
                if !rt_rpc::auth::bearer_authorized(header, Some(expected)) {
                    return Err(Status::unauthenticated("invalid or missing bearer token"));
                }
                true
            }
            _ => false,
        };
        req.extensions_mut().insert(CallerTrusted(trusted));
        Ok(req)
    }
}

/// Unix interceptor: the socket is permission-guarded, so its peer is trusted.
pub fn local_interceptor() -> impl Interceptor + Clone {
    move |mut req: Request<()>| {
        req.extensions_mut().insert(CallerTrusted(true));
        Ok(req)
    }
}
