//! PPO Critic 网络 — 基于 PPO 思想的价值评估模型
//!
//! 对应架构层:L4 Security
//! 对应 P3-3:ASA PPO 强化学习接入
//!
//! # 设计决策
//!
//! - **纯 Rust 实现**:无外部 ONNX 依赖,零外部库依赖(仅使用 `rand` 初始化)
//! - **Critic-only**:仅输出动作价值(Q 值),不输出策略分布(与 PPO Actor-Critic 的区别)
//! - **在线学习**:通过 `train()` 方法进行 MSE + SGD 更新,与 `AsaAuditor` 的反馈闭环集成
//! - **冷启动**:模型初始化后即可使用(随机权重),通过 `is_initialized()` 指示是否经过训练
//!
//! # 网络架构
//!
//! ```text
//! 输入层(4) → 全连接(16) → ReLU → 全连接(3) → 输出(3)
//! ```
//!
//! - 输入: `(keyword_count, history_failure_rate, complexity_score, operation_type_embedding)`
//! - 输出: 3 个动作的 Q 值 `(Allow, Warn, Block)`
//! - 隐藏层: 16 神经元,ReLU 激活
//!
//! # 得分与置信度
//!
//! - **动作选择**:取 Q 值最大的动作作为 PPO 推荐
//! - **置信度**:`max(Q) - min(Q)`,归一化到 [0, 1],表示 PPO 对推荐的确定程度
//! - **Q 值转评分**:将 Q 值归一化映射到 [0, 1] 区间,作为 PPO 评分

/// PPO Critic 网络 — 2 层全连接 + ReLU
///
/// # 状态空间
/// 输入为 4 维向量:
/// 1. `keyword_count` — 风险关键字匹配数(归一化到 [0, 1])
/// 2. `history_failure_rate` — 历史失败率 [0, 1]
/// 3. `complexity_score` — 操作复杂度 [0, 1]
/// 4. `operation_type_embedding` — 操作类型嵌入(0=read, 0.5=write, 1.0=admin)
///
/// # 动作空间
/// 输出为 3 维 Q 值向量,分别对应 Allow / Warn / Block 的价值估计。
#[derive(Debug, Clone)]
pub struct PpoCritic {
    /// 输入层→隐藏层权重 (16×4)
    w1: [[f32; 4]; 16],
    /// 隐藏层偏置 (16)
    b1: [f32; 16],
    /// 隐藏层→输出层权重 (3×16)
    w2: [[f32; 16]; 3],
    /// 输出层偏置 (3)
    b2: [f32; 3],
    /// 学习率(默认 0.01)
    learning_rate: f32,
    /// 训练计数(模型是否经过至少一次训练)
    trained_steps: u64,
}

impl PpoCritic {
    /// 创建 PPO Critic 网络,使用小随机值初始化权重
    ///
    /// 权重初始化:使用均匀分布 [-0.1, 0.1),偏置初始化为 0.0。
    /// WHY 小随机值初始化:避免初始权重过大导致 ReLU 饱和/梯度消失;
    /// 训练后权重逐渐调整到合理范围。
    pub fn new() -> Self {
        // 使用固定种子确保可重复性
        let mut rng = PpoRng::new(42);
        let mut w1 = [[0.0f32; 4]; 16];
        let mut w2 = [[0.0f32; 16]; 3];

        for row in w1.iter_mut() {
            for val in row.iter_mut() {
                *val = rng.uniform(-0.1, 0.1);
            }
        }
        for row in w2.iter_mut() {
            for val in row.iter_mut() {
                *val = rng.uniform(-0.1, 0.1);
            }
        }

        Self {
            w1,
            b1: [0.0f32; 16],
            w2,
            b2: [0.0f32; 3],
            learning_rate: 0.01,
            trained_steps: 0,
        }
    }

    /// 设置学习率
    pub fn with_learning_rate(mut self, lr: f32) -> Self {
        self.learning_rate = lr;
        self
    }

