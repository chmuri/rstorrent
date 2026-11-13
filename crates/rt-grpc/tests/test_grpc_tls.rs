//! Live TLS handshake on the gRPC TCP listener, bearer-token enforcement over
//! TLS, and rejection of a plaintext peer. This binary installs the
//! process-global security config exactly once (its own `OnceLock`), so it must
//! stay a single test.

use rt_core::{DownloadList, SessionManager, ViewManager};
use rt_grpc::system_service_client::SystemServiceClient;
use rt_grpc::{install_security, Empty, GrpcContext, GrpcSecurity, GrpcServer};
use rt_rpc::CommandMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::watch;
use tonic::transport::{Certificate, Channel, ClientTlsConfig};
use tonic::{Code, Request};

#[tokio::test]
async fn tls_listener_enforces_handshake_and_bearer_token() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let cert_path = fixtures.join("tls_cert.pem");
    let key_path = fixtures.join("tls_key.pem");

    install_security(GrpcSecurity {
        token: Some("s3cret".to_string()),
        tls_cert: Some(cert_path.clone()),
        tls_key: Some(key_path),
    });

    let command_map = Arc::new(RwLock::new(CommandMap::new()));
    let download_list = Arc::new(RwLock::new(DownloadList::new()));
    let view_manager = Arc::new(RwLock::new(ViewManager::new()));
    let session_manager = Arc::new(Mutex::new(SessionManager::new()));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let ctx = GrpcContext::new(
        command_map,
        download_list,
        view_manager,
        session_manager,
        shutdown_tx.clone(),
    );

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let server = GrpcServer::new(ctx);
    let handle = tokio::spawn(async move {
        let _ = server.run_tcp(addr, shutdown_rx).await;
    });

    let ca = std::fs::read_to_string(&cert_path).unwrap();
    let tls = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(ca))
        .domain_name("localhost");
    let uri = format!("https://127.0.0.1:{port}");

    // Retry until the TLS listener is bound and the handshake succeeds.
    let mut channel = None;
    let mut last_err = None;
    for _ in 0..100 {
        match Channel::from_shared(uri.clone())
            .unwrap()
            .tls_config(tls.clone())
            .unwrap()
            .connect()
            .await
        {
            Ok(ch) => {
                channel = Some(ch);
                break;
            }
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    let channel = channel.unwrap_or_else(|| panic!("TLS connect failed: {last_err:?}"));

    let mut system = SystemServiceClient::new(channel);

    // A TLS peer without a bearer token is unauthenticated.
    let err = system.get_version(Empty {}).await.unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);

    // A TLS peer presenting the configured token succeeds.
    let mut req = Request::new(Empty {});
    req.metadata_mut()
        .insert("authorization", "Bearer s3cret".parse().unwrap());
    let ver = system.get_version(req).await.unwrap().into_inner();
    assert!(!ver.version.is_empty());
    assert_eq!(ver.api_version, lt_common::version::VERSION);

    // A plaintext peer cannot talk to the TLS listener.
    let plain = Channel::from_shared(format!("http://127.0.0.1:{port}"))
        .unwrap()
        .connect()
        .await;
    let rejected = match plain {
        Ok(ch) => SystemServiceClient::new(ch)
            .get_version(Empty {})
            .await
            .is_err(),
        Err(_) => true,
    };
    assert!(
        rejected,
        "plaintext peer must be rejected by the TLS listener"
    );

    let _ = shutdown_tx.send(true);
    let _ = handle.await;
}
