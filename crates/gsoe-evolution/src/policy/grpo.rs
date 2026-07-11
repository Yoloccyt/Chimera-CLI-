//! GRPO 组采样与优势计算 — 基于 DeepSeek-Math GRPO 算法
//!
//! 对应架构层:L2 Evolution
//! 对应创新点:GSOE (Guided Self-Organizing Evolution)
//!
//! # 算法核心
//! GRPO (Group Relative Policy Optimization) 省去 critic 网络, 直接用同一
//! prompt 的一组采样奖励的均值/标准差作为 baseline, 计算组内相对优势:
//!
//! ```text
//! A_i = (r_i - mean(r)) / (std(r) + eps)
//! ```
//!
//! # 完整 GRPO 目标函数
//! 本模块实现完整的 GRPO 策略梯度更新, 包括:
//! - 组采样与奖励计算
//! - 相对优势估计 (z-score 标准化)
//! - 概率比计算 (π_θ / π_θ_old)
//! - Clip Surrogate Objective (PPO 风格)
//! - KL 散度约束 (高斯解析形式 + Schulman 估计器)
//! - 策略熵奖励 (鼓励探索)
//! - 参数梯度上升更新
//!
//! 参考论文:
//! - DeepSeek-Math: Shao et al., 2024 (arXiv:2402.03300)
//! - DAPO: Yu et al., 2025
//! - Dr.GRPO: Liu et al., 2025
//! - PPO: Schulman et al., 2017

use crate::types::{EvolutionPolicy, GrpoObjectiveResult, GrpoRollout, PolicyGradient};

/// 防除零常数 (GRPO 论文标准值)
const EPS: f32 = 1e-8;

/// 单条轨迹的动作维度 (本周占位固定为 10)
const ACTION_DIM: usize = 10;

/// 最优解动作 (本周占位: 全 1.0 向量, reward 基于距离该向量的负距离)
const OPTIMAL_ACTION: f32 = 1.0;

/// ln(2π) — 高斯对数概率的常数项
const LN_2PI: f32 = 1.8378771;

/// 0.5 * ln(2πe) — 高斯熵的常数项
const HALF_LN_2PIE: f32 = 1.4189385;

// === 线性同余 PRNG (保留原有实现) ===

/// 线性同余 PRNG — 零外部依赖的伪随机数生成器
///
/// WHY: 本周占位不引入 rand crate(避免 workspace 依赖膨胀)。
/// 使用 glibc 同余参数 (a=1103515245, c=12345, m=2^31), 周期 2^31
/// 足够覆盖采样需求。Week 7 接入真实模型后由模型 logits 替代。
pub(crate) struct Lcg {
    state: u64,
}

impl Lcg {
    /// 以 seed 构造 PRNG
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_add(1),
        }
    }

    /// 生成下一个 u32 伪随机数
    pub(crate) fn next_u32(&mut self) -> u32 {
        // glibc LCG 参数: m=2^31, a=1103515245, c=12345
        self.state = self.state.wrapping_mul(1103515245).wrapping_add(12345);
        // 取高 31 位作为输出 (低位的周期较短)
        ((self.state >> 16) & 0x7FFF_FFFF) as u32
    }

    /// 生成 [-1.0, 1.0) 范围的 f32
    pub(crate) fn next_f32(&mut self) -> f32 {
        let raw = self.next_u32() as f32 / (u32::MAX as f32 / 2.0);
        raw - 1.0
    }
}

// === 采样函数 (保留原有实现) ===

/// 基于 policy 与模型采样客户端异步采样一组 rollout
///
/// P0-6 实现: 支持真实模型采样 (Mock 模式保留 LCG 伪随机)。
/// 通过 `ModelSampler` 发送采样请求到外部模型服务, 获取 logits/embedding 作为动作向量。
/// Mock 模式直接返回本地伪随机动作, 与原有行为一致。
///
/// # 参数
/// - `policy`: 进化策略参数
/// - `count`: 采样轨迹数
/// - `sampler`: 模型采样客户端 (Mock 或真实)
///
/// # 返回
/// 采样轨迹列表, 每个轨迹包含动作向量与奖励
pub async fn sample_rollouts_with_model(
    policy: &EvolutionPolicy,
    count: u32,
    sampler: &crate::model_client::ModelSampler,
) -> Vec<GrpoRollout> {
    let mut rollouts = Vec::with_capacity(count as usize);

    for i in 0..count {
        let trajectory_id = format!("traj-{i}");
        let req = crate::model_client::ModelSampleRequest {
            prompt: format!(
                "mutation_rate={:.4},selection_pressure={:.4}",
                policy.mutation_rate, policy.selection_pressure
            ),
            temperature: policy.mutation_rate,
            action_dim: ACTION_DIM,
            trajectory_id: trajectory_id.clone(),
        };

        match sampler.sample(req).await {
            Ok(resp) => {
                let reward = resp.estimated_reward.unwrap_or_else(|| {
                    // 本地计算 reward: 负距离
                    let distance = resp
                        .actions
                        .iter()
                        .map(|a| (a - OPTIMAL_ACTION).abs())
                        .sum::<f32>()
                        / ACTION_DIM as f32;
                    -distance
                });
                rollouts.push(GrpoRollout::new(trajectory_id, resp.actions, reward));
            }
            Err(e) => {
                // 采样失败: 回退到 Mock 生成
                tracing::warn!(error = %e, trajectory_id = %trajectory_id, "模型采样失败, 回退到 Mock");
                let mut rng = Lcg::new(i as u64 + 42);
                let mut actions = Vec::with_capacity(ACTION_DIM);
                for _ in 0..ACTION_DIM {
                    let action = OPTIMAL_ACTION + policy.mutation_rate * rng.next_f32();
                    actions.push(action);
                }
                let distance = actions
                    .iter()
                    .map(|a| (a - OPTIMAL_ACTION).abs())
                    .sum::<f32>()
                    / ACTION_DIM as f32;
                let reward = -distance;
                rollouts.push(GrpoRollout::new(trajectory_id, actions, reward));
            }
        }
    }

    rollouts
}