    /// 前向推理 — 输入状态向量,输出 3 个动作的 Q 值
    ///
    /// # 参数
    /// - `state`: 4 维状态向量 `(keyword_count, history_failure_rate, complexity_score, op_type_embedding)`
    ///
    /// # 返回
    /// 3 维 Q 值向量 `[Allow_Q, Warn_Q, Block_Q]`
    ///
    /// # 计算过程
    /// 1. 隐藏层: `h = ReLU(w1 · state + b1)`
    /// 2. 输出层: `q = w2 · h + b2`
    #[allow(clippy::needless_range_loop)]
    pub fn forward(&self, state: &[f32; 4]) -> [f32; 3] {
        // 隐藏层: h = ReLU(w1 · state + b1)
        let mut hidden = [0.0f32; 16];
        for i in 0..16 {
            let mut sum = self.b1[i];
            for j in 0..4 {
                sum += self.w1[i][j] * state[j];
            }
            hidden[i] = relu(sum);
        }

        // 输出层: q = w2 · h + b2
        let mut output = [0.0f32; 3];
        for i in 0..3 {
            let mut sum = self.b2[i];
            for j in 0..16 {
                sum += self.w2[i][j] * hidden[j];
            }
            output[i] = sum;
        }
        output
    }

    /// 在线学习 — 使用 MSE 损失 + SGD 更新权重
    ///
    /// # 参数
    /// - `state`: 4 维状态向量
    /// - `target_q`: 目标 Q 值向量(3 维),来自反馈闭环(成功/失败)
    ///
    /// # 计算过程
    /// 1. 前向传播计算当前 Q 值
    /// 2. 计算 MSE 损失: L = mean((target_q - q)^2)
    /// 3. 反向传播计算梯度
    /// 4. SGD 更新: w = w - lr * dL/dw
    #[allow(clippy::needless_range_loop)]
    pub fn train(&mut self, state: &[f32; 4], target_q: &[f32; 3]) -> f32 {
        // 前向传播
        // 隐藏层: h = ReLU(w1 · state + b1)
        let mut hidden = [0.0f32; 16];
        let mut pre_activation = [0.0f32; 16]; // 保存激活前的值用于梯度计算
        for i in 0..16 {
            let mut sum = self.b1[i];
            for j in 0..4 {
                sum += self.w1[i][j] * state[j];
            }
            pre_activation[i] = sum;
            hidden[i] = relu(sum);
        }

        // 输出层: q = w2 · h + b2
        let mut output = [0.0f32; 3];
        for i in 0..3 {
            let mut sum = self.b2[i];
            for j in 0..16 {
                sum += self.w2[i][j] * hidden[j];
            }
            output[i] = sum;
        }

        // 计算 MSE 损失
        let mut loss = 0.0f32;
        for i in 0..3 {
            let diff = target_q[i] - output[i];
            loss += diff * diff;
        }
        loss /= 3.0;

        // 反向传播: 输出层梯度 dL/dw2, dL/db2
        // dL/dq_i = 2 * (q_i - target_i) / 3
        let mut d_output = [0.0f32; 3];
        for i in 0..3 {
            d_output[i] = 2.0 * (output[i] - target_q[i]) / 3.0;
        }

        // 更新 w2, b2
        for i in 0..3 {
            for j in 0..16 {
                self.w2[i][j] -= self.learning_rate * d_output[i] * hidden[j];
            }
            self.b2[i] -= self.learning_rate * d_output[i];
        }

        // 反向传播到隐藏层: dL/dh_j = sum_i(d_output[i] * w2[i][j])
        let mut d_hidden = [0.0f32; 16];
        for j in 0..16 {
            let mut sum = 0.0;
            for i in 0..3 {
                sum += d_output[i] * self.w2[i][j];
            }
            // ReLU 梯度: dL/dh_j = sum * (pre_activation > 0 ? 1 : 0)
            d_hidden[j] = sum * relu_derivative(pre_activation[j]);
        }

        // 更新 w1, b1
        for i in 0..16 {
            for j in 0..4 {
                self.w1[i][j] -= self.learning_rate * d_hidden[i] * state[j];
            }
            self.b1[i] -= self.learning_rate * d_hidden[i];
        }

        self.trained_steps += 1;
        loss
    }

