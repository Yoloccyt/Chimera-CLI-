//! 委托开销预测模型 — EWMA 自适应预测器
//!
//! 对应架构层:L8 Parliament
//! 对应 spec:l8-parliament-deep-optimization-round3 Task 4
//!
//! # 设计原则
//! - EWMA 平滑:平滑因子 α=0.3，快速响应工作负载变化
//! - 冷启动默认值:无历史数据时返回 500ms
//! - 预算跟踪:DelegationBudget 跟踪单次 Quest 的委托预算
//! - 线程安全:所有操作 Send + Sync

/// 委托开销预测器 — 基于 EWMA 的预测模型
///
/// # 公式
/// ```
/// prediction = α × actual + (1-α) × previous_prediction
/// ```
/// 其中 α=0.3 适用于快速变化的工作负载。
///
/// # 冷启动
/// 首次预测时返回默认值 500ms，随数据积累逐步收敛到真实值。
#[derive(Debug, Clone)]
pub struct DelegationOverheadPredictor {
    /// 当前预测值(ms)
    current_prediction: f64,
    /// 平滑因子 α (0.0, 1.0]
    alpha: f64,
    /// 样本数量(用于冷启动检测)
    sample_count: u64,
    /// 默认值(冷启动时返回)
    default_value: f64,
}

impl DelegationOverheadPredictor {
    /// 创建新的预测器
    ///
    /// # 参数
    /// - `alpha`: 平滑因子，默认 0.3
    /// - `default_value`: 冷启动默认值(ms)，默认 500.0
    pub fn new(alpha: Option<f64>, default_value: Option<f64>) -> Self {
        Self {
            current_prediction: default_value.unwrap_or(500.0),
            alpha: alpha.unwrap_or(0.3).clamp(0.01, 1.0),
            sample_count: 0,
            default_value: default_value.unwrap_or(500.0),
        }
    }

    /// 预测下一次委托开销
    ///
    /// # 参数
    /// - `fan_out`: 委托扇出数(委托给多少个 Agent)
    ///
    /// # 返回
    /// 预测开销(ms)，冷启动时返回默认值
    pub fn predict(&self, fan_out: usize) -> f64 {
        if self.sample_count == 0 {
            return self.default_value * (1.0 + 0.1 * fan_out as f64);
        }
        self.current_prediction * (1.0 + 0.05 * fan_out as f64)
    }

    /// 记录实际开销，更新 EWMA 预测
    ///
    /// # 参数
    /// - `actual`: 实际开销(ms)
    pub fn record(&mut self, actual: f64) {
        if self.sample_count == 0 {
            self.current_prediction = actual;
        } else {
            self.current_prediction =
                self.alpha * actual + (1.0 - self.alpha) * self.current_prediction;
        }
        self.sample_count += 1;
    }

    /// 获取当前预测值
    pub fn current_prediction(&self) -> f64 {
        self.current_prediction
    }

    /// 获取样本数量
    pub fn sample_count(&self) -> u64 {
        self.sample_count
    }

    /// 预测误差百分比(相对于实际值)
    ///
    /// # 返回
    /// 误差百分比 [0.0, 1.0]
    pub fn prediction_error(&self, actual: f64) -> f64 {
        if self.current_prediction == 0.0 {
            return if actual == 0.0 { 0.0 } else { 1.0 };
        }
        ((actual - self.current_prediction).abs() / self.current_prediction).min(1.0)
    }
}

/// 委托预算 — 跟踪单次 Quest 的委托开销预算
///
/// # 设计
/// 预算基于 EWMA 预测值 + 20% 缓冲，超限时返回错误。
/// WHY 20% 缓冲:避免瞬时波动触发误报，同时保持预算约束的有效性。
#[derive(Debug, Clone)]
pub struct DelegationBudget {
    /// 预算上限(ms)
    budget_ms: f64,
    /// 已使用的预算(ms)
    used_ms: f64,
    /// 是否已超限
    exceeded: bool,
}

impl DelegationBudget {
    /// 创建新的委托预算
    ///
    /// # 参数
    /// - `budget_ms`: 预算上限(ms)
    pub fn new(budget_ms: f64) -> Self {
        Self {
            budget_ms: budget_ms.max(0.0),
            used_ms: 0.0,
            exceeded: false,
        }
    }

    /// 从预测器创建预算
    ///
    /// 预算 = 预测值 × (1 + 缓冲系数)
    pub fn from_predictor(predictor: &DelegationOverheadPredictor, fan_out: usize) -> Self {
        let predicted = predictor.predict(fan_out);
        // 缓冲 20% 避免瞬时波动误报
        let budget = predicted * 1.2;
        Self::new(budget)
    }

