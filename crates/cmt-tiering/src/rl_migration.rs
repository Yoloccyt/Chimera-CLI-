//! DQN 驱动的记忆迁移决策（Milestone D-2a，设计 §6.4 目标形态）
//!
//! 预测式冷热迁移：根据访问频率/时效特征决策记忆应驻留的温度层。
//!
//! R2 冻结（ADR-042）+ ADR-049 降级裁决（RL 组件降级为规则/统计驱动）：
//! - Q 网络 = **规则式线性权重**（专家先验注入，非神经网络，无反向传播）
//! - `record` 仅追加回放记录，**不做梯度更新**——训练面占位，
//!   解冻后替换为 TD 误差 + Q 权重更新（设计 §6.4 `train`），不得破坏接口契约
//!
//! 依赖方向：L3 cmt-tiering 内部模块（0 新增 crate；rand 复用既有依赖）；
//! 温度层复用 `crate::types::Tier`（Hot/Warm/Cold/Ice，单一事实源）。

use crate::types::Tier;
use rand::Rng;
use std::collections::VecDeque;

/// 四温度层动作空间（`Tier` 顺序常量，对应 Q 权重索引）
pub const ALL_TIERS: [Tier; 4] = [Tier::Hot, Tier::Warm, Tier::Cold, Tier::Ice];

/// 迁移状态特征（设计 §6.4 `MigrationState`）
#[derive(Debug, Clone, PartialEq)]
pub struct MigrationState {
    /// 记忆块标识
    pub chunk_id: String,
    /// 1 分钟窗口访问次数
    pub access_frequency_1m: u32,
    /// 10 分钟窗口访问次数
    pub access_frequency_10m: u32,
    /// 1 小时窗口访问次数
    pub access_frequency_1h: u32,
    /// 距上次访问的毫秒数
    pub last_access_age_ms: u64,
}

impl MigrationState {
    /// 归一化特征向量 `[freq_1m, freq_10m, freq_1h, age]`，各 ∈ [0,1]
    ///
    /// WHY 归一化：Q 打分 = 权重 × 特征，尺度一致避免某特征主导。
    /// 上限为经验值（100/1K/10K 次、1 天年龄），超出即饱和为 1.0。
    pub fn features(&self) -> [f32; 4] {
        [
            (self.access_frequency_1m.min(100) as f32) / 100.0,
            (self.access_frequency_10m.min(1_000) as f32) / 1_000.0,
            (self.access_frequency_1h.min(10_000) as f32) / 10_000.0,
            (self.last_access_age_ms.min(86_400_000) as f32) / 86_400_000.0,
        ]
    }
}

/// 迁移经验（回放记录；R2 冻结下仅记录不训练）
#[derive(Debug, Clone, PartialEq)]
pub struct MigrationExperience {
    /// 决策时状态
    pub state: MigrationState,
    /// 决策层
    pub tier: Tier,
    /// 延迟反馈奖励（迁移后观察到的访问收益）
    pub reward: f32,
    /// 下一状态（迁移后的访问特征；None = 终止）
    pub next_state: Option<MigrationState>,
}

/// DQN 记忆迁移策略（规则式占位）
///
/// `q_weights[i]` = 第 i 层（`ALL_TIERS` 顺序）的 4 维特征权重；
/// Q 值 = 权重 · 特征，决策 = ε-贪婪 argmax。
#[derive(Debug, Clone)]
pub struct DQNMigrationPolicy {
    q_weights: Vec<[f32; 4]>,
    epsilon: f32,
    replay: VecDeque<MigrationExperience>,
    replay_limit: usize,
}

impl DQNMigrationPolicy {
    /// 构造策略；`q_weights` 必须恰好 4 组（Hot/Warm/Cold/Ice 顺序）
    ///
    /// # Panics
    /// `q_weights.len() != 4` 时 panic（构造期契约，非运行时边界）。
    pub fn new(q_weights: Vec<[f32; 4]>, epsilon: f32, replay_limit: usize) -> Self {
        assert_eq!(
            q_weights.len(),
            ALL_TIERS.len(),
            "Q 权重必须恰好 4 组（四层动作空间）"
        );
        Self {
            q_weights,
            epsilon: epsilon.clamp(0.0, 1.0),
            replay: VecDeque::with_capacity(replay_limit.min(1)),
            replay_limit,
        }
    }

    /// 决策目标层：ε-贪婪（随机探索 vs Q 值 argmax）
    pub fn decide_tier(&self, state: &MigrationState) -> Tier {
        let mut rng = rand::thread_rng();
        if rng.gen::<f32>() < self.epsilon {
            let idx = rng.gen_range(0..ALL_TIERS.len());
            ALL_TIERS[idx]
        } else {
            let features = state.features();
            let mut best = 0usize;
            let mut best_q = f32::NEG_INFINITY;
            for (i, w) in self.q_weights.iter().enumerate() {
                let q = w
                    .iter()
                    .zip(features.iter())
                    .map(|(wi, fi)| wi * fi)
                    .sum::<f32>();
                if q > best_q {
                    best_q = q;
                    best = i;
                }
            }
            ALL_TIERS[best]
        }
    }

    /// 记录迁移经验（R2 冻结：仅回放追加，不做梯度更新）
    ///
    /// 超出容量按 FIFO 淘汰最旧，保持内存有界。
    pub fn record(&mut self, exp: MigrationExperience) {
        if self.replay.len() >= self.replay_limit {
            self.replay.pop_front();
        }
        self.replay.push_back(exp);
    }

    /// 当前回放记录数
    pub fn replay_len(&self) -> usize {
        self.replay.len()
    }

    /// 回放中全部奖励（测试/审计用）
    pub fn replay_rewards(&self) -> Vec<f32> {
        self.replay.iter().map(|e| e.reward).collect()
    }
}
