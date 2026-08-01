//! affinity.d 卡片加载集成测试 — 验证随 crate 发布的真实 TOML 卡片可装载
//!
//! 对应计划:M0 W4 PR-8(spec_loader + affinity.d 元数据外置,原则 P8)
//!
//! WHY 独立集成测试而非单测: 验证的是"随 crate 发布的 zhipu.toml/deepseek.toml
//! 真实文件"能被 spec_loader 正确解析并通过全部校验,是 P8 元数据外置的
//! 端到端保证(卡片写错在 CI 即暴露,而非上线后厂商调用失败)。

use mca_gateway::prelude::*;
use mca_gateway::spec_loader::{load_spec_dir, parse_spec_toml};
use nexus_contracts::affinity::{
    ProtocolDialect, ProviderId, StatePreservationPolicy, ThinkingSupport,
};
use std::path::PathBuf;

/// affinity.d 目录路径(随 crate 发布)
fn affinity_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("affinity.d")
}

#[test]
fn load_all_shipped_cards() {
    // M1 交付七厂商卡:zhipu(2) + deepseek(2) + moonshot(1) + minimax(1)
    // + volcano(3) + alicloud(2) + stepfun(1) = 12 个描述符
    let specs = load_spec_dir(&affinity_dir()).expect("随 crate 发布的卡片必须全部可加载");
    assert_eq!(specs.len(), 12, "M1 应有 12 个模型描述符(七厂商全接)");
}

#[test]
fn all_seven_providers_present() {
    // 七厂商全覆盖(C1:不为单一模型亲和,对所有模型做渠道亲和)
    let specs = load_spec_dir(&affinity_dir()).unwrap();
    for provider in [
        ProviderId::Zhipu,
        ProviderId::DeepSeek,
        ProviderId::Moonshot,
        ProviderId::VolcanoArk,
        ProviderId::AlibabaCloud,
        ProviderId::MiniMax,
        ProviderId::StepFun,
    ] {
        assert!(
            specs.iter().any(|s| s.provider == provider),
            "厂商 {provider:?} 必须有至少一张卡"
        );
    }
}

#[test]
fn shipped_cards_register_into_gateway() {
    // 端到端:加载卡片 → 注册进网关 → 按路由键查得
    let gateway = McaGateway::new(McaGatewayConfig::default());
    for spec in load_spec_dir(&affinity_dir()).unwrap() {
        gateway.register_spec(spec);
    }
    assert_eq!(gateway.spec_count(), 12);
    assert!(gateway.lookup_spec("zhipu/glm-5.2").is_some());
    assert!(gateway.lookup_spec("deep_seek/deepseek-v4-flash").is_some());
    assert!(gateway.lookup_spec("mini_max/MiniMax-M3").is_some());
    assert!(gateway
        .lookup_spec("step_fun/step-3.5-flash-2603")
        .is_some());
}

#[test]
fn minimax_card_enforces_verbatim_thinking() {
    // MiniMax 最高优先级怪癖:interleaved thinking 逐字回传(C9 硬约束)
    let specs = load_spec_dir(&affinity_dir()).unwrap();
    let m3 = specs
        .iter()
        .find(|s| s.route_key() == "mini_max/MiniMax-M3")
        .expect("MiniMax-M3 必须存在");
    assert_eq!(
        m3.capabilities.state_preservation,
        StatePreservationPolicy::VerbatimThinking,
        "MiniMax 必须声明 VerbatimThinking(strip 即断链)"
    );
}

#[test]
fn stepfun_card_is_smallest_window() {
    // Step 256K —— 七家最小窗口(P5 窗口亲和最严样本)
    let specs = load_spec_dir(&affinity_dir()).unwrap();
    let step = specs
        .iter()
        .find(|s| s.route_key() == "step_fun/step-3.5-flash-2603")
        .expect("step-3.5-flash-2603 必须存在");
    assert_eq!(step.capabilities.context_window, 262144);
}

