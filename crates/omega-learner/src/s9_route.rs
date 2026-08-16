//! S9 接缝 — 通道路由学习器(MCA M3,RouteLLM 落点,ADR-065/ADR-031)
//!
//! 对应设计源:`Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.4 路由亲和
//! 对应 ADR: ADR-031(omega-learner 边界)、ADR-043(影子模式)、ADR-065(MCA)
//!
//! # S9 接缝概述
//!
//! | 字段 | 值 |
//! |------|-----|
//! | 接缝名 | S9Route(通道路由,RouteLLM 偏好数据路由落点) |
//! | 臂 | `provider × model × thinking_mode`(7 厂商 × 平均 2 模型 × 3 档 ≈ 40 臂) |
//! | 上下文 | 任务复杂度 / 预算水位 / 延迟敏感度 / 历史缓存命中率 / 风险等级 |
//! | 奖励 | 任务成功 × 质量分 − λ₁·成本 − λ₂·延迟 |
//!
//! # 上下文向量(6 维,§5.4)
//! ```text
//! x = [
//!   task_complexity,        // 0: 任务复杂度 CLV 投影 ∈ [0,1]
//!   budget_water_level,     // 1: 预算水位 ∈ [0,1](已用/总额)
//!   latency_sensitivity,    // 2: 延迟敏感度 ∈ [0,1]
//!   cache_hit_history,       // 3: 历史缓存命中率 ∈ [0,1]
//!   risk_level,             // 4: 风险等级 ∈ [0,1]
//!   bias,                   // 5: 常量 1.0(线性模型偏置)
//! ]
//! ```
//!
//! # R2 冻结红线(ADR-042)兼容
//! 学习路由只产出 `SelectorPolicy::Learned` **建议**,沿用影子模式(ADR-043)
//! 与本地 fallback,**不触碰 R2 冻结面**:本模块不 import gsoe-evolution /
//! auto-dpo,无 evolve/RL 训练路径,只做在线 LinUCB bandit 选择。
//!
//! # 40 臂决策延迟(性能红线,arm.rs L23 假设 ~10 臂)
//! 40 臂是既有设计假设的 4 倍,`select_arm` = O(K·d²) = 40×36 ≈ 1440 flops,
//! 理论 <10μs。`linucb_40arm` benchmark(criterion)验证 p99 < 50μs。

use nexus_contracts::{SelectorPolicy, SelectorWeights};

use crate::arm::{ArmId, ArmIndex, ArmSet, DiscreteArmSet};
use crate::context::SeamContext;
use crate::error::Result;
use crate::linucb::LinUCB;

/// S9 路由上下文维度(6 维,见模块文档)
pub const S9_CONTEXT_DIM: usize = 6;

/// 成本惩罚系数 λ₁(奖励 = 质量 − λ₁·成本 − λ₂·延迟)
const LAMBDA_COST: f64 = 0.3;
/// 延迟惩罚系数 λ₂
const LAMBDA_LATENCY: f64 = 0.2;

/// S9 路由上下文 — 6 维特征(§5.4)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct S9Context {
    /// 任务复杂度(CLV 投影,[0,1])
    pub task_complexity: f32,
    /// 预算水位([0,1],已用/总额)
    pub budget_water_level: f32,
    /// 延迟敏感度([0,1])
    pub latency_sensitivity: f32,
    /// 历史缓存命中率([0,1])
    pub cache_hit_history: f32,
    /// 风险等级([0,1])
    pub risk_level: f32,
}

impl S9Context {
    /// 转为 LinUCB SeamContext(6 维,附偏置项 1.0)
    ///
    /// 各特征钳制到 [0,1] 保证 `||x|| ≤ √6` 有界(LinUCB regret 上界假设)。
    pub fn to_seam_context(self) -> Result<SeamContext> {
        SeamContext::new(vec![
            self.task_complexity.clamp(0.0, 1.0),
            self.budget_water_level.clamp(0.0, 1.0),
            self.latency_sensitivity.clamp(0.0, 1.0),
            self.cache_hit_history.clamp(0.0, 1.0),
            self.risk_level.clamp(0.0, 1.0),
            1.0, // bias
        ])
    }
}

