//! Shared DTOs, event schema and error envelope (the API contract layer).

pub mod errors;
pub mod events;
pub mod models;
pub mod rollout;

pub use errors::{ApiError, ApiResult, ErrorEnvelope};
pub use events::{channel_for, DebugEvent, EventDraft};
pub use models::*;
pub use rollout::RolloutItem;
