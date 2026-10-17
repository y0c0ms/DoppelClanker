//! Team sync core. See the tests for the behaviour each module must keep.
pub mod auth;
pub mod membership;
pub mod session_keys;
pub mod metrics;
pub mod server;
pub mod backup;
pub mod http;
pub mod lease;
pub mod pg;
pub mod service;
pub mod spaces;
pub mod totp;
pub mod wrap;
