//! AEGIS Stage 4: AegisCritic — 变体候选审查与裁决
//!
//! 对应 ADR:ADR-050 决策 2(Critic 复用 CiGate)+ 决策 4(奖励欺骗启发式)
//!
//! # R2 冻结声明(ADR-042)
//! 本阶段复用既有 CiGate 回归门 + 变异幅度硬上限启发式:
//! 无神经评估、无梯度更新(FormalVerifier 落地前无条件冻结)。
//!
//! # 三重守护顺序
//! 1. **变异幅度守护**(先行,零成本):max_attempts ≤ 3× 基线且 ≤ 20,
//!    backoff_ms ∈ [100, 60_000] — 防奖励欺骗(reward hacking)
//! 2. **CiGate 回归门**(昂贵,幅度守护通过后才执行):烟雾/回归测试
//! 3. **单变体晋级**:多候选通过时仅取首个(单 lineage 更新,对齐 ADR-032)

use nexus_contracts::HarnessSpec;
use tracing::info;

use super::evolver::SpecCandidate;
use crate::ci_gate::CiGate;
use crate::error::GsoeError;

/// max_attempts 相对基线的最大放大倍数(ADR-050 决策 4)
const MAX_ATTEMPTS_AMPLIFY_LIMIT: u32 = 3;

/// max_attempts 绝对上限(ADR-050 决策 4)
const MAX_ATTEMPTS_ABSOLUTE_LIMIT: u32 = 20;

/// backoff_ms 合法区间(ADR-050 决策 4)
const BACKOFF_MS_RANGE: (u64, u64) = (100, 60_000);

/// 被拒绝的候选记录 — 供审计追溯拒绝原因
#[derive(Debug, Clone)]
pub struct RejectedCandidate {
    /// 候选 spec 的名称与版本(不保留完整 spec,降低裁决对象体积)
    pub spec_name: String,
    /// 候选版本号
    pub version: u32,
    /// 拒绝原因(人类可读)
    pub reason: String,
}

/// Critic 裁决 — 流水线最终输出
#[derive(Debug, Clone)]
pub struct CriticVerdict {
    /// 被接受的变体(≤1 个,单 lineage 更新)
    pub accepted: Option<HarnessSpec>,
    /// 被拒绝的候选清单(含拒绝原因)
    pub rejected: Vec<RejectedCandidate>,
}

impl CriticVerdict {
    /// 空裁决(无候选输入时)
    pub fn empty() -> Self {
        Self {
            accepted: None,
            rejected: Vec::new(),
        }
    }
}

/// 变体审查器 — Stage 4
#[derive(Debug, Default, Clone, Copy)]
pub struct AegisCritic;

impl AegisCritic {
    /// 创建审查器
    pub fn new() -> Self {
        Self
    }

    /// 审查候选集合并裁决(幅度守护 → CiGate → 单变体晋级)
    ///
    /// # 错误
    /// - `GsoeError::EvolutionFailed`:CiGate 执行本身失败(如 cargo 不可达)
    pub async fn select(
        &self,
        candidates: Vec<SpecCandidate>,
        base_spec: &HarnessSpec,
        ci_gate: &dyn CiGate,
    ) -> Result<CriticVerdict, GsoeError> {
        let mut verdict = CriticVerdict::empty();

        for candidate in candidates {
            let name = candidate.spec.meta.name.clone();
            let version = candidate.spec.meta.version;

            // 守护 1:变异幅度硬上限(零成本先行,防奖励欺骗)
            if let Err(reason) = check_mutation_bounds(&candidate.spec, base_spec) {
                verdict.rejected.push(RejectedCandidate {
                    spec_name: name,
                    version,
                    reason,
                });
                continue;
            }

            // 已有接受变体时,后续候选直接拒绝(单 lineage 更新,不再跑昂贵 CI)
            if verdict.accepted.is_some() {
                verdict.rejected.push(RejectedCandidate {
                    spec_name: name,
                    version,
                    reason: "本轮已有接受变体(单 lineage 更新约束)".into(),
                });
                continue;
            }

            // 守护 2:CiGate 回归门(烟雾/回归测试)
            let ci_result = ci_gate.execute(&candidate.spec).await.map_err(|e| {
                GsoeError::CiGateExecutionFailed {
                    reason: format!("AEGIS Critic 调用 CiGate 失败: {e}"),
                }
            })?;

            if ci_result.passed {
                info!(
                    spec = %candidate.spec.meta.name,
                    version = candidate.spec.meta.version,
                    rationale = %candidate.rationale,
                    "AEGIS Critic 接受变体候选"
                );
                verdict.accepted = Some(candidate.spec);
            } else {
                let failures: Vec<String> = ci_result
                    .failures
                    .iter()
                    .map(|f| f.kind.as_str().to_string())
                    .collect();
                verdict.rejected.push(RejectedCandidate {
                    spec_name: name,
                    version,
                    reason: format!("CiGate 回归门失败: {}", failures.join(", ")),
                });
            }
        }

        Ok(verdict)
    }
}

