//! 归档层级与压缩策略定义 — Task 17 §17.2 三级归档映射
//!
//! 架构层归属: L9 Quest(chimera-mas/archive 子模块)
//! 核心职责: 定义三级归档层级(1mo / 3mo / 6mo)与对应压缩策略
//!
//! ## 设计决策(WHY)
//!
//! - **三级层级枚举**: `ArchiveScheduleLevel` 编码 1mo / 3mo / 6mo 三级,
//!   每级绑定固定的 cron 表达式、源/目标 tier、压缩策略
//! - **触发条件枚举**: `ArchiveTriggerCondition` 区分 cron 触发与容量超限触发,
//!   1mo 级支持容量触发(> 10000 条目),3mo / 6mo 级仅 cron 触发
//! - **压缩策略枚举**: `CompressionStrategy` 编码 §17.2 三种压缩方式,
//!   1mo=HCW 摘要,3mo=关系抽取,6mo=深度压缩 + 模式抽取
//! - **INV-8 单调性**: `source_tier()` → `target_tier()` 严格降级
//!   (Hot→Warm / Warm→Cold / Cold→Ice),由 `ArchiveScheduler::trigger` 校验
//!
//! ## 红线对齐
//!
//! - §4.1: 库层 thiserror,无 unwrap/expect
//! - §6.1: 单函数 ≤ 200 行
//! - INV-8(§17.5): source_tier < target_tier 严格单调

use crate::invariants::ArchiveTier;

// ============================================================
// 常量(SubTask 17.9 REFACTOR — 抽取)
// ============================================================

/// 1 月级 cron 表达式(§17.2)— 每月 1 日 02:00 触发
///
/// 格式:`秒 分 时 日 月 *`
/// - `0 2 1 * *`:秒 0 / 分 2 / 时 1 / 日 * / 月 *
///
/// WHY 抽取为常量:防止 cron 表达式被意外修改导致触发时机漂移,
/// efficiency-monitor 订阅时与本常量保持一致。
pub const CRON_MONTH1: &str = "0 2 1 * *";

/// 3 月级 cron 表达式(§17.2)— 1/4/7/10 月 1 日 03:00 触发
///
/// 格式:`秒 分 时 日 月 *`
/// - `0 3 1 1,4,7,10 *`:秒 0 / 分 3 / 时 1 / 日 1,4,7,10 / 月 *
pub const CRON_MONTH3: &str = "0 3 1 1,4,7,10 *";

/// 6 月级 cron 表达式(§17.2)— 1/7 月 1 日 04:00 触发
///
/// 格式:`秒 分 时 日 月 *`
/// - `0 4 1 1,7 *`:秒 0 / 分 4 / 时 1 / 日 1,7 / 月 *
pub const CRON_MONTH6: &str = "0 4 1 1,7 *";

// ============================================================
// ArchiveScheduleLevel — 三级归档层级枚举
// ============================================================

/// Agent 记忆三级归档层级(§17.2)
///
/// 编码 1 月 / 3 月 / 6 月三级归档,每级绑定:
/// - `cron_expression()`:cron 触发表达式(供 efficiency-monitor 决定何时调用)
/// - `source_tier()` / `target_tier()`:INV-8 单调性源/目标 tier
/// - `compression_strategy()`:对应压缩策略(§17.2 三种)
///
/// ## 单调性链
///
/// `Month1: Hot→Warm` < `Month3: Warm→Cold` < `Month6: Cold→Ice`
///
/// ## 示例
///
/// ```
/// use chimera_mas::archive::ArchiveScheduleLevel;
/// use chimera_mas::invariants::ArchiveTier;
///
/// assert_eq!(ArchiveScheduleLevel::Month1.cron_expression(), "0 2 1 * *");
/// assert_eq!(ArchiveScheduleLevel::Month1.target_tier(), ArchiveTier::Warm);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveScheduleLevel {
    /// 1 月级归档 — cron `0 2 1 * *` 或 条目 > 10000 触发
    ///
    /// 压缩策略:HCW 摘要 ≤500 tok(权重 0.4/0.3/0.3)
    /// 存储层:CMT Warm(4096)
    /// 降级阈值:priority < 0.1
    Month1,

    /// 3 月级归档 — cron `0 3 1 1,4,7,10 *` 触发
    ///
    /// 压缩策略:关系抽取 → mlc L2 语义(512-dim CLV)
    /// 存储层:CMT Cold(65536)
    /// 衰减:τ=24h
    Month3,

    /// 6 月级归档 — cron `0 4 1 1,7 *` 触发
    ///
    /// 压缩策略:深度压缩 + 模式抽取
    /// 存储层:CMT Ice(∞,只读)
    /// 保留:KeepForever(无衰减)
    Month6,
}

