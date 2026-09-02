//! 归档压缩器与降级判定纯函数 — Task 17 §17.2 压缩策略 + §17.5 膨胀防护
//!
//! 架构层归属: L9 Quest(chimera-mas/archive 子模块)
//! 核心职责:
//! - 提供 `ArchiveCompressor::compress(strategy, content)` 三级压缩入口
//! - 提供 `compute_priority(access_count, delta_t, tau) -> f64` 衰减优先级纯函数
//! - 提供 `should_demote_metadata(access_count, delta_t, tau) -> bool` 降级判定纯函数
//!
//! ## 设计决策(WHY)
//!
//! - **复用 hcw-window Ω-Compress**(§17.1):
//!   HCW 的 `ContextCompressor` 面向 `ContextEntry` 数组(按重要性评分 Top-N 保留),
//!   不直接适配本模块"文本内容 → 摘要"的归档压缩场景。
//!   本模块提供轻量级本地实现(纯函数,不引入新依赖),按 §17.2 三种策略:
//!   - 1mo: HCW 摘要(≤500 tok,权重 0.4/0.3/0.3)— 复用 HCW 重要性评分公式
//!   - 3mo: 关系抽取(模拟,生成 512-dim CLV 占位向量)
//!   - 6mo: 深度压缩 + 模式抽取(关键决策不压缩,KeepForever)
//!   注释中明确标注"复用 crate API 不匹配,本地实现"
//! - **降级判定纯函数**(§17.2 / §17.5):
//!   `should_demote_metadata` 仅用 `access_count + delta_t + tau` 元数据,
//!   不加载 content,防内存峰值。复用 cmt-tiering 的衰减公式:
//!   `priority = access_count × exp(-Δt / τ)`,阈值 0.1
//! - **全程 f64**(§4.4 反模式 6):
//!   cmt-tiering 的 `DecayCalculator` 用 f32(历史遗留),本模块用 f64 避免
//!   f32 转 f64 精度膨胀(如 0.1f32 as f64 > 0.1)
//! - **6mo 级 KeepForever**(§17.5):
//!   6mo 级使用 `tau = f64::INFINITY`(无衰减),`should_demote_metadata`
//!   永远返回 false,确保关键决策不压缩、不降级
//!
//! ## 红线对齐
//!
//! - §4.1: 库层 thiserror,无 unwrap/expect
//! - §4.4 反模式 6: 全程 f64,禁止 f32 隐式转 f64
//! - §6.1: 单函数 ≤ 200 行
//! - §17.5: 6mo 级 KeepForever,关键决策不压缩

use crate::archive::tier::CompressionStrategy;
use crate::error::{MasError, Result};

// ============================================================
// 常量(SubTask 17.9 REFACTOR — 抽取降级阈值)
// ============================================================

/// 降级阈值(§17.2)— priority < 0.1 时触发降级迁移
///
/// 语义:`compute_priority` 返回值 < 此阈值时,`should_demote_metadata` 返回 true。
///
/// WHY 用 f64 而非 f32(§4.4 反模式 6):
/// cmt-tiering 的 `DEMOTION_THRESHOLD` 用 f32(历史遗留),
/// f32 转 f64 精度膨胀(0.1f32 as f64 > 0.1)导致误判。
/// 本模块全程 f64,确保降级判定精确。
pub const DEMOTION_THRESHOLD_F64: f64 = 0.1;

/// HCW 权重求和容差 — 吸收 `[0.4, 0.3, 0.3]` 在 f64 下的加法误差
///
/// 0.4 + 0.3 + 0.3 的 f64 结果约为 0.9999999999999999,偏差 ~1.1e-16;
/// 1e-6 既容得下浮点噪声,又足以拒掉真实的配置错误(如少给一档权重)。
const HCW_WEIGHT_SUM_TOLERANCE: f64 = 1e-6;

// ============================================================
// 降级判定纯函数(§17.2 + §17.5)
// ============================================================

