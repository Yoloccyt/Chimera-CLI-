//! 上下文能力档案 — 模型"标称窗口 vs 有效窗口 vs 计费档 vs 服务端线程"的 L0 契约
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts-mca）
//! 对应设计源: `HCW_上下文架构重构蓝图_v3.0_1M时代能力感知分层.md` §5.2 / §六（P0 交付）
//!
//! # 核心职责
//!
//! 1M 时代"能力感知分层"的数据基座：HCW 常驻层定容依据是**有效窗口**而非
//! 标称窗口（蓝图 §1.2：有效容量约为标称 50–65%，Gemini 3.1 Pro 特例 26%，
//! MRCR v2 8-needle 2026-05 基准快照——标称 ≠ 可用）。本模块把该事实变成
//! 能力协商的一等公民数据：
//!
//! | 类型 | 用途 | 消费层 |
//! |------|------|--------|
//! | [`ContextCapabilityProfile`] | 单模型上下文能力档案 | L10 mca-gateway 写入 / L2 hcw-window 只读 |
//! | [`CalibrationTier`] | 有效窗口标定置信度三档 | 探针校准闭环（对齐 ADR-198 off/observe/apply） |
//! | [`ThreadSupport`] | 服务端会话线程支持度 | L10 传输层 threads 模式 gating（蓝图 §二 R6 / §5.5） |
//! | [`PricingTier`] | 上下文长度计费档 | 成本模型 / decb-governor 预算治理（蓝图 §1.3 成本墙） |
//!
//! # 依赖方向（WHY——依赖铁律的教科书实例）
//!
//! mca-gateway（L10）探测并写入 → L0；hcw-window（L2）/ osa-coordinator（L6）
//! 读 L0。**HCW 永不感知 L10**：档案变更经 event-bus `ContextCapabilityUpdated`
//! （Normal 级）广播，L2 订阅刷新 WindowGovernor 定容输入——依赖方向由
//! L0 中转 + 事件解耦双保险（蓝图 §5.2）。
//!
//! # 数据来源与置信度
//!
//! 先验表（[`external_prior_profile`]）数值取自 2026-11 模型窗口检索核验
//! （蓝图 §1.1）与 MRCR v2 8-needle 2026-05 基准快照（蓝图 §1.2）；自家探针
//! 标定（5 针 × {0.25, 0.5, 0.75, 0.9}×标称 四档长度拟合，80% 检索准确率
//! 截点）回写后置信度升级为 [`CalibrationTier::OwnProbe`]（蓝图 §5.3）。
//!
//! # 不变量（文档约定，随 crate 惯例）
//!
//! `effective_window <= advertised_window`：有效窗口是标称窗口经衰减/截断后
//! 的子集，违反即探针数据损坏。**L0 纯类型不做运行时断言**（对齐 `PeakPeriod`
//! 0-23 小时无校验的 crate 惯例，ADR-033 纯类型边界）；校验责任在写入边界
//! （mca-gateway 探针回写 / 先验灌入），本模块以测试机证先验表全量满足。
//!
//! # 示例
//!
//! ```
//! use nexus_contracts_mca::affinity::ProviderId;
//! use nexus_contracts_mca::context_capability::external_prior_profile;
//!
//! // Gemini 3.1 Pro：1M 标称，但 MRCR v2 8-needle 仅 26.3%（2026-05 基准快照）
//! let profile = external_prior_profile(
//!     ProviderId::Custom("google".into()),
//!     "gemini-3.1-pro",
//! )
//! .expect("先验表内已知键必须命中");
//! assert_eq!(profile.advertised_window, 1_000_000);
//! assert!(profile.effective_window <= profile.advertised_window);
//! ```

use serde::{Deserialize, Serialize};

use crate::affinity::ProviderId;

// ============================================================
// 标定置信度
// ============================================================

