//! LinUCB 算法核心 — 上下文线性 bandit(Li et al., 2010)
//!
//! 对应任务: **P4-W13.1.3**（LinUCB 算法核心）
//! 对应 ADR: **ADR-031**（omega-learner 边界）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3
//!
//! # 算法概述
//!
//! LinUCB 是经典的上下文线性 bandit 算法,假设每个臂的期望奖励是上下文的线性函数:
//! `E[r_t | x_t, a] = θ_a^T x_t`
//!
//! 其中 `x_t ∈ R^d` 是上下文向量,`θ_a ∈ R^d` 是臂 a 的未知参数。
//!
//! ## UCB 选择策略
//!
//! 每个臂维护:
//! - `A_a ∈ R^{d×d}`: 累积外积矩阵 `A_a = I + Σ x_t x_t^T`(初始为单位矩阵)
//! - `b_a ∈ R^d`: 累积奖励向量 `b_a = Σ r_t · x_t`(初始为零向量)
//!
//! 选择臂的公式(LinUCB with disjoint linear models):
//! ```text
//! θ_a = A_a^{-1} · b_a
//! UCB_a = θ_a^T · x + α · sqrt(x^T · A_a^{-1} · x)
//! arm* = argmax_a UCB_a
//! ```
//!
//! `α` 控制探索-利用平衡: 越大越偏向探索。
//!
//! ## Regret 上界
//!
//! Li et al. (2010) 证明: 在 `||x|| ≤ 1` 假设下,LinUCB 的 T 步 regret 满足:
//! `R(T) = O(sqrt(T · d · ln(K · T)))`
//!
//! 其中 K 是臂数, d 是上下文维度。
//!
//! # 实现决策(WHY)
//!
//! ## 维护 A_a^{-1} 而非 A_a
//!
//! LinUCB 论文原文要求计算 `A_a^{-1} · b_a` 与 `x^T · A_a^{-1} · x`。
//! 朴素实现每次需 O(d³) 求逆,长期运行性能不可接受。
//!
//! 本实现维护 `A_a^{-1}` 而非 `A_a`,利用 Sherman-Morrison 公式增量更新:
//!
//! ```text
//! A_a^{-1} ← A_a^{-1} - (A_a^{-1} · x · x^T · A_a^{-1}) / (1 + x^T · A_a^{-1} · x)
//! ```
//!
//! 每次更新复杂度降为 O(d²),且完全避免外部线性代数依赖(无需 BLAS/LAPACK)。
//!
//! ## f64 内部精度
//!
//! LinUCB 矩阵运算使用 f64(而非 CLV 的 f32),因长期累积下 A_a^{-1} 数值范围可能极大,
//! f32 累加易漂移导致 regret 上界失效。上下文向量在接口层做 f32→f64 转换(开销 < 100ns)。
//!
//! ## 数值稳定性守卫
//!
//! Sherman-Morrison 分母 `1 + x^T · A_a^{-1} · x` 在矩阵病态时可能 ≤ 0 或 NaN,
//! 本实现检测此情况返回 `LearnerError::NumericalInstability`,避免污染模型。

use crate::arm::{ArmId, ArmIndex, ArmSet, DiscreteArmSet};
use crate::context::SeamContext;
use crate::error::{LearnerError, Result};
use ndarray::{Array1, Array2};
use serde::{Deserialize, Serialize};