impl ArchiveScheduleLevel {
    /// 返回该归档级的 cron 表达式(§17.2)
    ///
    /// 供 efficiency-monitor 决定何时调用 `ArchiveScheduler::trigger`。
    /// 注意:本模块不解析 cron 表达式(不新建调度 crate,§17.3),
    /// efficiency-monitor 负责实际调度。
    pub const fn cron_expression(self) -> &'static str {
        match self {
            ArchiveScheduleLevel::Month1 => CRON_MONTH1,
            ArchiveScheduleLevel::Month3 => CRON_MONTH3,
            ArchiveScheduleLevel::Month6 => CRON_MONTH6,
        }
    }

    /// 返回该归档级的源 tier(§17.5 INV-8 单调性)
    ///
    /// - `Month1`: Hot(level 0)
    /// - `Month3`: Warm(level 1)
    /// - `Month6`: Cold(level 2)
    pub const fn source_tier(self) -> ArchiveTier {
        match self {
            ArchiveScheduleLevel::Month1 => ArchiveTier::Hot,
            ArchiveScheduleLevel::Month3 => ArchiveTier::Warm,
            ArchiveScheduleLevel::Month6 => ArchiveTier::Cold,
        }
    }

    /// 返回该归档级的目标 tier(§17.5 INV-8 单调性)
    ///
    /// - `Month1`: Warm(level 1)
    /// - `Month3`: Cold(level 2)
    /// - `Month6`: Ice(level 3)
    pub const fn target_tier(self) -> ArchiveTier {
        match self {
            ArchiveScheduleLevel::Month1 => ArchiveTier::Warm,
            ArchiveScheduleLevel::Month3 => ArchiveTier::Cold,
            ArchiveScheduleLevel::Month6 => ArchiveTier::Ice,
        }
    }

    /// 返回该归档级的压缩策略(§17.2 三种压缩方式)
    ///
    /// - `Month1`: `HcwSummary { max_tokens: 500, weights: [0.4, 0.3, 0.3] }`
    /// - `Month3`: `RelationExtraction`
    /// - `Month6`: `DeepCompression`
    pub const fn compression_strategy(self) -> CompressionStrategy {
        match self {
            ArchiveScheduleLevel::Month1 => CompressionStrategy::HcwSummary {
                max_tokens: MAX_HCW_SUMMARY_TOKENS,
                weights: HCW_SUMMARY_WEIGHTS,
            },
            ArchiveScheduleLevel::Month3 => CompressionStrategy::RelationExtraction,
            ArchiveScheduleLevel::Month6 => CompressionStrategy::DeepCompression,
        }
    }
}

// ============================================================
// 常量 — HCW 摘要参数(§17.2)
// ============================================================

/// HCW 摘要最大 Token 数(§17.2)— 1mo 级压缩目标 ≤500 tok
///
/// WHY 抽取为常量:防止 max_tokens 被意外修改导致 HCW 摘要超限,
/// 多处引用(1mo 压缩策略 / 测试断言)共享同一真值源。
pub const MAX_HCW_SUMMARY_TOKENS: usize = 500;

/// HCW 摘要权重(§17.2)— 0.4 时近性 / 0.3 频次 / 0.3 任务相关性
///
/// 复用 hcw-window `ContextCompressor` 的重要性评分公式(§17.1):
/// `score = 0.4 × recency + 0.3 × frequency + 0.3 × relevance`
///
/// WHY 用 `[f64; 3]` 而非 struct:与 `CompressionStrategy::HcwSummary` 字段类型一致,
/// 便于直接赋值,避免字段名拼写错误。
pub const HCW_SUMMARY_WEIGHTS: [f64; 3] = [0.4, 0.3, 0.3];

// ============================================================
// CompressionStrategy — 压缩策略枚举
// ============================================================

/// 归档压缩策略(§17.2)— 三级归档对应三种压缩方式
///
/// ## 设计决策(WHY)
///
/// - **HcwSummary**:1mo 级,复用 hcw-window Ω-Compress 重要性评分公式,
///   生成 ≤500 tok 摘要。注:hcw-window `ContextCompressor` 面向 `ContextEntry`
///   数组(按评分 Top-N 保留),不直接适配"文本 → 摘要"场景,本地实现按权重切分。
/// - **RelationExtraction**:3mo 级,关系抽取 → mlc L2 语义(512-dim CLV)。
///   注:mlc-engine `SemanticMemory` 需 SQLite 持久化,本地实现生成 512-dim
///   零向量占位,实际语义抽取由 mlc-engine 异步完成。
/// - **DeepCompression**:6mo 级,深度压缩 + 模式抽取,关键决策不压缩(KeepForever)。
#[derive(Debug, Clone, PartialEq)]
pub enum CompressionStrategy {
    /// HCW 摘要(1mo 级)— 复用 hcw-window 重要性评分公式
    ///
    /// 字段:
    /// - `max_tokens`:摘要最大 Token 数(默认 500,§17.2)
    /// - `weights`:重要性评分权重 [recency, frequency, relevance](默认 [0.4, 0.3, 0.3])
    HcwSummary {
        /// 摘要最大 Token 数
        max_tokens: usize,
        /// 重要性评分权重 [时近性, 频次, 任务相关性]
        weights: [f64; 3],
    },