/// 有效窗口标定置信度三档 — 探针校准闭环的分级标记（蓝图 §5.3）
///
/// 来源分层：公开基准先验（[`CalibrationTier::ExternalPrior`]）→ 自家探针
/// 标定（[`CalibrationTier::OwnProbe`]）→ 无探针条件的保守推断
/// （[`CalibrationTier::Inferred`]，取标称 50% 且 resident_cap 再打 8 折）。
///
/// # WHY 默认 observe
/// 对齐 ADR-198 阈值校准 off/observe/apply 三态：先验/探针数据默认 observe 档
/// （留痕不驱动生产定容），转 apply 需独立治理裁决——RSI 三件套守门
/// （蓝图 §5.3 / §七 P0 验收线）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationTier {
    /// 公开基准先验（MRCR v2 / NIAH 第三方数据，作初始值）
    ExternalPrior,
    /// 自家探针标定（5 针 × 四档长度拟合，80% 检索准确率截点）
    OwnProbe,
    /// 无探针条件时的保守推断（标称 50%，resident_cap 再打 8 折）
    Inferred,
}

impl CalibrationTier {
    /// 稳定字符串标识 — 与 serde 命名同形
    ///
    /// WHY 必须与 serde 同形: event-bus `ContextCapabilityUpdated.confidence`
    /// 以字符串留痕（字段文档钉住 "external_prior"/"own_probe"/"inferred"），
    /// 网关发事件取本形态，漂移即事件消费面解析断裂。对齐
    /// `ProviderId::as_str` / `ThinkingPreference::as_str` 先例。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ExternalPrior => "external_prior",
            Self::OwnProbe => "own_probe",
            Self::Inferred => "inferred",
        }
    }
}

// ============================================================
// 服务端会话线程支持度
// ============================================================

/// 服务端会话线程支持度 — 厂商传输层增量会话能力（蓝图 §二 R6 / §5.5）
///
/// # WHY 数据而非代码（P1 禁止名字嗅探）
/// Anthropic Message threads（2026-11 公测）是"传输层的 HCW 哲学收敛"：
/// 首轮全量 create、后续增量 continue。网关按本字段 gating（`server_thread`
/// ≠ None 时启用 threads 模式），禁止 `model_name.contains("claude")` 判断。
///
/// # 红线（蓝图 §二裁决 3）
/// 不依赖线程做存储——服务端线程 best-effort（5 分钟空闲过期、最迟 1 小时
/// 删除），404 回退路径 = session-store 按 Thread 段完整回放重建（ADR-141）；
/// threads 只省 TTFT 不省 token（计费不变），常驻层预算治理不因 threads 放松。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadSupport {
    /// 不支持服务端线程（全量重传路径，零行为变化）
    None,
    /// 公测档 — 载荷为服务端线程空闲过期分钟数
    ///
    /// （Anthropic Message threads：最后请求后 5 分钟过期、最迟 1 小时删除；
    /// beta 头 `message-threads-2026-08-12`）
    Beta {
        /// 服务端线程空闲过期时长（分钟）
        expiry_minutes: u32,
    },
    /// 正式档（GA，无过期语义差异）
    Stable,
}

impl ThreadSupport {
    /// 是否支持任何形式的服务端线程（网关 threads 模式的 gating 输入）
    ///
    /// 对齐 `ThinkingSupport::is_supported` 三态查询先例（蓝图 §5.5：
    /// `server_thread ≠ None` 时首轮 create、后续 continue）。
    pub fn is_supported(&self) -> bool {
        !matches!(self, Self::None)
    }
}

// ============================================================
// 计费档
// ============================================================

