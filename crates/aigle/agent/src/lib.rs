//! Agent execution and photographic tools shared by the Qt editor and legacy frontend.
pub mod agent;
pub mod agent_preview;
pub mod auto_tone;
pub mod models;
pub use models::fetch_models;
pub use agent::{AgentEvent, Profile, RunStats, profile_ops, run};

#[derive(Clone)]
pub struct ActiveModel {
    pub endpoint: String,
    pub model: String,
    pub api_key: String,
}
