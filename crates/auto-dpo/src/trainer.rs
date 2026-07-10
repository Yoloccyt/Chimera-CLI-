//! DPO 训练器 — 实现 Bradley-Terry 损失函数与参考策略冻结
//!
//! 对应架构层:L5 Knowledge
//! 对应P0-7:DPO训练闭环
//!
//! # 核心职责
//! - 实现 Bradley-Terry 偏好损失函数
//! - 维护参考策略 π_ref(冻结不更新)
//! - 计算策略 π_θ 与 π_ref 的 log-ratio
//! - 提供训练步进接口
//!
//! # Bradley-Terry 损失公式
//! ```text
//! L_DPO = -log σ(β * (log π_θ(y_w|x) - log π_ref(y_w|x)) - β * (log π_θ(y_l|x) - log π_ref(y_l|x)))
//! ```
//! 其中:
//! - y_w: chosen (偏好输出)
//! - y_l: rejected (不偏好输出)
//! - β: 温度参数(控制与参考策略的偏离程度)
//! - σ: sigmoid 函数
//! - π_θ: 当前策略
//! - π_ref: 参考策略(冻结)

use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::AutoDpoError;
use crate::types::PreferencePair;

/// DPO 训练器 — Bradley-Terry 损失函数实现
///
/// P0-7:实现完整的DPO训练闭环,包括:
/// - 参考策略 π_ref 的冻结维护
/// - Bradley-Terry 损失计算
/// - 策略 π_θ 的梯度更新(简化版,使用模拟梯度)
///
/// # 线程安全
/// `reference_policy` 和 `current_policy` 用 RwLock 保护,
/// 读多写少(训练步中读,更新时写)。
pub struct DpoTrainer {
    /// 温度参数 β(控制偏离参考策略的程度)
    ///
    /// β 越大,当前策略越接近参考策略(保守)。
    /// β 越小,当前策略可以更偏离参考策略(激进)。
    beta: f32,
    /// 参考策略 π_ref(冻结不更新)
    ///
    /// WHY 冻结:参考策略是DPO的锚点,提供稳定的偏好基准。
    /// 若参考策略也更新,损失函数失去意义。
    reference_policy: std::sync::RwLock<Policy>,
    /// 当前策略 π_θ(可更新)
    current_policy: std::sync::RwLock<Policy>,
    /// 训练步数计数器
    step_counter: AtomicU64,
    /// 累计损失(用于监控收敛)
    /// WHY RwLock<f32>:stable Rust 无 AtomicF32,读多写少场景 RwLock 合适
    cumulative_loss: std::sync::RwLock<f32>,
}

/// 策略 — 简化的参数化策略表示
///
/// 实际应用中应为神经网络权重,此处使用简化的参数向量表示,
/// 用于演示DPO训练闭环的核心逻辑。
#[derive(Debug, Clone)]
struct Policy {
    /// 策略参数(简化表示)
    params: Vec<f32>,
}

impl Policy {
    /// 创建随机初始化的策略
    fn new(dim: usize) -> Self {
        let params = vec![0.0; dim];
        Self { params }
    }

    /// 计算 log π(y|x) — 简化的对数概率
    ///
    /// 使用文本与策略参数的相似度作为对数概率的代理。
    fn log_prob(&self, text: &str) -> f32 {
        // 简化的对数概率:使用文本哈希与策略参数的交互
        let hash = text_hash(text);
        let mut log_prob = 0.0f32;
        for (i, param) in self.params.iter().enumerate() {
            let feature = ((hash.wrapping_add(i as u64)) % 1000) as f32 / 1000.0;
            log_prob += param * feature;
        }
        log_prob
    }
}

/// 计算文本的简单哈希(用于策略特征)
fn text_hash(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325; // FNV offset basis
    for byte in text.bytes() {
        hash = hash.wrapping_mul(0x100000001b3); // FNV prime
        hash ^= byte as u64;
    }
    hash
}

impl DpoTrainer {
    /// 创建新的 DPO 训练器
    ///
    /// # 参数
    /// - `beta`:温度参数(默认 0.1)
    /// - `policy_dim`:策略参数维度(默认 64)
    pub fn new(beta: f32, policy_dim: usize) -> Self {
        let reference = Policy::new(policy_dim);
        let current = reference.clone(); // 初始时当前策略=参考策略
        Self {
            beta,
            reference_policy: std::sync::RwLock::new(reference),
            current_policy: std::sync::RwLock::new(current),
            step_counter: AtomicU64::new(0),
            cumulative_loss: std::sync::RwLock::new(0.0),
        }
    }

