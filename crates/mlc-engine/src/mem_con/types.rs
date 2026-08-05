//! MemCon 自适应控制器类型定义 — 统计与调整原因枚举
//!
//! 对应架构层:L2 Memory
//! 对应任务:P2-8 MemCon 自适应控制器
//!
//! # 类型职责
//! - `MemConStats`:MemCon 控制器的运行统计(累积计数)
//! - `AdjustmentReason`:策略调整原因的枚举(用于事件发布与日志)
//! - `AdjustmentOutcome`:策略调整结果(成功/熔断/无变化)
//!
//! # 设计决策(WHY)
//! - **MemConStats 使用 u64 原子计数**:与 MlcEngine 的 op_count/hit_count 一致,
//!   避免锁竞争,支持高并发场景。
//! - **AdjustmentReason 为 String 友好枚举**:序列化为 snake_case 字符串,
//!   便于事件发布、日志记录和 TUI 展示,与 NexusEvent 的 reason 字段对齐。

use serde::{Deserialize, Serialize};
use std::fmt;

/// MemCon 控制器运行统计
///
/// 累积统计 MemCon 控制器从创建到当前的生命周期指标。
/// 所有字段为 u64 简单计数器,适合原子操作。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct MemConStats {
    /// 总召回次数(通过 MemCon 的 recall hook 记录)
    pub total_recalls: u64,
    /// 总幽灵记忆检测次数(窗口内判定为幽灵的召回)
    pub total_ghost_detections: u64,
    /// 策略调整次数(StrategyAdapter 实际执行调整的次数)
    pub adjustments_count: u64,
    /// 熔断激活次数(调整后幽灵率仍超阈值,回退到 StandardTopK)
    pub circuit_breaker_activations: u64,
}

impl MemConStats {
    /// 创建新的空统计
    pub fn new() -> Self {
        Self::default()
    }
}

/// 策略调整原因枚举
///
/// 描述 MemCon 控制器为何调整记忆策略,用于事件发布和日志记录。
/// 序列化为 snake_case 字符串,与 NexusEvent 的 reason 字段对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdjustmentReason {
    /// 幽灵记忆检测 — 窗口内幽灵率超过阈值,需要收紧策略
    GhostMemoryDetected,
    /// 稳定恢复 — 窗口内幽灵率恢复正常,可以放宽策略
    StableRecovery,
    /// 熔断回退 — 调整后幽灵率仍超阈值,回退到 StandardTopK
    CircuitBreaker,
}

impl AdjustmentReason {
    /// 返回人类可读的描述字符串
    pub fn description(&self) -> &'static str {
        match self {
            Self::GhostMemoryDetected => "幽灵记忆检测率超过阈值,触发策略收紧",
            Self::StableRecovery => "幽灵记忆已恢复正常,触发策略放宽",
            Self::CircuitBreaker => "调整后幽灵率仍超阈值,熔断回退到StandardTopK",
        }
    }
}

impl fmt::Display for AdjustmentReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}

/// 策略调整结果
///
/// 描述一次策略调整尝试的结果,用于决策是否重置冷却期、是否需要
/// 熔断回退,以及事件发布。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdjustmentOutcome {
    /// 成功调整到新策略
    Adjusted,
    /// 熔断回退 — 新策略效果不佳,回退到 StandardTopK
    CircuitBroke,
    /// 无变化 — 当前策略无需调整(如幽灵率未超阈值,或冷却期中)
    NoChange,
}
