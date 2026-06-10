from __future__ import annotations

from typing import Any, Dict, List

from src.agent_debug.domain.models import SwarmNodeRegistration, SwarmShard
from src.agent_debug.infra.memory_store import InMemoryTable
from src.agent_debug.infra.utils import make_id, utc_now_iso


class SwarmCoordinator:
    def __init__(self) -> None:
        self.nodes = InMemoryTable[SwarmNodeRegistration]()
        self.shards = InMemoryTable[SwarmShard]()

    def register_node(self, payload: Dict[str, Any]) -> SwarmNodeRegistration:
        node = SwarmNodeRegistration(
            agent_id=payload["agent_id"],
            node_id=payload["node_id"],
            capabilities=payload.get("capabilities", []),
            supported_tools=payload.get("supported_tools", []),
            max_concurrency=payload.get("max_concurrency", 1),
            health_status=payload.get("health_status", "healthy"),
            load_score=payload.get("load_score", 0.0),
            version=payload.get("version", "0.1.0"),
            last_heartbeat_at=payload.get("last_heartbeat_at", ""),
        )
        self.nodes.save(node.node_id, node)
        return node

    def list_nodes(self) -> List[SwarmNodeRegistration]:
        return self.nodes.list_all()

    def list_shards(self) -> List[SwarmShard]:
        return self.shards.list_all()

    def remove_node(self, node_id: str) -> bool:
        if self.nodes.get(node_id) is None:
            return False
        del self.nodes._items[node_id]
        return True

    def shard_task(self, swarm_run_id: str, keys: List[str]) -> List[SwarmShard]:
        shards: List[SwarmShard] = []
        for key in keys:
            shard = SwarmShard(
                id=make_id("shard"),
                swarm_run_id=swarm_run_id,
                shard_key=key,
                shard_type="search-space",
                input_ref=make_id("input"),
            )
            self.shards.save(shard.id, shard)
            shards.append(shard)
        return shards

    def assign_shards_round_robin(self, swarm_run_id: str) -> List[SwarmShard]:
        nodes = self.list_nodes()
        shards = [shard for shard in self.list_shards() if shard.swarm_run_id == swarm_run_id]
        if not nodes:
            return shards
        for index, shard in enumerate(shards):
            shard.assigned_node_id = nodes[index % len(nodes)].node_id
            shard.status = "assigned"
            self.shards.save(shard.id, shard)
        return shards

    def reassign_shard(self, shard_id: str, node_id: str) -> SwarmShard | None:
        shard = self.shards.get(shard_id)
        if shard is None:
            return None
        shard.assigned_node_id = node_id
        shard.status = "reassigned"
        self.shards.save(shard.id, shard)
        return shard

    def seed_demo(self) -> Dict[str, List[Any]]:
        if not self.list_nodes():
            for index in range(3):
                self.register_node(
                    {
                        "agent_id": f"agent_{index + 1}",
                        "node_id": f"node_{index + 1}",
                        "capabilities": ["plan", "search", "code"],
                        "supported_tools": ["rg", "read", "ws"],
                        "max_concurrency": 2,
                        "health_status": "healthy",
                        "load_score": index * 0.2,
                        "version": "0.1.0",
                        "last_heartbeat_at": utc_now_iso(),
                    }
                )
        if not self.list_shards():
            self.shard_task("swarm_demo", ["files", "tests", "docs", "runtime"])
        self.assign_shards_round_robin("swarm_demo")
        return {"nodes": self.list_nodes(), "shards": self.list_shards()}
