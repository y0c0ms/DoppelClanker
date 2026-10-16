//! End to end: the daemon's HTTP client lists a space's sessions from a running team server.
//! Runs only when HS_E2E_TEAM_URL and HS_E2E_TOKEN_FILE are set (see the e2e run notes).

use harness_sync::remote::RemoteSessions;
use harness_sync::remote_http::HttpRemote;

#[test]
fn the_real_client_lists_sessions_from_a_running_server() {
    let Some(url) = std::env::var("HS_E2E_TEAM_URL").ok() else { return };
    let token = std::fs::read_to_string(std::env::var("HS_E2E_TOKEN_FILE").unwrap()).unwrap();
    let client = HttpRemote::new(url, token.trim().to_string(), "box-1".into());
    let sessions = client.list("team-e2e").expect("list sessions from the server");
    let ids: Vec<String> = sessions.iter().map(|s| s.id.clone()).collect();
    assert_eq!(ids, vec!["ses_a".to_string(), "ses_b".to_string()]);
}

#[test]
fn the_real_client_reports_an_unreachable_server() {
    use harness_sync::remote::RemoteError;
    let client = HttpRemote::new("http://127.0.0.1:1".into(), "t".into(), "box-1".into());
    assert_eq!(client.list("team-e2e").unwrap_err(), RemoteError::Unreachable);
}

#[test]
fn a_refused_token_is_not_retried() {
    // A 401 is a permanent answer; retrying cannot change it. Only connection failures retry.
    use harness_sync::remote::RemoteError;
    use harness_sync::remote_http::HttpRemote;
    let Some(url) = std::env::var("HS_E2E_TEAM_URL").ok() else { return };
    let client = HttpRemote::new(url, "not-a-real-token".into(), "box-1".into());
    let started = std::time::Instant::now();
    assert_eq!(client.list("team-e2e").unwrap_err(), RemoteError::Refused(401));
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "a refusal must return at once");
}

#[test]
fn an_unreachable_server_is_retried_before_it_is_reported() {
    use harness_sync::remote::RemoteError;
    use harness_sync::remote_http::HttpRemote;
    let client = HttpRemote::new("http://127.0.0.1:1".into(), "t".into(), "box-1".into());
    let started = std::time::Instant::now();
    assert_eq!(client.list("team-e2e").unwrap_err(), RemoteError::Unreachable);
    // Retries add a short, bounded wait; the total stays under the refresh timeout.
    assert!(started.elapsed() >= std::time::Duration::from_millis(200), "it retried before giving up");
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "the retries are bounded");
}
