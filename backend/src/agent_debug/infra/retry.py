"""重试 / 退避基础设施。

参考 Proma ``agent-orchestrator.ts`` 的重试策略：对「限流 / 超时 / 瞬时网络」
类错误做指数退避 + 抖动重试，区别于「不可重试」错误（鉴权失败、请求非法等）。

- ``RetryConfig``：最大尝试次数、基础/最大延迟、抖动比例。
- ``compute_backoff_seconds``：指数退避 + 抖动。
- ``is_retryable_error`` / ``classify_error``：错误分类。
- ``with_retries``：保留旧签名的通用重试包装（向后兼容）。
"""

from __future__ import annotations

import asyncio
import random
import re
from dataclasses import dataclass, field
from typing import Awaitable, Callable, List, Optional, TypeVar

T = TypeVar("T")

# 可重试的错误码（与 service.normalize_provider_error 对齐）。
RETRYABLE_CODES = {"timeout", "rate_limited", "transient"}

# 瞬时网络错误的文本特征（连接重置 / DNS / 临时不可用等）。
_TRANSIENT_PATTERNS = re.compile(
    r"(connection reset|connection aborted|temporarily unavailable|timed out|"
    r"econnreset|etimedout|eai_again|socket hang up|503|502|504|overloaded)",
    re.IGNORECASE,
)


@dataclass
class RetryConfig:
    max_attempts: int = 4
    base_delay_seconds: float = 0.4
    max_delay_seconds: float = 15.0
    jitter_ratio: float = 0.25
    retryable_codes: List[str] = field(default_factory=lambda: list(RETRYABLE_CODES))


def classify_error(exc: Exception) -> str:
    """把异常归类为 ``timeout`` / ``rate_limited`` / ``transient`` / ``fatal``。"""
    message = str(exc).lower()
    status_code = getattr(exc, "status_code", None) or getattr(exc, "status", None)

    if isinstance(exc, (asyncio.TimeoutError, TimeoutError)) or "timeout" in message:
        return "timeout"
    if status_code == 429 or "429" in message or "rate limit" in message or "too many requests" in message:
        return "rate_limited"
    if status_code in (500, 502, 503, 504) or _TRANSIENT_PATTERNS.search(message):
        return "transient"
    return "fatal"


def is_retryable_error(exc: Exception, retryable_codes: Optional[set[str]] = None) -> bool:
    codes = retryable_codes or RETRYABLE_CODES
    return classify_error(exc) in codes


def compute_backoff_seconds(attempt: int, config: RetryConfig) -> float:
    """第 ``attempt`` 次重试（从 1 开始）的退避时长（含抖动）。"""
    exp = config.base_delay_seconds * (2 ** max(0, attempt - 1))
    capped = min(exp, config.max_delay_seconds)
    jitter = capped * config.jitter_ratio
    return max(0.0, capped + random.uniform(-jitter, jitter))


async def with_retries(
    operation: Callable[[], Awaitable[T]],
    *,
    attempts: int = 3,
    base_delay_seconds: float = 0.3,
    config: Optional[RetryConfig] = None,
    on_retry: Optional[Callable[[int, Exception, float], None]] = None,
) -> T:
    cfg = config or RetryConfig(max_attempts=attempts, base_delay_seconds=base_delay_seconds)
    last_error: Exception | None = None
    for attempt in range(1, cfg.max_attempts + 1):
        try:
            return await operation()
        except Exception as exc:
            last_error = exc
            if attempt >= cfg.max_attempts or not is_retryable_error(exc, set(cfg.retryable_codes)):
                break
            delay = compute_backoff_seconds(attempt, cfg)
            if on_retry is not None:
                on_retry(attempt, exc, delay)
            await asyncio.sleep(delay)
    if last_error is None:
        raise RuntimeError("Retry helper failed without exception")
    raise last_error
