from __future__ import annotations

from typing import Any, AsyncIterator, Dict

from src.agent_debug.infra.utils import make_id, utc_now_iso
from src.agent_debug.provider.base import LLMProvider, ModelRequestContext, ProviderResponse


class MockProvider(LLMProvider):
    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        user_input = ""
        for message in request.get("messages", []):
            if message.get("role") == "user":
                user_input = message.get("content", "")

        operation = ctx.metadata.get("operation", "")
        if operation == "summary_generation":
            output: Dict[str, Any] = {
                "actions": ["collect context", "execute subagent task", "produce summary"],
                "keyFindings": ["Mock provider generated a structured summary"],
                "artifacts": [],
                "errors": [],
                "risks": [],
                "decisions": ["Keep structured rollback summaries"],
                "nextActions": ["Write compacted context back to the parent run"],
                "unresolvedQuestions": [],
                "contextDelta": {
                    "factsAdded": ["summary result has been compacted"],
                    "factsRemoved": [],
                    "assumptionsUpdated": [],
                },
                "lineage": {
                    "replacedContextRefs": [ctx.metadata.get("contextRef", "ctx_mock_raw")],
                    "sourceEventIds": [],
                    "canExpand": True,
                },
                "tokenUsage": {
                    "inputTokens": 200,
                    "outputTokens": 120,
                    "summaryTokens": 80,
                },
                "compressionLevel": "structured",
            }
        elif operation == "subagent_execution":
            output = {
                "message": f"Executed subagent objective: {user_input or 'mock objective'}",
            }
        elif operation == "composer_chat":
            # Prefer gateway metadata (pure user line). Legacy: user content
            # may still contain appended "[contextWindow]: ..." from old
            # servers — strip before echo.
            raw = (ctx.metadata.get("composer_user_text") or "").strip()
            if not raw:
                raw = (user_input or "").strip()
            if "[contextWindow]:" in raw:
                raw = raw.split("[contextWindow]:", 1)[0].rstrip()
            snippet = (raw or "…")[:200]
            mode = (ctx.metadata.get("composer_mode") or "build").strip().lower()
            if mode not in ("build", "debug", "ask"):
                mode = "build"
            if mode == "build":
                body = (
                    f"你好，我是 Moonlit 文档编译助手的 mock 助手，当前为 **Build** 模式占位。"
                    f"已收到：「{snippet}」。配置真实 LLM 后，会在此返回实现与代码向的回答。"
                )
            elif mode == "debug":
                body = (
                    f"你好，我是 Moonlit 文档编译助手的 mock 助手，当前为 **Debug** 模式占位。"
                    f"你提到：「{snippet}」。配置真实 LLM 后，可在此协助复现、日志与根因分析。"
                )
            else:
                body = (
                    f"你好，我是 Moonlit 文档编译助手的 mock 助手，当前为 **Ask** 模式占位。"
                    f"问题：「{snippet}」。配置真实 LLM 后，可在此作概念说明与对比。"
                )
            output = {
                "role": "assistant",
                "content": body,
            }
        else:
            objective = user_input.replace("/plan", "").strip()
            if objective.lower().startswith("create a concise executable plan for:"):
                objective = objective.split(":", 1)[1].strip()
            objective = objective or "Mock objective"
            output = {
                "objective": objective,
                "assumptions": ["保持现有前端架构不变"],
                "risks": ["当前为 mock provider 输出"],
                "openQuestions": [],
                "stages": [
                    {
                        "title": "理解与拆解需求",
                        "summary": "整理目标与范围",
                        "riskFlags": ["mock-provider"],
                        "tasks": [
                            {
                                "title": "生成执行计划",
                                "description": "分析需求并生成结构化计划",
                                "priority": "high",
                                "parallelism": "serial",
                                "ownerType": "main-agent",
                                "estimatedCost": {
                                    "latencyMs": 1500,
                                    "tokens": 1200,
                                    "modelTier": "balanced",
                                },
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

        return ProviderResponse(
            provider="mock",
            model=ctx.model,
            output=output,
            token_usage={"input": 128, "output": 256},
            finish_reason="stop",
        )

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        return ProviderResponse(
            provider="mock",
            model=ctx.model,
            output=request,
            token_usage={"input": 64, "output": 64},
            finish_reason="stop",
        )

    async def stream(self, request: Dict[str, Any], ctx: ModelRequestContext) -> AsyncIterator[Dict[str, Any]]:
        del request
        yield {"type": "delta", "requestId": ctx.request_id, "payload": {"delta": "Mock "}}
        yield {"type": "delta", "requestId": ctx.request_id, "payload": {"delta": "stream"}}
        yield {
            "type": "completed",
            "requestId": ctx.request_id,
            "payload": {"completedAt": utc_now_iso(), "id": make_id("stream")},
        }

    async def healthcheck(self) -> Dict[str, Any]:
        return {"ok": True, "latencyMs": 1}
