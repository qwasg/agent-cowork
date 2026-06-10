from __future__ import annotations

from dataclasses import dataclass


@dataclass
class CircuitBreaker:
    failure_threshold: int = 5
    failures: int = 0
    open: bool = False

    def on_success(self) -> None:
        self.failures = 0
        self.open = False

    def on_failure(self) -> None:
        self.failures += 1
        if self.failures >= self.failure_threshold:
            self.open = True

    def ensure_available(self) -> None:
        if self.open:
            raise RuntimeError("Circuit breaker is open")
