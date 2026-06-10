"""中国大模型相关：渠道、思考能力、CJK token、协议适配器转换。"""

from __future__ import annotations

from src.agent_debug.provider.anthropic_adapter import (
    _convert_messages,
    _convert_tools,
    _parse_response_content,
)
from src.agent_debug.provider.base import ModelRequestContext
from src.agent_debug.provider.channels import (
    Channel,
    default_base_url,
    is_anthropic_protocol,
    is_china_provider,
    provider_protocol,
)
from src.agent_debug.provider.cjk_token_estimator import estimate_messages_tokens, estimate_tokens
from src.agent_debug.provider.openai_compat_adapter import OpenAICompatibleProvider
from src.agent_debug.provider.thinking_capability import (
    apply_thinking_to_openai_request,
    detect_thinking_capability,
)


def test_provider_default_urls_and_protocol():
    assert default_base_url("qwen") == "https://dashscope.aliyuncs.com/compatible-mode/v1"
    assert default_base_url("zhipu") == "https://open.bigmodel.cn/api/paas/v4"
    assert default_base_url("doubao").startswith("https://ark.cn-beijing.volces.com")
    assert provider_protocol("kimi-api") == "anthropic"
    assert provider_protocol("qwen") == "openai"
    assert is_anthropic_protocol("minimax") is True
    assert is_china_provider("deepseek") is True
    assert is_china_provider("openai") is False


def test_channel_post_init_fills_base_url_and_protocol():
    channel = Channel(id="c1", name="千问", provider="qwen", api_key="sk-x", models=[])
    assert channel.base_url == "https://dashscope.aliyuncs.com/compatible-mode/v1"
    assert channel.protocol == "openai"
    assert channel.is_china is True


def test_thinking_capability_detection():
    assert detect_thinking_capability("qwen", "qwen-max").mode == "qwen-enable-flag"
    assert detect_thinking_capability("zhipu", "glm-4.6").mode == "glm-thinking-flag"
    assert detect_thinking_capability("deepseek", "deepseek-reasoner").mode == "manual-only"
    assert detect_thinking_capability("deepseek", "deepseek-v4-pro").mode == "effort-based-max"
    assert detect_thinking_capability("kimi-api", "kimi-k2").mode == "none"


def test_apply_thinking_to_openai_request_variants():
    qwen = apply_thinking_to_openai_request({}, detect_thinking_capability("qwen", "qwen-max"), enabled=True)
    assert qwen["extra_body"]["enable_thinking"] is True

    glm = apply_thinking_to_openai_request({}, detect_thinking_capability("zhipu", "glm-4.6"), enabled=False)
    assert glm["thinking"] == {"type": "disabled"}

    ds_on = apply_thinking_to_openai_request({}, detect_thinking_capability("deepseek", "deepseek-v4-pro"), enabled=True)
    assert ds_on["extra_body"]["output_config"]["effort"] == "max"

    ds_off = apply_thinking_to_openai_request({}, detect_thinking_capability("deepseek", "deepseek-v4-pro"), enabled=False)
    assert ds_off["thinking"] == {"type": "disabled"}


def test_cjk_token_estimator_weights_cjk_higher():
    cjk = estimate_tokens("你好世界你好世界")  # 8 个汉字
    latin = estimate_tokens("hello world hello")  # 约 17 字符
    assert cjk >= 5  # 8 / 1.5 ≈ 5.3
    assert latin <= 6
    total = estimate_messages_tokens([
        {"role": "user", "content": "你好"},
        {"role": "assistant", "content": "hi"},
    ])
    assert total > 0


def test_openai_compat_build_kwargs_injects_thinking():
    provider = OpenAICompatibleProvider(api_key="sk-test", provider_type="qwen", default_model="qwen-max")
    ctx = ModelRequestContext(request_id="r", trace_id="t", model="qwen-max", timeout_ms=1000)
    kwargs = provider._build_kwargs({"messages": [{"role": "user", "content": "hi"}]}, ctx)
    assert kwargs["model"] == "qwen-max"
    assert kwargs.get("extra_body", {}).get("enable_thinking") is True


def test_anthropic_convert_tools_and_messages():
    tools = _convert_tools([
        {"type": "function", "function": {"name": "read_file", "description": "d", "parameters": {"type": "object"}}}
    ])
    assert tools[0]["name"] == "read_file"
    assert "input_schema" in tools[0]

    system, msgs = _convert_messages([
        {"role": "system", "content": "你是助手"},
        {"role": "user", "content": "读取文件"},
        {
            "role": "assistant",
            "content": "",
            "tool_calls": [
                {"id": "call_1", "function": {"name": "read_file", "arguments": '{"path": "a.py"}'}}
            ],
        },
        {"role": "tool", "tool_call_id": "call_1", "name": "read_file", "content": "file body"},
    ])
    assert system == "你是助手"
    # user + assistant(tool_use) + user(tool_result)
    assert msgs[0]["role"] == "user"
    assistant_blocks = msgs[1]["content"]
    assert any(b["type"] == "tool_use" and b["name"] == "read_file" for b in assistant_blocks)
    tool_result_msg = msgs[2]
    assert tool_result_msg["role"] == "user"
    assert tool_result_msg["content"][0]["type"] == "tool_result"


def test_anthropic_parse_response_content_extracts_text_thinking_tools():
    text, tool_calls, reasoning = _parse_response_content([
        {"type": "thinking", "thinking": "让我想想"},
        {"type": "text", "text": "答案"},
        {"type": "tool_use", "id": "t1", "name": "grep", "input": {"q": "x"}},
    ])
    assert text == "答案"
    assert reasoning == "让我想想"
    assert tool_calls[0].name == "grep"
    assert tool_calls[0].arguments == {"q": "x"}