/// 计算衰减优先级(纯函数,全程 f64,§4.4 反模式 6)
///
/// 公式(§17.2):`priority = access_count × exp(-Δt / τ)`
///
/// ## 参数
///
/// - `access_count`:访问次数(0 表示从未访问)
/// - `delta_t_seconds`:距上次访问的秒数(Δt,非负)
/// - `tau_seconds`:衰减时间常数 τ(秒,正数;`f64::INFINITY` 表示无衰减)
///
/// ## 返回
///
/// 衰减后的优先级 `[0.0, +∞)`:
/// - `access_count = 0`:返回 `0.0`(从未访问的条目优先级最低)
/// - `delta_t = 0`:返回 `access_count as f64`(刚访问的条目优先级 = 访问次数)
/// - `tau = ∞`:返回 `access_count as f64`(无衰减,KeepForever)
/// - `tau = 0`:返回 `access_count as f64`(退化,按无衰减处理)
/// - 一般情况:`access_count × exp(-Δt / τ)`,Δt 越大优先级越低
///
/// ## 边界场景
///
/// - `access_count = 0, delta_t = 0`:返回 `0.0`(从未访问优先级最低)
/// - `access_count = 1, delta_t = 86400, tau = 86400`:返回 `exp(-1) ≈ 0.3679`
/// - `access_count = 1, delta_t = 259200, tau = 86400`:返回 `exp(-3) ≈ 0.0498 < 0.1`
/// - `access_count = 1, delta_t = 31536000, tau = ∞`:返回 `1.0`(KeepForever)
///
/// ## 红线对齐
///
/// - §4.1: 纯函数,无 unwrap/expect
/// - §4.4 反模式 6: 全程 f64,禁止 f32 隐式转 f64
/// - §6.1: 单函数 ≤ 200 行(本函数 < 30 行)
pub fn compute_priority(access_count: u64, delta_t_seconds: f64, tau_seconds: f64) -> f64 {
    // 从未访问的条目优先级为 0
    if access_count == 0 {
        return 0.0;
    }

    // tau ≤ 0 或 tau = ∞:无衰减,priority = access_count
    // WHY tau = ∞ 时 exp(-Δt / ∞) = exp(0) = 1.0,priority = access_count
    // WHY tau ≤ 0 时数学上无意义(除零),按无衰减处理(保守策略)
    if !tau_seconds.is_finite() || tau_seconds <= 0.0 {
        return access_count as f64;
    }

    // delta_t < 0(时钟漂移):按 0 处理(刚访问)
    let delta_t = delta_t_seconds.max(0.0);

    // priority = access_count × exp(-Δt / τ)
    let decay_factor = (-delta_t / tau_seconds).exp();
    access_count as f64 * decay_factor
}

/// 判断是否应触发降级迁移(纯函数,仅用元数据,§17.2 / §17.5)
///
/// 复用 cmt-tiering `DecayCalculator::should_demote_metadata` 的语义,
/// 但用 f64 全程计算(§4.4 反模式 6),避免 f32 精度膨胀。
///
/// ## 参数
///
/// - `access_count`:访问次数(仅元数据,不加载 content,防内存峰值)
/// - `delta_t_seconds`:距上次访问的秒数(Δt,仅元数据)
/// - `tau_seconds`:衰减时间常数 τ(秒)
///
/// ## 返回
///
/// - `true`:`priority < DEMOTION_THRESHOLD_F64 (0.1)`,应降级
/// - `false`:`priority ≥ 0.1`,不应降级
///
/// ## 边界场景
///
/// - `access_count = 0`:返回 `true`(从未访问的条目应降级)
/// - `access_count = 10, delta_t = 0`:返回 `false`(刚访问,priority=10 > 0.1)
/// - `access_count = 1, delta_t = 259200, tau = 86400`:返回 `true`(priority ≈ 0.0498 < 0.1)
/// - `access_count = 1, delta_t = 任意, tau = ∞`:返回 `false`(KeepForever,无衰减)
///
/// ## 红线对齐
///
/// - §4.1: 纯函数,无 unwrap/expect
/// - §4.4 反模式 6: 全程 f64
/// - §17.2: 仅用 access_count + delta_t + tau 元数据,不加载 content
/// - §17.5: 6mo 级 KeepForever(tau=∞)永不降级
pub fn should_demote_metadata(access_count: u64, delta_t_seconds: f64, tau_seconds: f64) -> bool {
    let priority = compute_priority(access_count, delta_t_seconds, tau_seconds);
    priority < DEMOTION_THRESHOLD_F64
}

// ============================================================
// CompressedContent — 压缩结果
// ============================================================

