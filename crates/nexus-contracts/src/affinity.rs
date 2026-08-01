//! 模型亲和契约 — MCA（Model-Channel Affinity）体系 L0 类型
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts）
//! 对应 ADR: **ADR-065**（MCA 总纲与 L10 mca-gateway 网关）
//! 对应设计源: `Chimera_全模型亲和适配体系设计文档_v1.0.md` §4.1（PANTHEON 计划）
//!
//! # 核心职责
//!
//! 承载"能力协商取代名字嗅探"（原则 P1）所需的全部纯类型契约：
//!
//! | 类型 | 用途 | 消费层 |
//! |------|------|--------|
//! | [`ProviderId`] | 厂商标识（闭集枚举） | L10 mca-gateway / L1 model-router / L8 parliament |
//! | [`ProtocolDialect`] | 三协议方言 | L10 Codec 分发 |
//! | [`CapabilitySet`] | 能力集——能力协商的唯一事实源 | L10 capability 协商 / L6 路由掩码 |
//! | [`ModelAffinitySpec`] | 模型亲和描述符（每厂商每模型一张，TOML 外置） | L10 spec_loader / L1 路由 |
//! | [`AffinityRequest`] / [`AffinityResponse`] | 网关与上层之间的统一请求/响应契约 | L10 网关 / L9 quest-engine |
//! | [`ContentBlock`] | 统一块模型（对齐 Anthropic 内容块语义） | L10 Codec / L2 mlc-engine 会话记忆 |
//!
//! # 设计决策（WHY）
//!
//! - **能力协商，禁止名字嗅探（P1）**: 任何特性启用决策必须查询 [`CapabilitySet`]，
//!   禁止 `model_name.contains("glm")` 式判断——Claude Code 尸检教训：按模型名做
//!   能力门控导致第三方模型接入即"降级残血"。
//! - **QuirkRule 为闭集枚举而非 free-form map**: 厂商怪癖规则数据化，TOML 拼错
//!   在反序列化阶段即失败，而非运行时静默忽略；新增怪癖 = 新增变体 + ADR 记录。
//! - **工具入参用原始 JSON 字符串（`Box<str>`）**: L0 零 crate 依赖铁律（ADR-033）
//!   禁止引入 `serde_json::Value`；由 L10 Codec 负责 JSON 解析/构造，L0 仅承载传输形态。
//! - **`Box<str>` 而非 `String`**: 契约字段构造后不可变，`Box<str>` 省一个容量字段
//!   （16 字节 vs 24 字节），对齐既有 harness_spec.rs 惯例。
//! - **`ThinkingPreference` 是 L1 `nexus_core::ThinkingMode` 的镜像**:
//!   L0 不能依赖 L1（依赖方向 L0 ← L1），TTG 三档语义在此镜像定义；
//!   两处必须保持同步（镜像注释互指），长期可评估将 ThinkingMode 下沉 L0。
//!
//! # 示例
//!
//! ```
//! use nexus_contracts::affinity::{
//!     CapabilitySet, ModelAffinitySpec, ProviderId, ProtocolDialect, ThinkingSupport,
//! };
//!
//! // 能力协商（P1）：查询描述符而非嗅探模型名
//! let spec = ModelAffinitySpec::minimal(
//!     ProviderId::Zhipu,
//!     "glm-5.2",
//!     ProtocolDialect::OpenAiChat,
//! );
//! assert_eq!(spec.route_key(), "zhipu/glm-5.2");
//! assert!(spec.supports_dialect(ProtocolDialect::OpenAiChat));
//! assert!(!spec.capabilities.thinking.is_supported());
//! ```

use serde::{Deserialize, Serialize};

// ============================================================
// 厂商与协议标识
// ============================================================

/// 厂商标识 — 闭集枚举（新增厂商 = 新增变体 + ADR 记录）
///
/// # 设计决策（WHY）
/// - **闭集而非字符串**: 编译期穷尽检查，防止拼写漂移；`Custom` 变体承载
///   聚合网关/自部署（vLLM/Ollama/OpenRouter 等）的开放世界扩展。
/// - **serde rename snake_case**: TOML 配置中书写 `provider = "zhipu"`，
///   与 affinity.d/*.toml 命名约定一致。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    /// 智谱 GLM（bigmodel.cn，OpenAI + Anthropic 双端点）
    Zhipu,
    /// DeepSeek（api.deepseek.com，OpenAI + Anthropic + Responses 三协议）
    DeepSeek,
    /// Kimi / Moonshot（api.moonshot.cn，Anthropic 协议原生）
    Moonshot,
    /// 豆包 / 火山方舟（字节 Seed，OpenAI 兼容）
    VolcanoArk,
    /// Qwen / 阿里云百炼（DashScope compatible-mode）
    AlibabaCloud,
    /// MiniMax（OpenAI + Anthropic 双协议，interleaved thinking 回传怪癖）
    MiniMax,
    /// 阶跃星辰 Step（OpenAI 兼容，low think mode）
    StepFun,
    /// 自定义通道 — 聚合网关/自部署（vLLM/Ollama/OpenRouter 等）
    ///
    /// WHY 开放变体: 新厂商接入零代码（P8 元数据外置），用户填 base_url +
    /// 能力自查表即可注册；载荷为通道命名标识（如 "openrouter" / "local-vllm"）。
    Custom(Box<str>),
}

