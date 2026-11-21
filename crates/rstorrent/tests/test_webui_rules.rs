//! REST integration tests for the rules engine endpoints (`/api/rules`,
//! `/api/labels`, `/api/seed`, `/api/torrents/{hash}/seed-limits`).
//! Mirrors the RSS REST test pattern: real service + gated router + oneshot.

use std::sync::{Arc, Mutex, RwLock};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use rstorrent::rules::{RuleService, RulesSettings, SeedSettings};
use rstorrent::webui::WebuiState;
use rt_core::session::SessionManager;
use rt_core::DownloadList;
use rt_rpc::CommandMap;
use serde_json::{json, Value};
use tower::ServiceExt;

struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("rt_rules_rest_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn harness(dir: &TempDir, token: Option<&str>, with_service: bool) -> WebuiState {
    let list = Arc::new(RwLock::new(DownloadList::new()));
    let mut state = WebuiState::new(
        Arc::clone(&list),
        Arc::new(RwLock::new(CommandMap::new())),
        token.map(str::to_string),
        Arc::new(rt_core::tiering::TieringManager::new(Default::default())),
        Arc::new(RwLock::new(rt_core::commands::tracker::DhtConfig::default())),
        Arc::new(RwLock::new(Some(dir.path.join("rstorrent.toml")))),
        Arc::new(RwLock::new(None)),
        Arc::new(RwLock::new(rstorrent::webui::DaemonLifecycle::Ready)),
    );
    if with_service {
        let service = RuleService::new(
            dir.path.join("rules.json"),
            SeedSettings::default(),
            RulesSettings::default(),
            list,
            Arc::new(Mutex::new(SessionManager::new())),
            Arc::new(RwLock::new(CommandMap::new())),
            Arc::new(rt_core::tiering::TieringManager::new(Default::default())),
        )
        .unwrap();
        state = state.with_rules(Arc::new(RwLock::new(Some(service))));
    }
    state
}

