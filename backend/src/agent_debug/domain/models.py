from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, List, Literal, Optional


PlanNodeStatus = Literal[
    "draft",
    "pending",
    "ready",
    "running",
    "blocked",
    "waiting_feedback",
    "completed",
    "failed",
    "cancelled",
    "summarized",
    "rolled_up",
]

TodoStatus = Literal[
    "queued",
    "ready",
    "running",
    "paused",
    "blocked",
    "waiting_review",
    "completed",
    "failed",
    "skipped",
    "cancelled",
    "rolled_up",
]


@dataclass
class DebugSession:
    id: str
    title: str
    status: str = "idle"
    mode: str = "hybrid"
    selected_model_id: Optional[str] = None
    web_search_enabled: bool = False
    active_plan_id: Optional[str] = None
    active_run_id: Optional[str] = None
    created_at: str = ""
    updated_at: str = ""
    pinned: bool = False
    title_manually_set: bool = False


@dataclass
class AgentModelOption:
    id: str
    label: str
    provider: str
    source: str
    tier: str = "balanced"
    supports_reasoning: bool = False
    context_window_tokens: Optional[int] = None
    availability: str = "available"
    is_default: bool = False


@dataclass
class AgentModelPreferences:
    global_default_model_id: Optional[str] = None


@dataclass
class Plan:
    id: str
    session_id: str
    objective: str
    status: str = "drafting"
    current_version_id: str = ""
    latest_execution_id: Optional[str] = None
    created_at: str = ""
    updated_at: str = ""


@dataclass
class PlanStage:
    id: str
    plan_id: str
    title: str
    order: int
    status: PlanNodeStatus = "draft"
    summary: Optional[str] = None
    risk_flags: List[str] = field(default_factory=list)


@dataclass
class PlanTask:
    id: str
    stage_id: str
    title: str
    description: str
    priority: str = "medium"
    parallelism: str = "serial"
    depends_on: List[str] = field(default_factory=list)
    status: PlanNodeStatus = "draft"
    owner_type: str = "main-agent"
    estimated_cost: Dict[str, Any] = field(default_factory=dict)


@dataclass
class PlanStep:
    id: str
    task_id: str
    title: str
    action_type: str
    status: PlanNodeStatus = "draft"
    input_snapshot_id: Optional[str] = None
    output_snapshot_id: Optional[str] = None
    owner_agent_id: Optional[str] = None
    retry_count: int = 0


@dataclass
class PlanVersion:
    id: str
    plan_id: str
    version: int
    reason: str
    created_by: str
    base_version_id: Optional[str] = None
    content_hash: str = ""
    created_at: str = ""


@dataclass
class PlanDiff:
    id: str
    plan_id: str
    from_version_id: str
    to_version_id: str
    changed_nodes: List[Dict[str, Any]] = field(default_factory=list)
    summary: str = ""
    created_at: str = ""


@dataclass
class TodoItem:
    id: str
    session_id: str
    title: str
    description: str
    source: str
    owner: Dict[str, str]
    priority: int
    status: TodoStatus = "queued"
    dependencies: List[str] = field(default_factory=list)
    related_plan_node_ids: List[str] = field(default_factory=list)
    related_agent_run_id: Optional[str] = None
    agent_todo_group_id: Optional[str] = None
    related_subagent_run_ids: List[str] = field(default_factory=list)
    related_swarm_shard_ids: List[str] = field(default_factory=list)
    retry_count: int = 0
    due_hint: Optional[str] = None
    artifacts: List[str] = field(default_factory=list)
    summary: Optional[str] = None
    last_error: Optional[str] = None
    archived_at: Optional[str] = None
    created_at: str = ""
    updated_at: str = ""


@dataclass
class AgentRun:
    id: str
    session_id: str
    plan_id: Optional[str] = None
    trigger: str = "manual"
    status: str = "starting"
    current_context_ref: str = ""
    active_node_ids: List[str] = field(default_factory=list)
    active_todo_ids: List[str] = field(default_factory=list)
    completed_todo_ids: List[str] = field(default_factory=list)
    failed_todo_ids: List[str] = field(default_factory=list)
    parallel_limit: int = 4
    created_at: str = ""
    updated_at: str = ""


@dataclass
class CodeEditProposal:
    """Persisted record of a :class:`CodeEditEngine` output for apply/discard."""

    id: str
    session_id: str
    schema_version: str
    summary: str
    source: str
    changes: List[Dict[str, Any]] = field(default_factory=list)
    status: str = "pending"
    applied_at: Optional[str] = None
    discarded_at: Optional[str] = None
    applied_files: List[str] = field(default_factory=list)
    apply_error: Optional[str] = None
    created_at: str = ""


@dataclass
class SubagentRun:
    id: str
    parent_run_id: str
    plan_node_ids: List[str]
    todo_ids: List[str]
    mode: str
    status: str = "queued"
    objective: str = ""
    context_ref: str = ""
    summary_ref: Optional[str] = None
    retry_count: int = 0
    started_at: Optional[str] = None
    ended_at: Optional[str] = None


@dataclass
class SubagentSummary:
    id: str
    task_id: str
    subagent_run_id: str
    objective: str
    input_goal: str
    actions: List[str] = field(default_factory=list)
    key_findings: List[str] = field(default_factory=list)
    artifacts: List[Dict[str, Any]] = field(default_factory=list)
    errors: List[Dict[str, Any]] = field(default_factory=list)
    risks: List[str] = field(default_factory=list)
    decisions: List[str] = field(default_factory=list)
    next_actions: List[str] = field(default_factory=list)
    unresolved_questions: List[str] = field(default_factory=list)
    context_delta: Dict[str, Any] = field(default_factory=dict)
    lineage: Dict[str, Any] = field(default_factory=dict)
    token_usage: Dict[str, Any] = field(default_factory=dict)
    compression_level: str = "structured"
    created_at: str = ""


@dataclass
class SwarmNodeRegistration:
    agent_id: str
    node_id: str
    capabilities: List[str] = field(default_factory=list)
    supported_tools: List[str] = field(default_factory=list)
    max_concurrency: int = 1
    health_status: str = "healthy"
    load_score: float = 0.0
    version: str = "0.1.0"
    last_heartbeat_at: str = ""


@dataclass
class SwarmShard:
    id: str
    swarm_run_id: str
    shard_key: str
    shard_type: str
    input_ref: str
    parent_plan_node_id: Optional[str] = None
    parent_todo_id: Optional[str] = None
    assigned_node_id: Optional[str] = None
    status: str = "queued"
    retry_count: int = 0


@dataclass
class DebugEvent:
    id: str
    session_id: str
    seq: int
    type: str
    ts: str
    source: Dict[str, str]
    payload: Dict[str, Any]
    correlation_id: Optional[str] = None


from dataclasses import asdict

def asdict_safe(obj: Any) -> Dict[str, Any]:
    from src.agent_debug.infra.utils import dict_keys_to_camel_case
    if hasattr(obj, "__dict__"):
        return dict_keys_to_camel_case(asdict(obj))
    raise TypeError(f"Unsupported type: {type(obj)!r}")