impl ProviderId {
    /// 返回厂商的稳定字符串标识（用于路由键 / SQLite 列 / 事件留痕）
    ///
    /// WHY 稳定标识: 路由历史与学习臂（`provider/model/mode`）需要跨进程
    /// 稳定的文本形态；与 serde snake_case 命名保持一致。
    pub fn as_str(&self) -> &str {
        match self {
            Self::Zhipu => "zhipu",
            Self::DeepSeek => "deep_seek",
            Self::Moonshot => "moonshot",
            Self::VolcanoArk => "volcano_ark",
            Self::AlibabaCloud => "alibaba_cloud",
            Self::MiniMax => "mini_max",
            Self::StepFun => "step_fun",
            Self::Custom(name) => name,
        }
    }
}

/// 协议方言 — 国内七厂商 API 的三种协议格局
///
/// 结论 F1（设计文档 §1.1）：协议层只有三种方言，七家厂商全部落在其内。
/// 渠道亲和的本质 = 三个协议码器（Codec）+ 每厂商一张能力描述符。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolDialect {
    /// OpenAI Chat Completions（`/v1/chat/completions`，覆盖面最广）
    OpenAiChat,
    /// Anthropic Messages（`/v1/messages`，thinking 块/tool_use 块原生）
    AnthropicMessages,
    /// OpenAI Responses API（DeepSeek V4-Flash 已原生支持）
    OpenAiResponses,
}

// ============================================================
// 能力集（能力协商的唯一事实源，P1）
// ============================================================

/// 思考模式支持度 — 厂商思考参数的能力抽象
///
/// # 设计决策（WHY）
/// TTG 三档（Fast/Standard/Deep）是全局语义，各厂商物理参数完全不同
/// （reasoning_effort 七档 / enable_thinking 开关 / thinking budget）。
/// 能力协商按此三态映射：`None` → TTG 强制 Fast 并降级留痕；`OnOff` →
/// Fast=关、Standard/Deep=开；`EffortLevels` → 按档位表精确映射，
/// 请求档位不在取值域内时就近取档并留痕。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingSupport {
    /// 不支持思考模式（协商时 TTG 强制 Fast，发布降级事件）
    None,
    /// 仅开/关两态（如 Qwen enable_thinking、豆包深度思考开关）
    OnOff,
    /// 多档位取值域（如 GLM reasoning_effort: none→max 七档）
    ///
    /// 载荷为厂商声明的档位名列表（保序，从弱到强），协商算法据此就近取档。
    EffortLevels(Vec<Box<str>>),
}

impl ThinkingSupport {
    /// 该模型是否支持任何形式的思考模式
    pub fn is_supported(&self) -> bool {
        !matches!(self, Self::None)
    }
}

/// prompt 缓存支持度 — 显式控制族与隐式自动族两分
///
/// 显式族（Anthropic 路径 cache_control）由 scc-cache 打断点；
/// 隐式族（DeepSeek/豆包自动命中）由路由层做会话粘性最大化命中率。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheSupport {
    /// 无 prompt 缓存
    None,
    /// 隐式自动缓存（厂商自动命中，返回 cache_hit token 数）
    Implicit,
    /// 显式控制（cache_control 断点，Anthropic 路径通用）
    ExplicitControl,
}

/// 会话状态守恒策略 — 厂商要求回传的中间态处理方式（原则 P4）
///
/// # 设计决策（WHY）
/// MiniMax M3 断链教训：interleaved thinking 内容必须逐字回传，strip 即
/// 多轮工具调用退化。守恒策略由描述符声明（数据），会话层按策略执行（代码），
/// 不写死 `if provider == MiniMax`（P1 禁止名字嗅探）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatePreservationPolicy {
    /// 无状态直通（无回传要求的模型）
    None,
    /// thinking 块与 tool_use 块按原序回传（Anthropic 路径通用，Kimi K3 要求）
    BlockPreservation,
    /// 思考内容逐字保真回传，禁止任何 strip 优化（MiniMax M3 专项）
    VerbatimThinking,
}