/// 压缩元数据 — 压缩过程中产生的辅助信息
///
/// 字段根据压缩策略不同而填充:
/// - `HcwSummary`:填充 `compression_ratio`,`clv` 为 None
/// - `RelationExtraction`:填充 `compression_ratio`,`clv` 为 None(未抽取)
/// - `DeepCompression`:填充 `compression_ratio`,`clv` 为 None
#[derive(Debug, Clone, PartialEq)]
pub struct CompressionMetadata {
    /// 压缩比(原始大小 / 压缩后大小,> 1.0 表示有压缩)
    pub compression_ratio: f64,
    /// 关系抽取出的语义 CLV(512-dim,`nexus_core::CLV` 形态)
    ///
    /// W8 假数据治理: 原实现生成 512-dim **零向量占位**——消费方无法区分
    /// "真实零活动语义"与"未抽取",构成虚假数据固化。改为 `Option`:
    /// `None` = CLV 未抽取(语义抽取由 mlc-engine 异步完成,诚实标注);
    /// `Some(..)` = 真实语义 CLV(未来接入 mlc-engine 后填充)。
    pub clv: Option<Vec<f32>>,
}

/// 压缩结果 — `ArchiveCompressor::compress` 的输出
///
/// 包含摘要文本、Token 数、压缩元数据。
#[derive(Debug, Clone, PartialEq)]
pub struct CompressedContent {
    /// 压缩后的摘要文本
    pub summary: String,
    /// 摘要的 Token 数(≤ max_tokens 约束)
    pub token_count: usize,
    /// 压缩元数据(压缩比、CLV 占位等)
    pub metadata: CompressionMetadata,
}

// ============================================================
// ArchiveCompressor — 归档压缩器
// ============================================================

/// 归档压缩器 — 三级归档压缩入口(§17.2 压缩策略)
///
/// 设计为关联函数(非 `&self` 方法),因为压缩无状态,无需实例化。
///
/// ## 复用映射(§17.1)
///
/// - **1mo HcwSummary**:hcw-window `ContextCompressor` 的重要性评分公式
///   (0.4×recency + 0.3×frequency + 0.3×relevance)面向 `ContextEntry` 数组按评分
///   Top-N 保留;本模块只有无属性的纯文本,没有可打分的时间/频次/相关性维度,
///   因此**权重不参与截断**,仅作为策略声明做自洽性校验,实际取前 `max_tokens`
///   字符作为摘要(诚实性说明见 `hcw_summary`)
/// - **3mo RelationExtraction**:复用 mlc-engine `SemanticMemory` 概念,
///   但 mlc-engine API 需 SQLite 持久化,本地实现生成 512-dim 零向量占位
/// - **6mo DeepCompression**:关键决策不压缩(§17.5 KeepForever),
///   本地实现:保留原文,仅记录压缩比为 1.0
///
/// ## 红线对齐
///
/// - §4.1: 库层 thiserror,无 unwrap/expect
/// - §6.1: 单函数 ≤ 200 行(本模块函数均 < 50 行)
/// - §17.1: 复用 hcw-window Ω-Compress,不自实现压缩算法
/// - §17.5: 6mo 级 KeepForever,关键决策不压缩
pub struct ArchiveCompressor;

impl ArchiveCompressor {
    /// 压缩内容(三级归档压缩入口,§17.2)
    ///
    /// 根据 `strategy` 分派到具体压缩方法:
    /// - `HcwSummary`:校验权重声明后,按 `max_tokens` 字符预算取前缀作为摘要
    /// - `RelationExtraction`:生成 512-dim 零向量占位 + 原文摘要
    /// - `DeepCompression`:保留原文(KeepForever,关键决策不压缩)
    ///
    /// ## 参数
    ///
    /// - `strategy`:压缩策略(由 `ArchiveScheduleLevel::compression_strategy()` 生成)
    /// - `content`:待压缩的原始内容
    ///
    /// ## 返回
    ///
    /// - `Ok(CompressedContent)`:压缩结果(摘要 + Token 数 + 元数据)
    /// - `Err(MasError::Internal)`:不应发生的内部错误
    pub fn compress(strategy: &CompressionStrategy, content: &str) -> Result<CompressedContent> {
        match strategy {
            CompressionStrategy::HcwSummary {
                max_tokens,
                weights,
            } => Self::hcw_summary(content, *max_tokens, *weights),
            CompressionStrategy::RelationExtraction => Self::relation_extraction(content),
            CompressionStrategy::DeepCompression => Self::deep_compress(content),
        }
    }