/// LinUCB 算法核心
///
/// # 设计
///
/// 维护 K 个臂的 `A_a^{-1}` 矩阵与 `b_a` 向量(详见模块级文档),
/// 提供 `select_arm` 与 `update` 两个核心 API。
///
/// # 示例
///
/// ## 基础用法
///
/// ```
/// use omega_learner::arm::{ArmId, DiscreteArmSet};
/// use omega_learner::context::SeamContext;
/// use omega_learner::linucb::LinUCB;
///
/// // 4 个臂,3 维上下文,探索强度 α=1.0
/// let arm_set = DiscreteArmSet::new(vec![
///     ArmId::new("rho=0.5"),
///     ArmId::new("rho=2"),
///     ArmId::new("rho=5"),
///     ArmId::new("rho=10"),
/// ]);
/// let mut linucb = LinUCB::new(3, &arm_set, 1.0).unwrap();
///
/// // 选择臂(初始所有臂 UCB 相同,选第一个)
/// let ctx = SeamContext::new(vec![0.5, 0.3, 0.2]).unwrap();
/// let arm = linucb.select_arm(&ctx).unwrap();
/// assert_eq!(arm.as_u32(), 0);
///
/// // 观察奖励并更新模型
/// linucb.update(arm, &ctx, 0.85).unwrap();
/// assert_eq!(linucb.total_steps(), 1);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinUCB {
    /// 上下文维度 d(构造后不可变)
    context_dim: usize,

    /// 探索强度 α(必须 > 0)
    alpha: f64,

    /// 臂集(用于持久化与 ID ↔ Index 转换)
    arm_set: DiscreteArmSet,

    /// 每个臂的 A_a^{-1} 矩阵(d×d),初始为单位矩阵
    ///
    /// WHY 维护 A_a^{-1} 而非 A_a:
    /// - Sherman-Morrison 公式增量更新(O(d²) vs 求逆 O(d³))
    /// - 避免外部 BLAS/LAPACK 依赖(保持 forbid(unsafe_code) 哲学)
    /// - LinUCB 论文标准实现(原文 disjoint variant)
    arm_inv_matrices: Vec<Array2<f64>>,

    /// 每个臂的 b_a 向量(d 维),初始为零向量
    arm_vectors: Vec<Array1<f64>>,

    /// 已观察到的总步数(用于 regret 分析与持久化诊断)
    total_steps: u64,
}

impl LinUCB {
    /// 创建 LinUCB 实例
    ///
    /// # 参数
    /// - `context_dim`: 上下文维度 d(必须 ≥ 1)
    /// - `arm_set`: 臂集(必须非空,且臂 ID 唯一)
    /// - `alpha`: 探索强度(必须 > 0 且有限)
    ///
    /// # 错误
    /// - `InvalidDimension`: context_dim == 0
    /// - `NoArms`: arm_set 为空
    /// - `InvalidAlpha`: alpha ≤ 0 或非有限
    ///
    /// # 初始化
    /// - A_a^{-1} = I_d(单位矩阵)
    /// - b_a = 0_d(零向量)
    pub fn new(context_dim: usize, arm_set: &DiscreteArmSet, alpha: f64) -> Result<Self> {
        if context_dim == 0 {
            return Err(LearnerError::InvalidDimension);
        }
        let num_arms = arm_set.len();
        if num_arms == 0 {
            return Err(LearnerError::NoArms);
        }
        if !alpha.is_finite() || alpha <= 0.0 {
            return Err(LearnerError::InvalidAlpha { alpha });
        }

        let identity = Array2::<f64>::eye(context_dim);
        let zero = Array1::<f64>::zeros(context_dim);

        Ok(Self {
            context_dim,
            alpha,
            arm_set: arm_set.clone(),
            arm_inv_matrices: vec![identity; num_arms],
            arm_vectors: vec![zero; num_arms],
            total_steps: 0,
        })
    }