/// 服务分层 — 厂商提供的服务档位（加价档必须显式授权，P6 成本先行）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceTier {
    /// 标准档（默认）
    Standard,
    /// 优先档（如 MiniMax priority 1.5× 价，必须 BudgetMask 显式授权）
    Priority,
    /// 批量档（离线批处理折扣价）
    Batch,
}

/// 模态种类 — 模型支持的输入模态
///
/// WHY 命名 `ModalityKind` 而非 `Modality`: L2 `nmc-encoder` 已有同名
/// `Modality` 类型（编码器视角），L0 契约命名加 Kind 后缀避免下游同时
/// 导入两 prelude 时歧义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModalityKind {
    /// 文本
    Text,
    /// 图像输入
    Image,
    /// 视频输入
    Video,
    /// 音频输入
    Audio,
}

/// 能力集 — 能力协商的唯一事实源（原则 P1）
///
/// 任何特性（思考模式/缓存/工具调用/流式事件）必须通过本描述符查询，
/// 禁止按模型名推断。字段值以**实测**为准，非厂商宣传值（P6）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilitySet {
    /// 是否支持 SSE 流式输出（核心能力，缺失则通道进入 ChannelRejected）
    pub streaming: bool,
    /// 是否支持工具调用（核心能力，缺失则工具任务路由掩零）
    pub tool_calling: bool,
    /// 思考模式支持度
    pub thinking: ThinkingSupport,
    /// 上下文窗口上限（token，实测值）——HCW 窗口折减（P5）的输入
    pub context_window: u32,
    /// 单次输出 token 上限
    pub max_output: u32,
    /// prompt 缓存支持度
    pub prompt_caching: CacheSupport,
    /// 可用服务分层（空 = 仅标准档）
    pub service_tiers: Vec<ServiceTier>,
    /// 会话状态守恒策略（P4）
    pub state_preservation: StatePreservationPolicy,
    /// 支持的输入模态（至少含 Text）
    pub modalities: Vec<ModalityKind>,
    /// 是否支持结构化输出（JSON mode / schema 约束）
    pub structured_output: bool,
}

impl CapabilitySet {
    /// 最小文本能力集 — 仅流式文本，无思考/缓存/工具
    ///
    /// WHY: 作为 `ModelAffinitySpec::minimal` 的保守默认，Custom 通道
    /// 未填能力自查表时以最小集起步，协商只降不升（P3 容错方向）。
    pub fn minimal_text(context_window: u32, max_output: u32) -> Self {
        Self {
            streaming: true,
            tool_calling: false,
            thinking: ThinkingSupport::None,
            context_window,
            max_output,
            prompt_caching: CacheSupport::None,
            service_tiers: Vec::new(),
            state_preservation: StatePreservationPolicy::None,
            modalities: vec![ModalityKind::Text],
            structured_output: false,
        }
    }
}

// ============================================================
// 定价 / 端点 / 怪癖规则（TOML 外置元数据，P8）
// ============================================================

/// 峰谷计价时段 — 按小时桶定义的价格系数
///
/// # 设计决策（WHY）
/// DeepSeek 高峰 2× 定价事实要求成本模型感知时段。用小时桶（0-23）而非
/// 任意时刻区间：路由热路径查表 O(1) 零 chrono 计算（复刻 cacr.rs 美分
/// 整数范式的性能思路）。factor_percent 用整数百分比避免 f32 精度教训
/// （sesa-router f32→f64 隐式膨胀事故）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeakPeriod {
    /// 起始小时（含，0-23，本地时区按厂商计费时区解释）
    pub start_hour: u8,
    /// 结束小时（不含，0-24；跨零点拆两条规则）
    pub end_hour: u8,
    /// 价格系数百分比（100 = 1×，200 = 高峰 2×）
    pub factor_percent: u16,
}

/// 计价货币 — 国内厂商 CNY，Custom 通道可能为 USD
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Currency {
    /// 人民币
    Cny,
    /// 美元
    Usd,
}

/// 定价规格 — 输入/输出/缓存命中/峰谷系数（随厂商调价漂移，一律走配置）
///
/// # 精度约定
/// 价格单位为**微元/百万 token**（µ¥，1e-6 元）：DeepSeek 缓存命中
/// ¥0.01/百万 → `10_000`。整数运算贯穿成本模型，禁止浮点中间态
/// （f32 精度红线）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PricingSpec {
    /// 计价货币
    pub currency: Currency,
    /// 输入价（微元/百万 token）
    pub input_micro_per_mtok: u64,
    /// 输出价（微元/百万 token）
    pub output_micro_per_mtok: u64,
    /// 缓存命中价（微元/百万 token；无缓存时 = input 价）
    pub cache_hit_micro_per_mtok: u64,
    /// 峰谷时段表（空 = 全天 1×）
    pub peak_periods: Vec<PeakPeriod>,
}

