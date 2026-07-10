//! SSRA 自适应策略选择 — 基于历史融合成功率自动选择最优策略
//!
//! 对应架构层:L7 Execution
//! 对应创新点:P2-6 GLM slime快速融合优化
//!
//! # 核心机制
//! - 跟踪三种策略(WeightedAverage/TopK/MeanField)的历史成功率
//! - 基于EWMA(指数加权移动平均)更新各策略评分
//! - 自动选择评分最高的策略作为主导策略
//! - 支持探索-利用平衡(ε-贪心)

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::types::FusionStrategy;

/// 策略性能记录
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StrategyPerformance {
    /// 策略类型
    strategy: FusionStrategy,
    /// EWMA评分(成功率)
    ewma_score: f32,
    /// 使用次数
    usage_count: u32,
    /// 成功次数
    success_count: u32,
}

impl StrategyPerformance {
    fn new(strategy: FusionStrategy) -> Self {
        Self {
            strategy,
            ewma_score: 0.5, // 初始中性评分
            usage_count: 0,
            success_count: 0,
        }
    }

    /// 更新性能记录
    fn update(&mut self, success: bool, beta: f32) {
        self.usage_count += 1;
        if success {
            self.success_count += 1;
        }
        let reward = if success { 1.0 } else { 0.0 };
        self.ewma_score = beta * self.ewma_score + (1.0 - beta) * reward;
    }
}

/// 自适应策略选择器
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveStrategySelector {
    /// 各策略性能记录
    performances: HashMap<String, StrategyPerformance>,
    /// EWMA平滑系数
    beta: f32,
    /// 探索概率(ε-贪心)
    epsilon: f32,
    /// 默认策略
    default_strategy: FusionStrategy,
}

impl Default for AdaptiveStrategySelector {
    fn default() -> Self {
        let mut performances = HashMap::new();
        performances.insert(
            "WeightedAverage".to_string(),
            StrategyPerformance::new(FusionStrategy::WeightedAverage),
        );
        performances.insert(
            "TopK".to_string(),
            StrategyPerformance::new(FusionStrategy::TopK),
        );
        performances.insert(
            "MeanField".to_string(),
            StrategyPerformance::new(FusionStrategy::MeanField),
        );

        Self {
            performances,
            beta: 0.9,
            epsilon: 0.1,
            default_strategy: FusionStrategy::TopK,
        }
    }
}

impl AdaptiveStrategySelector {
    /// 创建新的选择器
    pub fn new(beta: f32, epsilon: f32) -> Self {
        let mut s = Self::default();
        s.beta = beta.clamp(0.0, 1.0);
        s.epsilon = epsilon.clamp(0.0, 1.0);
        s
    }

    /// 选择策略(ε-贪心)
    pub fn select(&self) -> FusionStrategy {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        // ε概率探索:随机选择
        if rng.gen::<f32>() < self.epsilon {
            let strategies = [
                FusionStrategy::WeightedAverage,
                FusionStrategy::TopK,
                FusionStrategy::MeanField,
            ];
            let idx = rng.gen_range(0..strategies.len());
            return strategies[idx];
        }

        // 1-ε概率利用:选择评分最高
        self.exploit()
    }

    /// 纯利用:选择当前评分最高的策略
    fn exploit(&self) -> FusionStrategy {
        let best = self
            .performances
            .values()
            .max_by(|a, b| a.ewma_score.partial_cmp(&b.ewma_score).unwrap_or(std::cmp::Ordering::Equal));

        match best {
            Some(p) => p.strategy,
            None => self.default_strategy,
        }
    }

    /// 报告策略执行结果
    pub fn report(&mut self, strategy: FusionStrategy, success: bool) {
        let key = format!("{strategy:?}");
        if let Some(perf) = self.performances.get_mut(&key) {
            perf.update(success, self.beta);
        }
    }

    /// 获取策略评分
    pub fn score(&self, strategy: FusionStrategy) -> f32 {
        let key = format!("{strategy:?}");
        self.performances
            .get(&key)
            .map(|p| p.ewma_score)
            .unwrap_or(0.0)
    }

    /// 获取最佳策略
    pub fn best_strategy(&self) -> FusionStrategy {
        self.exploit()
    }

    /// 获取所有策略评分
    pub fn all_scores(&self) -> Vec<(FusionStrategy, f32)> {
        self.performances
            .values()
            .map(|p| (p.strategy, p.ewma_score))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selector_default() {
        let selector = AdaptiveStrategySelector::default();
        let scores = selector.all_scores();
        assert_eq!(scores.len(), 3);
        // 初始评分都应为0.5
        for (_, score) in &scores {
            assert!((score - 0.5).abs() < 1e-5);
        }
    }

    #[test]
    fn test_report_updates_score() {
        let mut selector = AdaptiveStrategySelector::default();
        let strategy = FusionStrategy::TopK;

        // 多次成功报告
        for _ in 0..10 {
            selector.report(strategy, true);
        }

        let score = selector.score(strategy);
        assert!(score > 0.5, "成功报告后评分应上升");
    }

    #[test]
    fn test_exploit_selects_best() {
        let mut selector = AdaptiveStrategySelector::new(0.5, 0.0); // epsilon=0,纯利用

        // TopK 多次成功,WeightedAverage 多次失败
        for _ in 0..10 {
            selector.report(FusionStrategy::TopK, true);
            selector.report(FusionStrategy::WeightedAverage, false);
        }

        let best = selector.best_strategy();
        assert_eq!(best, FusionStrategy::TopK);
    }

    #[test]
    fn test_report_failure_lowers_score() {
        let mut selector = AdaptiveStrategySelector::default();
        let strategy = FusionStrategy::MeanField;

        // 多次失败
        for _ in 0..10 {
            selector.report(strategy, false);
        }

        let score = selector.score(strategy);
        assert!(score < 0.5, "失败报告后评分应下降");
    }
}