/// S9 奖励信号 — 任务成功 × 质量分 − λ₁·成本 − λ₂·延迟(§5.4)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct S9Reward {
    /// 任务是否成功
    pub success: bool,
    /// 质量分([0,1])
    pub quality: f32,
    /// 归一化成本([0,1],相对预算)
    pub normalized_cost: f32,
    /// 归一化延迟([0,1],相对目标)
    pub normalized_latency: f32,
}

impl S9Reward {
    /// 计算标量奖励:success 门控 × 质量 − 成本惩罚 − 延迟惩罚
    ///
    /// WHY 加法惩罚: LinUCB 假设奖励是上下文线性函数,加法形式符合线性假设
    /// (与 s1_density 一致);失败时质量项归零,只留惩罚(强负反馈)。
    pub fn to_scalar(self) -> f64 {
        let quality = if self.success {
            self.quality.clamp(0.0, 1.0) as f64
        } else {
            0.0
        };
        quality
            - LAMBDA_COST * self.normalized_cost.clamp(0.0, 1.0) as f64
            - LAMBDA_LATENCY * self.normalized_latency.clamp(0.0, 1.0) as f64
    }
}

/// 从路由臂 ID 列表构造离散臂集(provider/model/mode 编码)
///
/// 臂 ID 由调用方(model-router `RouteTarget::arm_id()`)生成,S9 只消费
/// 字符串——保持 omega-learner 与 model-router 解耦(不引入 L1 依赖)。
pub fn build_route_arm_set(arm_ids: &[String]) -> DiscreteArmSet {
    DiscreteArmSet::new(arm_ids.iter().map(ArmId::new).collect())
}

/// S9 通道路由学习器 — LinUCB over 路由臂空间
///
/// 影子模式合规:`suggest_policy` 只产出 `SelectorPolicy::Learned` 建议,
/// 调用方(quest 编排器)在影子期对比学习臂与静态兜底,胜率达标才启用。
#[derive(Debug)]
pub struct S9RouteLearner {
    linucb: LinUCB,
    arm_set: DiscreteArmSet,
    /// 学习版本号(影子模式 A/B 与回滚追踪)
    version: u64,
}

impl S9RouteLearner {
    /// 创建 S9 路由学习器
    ///
    /// # 参数
    /// - `arm_ids`: 路由臂 ID 列表(provider/model/mode);非空
    /// - `alpha`: LinUCB 探索强度(> 0)
    pub fn new(arm_ids: &[String], alpha: f64) -> Result<Self> {
        let arm_set = build_route_arm_set(arm_ids);
        let linucb = LinUCB::new(S9_CONTEXT_DIM, &arm_set, alpha)?;
        Ok(Self {
            linucb,
            arm_set,
            version: 0,
        })
    }

    /// 臂数量
    pub fn arm_count(&self) -> usize {
        self.arm_set.len()
    }

    /// 选择路由臂,返回臂 ID(provider/model/mode)
    pub fn select_route(&self, ctx: S9Context) -> Result<String> {
        let seam_ctx = ctx.to_seam_context()?;
        let idx = self.linucb.select_arm(&seam_ctx)?;
        Ok(self
            .linucb
            .arm_id_of(idx)
            .map(|id| id.as_str().to_string())
            .unwrap_or_default())
    }

    /// 观察奖励并更新模型(版本号递增,供影子模式追踪)
    pub fn observe(&mut self, arm_id: &str, ctx: S9Context, reward: S9Reward) -> Result<()> {
        let seam_ctx = ctx.to_seam_context()?;
        if let Some(idx) = self.arm_set.index_of(&ArmId::new(arm_id)) {
            self.linucb.update(idx, &seam_ctx, reward.to_scalar())?;
            self.version += 1;
        }
        Ok(())
    }

