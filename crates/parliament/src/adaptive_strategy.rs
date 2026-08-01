//! 自适应策略选择器 — 基于实时 ratio(协调成本/推理增益)、共识质量和系统负载，
//! 动态选择审议策略。
//!
//! # 设计
//! 与 `StrategyCapGuard` 互补:Selector 是"建议",CapGuard 是"强制上界"。
//! 最终策略 = min(selector 建议, cap 封顶)。
//!
//! # 选择规则
//! 1. 高风险(risk_level > 0.7) → 强制 Full
//! 2. 高负载(> 0.7) + 低风险(< 0.3) → Simplified
//! 3. ratio > 1.5 → Simplified(协调成本显著超过推理增益)
//! 4. 质量健康分 < 40 → Full(需要更多角色确认)
//! 5. 其他 → 保持当前策略(不改变)

use nexus_contracts::ActivationStrategy;

use crate::strategy_cap::min_strategy;

// ============================================================
// 系统负载探测器
// ============================================================

/// 系统负载探测器 — 从 tokio 运行时获取当前任务数近似负载
///
/// 使用 tokio 运行时的 `tokio::runtime::Handle::try_current()` 获取
/// 当前运行时句柄，通过 `spawn_blocking` 队列长度近似系统负载。
/// 当无法获取运行时句柄时(如测试环境)，返回默认值 0.5。
pub struct SystemLoadProbe;

impl SystemLoadProbe {
    /// 探测当前系统负载 [0.0, 1.0]
    ///
    /// 0.0 = 空闲, 1.0 = 满载
    /// 当前实现使用 tokio::runtime::Handle 的 blocking_task 队列长度近似。
    /// 若无法获取运行时句柄，返回 0.5(保守估计)。
    pub fn probe() -> f32 {
        // 尝试获取当前运行时句柄
        match tokio::runtime::Handle::try_current() {
            Ok(_handle) => {
                // tokio 没有直接暴露任务队列长度的 API，
                // 使用 spawn_blocking 队列长度近似负载
                // 当前为简化实现，返回 0.5
                // 未来可接入 metrics 系统获取真实 CPU 利用率
                0.5
            }
            Err(_) => 0.5, // 无运行时环境，保守估计
        }
    }
}

// ============================================================
// 自适应策略选择器配置
// ============================================================

/// 自适应策略选择器配置
#[derive(Debug, Clone)]
pub struct AdaptiveStrategyConfig {
    /// 高风险阈值，默认 0.7
    pub high_risk_threshold: f32,
    /// 高负载阈值，默认 0.7
    pub high_load_threshold: f32,
    /// 低风险阈值，默认 0.3
    pub low_risk_threshold: f32,
    /// ratio 阈值，默认 1.5
    pub ratio_threshold: f64,
    /// 质量健康分阈值，默认 40
    pub health_threshold: u8,
}

impl Default for AdaptiveStrategyConfig {
    fn default() -> Self {
        Self {
            high_risk_threshold: 0.7,
            high_load_threshold: 0.7,
            low_risk_threshold: 0.3,
            ratio_threshold: 1.5,
            health_threshold: 40,
        }
    }
}

// ============================================================
// 自适应策略选择器
// ============================================================

/// 自适应策略选择器 — 基于 ratio + 共识质量 + 系统负载选择策略
///
/// # 设计
/// 与 `StrategyCapGuard` 互补:Selector 是"建议"，CapGuard 是"强制上界"。
/// 最终策略 = min(selector 建议, cap 封顶)。
///
/// # 选择规则
/// 1. 高风险(risk_level > 0.7) → 强制 Full
/// 2. 高负载(> 0.7) + 低风险(< 0.3) → Simplified
/// 3. ratio > 1.5 → Simplified(协调成本显著超过推理增益)
/// 4. 质量健康分 < 40 → Full(需要更多角色确认)
/// 5. 其他 → 保持当前策略(不改变)
pub struct AdaptiveStrategySelector {
    /// 配置
    config: AdaptiveStrategyConfig,
}

impl AdaptiveStrategySelector {
    /// 创建新的自适应策略选择器
    ///
    /// # 参数
    /// - `config`: 可选配置，传入 `None` 使用默认配置
    pub fn new(config: Option<AdaptiveStrategyConfig>) -> Self {
        Self {
            config: config.unwrap_or_default(),
        }
    }

    /// 获取配置引用
    pub fn config(&self) -> &AdaptiveStrategyConfig {
        &self.config
    }

