from __future__ import annotations

from dataclasses import asdict
from typing import Any, Dict, Generic, List, Optional, TypeVar

T = TypeVar("T")


class InMemoryTable(Generic[T]):
    def __init__(self) -> None:
        self._items: Dict[str, T] = {}

    def save(self, key: str, value: T) -> T:
        self._items[key] = value
        return value

    def get(self, key: str) -> Optional[T]:
        return self._items.get(key)

    def delete(self, key: str) -> bool:
        return self._items.pop(key, None) is not None

    def list_all(self) -> List[T]:
        return list(self._items.values())

    def list_by(self, field_name: str, expected: Any) -> List[T]:
        results: List[T] = []
        for item in self._items.values():
            if getattr(item, field_name, None) == expected:
                results.append(item)
        return results

    def dump(self) -> List[Dict[str, Any]]:
        return [asdict(item) for item in self._items.values()]