    /// 产出学习路由建议(影子模式合规:Learned 建议,非强制)
    ///
    /// WHY 复用 SelectorWeights: S9 学习结果以 SelectorPolicy::Learned 承载,
    /// 与既有 S4 selector 接缝的策略下发通道一致;调用方本地 fallback 到
    /// `SelectorPolicy::Static`(C4 合规,无跨 crate 旗标传播)。
    ///
    /// # W5 修复(ADR-084 决策 4)
    ///
    /// 原 `suggest_policy` 丢弃 LinUCB 选中臂(`let _ = arm_id;`)并以上下文
    /// 原值伪造权重——本方法改为:
    /// 1. **arm_id 不再丢弃**: `SuggestedRoute` 携带选中臂,路由决策可被消费
    /// 2. **权重由学习量派生**: LinUCB 线性系数 θ=A⁻¹b 的通道幅值 |θ| 经
    ///    L1 归一化(契约要求和为 1)——真正"学到"的特征重要性;无学习信号
    ///    (全零系数)时回退编译期默认权重(C4 合规,不伪造学习)
    /// 3. **期望奖励点估计**: θᵀx 随建议输出(影子模式 A/B 对比输入)
    ///
    /// 通道映射(可审计): relevance←|θ[0] 任务复杂度|, frequency←|θ[3] 缓存
    /// 命中|, recency←|θ[2] 延迟敏感|(时效性偏好)。
    pub fn suggest_route(&self, ctx: S9Context) -> Result<SuggestedRoute> {
        let seam_ctx = ctx.to_seam_context()?;
        let idx = self.linucb.select_arm(&seam_ctx)?;
        let arm_id = self
            .linucb
            .arm_id_of(idx)
            .map(|id| id.as_str().to_string())
            .unwrap_or_default();
        let expected_reward = self.expected_reward(idx, &seam_ctx)? as f32;
        let weights = self.learned_weights(idx)?;
        Ok(SuggestedRoute {
            arm_id,
            policy: SelectorPolicy::learned(self.version, weights),
            expected_reward,
        })
    }

    /// 期望奖励点估计 — θᵀx(θ = A⁻¹b,该臂线性模型系数)
    fn expected_reward(&self, arm: ArmIndex, ctx: &SeamContext) -> Result<f64> {
        let a_inv = self.linucb.arm_inverse_matrix(arm)?;
        let b = self.linucb.arm_vector(arm)?;
        let theta = a_inv.dot(b);
        Ok(theta.dot(&ctx.as_array().mapv(f64::from)))
    }

    /// 学习权重 — |θ| 通道幅值 L1 归一化(和为 1,契约 `is_valid` 要求)
    fn learned_weights(&self, arm: ArmIndex) -> Result<SelectorWeights> {
        let a_inv = self.linucb.arm_inverse_matrix(arm)?;
        let b = self.linucb.arm_vector(arm)?;
        let theta = a_inv.dot(b);
        // 通道映射见 suggest_route 文档
        let relevance = theta[0].abs() as f32;
        let frequency = theta[3].abs() as f32;
        let recency = theta[2].abs() as f32;
        let sum = relevance + frequency + recency;
        if sum <= f32::EPSILON {
            // 无学习信号: 编译期默认(0.4, 0.3, 0.3)——C4 合规回退,不伪造学习
            return Ok(SelectorWeights::default());
        }
        Ok(SelectorWeights::new(
            recency / sum,
            frequency / sum,
            relevance / sum,
        ))
    }

    /// 已观察步数(诊断/影子模式收敛判断)
    pub fn total_steps(&self) -> u64 {
        self.linucb.total_steps()
    }
}

