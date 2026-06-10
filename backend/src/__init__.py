"""Top-level ``src`` package marker.

Making ``src`` a *regular* package (rather than an implicit namespace
package) ensures that when ``backend`` is placed at the front of ``sys.path``
(e.g. ``python -m src.agent_debug.server`` with cwd=backend, or an explicit
``sys.path.insert(0, ...)``), this local copy wins over any other ``src``
package that may be importable from the environment. Without this file the
namespace-package resolution could fall through to an unrelated ``src`` on the
path and load stale code from a different checkout.
"""
