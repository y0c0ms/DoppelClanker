//! HTTP surface for the team service. Each route is a thin call into `Service`, so every rule
//! is the one the service tests check. JSON in, JSON out. Errors map to status codes and
//! never include plaintext or key material.

use crate::service::{Service, ServiceError, Store};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
pub struct JoinBody { pub space: String, pub member: String, pub machine: String, pub code: String }

#[derive(Deserialize)]
pub struct LeaseBody { pub space: String, pub conversation: String, pub machine: String }

/// The status code and JSON body for a service error. The message names the rule, not the
/// content.
pub fn error_response(e: &ServiceError) -> (u16, Value) {
    match e {
        ServiceError::NoSpace(_) => (404, json!({"error": "no such space"})),
        ServiceError::NotMember(_) => (403, json!({"error": "not a member"})),
        ServiceError::Auth(_) => (401, json!({"error": "join code refused"})),
        ServiceError::Lease(m) if m.contains("Held") => (409, json!({"error": "conversation is in use"})),
        ServiceError::Lease(m) if m.contains("Stale") => (409, json!({"error": "stale write refused"})),
        ServiceError::Lease(_) => (409, json!({"error": "no live lease for this write"})),
        ServiceError::Crypto(_) => (400, json!({"error": "record refused"})),
        ServiceError::Policy(m) => (400, json!({"error": "invalid backup policy", "detail": m})),
    }
}

/// Route a join request. Returns (status, body).
pub fn join<S: Store>(svc: &mut Service<S>, body: &JoinBody, now: u64) -> (u16, Value) {
    match svc.join(&body.space, &body.member, &body.machine, &body.code, now) {
        Ok(()) => (200, json!({"joined": true})),
        Err(e) => error_response(&e),
    }
}

/// Route an acquire request. Returns (status, body) with the lease version on success.
pub fn acquire<S: Store>(svc: &mut Service<S>, body: &LeaseBody, now: u64) -> (u16, Value) {
    match svc.acquire(&body.space, &body.conversation, &body.machine, now) {
        Ok(lease) => (200, json!({"version": lease.version, "last_seen": lease.last_seen})),
        Err(e) => error_response(&e),
    }
}

/// Route a release request.
pub fn release<S: Store>(svc: &mut Service<S>, body: &LeaseBody) -> (u16, Value) {
    match svc.release(&body.space, &body.conversation, &body.machine) {
        Ok(()) => (200, json!({"released": true})),
        Err(e) => error_response(&e),
    }
}
