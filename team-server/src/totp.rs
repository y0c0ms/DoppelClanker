//! Authenticator join: RFC 6238 TOTP, 6 digits, 30-second steps, HMAC-SHA1.
//!
//! The check proves the joining user holds the authenticator. It is not the encryption key:
//! a 6-digit code has only a million values, so it cannot carry or derive a key. A code is
//! accepted once, and five wrong codes lock joining for 15 minutes.

use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::collections::HashSet;

pub const STEP_SECS: u64 = 30;
pub const MAX_FAILURES: u32 = 5;
pub const LOCK_SECS: u64 = 15 * 60;

#[derive(Debug, PartialEq, Eq)]
pub enum JoinError {
    Wrong,
    AlreadyUsed,
    Locked { until: u64 },
}

/// The 6-digit TOTP code for `secret` at `time` (seconds since the epoch).
pub fn code_at(secret: &[u8], time: u64) -> String {
    let counter = (time / STEP_SECS).to_be_bytes();
    let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(secret).expect("HMAC takes any key length");
    mac.update(&counter);
    let digest = mac.finalize().into_bytes();
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let bin = ((digest[offset] as u32 & 0x7f) << 24)
        | ((digest[offset + 1] as u32) << 16)
        | ((digest[offset + 2] as u32) << 8)
        | (digest[offset + 3] as u32);
    format!("{:06}", bin % 1_000_000)
}

/// Gate for one user's join attempts: single use per code, and a lock after repeated failures.
pub struct JoinGate {
    secret: Vec<u8>,
    used: HashSet<(u64, String)>,
    failures: u32,
    locked_until: u64,
}

impl JoinGate {
    pub fn new(secret: Vec<u8>) -> JoinGate {
        JoinGate { secret, used: HashSet::new(), failures: 0, locked_until: 0 }
    }

    /// Check a code presented at `now`. Accepts the current step (and the previous one, to
    /// allow for clock drift), refuses a code already used, and locks after MAX_FAILURES.
    pub fn verify(&mut self, code: &str, now: u64) -> Result<(), JoinError> {
        if now < self.locked_until {
            return Err(JoinError::Locked { until: self.locked_until });
        }
        let step = now / STEP_SECS;
        for s in [step, step.saturating_sub(1)] {
            let t = s * STEP_SECS;
            if code_at(&self.secret, t) == code {
                if self.used.contains(&(s, code.to_string())) {
                    return Err(JoinError::AlreadyUsed);
                }
                self.used.insert((s, code.to_string()));
                self.failures = 0;
                return Ok(());
            }
        }
        self.failures += 1;
        if self.failures >= MAX_FAILURES {
            self.locked_until = now + LOCK_SECS;
            self.failures = 0;
        }
        Err(JoinError::Wrong)
    }
}