    /// 校验 `HcwSummary.weights` 声明自洽(项数固定 3、逐项有限非负、和 ≈ 1.0)
    ///
    /// ## 参数
    ///
    /// - `weights`:策略声明的重要性评分权重 [时近性, 频次, 任务相关性]
    ///
    /// ## 返回
    ///
    /// - `Ok(())`:声明自洽,可继续压缩
    /// - `Err(MasError::InvalidConfig)`:逐项非法或求和偏离 1.0
    ///
    /// WHY fail-closed:该字段是公开可构造的枚举载荷,外部调用方可传入任意值。
    /// 若放任 NaN / 负数 / 全零 / 和不为 1 的权重通过,策略声明会退化成静默
    /// 无操作 —— 正是 F-R3-01 的成因(权重被丢弃却无人察觉)。
    /// 容差取 [`HCW_WEIGHT_SUM_TOLERANCE`] 以吸收 `[0.4, 0.3, 0.3]` 的 f64 加法误差。
    fn validate_hcw_weights(weights: &[f64; 3]) -> Result<()> {
        for (idx, w) in weights.iter().enumerate() {
            if !w.is_finite() || *w < 0.0 {
                return Err(MasError::InvalidConfig {
                    field: format!("CompressionStrategy::HcwSummary.weights[{idx}]"),
                    value: w.to_string(),
                });
            }
        }

        let sum: f64 = weights.iter().sum();
        if (sum - 1.0).abs() > HCW_WEIGHT_SUM_TOLERANCE {
            return Err(MasError::InvalidConfig {
                field: "CompressionStrategy::HcwSummary.weights.sum".into(),
                value: format!("{sum}"),
            });
        }

        Ok(())
    }

    /// HCW 摘要压缩(1mo 级)— 字符预算前缀截断,权重仅校验不生效
    ///
    /// 复用 crate API 不匹配:hcw-window `ContextCompressor` 面向 `ContextEntry`
    /// 数组(按 `score = recency×w0 + frequency×w1 + relevance×w2` 做 Top-N 保留)。
    /// 本函数只拿到一段**无属性的纯文本**,没有访问时间、频次或任务相关性可打分,
    /// 因此**无法**实际应用该公式;强行"按权重比例把文本切成三段再拼回"只是把
    /// 同一段字符任意重排,信息量为零,还会破坏叙事头部的连续性(违反 INV-8
    /// 归档保真度方向)。
    ///
    /// 当前实现因此只做两件事:
    /// 1. [`Self::validate_hcw_weights`] 校验权重声明自洽(fail-closed);
    /// 2. 按 `max_tokens` 字符预算取前缀作为摘要。
    ///
    /// 接入真实评分需要调用方提供分段级 recency/frequency/relevance,
    /// 属公开 API 破坏性变更,须另立 ADR 后再做。
    ///
    /// ## 参数
    ///
    /// - `content`:待摘要的原始内容
    /// - `max_tokens`:字符预算上限(近似 token 数,见下方近似说明)
    /// - `weights`:重要性评分权重声明,仅做自洽性校验
    ///
    /// ## 返回
    ///
    /// - `Ok(CompressedContent)`:摘要长度 = min(原文字符数, max_tokens)
    /// - `Err(MasError::InvalidConfig)`:权重声明不自洽
    fn hcw_summary(
        content: &str,
        max_tokens: usize,
        weights: [f64; 3],
    ) -> Result<CompressedContent> {
        // 先校验策略声明:零预算不豁免校验(非法权重本身就是配置错误)
        Self::validate_hcw_weights(&weights)?;

        // 边界:max_tokens = 0 时返回空摘要
        if max_tokens == 0 {
            return Ok(CompressedContent {
                summary: String::new(),
                token_count: 0,
                metadata: CompressionMetadata {
                    compression_ratio: 1.0,
                    clv: None,
                },
            });
        }

        // 简化实现:按字符数近似 token 数(中文 1 字符 ≈ 1 token,英文 4 字符 ≈ 1 token)
        // 注:实际 Token 计数应由 hcw-window 的 tokenizer 完成,本地实现用字符数近似
        // 按 chars() 而非字节索引截断,保证多字节内容不落在字符边界内 panic
        let original_chars = content.chars().count();
        let summary_chars = original_chars.min(max_tokens);

        let summary: String = content.chars().take(summary_chars).collect();
        let token_count = summary.chars().count();

        // 压缩比 = 原始大小 / 压缩后大小(> 1.0 表示有压缩)
        let compression_ratio = if token_count > 0 {
            original_chars as f64 / token_count as f64
        } else {
            1.0
        };

        Ok(CompressedContent {
            summary,
            token_count,
            metadata: CompressionMetadata {
                compression_ratio,
                clv: None,
            },
        })
    }