    /// 选择臂 — argmax_a UCB_a(x)
    ///
    /// # 算法
    /// 对每个臂 a 计算:
    /// - `θ_a = A_a^{-1} · b_a`
    /// - `exploit = θ_a^T · x`
    /// - `explore = α · sqrt(x^T · A_a^{-1} · x)`
    /// - `UCB_a = exploit + explore`
    ///
    /// 返回 UCB 最大的臂。初始时所有臂 UCB 相同(矩阵相同),返回索引 0。
    ///
    /// # 平局打破
    /// 严格大于才更新最大值,保证首遇到的臂优先(确定性,便于测试)
    pub fn select_arm(&self, context: &SeamContext) -> Result<ArmIndex> {
        if context.dim() != self.context_dim {
            return Err(LearnerError::ContextDimensionMismatch {
                expected: self.context_dim,
                actual: context.dim(),
            });
        }

        let x = context.as_f64_array();
        let x_view = x.view();

        let mut best_arm: usize = 0;
        let mut best_score: f64 = f64::NEG_INFINITY;

        for (arm_idx, (a_inv, b_vec)) in self
            .arm_inv_matrices
            .iter()
            .zip(self.arm_vectors.iter())
            .enumerate()
        {
            // θ_a = A_a^{-1} · b_a  (O(d²) 矩阵-向量乘)
            let theta_a = a_inv.dot(b_vec);

            // exploit = θ_a^T · x
            let exploit = theta_a.dot(&x_view);

            // exploration = α · sqrt(x^T · A_a^{-1} · x)
            // 计算 A_a^{-1} · x(O(d²)),再取 x · (A_a^{-1} · x) = x^T · A_a^{-1} · x
            let a_inv_x = a_inv.dot(&x_view);
            let x_a_inv_x = x_view.dot(&a_inv_x);
            // x_a_inv_x 应 ≥ 0(因 A_a^{-1} 正定),防御性 clamp 避免 sqrt 出 NaN
            let x_a_inv_x_clamped = x_a_inv_x.max(0.0);
            let explore = self.alpha * x_a_inv_x_clamped.sqrt();

            let score = exploit + explore;
            if score > best_score {
                best_score = score;
                best_arm = arm_idx;
            }
        }

        Ok(ArmIndex::from(best_arm))
    }

    /// 观察奖励并更新模型 — Sherman-Morrison 增量更新
    ///
    /// # 算法
    /// ```text
    /// A_a^{-1} ← A_a^{-1} - (A_a^{-1} · x)(A_a^{-1} · x)^T / (1 + x^T · A_a^{-1} · x)
    /// b_a ← b_a + r · x
    /// ```
    ///
    /// # 参数
    /// - `arm`: 被选中的臂索引(由 `select_arm` 返回)
    /// - `context`: 上下文向量(必须与 `select_arm` 时一致)
    /// - `reward`: 观察到的奖励(必须有限)
    ///
    /// # 错误
    /// - `ArmOutOfRange`: arm 索引越界
    /// - `ContextDimensionMismatch`: 上下文维度不匹配
    /// - `InvalidReward`: reward 为 NaN/Infinity
    /// - `NumericalInstability`: Sherman-Morrison 分母 ≤ 0 或非有限
    pub fn update(&mut self, arm: ArmIndex, context: &SeamContext, reward: f64) -> Result<()> {
        let arm_idx = arm.as_usize();
        if arm_idx >= self.arm_inv_matrices.len() {
            return Err(LearnerError::ArmOutOfRange {
                arm: arm_idx,
                total: self.arm_inv_matrices.len(),
            });
        }
        if context.dim() != self.context_dim {
            return Err(LearnerError::ContextDimensionMismatch {
                expected: self.context_dim,
                actual: context.dim(),
            });
        }
        if !reward.is_finite() {
            return Err(LearnerError::InvalidReward { reward });
        }

        let x = context.as_f64_array();
        let x_view = x.view();

        // 取出臂 a 的 A_a^{-1}(borrow 后,避免双重借用)
        let a_inv = &self.arm_inv_matrices[arm_idx];

        // 计算 A_a^{-1} · x(O(d²) 矩阵-向量乘)
        let a_inv_x = a_inv.dot(&x_view);

        // 计算 x^T · A_a^{-1} · x(标量)
        let x_a_inv_x = x_view.dot(&a_inv_x);

        // Sherman-Morrison 分母: 1 + x^T · A_a^{-1} · x
        let denominator = 1.0 + x_a_inv_x;

        // 数值稳定性守卫
        // WHY 必要性: A_a^{-1} 长期运行后可能病态,导致 denominator ≤ 0 或 NaN
        // 此时更新会污染模型,应中断并报错让调用方决定是否重置模型
        if !denominator.is_finite() || denominator <= 0.0 {
            return Err(LearnerError::NumericalInstability);
        }

        // 构造外积 (A_a^{-1} · x) · (A_a^{-1} · x)^T,并按 1/denominator 缩放
        // WHY 用 from_shape_fn 而非 a_inv_x.outer(&a_inv_x.view()):
        // ndarray 0.16 outer API 签名因版本可能变化,from_shape_fn 是稳定 idiomatic 写法
        let d = self.context_dim;
        let scaled_outer =
            Array2::from_shape_fn((d, d), |(i, j)| a_inv_x[i] * a_inv_x[j] / denominator);

        // A_a^{-1} -= scaled_outer
        self.arm_inv_matrices[arm_idx] -= &scaled_outer;

        // b_a += r · x
        let scaled_x = &x * reward;
        self.arm_vectors[arm_idx] += &scaled_x;

        self.total_steps += 1;
        Ok(())
    }