/// 计费档 — 按单次请求输入 token 总量门槛的价格倍率（蓝图 §1.3 成本墙）
///
/// # WHY 不复用 `affinity::PricingSpec`
/// `PricingSpec` 是"每 token 单价 + 峰谷时段"的全局单价面；本类型是
/// "上下文长度门槛 → 倍率"的**条件计费面**（Gemini >200K 2×、GPT-6 >272K 2×，
/// 蓝图 §1.1）。语义不同构：单价是常量，门槛倍率是请求规模的函数；混用会让
/// 成本模型无法区分基础单价与长上下文加价。二者并存，
/// 成本模型 = `PricingSpec` 单价 × 命中档 `factor_percent`。
///
/// # 字段形态（对齐 `PeakPeriod` 整数百分比惯例）
/// `factor_percent` 沿用 `PeakPeriod.factor_percent`（100 = 1×，200 = 2×），
/// 整数运算贯穿成本模型，禁止浮点倍率（f32 精度红线，sesa-router 事故教训）；
/// 蓝图草案字段名 `price_multiplier` 依此裁决为百分比整数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PricingTier {
    /// 输入 token 总量门槛（达到即按 `factor_percent` 计费）
    ///
    /// 各厂商对"超出部分加价 vs 全单加价"的细则差异由消费面（成本模型）解释，
    /// L0 只承载数据（系统边界校验原则）。
    pub threshold_tokens: u32,
    /// 价格系数百分比（100 = 1×，200 = 2×）
    pub factor_percent: u16,
}

// ============================================================
// 能力档案（核心载体）
// ============================================================

/// 模型上下文能力档案 — 1M 时代能力感知分层的核心数据载体（蓝图 §5.2 核心交付）
///
/// 每厂商每模型一张。HCW 常驻层定容按 `effective_window` 而非
/// `advertised_window`（四面墙否决"标称 1M = 常驻 1M"的全量装载模式：
/// 有效上下文墙 / 成本墙 / 延迟墙 / 智能稀释墙，蓝图 §三）。
///
/// # 不变量（文档约定）
/// `effective_window <= advertised_window`（见模块文档"不变量"节）。
///
/// # WHY `Box<str>` 而非蓝图草案的 `String`
/// 契约字段构造后不可变，`Box<str>` 省一个容量字段（16 vs 24 字节），
/// 对齐本 crate `EndpointSpec` / `ModelAffinitySpec` 既有惯例
/// （蓝图草案的 `String` 属类型草案粗样，随 crate 惯例裁决）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ContextCapabilityProfile {
    /// 厂商标识（复用 affinity 闭集枚举，P1 能力协商取代名字嗅探）
    ///
    /// WHY 海外旗舰经 [`ProviderId::Custom`] 承载：闭集当前仅覆盖国内七厂商
    /// （ADR-065 范围），新增一等厂商变体须 ADR 记录（枚举治理约定）；
    /// 先验表对 Claude/GPT/Gemini/Llama 以 `Custom("anthropic"/"openai"/
    /// "google"/"meta")` 键入。未来 ADR 新增一等变体（as_str 同名）时
    /// 先验表查表键自动兼容，无需改表。
    pub provider: ProviderId,
    /// 模型名（厂商 API 侧真实标识，与 `ModelAffinitySpec.model` 同形）
    pub model: Box<str>,
    /// 标称窗口（官方公布，token）
    pub advertised_window: u32,
    /// 有效窗口（探针标定或先验，token）——**常驻层定容的真实依据**
    ///
    /// 契约约定 `<= advertised_window`；Inferred 档默认取标称 50%
    /// （蓝图 §5.3 ③）。
    pub effective_window: u32,
    /// 有效窗口的标定置信度三档
    pub effective_confidence: CalibrationTier,
    /// 上下文长度计费档（按 `threshold_tokens` 升序书写；空 = 无分段加价）
    pub pricing_tiers: Vec<PricingTier>,
    /// 服务端会话线程支持度（threads 传输模式的 gating 输入，蓝图 §5.5）
    pub server_thread: ThreadSupport,
    /// 单次输出 token 上限；0 = 先验未声明（消费面按保守缺省处理，
    /// 禁止直接参与 min() 预算计算——0 会被误算为零预算）
    pub max_output: u32,
    /// 自由文本备注（厂商口径 / 基准出处；**不入决策**——决策只读结构化字段，
    /// 蓝图 §5.2"自由文本不入决策"）
    pub long_context_notes: Option<Box<str>>,
}

