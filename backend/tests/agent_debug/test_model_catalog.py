from pathlib import Path

from src.agent_debug.provider.package_model_catalog import PackageModelCatalog
from src.agent_debug.provider.channel_store import ChannelStore
from src.agent_debug.provider.channels import Channel, ChannelModel


def _write_fake_package(package_root: Path) -> None:
    package_root.mkdir(parents=True, exist_ok=True)
    (package_root / "sdk-tools.d.ts").write_text(
        '\n'.join(
            [
                "export interface AgentInput {",
                '  model?: "sonnet" | "opus" | "haiku";',
                "}",
            ]
        ),
        encoding="utf-8",
    )


def test_package_model_catalog_reads_models_and_persists_default(tmp_path):
    package_root = tmp_path / "package"
    prefs_file = tmp_path / "agent_model_preferences.json"
    _write_fake_package(package_root)

    catalog = PackageModelCatalog(package_root=package_root, preferences_file=prefs_file)

    models = catalog.list_models()

    assert [item.id for item in models] == ["sonnet", "opus", "haiku"]
    assert models[0].is_default is True

    preferences = catalog.set_default_model_id("opus")

    assert preferences.global_default_model_id == "opus"
    assert catalog.get_default_model_id() == "opus"
    assert catalog.resolve_model() == "opus"
    assert catalog.resolve_model("haiku") == "haiku"


def test_package_model_catalog_rejects_unknown_model(tmp_path):
    package_root = tmp_path / "package"
    prefs_file = tmp_path / "agent_model_preferences.json"
    _write_fake_package(package_root)

    catalog = PackageModelCatalog(package_root=package_root, preferences_file=prefs_file)

    assert catalog.normalize_model_id("unknown-model") is None
    assert catalog.is_valid_model("unknown-model") is False


def test_channel_models_are_available_without_package_root(tmp_path):
    store = ChannelStore(store_dir=tmp_path)
    store.upsert_channel(
        Channel(
            id="chan-deepseek",
            name="deepseek",
            provider="deepseek",
            api_key="sk-test",
            models=[ChannelModel(id="deepseek-v4-pro")],
            enabled=True,
        )
    )
    catalog = PackageModelCatalog(
        package_root=tmp_path / "missing-package",
        preferences_file=tmp_path / "prefs.json",
        channel_store=store,
    )

    model = next(item for item in catalog.list_models() if item.id == "deepseek-v4-pro")

    assert model.source == "channel"
    assert model.provider == "deepseek"
    assert model.availability == "available"
    assert model.context_window_tokens == 1_000_000
    assert catalog.context_window_tokens("deepseek-v4-pro") == 1_000_000
    assert [item.id for item in catalog.list_models()] == ["deepseek-v4-pro"]
    assert catalog.normalize_model_id("sonnet") is None


def test_non_deepseek_models_keep_unknown_context_window(tmp_path):
    package_root = tmp_path / "package"
    prefs_file = tmp_path / "agent_model_preferences.json"
    _write_fake_package(package_root)

    catalog = PackageModelCatalog(package_root=package_root, preferences_file=prefs_file)

    model = next(item for item in catalog.list_models() if item.id == "sonnet")

    assert model.context_window_tokens is None
    assert catalog.context_window_tokens("sonnet") is None
