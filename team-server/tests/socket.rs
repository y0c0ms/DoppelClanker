//! The router over a real TCP socket, on a random local port: the same path a machine takes
//! over the network. Uses the in-memory store, so it needs no database.

use harness_sync_team::server::{self, AppState, Clock};
use harness_sync_team::service::{MemoryStore, Service};
use harness_sync_team::totp::code_at;
use std::sync::{Arc, Mutex};

const T0: u64 = 1_000_020;

async fn start() -> (String, Arc<AppState<MemoryStore>>, tokio::task::JoinHandle<()>) {
    let mut svc = Service::new(MemoryStore::default());
    svc.create_space("team-a", &["alice"]).unwrap();
    let clock: Clock = Arc::new(|| T0 + 1);
    let state = Arc::new(AppState { service: Mutex::new(svc), clock, metrics: Default::default() });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = server::router(state.clone());
    let h = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    (format!("http://{addr}"), state, h)
}

#[tokio::test]
async fn enrol_write_and_read_over_a_real_socket() {
    let (base, state, server) = start().await;
    let client = reqwest_like();
    let code = code_at(&state.service.lock().unwrap().space_secret("team-a").unwrap(), T0);
    let enrol = client.post(&format!("{base}/enrol"),
        &format!(r#"{{"space":"team-a","member":"alice","machine":"box-1","code":"{code}"}}"#), None).await;
    assert_eq!(enrol.0, 200, "enrol over the socket");
    let token = serde_json::from_str::<serde_json::Value>(&enrol.1).unwrap()["token"].as_str().unwrap().to_string();
    let auth = format!("Bearer {token}");

    let lease = client.post(&format!("{base}/lease/acquire"),
        r#"{"space":"team-a","conversation":"sock","machine":"box-1"}"#, Some(&auth)).await;
    assert_eq!(lease.0, 200);
    let version = serde_json::from_str::<serde_json::Value>(&lease.1).unwrap()["version"].as_u64().unwrap();

    let data = b"{\"over\":\"the wire\"}".to_vec();
    let write = client.post_bytes(&format!("{base}/record?space=team-a&conversation=sock&machine=box-1&version={version}"),
        data.clone(), Some(&auth)).await;
    assert_eq!(write.0, 200, "write over the socket");

    let read = client.post(&format!("{base}/record/read"),
        r#"{"space":"team-a","conversation":"sock","machine":"box-1"}"#, Some(&auth)).await;
    assert_eq!(read.0, 200);
    assert_eq!(read.1.as_bytes(), data.as_slice(), "the bytes come back over the socket unchanged");
    server.abort();
}

/// Minimal HTTP/1.1 client over a raw socket, so the test needs no client dependency.
struct Client;
fn reqwest_like() -> Client { Client }
impl Client {
    async fn post(&self, url: &str, body: &str, auth: Option<&str>) -> (u16, String) {
        let (st, b) = self.raw(url, body.as_bytes().to_vec(), auth).await;
        (st, String::from_utf8_lossy(&b).into_owned())
    }
    async fn post_bytes(&self, url: &str, body: Vec<u8>, auth: Option<&str>) -> (u16, String) {
        let (st, b) = self.raw(url, body, auth).await;
        (st, String::from_utf8_lossy(&b).into_owned())
    }
    async fn raw(&self, url: &str, body: Vec<u8>, auth: Option<&str>) -> (u16, Vec<u8>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let rest = url.trim_start_matches("http://");
        let (hostport, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap();
        let mut s = tokio::net::TcpStream::connect(hostport).await.unwrap();
        let auth_line = auth.map(|a| format!("Authorization: {a}\r\n")).unwrap_or_default();
        let head = format!("POST {path} HTTP/1.1\r\nHost: {hostport}\r\nContent-Type: application/json\r\n\
                            Content-Length: {}\r\n{auth_line}Connection: close\r\n\r\n", body.len());
        s.write_all(head.as_bytes()).await.unwrap();
        s.write_all(&body).await.unwrap();
        let mut resp = Vec::new();
        s.read_to_end(&mut resp).await.unwrap();
        let split = resp.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let status: u16 = std::str::from_utf8(&resp[9..12]).unwrap().parse().unwrap();
        (status, resp[split + 4..].to_vec())
    }
}
