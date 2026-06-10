from __future__ import annotations

import hashlib
import os
from typing import Any, Callable, Dict, Iterable

from src.agent_debug.domain.models import Plan, PlanStage, PlanStep, PlanTask, PlanVersion
from src.agent_debug.infra.memory_store import InMemoryTable
from src.agent_debug.infra.utils import make_id, utc_now_iso
from src.agent_debug.provider.base import ModelRequestContext
from src.agent_debug.provider.service import (
    ProviderExecutionError,
    ProviderExecutionService,
    extract_json_object,
)


def _to_string_list(value: Any) -> list[str]:
    return [str(item) for item in value] if isinstance(value, list) else []


class PlanEngine:
    def __init__(
        self,
        provider_service: ProviderExecutionService,
        model_resolver: Callable[[str], str] | None = None,
    ) -> None:
        self.provider_service = provider_service
        self.model_resolver = model_resolver or (lambda _session_id: os.getenv("OPENAI_MODEL", "mock-model"))
        self.plans = InMemoryTable[Plan]()
        self.versions = InMemoryTable[PlanVersion]()
        self.stages = InMemoryTable[PlanStage]()
        self.tasks = InMemoryTable[PlanTask]()
        self.steps = InMemoryTable[PlanStep]()

    async def generate(self, session_id: str, user_input: str) -> Dict[str, Any]:
        objective = user_input.replace("/plan", "").strip() or "Untitled objective"
        structured = self._fallback_structure(objective)
        request_context = ModelRequestContext(
            request_id=make_id("req"),
            trace_id=make_id("trace"),
            model=self.model_resolver(session_id),
            timeout_ms=8000,
            session_id=session_id,
            metadata={"operation": "plan_generation"},
        )

        try:
            result = await self.provider_service.execute_chat_operation(
                request=self._build_request(objective),
                ctx=request_context,
                session_id=session_id,
                operation="plan_generation",
                parser=self._parse_provider_plan,
            )
            structured = result.value
        except ProviderExecutionError:
            pass

        return self._persist_plan_bundle(session_id, structured)

    def delete_session_plans(self, session_id: str) -> int:
        """删除某会话关联的 plan 及其 stages/tasks/steps/versions（级联清理）。"""
        removed = 0
        for plan in self.plans.list_by("session_id", session_id):
            stages = self.stages.list_by("plan_id", plan.id)
            stage_ids = {stage.id for stage in stages}
            tasks = [task for task in self.tasks.list_all() if task.stage_id in stage_ids]
            task_ids = {task.id for task in tasks}
            for step in [s for s in self.steps.list_all() if s.task_id in task_ids]:
                self.steps.delete(step.id)
            for task in tasks:
                self.tasks.delete(task.id)
            for stage in stages:
                self.stages.delete(stage.id)
            for version in self.versions.list_by("plan_id", plan.id):
                self.versions.delete(version.id)
            if self.plans.delete(plan.id):
                removed += 1
        return removed

    def get(self, plan_id: str) -> Dict[str, Any] | None:
        plan = self.plans.get(plan_id)
        if not plan:
            return None
        stages = self.stages.list_by("plan_id", plan_id)
        stage_ids = {stage.id for stage in stages}
        tasks = [task for task in self.tasks.list_all() if task.stage_id in stage_ids]
        task_ids = {task.id for task in tasks}
        return {
            "plan": plan,
            "stages": stages,
            "tasks": tasks,
            "steps": [step for step in self.steps.list_all() if step.task_id in task_ids],
            "versions": self.versions.list_by("plan_id", plan_id),
        }

    def confirm(self, plan_id: str) -> Dict[str, Any] | None:
        """Move a plan to ``confirmed`` so the runtime is allowed to dispatch it."""
        plan = self.plans.get(plan_id)
        if not plan:
            return None
        plan.status = "confirmed"
        plan.updated_at = utc_now_iso()
        self.plans.save(plan.id, plan)
        return self.get(plan_id)

    def patch(
        self,
        plan_id: str,
        *,
        objective: str | None = None,
        status: str | None = None,
    ) -> Dict[str, Any] | None:
        plan = self.plans.get(plan_id)
        if not plan:
            return None
        if isinstance(objective, str) and objective.strip():
            plan.objective = objective.strip()
        if isinstance(status, str) and status.strip():
            plan.status = status.strip()
        plan.updated_at = utc_now_iso()
        self.plans.save(plan.id, plan)
        return self.get(plan_id)

    def mark_task_status(self, task_id: str, status: str) -> PlanTask | None:
        task = self.tasks.get(task_id)
        if not task:
            return None
        task.status = status  # type: ignore[assignment]
        self.tasks.save(task.id, task)
        for step in self.steps.list_by("task_id", task_id):
            step.status = status  # type: ignore[assignment]
            self.steps.save(step.id, step)
        return task

    def mark_plan_status(self, plan_id: str, status: str) -> Plan | None:
        plan = self.plans.get(plan_id)
        if not plan:
            return None
        plan.status = status
        plan.updated_at = utc_now_iso()
        self.plans.save(plan.id, plan)
        return plan

    def clone_plan_to_session(
        self,
        plan_id: str,
        new_session_id: str,
    ) -> tuple[Dict[str, Any], Dict[str, str]] | None:
        snapshot = self.get(plan_id)
        if snapshot is None:
            return None
        now = utc_now_iso()
        original = snapshot["plan"]
        new_plan = Plan(
            id=make_id("plan"),
            session_id=new_session_id,
            objective=original.objective,
            status=original.status,
            current_version_id=make_id("pv"),
            latest_execution_id=original.latest_execution_id,
            created_at=now,
            updated_at=now,
        )
        version = PlanVersion(
            id=new_plan.current_version_id,
            plan_id=new_plan.id,
            version=1,
            reason="fork",
            created_by="agent",
            base_version_id=original.current_version_id,
            content_hash=hashlib.sha256(str(snapshot).encode("utf-8")).hexdigest(),
            created_at=now,
        )
        self.plans.save(new_plan.id, new_plan)
        self.versions.save(version.id, version)

        stage_id_map: Dict[str, str] = {}
        task_id_map: Dict[str, str] = {}
        stages: list[PlanStage] = []
        tasks: list[PlanTask] = []
        steps: list[PlanStep] = []

        for stage in snapshot["stages"]:
            new_stage = PlanStage(
                id=make_id("stage"),
                plan_id=new_plan.id,
                title=stage.title,
                order=stage.order,
                status=stage.status,
                summary=stage.summary,
                risk_flags=list(stage.risk_flags or []),
            )
            stage_id_map[stage.id] = new_stage.id
            stages.append(new_stage)
            self.stages.save(new_stage.id, new_stage)

        for task in snapshot["tasks"]:
            new_task = PlanTask(
                id=make_id("task"),
                stage_id=stage_id_map.get(task.stage_id, task.stage_id),
                title=task.title,
                description=task.description,
                priority=task.priority,
                parallelism=task.parallelism,
                depends_on=[],
                status=task.status,
                owner_type=task.owner_type,
                estimated_cost=dict(task.estimated_cost or {}),
            )
            task_id_map[task.id] = new_task.id
            tasks.append(new_task)
            self.tasks.save(new_task.id, new_task)

        for old_task, new_task in zip(snapshot["tasks"], tasks):
            new_task.depends_on = [task_id_map[d] for d in (old_task.depends_on or []) if d in task_id_map]
            self.tasks.save(new_task.id, new_task)

        for step in snapshot["steps"]:
            new_step = PlanStep(
                id=make_id("step"),
                task_id=task_id_map.get(step.task_id, step.task_id),
                title=step.title,
                action_type=step.action_type,
                status=step.status,
                input_snapshot_id=step.input_snapshot_id,
                output_snapshot_id=step.output_snapshot_id,
                owner_agent_id=step.owner_agent_id,
                retry_count=step.retry_count,
            )
            steps.append(new_step)
            self.steps.save(new_step.id, new_step)

        return {
            "plan": new_plan,
            "version": version,
            "stages": stages,
            "tasks": tasks,
            "steps": steps,
            "assumptions": [],
            "risks": [],
            "openQuestions": [],
        }, task_id_map

    async def replan(
        self,
        plan_id: str,
        *,
        new_user_input: str,
        reason: str = "user_replan",
    ) -> Dict[str, Any] | None:
        """Generate a fresh plan structure for an existing session and link the
        new ``PlanVersion`` back to the previous one."""
        plan = self.plans.get(plan_id)
        if not plan:
            return None

        bundle = await self.generate(plan.session_id, new_user_input)
        new_plan = bundle["plan"]
        version = PlanVersion(
            id=make_id("pv"),
            plan_id=new_plan.id,
            version=2,
            reason=reason,
            created_by="agent",
            base_version_id=plan.current_version_id,
            content_hash=hashlib.sha256(str(bundle).encode("utf-8")).hexdigest(),
            created_at=utc_now_iso(),
        )
        self.versions.save(version.id, version)
        return bundle

    def _build_request(self, objective: str) -> dict[str, Any]:
        return {
            "messages": [
                {
                    "role": "system",
                    "content": (
                        "You generate execution plans for an agent-debug platform. "
                        "Return JSON only with keys: objective, assumptions, risks, openQuestions, stages. "
                        "Each stage must include title, summary, riskFlags, tasks. "
                        "Each task must include title, description, priority, parallelism, ownerType, estimatedCost, steps. "
                        "Each step must include title and actionType."
                    ),
                },
                {
                    "role": "user",
                    "content": f"Create a concise executable plan for: {objective}",
                },
            ]
        }

    def _parse_provider_plan(self, response: Any) -> dict[str, Any]:
        payload = extract_json_object(response.output)
        if "plan" in payload and isinstance(payload["plan"], dict):
            payload = payload["plan"]
        stages = payload.get("stages")
        if not isinstance(stages, list) or not stages:
            raise ValueError("Plan response must include at least one stage")
        for stage in stages:
            if not isinstance(stage, dict):
                raise ValueError("Plan stages must be objects")
            tasks = stage.get("tasks")
            if not isinstance(tasks, list) or not tasks:
                raise ValueError("Each stage must include at least one task")
            for task in tasks:
                if not isinstance(task, dict):
                    raise ValueError("Plan tasks must be objects")
                steps = task.get("steps")
                if not isinstance(steps, list) or not steps:
                    raise ValueError("Each task must include at least one step")
        return payload

    def _fallback_structure(self, objective: str) -> dict[str, Any]:
        return {
            "objective": objective,
            "assumptions": ["保持现有前端架构不变"],
            "risks": ["当前为骨架实现，部分模块先返回 stub 数据"],
            "openQuestions": [],
            "stages": [
                {
                    "title": "理解与拆解需求",
                    "summary": "分析目标与执行范围",
                    "riskFlags": ["scaffold-mode"],
                    "tasks": [
                        {
                            "title": "生成执行计划",
                            "description": "分析需求并生成结构化计划",
                            "priority": "high",
                            "parallelism": "serial",
                            "ownerType": "main-agent",
                            "estimatedCost": {"latencyMs": 1500, "tokens": 1200, "modelTier": "balanced"},
                            "steps": [
                                {
                                    "title": "提取 objective / stages / tasks / steps",
                                    "actionType": "analyze",
                                }
                            ],
                        }
                    ],
                }
            ],
        }

    def _persist_plan_bundle(self, session_id: str, structured: dict[str, Any]) -> Dict[str, Any]:
        now = utc_now_iso()
        plan_id = make_id("plan")
        version_id = make_id("pv")
        objective = str(structured.get("objective") or "Untitled objective")

        plan = Plan(
            id=plan_id,
            session_id=session_id,
            objective=objective,
            status="awaiting_confirmation",
            current_version_id=version_id,
            created_at=now,
            updated_at=now,
        )
        version = PlanVersion(
            id=version_id,
            plan_id=plan_id,
            version=1,
            reason="initial",
            created_by="agent",
            content_hash=hashlib.sha256(str(structured).encode("utf-8")).hexdigest(),
            created_at=now,
        )

        stages: list[PlanStage] = []
        tasks: list[PlanTask] = []
        steps: list[PlanStep] = []

        for stage_order, stage_payload in enumerate(self._iter_dicts(structured.get("stages")), start=1):
            stage = PlanStage(
                id=make_id("stage"),
                plan_id=plan_id,
                title=str(stage_payload.get("title") or f"Stage {stage_order}"),
                order=stage_order,
                status="pending",
                summary=str(stage_payload.get("summary") or "") or None,
                risk_flags=_to_string_list(stage_payload.get("riskFlags") or stage_payload.get("risk_flags")),
            )
            stages.append(stage)
            self.stages.save(stage.id, stage)

            for task_payload in self._iter_dicts(stage_payload.get("tasks")):
                task = PlanTask(
                    id=make_id("task"),
                    stage_id=stage.id,
                    title=str(task_payload.get("title") or "Untitled task"),
                    description=str(task_payload.get("description") or "Generated task"),
                    priority=str(task_payload.get("priority") or "medium"),
                    parallelism=str(task_payload.get("parallelism") or "serial"),
                    depends_on=_to_string_list(task_payload.get("dependsOn") or task_payload.get("depends_on")),
                    status="pending",
                    owner_type=str(task_payload.get("ownerType") or task_payload.get("owner_type") or "main-agent"),
                    estimated_cost=self._normalize_estimated_cost(task_payload.get("estimatedCost") or task_payload.get("estimated_cost")),
                )
                tasks.append(task)
                self.tasks.save(task.id, task)

                for step_payload in self._iter_dicts(task_payload.get("steps")):
                    step = PlanStep(
                        id=make_id("step"),
                        task_id=task.id,
                        title=str(step_payload.get("title") or "Execute step"),
                        action_type=str(step_payload.get("actionType") or step_payload.get("action_type") or "analyze"),
                        status="pending",
                        retry_count=int(step_payload.get("retryCount") or step_payload.get("retry_count") or 0),
                    )
                    steps.append(step)
                    self.steps.save(step.id, step)

        self.plans.save(plan.id, plan)
        self.versions.save(version.id, version)

        return {
            "plan": plan,
            "version": version,
            "stages": stages,
            "tasks": tasks,
            "steps": steps,
            "assumptions": _to_string_list(structured.get("assumptions")),
            "risks": _to_string_list(structured.get("risks")),
            "openQuestions": _to_string_list(structured.get("openQuestions") or structured.get("open_questions")),
        }

    def _iter_dicts(self, value: Any) -> Iterable[dict[str, Any]]:
        if isinstance(value, list):
            for item in value:
                if isinstance(item, dict):
                    yield item

    def _normalize_estimated_cost(self, value: Any) -> dict[str, Any]:
        if isinstance(value, dict):
            return {
                "latencyMs": int(value.get("latencyMs") or value.get("latency_ms") or 1500),
                "tokens": int(value.get("tokens") or 1200),
                "modelTier": str(value.get("modelTier") or value.get("model_tier") or "balanced"),
            }
        return {"latencyMs": 1500, "tokens": 1200, "modelTier": "balanced"}
