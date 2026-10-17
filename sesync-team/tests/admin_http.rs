//! The admin routes over HTTP. Admin only: a member token, or no token, is refused. The
//! operator's admin token creates a team once and invites members.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sesync_team::server::{self, constant_time_eq, AppState, Clock};
use sesync_team::service::{MemoryStore, Service};
use sesync_team::totp::code_at;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const T0: u64 = 1_000_020;
const ADMIN: &str = "admin-token-for-tests-0123456789abcdef";

fn app(admin: Option<&str>) -> (axum::Router, Arc<AppState<MemoryStore>>) {
    let svc = Service::new(MemoryStore::default());
    let clock: Clock = Arc::new(|| T0 + 1);
    let state = Arc::new(AppState {
        service: Mutex::new(svc), clock, metrics: Default::default(),
        admin_token: admin.map(str::to_string),
    });
    (server::router(state.clone()), state)
}

async fn call(app: &axum::Router, uri: &str, auth: Option<&str>, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let mut b = Request::builder().method("POST").uri(uri).header("content-type", "application/json");
    if let Some(a) = auth { b = b.header("authorization", a); }
    let resp = app.clone().oneshot(b.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let st = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (st, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
}

#[tokio::test]
async fn creating_a_team_needs_the_admin_token() {
    let (app, _) = app(Some(ADMIN));
    let body = serde_json::json!({"space":"personal","members":["alice"]});
    let (st, _) = call(&app, "/admin/team", None, body.clone()).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "no token");
    let (st, _) = call(&app, "/admin/team", Some("Bearer wrong"), body.clone()).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "wrong token");
}

#[tokio::test]
async fn a_member_token_cannot_create_or_invite() {
    let (app, state) = app(Some(ADMIN));
    let (st, _) = call(&app, "/admin/team", Some(&format!("Bearer {ADMIN}")),
                       serde_json::json!({"space":"personal","members":["alice"]})).await;
    assert_eq!(st, StatusCode::OK);
    // A member enrols with the team's code, and gets a member token.
    let code = code_at(&state.service.lock().unwrap().space_secret("personal").unwrap(), T0);
    let (st, j) = call(&app, "/enrol", None,
        serde_json::json!({"space":"personal","member":"alice","machine":"box-1","code":code})).await;
    assert_eq!(st, StatusCode::OK);
    let member = format!("Bearer {}", j["token"].as_str().unwrap());
    let (st, _) = call(&app, "/admin/invite", Some(&member),
                       serde_json::json!({"space":"personal","member":"bob"})).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "a member token must not invite");
}

#[tokio::test]
async fn the_admin_creates_a_team_once_and_gets_the_uri_once() {
    let (app, _) = app(Some(ADMIN));
    let auth = format!("Bearer {ADMIN}");
    let body = serde_json::json!({"space":"personal","members":["alice"]});
    let (st, j) = call(&app, "/admin/team", Some(&auth), body.clone()).await;
    assert_eq!(st, StatusCode::OK);
    assert!(j["otpauth_uri"].as_str().unwrap().starts_with("otpauth://totp/sesync:personal?secret="));
    let (st, _) = call(&app, "/admin/team", Some(&auth), body).await;
    assert_ne!(st, StatusCode::OK, "a second create is refused");
}

#[tokio::test]
async fn with_no_admin_token_configured_every_admin_call_is_refused() {
    let (app, _) = app(None);
    let (st, _) = call(&app, "/admin/team", Some("Bearer anything-at-all-0123456789abcdef"),
                       serde_json::json!({"space":"personal","members":["alice"]})).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "admin disabled means refused");
}

#[test]
fn the_admin_comparison_is_exact() {
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(!constant_time_eq(b"abc", b"abcd"));
    assert!(!constant_time_eq(b"", b"x"));
}
