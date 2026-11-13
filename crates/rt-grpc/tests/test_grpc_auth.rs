//! Server-decided trust for the gRPC transport, exercised directly on the
//! interceptors so no process-global security state (`OnceLock`) is involved.

use rt_grpc::security::{local_interceptor, tcp_interceptor, CallerTrusted};
use tonic::service::Interceptor;
use tonic::{Code, Request};

fn bearer_request(token: Option<&str>) -> Request<()> {
    let mut req = Request::new(());
    if let Some(token) = token {
        req.metadata_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
    }
    req
}

fn trusted(req: Request<()>) -> bool {
    req.extensions().get::<CallerTrusted>().unwrap().0
}

#[test]
fn tcp_with_valid_token_is_trusted() {
    let mut interceptor = tcp_interceptor(Some("s3cret".to_string()));
    let out = interceptor.call(bearer_request(Some("s3cret"))).unwrap();
    assert!(trusted(out));
}

#[test]
fn tcp_lowercase_bearer_scheme_is_accepted() {
    let mut interceptor = tcp_interceptor(Some("s3cret".to_string()));
    let mut req = Request::new(());
    req.metadata_mut()
        .insert("authorization", "bearer s3cret".parse().unwrap());
    let out = interceptor.call(req).unwrap();
    assert!(trusted(out));
}

#[test]
fn tcp_with_wrong_token_is_unauthenticated() {
    let mut interceptor = tcp_interceptor(Some("s3cret".to_string()));
    let err = interceptor.call(bearer_request(Some("wrong"))).unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);
}

#[test]
fn tcp_without_token_is_unauthenticated() {
    let mut interceptor = tcp_interceptor(Some("s3cret".to_string()));
    let err = interceptor.call(bearer_request(None)).unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);
}

#[test]
fn tcp_with_non_bearer_scheme_is_unauthenticated() {
    let mut interceptor = tcp_interceptor(Some("s3cret".to_string()));
    let mut req = Request::new(());
    req.metadata_mut()
        .insert("authorization", "Basic s3cret".parse().unwrap());
    let err = interceptor.call(req).unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);
}

#[test]
fn tcp_without_configured_token_is_untrusted() {
    // No token configured: the peer is allowed through but stays untrusted, so
    // the command layer restricts it to UNTRUSTED_SAFE commands.
    let mut interceptor = tcp_interceptor(None);
    let out = interceptor.call(bearer_request(None)).unwrap();
    assert!(!trusted(out));

    let mut empty = tcp_interceptor(Some(String::new()));
    let out = empty.call(bearer_request(Some("anything"))).unwrap();
    assert!(!trusted(out));
}

#[test]
fn unix_socket_peer_is_always_trusted() {
    let mut interceptor = local_interceptor();
    let out = interceptor.call(Request::new(())).unwrap();
    assert!(trusted(out));
}
