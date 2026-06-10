"""Read-only, capped project files from the workspace root (e.g. AGENT.md)."""

from __future__ import annotations

from pathlib import Path

# Cursor-style primary name; "AGENT" as secondary for parity with some setups.
AGENT_FILE_CANDIDATES: tuple[str, ...] = ("AGENT.md", "AGENT")
MAX_AGENT_MD_CHARS = 8_000
MAX_AGENT_MD_LINES = 200


def read_agent_md_snippet(
    root: str | Path | None,
    *,
    max_chars: int = MAX_AGENT_MD_CHARS,
    max_lines: int = MAX_AGENT_MD_LINES,
) -> str | None:
    """If ``root`` is set, try to read ``AGENT.md`` or ``AGENT`` under that directory.

    Truncates by line count then by character count. On missing file, permission error, or
    invalid path, returns ``None`` (caller should not surface errors to the user).
    """
    if root is None:
        return None
    try:
        base = Path(root).resolve()
    except OSError:
        return None
    for name in AGENT_FILE_CANDIDATES:
        path = base / name
        if not path.is_file():
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            return None
        lines = text.splitlines()
        if len(lines) > max_lines:
            text = "\n".join(lines[:max_lines]) + "\n... (truncated: line cap)"
        if len(text) > max_chars:
            text = text[:max_chars] + "\n... (truncated: size cap)"
        return text
    return None