/// 学习路由建议 — 影子模式合规产物(W5)
#[derive(Debug, Clone)]
pub struct SuggestedRoute {
    /// 选中的路由臂 ID(provider/model/mode)
    pub arm_id: String,
    /// 学习策略建议(version 供影子模式 A/B 与回滚追踪)
    pub policy: SelectorPolicy,
    /// 当前上下文下该臂的期望奖励(θᵀx 点估计)
    pub expected_reward: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造 ~40 臂路由集(7 厂商代表 × 模型 × 3 思考档)
    fn sample_arms() -> Vec<String> {
        let providers_models = [
            ("zhipu", "glm-5.2"),
            ("deep_seek", "deepseek-v4-flash"),
            ("deep_seek", "deepseek-v4-pro"),
            ("moonshot", "kimi-k3"),
            ("mini_max", "MiniMax-M3"),
            ("volcano_ark", "doubao-seed-2.1-pro"),
            ("alibaba_cloud", "qwen-max"),
            ("step_fun", "step-3.5-flash-2603"),
        ];
        let modes = ["fast", "standard", "deep"];
        let mut arms = Vec::new();
        for (p, m) in providers_models {
            for mode in modes {
                arms.push(format!("{p}/{m}/{mode}"));
            }
        }
        arms
    }

    #[test]
    fn arm_set_covers_provider_model_mode() {
        let arms = sample_arms();
        // 8 provider-model × 3 mode = 24 臂(代表集;真实 ~40 臂含更多模型)
        assert_eq!(arms.len(), 24);
        let learner = S9RouteLearner::new(&arms, 1.0).unwrap();
        assert_eq!(learner.arm_count(), 24);
    }

    #[test]
    fn select_returns_valid_arm_id() {
        let arms = sample_arms();
        let learner = S9RouteLearner::new(&arms, 1.0).unwrap();
        let ctx = S9Context {
            task_complexity: 0.8,
            budget_water_level: 0.3,
            latency_sensitivity: 0.5,
            cache_hit_history: 0.6,
            risk_level: 0.2,
        };
        let chosen = learner.select_route(ctx).unwrap();
        assert!(arms.contains(&chosen), "选中臂必在臂集内: {chosen}");
    }

    #[test]
    fn observe_updates_and_increments_version() {
        let arms = sample_arms();
        let mut learner = S9RouteLearner::new(&arms, 1.0).unwrap();
        let ctx = S9Context {
            task_complexity: 0.5,
            budget_water_level: 0.2,
            latency_sensitivity: 0.4,
            cache_hit_history: 0.7,
            risk_level: 0.1,
        };
        let reward = S9Reward {
            success: true,
            quality: 0.9,
            normalized_cost: 0.3,
            normalized_latency: 0.2,
        };
        learner.observe("zhipu/glm-5.2/deep", ctx, reward).unwrap();
        assert_eq!(learner.total_steps(), 1);
    }

    #[test]
    fn reward_scalar_penalizes_cost_and_latency() {
        // 成功高质量低成本低延迟 → 高奖励
        let good = S9Reward {
            success: true,
            quality: 1.0,
            normalized_cost: 0.0,
            normalized_latency: 0.0,
        };
        assert!((good.to_scalar() - 1.0).abs() < 1e-6);
        // 失败 → 质量归零,只留惩罚(负奖励)
        let bad = S9Reward {
            success: false,
            quality: 1.0,
            normalized_cost: 1.0,
            normalized_latency: 1.0,
        };
        assert!(bad.to_scalar() < 0.0, "失败必负奖励");
        // 成本/延迟惩罚生效:高成本降低奖励
        let costly = S9Reward {
            success: true,
            quality: 1.0,
            normalized_cost: 1.0,
            normalized_latency: 1.0,
        };
        assert!((costly.to_scalar() - (1.0 - LAMBDA_COST - LAMBDA_LATENCY)).abs() < 1e-6);
    }

