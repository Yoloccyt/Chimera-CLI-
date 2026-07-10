//! 在线学习器 — 基于反馈信号的参数更新算法

use crate::types::{FeedbackSignal, ParameterValue};

/// 在线学习器 trait — 定义参数更新接口
///
/// 实现者根据反馈信号计算参数更新量,并应用更新。
pub trait OnlineLearner: Send + Sync {
    /// 根据反馈信号更新参数值
    ///
    /// # 参数
    /// - `current`:当前参数值
    /// - `feedback`:反馈信号
    /// - `learning_rate`:学习率
    ///
    /// # 返回
    /// 更新后的参数值
    fn update(
        &self,
        current: &ParameterValue,
        feedback: FeedbackSignal,
        learning_rate: f32,
    ) -> ParameterValue;
}

/// 梯度下降学习器 — 最简单的在线学习算法
///
/// 更新公式: `new = old + learning_rate * reward`
/// 其中 reward 由 FeedbackSignal.to_reward() 计算。
#[derive(Debug, Clone, Copy, Default)]
pub struct GradientDescent;

impl OnlineLearner for GradientDescent {
    fn update(
        &self,
        current: &ParameterValue,
        feedback: FeedbackSignal,
        learning_rate: f32,
    ) -> ParameterValue {
        let reward = feedback.to_reward();
        match current {
            ParameterValue::Scalar(v) => {
                let new_v = v + learning_rate * reward;
                ParameterValue::Scalar(new_v)
            }
            ParameterValue::Vector(vec) => {
                let new_vec: Vec<f32> = vec.iter().map(|v| v + learning_rate * reward).collect();
                ParameterValue::Vector(new_vec)
            }
            ParameterValue::Matrix { rows, cols, data } => {
                let new_data: Vec<f32> = data.iter().map(|v| v + learning_rate * reward).collect();
                ParameterValue::Matrix {
                    rows: *rows,
                    cols: *cols,
                    data: new_data,
                }
            }
        }
    }
}

/// 指数加权移动平均学习器 — 平滑参数更新
///
/// 更新公式: `new = beta * old + (1 - beta) * target`
/// 其中 target = old + learning_rate * reward
#[derive(Debug, Clone, Copy)]
pub struct EwmaLearner {
    /// 平滑系数 [0.0, 1.0],越大历史权重越高
    pub beta: f32,
}

impl Default for EwmaLearner {
    fn default() -> Self {
        Self { beta: 0.9 }
    }
}

impl OnlineLearner for EwmaLearner {
    fn update(
        &self,
        current: &ParameterValue,
        feedback: FeedbackSignal,
        learning_rate: f32,
    ) -> ParameterValue {
        let gd = GradientDescent;
        let target = gd.update(current, feedback, learning_rate);
        match (current, target) {
            (ParameterValue::Scalar(old), ParameterValue::Scalar(t)) => {
                ParameterValue::Scalar(self.beta * old + (1.0 - self.beta) * t)
            }
            (ParameterValue::Vector(old), ParameterValue::Vector(t)) => {
                let new_vec: Vec<f32> = old
                    .iter()
                    .zip(t.iter())
                    .map(|(o, t)| self.beta * o + (1.0 - self.beta) * t)
                    .collect();
                ParameterValue::Vector(new_vec)
            }
            (
                ParameterValue::Matrix { rows, cols, data },
                ParameterValue::Matrix { data: t, .. },
            ) => {
                let new_data: Vec<f32> = data
                    .iter()
                    .zip(t.iter())
                    .map(|(o, t)| self.beta * o + (1.0 - self.beta) * t)
                    .collect();
                ParameterValue::Matrix {
                    rows: *rows,
                    cols: *cols,
                    data: new_data,
                }
            }
            _ => current.clone(), // 类型不匹配时保持原值
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gradient_descent_scalar() {
        let learner = GradientDescent;
        let current = ParameterValue::scalar(0.5);
        let updated = learner.update(&current, FeedbackSignal::Success, 0.1);
        // 0.5 + 0.1 * 1.0 = 0.6
        assert!((updated.as_scalar().unwrap() - 0.6).abs() < 1e-5);
    }

    #[test]
    fn test_gradient_descent_failure() {
        let learner = GradientDescent;
        let current = ParameterValue::scalar(0.5);
        let updated = learner.update(&current, FeedbackSignal::Failure, 0.1);
        // 0.5 + 0.1 * (-1.0) = 0.4
        assert!((updated.as_scalar().unwrap() - 0.4).abs() < 1e-5);
    }

    #[test]
    fn test_ewma_scalar() {
        let learner = EwmaLearner { beta: 0.9 };
        let current = ParameterValue::scalar(0.5);
        let updated = learner.update(&current, FeedbackSignal::Success, 0.1);
        // target = 0.6, new = 0.9 * 0.5 + 0.1 * 0.6 = 0.51
        assert!((updated.as_scalar().unwrap() - 0.51).abs() < 1e-5);
    }
}