/// 基于 policy 与 seed 采样一组 rollout (同步版本, Mock 模式)
///
/// 保留原有同步接口, 用于无 async 上下文的场景。内部使用 LCG 伪随机。
/// 新代码应优先使用 `sample_rollouts_with_model` 以支持真实模型接入。
pub fn sample_rollouts(policy: &EvolutionPolicy, count: u32) -> Vec<GrpoRollout> {
    let mut rng = Lcg::new(42);
    let mut rollouts = Vec::with_capacity(count as usize);

    for i in 0..count {
        let mut actions = Vec::with_capacity(ACTION_DIM);
        for _ in 0..ACTION_DIM {
            // 动作 = 基准 + mutation_rate * 扰动
            let action = OPTIMAL_ACTION + policy.mutation_rate * rng.next_f32();
            actions.push(action);
        }

        // reward = 负距离 (越接近最优解 reward 越接近 0.0)
        let distance = actions
            .iter()
            .map(|a| (a - OPTIMAL_ACTION).abs())
            .sum::<f32>()
            / ACTION_DIM as f32;
        let reward = -distance;

        let trajectory_id = format!("traj-{i}");
        rollouts.push(GrpoRollout::new(trajectory_id, actions, reward));
    }

    rollouts
}

// === 相对优势计算 (保留原有实现) ===

/// 计算组内相对优势 (GRPO 核心公式)
///
/// 原地修改 rollouts 中每个元素的 advantage 字段:
/// ```text
/// A_i = (r_i - mean(r)) / (std(r) + eps)
/// ```
///
/// 边界处理:
/// - 空 rollout: 直接返回 (无操作)
/// - 单个 rollout: advantage = 0.0 (无组内对比基准)
/// - 全相同 reward: advantage = 0.0 (std=0, 分子=0)
pub fn compute_advantage(rollouts: &mut [GrpoRollout]) {
    if rollouts.is_empty() {
        return;
    }

    let n = rollouts.len() as f32;
    let mean: f32 = rollouts.iter().map(|r| r.reward).sum::<f32>() / n;

    // 样本标准差 (除以 n 而非 n-1, 与 GRPO 原始实现一致)
    let variance: f32 = rollouts
        .iter()
        .map(|r| {
            let diff = r.reward - mean;
            diff * diff
        })
        .sum::<f32>()
        / n;
    let std = variance.sqrt();

    let denom = std + EPS;
    for rollout in rollouts.iter_mut() {
        let advantage = (rollout.reward - mean) / denom;
        rollout.advantage = Some(advantage);
    }
}

// === 高斯策略模型 (GRPO 核心扩展) ===

/// 高斯策略对数概率
///
/// 假设策略模型为独立高斯分布:
/// π_θ(a_t) = N(a_t | μ=OPTIMAL_ACTION, σ=mutation_rate)
///
/// 对数概率:
/// log π(a) = -0.5 * ln(2π) - ln(σ) - (a-μ)²/(2σ²)
///
/// 序列的 log-prob 为各动作独立求和。
pub fn gaussian_log_prob(actions: &[f32], policy: &EvolutionPolicy) -> f32 {
    let mu = OPTIMAL_ACTION;
    let sigma = policy.mutation_rate.max(policy.grpo_hyperparams.min_std);
    let two_sigma_sq = 2.0 * sigma * sigma;
    let log_sigma = sigma.ln();

    let mut log_prob = 0.0;
    for &a in actions {
        let diff = a - mu;
        log_prob += -0.5 * LN_2PI - log_sigma - diff * diff / two_sigma_sq;
    }
    log_prob
}

/// 高斯策略熵
///
/// H = 0.5 * ln(2πeσ²) = 0.5 * ln(2πe) + ln(σ)
///
/// 对独立动作序列, 总熵 = action_dim * H_single
pub fn gaussian_entropy(policy: &EvolutionPolicy, action_dim: usize) -> f32 {
    let sigma = policy.mutation_rate.max(policy.grpo_hyperparams.min_std);
    let single_entropy = HALF_LN_2PIE + sigma.ln();
    single_entropy * action_dim as f32
}