    /// 记录开销并检查预算
    ///
    /// # 返回
    /// - `Ok(())`: 预算未超限
    /// - `Err(DelegationBudgetExceeded)`: 预算已超限
    pub fn record(&mut self, cost_ms: f64) -> Result<(), DelegationBudgetExceeded> {
        self.used_ms += cost_ms;
        if self.used_ms > self.budget_ms && !self.exceeded {
            self.exceeded = true;
            return Err(DelegationBudgetExceeded {
                budget_ms: self.budget_ms,
                used_ms: self.used_ms,
                exceeded_by_ms: self.used_ms - self.budget_ms,
            });
        }
        Ok(())
    }

    /// 获取预算上限
    pub fn budget_ms(&self) -> f64 {
        self.budget_ms
    }

    /// 获取已使用预算
    pub fn used_ms(&self) -> f64 {
        self.used_ms
    }

    /// 是否已超限
    pub fn is_exceeded(&self) -> bool {
        self.exceeded
    }

    /// 剩余预算
    pub fn remaining_ms(&self) -> f64 {
        (self.budget_ms - self.used_ms).max(0.0)
    }
}

/// 委托预算超限错误
#[derive(Debug, Clone, thiserror::Error)]
#[error("委托预算超限: 预算 {budget_ms:.1}ms, 已使用 {used_ms:.1}ms, 超出 {exceeded_by_ms:.1}ms")]
pub struct DelegationBudgetExceeded {
    /// 预算上限(ms)
    pub budget_ms: f64,
    /// 已使用预算(ms)
    pub used_ms: f64,
    /// 超出量(ms)
    pub exceeded_by_ms: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_predictor_cold_start() {
        let predictor = DelegationOverheadPredictor::new(None, None);
        assert_eq!(predictor.sample_count(), 0);
        // 冷启动返回默认值+扇出调整
        let prediction = predictor.predict(1);
        assert!(prediction > 0.0);
    }

    #[test]
    fn test_predictor_record_updates_prediction() {
        let mut predictor = DelegationOverheadPredictor::new(Some(0.5), Some(100.0));
        predictor.record(200.0);
        // 新预测 = 0.5 * 200 + 0.5 * 100 = 150
        assert!((predictor.current_prediction() - 150.0).abs() < 1e-9);
        assert_eq!(predictor.sample_count(), 1);
    }

    #[test]
    fn test_predictor_ewma_convergence() {
        let mut predictor = DelegationOverheadPredictor::new(Some(0.3), Some(100.0));
        // 多次记录真实值 200ms，预测应收敛到 200
        for _ in 0..20 {
            predictor.record(200.0);
        }
        // 20 次后预测应接近 200
        assert!((predictor.current_prediction() - 200.0).abs() < 5.0);
    }

    #[test]
    fn test_predictor_error_within_twenty_percent() {
        let mut predictor = DelegationOverheadPredictor::new(Some(0.3), Some(100.0));
        for _ in 0..10 {
            predictor.record(150.0);
        }
        // 预测应接近 150，误差 < 20%
        let error = predictor.prediction_error(150.0);
        assert!(error < 0.2, "预测误差应 < 20%, 实际: {:.2}%", error * 100.0);
    }

    #[test]
    fn test_budget_allows_normal_usage() {
        let mut budget = DelegationBudget::new(100.0);
        assert!(budget.record(30.0).is_ok());
        assert!(budget.record(50.0).is_ok());
        assert!(!budget.is_exceeded());
    }

    #[test]
    fn test_budget_exceeded_returns_error() {
        let mut budget = DelegationBudget::new(100.0);
        assert!(budget.record(60.0).is_ok());
        let err = budget.record(50.0).unwrap_err();
        assert!(err.exceeded_by_ms > 0.0);
        assert!(budget.is_exceeded());
    }

    #[test]
    fn test_budget_from_predictor() {
        let predictor = DelegationOverheadPredictor::new(Some(0.3), Some(100.0));
        let budget = DelegationBudget::from_predictor(&predictor, 2);
        // 预测 = 100 * (1 + 0.05*2) = 110, 预算 = 110 * 1.2 = 132
        assert!(budget.budget_ms() > 100.0);
    }

    #[test]
    fn test_budget_remaining() {
        let mut budget = DelegationBudget::new(100.0);
        assert!((budget.remaining_ms() - 100.0).abs() < 1e-9);
        let _ = budget.record(30.0);
        assert!((budget.remaining_ms() - 70.0).abs() < 1e-9);
    }
}