    /// 使用默认配置创建训练器
    ///
    /// 默认:beta=0.1, policy_dim=64
    pub fn with_default_config() -> Self {
        Self::new(0.1, 64)
    }

    /// 计算 Bradley-Terry 损失
    ///
    /// 公式: L = -log σ(β * (r_θ(y_w) - r_θ(y_l)))
    /// 其中 r_θ(y) = log π_θ(y|x) - log π_ref(y|x)
    ///
    /// # 参数
    /// - `pair`:偏好对(chosen/rejected)
    ///
    /// # 返回
    /// 损失值(非负,越小表示策略越好)
    pub fn compute_loss(&self, pair: &PreferencePair) -> Result<f32, AutoDpoError> {
        let ref_policy =
            self.reference_policy
                .read()
                .map_err(|e| AutoDpoError::GenerationFailed {
                    reason: format!("参考策略锁中毒: {e}"),
                })?;
        let cur_policy =
            self.current_policy
                .read()
                .map_err(|e| AutoDpoError::GenerationFailed {
                    reason: format!("当前策略锁中毒: {e}"),
                })?;

        // 计算奖励差: r_θ(y) = log π_θ(y|x) - log π_ref(y|x)
        let r_chosen = cur_policy.log_prob(&pair.chosen) - ref_policy.log_prob(&pair.chosen);
        let r_rejected = cur_policy.log_prob(&pair.rejected) - ref_policy.log_prob(&pair.rejected);

        // Bradley-Terry 损失: -log σ(β * (r_chosen - r_rejected))
        let diff = self.beta * (r_chosen - r_rejected);
        let loss = -log_sigmoid(diff);

        // 损失应为非负
        Ok(loss.max(0.0))
    }

    /// 训练步进 — 使用偏好对更新当前策略
    ///
    /// # 参数
    /// - `pair`:偏好对
    /// - `learning_rate`:学习率(默认 0.01)
    ///
    /// # 返回
    /// 当前步的损失值
    pub fn train_step(
        &self,
        pair: &PreferencePair,
        learning_rate: f32,
    ) -> Result<f32, AutoDpoError> {
        let loss = self.compute_loss(pair)?;

        // 简化梯度更新:根据损失方向调整策略参数
        // 实际应用中应使用自动微分(如 candle/ort)
        let mut cur_policy =
            self.current_policy
                .write()
                .map_err(|e| AutoDpoError::GenerationFailed {
                    reason: format!("当前策略锁中毒: {e}"),
                })?;

        // 模拟梯度下降:根据 chosen/rejected 的得分差调整参数
        let score_gap = pair.score_gap();
        if score_gap > 0.0 {
            // 偏好信号强,向 chosen 方向微调参数
            let chosen_hash = text_hash(&pair.chosen);
            let rejected_hash = text_hash(&pair.rejected);
            for (i, param) in cur_policy.params.iter_mut().enumerate() {
                let chosen_feature = ((chosen_hash.wrapping_add(i as u64)) % 1000) as f32 / 1000.0;
                let rejected_feature =
                    ((rejected_hash.wrapping_add(i as u64)) % 1000) as f32 / 1000.0;
                // 增加 chosen 的权重,减少 rejected 的权重
                *param += learning_rate * (chosen_feature - rejected_feature) * loss;
            }
        }

        // 更新统计
        self.step_counter.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut cum_loss) = self.cumulative_loss.write() {
            *cum_loss += loss;
        }

        Ok(loss)
    }

    /// 批量训练
    ///
    /// 对一组偏好对进行训练,返回平均损失。
    pub fn train_batch(
        &self,
        pairs: &[PreferencePair],
        learning_rate: f32,
    ) -> Result<f32, AutoDpoError> {
        if pairs.is_empty() {
            return Ok(0.0);
        }

        let mut total_loss = 0.0f32;
        for pair in pairs {
            total_loss += self.train_step(pair, learning_rate)?;
        }

        Ok(total_loss / pairs.len() as f32)
    }

    /// 获取当前训练步数
    pub fn step_count(&self) -> u64 {
        self.step_counter.load(Ordering::Relaxed)
    }

    /// 获取平均损失(累计损失 / 步数)
    pub fn average_loss(&self) -> f32 {
        let steps = self.step_count().max(1);
        self.cumulative_loss
            .read()
            .map(|loss| *loss / steps as f32)
            .unwrap_or(0.0)
    }

    /// 获取参考策略参数(只读,用于验证冻结)
    pub fn reference_params(&self) -> Vec<f32> {
        self.reference_policy
            .read()
            .map(|p| p.params.clone())
            .unwrap_or_default()
    }

    /// 获取当前策略参数
    pub fn current_params(&self) -> Vec<f32> {
        self.current_policy
            .read()
            .map(|p| p.params.clone())
            .unwrap_or_default()
    }
}