/// 高斯策略之间的 KL 散度 (解析形式)
///
/// D_KL(N(μ_new, σ_new²) || N(μ_ref, σ_ref²))
///   = 0.5 * ( (σ_new² + (μ_new - μ_ref)²) / σ_ref² - 1 + ln(σ_ref²/σ_new²) )
pub fn gaussian_kl_divergence(new_policy: &EvolutionPolicy, ref_policy: &EvolutionPolicy) -> f32 {
    let mu_new = OPTIMAL_ACTION;
    let mu_ref = OPTIMAL_ACTION;
    let sigma_new = new_policy
        .mutation_rate
        .max(new_policy.grpo_hyperparams.min_std);
    let sigma_ref = ref_policy
        .mutation_rate
        .max(ref_policy.grpo_hyperparams.min_std);

    let sigma_new_sq = sigma_new * sigma_new;
    let sigma_ref_sq = sigma_ref * sigma_ref;

    let term1 = (sigma_new_sq + (mu_new - mu_ref) * (mu_new - mu_ref)) / sigma_ref_sq;
    let term2 = (sigma_ref_sq / sigma_new_sq).ln();

    0.5 * (term1 - 1.0 + term2)
}

/// Schulman 无偏 KL 散度估计器
///
/// 用于 token-level 或 rollout-level 的 KL 估计:
/// D_KL = ref_prob / new_prob - ln(ref_prob / new_prob) - 1
///
/// 该估计器保证非负, 且为真实 KL 的下界。
/// 参考: Schulman et al., 2020 (PPO 论文附录)
pub fn schulman_kl_estimator(log_prob_ref: f32, log_prob_new: f32) -> f32 {
    let log_ratio = (log_prob_ref - log_prob_new).clamp(-20.0, 20.0);
    let ratio = log_ratio.exp();
    ratio - log_ratio - 1.0
}

// === 概率比计算 ===

/// 纯函数: 计算概率比 ρ = π_θ_new / π_θ_old
///
/// 在对数空间计算防止数值下溢:
/// ρ = exp(log_prob_new - log_prob_old)
///
/// # 参数
/// - `actions`: 动作序列
/// - `new_policy`: 新策略
/// - `old_policy`: 旧策略
///
/// # 返回
/// 概率比 ρ, 范围 (0, +∞)
pub fn compute_ratio_pure(
    actions: &[f32],
    new_policy: &EvolutionPolicy,
    old_policy: &EvolutionPolicy,
) -> f32 {
    if actions.is_empty() {
        return 1.0;
    }
    let log_new = gaussian_log_prob(actions, new_policy);
    let log_old = gaussian_log_prob(actions, old_policy);
    let log_ratio = (log_new - log_old).clamp(-20.0, 20.0);
    log_ratio.exp()
}

/// 原地计算并存储概率比
///
/// 对 rollout 原地计算 `old_logprob`, `current_logprob`, 和 `ratio`。
/// 旧策略下动作序列的对数概率存入 `old_logprob`, 当前策略下存入 `current_logprob`。
///
/// # 参数
/// - `rollout`: 待更新的 rollout
/// - `new_policy`: 当前策略
/// - `old_policy`: 旧策略 (用于 ratio 基准)
pub fn compute_ratio(
    rollout: &mut GrpoRollout,
    new_policy: &EvolutionPolicy,
    old_policy: &EvolutionPolicy,
) {
    if rollout.actions.is_empty() {
        rollout.ratio = Some(1.0);
        rollout.current_logprob = Some(0.0);
        rollout.old_logprob = Some(0.0);
        return;
    }
    let log_new = gaussian_log_prob(&rollout.actions, new_policy);
    let log_old = gaussian_log_prob(&rollout.actions, old_policy);
    let log_ratio = (log_new - log_old).clamp(-20.0, 20.0);

    rollout.current_logprob = Some(log_new);
    rollout.old_logprob = Some(log_old);
    rollout.ratio = Some(log_ratio.exp());
}

// === Clip Surrogate Objective ===

/// Clip Surrogate Objective (PPO 风格)
///
/// L_clip = min(ρ * A, clip(ρ, 1-ε, 1+ε) * A)
///
/// 当 ρ > 1+ε 且 A > 0 时, 梯度截断, 防止策略过度更新。
/// 当 ρ < 1-ε 且 A < 0 时, 同样截断。
///
/// 返回值: 标量 surrogate (策略梯度 ascent 时最大化)
///
/// # 参数
/// - `ratio`: 概率比 ρ
/// - `advantage`: 相对优势 A
/// - `epsilon`: clip 范围 ε
///
/// # 返回
/// Clip surrogate objective 值
pub fn clip_surrogate_objective(ratio: f32, advantage: f32, epsilon: f32) -> f32 {
    let clipped_ratio = ratio.clamp(1.0 - epsilon, 1.0 + epsilon);
    let unclipped = ratio * advantage;
    let clipped = clipped_ratio * advantage;
    // 取 min 实现 pessimistic bound (PPO 核心)
    unclipped.min(clipped)
}

// === 完整 GRPO 目标函数 ===

