//! AEGIS Stage 3: SpecEvolver — HarnessSpec 参数变体生成
//!
//! 对应 ADR:ADR-050 决策 2(Evolver 降级为仅生成 Spec 参数变体,不生成代码)
//!
//! # R2 冻结声明(ADR-042)
//! 本阶段仅在预定义方向上变异 `RetryPolicy` 参数并生成新版本 HarnessSpec:
//! 无 CodeGenerator、无代码自修改、无梯度更新(FormalVerifier 落地前无条件冻结)。
//!
//! # 可进化面边界(ADR-050 否决方案 3)
//! 首期仅开放 `RetryPolicy`(max_attempts / backoff_ms);
//! HopSpec 流程拓扑与 ContractSpec 契约不可变异(回归面不可控,推迟)。

use nexus_contracts::HarnessSpec;
use tracing::debug;

use super::planner::{AdaptationDirection, AdaptationPlan};

/// 重试放宽系数 — RelaxRetries 时 max_attempts 的乘法因子
///
/// WHY 2×:单步放宽幅度保守(5 → 10),配合 Critic 的 3× 硬上限
/// (ADR-050 决策 4),两代内不会触顶,给显著性检测留观察窗口。
const RELAX_FACTOR: u32 = 2;

/// 重试收紧下限 — TightenRetries 时 max_attempts 不低于此值
///
/// WHY 1:至少保留一次尝试,0 次尝试等价于禁用该 hop(语义越界)。
const TIGHTEN_FLOOR: u32 = 1;

/// 变体候选 — Stage 4 Critic 的输入
#[derive(Debug, Clone)]
pub struct SpecCandidate {
    /// 变体 spec(版本已递增,parent 已指向基线)
    pub spec: HarnessSpec,
    /// 变异依据(继承自 AdaptationPlan.rationale + 变异明细)
    pub rationale: String,
}

/// 变体生成器 — Stage 3
#[derive(Debug, Default, Clone, Copy)]
pub struct SpecEvolver;

impl SpecEvolver {
    /// 创建变体生成器
    pub fn new() -> Self {
        Self
    }

    /// 依据适应计划生成变体候选
    ///
    /// # 守护规则
    /// - `base_spec.meta.immutable == true` → 直接返回空(不可进化面,ADR-050 决策 2)
    /// - 生成的变体必须通过 `spec.validate()`,失败即丢弃并记日志
    /// - NoChange 方向不生成候选
    pub fn generate(&self, plan: &AdaptationPlan, base_spec: &HarnessSpec) -> Vec<SpecCandidate> {
        // 不可进化面守护:immutable spec 拒绝任何变异
        if base_spec.meta.immutable {
            debug!(
                spec = %base_spec.meta.name,
                "基线 spec 标记 immutable,跳过变体生成"
            );
            return Vec::new();
        }

        plan.directions
            .iter()
            .filter_map(|direction| self.mutate_one(*direction, plan, base_spec))
            .collect()
    }

    /// 按单一方向变异基线 spec(NoChange 与非法变体返回 None)
    fn mutate_one(
        &self,
        direction: AdaptationDirection,
        plan: &AdaptationPlan,
        base_spec: &HarnessSpec,
    ) -> Option<SpecCandidate> {
        let mut spec = base_spec.clone();
        // 谱系维护:版本递增,parent 指向基线(SpecRegistry lineage 依赖此约定)
        spec.meta.version = base_spec.meta.version + 1;
        spec.meta.parent = Some(base_spec.meta.version);

        let mutation_detail = match direction {
            AdaptationDirection::RelaxRetries => {
                spec.retry.max_attempts = base_spec.retry.max_attempts.saturating_mul(RELAX_FACTOR);
                // backoff 同步放宽:瞬态故障需要更长恢复窗口
                spec.retry.backoff_ms = base_spec.retry.backoff_ms.saturating_mul(2);
                format!(
                    "max_attempts {} → {},backoff_ms {} → {}",
                    base_spec.retry.max_attempts,
                    spec.retry.max_attempts,
                    base_spec.retry.backoff_ms,
                    spec.retry.backoff_ms
                )
            }
            AdaptationDirection::TightenRetries => {
                spec.retry.max_attempts = (base_spec.retry.max_attempts / 2).max(TIGHTEN_FLOOR);
                format!(
                    "max_attempts {} → {}",
                    base_spec.retry.max_attempts, spec.retry.max_attempts
                )
            }
            AdaptationDirection::NoChange => return None,
        };

        // 变体必须通过契约校验(ImmutableSurface / 谱系单调性等)
        if let Err(e) = spec.validate() {
            debug!(error = %e, "变体校验失败,丢弃候选");
            return None;
        }

        Some(SpecCandidate {
            spec,
            rationale: format!("{};变异:{}", plan.rationale, mutation_detail),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::{HarnessMeta, RetryPolicy};

    fn base_spec() -> HarnessSpec {
        HarnessSpec {
            meta: HarnessMeta {
                name: "evolver-test".into(),
                version: 3,
                immutable: false,
                parent: Some(2),
                task_type: None,
            },
            contracts: vec![],
            hops: vec![],
            retry: RetryPolicy::default(), // max_attempts=5, backoff_ms=1000
            auxiliary: None,
        }
    }

    fn plan_with(direction: AdaptationDirection) -> AdaptationPlan {
        AdaptationPlan {
            directions: vec![direction],
            rationale: "test".into(),
        }
    }

    #[test]
    fn test_relax_retries_doubles_attempts_and_backoff() {
        let candidates = SpecEvolver::new()
            .generate(&plan_with(AdaptationDirection::RelaxRetries), &base_spec());
        assert_eq!(candidates.len(), 1);
        let spec = &candidates[0].spec;
        assert_eq!(spec.retry.max_attempts, 10);
        assert_eq!(spec.retry.backoff_ms, 2000);
        // 谱系:版本 3 → 4,parent = 3
        assert_eq!(spec.meta.version, 4);
        assert_eq!(spec.meta.parent, Some(3));
    }

    #[test]
    fn test_tighten_retries_halves_attempts_with_floor() {
        let candidates = SpecEvolver::new().generate(
            &plan_with(AdaptationDirection::TightenRetries),
            &base_spec(),
        );
        assert_eq!(candidates[0].spec.retry.max_attempts, 2);

        // 下限守护:max_attempts=1 收紧后仍为 1
        let mut floor_spec = base_spec();
        floor_spec.retry.max_attempts = 1;
        let candidates = SpecEvolver::new()
            .generate(&plan_with(AdaptationDirection::TightenRetries), &floor_spec);
        assert_eq!(candidates[0].spec.retry.max_attempts, TIGHTEN_FLOOR);
    }

    #[test]
    fn test_no_change_produces_no_candidate() {
        let candidates =
            SpecEvolver::new().generate(&plan_with(AdaptationDirection::NoChange), &base_spec());
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_immutable_spec_rejects_mutation() {
        let mut immutable_spec = base_spec();
        immutable_spec.meta.immutable = true;
        let candidates = SpecEvolver::new().generate(
            &plan_with(AdaptationDirection::RelaxRetries),
            &immutable_spec,
        );
        // 不可进化面守护:immutable 基线不产出任何变体
        assert!(candidates.is_empty());
    }
}
