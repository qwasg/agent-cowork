//! API layer: app services container + per-domain handlers + axum routes + SSE.

pub mod handlers;
pub mod openapi;
pub mod routes;
pub mod services;
pub mod sse;

pub use services::AppServices;