    #[test]
    fn suggest_route_carries_arm_and_learned_version() {
        let arms = sample_arms();
        let mut learner = S9RouteLearner::new(&arms, 1.0).unwrap();
        let ctx = S9Context {
            task_complexity: 0.6,
            budget_water_level: 0.5,
            latency_sensitivity: 0.3,
            cache_hit_history: 0.5,
            risk_level: 0.2,
        };
        // 观察一次后 version=1
        learner
            .observe(
                "deep_seek/deepseek-v4-flash/fast",
                ctx,
                S9Reward {
                    success: true,
                    quality: 0.8,
                    normalized_cost: 0.1,
                    normalized_latency: 0.1,
                },
            )
            .unwrap();
        let suggestion = learner.suggest_route(ctx).unwrap();
        // W5 修复: arm_id 不再丢弃——路由决策可被消费
        assert!(!suggestion.arm_id.is_empty(), "arm_id 必须携带");
        assert!(arms.contains(&suggestion.arm_id), "臂必在臂集内");
        // 影子模式合规:产出 Learned 策略(非 Static),携带 version
        assert!(suggestion.policy.is_learned());
        assert_eq!(suggestion.policy.version(), Some(1));
        // 契约: 权重合法(非负且和为 1)
        assert!(suggestion.policy.weights().is_valid());
    }

    #[test]
    fn unlearned_suggestion_falls_back_to_default_weights() {
        // 无学习信号(全零系数) → 编译期默认权重(C4 合规,不伪造学习)
        let arms = sample_arms();
        let learner = S9RouteLearner::new(&arms, 1.0).unwrap();
        let ctx = S9Context {
            task_complexity: 0.5,
            budget_water_level: 0.5,
            latency_sensitivity: 0.5,
            cache_hit_history: 0.5,
            risk_level: 0.5,
        };
        let suggestion = learner.suggest_route(ctx).unwrap();
        assert!(!suggestion.arm_id.is_empty());
        assert_eq!(suggestion.policy.version(), Some(0));
        // 全零 θ → 回退默认 (0.4, 0.3, 0.3)
        let (recency, frequency, relevance) = suggestion.policy.weights().as_tuple();
        assert!((recency - 0.4).abs() < 1e-6);
        assert!((frequency - 0.3).abs() < 1e-6);
        assert!((relevance - 0.3).abs() < 1e-6);
    }

    #[test]
    fn learned_weights_derive_from_theta_not_context() {
        // W5 核心断言: 权重由 θ 派生,不再等于上下文原值——
        // 对同一臂持续高奖励观察后,系数通道幅值主导权重(与上下文解耦)
        let arms = sample_arms();
        let mut learner = S9RouteLearner::new(&arms, 0.5).unwrap();
        let ctx = S9Context {
            task_complexity: 0.9,
            budget_water_level: 0.1,
            latency_sensitivity: 0.2,
            cache_hit_history: 0.7,
            risk_level: 0.1,
        };
        // 对同一臂重复正奖励观察(驱动 θ 偏离零)
        for _ in 0..8 {
            learner
                .observe(
                    "zhipu/glm-5.2/standard",
                    ctx,
                    S9Reward {
                        success: true,
                        quality: 1.0,
                        normalized_cost: 0.0,
                        normalized_latency: 0.0,
                    },
                )
                .unwrap();
        }
        let suggestion = learner.suggest_route(ctx).unwrap();
        let weights = suggestion.policy.weights();
        assert!(weights.is_valid(), "L1 归一化契约");
        // 旧伪造行为: relevance == ctx.task_complexity(0.9)——修复后不再成立
        let (recency, frequency, relevance) = weights.as_tuple();
        assert!(
            (relevance - 0.9).abs() > 1e-3 || (frequency - 0.2).abs() > 1e-3,
            "权重由 θ 派生而非上下文复制(r={recency}, f={frequency}, rel={relevance})"
        );
        // 期望奖励点估计有界(θᵀx,学习后对该臂应为正)
        assert!(suggestion.expected_reward > -1.0 && suggestion.expected_reward < 2.0);
    }

    #[test]
    fn context_bias_dimension_present() {
        let ctx = S9Context {
            task_complexity: 0.5,
            budget_water_level: 0.5,
            latency_sensitivity: 0.5,
            cache_hit_history: 0.5,
            risk_level: 0.5,
        };
        let seam = ctx.to_seam_context().unwrap();
        assert_eq!(seam.dim(), S9_CONTEXT_DIM);
    }
}
