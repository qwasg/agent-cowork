"""Best-effort skill discovery for the dynamic system-reminder block and the
``read_skill`` tool.

Scans well-known skill directories for ``SKILL.md`` files and returns capped
records suitable for :func:`format_skills_system_reminder`, the ``read_skill``
tool, and the ``GET /skills`` REST endpoint.

Conventions supported (in priority order):

- Workspace level (scope ``workspace``):
  - ``<root>/.cursor/skills/*/SKILL.md``
  - ``<root>/.codex/skills/*/SKILL.md``
  - ``<root>/.claude/skills/*/SKILL.md``
  - ``<root>/skills/*/SKILL.md``
- User level (scope ``user``):
  - ``~/.cursor/skills/*/SKILL.md``
  - ``~/.codex/skills/*/SKILL.md``
  - ``~/.claude/skills/*/SKILL.md``

When a skill ``name`` exists at both scopes, the workspace copy wins (it is
discovered first). The implementation intentionally avoids globbing too widely
to keep startup cost negligible. ``_MAX_SKILLS`` and ``_SUMMARY_CHAR_CAP`` bound
the output size so we never blow the system prompt.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional, Tuple

_SKILL_FILE = "SKILL.md"
_SKILL_DIRS = (
    ".cursor/skills",
    ".codex/skills",
    ".claude/skills",
    "skills",
)
# User-level skill homes (scanned after the workspace so workspace wins on name clashes).
_USER_SKILL_DIRS = (
    ".cursor/skills",
    ".codex/skills",
    ".claude/skills",
)
_MAX_SKILLS = 32
_SUMMARY_CHAR_CAP = 280


@dataclass(frozen=True)
class SkillRecord:
    """A discovered skill: ``name`` (directory name), one-line ``summary``,
    absolute ``path`` to its ``SKILL.md`` and ``scope`` (``workspace``/``user``)."""

    name: str
    summary: str
    path: str
    scope: str


def _read_summary(skill_md: Path) -> str:
    try:
        text = skill_md.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""
    body = text.strip()
    if not body:
        return ""
    # Skip a leading ``#`` heading: the *first non-heading paragraph* is the
    # actual summary in practice. Fall back to the whole first line otherwise.
    first_line = body.splitlines()[0].strip()
    if first_line.startswith("#"):
        # find the next non-empty paragraph
        for chunk in body.split("\n\n"):
            paragraph = chunk.strip()
            if not paragraph or paragraph.startswith("#"):
                continue
            return paragraph[:_SUMMARY_CHAR_CAP]
        return first_line.lstrip("#").strip()[:_SUMMARY_CHAR_CAP]
    return first_line[:_SUMMARY_CHAR_CAP]


def _scan_dir(skills_root: Path, scope: str) -> List[SkillRecord]:
    if not skills_root.is_dir():
        return []
    try:
        entries = sorted(p for p in skills_root.iterdir() if p.is_dir())
    except OSError:
        return []
    found: List[SkillRecord] = []
    for skill_dir in entries:
        skill_md = skill_dir / _SKILL_FILE
        if not skill_md.is_file():
            continue
        summary = _read_summary(skill_md)
        if not summary:
            continue
        found.append(
            SkillRecord(
                name=skill_dir.name,
                summary=summary,
                path=str(skill_md.resolve()),
                scope=scope,
            )
        )
    return found


def discover_skill_records(root: str | Path | None) -> List[SkillRecord]:
    """Discover skills across workspace and user-level skill homes.

    Returns a de-duplicated (by ``name``) list capped at ``_MAX_SKILLS`` with
    workspace skills taking precedence over same-named user skills.
    """
    seen: set[str] = set()
    out: List[SkillRecord] = []

    def _absorb(records: List[SkillRecord]) -> bool:
        for record in records:
            if record.name in seen:
                continue
            seen.add(record.name)
            out.append(record)
            if len(out) >= _MAX_SKILLS:
                return True
        return False

    if root is not None:
        try:
            base = Path(root).resolve()
        except OSError:
            base = None
        if base is not None and base.is_dir():
            for rel_dir in _SKILL_DIRS:
                if _absorb(_scan_dir(base / rel_dir, "workspace")):
                    return out

    try:
        home = Path.home()
    except (OSError, RuntimeError):
        home = None
    if home is not None:
        for rel_dir in _USER_SKILL_DIRS:
            if _absorb(_scan_dir(home / rel_dir, "user")):
                return out

    return out


def discover_workspace_skills(root: str | Path | None) -> List[Tuple[str, str]]:
    """Backwards-compatible ``[(name, summary), ...]`` view of discovered skills."""
    return [(record.name, record.summary) for record in discover_skill_records(root)]


def resolve_skill_path(root: str | Path | None, name: str) -> Optional[str]:
    """Resolve a skill ``name`` to its absolute ``SKILL.md`` path, or ``None``."""
    if not name:
        return None
    target = name.strip()
    for record in discover_skill_records(root):
        if record.name == target:
            return record.path
    return None