impl PricingSpec {
    /// 零价占位 — Custom 自部署通道（本地 vLLM/Ollama 无 API 计费）
    pub fn free() -> Self {
        Self {
            currency: Currency::Cny,
            input_micro_per_mtok: 0,
            output_micro_per_mtok: 0,
            cache_hit_micro_per_mtok: 0,
            peak_periods: Vec::new(),
        }
    }
}

/// 端点规格 — base_url、鉴权、超时、限流参数
///
/// # 安全约定（WHY）
/// API Key **不入 spec**：`api_key_env` 仅存环境变量名，密钥由运行时
/// 从环境读取。TOML 配置文件可安全入库/分享，杜绝密钥泄漏面。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointSpec {
    /// API 基址（如 `https://open.bigmodel.cn/api/paas/v4`）
    pub base_url: Box<str>,
    /// 存放 API Key 的环境变量名（如 `ZHIPU_API_KEY`）
    pub api_key_env: Box<str>,
    /// 请求总超时（毫秒）
    pub timeout_ms: u64,
    /// 连接超时（毫秒）
    pub connect_timeout_ms: u64,
    /// 每分钟请求数限制（None = 厂商未声明，不做客户端限流）
    pub rpm_limit: Option<u32>,
    /// 每分钟 token 数限制（None = 厂商未声明）
    pub tpm_limit: Option<u64>,
}

/// 厂商怪癖规则 — 数据而非代码（原则 P1/P8）
///
/// # 设计决策（WHY 闭集枚举）
/// 怪癖写成 TOML 里的数据条目，适配器按规则执行，不写死厂商 if 分支。
/// 闭集枚举保证 TOML 拼错在反序列化即失败（快速失败），而非运行时静默
/// 忽略；新增怪癖变体需 ADR 记录（对齐 ProviderId 闭集治理约定）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "rule")]
pub enum QuirkRule {
    /// 优先使用指定协议方言（如 Kimi 优先 Anthropic 路径，OpenAI 转换
    /// 路径会丢工具链特性，走之必须降级留痕）
    PreferDialect {
        /// 优先方言
        dialect: ProtocolDialect,
    },
    /// Anthropic 路径下思考档位改用 thinking budget 参数
    /// （GLM：Anthropic 端点不支持 reasoning_effort，TTG Deep → budget 高档）
    AnthropicThinkingBudget,
    /// 已废弃模型名清单 — 注册时拒绝（DeepSeek 旧名 deepseek-chat/reasoner
    /// 已于 2026-07-24 废弃，禁止注册）
    DeprecatedModelNames {
        /// 废弃模型名列表
        names: Vec<Box<str>>,
    },
    /// 质量漂移观察位 — 健康探针重点监控（豆包 Evolving 月更 2-4 次，
    /// 模型行为漂移由质量分触发自动降权）
    QualityDriftWatch,
    /// 高吞吐批量优先 — 批量任务路由权重上调（Step 350 TPS）
    HighThroughputBatchPreferred,
}

// ============================================================
// 模型亲和描述符（每厂商每模型一张，TOML 外置）
// ============================================================

/// 模型亲和描述符 — 渠道亲和的完整元数据（原则 P8 配置外置）
///
/// 每厂商每模型一张，存于 `affinity.d/{vendor}.toml`；厂商调价/改名/
/// 发新模型只改 TOML 不发版，代码零厂商字符串。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelAffinitySpec {
    /// 厂商标识
    pub provider: ProviderId,
    /// 模型名（厂商 API 侧的真实模型标识）
    pub model: Box<str>,
    /// 该模型支持的协议方言（可多协议，首个为默认偏好）
    pub dialects: Vec<ProtocolDialect>,
    /// 能力集（能力协商唯一事实源）
    pub capabilities: CapabilitySet,
    /// 定价规格
    pub pricing: PricingSpec,
    /// 端点规格
    pub endpoint: EndpointSpec,
    /// 厂商怪癖规则（数据化，非代码）
    ///
    /// WHY `#[serde(default)]`: 多数模型无怪癖(如 glm-5.2-fast-preview),
    /// TOML 卡片可省略整个 quirks 段而非强制写 `quirks = []`(P8 配置友好)。
    #[serde(default)]
    pub quirks: Vec<QuirkRule>,
}