/// 变异幅度守护 — 奖励欺骗启发式(ADR-050 决策 4)
///
/// WHY 硬上限:进化器若能无限放大重试参数,可通过"重试到成功"游戏化
/// 成功率指标而不修复根因。上限迫使进化收益来自真实的参数适配。
fn check_mutation_bounds(candidate: &HarnessSpec, base: &HarnessSpec) -> Result<(), String> {
    let attempts = candidate.retry.max_attempts;
    let amplify_limit = base
        .retry
        .max_attempts
        .saturating_mul(MAX_ATTEMPTS_AMPLIFY_LIMIT);

    if attempts > amplify_limit {
        return Err(format!(
            "max_attempts {attempts} 超过基线 {} 的 {MAX_ATTEMPTS_AMPLIFY_LIMIT}× 上限(奖励欺骗守护)",
            base.retry.max_attempts
        ));
    }
    if attempts > MAX_ATTEMPTS_ABSOLUTE_LIMIT {
        return Err(format!(
            "max_attempts {attempts} 超过绝对上限 {MAX_ATTEMPTS_ABSOLUTE_LIMIT}(奖励欺骗守护)"
        ));
    }
    let backoff = candidate.retry.backoff_ms;
    if backoff < BACKOFF_MS_RANGE.0 || backoff > BACKOFF_MS_RANGE.1 {
        return Err(format!(
            "backoff_ms {backoff} 越出合法区间 [{}, {}]",
            BACKOFF_MS_RANGE.0, BACKOFF_MS_RANGE.1
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ci_gate::{CiFailure, CiFailureKind, MockCiGate};
    use nexus_contracts::{HarnessMeta, RetryPolicy};

    fn base_spec() -> HarnessSpec {
        HarnessSpec {
            meta: HarnessMeta {
                name: "critic-test".into(),
                version: 1,
                immutable: false,
                parent: None,
                task_type: None,
            },
            contracts: vec![],
            hops: vec![],
            retry: RetryPolicy::default(), // max_attempts=5, backoff_ms=1000
            auxiliary: None,
        }
    }

    fn candidate_with(max_attempts: u32, backoff_ms: u64) -> SpecCandidate {
        let mut spec = base_spec();
        spec.meta.version = 2;
        spec.meta.parent = Some(1);
        spec.retry.max_attempts = max_attempts;
        spec.retry.backoff_ms = backoff_ms;
        SpecCandidate {
            spec,
            rationale: "test".into(),
        }
    }

    #[tokio::test]
    async fn test_critic_accepts_bounded_candidate_passing_ci() {
        let critic = AegisCritic::new();
        let gate = MockCiGate::with_passing_result();
        let verdict = critic
            .select(vec![candidate_with(10, 2000)], &base_spec(), &gate)
            .await
            .expect("裁决不应报错");
        assert!(verdict.accepted.is_some());
        assert!(verdict.rejected.is_empty());
    }

    #[tokio::test]
    async fn test_critic_rejects_amplify_limit_violation() {
        let critic = AegisCritic::new();
        let gate = MockCiGate::with_passing_result();
        // 16 > 5 × 3 = 15 → 奖励欺骗守护拒绝(CI 门不应被调用)
        let verdict = critic
            .select(vec![candidate_with(16, 1000)], &base_spec(), &gate)
            .await
            .expect("裁决不应报错");
        assert!(verdict.accepted.is_none());
        assert_eq!(verdict.rejected.len(), 1);
        assert!(verdict.rejected[0].reason.contains("奖励欺骗守护"));
    }

    #[tokio::test]
    async fn test_critic_rejects_backoff_out_of_range() {
        let critic = AegisCritic::new();
        let gate = MockCiGate::with_passing_result();
        let verdict = critic
            .select(vec![candidate_with(10, 61_000)], &base_spec(), &gate)
            .await
            .expect("裁决不应报错");
        assert!(verdict.accepted.is_none());
        assert!(verdict.rejected[0].reason.contains("backoff_ms"));
    }

    #[tokio::test]
    async fn test_critic_rejects_on_ci_gate_failure() {
        let critic = AegisCritic::new();
        let gate = MockCiGate::with_failing_result(vec![CiFailure::new(
            CiFailureKind::TestFailed,
            "smoke test broke",
        )]);
        let verdict = critic
            .select(vec![candidate_with(10, 2000)], &base_spec(), &gate)
            .await
            .expect("裁决不应报错");
        assert!(verdict.accepted.is_none());
        assert!(verdict.rejected[0].reason.contains("CiGate 回归门失败"));
    }

    #[tokio::test]
    async fn test_critic_single_lineage_update() {
        let critic = AegisCritic::new();
        let gate = MockCiGate::with_passing_result();
        // 两个合法候选:仅首个晋级,次个因单 lineage 约束被拒
        let verdict = critic
            .select(
                vec![candidate_with(10, 2000), candidate_with(2, 1000)],
                &base_spec(),
                &gate,
            )
            .await
            .expect("裁决不应报错");
        assert!(verdict.accepted.is_some());
        assert_eq!(verdict.rejected.len(), 1);
        assert!(verdict.rejected[0].reason.contains("单 lineage"));
    }
}