async fn call(
    state: &WebuiState,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let request = match body {
        Some(value) => builder
            .header("content-type", "application/json")
            .body(Body::from(value.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = rstorrent::webui::router(state.clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

fn rule_json(name: &str) -> Value {
    json!({
        "id": 0,
        "name": name,
        "enabled": true,
        "priority": 0,
        "scope": "global",
        "trigger": "periodic",
        "cond_mode": "all",
        "conditions": [{"ratio_at_least": 2.0}],
        "actions": ["stop"],
        "once": false,
        "cooldown_secs": null,
        "allow_destructive": false
    })
}

#[tokio::test]
async fn rules_crud_roundtrip() {
    let dir = TempDir::new("crud");
    let state = harness(&dir, Some("secret"), true);

    let (status, created) = call(
        &state,
        "POST",
        "/api/rules",
        Some("secret"),
        Some(rule_json("cleanup")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["id"], 1);
    assert_eq!(created["name"], "cleanup");

    let (status, list) = call(&state, "GET", "/api/rules", Some("secret"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);

    let (status, got) = call(&state, "GET", "/api/rules/1", Some("secret"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["name"], "cleanup");

    let mut updated = rule_json("renamed");
    updated["priority"] = json!(-3);
    let (status, _) = call(&state, "PUT", "/api/rules/1", Some("secret"), Some(updated)).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = call(&state, "POST", "/api/rules/1/disable", Some("secret"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, got) = call(&state, "GET", "/api/rules/1", Some("secret"), None).await;
    assert_eq!(got["enabled"], false);
    assert_eq!(got["priority"], -3);

    let (status, _) = call(&state, "DELETE", "/api/rules/1", Some("secret"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(&state, "GET", "/api/rules/1", Some("secret"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn invalid_rule_is_rejected_without_mutation() {
    let dir = TempDir::new("invalid");
    let state = harness(&dir, Some("secret"), true);
    let mut bad = rule_json("bad");
    bad["conditions"] = json!([]);
    let (status, _) = call(&state, "POST", "/api/rules", Some("secret"), Some(bad)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, list) = call(&state, "GET", "/api/rules", Some("secret"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn dry_run_test_changes_nothing() {
    let dir = TempDir::new("dryrun");
    let state = harness(&dir, Some("secret"), true);
    let (_, before) = call(&state, "GET", "/api/rules/export", Some("secret"), None).await;
    let (status, result) = call(
        &state,
        "POST",
        "/api/rules/test",
        Some("secret"),
        Some(json!({ "rule": rule_json("probe"), "limit": 10 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(result["matches"].as_array().unwrap().is_empty());
    assert!(result["no_data"].as_array().unwrap().is_empty());
    let (_, after) = call(&state, "GET", "/api/rules/export", Some("secret"), None).await;
    assert_eq!(before, after);
    let (_, history) = call(&state, "GET", "/api/rules/history", Some("secret"), None).await;
    assert_eq!(history.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn labels_crud_and_policy() {
    let dir = TempDir::new("labels");
    let state = harness(&dir, Some("secret"), true);
    let label = json!({ "name": "movies", "color": "#7c3aed" });
    let (status, _) = call(
        &state,
        "POST",
        "/api/labels",
        Some("secret"),
        Some(label.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = call(&state, "POST", "/api/labels", Some("secret"), Some(label)).await;
    assert_eq!(status, StatusCode::CONFLICT);

    let policy = json!({ "ratio": {"set": 3.0}, "mode": "all" });
    let (status, _) = call(
        &state,
        "PUT",
        "/api/labels/movies/policy",
        Some("secret"),
        Some(policy),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, got) = call(
        &state,
        "GET",
        "/api/labels/movies/policy",
        Some("secret"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["ratio"], json!({"set": 3.0}));
    assert_eq!(got["mode"], "all");

    let (status, _) = call(&state, "DELETE", "/api/labels/movies", Some("secret"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(
        &state,
        "GET",
        "/api/labels/movies/policy",
        Some("secret"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn seed_settings_roundtrip() {
    let dir = TempDir::new("seed");
    let state = harness(&dir, Some("secret"), true);
    let (status, defaults) = call(&state, "GET", "/api/seed", Some("secret"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(defaults["seed"]["enabled"], false);
    assert_eq!(defaults["rules"]["allow_destructive"], false);

    let config = json!({
        "seed": {
            "enabled": true,
            "ratio": 3.0,
            "seeding_time_secs": 604800,
            "inactive_secs": 0,
            "mode": "any",
            "action": {"throttle": "seed"},
            "check_interval_secs": 15
        },
        "rules": { "enabled": true, "allow_destructive": true }
    });
    let (status, saved) = call(
        &state,
        "PUT",
        "/api/seed",
        Some("secret"),
        Some(json!({ "config": config, "save_file": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["seed"]["ratio"], 3.0);
    assert_eq!(saved["seed"]["action"], json!({"throttle": "seed"}));
    assert_eq!(saved["rules"]["allow_destructive"], true);

    let (_, again) = call(&state, "GET", "/api/seed", Some("secret"), None).await;
    assert_eq!(again["seed"]["seeding_time_secs"], 604800);
    assert_eq!(again["seed"]["check_interval_secs"], 15);

    // save_file persisted the sections into the TOML file.
    let content = std::fs::read_to_string(dir.path.join("rstorrent.toml")).unwrap();
    assert!(content.contains("[seed]"));
    assert!(content.contains("ratio = 3.0"));
    assert!(content.contains("[rules]"));

    let (status, rules_settings) = call(
        &state,
        "PUT",
        "/api/rules/settings",
        Some("secret"),
        Some(json!({ "enabled": false, "allow_destructive": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rules_settings["enabled"], false);
}

#[tokio::test]
async fn auth_gates() {
    let dir = TempDir::new("auth");
    // No token configured: caller stays untrusted -> 403 even on reads.
    let untrusted = harness(&dir, None, true);
    let (status, _) = call(&untrusted, "GET", "/api/rules", None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Token configured but missing/wrong -> 401.
    let trusted = harness(&dir, Some("secret"), true);
    let (status, _) = call(&trusted, "GET", "/api/rules", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = call(&trusted, "GET", "/api/rules", Some("wrong"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // No service -> 503 for trusted callers.
    let no_service = harness(&dir, Some("secret"), false);
    let (status, _) = call(&no_service, "GET", "/api/rules", Some("secret"), None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn seed_limits_unknown_torrent_is_404() {
    let dir = TempDir::new("limits");
    let state = harness(&dir, Some("secret"), true);
    let (status, _) = call(
        &state,
        "GET",
        "/api/torrents/0123456789abcdef0123456789abcdef01234567/seed-limits",
        Some("secret"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
