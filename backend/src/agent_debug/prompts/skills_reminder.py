"""Skills discovery placeholder: character budget + <system-reminder> shape (easy-agent aligned)."""

from __future__ import annotations

# Reasonable default cap for injected skill blurbs (align with easy-agent budget idea).
DEFAULT_BUDGET_CHARS = 12_000


def format_skills_system_reminder(
    items: list[tuple[str, str]] | None,
    *,
    budget_chars: int = DEFAULT_BUDGET_CHARS,
) -> str:
    """Format available skills as a system-reminder block. Empty or missing list returns "".

    Each item is ``(name, body)`` — name is a short label; body is one-line or short description.
    """
    if not items:
        return ""
    parts: list[str] = []
    used = 0
    for name, body in items:
        line = f"- **{name}**: {body}\n"
        if used + len(line) > budget_chars:
            break
        parts.append(line)
        used += len(line)
    if not parts:
        return ""
    inner = "".join(parts).rstrip()
    return (
        "<system-reminder>\n"
        "Available skills. When one is relevant to the task, call the `read_skill` "
        "tool with its name to load the full SKILL.md, then follow its instructions:\n"
        f"{inner}\n"
        "</system-reminder>"
    )
