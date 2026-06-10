from __future__ import annotations

import re
from datetime import datetime, timezone
from typing import Any, Dict, List
from uuid import uuid4


def utc_now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def make_id(prefix: str) -> str:
    return f"{prefix}_{uuid4().hex[:10]}"


def to_camel_case(snake_str: str) -> str:
    components = snake_str.split('_')
    return components[0] + ''.join(x.title() for x in components[1:])


def dict_keys_to_camel_case(data: Any) -> Any:
    if isinstance(data, dict):
        return {to_camel_case(k): dict_keys_to_camel_case(v) for k, v in data.items()}
    elif isinstance(data, list):
        return [dict_keys_to_camel_case(item) for item in data]
    else:
        return data