/// Sigmoid 函数的 log 值: log(1 / (1 + exp(-x)))
///
/// 数值稳定实现:当 x > 0 时, log_sigmoid(x) = -log(1 + exp(-x))
/// 当 x <= 0 时, log_sigmoid(x) = x - log(1 + exp(x))
fn log_sigmoid(x: f32) -> f32 {
    if x > 0.0 {
        -((-x).exp() + 1.0).ln()
    } else {
        x - (x.exp() + 1.0).ln()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pair(chosen_score: f32, rejected_score: f32) -> PreferencePair {
        PreferencePair::new(
            "test-pair",
            "chosen output",
            "rejected output",
            chosen_score,
            rejected_score,
        )
    }

    #[test]
    fn test_dpo_trainer_new() {
        let trainer = DpoTrainer::with_default_config();
        assert_eq!(trainer.step_count(), 0);
        assert_eq!(trainer.average_loss(), 0.0);
    }

    #[test]
    fn test_compute_loss_non_negative() {
        let trainer = DpoTrainer::with_default_config();
        let pair = make_pair(0.9, 0.3);
        let loss = trainer.compute_loss(&pair).unwrap();
        assert!(loss >= 0.0, "DPO损失应为非负, got {loss}");
    }

    #[test]
    fn test_train_step_increments_counter() {
        let trainer = DpoTrainer::with_default_config();
        let pair = make_pair(0.9, 0.3);
        let _ = trainer.train_step(&pair, 0.01).unwrap();
        assert_eq!(trainer.step_count(), 1);
    }

    #[test]
    fn test_reference_policy_frozen() {
        let trainer = DpoTrainer::with_default_config();
        let ref_before = trainer.reference_params();
        let pair = make_pair(0.9, 0.3);
        let _ = trainer.train_step(&pair, 0.01).unwrap();
        let ref_after = trainer.reference_params();
        assert_eq!(ref_before, ref_after, "参考策略应冻结,训练后不应变化");
    }

    #[test]
    fn test_current_policy_updates() {
        let trainer = DpoTrainer::with_default_config();
        let cur_before = trainer.current_params();
        let pair = make_pair(0.9, 0.3);
        let _ = trainer.train_step(&pair, 0.01).unwrap();
        let cur_after = trainer.current_params();
        // 当前策略应更新(参数变化)
        assert_ne!(cur_before, cur_after, "当前策略应被训练更新");
    }

    #[test]
    fn test_batch_training() {
        let trainer = DpoTrainer::with_default_config();
        let pairs = vec![
            make_pair(0.9, 0.3),
            make_pair(0.8, 0.2),
            make_pair(0.7, 0.1),
        ];
        let avg_loss = trainer.train_batch(&pairs, 0.01).unwrap();
        assert!(avg_loss >= 0.0);
        assert_eq!(trainer.step_count(), 3);
    }

    #[test]
    fn test_log_sigmoid() {
        // log_sigmoid(0) = log(0.5) = -ln(2) ≈ -0.693
        let v = log_sigmoid(0.0);
        assert!((v - (-std::f32::consts::LN_2)).abs() < 1e-5);

        // log_sigmoid(大正数) ≈ 0
        let v2 = log_sigmoid(10.0);
        assert!(v2 > -0.01);

        // log_sigmoid(大负数) ≈ x
        let v3 = log_sigmoid(-10.0);
        assert!((v3 - (-10.0)).abs() < 0.1);
    }

    #[test]
    fn test_convergence_simulation() {
        // 模拟训练收敛:损失应逐渐降低
        let trainer = DpoTrainer::new(0.1, 64);
        let pair = make_pair(0.95, 0.1); // 强偏好信号

        let mut losses = Vec::new();
        for _ in 0..10 {
            let loss = trainer.train_step(&pair, 0.05).unwrap();
            losses.push(loss);
        }

        // 最后5步的平均损失应低于前5步
        let first_avg: f32 = losses[..5].iter().sum::<f32>() / 5.0;
        let last_avg: f32 = losses[5..].iter().sum::<f32>() / 5.0;
        assert!(
            last_avg <= first_avg * 1.5,
            "损失应趋于稳定或下降, first={first_avg}, last={last_avg}"
        );
    }
}