    /// 关系抽取压缩(3mo 级)— 摘要保留原文,CLV 诚实标注未抽取
    ///
    /// 复用 crate API 不匹配:mlc-engine `SemanticMemory` 需 SQLite 持久化,
    /// 本地实现不伪造语义——`clv: None` 明确标注"语义抽取由 mlc-engine
    /// 异步完成",消费方不得将 None 当作真实零活动语义(W8 假数据治理)。
    fn relation_extraction(content: &str) -> Result<CompressedContent> {
        // 摘要保留原文(关系抽取阶段不压缩文本,CLV 语义抽取待异步完成)
        let summary = content.to_string();
        let token_count = content.chars().count();
        let compression_ratio = 1.0;

        Ok(CompressedContent {
            summary,
            token_count,
            metadata: CompressionMetadata {
                compression_ratio,
                clv: None,
            },
        })
    }

    /// 深度压缩(6mo 级)— 关键决策不压缩,KeepForever(§17.5)
    ///
    /// 语义(§17.5):"6 月级 KeepForever 且关键决策不压缩(防幽灵记忆)"
    ///
    /// 本地实现:保留原文,仅记录压缩比为 1.0(无压缩)。
    /// 实际深度压缩 + 模式抽取由后续 Stage B 实现(见 §17.4 降级适配表)。
    fn deep_compress(content: &str) -> Result<CompressedContent> {
        let token_count = content.chars().count();
        Ok(CompressedContent {
            summary: content.to_string(),
            token_count,
            metadata: CompressionMetadata {
                compression_ratio: 1.0,
                clv: None,
            },
        })
    }
}

