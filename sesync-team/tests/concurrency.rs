//! Two machines race for one conversation's lease over HTTP. Exactly one must win; the other
//! must be refused with 409 while the first holds it, and must win only after the first
//! releases. Many rounds, so a single lucky ordering cannot hide a race.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sesync_team::server::{self, AppState, Clock};
use sesync_team::service::{MemoryStore, Service};
use sesync_team::totp::code_at;
use http_body_util::BodyExt;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const T0: u64 = 1_000_020;

fn make_app() -> (axum::Router, Arc<AppState<MemoryStore>>) {
    let mut svc = Service::new(MemoryStore::default());
    svc.create_space("team-a", &["alice", "bob"]).unwrap();
    let clock: Clock = Arc::new(|| T0 + 1);
    let state = Arc::new(AppState { service: Mutex::new(svc), clock, metrics: Default::default() });
    (server::router(state.clone()), state)
}

async fn post(app: &axum::Router, uri: &str, auth: Option<&str>, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let mut b = Request::builder().method("POST").uri(uri).header("content-type", "application/json");
    if let Some(a) = auth { b = b.header("authorization", a); }
    let resp = app.clone().oneshot(b.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let st = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (st, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
}

async fn enrol(app: &axum::Router, state: &Arc<AppState<MemoryStore>>, who: &str, machine: &str) -> String {
    let code = code_at(&state.service.lock().unwrap().space_secret("team-a").unwrap(), T0);
    let (st, j) = post(app, "/enrol", None,
        serde_json::json!({"space":"team-a","member":who,"machine":machine,"code":code})).await;
    assert_eq!(st, StatusCode::OK);
    format!("Bearer {}", j["token"].as_str().unwrap())
}

#[tokio::test]
async fn two_machines_race_for_one_lease_and_exactly_one_wins() {
    for round in 0..50 {
        let (app, state) = make_app();
        let a = enrol(&app, &state, "alice", "box-a").await;
        let b = enrol(&app, &state, "bob", "box-b").await;
        let conv = format!("ses_{round}");
        let req_a = serde_json::json!({"space":"team-a","conversation":conv,"machine":"box-a"});
        let req_b = serde_json::json!({"space":"team-a","conversation":conv,"machine":"box-b"});

        // Fire both acquisitions at once.
        let (ra, rb) = tokio::join!(
            post(&app, "/lease/acquire", Some(&a), req_a),
            post(&app, "/lease/acquire", Some(&b), req_b),
        );
        let wins = [ra.0 == StatusCode::OK, rb.0 == StatusCode::OK].iter().filter(|w| **w).count();
        assert_eq!(wins, 1, "round {round}: exactly one machine must hold the lease ({:?} {:?})", ra.0, rb.0);
        let loser_status = if ra.0 == StatusCode::OK { rb.0 } else { ra.0 };
        assert_eq!(loser_status, StatusCode::CONFLICT, "round {round}: the loser is refused with 409");
    }
}

#[tokio::test]
async fn the_loser_wins_only_after_the_holder_releases() {
    let (app, state) = make_app();
    let a = enrol(&app, &state, "alice", "box-a").await;
    let b = enrol(&app, &state, "bob", "box-b").await;
    let (st, _) = post(&app, "/lease/acquire", Some(&a),
        serde_json::json!({"space":"team-a","conversation":"c","machine":"box-a"})).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = post(&app, "/lease/acquire", Some(&b),
        serde_json::json!({"space":"team-a","conversation":"c","machine":"box-b"})).await;
    assert_eq!(st, StatusCode::CONFLICT, "bob is refused while alice holds it");
    let (st, _) = post(&app, "/lease/release", Some(&a),
        serde_json::json!({"space":"team-a","conversation":"c","machine":"box-a"})).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = post(&app, "/lease/acquire", Some(&b),
        serde_json::json!({"space":"team-a","conversation":"c","machine":"box-b"})).await;
    assert_eq!(st, StatusCode::OK, "bob wins once alice releases");
}
