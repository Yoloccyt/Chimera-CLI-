//! 在线学习框架类型定义

use serde::{Deserialize, Serialize};

/// 参数值 — 支持标量、向量、矩阵三种形式
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ParameterValue {
    /// 标量值
    Scalar(f32),
    /// 向量值
    Vector(Vec<f32>),
    /// 矩阵值(行优先展平)
    Matrix {
        /// 行数
        rows: usize,
        /// 列数
        cols: usize,
        /// 行优先展平数据
        data: Vec<f32>,
    },
}

impl ParameterValue {
    /// 创建标量参数
    pub fn scalar(v: f32) -> Self {
        Self::Scalar(v)
    }

    /// 创建向量参数
    pub fn vector(v: Vec<f32>) -> Self {
        Self::Vector(v)
    }

    /// 创建矩阵参数
    pub fn matrix(rows: usize, cols: usize, data: Vec<f32>) -> Self {
        Self::Matrix { rows, cols, data }
    }

    /// 获取标量值(非标量返回None)
    pub fn as_scalar(&self) -> Option<f32> {
        match self {
            Self::Scalar(v) => Some(*v),
            _ => None,
        }
    }

    /// 获取向量值(非向量返回None)
    pub fn as_vector(&self) -> Option<&Vec<f32>> {
        match self {
            Self::Vector(v) => Some(v),
            _ => None,
        }
    }

    /// 将参数值转换为f32(标量直接返回,向量取平均,矩阵取平均)
    pub fn to_f32(&self) -> f32 {
        match self {
            Self::Scalar(v) => *v,
            Self::Vector(v) => {
                if v.is_empty() {
                    0.0
                } else {
                    v.iter().sum::<f32>() / v.len() as f32
                }
            }
            Self::Matrix { data, .. } => {
                if data.is_empty() {
                    0.0
                } else {
                    data.iter().sum::<f32>() / data.len() as f32
                }
            }
        }
    }

    /// 参数维度数(标量=1,向量=长度,矩阵=行×列)
    pub fn dimension(&self) -> usize {
        match self {
            Self::Scalar(_) => 1,
            Self::Vector(v) => v.len(),
            Self::Matrix { rows, cols, .. } => rows * cols,
        }
    }
}

/// 可学习参数 — 携带元数据与当前值
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LearnableParameter {
    /// 参数ID(全局唯一,如"osa_sparsity_base")
    pub id: String,
    /// 参数名称(人类可读)
    pub name: String,
    /// 所属crate(如"osa-coordinator")
    pub crate_name: String,
    /// 当前值
    pub value: ParameterValue,
    /// 默认值(用于重置)
    pub default_value: ParameterValue,
    /// 学习率
    pub learning_rate: f32,
    /// 最小值约束
    pub min_value: Option<f32>,
    /// 最大值约束
    pub max_value: Option<f32>,
    /// 更新次数
    pub update_count: u64,
    /// 最后更新时间戳(UTC ISO 8601)
    pub last_updated: String,
}

impl LearnableParameter {
    /// 创建新的可学习参数
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        crate_name: impl Into<String>,
        value: ParameterValue,
    ) -> Self {
        let id = id.into();
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            name: name.into(),
            crate_name: crate_name.into(),
            default_value: value.clone(),
            value,
            learning_rate: 0.01,
            min_value: None,
            max_value: None,
            update_count: 0,
            last_updated: now,
            id,
        }
    }

    /// 设置学习率
    pub fn with_learning_rate(mut self, lr: f32) -> Self {
        self.learning_rate = lr;
        self
    }

    /// 设置值范围约束
    pub fn with_bounds(mut self, min: f32, max: f32) -> Self {
        self.min_value = Some(min);
        self.max_value = Some(max);
        self
    }

    /// 应用值约束(clamp)
    pub fn clamp_value(&mut self) {
        if let Some(min) = self.min_value {
            if let Some(v) = self.value.as_scalar() {
                self.value = ParameterValue::Scalar(v.max(min));
            }
        }
        if let Some(max) = self.max_value {
            if let Some(v) = self.value.as_scalar() {
                self.value = ParameterValue::Scalar(v.min(max));
            }
        }
    }
}

/// 反馈信号 — 用于驱动参数更新
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FeedbackSignal {
    /// 任务成功完成
    Success,
    /// 任务失败
    Failure,
    /// 延迟反馈(毫秒)
    Latency(u64),
    /// 资源消耗反馈(token数/内存等)
    ResourceUsage(u64),
    /// 自定义评分 [-1.0, 1.0],正数表示正向反馈
    CustomScore(f32),
}

impl FeedbackSignal {
    /// 将反馈信号转换为标准化奖励值 [-1.0, 1.0]
    pub fn to_reward(&self) -> f32 {
        match self {
            Self::Success => 1.0,
            Self::Failure => -1.0,
            Self::Latency(ms) => {
                // 延迟越低越好,<100ms为满分,>5000ms为0分
                let score = 1.0 - (*ms as f32 / 5000.0).min(1.0);
                score * 2.0 - 1.0 // 映射到[-1,1]
            }
            Self::ResourceUsage(tokens) => {
                // 资源消耗越低越好
                let score = 1.0 - (*tokens as f32 / 10000.0).min(1.0);
                score * 2.0 - 1.0
            }
            Self::CustomScore(s) => s.clamp(-1.0, 1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parameter_value_scalar() {
        let v = ParameterValue::scalar(0.5);
        assert_eq!(v.as_scalar(), Some(0.5));
        assert_eq!(v.to_f32(), 0.5);
        assert_eq!(v.dimension(), 1);
    }

    #[test]
    fn test_parameter_value_vector() {
        let v = ParameterValue::vector(vec![0.1, 0.2, 0.3]);
        assert_eq!(v.as_scalar(), None);
        assert!((v.to_f32() - 0.2).abs() < 1e-5);
        assert_eq!(v.dimension(), 3);
    }

    #[test]
    fn test_feedback_signal_reward() {
        assert_eq!(FeedbackSignal::Success.to_reward(), 1.0);
        assert_eq!(FeedbackSignal::Failure.to_reward(), -1.0);
        assert!(FeedbackSignal::Latency(100).to_reward() > 0.9);
        assert!(FeedbackSignal::Latency(5000).to_reward() < -0.9);
    }

    #[test]
    fn test_learnable_parameter_clamp() {
        let mut p =
            LearnableParameter::new("test", "Test", "test-crate", ParameterValue::scalar(1.5))
                .with_bounds(0.0, 1.0);
        p.clamp_value();
        assert_eq!(p.value.as_scalar(), Some(1.0));
    }
}