impl ModelAffinitySpec {
    /// 最小描述符 — 测试与 Custom 通道起步用（最小文本能力 + 零价 + 空端点）
    ///
    /// WHY: 单测/录播回放不需要真实端点与定价；Custom 通道未填自查表时
    /// 以保守最小集注册，能力只降不升。
    pub fn minimal(provider: ProviderId, model: &str, dialect: ProtocolDialect) -> Self {
        Self {
            provider,
            model: model.into(),
            dialects: vec![dialect],
            capabilities: CapabilitySet::minimal_text(4096, 4096),
            pricing: PricingSpec::free(),
            endpoint: EndpointSpec {
                base_url: "".into(),
                api_key_env: "".into(),
                timeout_ms: 60_000,
                connect_timeout_ms: 10_000,
                rpm_limit: None,
                tpm_limit: None,
            },
            quirks: Vec::new(),
        }
    }

    /// 路由键 — `provider/model` 稳定文本形态
    ///
    /// 用于通道注册表键、SQLite route_history 行、omega-learner 臂编码前缀。
    pub fn route_key(&self) -> String {
        format!("{}/{}", self.provider.as_str(), self.model)
    }

    /// 是否支持指定协议方言
    pub fn supports_dialect(&self, dialect: ProtocolDialect) -> bool {
        self.dialects.contains(&dialect)
    }

    /// 默认偏好方言（dialects 首个；含 PreferDialect 怪癖时以怪癖为准）
    ///
    /// WHY 怪癖优先: Kimi 声明 OpenAI + Anthropic 双方言，但 OpenAI 转换
    /// 路径丢工具链特性，怪癖规则显式钉住 Anthropic 优先。
    pub fn preferred_dialect(&self) -> Option<ProtocolDialect> {
        for quirk in &self.quirks {
            if let QuirkRule::PreferDialect { dialect } = quirk {
                if self.supports_dialect(*dialect) {
                    return Some(*dialect);
                }
            }
        }
        self.dialects.first().copied()
    }
}

// ============================================================
// 统一请求/响应契约（网关与上层之间）
// ============================================================

/// 思考偏好 — TTG 三档的 L0 镜像
///
/// # 镜像关系（WHY 重复定义）
/// `nexus_core::ThinkingMode`（L1）承载 TTG 全局语义，但 L0 零 crate
/// 依赖铁律禁止 L0 → L1 引用。此处镜像定义三档，**两处必须保持同步**
/// （nexus-core/src/types.rs L63 有互指注释）；长期可评估将 ThinkingMode
/// 下沉 L0（对齐 TaskStatus/Checkpoint 下沉先例）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingPreference {
    /// 快速模式：低延迟（映射：GLM reasoning_effort=none / thinking 关 / Step low）
    Fast,
    /// 标准模式：平衡（映射：GLM medium / thinking 开 / MiniMax adaptive）
    Standard,
    /// 深度模式：深推理（映射：GLM xhigh/max / thinking 开 + 长预算 / Step high）
    Deep,
}

impl ThinkingPreference {
    /// 稳定字符串标识 — 用于路由臂编码（`provider/model/mode` 第三段）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Standard => "standard",
            Self::Deep => "deep",
        }
    }
}

/// 消息角色 — 会话历史中单条消息的发言方
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// 系统提示
    System,
    /// 用户输入
    User,
    /// 助手（模型）输出——含需回传守恒的 thinking/tool_use 块
    Assistant,
    /// 工具执行结果
    Tool,
}

/// 统一内容块 — 响应侧统一块模型（对齐 Anthropic 内容块语义）
///
/// # 设计裁决（设计文档 §4.1）
/// 响应统一为 `Vec<ContentBlock>`（Kimi/MiniMax/GLM 的 Anthropic 路径
/// 原生如此），OpenAI 路径由 Codec 转换。**请求侧不做统一**——P2 方言
/// 保真，各 Codec 直接构造原生请求。
///
/// # WHY 工具入参/出参用原始 JSON 字符串
/// L0 零依赖铁律禁止 `serde_json::Value`；`Box<str>` 承载原始 JSON 文本，
/// 解析责任在 L10 Codec（系统边界校验原则）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "block")]
pub enum ContentBlock {
    /// 文本块
    Text {
        /// 文本内容
        text: Box<str>,
    },
    /// 思考块 — 状态守恒（P4）的一等公民，按 StatePreservationPolicy 回传
    Thinking {
        /// 思考内容（VerbatimThinking 策略下逐字保真，禁止 strip）
        thinking: Box<str>,
        /// 厂商签名（Anthropic thinking 块的 signature 字段，回传校验用）
        signature: Option<Box<str>>,
    },
    /// 工具调用块
    ToolUse {
        /// 调用标识（多轮回传关联用）
        id: Box<str>,
        /// 工具名
        name: Box<str>,
        /// 入参（原始 JSON 文本，由 Codec 解析）
        input_json: Box<str>,
    },
    /// 工具结果块
    ToolResult {
        /// 对应 ToolUse 的调用标识
        tool_use_id: Box<str>,
        /// 结果内容（原始文本或 JSON）
        content: Box<str>,
        /// 是否为错误结果
        is_error: bool,
    },
}