    /// 返回当前总学习步数
    pub fn total_steps(&self) -> u64 {
        self.total_steps
    }

    /// 返回上下文维度 d
    pub fn context_dim(&self) -> usize {
        self.context_dim
    }

    /// 返回探索强度 α
    pub fn alpha(&self) -> f64 {
        self.alpha
    }

    /// 返回臂数 K
    pub fn num_arms(&self) -> usize {
        self.arm_inv_matrices.len()
    }

    /// 返回臂集引用(用于 ID ↔ Index 转换)
    pub fn arm_set(&self) -> &DiscreteArmSet {
        &self.arm_set
    }

    /// 根据 ArmId 查询 ArmIndex(委托给 ArmSet)
    pub fn arm_index_of(&self, id: &ArmId) -> Option<ArmIndex> {
        self.arm_set.index_of(id)
    }

    /// 根据 ArmIndex 查询 ArmId(委托给 ArmSet)
    pub fn arm_id_of(&self, idx: ArmIndex) -> Option<&ArmId> {
        self.arm_set.id_of(idx)
    }

    /// 序列化 LinUCB 状态为 JSON 字符串(用于断点恢复)
    ///
    /// WHY 持久化场景: LinUCB 模型长期累积 A_a^{-1} 与 b_a,
    /// 进程重启后应能恢复而非重新学习(避免重复探索期 regret)
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(LearnerError::from)
    }

    /// 从 JSON 字符串反序列化 LinUCB 状态
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(LearnerError::from)
    }

    /// 重置模型为初始状态(单位矩阵 + 零向量)
    ///
    /// WHY 重置场景:
    /// - 数值不稳定错误后调用方主动重置
    /// - spec 版本切换时清理旧学习数据
    /// - 测试场景隔离
    pub fn reset(&mut self) {
        let identity = Array2::<f64>::eye(self.context_dim);
        let zero = Array1::<f64>::zeros(self.context_dim);
        for a_inv in &mut self.arm_inv_matrices {
            *a_inv = identity.clone();
        }
        for b_vec in &mut self.arm_vectors {
            *b_vec = zero.clone();
        }
        self.total_steps = 0;
    }

    /// 返回指定臂的 A_a^{-1} 矩阵引用(诊断与调试用)
    ///
    /// WHY 暴露内部状态: 单元测试需校验 Sherman-Morrison 更新正确性,
    /// regret 分析也需读取矩阵特征值判断模型健康状况。
    /// 不提供 mut 访问,避免外部破坏模型不变量。
    pub fn arm_inverse_matrix(&self, arm: ArmIndex) -> Result<&Array2<f64>> {
        let arm_idx = arm.as_usize();
        if arm_idx >= self.arm_inv_matrices.len() {
            return Err(LearnerError::ArmOutOfRange {
                arm: arm_idx,
                total: self.arm_inv_matrices.len(),
            });
        }
        Ok(&self.arm_inv_matrices[arm_idx])
    }

    /// 返回指定臂的 b_a 向量引用(诊断与调试用)
    pub fn arm_vector(&self, arm: ArmIndex) -> Result<&Array1<f64>> {
        let arm_idx = arm.as_usize();
        if arm_idx >= self.arm_vectors.len() {
            return Err(LearnerError::ArmOutOfRange {
                arm: arm_idx,
                total: self.arm_vectors.len(),
            });
        }
        Ok(&self.arm_vectors[arm_idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arm::ArmId;

    // ============================================================
    // 辅助函数
    // ============================================================

    /// 4 臂 / 3 维上下文的样本 LinUCB
    fn sample_linucb() -> LinUCB {
        let arm_set = DiscreteArmSet::new(vec![
            ArmId::new("rho=0.5"),
            ArmId::new("rho=2"),
            ArmId::new("rho=5"),
            ArmId::new("rho=10"),
        ]);
        LinUCB::new(3, &arm_set, 1.0).unwrap()
    }

    // ============================================================
    // 构造与参数校验测试
    // ============================================================

    #[test]
    fn test_new_basic() {
        let linucb = sample_linucb();
        assert_eq!(linucb.context_dim(), 3);
        assert_eq!(linucb.num_arms(), 4);
        assert!((linucb.alpha() - 1.0).abs() < 1e-9);
        assert_eq!(linucb.total_steps(), 0);
    }

    #[test]
    fn test_new_invalid_dimension() {
        let arm_set = DiscreteArmSet::new(vec![ArmId::new("a")]);
        let result = LinUCB::new(0, &arm_set, 1.0);
        assert!(matches!(result, Err(LearnerError::InvalidDimension)));
    }

    #[test]
    fn test_new_no_arms() {
        let arm_set = DiscreteArmSet::new(vec![]);
        let result = LinUCB::new(3, &arm_set, 1.0);
        assert!(matches!(result, Err(LearnerError::NoArms)));
    }

    #[test]
    fn test_new_zero_alpha_fails() {
        let arm_set = DiscreteArmSet::new(vec![ArmId::new("a")]);
        let result = LinUCB::new(3, &arm_set, 0.0);
        assert!(matches!(result, Err(LearnerError::InvalidAlpha { .. })));
    }

    #[test]
    fn test_new_negative_alpha_fails() {
        let arm_set = DiscreteArmSet::new(vec![ArmId::new("a")]);
        let result = LinUCB::new(3, &arm_set, -1.0);
        assert!(matches!(result, Err(LearnerError::InvalidAlpha { .. })));
    }

    #[test]
    fn test_new_nan_alpha_fails() {
        let arm_set = DiscreteArmSet::new(vec![ArmId::new("a")]);
        let result = LinUCB::new(3, &arm_set, f64::NAN);
        assert!(matches!(result, Err(LearnerError::InvalidAlpha { .. })));
    }

    #[test]
    fn test_new_infinity_alpha_fails() {
        let arm_set = DiscreteArmSet::new(vec![ArmId::new("a")]);
        let result = LinUCB::new(3, &arm_set, f64::INFINITY);
        assert!(matches!(result, Err(LearnerError::InvalidAlpha { .. })));
    }

    #[test]
    fn test_new_single_arm() {
        let arm_set = DiscreteArmSet::new(vec![ArmId::new("only")]);
        let linucb = LinUCB::new(2, &arm_set, 0.5).unwrap();
        assert_eq!(linucb.num_arms(), 1);
    }

    #[test]
    fn test_new_high_dimension() {
        let arm_set = DiscreteArmSet::new(vec![ArmId::new("a"), ArmId::new("b")]);
        let linucb = LinUCB::new(20, &arm_set, 1.0).unwrap();
        assert_eq!(linucb.context_dim(), 20);
    }

    // ============================================================
    // select_arm 测试
    // ============================================================

    #[test]
    fn test_select_arm_initial_returns_first() {
        // 初始所有臂 UCB 相同,选第一个(平局打破: 严格大于才更新)
        let linucb = sample_linucb();
        let ctx = SeamContext::new(vec![0.5, 0.3, 0.2]).unwrap();
        let arm = linucb.select_arm(&ctx).unwrap();
        assert_eq!(arm.as_u32(), 0);
    }

    #[test]
    fn test_select_arm_dimension_mismatch() {
        let linucb = sample_linucb();
        let ctx = SeamContext::new(vec![0.5, 0.3]).unwrap(); // dim=2, expected 3
        let result = linucb.select_arm(&ctx);
        assert!(matches!(
            result,
            Err(LearnerError::ContextDimensionMismatch {
                expected: 3,
                actual: 2
            })
        ));
    }

    #[test]
    fn test_select_arm_single_arm() {
        let arm_set = DiscreteArmSet::new(vec![ArmId::new("only")]);
        let linucb = LinUCB::new(2, &arm_set, 1.0).unwrap();
        let ctx = SeamContext::new(vec![0.5, 0.5]).unwrap();
        let arm = linucb.select_arm(&ctx).unwrap();
        assert_eq!(arm.as_u32(), 0);
    }

    #[test]
    fn test_select_arm_after_update_preferred_arm() {
        // 更新臂 0 多次,使其 θ_0 朝向 x 方向,后续应优先选臂 0
        let mut linucb = sample_linucb();
        let ctx = SeamContext::new(vec![0.5, 0.3, 0.2]).unwrap();

        // 多次更新臂 0 高奖励
        for _ in 0..10 {
            linucb.update(ArmIndex::new(0), &ctx, 1.0).unwrap();
        }
        // 更新臂 1 低奖励
        for _ in 0..10 {
            linucb.update(ArmIndex::new(1), &ctx, 0.1).unwrap();
        }

        // 现在臂 0 的 UCB 应明显高于臂 1(高 exploit)
        let selected = linucb.select_arm(&ctx).unwrap();
        assert_eq!(selected.as_u32(), 0);
    }

    // ============================================================
    // update 测试
    // ============================================================

    #[test]
    fn test_update_increments_steps() {
        let mut linucb = sample_linucb();
        let ctx = SeamContext::new(vec![0.5, 0.3, 0.2]).unwrap();
        let arm = linucb.select_arm(&ctx).unwrap();

        assert_eq!(linucb.total_steps(), 0);
        linucb.update(arm, &ctx, 0.8).unwrap();
        assert_eq!(linucb.total_steps(), 1);
    }

    #[test]
    fn test_update_arm_out_of_range() {
        let mut linucb = sample_linucb();
        let ctx = SeamContext::new(vec![0.5, 0.3, 0.2]).unwrap();
        let result = linucb.update(ArmIndex::new(99), &ctx, 0.5);
        assert!(matches!(
            result,
            Err(LearnerError::ArmOutOfRange { arm: 99, total: 4 })
        ));
    }

    #[test]
    fn test_update_dimension_mismatch() {
        let mut linucb = sample_linucb();
        let ctx = SeamContext::new(vec![0.5, 0.3]).unwrap(); // dim=2
        let result = linucb.update(ArmIndex::new(0), &ctx, 0.5);
        assert!(matches!(
            result,
            Err(LearnerError::ContextDimensionMismatch { .. })
        ));
    }

    #[test]
    fn test_update_nan_reward_fails() {
        let mut linucb = sample_linucb();
        let ctx = SeamContext::new(vec![0.5, 0.3, 0.2]).unwrap();
        let result = linucb.update(ArmIndex::new(0), &ctx, f64::NAN);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_update_infinity_reward_fails() {
        let mut linucb = sample_linucb();
        let ctx = SeamContext::new(vec![0.5, 0.3, 0.2]).unwrap();
        let result = linucb.update(ArmIndex::new(0), &ctx, f64::INFINITY);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_update_zero_reward_ok() {
        let mut linucb = sample_linucb();
        let ctx = SeamContext::new(vec![0.5, 0.3, 0.2]).unwrap();
        linucb.update(ArmIndex::new(0), &ctx, 0.0).unwrap();
        assert_eq!(linucb.total_steps(), 1);
    }

    #[test]
    fn test_update_negative_reward_ok() {
        // 负奖励是合法的(可能表示失败/惩罚)
        let mut linucb = sample_linucb();
        let ctx = SeamContext::new(vec![0.5, 0.3, 0.2]).unwrap();
        linucb.update(ArmIndex::new(0), &ctx, -0.5).unwrap();
        assert_eq!(linucb.total_steps(), 1);
    }

    #[test]
    fn test_update_modifies_b_vector() {
        // 更新后 b_a[0] 应包含 reward * x
        // WHY 容差 1e-6 而非 1e-9: 上下文向量在接口层从 f32 转 f64,
        // `0.3f32 as f64 = 0.3000000119...`,乘 2.0 后误差 ~2.4e-7 > 1e-9
        let mut linucb = sample_linucb();
        let ctx = SeamContext::new(vec![0.5, 0.3, 0.2]).unwrap();
        linucb.update(ArmIndex::new(0), &ctx, 2.0).unwrap();

        let b_vec = linucb.arm_vector(ArmIndex::new(0)).unwrap();
        // b_a += 2.0 * [0.5, 0.3, 0.2] = [1.0, 0.6, 0.4](容差考虑 f32→f64 转换)
        assert!((b_vec[0] - 1.0).abs() < 1e-6);
        assert!((b_vec[1] - 0.6).abs() < 1e-6);
        assert!((b_vec[2] - 0.4).abs() < 1e-6);
    }

    #[test]
    fn test_update_other_arms_b_vector_unchanged() {
        // 更新臂 0 不应影响臂 1 的 b 向量
        let mut linucb = sample_linucb();
        let ctx = SeamContext::new(vec![0.5, 0.3, 0.2]).unwrap();
        linucb.update(ArmIndex::new(0), &ctx, 1.0).unwrap();

        let b_vec_1 = linucb.arm_vector(ArmIndex::new(1)).unwrap();
        assert!(b_vec_1.iter().all(|&v| v.abs() < 1e-9));
    }

    // ============================================================
    // reset 测试
    // ============================================================

    #[test]
    fn test_reset_clears_state() {
        let mut linucb = sample_linucb();
        let ctx = SeamContext::new(vec![0.5, 0.3, 0.2]).unwrap();
        linucb.update(ArmIndex::new(0), &ctx, 1.0).unwrap();
        linucb.update(ArmIndex::new(1), &ctx, 0.5).unwrap();
        assert_eq!(linucb.total_steps(), 2);

        linucb.reset();
        assert_eq!(linucb.total_steps(), 0);

        // b 向量应归零
        let b_vec = linucb.arm_vector(ArmIndex::new(0)).unwrap();
        assert!(b_vec.iter().all(|&v| v.abs() < 1e-9));
    }

    // ============================================================
    // 序列化测试
    // ============================================================

    #[test]
    fn test_json_roundtrip_empty_model() {
        let linucb = sample_linucb();
        let json = linucb.to_json().unwrap();
        let restored = LinUCB::from_json(&json).unwrap();
        assert_eq!(restored.context_dim(), linucb.context_dim());
        assert_eq!(restored.num_arms(), linucb.num_arms());
        assert_eq!(restored.total_steps(), linucb.total_steps());
    }

    #[test]
    fn test_json_roundtrip_after_updates() {
        let mut linucb = sample_linucb();
        let ctx = SeamContext::new(vec![0.5, 0.3, 0.2]).unwrap();
        linucb.update(ArmIndex::new(0), &ctx, 0.8).unwrap();
        linucb.update(ArmIndex::new(2), &ctx, 0.5).unwrap();

        let json = linucb.to_json().unwrap();
        let restored = LinUCB::from_json(&json).unwrap();

        assert_eq!(restored.total_steps(), 2);
        // b 向量也应一致
        let original_b = linucb.arm_vector(ArmIndex::new(0)).unwrap();
        let restored_b = restored.arm_vector(ArmIndex::new(0)).unwrap();
        for i in 0..3 {
            assert!((original_b[i] - restored_b[i]).abs() < 1e-9);
        }
    }

    // ============================================================
    // ArmSet 委托方法测试
    // ============================================================

    #[test]
    fn test_arm_index_of_by_id() {
        let linucb = sample_linucb();
        assert_eq!(
            linucb.arm_index_of(&ArmId::new("rho=0.5")),
            Some(ArmIndex::new(0))
        );
        assert_eq!(
            linucb.arm_index_of(&ArmId::new("rho=10")),
            Some(ArmIndex::new(3))
        );
        assert_eq!(linucb.arm_index_of(&ArmId::new("rho=99")), None);
    }

    #[test]
    fn test_arm_id_of_by_index() {
        let linucb = sample_linucb();
        assert_eq!(
            linucb.arm_id_of(ArmIndex::new(0)),
            Some(&ArmId::new("rho=0.5"))
        );
        assert_eq!(linucb.arm_id_of(ArmIndex::new(99)), None);
    }

    // ============================================================
    // 诊断方法测试
    // ============================================================

    #[test]
    fn test_arm_inverse_matrix_initial_is_identity() {
        let linucb = sample_linucb();
        let a_inv = linucb.arm_inverse_matrix(ArmIndex::new(0)).unwrap();
        // 初始 A_a^{-1} = I
        for i in 0..3 {
            for j in 0..3 {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!((a_inv[[i, j]] - expected).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn test_arm_inverse_matrix_out_of_range() {
        let linucb = sample_linucb();
        let result = linucb.arm_inverse_matrix(ArmIndex::new(99));
        assert!(matches!(result, Err(LearnerError::ArmOutOfRange { .. })));
    }

    #[test]
    fn test_arm_vector_out_of_range() {
        let linucb = sample_linucb();
        let result = linucb.arm_vector(ArmIndex::new(99));
        assert!(matches!(result, Err(LearnerError::ArmOutOfRange { .. })));
    }

    // ============================================================
    // 不变量测试(LinUCB 算法性质)
    // ============================================================

    #[test]
    fn test_invariant_a_inv_remains_symmetric_after_update() {
        // Sherman-Morrison 更新应保持 A_a^{-1} 对称性
        // WHY 数学保证: 单位矩阵 + 对称外积 - 对称外积 = 对称矩阵
        let mut linucb = sample_linucb();
        let ctx = SeamContext::new(vec![0.5, 0.3, 0.2]).unwrap();
        linucb.update(ArmIndex::new(0), &ctx, 0.8).unwrap();

        let a_inv = linucb.arm_inverse_matrix(ArmIndex::new(0)).unwrap();
        for i in 0..3 {
            for j in 0..3 {
                assert!(
                    (a_inv[[i, j]] - a_inv[[j, i]]).abs() < 1e-9,
                    "A_a^-1 not symmetric at ({},{}) vs ({},{}): {} vs {}",
                    i,
                    j,
                    j,
                    i,
                    a_inv[[i, j]],
                    a_inv[[j, i]]
                );
            }
        }
    }

    #[test]
    fn test_invariant_a_inv_diagonal_positive() {
        // A_a^{-1} 对角线应保持正(正定矩阵性质)
        let mut linucb = sample_linucb();
        let ctx = SeamContext::new(vec![0.5, 0.3, 0.2]).unwrap();
        for _ in 0..10 {
            linucb.update(ArmIndex::new(0), &ctx, 1.0).unwrap();
        }

        let a_inv = linucb.arm_inverse_matrix(ArmIndex::new(0)).unwrap();
        for i in 0..3 {
            assert!(
                a_inv[[i, i]] > 0.0,
                "diagonal element {} non-positive: {}",
                i,
                a_inv[[i, i]]
            );
        }
    }

    #[test]
    fn test_invariant_steps_count_matches_updates() {
        let mut linucb = sample_linucb();
        let ctx = SeamContext::new(vec![0.5, 0.3, 0.2]).unwrap();
        for arm_idx in 0..4u32 {
            linucb.update(ArmIndex::new(arm_idx), &ctx, 0.5).unwrap();
        }
        assert_eq!(linucb.total_steps(), 4);
    }
}
