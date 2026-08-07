//! 多通道亲和网关 — MCA(Model-Channel Affinity)体系的 L10 通道层
//!
//! 对应架构层:L10 Interface(与 chtc-bridge/mcp-mesh 同级同构)
//! 对应 ADR:**ADR-065**(MCA 总纲与 L10 网关,PANTHEON 计划)
//! 对应设计源:`Chimera_全模型亲和适配体系设计文档_v1.0.md`
//!
//! # 核心机制
//! - 三协议码器(OpenAI Chat / Anthropic Messages / OpenAI Responses),enum 分发
//! - spec 驱动的通用厂商适配器(VendorAdapter),厂商差异全部数据化为
//!   `ModelAffinitySpec`(affinity.d/*.toml 外置),代码零厂商字符串(原则 P8)
//! - SSE 流式归一器:三方言流式语法 → 统一 `StreamEvent`
//! - 能力协商(CapabilitySet)取代名字嗅探(原则 P1,Claude Code 尸检教训)
//!
//! # 架构约束
//! 本 crate 仅依赖 L0(nexus-contracts)与 L1(event-bus),不直接依赖 L2-L9
//! 任何 crate。跨层通信只走 EventBus;**流式数据面(per-token delta)走专用
//! bounded mpsc 直连调用方,不进 event-bus**(ADR-065 决策 4:broadcast 1024
//! 容量承载不了 per-token 流,Lagged 丢弃会破坏 TUI 体验)。
//!
//! # 模块职责矩阵
//! | 模块 | 职责 | 关键类型 |
//! |------|------|--------|
//! | capability | 能力协商(TTG→指令→三态保真度) | NegotiationOutcome, ThinkingDirective |
//! | cost | 成本预估/回算纯函数(无状态) | CostEstimate |
//! | cost_guard | 成本熔断状态机(原子无锁) | CostGuard |
//! | adapters | 请求全周期编排(invoke 流水线) | VendorAdapter |
//! | codec | 三协议方言编解码(enum 分发) | Codec, DecodedResponse |
//! | spec_loader | affinity.d/*.toml 装载与边界校验 | parse_spec_toml |
//! | gateway | 通道注册/查找/路由(McaGateway) | McaGateway |
//!
//! # 快速示例
//! ```
//! use mca_gateway::{McaGateway, McaGatewayConfig};
//! use nexus_contracts::affinity::{ModelAffinitySpec, ProviderId, ProtocolDialect};
//!
//! let gateway = McaGateway::new(McaGatewayConfig::default());
//! let spec = ModelAffinitySpec::minimal(
//!     ProviderId::Zhipu,
//!     "glm-5.2",
//!     ProtocolDialect::OpenAiChat,
//! );
//! gateway.register_spec(spec);
//! assert!(gateway.lookup_spec("zhipu/glm-5.2").is_some());
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod adapters;
pub mod capability;
pub mod coalescing;
pub mod codec;
pub mod conversation_trim;
pub(crate) mod cost;
pub mod cost_guard;
pub mod early_stop;
pub mod error;
pub mod gateway;
pub mod hcw_integration;
pub mod health;
pub mod prompt_compress;
pub mod prompt_norm;
pub mod semantic_fingerprint;
pub mod session;
pub mod spec_loader;
pub mod sse;
pub mod token_estimate;
pub mod transport;

// === 关键类型重导出,简化外部导入 ===
pub use adapters::{AdapterOptions, VendorAdapter};
pub use capability::{negotiate, negotiate_budget, NegotiationOutcome, ThinkingDirective};
pub use coalescing::{
    coalesce_failure, CoalesceKey, CoalesceResult, JoinOutcome, RequestCoalescer,
};
pub use codec::{Codec, DecodedResponse};
pub use conversation_trim::{conversation_budget, estimate_tokens, trim_to_budget};
pub use cost_guard::{CostGuard, CostGuardError};
pub use early_stop::{EarlyStopController, StopDecision, StopReason};
pub use error::AffinityError;
pub use gateway::{McaGateway, McaGatewayConfig};
pub use hcw_integration::spawn_hcw_integration;
pub use health::{ChannelHealth, HealthRegistry};
pub use prompt_compress::PromptCompressor;
pub use prompt_norm::{
    build_token_cache_key, compute_system_prompt_hash, compute_tool_schema_hash, layout_messages,
    NormalizedPrompt,
};
pub use semantic_fingerprint::{semantic_fingerprint, FINGERPRINT_DIM};
pub use session::{apply_preservation_policy, migrate_history, MigrationResult, SessionStore};
pub use spec_loader::{
    apply_profile_override, load_profile_dir, load_spec_dir, load_spec_dir_with_profiles,
    parse_profile_toml, parse_spec_toml, parse_spec_toml_with_profiles, ClientRelevant,
    DeploymentProfile, ProfileMeta,
};
pub use sse::{SseParser, StreamEvent, StreamNormalizer};
pub use token_estimate::{estimate_text, TokenEstimator};
pub use transport::{CircuitBreaker, RateLimiter, Transport};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::adapters::VendorAdapter;
    pub use crate::capability::{negotiate, NegotiationOutcome, ThinkingDirective};
    pub use crate::codec::{Codec, DecodedResponse};
    pub use crate::error::AffinityError;
    pub use crate::gateway::{McaGateway, McaGatewayConfig};
    pub use crate::health::HealthRegistry;
    pub use crate::session::{apply_preservation_policy, migrate_history, SessionStore};
    pub use crate::spec_loader::{load_spec_dir, parse_spec_toml};
    pub use crate::sse::{StreamEvent, StreamNormalizer};
    // L0 契约类型转发:调用方通常同时需要 spec/请求/响应类型
    pub use nexus_contracts::affinity::{
        AffinityRequest, AffinityResponse, CapabilitySet, ModelAffinitySpec, ProtocolDialect,
        ProviderId,
    };
}