    /// 计算输出置信度 — 基于 Q 值差异度量
    ///
    /// 公式: `confidence = (max(Q) - min(Q)) / max(|max(Q)|, |min(Q)|, 1e-8)`
    /// 返回 [0, 1] 区间,越高表示模型对推荐动作越确定。
    ///
    /// WHY 此置信度公式:Q 值差异越大,表示模型对各动作的区分度越高,
    /// 推荐越可靠。当所有 Q 值相近时,置信度低,应回退到规则评分。
    pub fn confidence(&self, output: &[f32; 3]) -> f32 {
        let max_q = output.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let min_q = output.iter().cloned().fold(f32::INFINITY, f32::min);
        let range = max_q - min_q;
        let denom = max_q.abs().max(min_q.abs()).max(1e-8);
        (range / denom).clamp(0.0, 1.0)
    }

    /// 检查模型是否经过至少一次训练
    pub fn is_initialized(&self) -> bool {
        self.trained_steps > 0
    }

    /// 获取训练步数
    pub fn trained_steps(&self) -> u64 {
        self.trained_steps
    }

    /// 将 Q 值向量归一化为 [0, 1] 评分
    ///
    /// 使用 softmax 归一化 + 加权,将 Q 值映射到 [0, 1]。
    /// 公式: `score = sum(softmax(Q)_i * normalized_Q_i)`
    /// 其中 `normalized_Q_i` 将 Q 值线性映射到 [0, 1]。
    ///
    /// 返回:聚合后的 PPO 评分,表示模型对操作安全性的整体评估。
    pub fn q_values_to_score(output: &[f32; 3]) -> f32 {
        // 将 Q 值映射到 [0, 1]
        let max_q = output.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let min_q = output.iter().cloned().fold(f32::INFINITY, f32::min);
        let range = (max_q - min_q).max(1e-8);

        let mut normalized = [0.0f32; 3];
        for i in 0..3 {
            normalized[i] = (output[i] - min_q) / range;
        }

        // 使用 softmax 权重(Allow 权重最高,Block 权重最低)
        let weights = [0.5, 0.3, 0.2]; // Allow, Warn, Block
        let mut score = 0.0;
        for i in 0..3 {
            score += weights[i] * normalized[i];
        }
        score.clamp(0.0, 1.0)
    }
}

impl Default for PpoCritic {
    fn default() -> Self {
        Self::new()
    }
}

/// ReLU 激活函数
fn relu(x: f32) -> f32 {
    x.max(0.0)
}

/// ReLU 导数(用于反向传播)
fn relu_derivative(x: f32) -> f32 {
    if x > 0.0 {
        1.0
    } else {
        0.0
    }
}

/// 简单伪随机数生成器(确定性,用于权重初始化)
///
/// WHY 自实现而非使用 `rand` crate:保持零外部依赖,确定性初始化
/// 确保可重复性(相同种子产生相同权重)。
struct PpoRng {
    state: u64,
}

