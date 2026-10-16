//! The team server binary. Reads its settings from the environment, so no secret or address
//! is written into the code or the repo:
//!   TEAM_SERVER_BIND     address to listen on, e.g. 127.0.0.1:8080 (required)
//!   TEAM_SERVER_DATABASE Postgres connection string (required)
//!   TEAM_MASTER_KEY_FILE path to the 32-byte master key file (required)

use axum::{body::Bytes, extract::State, http::{HeaderMap, StatusCode}, response::IntoResponse,
           routing::post, Json, Router};
use harness_sync_team::http::{self, EnrolBody, LeaseBody};
use harness_sync_team::pg::PgStore;
use harness_sync_team::service::Service;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Refuse request bodies over 64 KiB. Team messages are small; anything larger is abuse.
const MAX_BODY: usize = 64 * 1024;

type Shared = Arc<Mutex<Service<PgStore>>>;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn respond(pair: (u16, Value)) -> axum::response::Response {
    let status = StatusCode::from_u16(pair.0).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(pair.1)).into_response()
}

fn parse<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, axum::response::Response> {
    if body.len() > MAX_BODY {
        return Err(respond((413, serde_json::json!({"error": "request too large"}))));
    }
    serde_json::from_slice(body).map_err(|_| respond((400, serde_json::json!({"error": "bad request"}))))
}

fn auth_header(h: &HeaderMap) -> Option<String> {
    h.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).map(str::to_string)
}

/// Run a database-touching handler on a blocking thread. The sync Postgres client must not run
/// on a tokio worker, where it would try to start a runtime inside one and panic.
async fn blocking<F>(s: Shared, f: F) -> axum::response::Response
where
    F: FnOnce(&mut Service<PgStore>) -> (u16, Value) + Send + 'static,
{
    match tokio::task::spawn_blocking(move || {
        let mut svc = s.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut svc)
    }).await {
        Ok(pair) => respond(pair),
        Err(_) => respond((500, serde_json::json!({"error": "internal error"}))),
    }
}

async fn enrol(State(s): State<Shared>, body: Bytes) -> axum::response::Response {
    let body: EnrolBody = match parse(&body) { Ok(b) => b, Err(r) => return r };
    blocking(s, move |svc| http::enrol(svc, &body, now())).await
}

async fn acquire(State(s): State<Shared>, headers: HeaderMap, body: Bytes) -> axum::response::Response {
    let body: LeaseBody = match parse(&body) { Ok(b) => b, Err(r) => return r };
    let auth = auth_header(&headers);
    blocking(s, move |svc| http::acquire(svc, auth.as_deref(), &body, now())).await
}

async fn healthz(State(s): State<Shared>) -> axum::response::Response {
    blocking(s, |svc| http::health(svc)).await
}

async fn release(State(s): State<Shared>, headers: HeaderMap, body: Bytes) -> axum::response::Response {
    let body: LeaseBody = match parse(&body) { Ok(b) => b, Err(r) => return r };
    let auth = auth_header(&headers);
    blocking(s, move |svc| http::release(svc, auth.as_deref(), &body, now())).await
}

/// Listing sessions. The machine id travels in the body, not the URL.
async fn sessions(State(s): State<Shared>, headers: HeaderMap, body: Bytes) -> axum::response::Response {
    #[derive(serde::Deserialize)]
    struct Body { space: String, machine: String }
    let body: Body = match parse(&body) { Ok(b) => b, Err(r) => return r };
    let auth = auth_header(&headers);
    blocking(s, move |svc| http::list_sessions(svc, auth.as_deref(), &body.space, &body.machine, now())).await
}

fn app(state: Shared) -> Router {
    Router::new()
        .route("/enrol", post(enrol))
        .route("/lease/acquire", post(acquire))
        .route("/lease/release", post(release))
        .route("/sessions", post(sessions))
        .route("/healthz", axum::routing::get(healthz))
        .with_state(state)
}

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        eprintln!("team-server: {name} is not set");
        std::process::exit(2);
    })
}

/// Set up the store and the bind address before any async runtime exists. The sync Postgres
/// client starts its own runtime, which must not run inside tokio.
fn main() {
    let bind = required("TEAM_SERVER_BIND");
    let db = required("TEAM_SERVER_DATABASE");
    let key_file = required("TEAM_MASTER_KEY_FILE");
    let raw = std::fs::read(&key_file).unwrap_or_else(|e| {
        eprintln!("team-server: cannot read master key file: {e}");
        std::process::exit(2);
    });
    let master: [u8; 32] = raw.as_slice().try_into().unwrap_or_else(|_| {
        eprintln!("team-server: master key must be exactly 32 bytes");
        std::process::exit(2);
    });
    // Retry at startup: the database may not be reachable for the first seconds (network
    // policy programming). About 1 + 2 + 3 + 4 s of waiting before giving up.
    let store = match PgStore::connect_with_retry(&db, master, 5, std::time::Duration::from_secs(1)) {
        Ok(s) => s,
        Err(e) => { eprintln!("team-server: database unavailable: {e}"); std::process::exit(1); }
    };
    let state: Shared = Arc::new(Mutex::new(Service::new(store)));
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(async move {
            let listener = tokio::net::TcpListener::bind(&bind).await.expect("bind");
            axum::serve(listener, app(state)).await.expect("serve");
        });
}
