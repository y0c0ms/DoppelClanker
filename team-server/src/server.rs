//! The HTTP server, generic over the store so tests run the real routes in-process.
//!
//! Every route except `/enrol` and `/healthz` needs `Authorization: Bearer <token>`, bound to
//! the requesting machine. Database work runs on a blocking thread, so the synchronous Postgres
//! client never runs inside the async runtime.

use crate::http::{self, EnrolBody, LeaseBody};
use crate::service::{Service, Store};
use axum::{body::Bytes, extract::{DefaultBodyLimit, State}, http::{HeaderMap, StatusCode},
           response::{IntoResponse, Response}, routing::{get, post}, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// Largest JSON control message (enrol, lease, list). These are a few hundred bytes.
pub const MAX_CONTROL_BODY: usize = 16 * 1024;

/// Largest session upload. A long coding session can be several megabytes; this bounds the
/// memory one request can take while still fitting real sessions.
pub const MAX_SESSION_BODY: usize = 64 * 1024 * 1024;

/// The clock. Production passes the system time; tests pass a fixed or stepped time.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub struct AppState<S: Store + Send + 'static> {
    pub service: Mutex<Service<S>>,
    pub clock: Clock,
    pub metrics: crate::metrics::Metrics,
}

pub type Shared<S> = Arc<AppState<S>>;

pub fn system_clock() -> Clock {
    Arc::new(|| std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))
}

/// Record the outcome of a route in the metrics, then return the response unchanged.
fn counted<S: Store + Send + 'static>(s: &AppState<S>, route: &'static str, r: Response) -> Response {
    s.metrics.record(route, r.status().as_u16());
    r
}

fn respond(pair: (u16, Value)) -> Response {
    let status = StatusCode::from_u16(pair.0).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(pair.1)).into_response()
}

fn parse<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, Response> {
    serde_json::from_slice(body).map_err(|_| respond((400, json!({"error": "bad request"}))))
}

fn auth_header(h: &HeaderMap) -> Option<String> {
    h.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).map(str::to_string)
}

/// Run a service call on a blocking thread, with the clock read first.
async fn run<S, F>(state: Shared<S>, f: F) -> Response
where
    S: Store + Send + 'static,
    F: FnOnce(&mut Service<S>, u64) -> (u16, Value) + Send + 'static,
{
    let now = (state.clock)();
    match tokio::task::spawn_blocking(move || {
        let mut svc = state.service.lock().unwrap_or_else(|p| p.into_inner());
        f(&mut svc, now)
    }).await {
        Ok(pair) => respond(pair),
        Err(_) => respond((500, json!({"error": "internal error"}))),
    }
}

async fn healthz<S: Store + Send + 'static>(State(s): State<Shared<S>>) -> Response {
    let r = run(s.clone(), |svc, _| http::health(svc)).await;
    counted(&s, "healthz", r)
}

/// Prometheus text. No auth: it carries counts only, never user data or identifiers.
async fn metrics_route<S: Store + Send + 'static>(State(s): State<Shared<S>>) -> Response {
    (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")], s.metrics.render())
        .into_response()
}

async fn enrol<S: Store + Send + 'static>(State(s): State<Shared<S>>, body: Bytes) -> Response {
    let body: EnrolBody = match parse(&body) { Ok(b) => b, Err(r) => return r };
    run(s, move |svc, now| http::enrol(svc, &body, now)).await
}

async fn acquire<S: Store + Send + 'static>(State(s): State<Shared<S>>, headers: HeaderMap, body: Bytes) -> Response {
    let body: LeaseBody = match parse(&body) { Ok(b) => b, Err(r) => return r };
    let auth = auth_header(&headers);
    run(s, move |svc, now| http::acquire(svc, auth.as_deref(), &body, now)).await
}

async fn release<S: Store + Send + 'static>(State(s): State<Shared<S>>, headers: HeaderMap, body: Bytes) -> Response {
    let body: LeaseBody = match parse(&body) { Ok(b) => b, Err(r) => return r };
    let auth = auth_header(&headers);
    run(s, move |svc, now| http::release(svc, auth.as_deref(), &body, now)).await
}

#[derive(Deserialize)]
struct ListBody { space: String, machine: String }

async fn sessions<S: Store + Send + 'static>(State(s): State<Shared<S>>, headers: HeaderMap, body: Bytes) -> Response {
    let body: ListBody = match parse(&body) { Ok(b) => b, Err(r) => return r };
    let auth = auth_header(&headers);
    run(s, move |svc, now| http::list_sessions(svc, auth.as_deref(), &body.space, &body.machine, now)).await
}

/// Write one session. The caller must hold the live lease on the conversation, with the
/// version it was granted. The body is the session bytes, raw.
#[derive(Deserialize)]
struct WriteQuery { space: String, conversation: String, machine: String, version: u64 }

async fn write_record<S: Store + Send + 'static>(State(s): State<Shared<S>>, headers: HeaderMap,
                                                 axum::extract::Query(q): axum::extract::Query<WriteQuery>,
                                                 body: Bytes) -> Response {
    let auth = auth_header(&headers);
    let plain = body.to_vec();
    run(s, move |svc, now| http::write_record(svc, auth.as_deref(), &q.space, &q.conversation,
                                              &q.machine, q.version, plain, now)).await
}

#[derive(Deserialize)]
struct ReadBody { space: String, conversation: String, machine: String }

/// Read one session. Members only; returns the plaintext to the member who asks.
async fn read_record<S: Store + Send + 'static>(State(s): State<Shared<S>>, headers: HeaderMap, body: Bytes) -> Response {
    let body: ReadBody = match parse(&body) { Ok(b) => b, Err(r) => return r };
    let auth = auth_header(&headers);
    let (status, bytes) = {
        let now = (s.clock)();
        let state = s.clone();
        match tokio::task::spawn_blocking(move || {
            let svc = state.service.lock().unwrap_or_else(|p| p.into_inner());
            http::read_record(&svc, auth.as_deref(), &body.space, &body.conversation, &body.machine, now)
        }).await {
            Ok(pair) => pair,
            Err(_) => (500, Err(json!({"error": "internal error"}))),
        }
    };
    match bytes {
        Ok(plain) => (StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
                      [(axum::http::header::CONTENT_TYPE, "application/octet-stream")], plain).into_response(),
        Err(err) => respond((status, err)),
    }
}

/// The router. Control routes take small JSON bodies; the write route takes a large raw body.
pub fn router<S: Store + Send + 'static>(state: Shared<S>) -> Router {
    Router::new()
        .route("/healthz", get(healthz::<S>))
        .route("/metrics", get(metrics_route::<S>))
        .route("/enrol", post(enrol::<S>).layer(DefaultBodyLimit::max(MAX_CONTROL_BODY)))
        .route("/lease/acquire", post(acquire::<S>).layer(DefaultBodyLimit::max(MAX_CONTROL_BODY)))
        .route("/lease/release", post(release::<S>).layer(DefaultBodyLimit::max(MAX_CONTROL_BODY)))
        .route("/sessions", post(sessions::<S>).layer(DefaultBodyLimit::max(MAX_CONTROL_BODY)))
        .route("/record/read", post(read_record::<S>).layer(DefaultBodyLimit::max(MAX_CONTROL_BODY)))
        .route("/record", post(write_record::<S>).layer(DefaultBodyLimit::max(MAX_SESSION_BODY)))
        .with_state(state)
}