impl PpoRng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// 生成 [min, max) 区间内的均匀分布随机浮点数
    fn uniform(&mut self, min: f32, max: f32) -> f32 {
        // xorshift64 算法
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        let normalized = (self.state as f64 / u64::MAX as f64) as f32;
        min + normalized * (max - min)
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ppo_forward_shape() {
        // 前向推理输出形状为 3 维
        let critic = PpoCritic::new();
        let state = [0.5, 0.3, 0.2, 0.0];
        let output = critic.forward(&state);
        assert_eq!(output.len(), 3, "PPO 输出应为 3 维");
        // 输出值应为有限值(非 NaN)
        for v in output {
            assert!(v.is_finite(), "PPO 输出值应有限, 实际: {}", v);
        }
    }

    #[test]
    fn test_ppo_forward_different_states_produce_different_outputs() {
        // 不同状态输入产生不同输出
        let critic = PpoCritic::new();
        let state1 = [0.0, 0.0, 0.0, 0.0];
        let state2 = [1.0, 1.0, 1.0, 1.0];
        let out1 = critic.forward(&state1);
        let out2 = critic.forward(&state2);
        // 至少有一个值不同
        let differs = out1
            .iter()
            .zip(out2.iter())
            .any(|(a, b)| (a - b).abs() > 1e-6);
        assert!(differs, "不同状态输入应产生不同输出");
    }

    #[test]
    fn test_ppo_confidence_range() {
        // 置信度应在 [0, 1] 区间
        let critic = PpoCritic::new();
        let state = [0.5, 0.3, 0.2, 0.0];
        let output = critic.forward(&state);
        let conf = critic.confidence(&output);
        assert!(
            (0.0..=1.0).contains(&conf),
            "置信度应在 [0,1], 实际: {}",
            conf
        );
    }

    #[test]
    fn test_ppo_initialized_false_at_start() {
        // 新创建的模型未经过训练
        let critic = PpoCritic::new();
        assert!(!critic.is_initialized());
        assert_eq!(critic.trained_steps(), 0);
    }

    #[test]
    fn test_ppo_training_changes_weights() {
        // 训练后权重发生变化
        let mut critic = PpoCritic::new();
        let state = [0.5, 0.3, 0.2, 0.0];
        let target = [0.8, 0.5, 0.2];

        let output_before = critic.forward(&state);
        let loss = critic.train(&state, &target);

        let output_after = critic.forward(&state);
        // 损失应为有限值
        assert!(loss.is_finite(), "损失应有限, 实际: {}", loss);
        // 训练后输出应变化
        let changed = output_before
            .iter()
            .zip(output_after.iter())
            .any(|(a, b)| (a - b).abs() > 1e-6);
        assert!(changed, "训练后输出应发生变化");
        // 训练步数递增
        assert_eq!(critic.trained_steps(), 1);
        assert!(critic.is_initialized());
    }

    #[test]
    fn test_ppo_training_reduces_loss() {
        // 多次训练后损失应降低
        let mut critic = PpoCritic::new();
        let state = [0.5, 0.3, 0.2, 0.0];
        let target = [0.9, 0.5, 0.1];

        let mut losses = Vec::new();
        for _ in 0..50 {
            let loss = critic.train(&state, &target);
            losses.push(loss);
        }

        // 最后一步的损失应小于第一步
        assert!(
            losses.last().unwrap() < losses.first().unwrap(),
            "训练后损失应降低: first={}, last={}",
            losses.first().unwrap(),
            losses.last().unwrap()
        );
    }

    #[test]
    fn test_ppo_q_values_to_score_in_range() {
        // Q 值转评分应在 [0, 1] 区间
        let output = [0.5, 0.3, 0.1];
        let score = PpoCritic::q_values_to_score(&output);
        assert!(
            (0.0..=1.0).contains(&score),
            "评分应在 [0,1], 实际: {}",
            score
        );
    }

    #[test]
    fn test_ppo_q_values_to_score_higher_q_higher_score() {
        // 更高的 Allow Q 值应产生更高的评分
        let low_allow = PpoCritic::q_values_to_score(&[0.1, 0.3, 0.5]);
        let high_allow = PpoCritic::q_values_to_score(&[0.9, 0.3, 0.1]);
        assert!(
            high_allow > low_allow,
            "Allow Q 值越高评分应越高: low={}, high={}",
            low_allow,
            high_allow
        );
    }

    #[test]
    fn test_ppo_forward_no_nan() {
        // 任意合法输入,前向推理输出无 NaN
        let critic = PpoCritic::new();
        let inputs = [
            [0.0, 0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0, 1.0],
            [0.5, 0.5, 0.5, 0.5],
            [0.0, 0.0, 0.0, 1.0],
            [0.3, 0.7, 0.2, 0.5],
        ];
        for state in &inputs {
            let output = critic.forward(state);
            for v in output {
                assert!(v.is_finite(), "输入 {:?} 的输出 {} 应为有限值", state, v);
            }
        }
    }
}