#[test]
fn moonshot_card_prefers_anthropic() {
    // Kimi K3:Anthropic 块协议原生,OpenAI 转换路径降级——quirk 钉住 Anthropic
    let specs = load_spec_dir(&affinity_dir()).unwrap();
    let kimi = specs
        .iter()
        .find(|s| s.route_key() == "moonshot/kimi-k3")
        .expect("kimi-k3 必须存在");
    assert_eq!(
        kimi.preferred_dialect(),
        Some(ProtocolDialect::AnthropicMessages)
    );
}

#[test]
fn custom_channel_registers_zero_code() {
    // §6.8 Custom 通道零代码接入:用户填 base_url + model + 能力自查表即可注册
    let toml = r#"
schema_version = 1

[[models]]
provider = { custom = "openrouter" }
model = "anthropic/claude-x"
dialects = ["open_ai_chat"]

[models.capabilities]
streaming = true
tool_calling = true
thinking = "none"
context_window = 200000
max_output = 8192
prompt_caching = "none"
service_tiers = ["standard"]
state_preservation = "none"
modalities = ["text"]
structured_output = false

[models.pricing]
currency = "usd"
input_micro_per_mtok = 0
output_micro_per_mtok = 0
cache_hit_micro_per_mtok = 0
peak_periods = []

[models.endpoint]
base_url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"
timeout_ms = 120000
connect_timeout_ms = 10000
"#;
    let specs = parse_spec_toml(toml).expect("Custom 通道卡必须可解析");
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].provider, ProviderId::Custom("openrouter".into()));
    assert_eq!(specs[0].route_key(), "openrouter/anthropic/claude-x");
}

#[test]
fn glm_card_honors_anthropic_prefer_quirk() {
    // GLM 声明双方言,quirk 钉住 Anthropic 优先(工具链保真)
    let specs = load_spec_dir(&affinity_dir()).unwrap();
    let glm = specs
        .iter()
        .find(|s| s.route_key() == "zhipu/glm-5.2")
        .expect("glm-5.2 必须存在");
    assert_eq!(
        glm.preferred_dialect(),
        Some(ProtocolDialect::AnthropicMessages),
        "GLM 主力应优先 Anthropic 路径"
    );
    // reasoning_effort 七档(EffortLevels)正确解析
    assert!(
        matches!(glm.capabilities.thinking, ThinkingSupport::EffortLevels(ref v) if v.len() == 7)
    );
}

#[test]
fn deepseek_card_carries_peak_pricing() {
    // DeepSeek 峰谷定价(高峰 2×)与隐式缓存正确装载
    let specs = load_spec_dir(&affinity_dir()).unwrap();
    let flash = specs
        .iter()
        .find(|s| s.route_key() == "deep_seek/deepseek-v4-flash")
        .expect("deepseek-v4-flash 必须存在");
    assert_eq!(flash.provider, ProviderId::DeepSeek);
    assert_eq!(flash.pricing.peak_periods.len(), 1);
    assert_eq!(flash.pricing.peak_periods[0].factor_percent, 200);
    // 缓存命中 ¥0.01/M = 10_000 微元(隐式缓存族,会话粘性权重最高)
    assert_eq!(flash.pricing.cache_hit_micro_per_mtok, 10_000);
}

#[test]
fn shipped_cards_assemble_into_adapters() {
    // 装配为适配器(bus=None 静默模式):每张卡 preferred 方言必须有可用码器
    // 且装配后方言必在该卡声明的 dialects 集内(M1 三方言均有码器)
    for spec in load_spec_dir(&affinity_dir()).unwrap() {
        let route_key = spec.route_key();
        let declared = spec.dialects.clone();
        let adapter = VendorAdapter::assemble(std::sync::Arc::new(spec), None)
            .unwrap_or_else(|e| panic!("卡片 {route_key} 装配失败: {e}"));
        assert!(
            declared.contains(&adapter.dialect()),
            "{route_key} 装配方言 {:?} 必须在声明集 {declared:?} 内",
            adapter.dialect()
        );
    }
}
