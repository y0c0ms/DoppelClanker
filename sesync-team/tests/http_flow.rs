//! The real router, in-process, against the in-memory store: enrol, lease, write, read, list,
//! and every refusal the protocol promises. These run without a database or a network port.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sesync_team::server::{self, AppState, Clock};
use sesync_team::service::{MemoryStore, Service};
use sesync_team::totp::code_at;
use http_body_util::BodyExt;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const T0: u64 = 1_000_020;

fn app_at(now: u64) -> (axum::Router, Arc<AppState<MemoryStore>>) {
    let mut svc = Service::new(MemoryStore::default());
    svc.create_space("team-a", &["alice", "bob"]).unwrap();
    let clock: Clock = Arc::new(move || now);
    let state = Arc::new(AppState { service: Mutex::new(svc), clock, metrics: Default::default() });
    (server::router(state.clone()), state)
}

async fn send(app: &axum::Router, method: &str, uri: &str, auth: Option<&str>, body: Vec<u8>) -> (StatusCode, Vec<u8>) {
    let mut b = Request::builder().method(method).uri(uri).header("content-type", "application/json");
    if let Some(a) = auth { b = b.header("authorization", a); }
    let resp = app.clone().oneshot(b.body(Body::from(body)).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, bytes)
}

fn json(b: &[u8]) -> serde_json::Value { serde_json::from_slice(b).unwrap_or(serde_json::Value::Null) }

async fn enrol(app: &axum::Router, state: &Arc<AppState<MemoryStore>>, who: &str, machine: &str) -> String {
    let code = {
        let svc = state.service.lock().unwrap();
        code_at(&svc.space_secret("team-a").unwrap(), T0)
    };
    let body = serde_json::json!({"space":"team-a","member":who,"machine":machine,"code":code});
    let (st, b) = send(app, "POST", "/enrol", None, body.to_string().into_bytes()).await;
    assert_eq!(st, StatusCode::OK, "enrol: {}", String::from_utf8_lossy(&b));
    format!("Bearer {}", json(&b)["token"].as_str().unwrap())
}

#[tokio::test]
async fn full_flow_enrol_lease_write_read_list() {
    let (app, state) = app_at(T0 + 1);
    let auth = enrol(&app, &state, "alice", "box-1").await;

    let lease = serde_json::json!({"space":"team-a","conversation":"ses_1","machine":"box-1"});
    let (st, b) = send(&app, "POST", "/lease/acquire", Some(&auth), lease.to_string().into_bytes()).await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&b));
    let version = json(&b)["version"].as_u64().unwrap();

    let uri = format!("/record?space=team-a&conversation=ses_1&machine=box-1&version={version}");
    let session = b"{\"type\":\"user\",\"text\":\"hello\"}\n".to_vec();
    let (st, b) = send(&app, "POST", &uri, Some(&auth), session.clone()).await;
    assert_eq!(st, StatusCode::OK, "write: {}", String::from_utf8_lossy(&b));

    let read = serde_json::json!({"space":"team-a","conversation":"ses_1","machine":"box-1"});
    let (st, b) = send(&app, "POST", "/record/read", Some(&auth), read.to_string().into_bytes()).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(b, session, "the member reads back exactly what was written");

    let list = serde_json::json!({"space":"team-a","machine":"box-1"});
    let (st, b) = send(&app, "POST", "/sessions", Some(&auth), list.to_string().into_bytes()).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(json(&b)["conversations"], serde_json::json!(["ses_1"]));
}

#[tokio::test]
async fn a_write_without_the_lease_is_refused() {
    let (app, state) = app_at(T0 + 1);
    let auth = enrol(&app, &state, "alice", "box-1").await;
    let (st, _) = send(&app, "POST", "/record?space=team-a&conversation=ses_9&machine=box-1&version=1",
                       Some(&auth), b"x".to_vec()).await;
    assert_eq!(st, StatusCode::CONFLICT, "no lease, no write");
}

#[tokio::test]
async fn an_unauthenticated_write_is_401_and_stores_nothing() {
    let (app, _state) = app_at(T0 + 1);
    let (st, _) = send(&app, "POST", "/record?space=team-a&conversation=ses_1&machine=box-1&version=1", None, b"x".to_vec()).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_read_by_a_non_member_is_refused() {
    let (app, state) = app_at(T0 + 1);
    let auth = enrol(&app, &state, "alice", "box-1").await;
    let lease = serde_json::json!({"space":"team-a","conversation":"ses_1","machine":"box-1"});
    let (_, b) = send(&app, "POST", "/lease/acquire", Some(&auth), lease.to_string().into_bytes()).await;
    let v = json(&b)["version"].as_u64().unwrap();
    send(&app, "POST", &format!("/record?space=team-a&conversation=ses_1&machine=box-1&version={v}"), Some(&auth), b"secret".to_vec()).await;
    // A token for a machine that never enrolled cannot read.
    let (st, _) = send(&app, "POST", "/record/read", Some("Bearer not-a-token"),
        serde_json::json!({"space":"team-a","conversation":"ses_1","machine":"box-9"}).to_string().into_bytes()).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_expired_token_is_refused() {
    let (app, state) = app_at(T0 + 1);
    let auth = enrol(&app, &state, "alice", "box-1").await;
    let token = auth.trim_start_matches("Bearer ").to_string();
    let svc = state.service.lock().unwrap();
    assert!(svc.authenticate("team-a", &token, "box-1", T0 + 1).is_ok(), "valid now");
    assert!(svc.authenticate("team-a", &token, "box-1", T0 + 1 + 86_401).is_err(), "expired after 24 h");
}

#[tokio::test]
async fn an_oversized_control_message_is_refused() {
    let (app, _state) = app_at(T0 + 1);
    let big = vec![b'a'; server::MAX_CONTROL_BODY + 1];
    let (st, _) = send(&app, "POST", "/lease/acquire", None, big).await;
    assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE);
}
