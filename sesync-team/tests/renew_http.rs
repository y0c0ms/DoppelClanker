//! Renewal over HTTP: the token renews for its own machine inside the window, and the route
//! refuses an expired-window enrolment, a token from another machine, and no token.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sesync_team::server::{self, AppState, Clock};
use sesync_team::service::{MemoryStore, Service, RENEW_WINDOW_SECS};
use sesync_team::totp::code_at;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const T0: u64 = 1_000_020;
const DAY: u64 = 86_400;

fn app_at(now: Arc<Mutex<u64>>) -> (axum::Router, Arc<AppState<MemoryStore>>) {
    let mut svc = Service::new(MemoryStore::default());
    svc.create_space("personal", &["alice"]).unwrap();
    let clock: Clock = Arc::new(move || *now.lock().unwrap());
    let state = Arc::new(AppState { service: Mutex::new(svc), clock, metrics: Default::default(), admin_token: None });
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

#[tokio::test]
async fn an_expired_token_renews_inside_the_window_over_http() {
    let now = Arc::new(Mutex::new(T0 + 1));
    let (app, state) = app_at(now.clone());
    let code = code_at(&state.service.lock().unwrap().space_secret("personal").unwrap(), T0);
    let (_, j) = post(&app, "/enrol", None, serde_json::json!({"space":"personal","member":"alice","machine":"box-1","code":code})).await;
    let old = format!("Bearer {}", j["token"].as_str().unwrap());
    *now.lock().unwrap() = T0 + 2 * DAY; // the 24 h token has expired; the window has not
    let (st, j) = post(&app, "/renew", Some(&old), serde_json::json!({"space":"personal","machine":"box-1"})).await;
    assert_eq!(st, StatusCode::OK, "{j}");
    assert!(j["token"].is_string());
}

#[tokio::test]
async fn renewal_is_refused_after_the_window() {
    let now = Arc::new(Mutex::new(T0 + 1));
    let (app, state) = app_at(now.clone());
    let code = code_at(&state.service.lock().unwrap().space_secret("personal").unwrap(), T0);
    let (_, j) = post(&app, "/enrol", None, serde_json::json!({"space":"personal","member":"alice","machine":"box-1","code":code})).await;
    let old = format!("Bearer {}", j["token"].as_str().unwrap());
    *now.lock().unwrap() = T0 + RENEW_WINDOW_SECS + 1;
    let (st, _) = post(&app, "/renew", Some(&old), serde_json::json!({"space":"personal","machine":"box-1"})).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "the code is needed again");
}

#[tokio::test]
async fn renewal_needs_a_token_for_the_same_machine() {
    let now = Arc::new(Mutex::new(T0 + 1));
    let (app, state) = app_at(now.clone());
    let code = code_at(&state.service.lock().unwrap().space_secret("personal").unwrap(), T0);
    let (_, j) = post(&app, "/enrol", None, serde_json::json!({"space":"personal","member":"alice","machine":"box-1","code":code})).await;
    let old = format!("Bearer {}", j["token"].as_str().unwrap());
    let (st, _) = post(&app, "/renew", None, serde_json::json!({"space":"personal","machine":"box-1"})).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "no token, no renewal");
    let (st, _) = post(&app, "/renew", Some(&old), serde_json::json!({"space":"personal","machine":"box-2"})).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "a token for box-1 cannot renew for box-2");
}
