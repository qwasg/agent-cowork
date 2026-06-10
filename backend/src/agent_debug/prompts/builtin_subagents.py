"""内置子代理（sub-agent）定义。

参考 Proma ``buildBuiltinAgents``：提供若干预置子代理画像（探索 / 调研 /
代码评审），供主代理在编排时按职责委派，并可注入系统提示词说明可用子代理。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Dict, List


@dataclass(frozen=True)
class SubagentProfile:
    name: str
    description: str
    system_prompt: str
    allowed_tools: List[str] = field(default_factory=list)


# 子代理可用的默认工具集（永不包含 ``Task``，防止递归委派）。
DEFAULT_READONLY_TOOLS: List[str] = [
    "read_file",
    "list_dir",
    "grep",
    "glob",
    "read_skill",
    "web_search",
    "web_fetch",
]
DEFAULT_WRITE_TOOLS: List[str] = DEFAULT_READONLY_TOOLS + [
    "write_file",
    "create_document",
    "delete_file",
    "run_command",
    "check_command_status",
    "stop_command",
    "write_todos",
]


BUILTIN_SUBAGENTS: List[SubagentProfile] = [
    SubagentProfile(
        name="explorer",
        description="只读探索代码库：定位文件、梳理结构、回答“在哪里/怎么实现”。",
        system_prompt=(
            "你是探索型子代理。只读地检索与阅读代码，快速定位相关文件与调用关系，"
            "产出结构化的发现与文件路径，不做任何修改。"
        ),
        allowed_tools=["read_file", "list_dir", "grep", "glob"],
    ),
    SubagentProfile(
        name="researcher",
        description="资料调研：调用联网搜索与网页抓取工具，汇总要点与权衡。",
        system_prompt=(
            "你是调研型子代理。围绕给定问题使用联网搜索和网页抓取工具检索资料，输出要点、对比与建议，"
            "标注信息来源，不修改工作区。"
        ),
        allowed_tools=["read_file", "grep", "web_search", "web_fetch"],
    ),
    SubagentProfile(
        name="code-reviewer",
        description="代码评审：审查改动，指出风险、缺陷与改进点。",
        system_prompt=(
            "你是代码评审型子代理。审阅给定改动或文件，按严重程度列出问题"
            "（正确性 / 安全 / 性能 / 可维护性）与具体修改建议。"
        ),
        allowed_tools=["read_file", "list_dir", "grep"],
    ),
    SubagentProfile(
        name="general",
        description="通用子代理：可读写工作区，完成探索、编辑、调研等综合子任务。",
        system_prompt=(
            "你是通用型子代理。围绕给定任务自主使用可用工具（读取、检索、必要时写入），"
            "完成后产出简洁、结构化的结果摘要，包含关键文件路径与结论。"
        ),
        allowed_tools=list(DEFAULT_WRITE_TOOLS),
    ),
]

_BY_NAME: Dict[str, SubagentProfile] = {p.name: p for p in BUILTIN_SUBAGENTS}


def get_subagent(name: str) -> SubagentProfile | None:
    return _BY_NAME.get(name)


def render_subagents_prompt() -> str:
    """渲染成可注入系统提示词的可用子代理清单。"""
    lines = ["可委派的内置子代理："]
    for profile in BUILTIN_SUBAGENTS:
        lines.append(f"- {profile.name}: {profile.description}")
    return "\n".join(lines)


def subagents_as_dicts() -> List[Dict[str, object]]:
    return [
        {
            "name": p.name,
            "description": p.description,
            "allowedTools": list(p.allowed_tools),
        }
        for p in BUILTIN_SUBAGENTS
    ]