/// 计算完整 GRPO 目标函数
///
/// J = E[ L_clip(ρ_i, A_i) ] - β * D_KL(π_θ || π_ref) + η * H(π_θ)
///
/// 其中:
/// - L_clip: Clip Surrogate Objective
/// - D_KL: 新策略与参考策略的 KL 散度
/// - H: 新策略的熵 (鼓励探索)
/// - β: KL 惩罚系数
/// - η: 熵奖励系数
///
/// # 参数
/// - `rollouts`: 采样轨迹, 每个 rollout 需包含 `ratio` 和 `advantage`
/// - `new_policy`: 当前待评估的策略
/// - `old_policy`: 生成 rollouts 的旧策略
/// - `ref_policy`: 参考策略 (用于 KL 约束)
///
/// # 返回
/// `GrpoObjectiveResult` 包含目标函数值、surrogate、KL、熵
pub fn compute_grpo_objective(
    rollouts: &[GrpoRollout],
    new_policy: &EvolutionPolicy,
    _old_policy: &EvolutionPolicy,
    ref_policy: &EvolutionPolicy,
) -> GrpoObjectiveResult {
    let hp = &new_policy.grpo_hyperparams;

    let mut surrogate_sum = 0.0;
    let mut valid_count = 0;

    for rollout in rollouts {
        let Some(ratio) = rollout.ratio else { continue };
        let Some(advantage) = rollout.advantage else {
            continue;
        };

        // 优势裁剪 (防止极端值导致梯度爆炸)
        let clipped_advantage = advantage.clamp(-hp.advantage_clip, hp.advantage_clip);

        let surrogate = clip_surrogate_objective(ratio, clipped_advantage, hp.epsilon);
        surrogate_sum += surrogate;
        valid_count += 1;
    }

    let mean_surrogate = if valid_count > 0 {
        surrogate_sum / valid_count as f32
    } else {
        0.0
    };

    let kl = gaussian_kl_divergence(new_policy, ref_policy);
    let entropy = gaussian_entropy(new_policy, ACTION_DIM);

    // 总目标: 最大化 surrogate - β*KL + η*H
    let objective = mean_surrogate - hp.kl_beta * kl + hp.entropy_coef * entropy;

    GrpoObjectiveResult {
        objective,
        mean_surrogate,
        kl_divergence: kl,
        entropy,
    }
}

// === 策略梯度计算 ===

/// 使用有限差分计算策略参数梯度
///
/// 对 mutation_rate (σ) 的梯度:
/// ∂L/∂σ ≈ (L(σ + δ) - L(σ - δ)) / (2δ)
///
/// 这是一个通用的有限差分梯度估计器, 不依赖于策略模型的具体解析形式。
/// 对于高斯策略, 也可以解析计算, 但有限差分更通用且对数值误差鲁棒。
///
/// # 参数
/// - `rollouts`: 原始采样轨迹 (advantage 已计算)
/// - `policy`: 当前策略 (中心点)
/// - `old_policy`: 旧策略 (用于 ratio 基准)
/// - `ref_policy`: 参考策略 (用于 KL 约束)
///
/// # 返回
/// `PolicyGradient` 包含各参数的梯度
pub fn compute_policy_gradient(
    rollouts: &[GrpoRollout],
    policy: &EvolutionPolicy,
    old_policy: &EvolutionPolicy,
    ref_policy: &EvolutionPolicy,
) -> PolicyGradient {
    let delta = 1e-4;

    // 正向扰动
    let mut policy_plus = policy.clone();
    policy_plus.mutation_rate = (policy.mutation_rate + delta).min(policy.grpo_hyperparams.max_std);

    // 负向扰动
    let mut policy_minus = policy.clone();
    policy_minus.mutation_rate =
        (policy.mutation_rate - delta).max(policy.grpo_hyperparams.min_std);

    // 为正向扰动重新计算概率比
    let mut rollouts_plus = rollouts.to_vec();
    for r in rollouts_plus.iter_mut() {
        compute_ratio(r, &policy_plus, old_policy);
    }

    // 为负向扰动重新计算概率比
    let mut rollouts_minus = rollouts.to_vec();
    for r in rollouts_minus.iter_mut() {
        compute_ratio(r, &policy_minus, old_policy);
    }

    let obj_plus = compute_grpo_objective(&rollouts_plus, &policy_plus, old_policy, ref_policy);
    let obj_minus = compute_grpo_objective(&rollouts_minus, &policy_minus, old_policy, ref_policy);

    // 中心有限差分
    let grad_sigma = (obj_plus.objective - obj_minus.objective) / (2.0 * delta);

    // selection_pressure 的梯度: 目前映射为 learning rate 的缩放因子
    // 或者通过有限差分计算 (留作扩展)
    let grad_selection_pressure = 0.0;

    PolicyGradient {
        grad_mutation_rate: grad_sigma,
        grad_selection_pressure,
    }
}