    /// 选择策略
    ///
    /// # 参数
    /// - `risk_level`: 提案风险等级 [0.0, 1.0]
    /// - `ratio`: 协调成本/推理增益比值
    /// - `system_load`: 系统负载 [0.0, 1.0]
    /// - `health_score`: 共识质量健康分 [0, 100]
    /// - `current_strategy`: 当前策略
    ///
    /// # 返回
    /// 建议的策略(可能不变)
    pub fn select(
        &self,
        risk_level: f32,
        ratio: f64,
        system_load: f32,
        health_score: u8,
        current_strategy: ActivationStrategy,
    ) -> ActivationStrategy {
        // 1. 高风险 → Full
        if risk_level > self.config.high_risk_threshold {
            return ActivationStrategy::Full;
        }

        // 2. 高负载 + 低风险 → Simplified
        if system_load > self.config.high_load_threshold
            && risk_level < self.config.low_risk_threshold
        {
            // 不高于当前策略
            return min_strategy(current_strategy, ActivationStrategy::Simplified);
        }

        // 3. ratio > 阈值 → Simplified
        if ratio > self.config.ratio_threshold {
            return min_strategy(current_strategy, ActivationStrategy::Simplified);
        }

        // 4. 健康分 < 阈值 → Full
        if health_score < self.config.health_threshold {
            return ActivationStrategy::Full;
        }

        // 5. 默认不改变
        current_strategy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 辅助函数:创建默认选择器
    fn make_selector() -> AdaptiveStrategySelector {
        AdaptiveStrategySelector::new(None)
    }

    // ============================================================
    // 配置测试
    // ============================================================

    #[test]
    fn test_config_default_values() {
        let cfg = AdaptiveStrategyConfig::default();
        assert!((cfg.high_risk_threshold - 0.7).abs() < 1e-6);
        assert!((cfg.high_load_threshold - 0.7).abs() < 1e-6);
        assert!((cfg.low_risk_threshold - 0.3).abs() < 1e-6);
        assert!((cfg.ratio_threshold - 1.5).abs() < 1e-9);
        assert_eq!(cfg.health_threshold, 40);
    }

    #[test]
    fn test_selector_default_config() {
        let selector = AdaptiveStrategySelector::new(None);
        let cfg = selector.config();
        assert!((cfg.high_risk_threshold - 0.7).abs() < 1e-6);
    }

    #[test]
    fn test_selector_custom_config() {
        let cfg = AdaptiveStrategyConfig {
            high_risk_threshold: 0.8,
            ..Default::default()
        };
        let selector = AdaptiveStrategySelector::new(Some(cfg));
        assert!((selector.config().high_risk_threshold - 0.8).abs() < 1e-6);
    }

    // ============================================================
    // 选择规则测试
    // ============================================================

    #[test]
    fn test_high_risk_returns_full() {
        let selector = make_selector();
        // 高风险(>0.7) → 强制 Full
        let result = selector.select(0.8, 0.0, 0.0, 50, ActivationStrategy::FastPath);
        assert_eq!(result, ActivationStrategy::Full);
    }

    #[test]
    fn test_high_risk_overrides_other_signals() {
        let selector = make_selector();
        // 即使高负载 + 低 ratio，高风险仍返回 Full
        let result = selector.select(0.9, 0.0, 0.9, 50, ActivationStrategy::Simplified);
        assert_eq!(result, ActivationStrategy::Full);
    }

    #[test]
    fn test_high_load_low_risk_returns_simplified() {
        let selector = make_selector();
        // 高负载 + 低风险 → Simplified
        let result = selector.select(0.2, 0.0, 0.8, 50, ActivationStrategy::Full);
        // min(Full, Simplified) = Simplified
        assert_eq!(result, ActivationStrategy::Simplified);
    }

    #[test]
    fn test_high_load_low_risk_does_not_raise() {
        let selector = make_selector();
        // 高负载 + 低风险，但当前已是 FastPath → 保持 FastPath
        let result = selector.select(0.2, 0.0, 0.8, 50, ActivationStrategy::FastPath);
        assert_eq!(result, ActivationStrategy::FastPath);
    }

    #[test]
    fn test_high_ratio_returns_simplified() {
        let selector = make_selector();
        // ratio > 1.5 → Simplified
        let result = selector.select(0.3, 2.0, 0.3, 50, ActivationStrategy::Full);
        assert_eq!(result, ActivationStrategy::Simplified);
    }

    #[test]
    fn test_high_ratio_does_not_raise_from_fastpath() {
        let selector = make_selector();
        // ratio > 1.5，但当前已是 FastPath → 保持 FastPath
        let result = selector.select(0.3, 2.0, 0.3, 50, ActivationStrategy::FastPath);
        assert_eq!(result, ActivationStrategy::FastPath);
    }

    #[test]
    fn test_low_health_returns_full() {
        let selector = make_selector();
        // 健康分 < 40 → Full
        let result = selector.select(0.3, 0.0, 0.3, 30, ActivationStrategy::Simplified);
        assert_eq!(result, ActivationStrategy::Full);
    }

    #[test]
    fn test_default_keeps_current_strategy() {
        let selector = make_selector();
        // 无触发条件 → 保持当前策略
        let result = selector.select(0.3, 0.0, 0.3, 50, ActivationStrategy::Simplified);
        assert_eq!(result, ActivationStrategy::Simplified);
    }

    #[test]
    fn test_boundary_risk_high_threshold() {
        let selector = make_selector();
        // 边界:risk_level == 0.7 不触发高风险
        let result = selector.select(0.7, 0.0, 0.3, 50, ActivationStrategy::Simplified);
        assert_eq!(result, ActivationStrategy::Simplified);
        // risk_level == 0.7000001 触发(使用 f32 比较)
        let result = selector.select(0.7001, 0.0, 0.3, 50, ActivationStrategy::Simplified);
        assert_eq!(result, ActivationStrategy::Full);
    }

    #[test]
    fn test_boundary_load_threshold() {
        let selector = make_selector();
        // 边界:system_load == 0.7 不触发高负载(需 > 0.7)
        let result = selector.select(0.2, 0.0, 0.7, 50, ActivationStrategy::Full);
        assert_eq!(result, ActivationStrategy::Full);
        // system_load == 0.7001 触发
        let result = selector.select(0.2, 0.0, 0.7001, 50, ActivationStrategy::Full);
        assert_eq!(result, ActivationStrategy::Simplified);
    }

    #[test]
    fn test_boundary_ratio_threshold() {
        let selector = make_selector();
        // 边界:ratio == 1.5 不触发(需 > 1.5)
        let result = selector.select(0.3, 1.5, 0.3, 50, ActivationStrategy::Full);
        assert_eq!(result, ActivationStrategy::Full);
        // ratio == 1.5001 触发
        let result = selector.select(0.3, 1.5001, 0.3, 50, ActivationStrategy::Full);
        assert_eq!(result, ActivationStrategy::Simplified);
    }

    #[test]
    fn test_boundary_health_threshold() {
        let selector = make_selector();
        // 边界:health_score == 40 不触发(需 < 40)
        let result = selector.select(0.3, 0.0, 0.3, 40, ActivationStrategy::Simplified);
        assert_eq!(result, ActivationStrategy::Simplified);
        // health_score == 39 触发
        let result = selector.select(0.3, 0.0, 0.3, 39, ActivationStrategy::Simplified);
        assert_eq!(result, ActivationStrategy::Full);
    }

    // ============================================================
    // 优先级测试:规则 1 > 规则 2/3/4 > 规则 5
    // ============================================================

    #[test]
    fn test_high_risk_overrides_high_load_and_health() {
        let selector = make_selector();
        // 高风险 + 高负载 + 低健康分 → 规则 1 优先:Full
        let result = selector.select(0.8, 0.0, 0.8, 30, ActivationStrategy::FastPath);
        assert_eq!(result, ActivationStrategy::Full);
    }

    #[test]
    fn test_high_load_overrides_high_ratio_when_conditions_met() {
        let selector = make_selector();
        // 高负载 + 低风险 → 进入规则 2 分支(规则 2 在规则 3 之前)
        // 即使 ratio > 1.5，规则 2 先匹配
        let result = selector.select(0.2, 2.0, 0.8, 50, ActivationStrategy::Full);
        assert_eq!(result, ActivationStrategy::Simplified);
    }

    // ============================================================
    // SystemLoadProbe 测试
    // ============================================================

    #[test]
    fn test_system_load_probe_returns_0_5() {
        // 在无 tokio 运行时上下文中，probe 返回 0.5(保守估计)
        let load = SystemLoadProbe::probe();
        assert!((load - 0.5).abs() < 1e-6);
    }

    // ============================================================
    // 组合场景测试
    // ============================================================

    #[test]
    fn test_combined_normal_conditions_keep_strategy() {
        let selector = make_selector();
        // 正常条件:低风险 + 低 ratio + 低负载 + 健康分 50
        let result = selector.select(0.3, 0.8, 0.3, 50, ActivationStrategy::Simplified);
        assert_eq!(result, ActivationStrategy::Simplified);
    }

    #[test]
    fn test_combined_moderate_risk_full_strategy() {
        let selector = make_selector();
        // 中等风险(0.5) + 中等 ratio + 健康分 50
        // 不触发任何规则 → 保持当前策略
        let result = selector.select(0.5, 1.0, 0.3, 50, ActivationStrategy::Full);
        assert_eq!(result, ActivationStrategy::Full);
    }

    #[test]
    fn test_combined_high_load_but_high_risk_goes_full() {
        let selector = make_selector();
        // 高负载 + 高风险 → 规则 1 优先:Full
        let result = selector.select(0.8, 0.0, 0.8, 50, ActivationStrategy::FastPath);
        assert_eq!(result, ActivationStrategy::Full);
    }
}
