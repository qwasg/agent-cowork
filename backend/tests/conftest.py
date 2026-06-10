# ============================================================================
# Test session bootstrap for the document-processing / AI-compile backend.
# Keeps the test environment lightweight: no engine / GPU runtime is imported.
# ============================================================================

import importlib.machinery
import importlib.util
import os
import sys

import pytest

# ---------------------------------------------------------------------------
# 确保导入的 `src` 包指向「本工作区」而非其它项目。
#
# 本机存在其它项目（taichi engine 等）以 editable 方式安装了同名顶层包 `src`，
# 其 meta-path finder 会把 `src` 解析到别处目录，导致 pytest 误用其它代码树。
# 这里在测试启动时：
#   1) 把工作区 backend 目录插到 sys.path 最前；
#   2) 清掉已缓存的 `src` / `src.*` 模块；
#   3) 预先把 `src` 绑定为指向本工作区 backend/src 的命名空间包；
#   4) 移除会劫持 `src` 的第三方 editable meta-path finder。
# ---------------------------------------------------------------------------
_BACKEND_DIR = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
_SRC_DIR = os.path.join(_BACKEND_DIR, "src")

sys.path.insert(0, _BACKEND_DIR)

for _name in [n for n in sys.modules if n == "src" or n.startswith("src.")]:
    del sys.modules[_name]


def _is_editable_install_finder(finder: object) -> bool:
    """仅识别第三方 editable 安装注入的 meta-path finder（不动 stdlib finder）。"""
    module = getattr(type(finder), "__module__", "") or ""
    return module.startswith("__editable__") or "__editable__" in module


# 移除会把顶层包名（含 `src`）劫持到其它项目的 editable finder；
# 保留 BuiltinImporter / FrozenImporter / PathFinder 等标准 finder。
sys.meta_path[:] = [f for f in sys.meta_path if not _is_editable_install_finder(f)]

# 预绑定 `src` 命名空间包到工作区，避免子模块再被外部 finder 劫持。
if "src" not in sys.modules:
    _spec = importlib.machinery.ModuleSpec("src", loader=None, is_package=True)
    _spec.submodule_search_locations = [_SRC_DIR]  # type: ignore[assignment]
    sys.modules["src"] = importlib.util.module_from_spec(_spec)


@pytest.fixture
def temp_output_dir(tmp_path):
    """Create a temporary directory for test outputs."""
    d = tmp_path / "test_output"
    d.mkdir(exist_ok=True)
    return d


@pytest.fixture(autouse=True)
def _isolate_agent_data(tmp_path_factory, monkeypatch):
    """把会话索引/事件持久化目录隔离到独立临时目录。

    ``SessionService`` 现在把 ``agent_sessions.json`` 锚定到 backend 目录，
    若不隔离，测试中的 create/delete 会污染真实的会话数据文件。使用独立的
    ``tmp_path_factory`` 目录（而非测试共用的 ``tmp_path``），避免污染依赖
    工作目录列举的测试。
    """
    data_dir = tmp_path_factory.mktemp("agent-data")
    monkeypatch.setenv("AGENT_DEBUG_DATA_DIR", str(data_dir))
    monkeypatch.setenv("AGENT_DEBUG_SESSION_DIR", str(data_dir / "agent-sessions"))
    yield
