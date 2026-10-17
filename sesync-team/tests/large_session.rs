//! Large sessions through the real HTTP write and read routes. A coding session can be several
//! megabytes; it must round-trip byte for byte, and a session over the cap must be refused
//! before it is stored.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sesync_team::server::{self, AppState, Clock, MAX_SESSION_BODY};
use sesync_team::service::{MemoryStore, Service};
use sesync_team::totp::code_at;
use http_body_util::BodyExt;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const T0: u64 = 1_000_020;

fn make_app() -> (axum::Router, Arc<AppState<MemoryStore>>) {
    let mut svc = Service::new(MemoryStore::default());
    svc.create_space("team-a", &["alice"]).unwrap();
    let clock: Clock = Arc::new(|| T0 + 1);
    let state = Arc::new(AppState { service: Mutex::new(svc), clock, metrics: Default::default() });
    (server::router(state.clone()), state)
}

async fn call(app: &axum::Router, method: &str, uri: &str, auth: Option<&str>, body: Vec<u8>) -> (StatusCode, Vec<u8>) {
    let mut b = Request::builder().method(method).uri(uri).header("content-type", "application/json");
    if let Some(a) = auth { b = b.header("authorization", a); }
    let resp = app.clone().oneshot(b.body(Body::from(body)).unwrap()).await.unwrap();
    let st = resp.status();
    (st, resp.into_body().collect().await.unwrap().to_bytes().to_vec())
}

async fn logged_in(app: &axum::Router, state: &Arc<AppState<MemoryStore>>) -> String {
    let code = code_at(&state.service.lock().unwrap().space_secret("team-a").unwrap(), T0);
    let body = serde_json::json!({"space":"team-a","member":"alice","machine":"box-1","code":code}).to_string();
    let (st, b) = call(app, "POST", "/enrol", None, body.into_bytes()).await;
    assert_eq!(st, StatusCode::OK);
    let j: serde_json::Value = serde_json::from_slice(&b).unwrap();
    format!("Bearer {}", j["token"].as_str().unwrap())
}

/// A deterministic multi-megabyte payload that is not compressible to nothing, so a truncated
/// or mangled copy cannot pass by accident.
fn session_bytes(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 2654435761usize) >> 13) as u8).collect()
}

#[tokio::test]
async fn a_five_megabyte_session_round_trips_byte_for_byte() {
    let (app, state) = make_app();
    let auth = logged_in(&app, &state).await;
    let lease = serde_json::json!({"space":"team-a","conversation":"big","machine":"box-1"}).to_string();
    let (_, b) = call(&app, "POST", "/lease/acquire", Some(&auth), lease.into_bytes()).await;
    let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
    let version = v["version"].as_u64().unwrap();

    let data = session_bytes(5 * 1024 * 1024);
    let uri = format!("/record?space=team-a&conversation=big&machine=box-1&version={version}");
    let (st, body) = call(&app, "POST", &uri, Some(&auth), data.clone()).await;
    assert_eq!(st, StatusCode::OK, "write: {}", String::from_utf8_lossy(&body));

    let read = serde_json::json!({"space":"team-a","conversation":"big","machine":"box-1"}).to_string();
    let (st, back) = call(&app, "POST", "/record/read", Some(&auth), read.into_bytes()).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(back.len(), data.len(), "same length");
    assert!(back == data, "same bytes");
}

#[tokio::test]
async fn a_session_over_the_cap_is_refused_and_nothing_is_stored() {
    let (app, state) = make_app();
    let auth = logged_in(&app, &state).await;
    let lease = serde_json::json!({"space":"team-a","conversation":"huge","machine":"box-1"}).to_string();
    let (_, b) = call(&app, "POST", "/lease/acquire", Some(&auth), lease.into_bytes()).await;
    let version = serde_json::from_slice::<serde_json::Value>(&b).unwrap()["version"].as_u64().unwrap();
    let too_big = vec![0u8; MAX_SESSION_BODY + 1];
    let uri = format!("/record?space=team-a&conversation=huge&machine=box-1&version={version}");
    let (st, _) = call(&app, "POST", &uri, Some(&auth), too_big).await;
    assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(state.service.lock().unwrap().raw_stored("team-a", "huge").is_none(), "nothing stored");
}
