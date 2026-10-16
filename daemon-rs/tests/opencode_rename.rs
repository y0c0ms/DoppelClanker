//! Renaming an OpenCode session through its HTTP API (PATCH /session/{id}), ported from
//! daemon/hs_manage.py. Issue #9: a rename must reuse an `opencode serve` that is already
//! running, and must stop only a server it started itself.
//! A small fake HTTP server stands in for `opencode serve`, so no real store is touched.

use harness_sync::opencode_rename::{rename_session, RenameError};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// A fake OpenCode server: answers GET /session and PATCH /session/<id>, records requests.
struct Fake {
    port: u16,
    patched: Arc<Mutex<Vec<(String, String)>>>,
}

fn fake_server(known_id: &str) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let patched = Arc::new(Mutex::new(Vec::new()));
    let log = patched.clone();
    let known = known_id.to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() { continue; }
            let mut parts = line.split_whitespace();
            let method = parts.next().unwrap_or("").to_string();
            let path = parts.next().unwrap_or("").to_string();
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" { break; }
                if let Some(v) = h.to_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; len];
            let _ = reader.read_exact(&mut body);
            let (status, reply) = if method == "GET" && path == "/session" {
                ("200 OK", "[]".to_string())
            } else if method == "PATCH" && path.starts_with("/session/") {
                let id = path.trim_start_matches("/session/").to_string();
                let title = serde_json::from_slice::<serde_json::Value>(&body).ok()
                    .and_then(|v| v["title"].as_str().map(str::to_string)).unwrap_or_default();
                if id == known {
                    log.lock().unwrap().push((id.clone(), title.clone()));
                    ("200 OK", format!("{{\"id\":\"{id}\",\"title\":\"{title}\"}}"))
                } else {
                    ("404 Not Found", "{}".to_string())
                }
            } else {
                ("404 Not Found", "{}".to_string())
            };
            let mut s = stream;
            let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
        }
    });
    Fake { port, patched }
}

const SID: &str = "ses_abc123def456ghi789jkl012mn";

#[test]
fn a_running_server_is_reused_and_not_stopped() {
    let fake = fake_server(SID);
    let saved = rename_session(SID, "New name", Some(fake.port), None)
        .expect("rename through the running server");
    assert_eq!(saved, "New name");
    assert_eq!(fake.patched.lock().unwrap().as_slice(), &[(SID.to_string(), "New name".to_string())]);
    // The server belongs to someone else: it must still answer after the rename.
    let still_up = std::net::TcpStream::connect(("127.0.0.1", fake.port)).is_ok();
    assert!(still_up, "a server we did not start must not be stopped by the rename");
}

#[test]
fn an_unknown_session_is_refused_with_a_clear_error() {
    let fake = fake_server(SID);
    match rename_session("ses_zzzzzzzzzzzzzzzzzzzzzzzzzz", "x", Some(fake.port), None) {
        Err(RenameError::NotFound(_)) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn a_bad_session_id_never_reaches_the_server() {
    let fake = fake_server(SID);
    assert!(matches!(rename_session("ses_x; rm -rf /", "x", Some(fake.port), None),
        Err(RenameError::BadId(_))));
    assert!(fake.patched.lock().unwrap().is_empty());
}

#[test]
fn an_empty_title_is_refused_before_any_request() {
    let fake = fake_server(SID);
    assert!(matches!(rename_session(SID, "   ", Some(fake.port), None), Err(RenameError::BadTitle(_))));
    assert!(fake.patched.lock().unwrap().is_empty());
}

#[test]
fn with_no_server_running_and_no_binary_it_fails_without_stopping_anything() {
    // Nothing listening on the port and no binary to start: a clear error, nothing killed.
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let r = rename_session(SID, "x", Some(port), Some(std::path::Path::new("/no/such/opencode")));
    assert!(r.is_err());
}