    /// 关系抽取(3mo 级)— 关系抽取 → mlc L2 语义(512-dim CLV)
    ///
    /// 实际语义抽取由 mlc-engine 异步完成,本地实现生成 512-dim 零向量占位。
    RelationExtraction,

    /// 深度压缩 + 模式抽取(6mo 级)— 关键决策不压缩,KeepForever
    DeepCompression,
}

// ============================================================
// ArchiveTriggerCondition — 触发条件枚举
// ============================================================

/// 归档触发条件(§17.2 "时间 + 容量混合")
///
/// 区分 cron 定时触发与容量超限触发,1mo 级支持两种,3mo / 6mo 级仅 cron 触发。
///
/// ## 设计决策(WHY)
///
/// - **Cron**:定时触发(每月 1 日 / 季度首月 1 日 / 半年首月 1 日),
///   由 efficiency-monitor 订阅 `NexusEvent` 后调用 `ArchiveScheduler::trigger`
/// - **CapacityExceeded**:容量超限触发(1mo 级条目 > 10000),
///   efficiency-monitor 监控 Hot 层条目数,超阈值时触发
#[derive(Debug, Clone, PartialEq)]
pub enum ArchiveTriggerCondition {
    /// Cron 定时触发(§17.2)
    ///
    /// efficiency-monitor 按 `ArchiveScheduleLevel::cron_expression()` 调度
    Cron,

    /// 容量超限触发(§17.2,仅 1mo 级)— 条目 > threshold 时触发
    ///
    /// 字段:
    /// - `entry_count`:当前条目数
    /// - `threshold`:触发阈值(1mo 级 = 10000)
    CapacityExceeded {
        /// 当前条目数
        entry_count: usize,
        /// 触发阈值
        threshold: usize,
    },
}

// ============================================================
// ArchiveOperation — 归档操作计划
// ============================================================

/// 归档操作计划 — `ArchiveScheduler::trigger` 的输出
///
/// 描述一次归档操作的完整信息:层级、触发条件、源/目标 tier、压缩策略。
/// 调用方(效率监控 / 调度器)负责实际执行(调用 `ArchiveCompressor::compress`
/// + 写入 CMT 对应层 + 发布 Critical 事件)。
///
/// ## 设计决策(WHY)
///
/// - **纯数据结构**:无方法,仅描述操作计划,执行由调用方负责
/// - **source_tier / target_tier**:冗余存储 level 的 source/target,
///   便于调用方直接使用,无需再调用 `level.source_tier()`
/// - **compression**:冗余存储 level 的压缩策略,便于调用方直接传给
///   `ArchiveCompressor::compress`
#[derive(Debug, Clone, PartialEq)]
pub struct ArchiveOperation {
    /// 归档层级(1mo / 3mo / 6mo)
    pub level: ArchiveScheduleLevel,
    /// 触发条件(Cron 或 CapacityExceeded)
    pub condition: ArchiveTriggerCondition,
    /// 源归档层(INV-8 单调性:source < target)
    pub source_tier: ArchiveTier,
    /// 目标归档层(INV-8 单调性:source < target)
    pub target_tier: ArchiveTier,
    /// 压缩策略(由 level 决定)
    pub compression: CompressionStrategy,
}

// ============================================================
// 单元测试(模块内,与集成测试 tests/archive_test.rs 互补)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// cron 表达式常量稳定性:防止被意外修改
    #[test]
    fn cron_constants_are_stable() {
        assert_eq!(CRON_MONTH1, "0 2 1 * *");
        assert_eq!(CRON_MONTH3, "0 3 1 1,4,7,10 *");
        assert_eq!(CRON_MONTH6, "0 4 1 1,7 *");
    }

    /// HCW 摘要参数常量稳定性
    #[test]
    fn hcw_summary_constants_are_stable() {
        assert_eq!(MAX_HCW_SUMMARY_TOKENS, 500);
        assert_eq!(HCW_SUMMARY_WEIGHTS, [0.4, 0.3, 0.3]);
    }

    /// ArchiveScheduleLevel 三级的 cron 表达式均不为空
    #[test]
    fn schedule_level_cron_expressions_non_empty() {
        assert!(!ArchiveScheduleLevel::Month1.cron_expression().is_empty());
        assert!(!ArchiveScheduleLevel::Month3.cron_expression().is_empty());
        assert!(!ArchiveScheduleLevel::Month6.cron_expression().is_empty());
    }
}
