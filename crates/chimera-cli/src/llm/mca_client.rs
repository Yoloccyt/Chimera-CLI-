//! `McaLlmClient` —— 经 L10 `mca-gateway` 的真实厂商客户端（Phase 7 / T7.6，ADR-232）
//!
//! 对应架构层: L10 Interface（chimera-cli，feature `mca`）
//!
//! # 只做两件事
//! 1. 从 `config.model_router.providers[0]` 装配一个 [`VendorAdapter`]
//!    （`ModelAffinitySpec::minimal` + `CHIMERA_LLM_MODEL` 覆盖模型名）；
//! 2. 把 `AffinityError` 映射成本层的 [`LlmError`]。
//!
//! **不改 `VendorAdapter`、不自己拼 HTTP**：传输、方言编解码、熔断、成本回算、
//! 语义缓存全在 gateway 内（那是它的既有职责面）。本文件是薄壳。
//!
//! # 端点与 allowlist
//! `minimal` 走 gateway 既有 `Transport` allowlist；自定义端点需要显式放行，
//! 本层不擅自放开（放开等于把"配置里的 endpoint 字符串"当成可信输入）。
//! 因此本客户端**只保证装配正确**；真实联网由 gateway 侧既有测试与
//! `mca-feature` CI job 承担，本模块不新增网络测试（§7.4 离线纪律）。

use async_trait::async_trait;
use mca_gateway::adapters::VendorAdapter;
use mca_gateway::error::AffinityError;
use nexus_contracts::affinity::{
    AffinityRequest, AffinityResponse, ModelAffinitySpec, ProtocolDialect, ProviderId,
};

use super::client::{LlmClient, LlmError};
use crate::config::ChimeraConfig;

/// provider 名 → `ProviderId`
///
/// WHY 未知名落 `Custom`：`ProviderId::Custom` 是契约里为厂商白名单外通道预留的
/// 一等变体（路由臂 `provider/model/mode` 第一段），把未知名映射成"某个已知厂商"
/// 会伪造路由留痕。
#[must_use]
pub fn provider_id_from_name(name: &str) -> ProviderId {
    let normalized = name.trim().to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "zhipu" | "glm" | "z_ai" => ProviderId::Zhipu,
        "deepseek" | "deep_seek" => ProviderId::DeepSeek,
        "moonshot" | "kimi" => ProviderId::Moonshot,
        "volcano" | "volcano_ark" | "ark" => ProviderId::VolcanoArk,
        "alibaba" | "alibaba_cloud" | "dashscope" | "qwen" => ProviderId::AlibabaCloud,
        "minimax" | "mini_max" => ProviderId::MiniMax,
        "step" | "step_fun" => ProviderId::StepFun,
        _ => ProviderId::Custom(name.to_string().into_boxed_str()),
    }
}

/// 厂商客户端（薄壳）
pub struct McaLlmClient {
    adapter: VendorAdapter,
    /// 供 `LlmClient::provider_id` 与日志用的稳定标签
    label: String,
}

impl McaLlmClient {
    /// 从配置装配一个客户端
    ///
    /// # 错误
    /// 配置里没有 provider ⇒ [`LlmError::ProviderUnavailable`]（调用方映射为
    /// `ConfigError`，退出码 1 —— 这是"用户可纠正"而非引擎故障）
    pub fn from_config(config: &ChimeraConfig) -> Result<Self, LlmError> {
        let provider =
            config.model_router.providers.first().ok_or_else(|| {
                LlmError::ProviderUnavailable("model_router.providers 为空".into())
            })?;

        let provider_id = provider_id_from_name(&provider.name);
        // 模型名：`CHIMERA_LLM_MODEL` 优先，缺省用 provider.name（不猜厂商默认型号，
        // 因为型号决定上下文窗口与计费，猜错会以"未知模型"形式在真实调用才炸）
        let model = std::env::var("CHIMERA_LLM_MODEL").unwrap_or_else(|_| provider.name.clone());
        let dialect = match std::env::var("CHIMERA_LLM_DIALECT").ok().as_deref() {
            Some("anthropic") => ProtocolDialect::AnthropicMessages,
            Some("responses") => ProtocolDialect::OpenAiResponses,
            _ => ProtocolDialect::OpenAiChat,
        };
        let spec = ModelAffinitySpec::minimal(provider_id, &model, dialect);
        let adapter = VendorAdapter::assemble(std::sync::Arc::new(spec), None)
            .map_err(affinity_to_llm_error)?;
        Ok(Self {
            adapter,
            label: format!("mca:{}", provider.name),
        })
    }
}

/// `AffinityError` → `LlmError`（不新增错误面：gateway 的错误分三类映射）
fn affinity_to_llm_error(error: AffinityError) -> LlmError {
    let text = error.to_string();
    // WHY 字符串分类：AffinityError 的变体形状是 `Transport{..}/Protocol{..}` 带字段，
    // 其 Display 已含类别前缀；在此匹配避免对本层不需要的细节做穷举耦合。
    if text.contains("transport") || text.contains("timeout") || text.contains("status") {
        LlmError::Transport(text)
    } else {
        LlmError::Protocol(text)
    }
}

#[async_trait]
impl LlmClient for McaLlmClient {
    fn provider_id(&self) -> &str {
        &self.label
    }

    async fn complete(&self, request: &AffinityRequest) -> Result<AffinityResponse, LlmError> {
        self.adapter
            .invoke(request)
            .await
            .map_err(affinity_to_llm_error)
    }
}
