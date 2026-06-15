//! Swarm coordinator (port of `domain/swarm_coordinator.py`): node registry +
//! task sharding with round-robin assignment. In-memory, like the Python
//! reference — swarm state is demo/ephemeral by design.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use agent_protocol::models::{new_id, now_ts};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwarmNodeRegistration {
    pub agent_id: String,
    pub node_id: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub supported_tools: Vec<String>,
    #[serde(default)]
    pub max_concurrency: i64,
    #[serde(default)]
    pub health_status: String,
    #[serde(default)]
    pub load_score: f64,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub last_heartbeat_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwarmShard {
    pub id: String,
    pub swarm_run_id: String,
    pub shard_key: String,
    pub shard_type: String,
    pub input_ref: String,
    #[serde(default)]
    pub parent_plan_node_id: Option<String>,
    #[serde(default)]
    pub parent_todo_id: Option<String>,
    #[serde(default)]
    pub assigned_node_id: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub retry_count: i64,
}

#[derive(Default)]
pub struct SwarmCoordinator {
    nodes: Mutex<HashMap<String, SwarmNodeRegistration>>,
    shards: Mutex<HashMap<String, SwarmShard>>,
}

impl SwarmCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_node(&self, payload: &Value) -> SwarmNodeRegistration {
        let node = SwarmNodeRegistration {
            agent_id: payload
                .get("agent_id")
                .or_else(|| payload.get("agentId"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            node_id: payload
                .get("node_id")
                .or_else(|| payload.get("nodeId"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            capabilities: str_vec(payload.get("capabilities")),
            supported_tools: str_vec(
                payload
                    .get("supported_tools")
                    .or_else(|| payload.get("supportedTools")),
            ),
            max_concurrency: payload
                .get("max_concurrency")
                .or_else(|| payload.get("maxConcurrency"))
                .and_then(|v| v.as_i64())
                .unwrap_or(1),
            health_status: payload
                .get("health_status")
                .or_else(|| payload.get("healthStatus"))
                .and_then(|v| v.as_str())
                .unwrap_or("healthy")
                .to_string(),
            load_score: payload
                .get("load_score")
                .or_else(|| payload.get("loadScore"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            version: payload
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("0.1.0")
                .to_string(),
            last_heartbeat_at: payload
                .get("last_heartbeat_at")
                .or_else(|| payload.get("lastHeartbeatAt"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        };
        self.nodes
            .lock()
            .unwrap()
            .insert(node.node_id.clone(), node.clone());
        node
    }

    pub fn list_nodes(&self) -> Vec<SwarmNodeRegistration> {
        let mut out: Vec<_> = self.nodes.lock().unwrap().values().cloned().collect();
        out.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        out
    }

    pub fn list_shards(&self) -> Vec<SwarmShard> {
        let mut out: Vec<_> = self.shards.lock().unwrap().values().cloned().collect();
        out.sort_by(|a, b| a.shard_key.cmp(&b.shard_key));
        out
    }

    pub fn shard_task(&self, swarm_run_id: &str, keys: &[&str]) -> Vec<SwarmShard> {
        let mut created = Vec::new();
        let mut shards = self.shards.lock().unwrap();
        for key in keys {
            let shard = SwarmShard {
                id: new_id("shard"),
                swarm_run_id: swarm_run_id.to_string(),
                shard_key: key.to_string(),
                shard_type: "search-space".to_string(),
                input_ref: new_id("input"),
                parent_plan_node_id: None,
                parent_todo_id: None,
                assigned_node_id: None,
                status: "queued".to_string(),
                retry_count: 0,
            };
            shards.insert(shard.id.clone(), shard.clone());
            created.push(shard);
        }
        created
    }

    pub fn assign_shards_round_robin(&self, swarm_run_id: &str) {
        let nodes = self.list_nodes();
        if nodes.is_empty() {
            return;
        }
        let mut shards = self.shards.lock().unwrap();
        let mut target: Vec<&mut SwarmShard> = shards
            .values_mut()
            .filter(|s| s.swarm_run_id == swarm_run_id)
            .collect();
        target.sort_by(|a, b| a.shard_key.cmp(&b.shard_key));
        for (index, shard) in target.into_iter().enumerate() {
            shard.assigned_node_id = Some(nodes[index % nodes.len()].node_id.clone());
            shard.status = "assigned".to_string();
        }
    }

    /// State payload (`GET /swarm/state`), Python shape `{nodes, shards}`.
    pub fn state(&self) -> Value {
        json!({ "nodes": self.list_nodes(), "shards": self.list_shards() })
    }

    /// Seed three demo nodes + four shards and round-robin assign them.
    pub fn seed_demo(&self) -> Value {
        if self.list_nodes().is_empty() {
            for index in 0..3i64 {
                self.register_node(&json!({
                    "agent_id": format!("agent_{}", index + 1),
                    "node_id": format!("node_{}", index + 1),
                    "capabilities": ["plan", "search", "code"],
                    "supported_tools": ["rg", "read", "ws"],
                    "max_concurrency": 2,
                    "health_status": "healthy",
                    "load_score": index as f64 * 0.2,
                    "version": "0.1.0",
                    "last_heartbeat_at": now_ts(),
                }));
            }
        }
        if self.list_shards().is_empty() {
            self.shard_task("swarm_demo", &["files", "tests", "docs", "runtime"]);
        }
        self.assign_shards_round_robin("swarm_demo");
        self.state()
    }
}

fn str_vec(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_demo_creates_nodes_and_assigned_shards() {
        let swarm = SwarmCoordinator::new();
        let state = swarm.seed_demo();
        assert_eq!(state["nodes"].as_array().unwrap().len(), 3);
        let shards = state["shards"].as_array().unwrap();
        assert_eq!(shards.len(), 4);
        assert!(shards
            .iter()
            .all(|s| s["status"] == "assigned" && s["assignedNodeId"].is_string()));
        // Idempotent.
        let again = swarm.seed_demo();
        assert_eq!(again["nodes"].as_array().unwrap().len(), 3);
        assert_eq!(again["shards"].as_array().unwrap().len(), 4);
    }
}