/// 工具声明 — 请求侧的工具定义（各 Codec 转换为方言原生形态）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDecl {
    /// 工具名
    pub name: Box<str>,
    /// 工具描述（供模型理解用途）
    pub description: Box<str>,
    /// 入参 JSON Schema（原始 JSON 文本）
    pub parameters_schema: Box<str>,
}

/// 会话消息 — 统一请求中的单条历史消息
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffinityMessage {
    /// 发言角色
    pub role: MessageRole,
    /// 内容块序列（assistant 消息可含 Thinking/ToolUse 块，按守恒策略回传）
    pub blocks: Vec<ContentBlock>,
}

/// 亲和覆盖 — 上层对路由决策的显式钉选（默认全 None = 学习路由）
///
/// WHY 默认不钉选: "单一首选厂商默认配置"与厂商集中度免疫探针（N7）冲突，
/// 默认必须是学习路由 + 手动钉选可选（设计文档 §8.3 否决清单）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AffinityOverrides {
    /// 钉选厂商（None = 路由自选）
    pub pinned_provider: Option<ProviderId>,
    /// 钉选模型（None = 路由自选）
    pub pinned_model: Option<Box<str>>,
    /// 指定服务分层（加价档必须显式指定，禁止默认开启，P6）
    pub service_tier: Option<ServiceTier>,
}

/// 统一请求契约 — 上层（quest-engine 等）→ 网关
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AffinityRequest {
    /// 关联的用户意图标识（全链路追踪）
    pub intent_id: Box<str>,
    /// 会话消息历史（含状态守恒回传块）
    pub messages: Vec<AffinityMessage>,
    /// 可用工具声明（空 = 纯对话）
    pub tools: Vec<ToolDecl>,
    /// TTG 思考偏好
    pub thinking_pref: ThinkingPreference,
    /// 预算提示（微元；None = 不限，由 acb-governor 全局治理兜底）
    pub budget_hint_micro: Option<u64>,
    /// 路由覆盖（默认零钉选）
    pub overrides: AffinityOverrides,
}

/// usage 统计 — 厂商返回的 token 计量（成本回写与缓存命中率的数据源）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageReport {
    /// 输入 token 数
    pub input_tokens: u64,
    /// 输出 token 数
    pub output_tokens: u64,
    /// 缓存命中 token 数（隐式族厂商返回；无缓存 = 0）
    pub cache_hit_tokens: u64,
    /// 思考 token 数（厂商单列时填写；未单列 = None，计入 output）
    pub thinking_tokens: Option<u64>,
}

/// 成本估算 — 路由决策附带的预估成本（原则 P6 成本先行）
///
/// 整数微元运算，禁止浮点中间态（f32 精度红线）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostEstimate {
    /// 预估总成本（微元）
    pub total_micro: u64,
    /// 生效的峰谷系数百分比（100 = 1×）
    pub peak_factor_percent: u16,
    /// 缓存折扣节省（微元，信息性字段）
    pub cache_discount_micro: u64,
}

/// 结束原因 — 归一后的会话终止语义
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// 正常结束
    Stop,
    /// 达到输出上限
    MaxTokens,
    /// 等待工具调用结果
    ToolUse,
    /// 内容过滤拦截
    ContentFilter,
    /// 其他（厂商专有原因，原文留存于事件留痕）
    Other,
}

/// 厂商回执 — 响应溯源信息（路由留痕与审计用）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderReceipt {
    /// 实际服务的厂商
    pub provider: ProviderId,
    /// 实际服务的模型
    pub model: Box<str>,
    /// 实际使用的协议方言
    pub dialect: ProtocolDialect,
    /// 厂商侧请求标识（限流申诉与问题排查用）
    pub request_id: Option<Box<str>>,
}

/// 统一响应契约 — 网关 → 上层
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AffinityResponse {
    /// 内容块序列（统一块模型）
    pub blocks: Vec<ContentBlock>,
    /// token 计量
    pub usage: UsageReport,
    /// 实际成本（基于 usage 回算，微元）
    pub cost: CostEstimate,
    /// 结束原因
    pub finish_reason: FinishReason,
    /// 厂商回执
    pub receipt: ProviderReceipt,
}

