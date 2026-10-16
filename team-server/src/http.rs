//! HTTP surface for the team service. Each route is a thin call into `Service`, so every rule
//! is the one the service tests check. JSON in, JSON out. Errors map to status codes and
//! never include plaintext or key material.
//!
//! Every route except `POST /enrol` requires `Authorization: Bearer <token>`. The token must
//! have been issued to the same machine named in the request, and it must not be expired.

use crate::service::{Service, ServiceError, Store};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
pub struct EnrolBody { pub space: String, pub member: String, pub machine: String, pub code: String }

#[derive(Deserialize)]
pub struct LeaseBody { pub space: String, pub conversation: String, pub machine: String }

/// The status code and JSON body for a service error. The message names the rule, not the
/// content.
pub fn error_response(e: &ServiceError) -> (u16, Value) {
    match e {
        ServiceError::Unavailable => (503, json!({"error": "service unavailable, retry later"})),
        ServiceError::NoSpace(_) => (404, json!({"error": "no such space"})),
        ServiceError::NotMember(_) => (403, json!({"error": "not a member"})),
        ServiceError::Auth(_) => (401, json!({"error": "authentication refused"})),
        ServiceError::Lease(m) if m.contains("Held") => (409, json!({"error": "conversation is in use"})),
        ServiceError::Lease(m) if m.contains("Stale") => (409, json!({"error": "stale write refused"})),
        ServiceError::Lease(_) => (409, json!({"error": "no live lease for this write"})),
        ServiceError::Crypto(_) => (400, json!({"error": "record refused"})),
        ServiceError::Policy(m) => (400, json!({"error": "invalid backup policy", "detail": m})),
    }
}

/// Check the bearer token for a request from `machine` in `space`. Returns the member on
/// success, or a 401 response.
pub fn require_token<S: Store>(svc: &Service<S>, space: &str, auth_header: Option<&str>,
                               machine: &str, now: u64) -> Result<String, (u16, Value)>
{
    let token = auth_header
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or((401, json!({"error": "missing bearer token"})))?;
    svc.authenticate(space, token, machine, now).map_err(|e| error_response(&e))
}

/// List the conversation ids in a space. The caller must hold a token for its machine and
/// must be a member of the space.
pub fn list_sessions<S: Store>(svc: &Service<S>, auth: Option<&str>, space: &str, machine: &str,
                               now: u64) -> (u16, Value)
{
    let member = match require_token(svc, space, auth, machine, now) {
        Ok(m) => m,
        Err(r) => return r,
    };
    match svc.list_conversations(space, &member) {
        Ok(ids) => (200, json!({"space": space, "conversations": ids})),
        Err(e) => error_response(&e),
    }
}

/// Health: 200 when the store answers, 503 when it does not. No auth, no data: it only says
/// whether the service can do its job, so a monitor can catch a wedge.
pub fn health<S: Store>(svc: &Service<S>) -> (u16, Value) {
    if svc.healthy() {
        (200, json!({"status": "ok"}))
    } else {
        (503, json!({"status": "database unreachable"}))
    }
}

/// Enrol a machine. Confirms the authenticator code, then returns a token bound to the machine.
pub fn enrol<S: Store>(svc: &mut Service<S>, body: &EnrolBody, now: u64) -> (u16, Value) {
    match svc.enrol(&body.space, &body.member, &body.machine, &body.code, now) {
        Ok(token) => (200, json!({"token": token, "expires_in": crate::service::TOKEN_LIFETIME_SECS})),
        Err(e) => error_response(&e),
    }
}

/// Acquire a conversation lease. The caller must hold a token for its machine.
pub fn acquire<S: Store>(svc: &mut Service<S>, auth: Option<&str>, body: &LeaseBody, now: u64) -> (u16, Value) {
    if let Err(r) = require_token(svc, &body.space, auth, &body.machine, now) {
        return r;
    }
    match svc.acquire(&body.space, &body.conversation, &body.machine, now) {
        Ok(lease) => (200, json!({"version": lease.version})),
        Err(e) => error_response(&e),
    }
}

/// Release a conversation lease. The caller must hold a token for its machine.
pub fn release<S: Store>(svc: &mut Service<S>, auth: Option<&str>, body: &LeaseBody, now: u64) -> (u16, Value) {
    if let Err(r) = require_token(svc, &body.space, auth, &body.machine, now) {
        return r;
    }
    match svc.release(&body.space, &body.conversation, &body.machine) {
        Ok(()) => (200, json!({"released": true})),
        Err(e) => error_response(&e),
    }
}

/// Store a session under the caller's live lease. The caller must hold a token for its machine.
pub fn write_record<S: Store>(svc: &mut Service<S>, auth: Option<&str>, space: &str, conv: &str,
                              machine: &str, version: u64, plain: Vec<u8>, now: u64) -> (u16, Value)
{
    if let Err(r) = require_token(svc, space, auth, machine, now) {
        return r;
    }
    match svc.put_record(space, conv, machine, version, plain, now) {
        Ok(()) => (200, json!({"stored": true})),
        Err(e) => error_response(&e),
    }
}

/// Read a session. Returns the plaintext on success, or (status, error body).
pub fn read_record<S: Store>(svc: &Service<S>, auth: Option<&str>, space: &str, conv: &str,
                             machine: &str, now: u64) -> (u16, Result<Vec<u8>, Value>)
{
    let member = match require_token(svc, space, auth, machine, now) {
        Ok(m) => m,
        Err((status, body)) => return (status, Err(body)),
    };
    match svc.get_record(space, conv, &member) {
        Ok(plain) => (200, Ok(plain)),
        Err(e) => {
            let (status, body) = error_response(&e);
            (status, Err(body))
        }
    }
}