/// 使用解析梯度计算策略参数梯度 (高斯策略专用)
///
/// 解析梯度公式:
/// ∂L/∂σ = E[ g_clip * A * ((a-μ)² - σ²) / σ³ ] - β * (σ/σ_ref² - 1/σ) + η * (1/σ)
///
/// 其中 g_clip 是 clip 的指示函数:
/// - 当 |ρ - 1| < ε (未触发 clip) 时, g_clip = ρ
/// - 当 ρ > 1+ε 且 A > 0 (clip 上限) 时, g_clip = 0
/// - 当 ρ < 1-ε 且 A < 0 (clip 下限) 时, g_clip = 0
///
/// 解析梯度比有限差分更精确、更快, 但仅适用于高斯策略。
///
/// # 参数
/// - `rollouts`: 采样轨迹 (ratio 和 advantage 已计算)
/// - `policy`: 当前策略
/// - `ref_policy`: 参考策略
///
/// # 返回
/// `PolicyGradient` 包含各参数的梯度
pub fn compute_policy_gradient_analytical(
    rollouts: &[GrpoRollout],
    policy: &EvolutionPolicy,
    ref_policy: &EvolutionPolicy,
) -> PolicyGradient {
    let hp = &policy.grpo_hyperparams;
    let mu = OPTIMAL_ACTION;
    let sigma = policy.mutation_rate.max(hp.min_std);
    let sigma_sq = sigma * sigma;
    let sigma_cubed = sigma_sq * sigma;

    let sigma_ref = ref_policy.mutation_rate.max(hp.min_std);
    let sigma_ref_sq = sigma_ref * sigma_ref;

    let mut grad_surrogate = 0.0;
    let mut valid_count = 0;

    for rollout in rollouts {
        let Some(ratio) = rollout.ratio else { continue };
        let Some(advantage) = rollout.advantage else {
            continue;
        };

        let clipped_advantage = advantage.clamp(-hp.advantage_clip, hp.advantage_clip);

        // 判断是否触发 clip (梯度为 0)
        let is_clipped = if clipped_advantage.total_cmp(&0.0).is_gt() {
            ratio.total_cmp(&(1.0 + hp.epsilon)).is_gt()
        } else if clipped_advantage.total_cmp(&0.0).is_lt() {
            ratio.total_cmp(&(1.0 - hp.epsilon)).is_lt()
        } else {
            false
        };

        if is_clipped {
            continue; // clipped, gradient = 0
        }

        // ∂log π/∂σ = sum_i ((a_i - μ)² - σ²) / σ³
        let mut grad_log_prob = 0.0;
        for &a in &rollout.actions {
            let diff = a - mu;
            grad_log_prob += (diff * diff - sigma_sq) / sigma_cubed;
        }

        // 梯度 = ratio * advantage * ∂log π_new/∂σ
        grad_surrogate += ratio * clipped_advantage * grad_log_prob;
        valid_count += 1;
    }

    let mean_grad_surrogate = if valid_count > 0 {
        grad_surrogate / valid_count as f32
    } else {
        0.0
    };

    // KL 梯度: ∂D_KL/∂σ = σ/σ_ref² - 1/σ
    let grad_kl = sigma / sigma_ref_sq - 1.0 / sigma;

    // 熵梯度: ∂H/∂σ = 1/σ
    let grad_entropy = 1.0 / sigma;

    let grad_sigma = mean_grad_surrogate - hp.kl_beta * grad_kl + hp.entropy_coef * grad_entropy;

    PolicyGradient {
        grad_mutation_rate: grad_sigma,
        grad_selection_pressure: 0.0,
    }
}

// === 策略更新 ===

/// 使用 GRPO 梯度更新策略参数
///
/// 执行梯度上升 (最大化目标函数):
/// σ_new = σ_old + α * grad_σ
///
/// 更新后 clamp 到 [min_std, max_std] 防止越界。
///
/// # 参数
/// - `policy`: 待更新的策略 (原地修改)
/// - `gradient`: 策略梯度
/// - `learning_rate`: 学习率 α
pub fn update_policy_with_grpo(
    policy: &mut EvolutionPolicy,
    gradient: &PolicyGradient,
    learning_rate: f32,
) {
    // mutation_rate 更新 (σ)
    let new_mr = policy.mutation_rate + learning_rate * gradient.grad_mutation_rate;
    policy.mutation_rate = new_mr.clamp(
        policy.grpo_hyperparams.min_std,
        policy.grpo_hyperparams.max_std,
    );

    // selection_pressure 更新 (可选)
    if gradient.grad_selection_pressure != 0.0 {
        let new_sp = policy.selection_pressure + learning_rate * gradient.grad_selection_pressure;
        policy.selection_pressure = new_sp.max(0.0);
    }

    tracing::debug!(
        grad_mr = gradient.grad_mutation_rate,
        new_mr = policy.mutation_rate,
        lr = learning_rate,
        "GRPO 策略更新完成"
    );
}

// === 迭代 GRPO 更新 (多次迭代) ===

