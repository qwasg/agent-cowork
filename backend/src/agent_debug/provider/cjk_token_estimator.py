"""CJK 友好的 token 估算。

参考 Proma ``agent-tool-token-estimator.ts`` 的思路：英文按 ~4 字符/token，
中日韩等表意文字 token 密度更高（约 1.5 字符/token）。该估算无需依赖
具体厂商分词器，用于上下文窗口预算与大文件写入拆分提示等粗略判断。
"""

from __future__ import annotations

from typing import Any, Iterable

# 经验比率：每 token 对应的字符数。
_CHARS_PER_TOKEN_LATIN = 4.0
_CHARS_PER_TOKEN_CJK = 1.5


def _is_cjk(ch: str) -> bool:
    code = ord(ch)
    return (
        0x4E00 <= code <= 0x9FFF  # CJK 统一表意文字
        or 0x3040 <= code <= 0x30FF  # 平假名 / 片假名
        or 0x3400 <= code <= 0x4DBF  # CJK 扩展 A
        or 0xAC00 <= code <= 0xD7A3  # 谚文音节
        or 0xFF00 <= code <= 0xFFEF  # 全角符号
    )


def estimate_tokens(text: str) -> int:
    """估算单段文本的 token 数（CJK 加权）。"""
    if not text:
        return 0
    cjk = 0
    other = 0
    for ch in text:
        if ch.isspace():
            other += 1
            continue
        if _is_cjk(ch):
            cjk += 1
        else:
            other += 1
    tokens = cjk / _CHARS_PER_TOKEN_CJK + other / _CHARS_PER_TOKEN_LATIN
    return int(tokens) + 1


def _text_of(value: Any) -> str:
    """从任意消息内容里抽取可估算的纯文本。"""
    if value is None:
        return ""
    if isinstance(value, str):
        return value
    if isinstance(value, dict):
        parts: list[str] = []
        for key in ("text", "content", "value", "input", "arguments"):
            sub = value.get(key)
            if sub is not None:
                parts.append(_text_of(sub))
        return "\n".join(p for p in parts if p)
    if isinstance(value, (list, tuple)):
        return "\n".join(_text_of(v) for v in value)
    return str(value)


def estimate_message_tokens(message: dict[str, Any]) -> int:
    """估算单条 chat message 的 token 数（含角色与工具调用开销）。"""
    base = 4  # 角色 / 分隔符固定开销
    base += estimate_tokens(_text_of(message.get("content")))
    tool_calls = message.get("tool_calls")
    if isinstance(tool_calls, list):
        for call in tool_calls:
            base += estimate_tokens(_text_of(call))
    return base


def estimate_messages_tokens(messages: Iterable[dict[str, Any]]) -> int:
    return sum(estimate_message_tokens(m) for m in messages)
