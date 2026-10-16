//! The counters must reflect what the server actually answered, and must never carry user data.

use harness_sync_team::metrics::Metrics;

#[test]
fn counts_by_route_and_class() {
    let m = Metrics::default();
    m.record("healthz", 200);
    m.record("healthz", 200);
    m.record("record", 409);
    let text = m.render();
    assert!(text.contains(r#"team_requests_total{route="healthz",class="2xx"} 2"#), "{text}");
    assert!(text.contains(r#"team_requests_total{route="record",class="4xx"} 1"#), "{text}");
}

#[test]
fn unauthenticated_and_outage_responses_have_their_own_counters() {
    let m = Metrics::default();
    m.record("sessions", 401);
    m.record("sessions", 401);
    m.record("healthz", 503);
    let text = m.render();
    assert!(text.contains("team_refused_unauthenticated_total 2"), "{text}");
    assert!(text.contains("team_outage_responses_total 1"), "{text}");
}

#[test]
fn the_output_carries_only_fixed_names_and_numbers() {
    let m = Metrics::default();
    m.record("record", 200);
    let text = m.render();
    // Every series is a fixed metric name with numeric values: no space names, no tokens.
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        let value = line.rsplit(' ').next().unwrap();
        assert!(value.parse::<u64>().is_ok(), "value must be a number: {line}");
        assert!(line.starts_with("team_"), "fixed metric name: {line}");
    }
}