impl ContextCapabilityProfile {
    /// 全字段构造 — L0 契约的写入面 API
    ///
    /// WHY 必须存在：`#[non_exhaustive]` 禁止外部 crate 字面量构造（前向兼容，
    /// 对齐 `EndpointSpec` / `ModelPrice` 先例），mca-gateway 探针回写
    /// （OwnProbe 档）经此构造。L0 不做校验（见模块文档"不变量"节，
    /// 校验责任在写入边界）。
    // 数据载体全字段显式构造；参数多为 u32/枚举值，命名实参调用方书写，
    // 超出 clippy 默认 7 参数阈值属 L0 数据载体的合理形态（EndpointSpec 同理）
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        provider: ProviderId,
        model: Box<str>,
        advertised_window: u32,
        effective_window: u32,
        effective_confidence: CalibrationTier,
        pricing_tiers: Vec<PricingTier>,
        server_thread: ThreadSupport,
        max_output: u32,
        long_context_notes: Option<Box<str>>,
    ) -> Self {
        Self {
            provider,
            model,
            advertised_window,
            effective_window,
            effective_confidence,
            pricing_tiers,
            server_thread,
            max_output,
            long_context_notes,
        }
    }
}

// ============================================================
// 公开先验灌入表（P0 交付：蓝图 §七 P0"公开先验灌入"）
// ============================================================