// ============================================================
// 单元测试(模块内,与集成测试 tests/archive_test.rs 互补)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// DEMOTION_THRESHOLD_F64 常量稳定性
    #[test]
    fn demotion_threshold_constant_stable() {
        assert_eq!(DEMOTION_THRESHOLD_F64, 0.1);
    }

    /// compute_priority 边界:access_count = 0 返回 0.0
    #[test]
    fn compute_priority_zero_access_count() {
        assert_eq!(compute_priority(0, 100.0, 86400.0), 0.0);
    }

    /// compute_priority 边界:delta_t = 0 返回 access_count
    #[test]
    fn compute_priority_zero_delta_t() {
        assert!((compute_priority(5, 0.0, 86400.0) - 5.0).abs() < 1e-9);
    }

    /// compute_priority 边界:tau = ∞ 返回 access_count(KeepForever)
    #[test]
    fn compute_priority_infinite_tau() {
        assert!((compute_priority(3, 1_000_000.0, f64::INFINITY) - 3.0).abs() < 1e-9);
    }

    /// compute_priority 公式:tau=24h, delta=24h → exp(-1) ≈ 0.3679
    #[test]
    fn compute_priority_exp_neg1() {
        let p = compute_priority(1, 86400.0, 86400.0);
        assert!((p - (-1.0_f64).exp()).abs() < 1e-9);
    }

    /// should_demote_metadata:access_count=0 返回 true
    #[test]
    fn should_demote_zero_access_count() {
        assert!(should_demote_metadata(0, 0.0, 86400.0));
    }

    /// should_demote_metadata:tau=∞ 返回 false(KeepForever)
    #[test]
    fn should_demote_infinite_tau_never_demotes() {
        assert!(!should_demote_metadata(1, 1_000_000.0, f64::INFINITY));
    }

    // ------------------------------------------------------------
    // F-R3-01 契约:HcwSummary.weights 是公开可构造的策略字段,必须
    // fail-closed 校验其自洽性(否则"声明与实现脱钩"可再次静默发生);
    // 摘要必须受 max_tokens 预算约束,截断只能落在 char 边界。
    // ------------------------------------------------------------

    use crate::archive::tier::HCW_SUMMARY_WEIGHTS;
    use crate::error::MasError;
    use proptest::prelude::*;

    fn hcw_strategy(max_tokens: usize, weights: [f64; 3]) -> CompressionStrategy {
        CompressionStrategy::HcwSummary {
            max_tokens,
            weights,
        }
    }

    fn expect_invalid_config(weights: [f64; 3], max_tokens: usize) {
        let err = ArchiveCompressor::compress(&hcw_strategy(max_tokens, weights), "归档内容样本")
            .expect_err("非法权重必须被拒绝");
        assert!(
            matches!(err, MasError::InvalidConfig { .. }),
            "期望 InvalidConfig,实际 {err:?}"
        );
    }

    #[test]
    fn hcw_summary_rejects_negative_weight() {
        expect_invalid_config([0.7, -0.1, 0.4], 10);
    }

    #[test]
    fn hcw_summary_rejects_weight_sum_drift() {
        expect_invalid_config([0.4, 0.3, 0.1], 10);
    }

    #[test]
    fn hcw_summary_rejects_non_finite_weight() {
        expect_invalid_config([f64::NAN, 0.5, 0.5], 10);
        expect_invalid_config([f64::INFINITY, 0.0, 0.0], 10);
    }

    /// 零预算只说明"不保留内容",不豁免策略自洽性校验
    #[test]
    fn hcw_summary_validates_weights_even_with_zero_budget() {
        expect_invalid_config([0.5, 0.5, 0.5], 0);
    }

    #[test]
    fn hcw_summary_accepts_declared_default_weights() {
        let content = "一段需要摘要的归档内容";
        let out = ArchiveCompressor::compress(&hcw_strategy(500, HCW_SUMMARY_WEIGHTS), content)
            .expect("默认权重 [0.4,0.3,0.3] 必须通过校验");
        assert_eq!(out.token_count, content.chars().count());
    }

    /// 每个 '中' 占 3 字节:按字节索引截断会直接 panic
    #[test]
    fn hcw_summary_truncates_on_char_boundary() {
        let content = "中".repeat(300);
        let out = ArchiveCompressor::compress(&hcw_strategy(100, HCW_SUMMARY_WEIGHTS), &content)
            .expect("默认权重应通过校验");
        assert_eq!(out.summary.chars().count(), 100);
        assert_eq!(out.token_count, 100);
        assert!(
            content.starts_with(out.summary.as_str()),
            "摘要必须是原文前缀"
        );
    }

    #[test]
    fn hcw_summary_ratio_never_below_one_when_truncating() {
        let content = "abcdefghij".repeat(50);
        let out = ArchiveCompressor::compress(&hcw_strategy(20, HCW_SUMMARY_WEIGHTS), &content)
            .expect("默认权重应通过校验");
        assert_eq!(out.token_count, 20);
        assert!(out.metadata.compression_ratio >= 1.0);
    }

    proptest! {
        #[test]
        fn budget_never_exceeded(max_tokens in 0usize..400, len in 0usize..600) {
            let content = "字".repeat(len);
            let out = ArchiveCompressor::compress(
                &hcw_strategy(max_tokens, HCW_SUMMARY_WEIGHTS),
                &content,
            )
            .expect("合法权重不应失败");
            prop_assert!(out.token_count <= max_tokens);
            prop_assert_eq!(out.token_count, len.min(max_tokens));
        }

        #[test]
        fn invalid_weights_never_accepted(a in -2.0f64..2.0, b in -2.0f64..2.0, c in -2.0f64..2.0) {
            let valid = a >= 0.0 && b >= 0.0 && c >= 0.0 && (a + b + c - 1.0).abs() <= 1e-6;
            let res = ArchiveCompressor::compress(&hcw_strategy(8, [a, b, c]), "归档内容样本");
            if valid {
                prop_assert!(res.is_ok(), "合法权重 {a},{b},{c} 被误拒");
            } else {
                prop_assert!(res.is_err(), "非法权重 {a},{b},{c} 被误接受");
            }
        }
    }
}
