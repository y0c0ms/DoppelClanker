//! Request counters in the Prometheus text format, with no dependency. Each counter is an
//! atomic, so the hot path never takes a lock. Labels are fixed names, never user input, so
//! the number of series is bounded.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Default)]
pub struct Metrics {
    /// (route, status class) -> count. Route is one of the fixed route names.
    counts: Mutex<BTreeMap<(&'static str, &'static str), AtomicU64>>,
    pub refused_unauthenticated: AtomicU64,
    pub outage_responses: AtomicU64,
}

impl Metrics {
    pub fn record(&self, route: &'static str, status: u16) {
        let class = match status {
            200..=299 => "2xx",
            400..=499 => "4xx",
            500..=599 => "5xx",
            _ => "other",
        };
        if status == 401 {
            self.refused_unauthenticated.fetch_add(1, Ordering::Relaxed);
        }
        if status == 503 {
            self.outage_responses.fetch_add(1, Ordering::Relaxed);
        }
        let mut m = self.counts.lock().unwrap_or_else(|p| p.into_inner());
        m.entry((route, class)).or_insert_with(|| AtomicU64::new(0)).fetch_add(1, Ordering::Relaxed);
    }

    /// The counters in Prometheus text exposition format.
    pub fn render(&self) -> String {
        let mut out = String::from("# TYPE team_requests_total counter\n");
        let m = self.counts.lock().unwrap_or_else(|p| p.into_inner());
        for ((route, class), n) in m.iter() {
            out.push_str(&format!(
                "team_requests_total{{route=\"{route}\",class=\"{class}\"}} {}\n", n.load(Ordering::Relaxed)));
        }
        out.push_str(&format!("# TYPE team_refused_unauthenticated_total counter\nteam_refused_unauthenticated_total {}\n",
            self.refused_unauthenticated.load(Ordering::Relaxed)));
        out.push_str(&format!("# TYPE team_outage_responses_total counter\nteam_outage_responses_total {}\n",
            self.outage_responses.load(Ordering::Relaxed)));
        out
    }
}
