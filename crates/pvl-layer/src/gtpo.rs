//! GTPO：Group Turn Policy Optimization（Milestone D-2c，设计 §11.1 目标形态）
//!
//! Turn-Level 奖励：折扣回报 `G_t = r_t + γ·G_{t+1}` + 归一化优势
//! `(G_t − mean) / (std + 1e-8)`。
//!
//! R2 冻结（ADR-042）+ ADR-049 降级裁决：纯函数计算（无参数学习），
//! 优势输出供规则式策略消费；解冻后由训练面消费同一接口。

/// Turn 级轨迹（奖励序列；设计 §11.1 `Trajectory.rewards` 目标形态）
#[derive(Debug, Clone, PartialEq)]
pub struct TurnTrajectory {
    /// 每 turn 的奖励
    pub rewards: Vec<f32>,
}

impl TurnTrajectory {
    /// 轨迹长度（turn 数）
    pub fn len(&self) -> usize {
        self.rewards.len()
    }

    /// 是否空轨迹
    pub fn is_empty(&self) -> bool {
        self.rewards.is_empty()
    }
}

/// GTPO：Group Turn Policy Optimization（设计 §11.1）
#[derive(Debug, Clone, Copy)]
pub struct GTPO {
    /// 折扣因子 γ ∈ [0,1]
    gamma: f32,
}

impl GTPO {
    /// 构造：折扣因子（通常 0.9~0.99）
    pub fn new(gamma: f32) -> Self {
        Self {
            gamma: gamma.clamp(0.0, 1.0),
        }
    }

    /// 计算 Turn-Level 优势（设计 §11.1 `compute_advantages`）
    ///
    /// 1. 后向折扣回报：`G_t = r_t + γ·G_{t+1}`
    /// 2. 归一化：`(G_t − mean) / (std + 1e-8)`
    ///
    /// 空轨迹 → 空向量；单元素 → [0.0]（无方差信息）；恒定回报 → 全 0。
    pub fn compute_advantages(&self, trajectory: &TurnTrajectory) -> Vec<f32> {
        let n = trajectory.len();
        if n == 0 {
            return Vec::new();
        }
        // 折扣回报（f64 中间精度，避免长轨迹 f32 累加漂移）
        let mut returns = vec![0.0f64; n];
        let mut g = 0.0f64;
        for t in (0..n).rev() {
            g = f64::from(trajectory.rewards[t]) + f64::from(self.gamma) * g;
            returns[t] = g;
        }
        if n == 1 {
            return vec![0.0];
        }
        let mean = returns.iter().sum::<f64>() / n as f64;
        let variance = returns.iter().map(|r| (r - mean) * (r - mean)).sum::<f64>() / n as f64;
        let std = variance.sqrt() as f32;
        returns
            .iter()
            .map(|r| ((*r as f32) - mean as f32) / (std + 1e-8))
            .collect()
    }
}
