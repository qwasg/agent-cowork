//! REST operation implementations, grouped by domain. Each module extends
//! [`AppServices`](crate::api::AppServices) with an `impl` block; the axum
//! routes in [`crate::api::routes`] dispatch into these.

pub mod access;
pub mod channels;
pub mod chat;
pub mod checkpoints;
pub mod mcp;
pub mod memories;
pub mod models;
pub mod proposals;
pub mod runs;
pub mod sessions;
pub mod snapshot;
pub mod swarm;
pub mod system;
pub mod todos;
pub mod workspace;
