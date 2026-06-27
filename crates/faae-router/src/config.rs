//! FaaE 配置 — Function-as-Expert 语义路由与 EDSB 熵均衡的参数化配置
//!
//! 对应架构层:L6 Router
//!
//! # 设计决策(WHY)
//! - **top_k 默认 8**:FaaE 作为 KVBSR 的"精筛"层,从 KVBSR 粗筛的 Top-3 块工具集中
//!   精筛 Top-8 工具。8 个工具覆盖大多数任务需求,与 OSA 的 routing Top-K 下界对齐
//! - **entropy_threshold 默认 0.6**:熵值低于此阈值时触发均衡。0.6 平衡准确性与均衡性,
//!   过高会过度均衡(破坏语义路由),过低会均衡不足(负载持续集中)
//! - **decay_tau 默认 3600 秒(1 小时)**:指数衰减时间常数。1 小时平衡时近性与历史,
//!   近期使用权重更高,过时工具的负载逐渐淡出
//! - **balance_enabled 默认 true**:默认启用 EDSB 均衡,可在测试或特殊场景关闭

use serde::{Deserialize, Serialize};

/// FaaE 路由器配置 — Function-as-Expert 语义路由与 EDSB 熵均衡的参数化控制
///
/// 所有字段在创建时填充,不可变(无需内部可变性)。
/// `Default` 提供架构手册推荐的默认值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FaaeConfig {
    /// 精筛 Top-K 工具数(默认 8)
    ///
    /// WHY:FaaE 作为 KVBSR 的"精筛"层,从 KVBSR 粗筛的 Top-3 块工具集中精筛 Top-8 工具。
    /// 8 个工具覆盖大多数任务需求,与 OSA 的 routing Top-K 下界对齐
    pub top_k: usize,

    /// 熵均衡阈值(默认 0.6),熵值低于此阈值时触发均衡
    ///
    /// WHY:0.6 平衡准确性与均衡性。熵值 ∈ [0, 1],0 表示完全集中,1 表示完全均匀。
    /// 低于 0.6 表示负载过度集中,需要均衡;高于 0.6 表示负载足够均匀,无需干预
    pub entropy_threshold: f32,

    /// 指数衰减时间常数 τ(默认 3600 秒 = 1 小时)
    ///
    /// WHY:1 小时平衡时近性与历史。衰减公式 `decayed = raw × exp(-Δt / τ)`:
    /// - Δt = 5 分钟:衰减 8%(近期使用几乎不衰减)
    /// - Δt = 1 小时:衰减 63%(一小时前的使用权重减半)
    /// - Δt = 2 小时:衰减 86%(两小时前的使用几乎淡出)
    pub decay_tau: f64,

    /// 是否启用 EDSB 熵均衡(默认 true)
    ///
    /// WHY:默认启用均衡以确保负载分布。可在测试或特殊场景(如纯语义路由基准)关闭
    pub balance_enabled: bool,
}

impl FaaeConfig {
    /// 创建默认配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置精筛 Top-K 工具数
    pub fn with_top_k(mut self, k: usize) -> Self {
        self.top_k = k;
        self
    }

    /// 设置熵均衡阈值
    pub fn with_entropy_threshold(mut self, threshold: f32) -> Self {
        self.entropy_threshold = threshold;
        self
    }

    /// 设置指数衰减时间常数 τ(秒)
    pub fn with_decay_tau(mut self, tau: f64) -> Self {
        self.decay_tau = tau;
        self
    }

    /// 设置是否启用 EDSB 熵均衡
    pub fn with_balance_enabled(mut self, enabled: bool) -> Self {
        self.balance_enabled = enabled;
        self
    }
}

impl Default for FaaeConfig {
    fn default() -> Self {
        Self {
            top_k: 8,
            entropy_threshold: 0.6,
            decay_tau: 3600.0,
            balance_enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = FaaeConfig::default();
        assert_eq!(config.top_k, 8);
        assert!((config.entropy_threshold - 0.6).abs() < 1e-6);
        assert!((config.decay_tau - 3600.0).abs() < 1e-6);
        assert!(config.balance_enabled);
    }

    #[test]
    fn test_builder_chain() {
        let config = FaaeConfig::new()
            .with_top_k(16)
            .with_entropy_threshold(0.8)
            .with_decay_tau(1800.0)
            .with_balance_enabled(false);
        assert_eq!(config.top_k, 16);
        assert!((config.entropy_threshold - 0.8).abs() < 1e-6);
        assert!((config.decay_tau - 1800.0).abs() < 1e-6);
        assert!(!config.balance_enabled);
    }
}