/// 协商保真度 — 三态降级协议（设计文档 §7 Round 3 裁决）
///
/// 降级是产品行为，不是技术兜底：降级必须明确告知（E4 不变量），
/// 核心能力缺失的通道直接不进路由池。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NegotiationFidelity {
    /// 特性全启用
    FullFidelity,
    /// 降级 + 事件留痕 + 会话内一次性明确告知
    DegradedNotified,
    /// 核心能力缺失（不支持流式/工具调用），通道不进路由池
    ChannelRejected,
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一张接近真实的描述符（GLM 样例）供序列化测试复用
    fn sample_spec() -> ModelAffinitySpec {
        ModelAffinitySpec {
            provider: ProviderId::Zhipu,
            model: "glm-5.2".into(),
            dialects: vec![
                ProtocolDialect::OpenAiChat,
                ProtocolDialect::AnthropicMessages,
            ],
            capabilities: CapabilitySet {
                streaming: true,
                tool_calling: true,
                thinking: ThinkingSupport::EffortLevels(vec![
                    "none".into(),
                    "medium".into(),
                    "xhigh".into(),
                    "max".into(),
                ]),
                context_window: 1_000_000,
                max_output: 128_000,
                prompt_caching: CacheSupport::ExplicitControl,
                service_tiers: vec![ServiceTier::Standard],
                state_preservation: StatePreservationPolicy::BlockPreservation,
                modalities: vec![ModalityKind::Text],
                structured_output: true,
            },
            pricing: PricingSpec {
                currency: Currency::Cny,
                input_micro_per_mtok: 2_000_000,
                output_micro_per_mtok: 8_000_000,
                cache_hit_micro_per_mtok: 400_000,
                peak_periods: vec![PeakPeriod {
                    start_hour: 8,
                    end_hour: 20,
                    factor_percent: 200,
                }],
            },
            endpoint: EndpointSpec {
                base_url: "https://open.bigmodel.cn/api/paas/v4".into(),
                api_key_env: "ZHIPU_API_KEY".into(),
                timeout_ms: 120_000,
                connect_timeout_ms: 10_000,
                rpm_limit: Some(600),
                tpm_limit: None,
            },
            quirks: vec![QuirkRule::AnthropicThinkingBudget],
        }
    }

    #[test]
    fn provider_id_as_str_stable() {
        // WHY: as_str 是路由键/SQLite/臂编码的稳定形态，变更即破坏历史数据
        assert_eq!(ProviderId::Zhipu.as_str(), "zhipu");
        assert_eq!(ProviderId::DeepSeek.as_str(), "deep_seek");
        assert_eq!(
            ProviderId::Custom("openrouter".into()).as_str(),
            "openrouter"
        );
    }

    #[test]
    fn provider_id_serde_matches_as_str() {
        // WHY: TOML `provider = "zhipu"` 与 as_str 必须同形，防止两套命名漂移
        for p in [
            ProviderId::Zhipu,
            ProviderId::DeepSeek,
            ProviderId::Moonshot,
            ProviderId::VolcanoArk,
            ProviderId::AlibabaCloud,
            ProviderId::MiniMax,
            ProviderId::StepFun,
        ] {
            let json = serde_json::to_string(&p).unwrap();
            assert_eq!(json, format!("\"{}\"", p.as_str()));
        }
    }

    #[test]
    fn spec_json_roundtrip() {
        let spec = sample_spec();
        let json = serde_json::to_string(&spec).unwrap();
        let back: ModelAffinitySpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, back);
    }

    #[test]
    fn spec_messagepack_roundtrip() {
        // ADR-004: 跨层序列化协议为 MessagePack，契约类型必须可往返
        let spec = sample_spec();
        let bytes = rmp_serde::to_vec(&spec).unwrap();
        let back: ModelAffinitySpec = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(spec, back);
    }

    #[test]
    fn route_key_format() {
        assert_eq!(sample_spec().route_key(), "zhipu/glm-5.2");
    }

    #[test]
    fn preferred_dialect_defaults_to_first() {
        let spec = sample_spec();
        assert_eq!(spec.preferred_dialect(), Some(ProtocolDialect::OpenAiChat));
    }

    #[test]
    fn preferred_dialect_honors_quirk() {
        // Kimi 场景：双方言声明但怪癖钉住 Anthropic 优先
        let mut spec = sample_spec();
        spec.quirks.push(QuirkRule::PreferDialect {
            dialect: ProtocolDialect::AnthropicMessages,
        });
        assert_eq!(
            spec.preferred_dialect(),
            Some(ProtocolDialect::AnthropicMessages)
        );
    }

    #[test]
    fn preferred_dialect_ignores_unsupported_quirk() {
        // P3 容错：怪癖指向未声明方言时回落 dialects 首个，不 panic 不报错
        let mut spec = sample_spec();
        spec.quirks.insert(
            0,
            QuirkRule::PreferDialect {
                dialect: ProtocolDialect::OpenAiResponses,
            },
        );
        assert_eq!(spec.preferred_dialect(), Some(ProtocolDialect::OpenAiChat));
    }

    #[test]
    fn quirk_rule_tagged_serde() {
        // WHY: tag = "rule" 使 TOML 写作 rule = "prefer_dialect"，拼错即反序列化失败
        let quirk = QuirkRule::PreferDialect {
            dialect: ProtocolDialect::AnthropicMessages,
        };
        let json = serde_json::to_string(&quirk).unwrap();
        assert!(
            json.contains("\"rule\":\"prefer_dialect\""),
            "json = {json}"
        );
        // 拼错的 rule 名必须失败（闭集枚举快速失败语义）
        let bad = r#"{"rule":"prefer_dailect","dialect":"anthropic_messages"}"#;
        assert!(serde_json::from_str::<QuirkRule>(bad).is_err());
    }

    #[test]
    fn thinking_support_query() {
        assert!(!ThinkingSupport::None.is_supported());
        assert!(ThinkingSupport::OnOff.is_supported());
        assert!(ThinkingSupport::EffortLevels(vec!["low".into()]).is_supported());
    }

    #[test]
    fn minimal_spec_is_conservative() {
        // Custom 通道未填自查表时以最小能力集起步（协商只降不升）
        let spec = ModelAffinitySpec::minimal(
            ProviderId::Custom("local-vllm".into()),
            "qwen3-32b",
            ProtocolDialect::OpenAiChat,
        );
        assert!(spec.capabilities.streaming);
        assert!(!spec.capabilities.tool_calling);
        assert_eq!(spec.capabilities.thinking, ThinkingSupport::None);
        assert_eq!(spec.pricing.input_micro_per_mtok, 0);
        assert_eq!(spec.route_key(), "local-vllm/qwen3-32b");
    }

    #[test]
    fn affinity_request_response_roundtrip() {
        let req = AffinityRequest {
            intent_id: "intent-001".into(),
            messages: vec![AffinityMessage {
                role: MessageRole::User,
                blocks: vec![ContentBlock::Text {
                    text: "写一个快速排序".into(),
                }],
            }],
            tools: vec![ToolDecl {
                name: "read_file".into(),
                description: "读取文件内容".into(),
                parameters_schema: r#"{"type":"object","properties":{"path":{"type":"string"}}}"#
                    .into(),
            }],
            thinking_pref: ThinkingPreference::Standard,
            budget_hint_micro: Some(50_000),
            overrides: AffinityOverrides::default(),
        };
        let bytes = rmp_serde::to_vec(&req).unwrap();
        let back: AffinityRequest = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(req, back);

        let resp = AffinityResponse {
            blocks: vec![
                ContentBlock::Thinking {
                    thinking: "先分析复杂度...".into(),
                    signature: None,
                },
                ContentBlock::ToolUse {
                    id: "call-1".into(),
                    name: "read_file".into(),
                    input_json: r#"{"path":"src/main.rs"}"#.into(),
                },
            ],
            usage: UsageReport {
                input_tokens: 120,
                output_tokens: 80,
                cache_hit_tokens: 100,
                thinking_tokens: Some(40),
            },
            cost: CostEstimate {
                total_micro: 1_240,
                peak_factor_percent: 100,
                cache_discount_micro: 160,
            },
            finish_reason: FinishReason::ToolUse,
            receipt: ProviderReceipt {
                provider: ProviderId::Zhipu,
                model: "glm-5.2".into(),
                dialect: ProtocolDialect::OpenAiChat,
                request_id: Some("req-abc".into()),
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: AffinityResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, back);
    }

    #[test]
    fn thinking_preference_arm_encoding() {
        // WHY: as_str 是 omega-learner 臂编码 `provider/model/mode` 的第三段
        assert_eq!(ThinkingPreference::Fast.as_str(), "fast");
        assert_eq!(ThinkingPreference::Standard.as_str(), "standard");
        assert_eq!(ThinkingPreference::Deep.as_str(), "deep");
    }

    #[test]
    fn overrides_default_is_unpinned() {
        // §8.3 否决清单：默认配置必须是"学习路由 + 手动钉选可选"
        let o = AffinityOverrides::default();
        assert!(o.pinned_provider.is_none());
        assert!(o.pinned_model.is_none());
        assert!(o.service_tier.is_none());
    }
}
