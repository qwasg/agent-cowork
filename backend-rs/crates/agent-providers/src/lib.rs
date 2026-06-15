//! LLM provider abstraction: trait, adapters, channel catalog, and the
//! execution service (resilience: retry + circuit breaker + fallback chain).

pub mod anthropic;
pub mod channels;
pub mod google;
pub mod mock;
pub mod openai;
pub mod service;
pub mod thinking;
pub mod tokens;
pub mod types;

use async_trait::async_trait;

use crate::types::{DeltaSink, ProviderRequest, ProviderResponse};
use agent_protocol::ApiResult;

#[async_trait]
pub trait LLMProvider: Send + Sync {
    fn name(&self) -> &str;
    fn is_ready(&self) -> bool {
        true
    }
    async fn chat(&self, req: &ProviderRequest, sink: DeltaSink<'_>)
        -> ApiResult<ProviderResponse>;
}

pub use service::ProviderExecutionService;
