//! API layer: business gateway + axum routes + SSE.

pub mod gateway;
pub mod routes;
pub mod sse;

pub use gateway::AppServices;
