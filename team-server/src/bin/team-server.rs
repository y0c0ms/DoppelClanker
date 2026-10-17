//! The team server binary. Reads its settings from the environment, so no secret or address
//! is written into the code or the repo:
//!   TEAM_SERVER_BIND     address to listen on, e.g. 127.0.0.1:8080 (required)
//!   TEAM_SERVER_DATABASE Postgres connection string (required)
//!   TEAM_MASTER_KEY_FILE path to the 32-byte master key file (required)
//!
//! Startup is synchronous (the Postgres client starts its own runtime), then the async server
//! runs. SIGINT and SIGTERM stop accepting new connections and let in-flight requests finish.

use harness_sync_team::pg::PgStore;
use harness_sync_team::server::{self, AppState};
use harness_sync_team::service::Service;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        eprintln!("team-server: {name} is not set");
        std::process::exit(2);
    })
}

/// One structured log line per event: `ts level event key=value ...`. No plaintext, no secrets.
pub fn log(level: &str, event: &str, fields: &[(&str, String)]) {
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs()).unwrap_or(0);
    let kv: String = fields.iter().map(|(k, v)| format!(" {k}={v}")).collect();
    eprintln!("ts={ts} level={level} event={event}{kv}");
}

fn main() {
    let bind = required("TEAM_SERVER_BIND");
    let db = required("TEAM_SERVER_DATABASE");
    let key_file = required("TEAM_MASTER_KEY_FILE");
    let raw = std::fs::read(&key_file).unwrap_or_else(|e| {
        log("error", "key_unreadable", &[("reason", e.to_string())]);
        std::process::exit(2);
    });
    let master: [u8; 32] = raw.as_slice().try_into().unwrap_or_else(|_| {
        log("error", "key_wrong_length", &[]);
        std::process::exit(2);
    });
    // Retry at startup: the database may not be reachable for the first seconds (network
    // policy programming). About 1 + 2 + 3 + 4 + 5 s of waiting before giving up.
    let store = match PgStore::connect_with_retry(&db, master, 5, Duration::from_secs(1)) {
        Ok(s) => s,
        Err(e) => {
            log("error", "database_unavailable", &[("reason", e)]);
            std::process::exit(1);
        }
    };
    if let Err(e) = store.migrate() {
        log("error", "schema_unavailable", &[("reason", e)]);
        std::process::exit(1);
    }
    log("info", "started", &[("bind", bind.clone())]);
    let state = Arc::new(AppState { service: Mutex::new(Service::new(store)), clock: server::system_clock(),
                                    metrics: Default::default() });
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime")
        .block_on(async move {
            let listener = tokio::net::TcpListener::bind(&bind).await.expect("bind");
            axum::serve(listener, server::router(state))
                .with_graceful_shutdown(shutdown_signal())
                .await
                .expect("serve");
            log("info", "stopped", &[]);
        });
}

/// Resolves when SIGINT or SIGTERM arrives. Stopping new connections and draining the
/// in-flight ones is what makes a rollout safe.
async fn shutdown_signal() {
    let ctrl_c = async { tokio::signal::ctrl_c().await.ok(); };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
    log("info", "shutdown_requested", &[]);
}
