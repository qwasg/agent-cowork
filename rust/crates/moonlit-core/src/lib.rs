//! `moonlit-core` — shared foundations for the Moonlit native (Rust) frontends.
//!
//! Contains:
//! - [`models`]: serde domain models mirroring the Python backend contract.
//! - [`hash`]: JS-compatible stable content hashing (compile/preview cache key).
//! - [`ids`]: pluggable id factories (default + deterministic).
//! - [`store`]: a JSON-file config store replacing browser `localStorage`.

pub mod hash;
pub mod ids;
pub mod models;
pub mod store;

pub use hash::{content_hash, fnv1a64, stable_stringify};
pub use ids::{DefaultIdFactory, IdFactory, SeqIdFactory};
pub use store::{ConfigStore, StoreError};
