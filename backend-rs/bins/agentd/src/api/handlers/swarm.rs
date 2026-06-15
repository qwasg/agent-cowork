//! Swarm demo state.

use serde_json::Value;

use agent_protocol::events::EventDraft;

use crate::api::AppServices;

impl AppServices {
    pub fn list_swarm_state(&self) -> Value {
        self.swarm.state()
    }

    /// Seed demo swarm nodes/shards and mirror them into the event stream
    /// (port of Python `seed_swarm_demo`).
    pub fn seed_swarm_demo(&self) -> Value {
        let state = self.swarm.seed_demo();
        if let Some(nodes) = state["nodes"].as_array() {
            for node in nodes {
                self.bus.emit(
                    EventDraft::new("swarm_demo", "swarm.node.joined", "swarm")
                        .payload(node.clone()),
                );
            }
        }
        if let Some(shards) = state["shards"].as_array() {
            for shard in shards {
                let etype = if shard["status"] == "assigned" {
                    "swarm.shard.assigned"
                } else {
                    "swarm.shard.reassigned"
                };
                self.bus
                    .emit(EventDraft::new("swarm_demo", etype, "swarm").payload(shard.clone()));
            }
        }
        state
    }
}
