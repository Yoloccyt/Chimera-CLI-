//! MemCon 自适应控制器配置 — 幽灵记忆检测与策略自适应调整参数
//!
//! 对应架构层:L2 Memory
//! 对应任务:P2-8 MemCon 自适应控制器
//!
//! # 设计决策(WHY)
//! - **window_size=100**:滑动窗口大小,平衡检测灵敏度与稳定性。过小(如10)导致
//!   误报率升高(单次副作用假阳性),过大(如1000)导致检测滞后(策略调整延迟)。
//!   100次召回 ≈ 1-10秒(取决于操作频率),提供合理的检测实时性。
//! - **ghost_threshold=0.3**:30%幽灵率触发调整。经验值:正常召回中≤10%的条目
//!   可能因数据写入到读取的间隙产生"看似幽灵"的现象,15-20%阈值可过滤正常波动,
//!   30%确保幽灵记忆真正成为系统性模式。
//! - **cooldown_secs=60**:调整后冷却期,避免频繁震荡。策略调整生效后需要时间
//!   观察效果,60秒内不重复调整,给系统足够的稳定窗口。
//! - **circuit_breaker_ghost_rate=0.8**:调整后幽灵率仍≥80%时触发熔断回退。
//!   80%意味着几乎全部召回都是幽灵记忆,策略调整不仅无效甚至可能恶化。
//!   熔断回退到 StandardTopK(最保守的 fallback),确保系统安全。
//!
//! # C4 合规三层 fallback
//! - 默认值层:config 中的默认值(编译期常量)
//! - 异常回退层:无效配置回退到默认值
//! - 熔断入口层:circuit_breaker_ghost_rate 触发熔断时回退到 StandardTopK

use serde::{Deserialize, Serialize};

/// MemCon 自适应控制器配置
///
/// 控制幽灵记忆检测与策略自适应调整的阈值、窗口和冷却参数。
/// 所有字段均有合理的默认值(编译期常量),调用方可按需覆盖。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemConConfig {
    /// 滑动窗口大小(默认 100)
    ///
    /// 幽灵记忆检测的滑动窗口大小,即最近 N 次召回操作的结果。
    /// 窗口内记录每次召回是否为幽灵记忆,用于计算幽灵率。
    pub window_size: usize,

    /// 幽灵记忆检测阈值(默认 0.3,范围 [0.0, 1.0])
    ///
    /// 当窗口内幽灵率超过此阈值时,触发策略调整。
    pub ghost_threshold: f32,

    /// 调整后冷却期(秒,默认 60)
    ///
    /// 策略调整后在此时间内不再重复调整,避免频繁震荡。
    pub cooldown_secs: u64,

    /// 熔断幽灵率阈值(默认 0.8,范围 [0.0, 1.0])
    ///
    /// 调整后若幽灵率仍≥此值,触发熔断回退到 StandardTopK。
    pub circuit_breaker_ghost_rate: f32,

    /// 是否启用 MemCon 自适应控制器(默认 true)
    ///
    /// WHY 提供开关:在调试或性能基准测试中,可临时禁用 MemCon 以排除干扰。
    pub enabled: bool,
}

impl Default for MemConConfig {
    fn default() -> Self {
        Self {
            window_size: 100,
            ghost_threshold: 0.3,
            cooldown_secs: 60,
            circuit_breaker_ghost_rate: 0.8,
            enabled: true,
        }
    }
}

impl MemConConfig {
    /// 验证配置有效性
    ///
    /// 返回不合法字段的错误信息列表(空 Vec 表示完全合法)。
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();

        if self.window_size == 0 {
            errors.push("window_size 必须大于 0".into());
        }
        if self.window_size > 10_000 {
            errors.push("window_size 不能超过 10000".into());
        }
        if !(0.0..=1.0).contains(&self.ghost_threshold) {
            errors.push("ghost_threshold 必须在 [0.0, 1.0] 范围内".into());
        }
        if self.cooldown_secs == 0 {
            errors.push("cooldown_secs 必须大于 0".into());
        }
        if self.cooldown_secs > 3600 {
            errors.push("cooldown_secs 不能超过 3600".into());
        }
        if !(0.0..=1.0).contains(&self.circuit_breaker_ghost_rate) {
            errors.push("circuit_breaker_ghost_rate 必须在 [0.0, 1.0] 范围内".into());
        }

        errors
    }

    /// 创建默认配置(便捷方法,与 Default 一致)
    pub fn default_config() -> Self {
        Self::default()
    }
}