/// 公开先验灌入表 — 2026-11 主流旗舰模型的能力档案先验
///
/// 按蓝图 §1.1（2026-11 检索核验）与 §1.2（MRCR v2 8-needle 2026-05 基准
/// 快照）灌入 7 个主流模型的标称/有效窗口。**有效值为先验而非实测**：
/// 置信度一律 [`CalibrationTier::ExternalPrior`]；Llama 4 Scout 例外——
/// 10M 无公开基准验证，按蓝图 §5.3 ③保守取标称 50% 并标
/// [`CalibrationTier::Inferred`]。自家探针标定后经
/// [`ContextCapabilityProfile::new`] 以 OwnProbe 档覆盖。
///
/// # WHY 查表键用 `ProviderId::as_str()`
/// 与事件 `ContextCapabilityUpdated.route_key`（`provider/model` 形态）及
/// `ModelAffinitySpec::route_key` 同源；海外厂商经 Custom 变体键入
/// （见 `ContextCapabilityProfile::provider` 字段文档）。
#[must_use]
pub fn external_prior_profile(
    provider: ProviderId,
    model: &str,
) -> Option<ContextCapabilityProfile> {
    // 数据行按蓝图 §1.1 表序排列；effective 值来源逐行注明（MRCR v2 8-needle
    // 2026-05 基准快照 + 2026-11 检索），先验表全量满足 effective <= advertised
    // （由 external_prior_invariant_and_confidence 测试机证）
    match (provider.as_str(), model) {
        // Anthropic Claude Opus 5.5 — 1M 标准价无加价；threads 公测中
        ("anthropic", "claude-opus-5.5") => Some(ContextCapabilityProfile {
            provider: provider.clone(),
            model: "claude-opus-5.5".into(),
            advertised_window: 1_000_000,
            // 有效约 65 万：标称 65%（§1.2 行业共识区间 50–65% 上沿）
            effective_window: 650_000,
            effective_confidence: CalibrationTier::ExternalPrior,
            pricing_tiers: Vec::new(),
            server_thread: ThreadSupport::Beta { expiry_minutes: 5 },
            max_output: 128_000,
            long_context_notes: Some(
                "MRCR v2 8-needle@1M 同族 Opus 4.6 76%（2026-05 基准快照，全场最高）；\
                 1M 标准价无加价，缓存读 -75%"
                    .into(),
            ),
        }),
        // Anthropic Claude Sonnet 5.5 — 与 Opus 5.5 同族同窗
        ("anthropic", "claude-sonnet-5.5") => Some(ContextCapabilityProfile {
            provider: provider.clone(),
            model: "claude-sonnet-5.5".into(),
            advertised_window: 1_000_000,
            effective_window: 650_000,
            effective_confidence: CalibrationTier::ExternalPrior,
            pricing_tiers: Vec::new(),
            server_thread: ThreadSupport::Beta { expiry_minutes: 5 },
            max_output: 128_000,
            long_context_notes: Some(
                "Claude 5.5 同族（Opus/Sonnet 同窗）；有效窗口按同族 MRCR v2 \
                 8-needle 2026-05 基准快照外推"
                    .into(),
            ),
        }),
        // OpenAI GPT-6 Astra — 1.05M 标称，>272K 输入 2× 计费
        ("openai", "gpt-6-astra") => Some(ContextCapabilityProfile {
            provider: provider.clone(),
            model: "gpt-6-astra".into(),
            advertised_window: 1_050_000,
            // 有效约 68 万：标称 65%（§1.2 区间上沿，同族 GPT-5.5 MRCR 74%）
            effective_window: 680_000,
            effective_confidence: CalibrationTier::ExternalPrior,
            pricing_tiers: vec![PricingTier {
                threshold_tokens: 272_000,
                factor_percent: 200,
            }],
            server_thread: ThreadSupport::None,
            max_output: 128_000,
            long_context_notes: Some(
                "MRCR v2 8-needle@1M GPT-5.5 74%（2026-05 基准快照）；\
                 >272K 输入 2× 计费"
                    .into(),
            ),
        }),
        // Google Gemini 3.1 Pro — 1M 标称但 MRCR 仅 26.3%（最差特例）
        ("google", "gemini-3.1-pro") => Some(ContextCapabilityProfile {
            provider: provider.clone(),
            model: "gemini-3.1-pro".into(),
            advertised_window: 1_000_000,
            // 有效约 26 万：MRCR v2 8-needle@1M 26.3%（2026-05 基准快照）
            // ——1M 世代有效窗口最差特例，蓝图 §1.2 点名"惨案"
            effective_window: 260_000,
            effective_confidence: CalibrationTier::ExternalPrior,
            pricing_tiers: vec![PricingTier {
                threshold_tokens: 200_000,
                factor_percent: 200,
            }],
            server_thread: ThreadSupport::None,
            max_output: 64_000,
            long_context_notes: Some(
                "MRCR v2 8-needle@1M 26.3%（2026-05 基准快照）——1M 世代有效窗口\
                 最差特例；>200K 档位 2× 计费"
                    .into(),
            ),
        }),
        // DeepSeek V4 Pro — 1M 窗口 + 384K 输出上限（长程 Quest 独特价值）
        ("deep_seek", "deepseek-v4-pro") => Some(ContextCapabilityProfile {
            provider: provider.clone(),
            model: "deepseek-v4-pro".into(),
            advertised_window: 1_000_000,
            // 有效约 50 万：标称 50%（§1.2 行业共识区间下沿；多针检索崩塌
            // NIAH 8-needle 41%，2026-05 基准快照）
            effective_window: 500_000,
            effective_confidence: CalibrationTier::ExternalPrior,
            pricing_tiers: Vec::new(),
            server_thread: ThreadSupport::None,
            max_output: 384_000,
            long_context_notes: Some(
                "NIAH 8-needle 41%、多针检索崩塌（2026-05 基准快照）；384K 输出\
                 上限为长程 Quest 独特价值（蓝图附录 B-2 探针首测候选）"
                    .into(),
            ),
        }),
        // 阿里 Qwen3.5-Plus — 1M 窗口，开源权重
        ("alibaba_cloud", "qwen3.5-plus") => Some(ContextCapabilityProfile {
            provider: provider.clone(),
            model: "qwen3.5-plus".into(),
            advertised_window: 1_000_000,
            // 有效约 50 万：标称 50%（§1.2 国内 flash 1M 档真实可用量
            // 200–500K 区间上沿 + 共识区间下沿的交点）
            effective_window: 500_000,
            effective_confidence: CalibrationTier::ExternalPrior,
            pricing_tiers: Vec::new(),
            server_thread: ThreadSupport::None,
            max_output: 131_000,
            long_context_notes: Some(
                "开源权重；有效窗口取行业共识区间（标称 50–65%）下沿 50%\
                 （2026-11 检索，蓝图 §1.2）"
                    .into(),
            ),
        }),
        // Meta Llama 4 Scout — 10M 标称但无公开基准验证（窗口军备竞赛不追，
        // 蓝图 §八减法；按通用机制自然兼容）
        ("meta", "llama-4-scout") => Some(ContextCapabilityProfile {
            provider: provider.clone(),
            model: "llama-4-scout".into(),
            advertised_window: 10_000_000,
            // 保守 500 万：标称 50% 保守推断（10M 无公开基准验证，
            // 蓝图 §1.1 + §5.3 ③ Inferred 档，resident_cap 再打 8 折）
            effective_window: 5_000_000,
            effective_confidence: CalibrationTier::Inferred,
            pricing_tiers: Vec::new(),
            server_thread: ThreadSupport::None,
            // 输出上限未声明（蓝图 §1.1 表"—"）——0 占位，消费面按保守缺省处理
            max_output: 0,
            long_context_notes: Some(
                "10M 无公开基准验证（蓝图 §1.1）——保守取标称 50% 并标 Inferred，\
                 resident_cap 按 §5.3 Inferred 档再打 8 折；输出上限未声明（0 占位）"
                    .into(),
            ),
        }),
        // 未灌入先验的模型 → None（消费面回落 OwnProbe 探针或 Inferred 推断）
        _ => None,
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;

    /// 构造一张接近真实的能力档案（GLM 样例，仿 affinity sample_spec）
    ///
    /// 数值为 fixture 而非厂商声明（sample_spec 定价同理），供序列化测试复用；
    /// 刻意含 PricingTier 载荷以覆盖 Vec 字段的 serde 往返。
    fn sample_profile() -> ContextCapabilityProfile {
        ContextCapabilityProfile {
            provider: ProviderId::Zhipu,
            model: "glm-5.2".into(),
            advertised_window: 1_000_000,
            effective_window: 650_000,
            effective_confidence: CalibrationTier::OwnProbe,
            pricing_tiers: vec![PricingTier {
                threshold_tokens: 200_000,
                factor_percent: 200,
            }],
            server_thread: ThreadSupport::None,
            max_output: 128_000,
            long_context_notes: Some(
                "自有探针 5 针 × {0.25, 0.5, 0.75, 0.9}×标称 四档长度拟合，\
                 80% 检索准确率截点"
                    .into(),
            ),
        }
    }

    #[test]
    fn profile_json_roundtrip() {
        let profile = sample_profile();
        let json = serde_json::to_string(&profile).unwrap();
        let back: ContextCapabilityProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(profile, back);
    }

    #[test]
    fn profile_messagepack_roundtrip() {
        // ADR-004: 跨层序列化协议为 MessagePack，契约类型必须可往返
        let profile = sample_profile();
        let bytes = rmp_serde::to_vec(&profile).unwrap();
        let back: ContextCapabilityProfile = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(profile, back);
    }

    #[test]
    fn calibration_tier_serde_locks_event_convention() {
        // WHY: event-bus ContextCapabilityUpdated.confidence 以字符串留痕
        // （types.rs 字段文档钉住 "external_prior"/"own_probe"/"inferred"），
        // L0 serde 命名与该约定必须同形，漂移即事件消费面解析断裂
        assert_eq!(
            serde_json::to_string(&CalibrationTier::ExternalPrior).unwrap(),
            "\"external_prior\""
        );
        assert_eq!(
            serde_json::to_string(&CalibrationTier::OwnProbe).unwrap(),
            "\"own_probe\""
        );
        assert_eq!(
            serde_json::to_string(&CalibrationTier::Inferred).unwrap(),
            "\"inferred\""
        );
    }

    #[test]
    fn calibration_tier_as_str_matches_serde() {
        // as_str 与 serde 同形（对齐 affinity provider_id_serde_matches_as_str 先例）
        for t in [
            CalibrationTier::ExternalPrior,
            CalibrationTier::OwnProbe,
            CalibrationTier::Inferred,
        ] {
            let json = serde_json::to_string(&t).unwrap();
            assert_eq!(json, format!("\"{}\"", t.as_str()));
        }
    }

    #[test]
    fn thread_support_serde_variants() {
        // 单元变体序列化为裸字符串，载荷变体为外部标记结构（对齐 ThinkingSupport）
        assert_eq!(
            serde_json::to_string(&ThreadSupport::None).unwrap(),
            "\"none\""
        );
        assert_eq!(
            serde_json::to_string(&ThreadSupport::Stable).unwrap(),
            "\"stable\""
        );
        assert_eq!(
            serde_json::to_string(&ThreadSupport::Beta { expiry_minutes: 5 }).unwrap(),
            r#"{"beta":{"expiry_minutes":5}}"#
        );
        // 三变体全量往返
        for t in [
            ThreadSupport::None,
            ThreadSupport::Beta { expiry_minutes: 5 },
            ThreadSupport::Stable,
        ] {
            let json = serde_json::to_string(&t).unwrap();
            assert_eq!(serde_json::from_str::<ThreadSupport>(&json).unwrap(), t);
        }
    }

    #[test]
    fn thread_support_query() {
        // gating 语义：None 之外的任何档位均视为支持（蓝图 §5.5 server_thread ≠ None）
        assert!(!ThreadSupport::None.is_supported());
        assert!(ThreadSupport::Beta { expiry_minutes: 5 }.is_supported());
        assert!(ThreadSupport::Stable.is_supported());
    }

    #[test]
    fn external_prior_invariant_and_confidence() {
        // 先验表全量机证：effective <= advertised（守护未来表编辑不回归不变量）
        // + 置信度档位正确（Scout 为 Inferred 特例，其余 ExternalPrior）
        let cases = [
            (
                ProviderId::Custom("anthropic".into()),
                "claude-opus-5.5",
                CalibrationTier::ExternalPrior,
            ),
            (
                ProviderId::Custom("anthropic".into()),
                "claude-sonnet-5.5",
                CalibrationTier::ExternalPrior,
            ),
            (
                ProviderId::Custom("openai".into()),
                "gpt-6-astra",
                CalibrationTier::ExternalPrior,
            ),
            (
                ProviderId::Custom("google".into()),
                "gemini-3.1-pro",
                CalibrationTier::ExternalPrior,
            ),
            (
                ProviderId::DeepSeek,
                "deepseek-v4-pro",
                CalibrationTier::ExternalPrior,
            ),
            (
                ProviderId::AlibabaCloud,
                "qwen3.5-plus",
                CalibrationTier::ExternalPrior,
            ),
            (
                ProviderId::Custom("meta".into()),
                "llama-4-scout",
                CalibrationTier::Inferred,
            ),
        ];
        for (provider, model, expected_confidence) in cases {
            let p = external_prior_profile(provider, model).expect("先验表已知键必须命中");
            assert!(
                p.effective_window <= p.advertised_window,
                "{}: effective({}) > advertised({})",
                p.model,
                p.effective_window,
                p.advertised_window
            );
            assert_eq!(p.effective_confidence, expected_confidence);
        }
    }

    #[test]
    fn external_prior_key_values() {
        // 关键数值锚点（蓝图 §1.1/§1.2 + MRCR v2 2026-05 基准快照）
        let gpt =
            external_prior_profile(ProviderId::Custom("openai".into()), "gpt-6-astra").unwrap();
        assert_eq!(gpt.advertised_window, 1_050_000);
        assert_eq!(gpt.effective_window, 680_000);
        assert_eq!(gpt.max_output, 128_000);
        assert_eq!(
            gpt.pricing_tiers,
            vec![PricingTier {
                threshold_tokens: 272_000,
                factor_percent: 200,
            }]
        );

        // Gemini 特例：有效窗口 26%（MRCR 26.3% 惨案）+ >200K 2× 计费档
        let gemini =
            external_prior_profile(ProviderId::Custom("google".into()), "gemini-3.1-pro").unwrap();
        assert_eq!(gemini.advertised_window, 1_000_000);
        assert_eq!(gemini.effective_window, 260_000);
        assert_eq!(
            gemini.pricing_tiers,
            vec![PricingTier {
                threshold_tokens: 200_000,
                factor_percent: 200,
            }]
        );

        // Scout：10M 标称 / 保守 500 万 Inferred / 输出上限未声明（0 占位）
        let scout =
            external_prior_profile(ProviderId::Custom("meta".into()), "llama-4-scout").unwrap();
        assert_eq!(scout.advertised_window, 10_000_000);
        assert_eq!(scout.effective_window, 5_000_000);
        assert_eq!(scout.effective_confidence, CalibrationTier::Inferred);
        assert_eq!(scout.max_output, 0);

        // Anthropic threads 公测档（空闲 5 分钟过期）
        let opus =
            external_prior_profile(ProviderId::Custom("anthropic".into()), "claude-opus-5.5")
                .unwrap();
        assert_eq!(
            opus.server_thread,
            ThreadSupport::Beta { expiry_minutes: 5 }
        );
        assert_eq!(opus.max_output, 128_000);

        // 原生闭集厂商键：DeepSeek / AlibabaCloud 走 as_str 键命中
        let deepseek = external_prior_profile(ProviderId::DeepSeek, "deepseek-v4-pro").unwrap();
        assert_eq!(deepseek.effective_window, 500_000);
        assert_eq!(deepseek.max_output, 384_000);
        let qwen = external_prior_profile(ProviderId::AlibabaCloud, "qwen3.5-plus").unwrap();
        assert_eq!(qwen.effective_window, 500_000);
    }

    #[test]
    fn external_prior_unknown_returns_none() {
        // 未灌入先验的模型 → None（消费面回落 OwnProbe 探针或 Inferred 推断）
        assert!(external_prior_profile(ProviderId::Zhipu, "glm-5.2").is_none());
        assert!(
            external_prior_profile(ProviderId::Custom("anthropic".into()), "claude-haiku-9")
                .is_none()
        );
        // Custom 键不匹配 → None：先验不可跨厂商外推（openrouter 转发 Claude
        // 的有效窗口 ≠ Anthropic 原生有效窗口）
        assert!(
            external_prior_profile(ProviderId::Custom("openrouter".into()), "claude-opus-5.5")
                .is_none()
        );
    }

    #[test]
    fn external_prior_json_roundtrip() {
        // 先验表数据必须可 serde 往返（覆盖 Beta 载荷变体、10M 大数、计费档 Vec）
        for (provider, model) in [
            (ProviderId::Custom("anthropic".into()), "claude-opus-5.5"),
            (ProviderId::Custom("anthropic".into()), "claude-sonnet-5.5"),
            (ProviderId::Custom("openai".into()), "gpt-6-astra"),
            (ProviderId::Custom("google".into()), "gemini-3.1-pro"),
            (ProviderId::DeepSeek, "deepseek-v4-pro"),
            (ProviderId::AlibabaCloud, "qwen3.5-plus"),
            (ProviderId::Custom("meta".into()), "llama-4-scout"),
        ] {
            let p = external_prior_profile(provider, model).unwrap();
            let json = serde_json::to_string(&p).unwrap();
            let back: ContextCapabilityProfile = serde_json::from_str(&json).unwrap();
            assert_eq!(p, back, "json roundtrip failed for {}", p.model);
        }
    }

    #[test]
    fn new_constructor_full_fields() {
        // 写入面 API：mca-gateway 探针回写（OwnProbe 档）经 new() 构造
        let p = ContextCapabilityProfile::new(
            ProviderId::Moonshot,
            "kimi-k3".into(),
            256_000,
            160_000,
            CalibrationTier::OwnProbe,
            vec![PricingTier {
                threshold_tokens: 128_000,
                factor_percent: 150,
            }],
            ThreadSupport::Stable,
            64_000,
            Some("探针标定: 80% 检索准确率截点 160K".into()),
        );
        assert_eq!(p.provider, ProviderId::Moonshot);
        assert_eq!(p.model.as_ref(), "kimi-k3");
        assert!(p.effective_window <= p.advertised_window);
        assert_eq!(p.effective_confidence, CalibrationTier::OwnProbe);
        assert!(p.server_thread.is_supported());
        assert_eq!(
            p.long_context_notes.as_deref(),
            Some("探针标定: 80% 检索准确率截点 160K")
        );
    }
}