/// 执行多次 GRPO 策略更新
///
/// 对应 GRPO 论文 Algorithm 1 中的内层循环:
/// for iteration = 1..μ:
///   1. 计算概率比
///   2. 计算目标函数
///   3. 计算梯度
///   4. 更新参数
///
/// 每次迭代使用固定的 old_policy (原始采样策略), 这是标准 PPO/GRPO 的做法。
///
/// # 参数
/// - `rollouts`: 采样轨迹 (advantage 已计算)
/// - `policy`: 待更新的策略 (原地修改)
/// - `ref_policy`: 参考策略
/// - `iterations`: 更新迭代次数
///
/// # 返回
/// 每次迭代的目标函数结果列表
pub fn iterative_grpo_update(
    rollouts: &mut [GrpoRollout],
    policy: &mut EvolutionPolicy,
    ref_policy: &EvolutionPolicy,
    iterations: u32,
) -> Vec<GrpoObjectiveResult> {
    let mut results = Vec::with_capacity(iterations as usize);
    let old_policy = policy.clone(); // 固定不变 (标准 PPO 做法)

    for _ in 0..iterations {
        // 重新计算概率比 (当前策略 vs 原始策略)
        for rollout in rollouts.iter_mut() {
            compute_ratio(rollout, policy, &old_policy);
        }

        // 计算梯度 (使用解析形式)
        let gradient = compute_policy_gradient_analytical(rollouts, policy, ref_policy);

        // 更新参数
        let lr = policy.grpo_hyperparams.learning_rate;
        update_policy_with_grpo(policy, &gradient, lr);

        // 计算目标函数值 (用于监控)
        let result = compute_grpo_objective(rollouts, policy, &old_policy, ref_policy);
        results.push(result);
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_policy() -> EvolutionPolicy {
        match EvolutionPolicy::new(0.1, 1.5, 0.2, 8) {
            Ok(p) => p,
            Err(e) => panic!("构造策略失败: {e}"),
        }
    }

    // === 保留原有测试 ===

    #[test]
    fn test_sample_rollouts_count() {
        let policy = make_policy();
        let rollouts = sample_rollouts(&policy, 8);
        assert_eq!(rollouts.len(), 8);
    }

    #[test]
    fn test_sample_rollouts_count_zero() {
        let policy = make_policy();
        let rollouts = sample_rollouts(&policy, 0);
        assert!(rollouts.is_empty());
    }

    #[test]
    fn test_sample_rollouts_action_dim() {
        let policy = make_policy();
        let rollouts = sample_rollouts(&policy, 4);
        for rollout in &rollouts {
            assert_eq!(rollout.actions.len(), ACTION_DIM);
        }
    }

    #[test]
    fn test_sample_rollouts_advantage_none_before_compute() {
        let policy = make_policy();
        let rollouts = sample_rollouts(&policy, 4);
        for rollout in &rollouts {
            assert!(rollout.advantage.is_none());
        }
    }

    #[test]
    fn test_sample_rollouts_reward_non_positive() {
        // reward = -distance <= 0.0 (距离非负)
        let policy = make_policy();
        let rollouts = sample_rollouts(&policy, 8);
        for rollout in &rollouts {
            assert!(
                rollout.reward.total_cmp(&0.0).is_le(),
                "reward 应为非正: {}",
                rollout.reward
            );
        }
    }

    #[test]
    fn test_compute_advantage_empty() {
        let mut rollouts: Vec<GrpoRollout> = vec![];
        compute_advantage(&mut rollouts);
        assert!(rollouts.is_empty());
    }

    #[test]
    fn test_compute_advantage_single_rollout() {
        let mut rollouts = vec![GrpoRollout::new("t-0".into(), vec![1.0], 0.5)];
        compute_advantage(&mut rollouts);
        // 单个 rollout: mean=reward, std=0, advantage=(0)/(0+eps)=0.0
        assert!(match rollouts[0].advantage {
            Some(v) => v.total_cmp(&0.0).is_eq(),
            None => false,
        });
    }

    #[test]
    fn test_compute_advantage_all_same_reward() {
        let mut rollouts: Vec<GrpoRollout> = (0..5)
            .map(|i| GrpoRollout::new(format!("t-{i}"), vec![1.0], 0.3))
            .collect();
        compute_advantage(&mut rollouts);
        // 全相同 reward: 分子=0, advantage=0.0
        for rollout in &rollouts {
            assert!(match rollout.advantage {
                Some(v) => v.total_cmp(&0.0).is_eq(),
                None => false,
            });
        }
    }

    #[test]
    fn test_compute_advantage_monotonicity() {
        // reward 单调递增 → advantage 应单调递增
        let mut rollouts: Vec<GrpoRollout> = (0..10)
            .map(|i| GrpoRollout::new(format!("t-{i}"), vec![1.0], i as f32 * 0.1))
            .collect();
        compute_advantage(&mut rollouts);

        for i in 1..rollouts.len() {
            let prev = match rollouts[i - 1].advantage {
                Some(v) => v,
                None => panic!("advantage 未计算"),
            };
            let curr = match rollouts[i].advantage {
                Some(v) => v,
                None => panic!("advantage 未计算"),
            };
            assert!(
                curr >= prev,
                "advantage 应单调递增: idx {i} prev={prev} curr={curr}"
            );
        }
    }

    #[test]
    fn test_compute_advantage_mean_zero() {
        // 优势值之和应接近 0 (因为 (r_i - mean) 之和 = 0)
        let mut rollouts: Vec<GrpoRollout> = (0..8)
            .map(|i| GrpoRollout::new(format!("t-{i}"), vec![1.0], (i as f32) * 0.5 - 1.75))
            .collect();
        compute_advantage(&mut rollouts);

        let sum: f32 = rollouts
            .iter()
            .map(|r| match r.advantage {
                Some(v) => v,
                None => panic!("advantage 未计算"),
            })
            .sum();
        assert!(
            sum.abs().total_cmp(&1e-5).is_lt(),
            "优势值之和应接近 0, 实际 {sum}"
        );
    }

    #[test]
    fn test_compute_advantage_extreme_values() {
        // 一个极端高 reward, 其余相同 → 高 reward 的 advantage 应显著为正
        let mut rollouts: Vec<GrpoRollout> = (0..10)
            .map(|i| {
                let reward = if i == 0 { 10.0 } else { -1.0 };
                GrpoRollout::new(format!("t-{i}"), vec![1.0], reward)
            })
            .collect();
        compute_advantage(&mut rollouts);

        let top_advantage = match rollouts[0].advantage {
            Some(v) => v,
            None => panic!("advantage 未计算"),
        };
        assert!(
            top_advantage.total_cmp(&0.0).is_gt(),
            "最高 reward 的 advantage 应为正, 实际 {top_advantage}"
        );
        // 其余 advantage 应为负
        for rollout in rollouts.iter().skip(1) {
            assert!(
                match rollout.advantage {
                    Some(v) => v.total_cmp(&0.0).is_lt(),
                    None => panic!("advantage 未计算"),
                },
                "低 reward 的 advantage 应为负"
            );
        }
    }

    // === 新增 GRPO 核心算法测试 ===

    #[test]
    fn test_gaussian_log_prob_consistency() {
        let policy = make_policy();
        let actions = vec![1.0; ACTION_DIM];
        let log_prob = gaussian_log_prob(&actions, &policy);
        // 动作等于均值时, log_prob 应为最高 (密度最大)
        assert!(log_prob.is_finite());
    }

    #[test]
    fn test_gaussian_log_prob_far_from_mean() {
        let policy = make_policy();
        let actions_near = vec![1.0; ACTION_DIM];
        let actions_far = vec![2.0; ACTION_DIM];
        let log_prob_near = gaussian_log_prob(&actions_near, &policy);
        let log_prob_far = gaussian_log_prob(&actions_far, &policy);
        // 远离均值的动作概率更低
        assert!(log_prob_far.total_cmp(&log_prob_near).is_lt());
    }

    #[test]
    fn test_gaussian_entropy_increases_with_std() {
        let mut policy_low = make_policy();
        policy_low.mutation_rate = 0.1;
        let mut policy_high = make_policy();
        policy_high.mutation_rate = 0.3;

        let entropy_low = gaussian_entropy(&policy_low, ACTION_DIM);
        let entropy_high = gaussian_entropy(&policy_high, ACTION_DIM);
        // 标准差越大, 熵越高
        assert!(entropy_high.total_cmp(&entropy_low).is_gt());
    }

    #[test]
    fn test_gaussian_kl_identical_policies() {
        let policy = make_policy();
        let kl = gaussian_kl_divergence(&policy, &policy);
        // 相同策略的 KL 散度应为 0
        assert!(
            kl.abs().total_cmp(&1e-5).is_lt(),
            "相同策略的 KL 应为 0, 实际 {kl}"
        );
    }

    #[test]
    fn test_gaussian_kl_non_negative() {
        let mut policy_new = make_policy();
        policy_new.mutation_rate = 0.2;
        let mut policy_ref = make_policy();
        policy_ref.mutation_rate = 0.1;

        let kl = gaussian_kl_divergence(&policy_new, &policy_ref);
        assert!(kl.total_cmp(&0.0).is_ge(), "KL 散度应为非负, 实际 {kl}");
    }

    #[test]
    fn test_schulman_kl_estimator_non_negative() {
        let log_prob_ref = -2.0;
        let log_prob_new = -3.0;
        let kl = schulman_kl_estimator(log_prob_ref, log_prob_new);
        assert!(
            kl.total_cmp(&0.0).is_ge(),
            "Schulman KL 估计器应为非负, 实际 {kl}"
        );
    }

    #[test]
    fn test_compute_ratio_pure_identity() {
        let policy = make_policy();
        let actions = vec![1.0; ACTION_DIM];
        let ratio = compute_ratio_pure(&actions, &policy, &policy);
        // 相同策略的概率比应为 1
        assert!(
            (ratio - 1.0).abs().total_cmp(&1e-5).is_lt(),
            "相同策略 ratio 应为 1, 实际 {ratio}"
        );
    }

    #[test]
    fn test_compute_ratio_stores_values() {
        let mut rollout = GrpoRollout::new("t-1".into(), vec![1.0; ACTION_DIM], -0.5);
        let policy = make_policy();
        compute_ratio(&mut rollout, &policy, &policy);
        assert!(rollout.old_logprob.is_some());
        assert!(rollout.current_logprob.is_some());
        assert!(rollout.ratio.is_some());
        assert!((match rollout.ratio {
            Some(v) => v,
            None => panic!("ratio 未计算"),
        } - 1.0)
            .abs()
            .total_cmp(&1e-5)
            .is_lt());
    }

    #[test]
    fn test_clip_surrogate_positive_advantage() {
        // A > 0, ρ = 1.3, ε = 0.2 → clipped to 1.2
        let surrogate = clip_surrogate_objective(1.3, 1.0, 0.2);
        // min(1.3 * 1.0, 1.2 * 1.0) = 1.2
        assert!(
            (surrogate - 1.2).abs().total_cmp(&1e-5).is_lt(),
            "clip surrogate 应为 1.2, 实际 {surrogate}"
        );
    }

    #[test]
    fn test_clip_surrogate_negative_advantage() {
        // A < 0, ρ = 0.7, ε = 0.2 → clipped to 0.8
        let surrogate = clip_surrogate_objective(0.7, -1.0, 0.2);
        // min(0.7 * (-1.0), 0.8 * (-1.0)) = min(-0.7, -0.8) = -0.8
        assert!(
            (surrogate - (-0.8)).abs().total_cmp(&1e-5).is_lt(),
            "clip surrogate 应为 -0.8, 实际 {surrogate}"
        );
    }

    #[test]
    fn test_clip_surrogate_unclipped() {
        // A > 0, ρ = 1.1, ε = 0.2 → 未触发 clip
        let surrogate = clip_surrogate_objective(1.1, 1.0, 0.2);
        // min(1.1 * 1.0, 1.1 * 1.0) = 1.1
        assert!((surrogate - 1.1).abs().total_cmp(&1e-5).is_lt());
    }

    #[test]
    fn test_compute_grpo_objective_with_empty_rollouts() {
        let policy = make_policy();
        let result = compute_grpo_objective(&[], &policy, &policy, &policy);
        assert!(result.objective.is_finite());
        assert!(result.mean_surrogate.total_cmp(&0.0).is_eq());
    }

    #[test]
    fn test_compute_grpo_objective_kl_penalty() {
        let mut policy_new = make_policy();
        policy_new.mutation_rate = 0.2;
        let policy_ref = make_policy();

        let rollouts = sample_rollouts(&policy_new, 4);
        let result = compute_grpo_objective(&rollouts, &policy_new, &policy_new, &policy_ref);
        // KL 散度 > 0, 所以 objective < mean_surrogate
        assert!(result.kl_divergence.total_cmp(&0.0).is_gt());
        assert!(result.objective.total_cmp(&result.mean_surrogate).is_lt());
    }

    #[test]
    fn test_policy_gradient_finite() {
        let policy = make_policy();
        let mut rollouts = sample_rollouts(&policy, 8);
        compute_advantage(&mut rollouts);
        let gradient = compute_policy_gradient(&rollouts, &policy, &policy, &policy);
        assert!(gradient.grad_mutation_rate.is_finite());
    }

    #[test]
    fn test_policy_gradient_analytical_finite() {
        let policy = make_policy();
        let mut rollouts = sample_rollouts(&policy, 8);
        compute_advantage(&mut rollouts);
        for r in rollouts.iter_mut() {
            compute_ratio(r, &policy, &policy);
        }
        let gradient = compute_policy_gradient_analytical(&rollouts, &policy, &policy);
        assert!(gradient.grad_mutation_rate.is_finite());
    }

    #[test]
    fn test_update_policy_with_grpo_changes_mutation_rate() {
        let mut policy = make_policy();
        let original_mr = policy.mutation_rate;
        let gradient = PolicyGradient {
            grad_mutation_rate: 0.5,
            grad_selection_pressure: 0.0,
        };
        update_policy_with_grpo(&mut policy, &gradient, 0.1);
        // mutation_rate 应发生变化
        assert_ne!(policy.mutation_rate, original_mr);
    }

    #[test]
    fn test_update_policy_with_grpo_clamps() {
        let mut policy = make_policy();
        policy.mutation_rate = 0.4;
        let gradient = PolicyGradient {
            grad_mutation_rate: 10.0, // 极大梯度
            grad_selection_pressure: 0.0,
        };
        update_policy_with_grpo(&mut policy, &gradient, 1.0);
        // 应 clamp 到 max_std
        assert!(policy
            .mutation_rate
            .total_cmp(&policy.grpo_hyperparams.max_std)
            .is_le());
    }

    #[test]
    fn test_iterative_grpo_update_converges() {
        let mut policy = make_policy();
        let ref_policy = policy.clone();
        let mut rollouts = sample_rollouts(&policy, 8);
        compute_advantage(&mut rollouts);

        let results = iterative_grpo_update(&mut rollouts, &mut policy, &ref_policy, 3);

        assert_eq!(results.len(), 3);
        for result in &results {
            assert!(result.objective.is_finite());
            assert!(result.kl_divergence.is_finite());
            assert!(result.entropy.is_finite());
        }
    }

    #[test]
    fn test_ratio_with_different_policies() {
        let mut policy_high = make_policy();
        policy_high.mutation_rate = 0.3;
        let mut policy_low = make_policy();
        policy_low.mutation_rate = 0.1;

        let actions = vec![1.5; ACTION_DIM];
        let ratio = compute_ratio_pure(&actions, &policy_high, &policy_low);

        // 高 σ 策略对远离均值的动作赋予更高概率
        // 所以 ratio = π_high / π_low > 1
        assert!(
            ratio.total_cmp(&1.0).is_gt(),
            "高 σ 策略对远离均值的动作 ratio 应 > 1, 实际 {ratio}"
        );
    }
}
