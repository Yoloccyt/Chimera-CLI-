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

use nexus_contracts::formal_props::VerificationResult;
use nexus_contracts::HarnessSpec;
use tracing::info;

use crate::formal::critic_monotonicity::CriticMonotonicityChecker;

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
///
/// # M0 形式化验证守卫 (P1-2)
///
/// 在 CiGate 回归门之前运行 CriticMonotonicityChecker 的三项验证:
/// 1. 单调性: 分数不随适应度提升而倒退
/// 2. 反奖励黑客: 分数提升不超适应度提升的 tolerance 倍
/// 3. 有界性: 所有分数在 [0.0, 1.0] 合法区间
///
/// 违规候选直接拒绝，无需跑昂贵的 CiGate 子进程。
#[derive(Debug, Clone)]
pub struct AegisCritic {
    /// M0 形式化验证器（单调性 + 反奖励黑客 + 有界性）
    m0_checker: CriticMonotonicityChecker,
    /// 是否启用 M0 守卫（由 GsoeConfig.enable_formal_verification 控制）
    enable_formal_verification: bool,
    /// 已接受候选的复合分数历史（用于单调性验证）
    score_history: Vec<f64>,
    /// 已接受候选的适应度历史（与 score_history 一一对应，用于单调性验证的 fitness 序列）
    ///
    /// WHY 与 score_history 分离: M0 单调性验证需要两个独立序列（fitness 和 score），
    /// 它们代表不同语义: fitness 是"客观质量"，score 是"评分"。
    /// 分离可检测"适应度下降但评分上升"的奖励黑客行为。
    fitness_history: Vec<f64>,
    /// 反奖励黑客 tolerance（评分增量/适应度增量 上限）
    reward_hacking_tolerance: f64,
}

impl Default for AegisCritic {
    fn default() -> Self {
        Self::new()
    }
}

impl AegisCritic {
    /// 创建审查器（默认启用 M0 守卫）
    pub fn new() -> Self {
        Self {
            m0_checker: CriticMonotonicityChecker::new(),
            enable_formal_verification: true,
            score_history: Vec::new(),
            fitness_history: Vec::new(),
            reward_hacking_tolerance: 2.0, // 默认 tolerance: 评分提升 ≤ 2× 适应度提升
        }
    }

    /// 创建审查器并指定 M0 守卫配置
    ///
    /// # 参数
    /// - `enable_formal_verification`: 是否启用 M0 形式化验证守卫
    /// - `reward_hacking_tolerance`: 反奖励黑客 tolerance（> 0）
    ///
    /// # 使用场景
    /// - 测试环境: `with_config(false, 2.0)` 关闭 M0 守卫
    /// - 严格模式: `with_config(true, 1.0)` 不允许任何评分放大
    pub fn with_config(enable_formal_verification: bool, reward_hacking_tolerance: f64) -> Self {
        Self {
            m0_checker: CriticMonotonicityChecker::new(),
            enable_formal_verification,
            score_history: Vec::new(),
            fitness_history: Vec::new(),
            reward_hacking_tolerance,
        }
    }

    /// 审查候选集合并裁决(幅度守护 → M0 形式化验证 → CiGate → 单变体晋级)
    ///
    /// # 三重守护顺序
    /// 1. **变异幅度守护**(零成本,先行):max_attempts ≤ 3× 基线且 ≤ 20,
    ///    backoff_ms ∈ [100, 60_000]
    /// 2. **M0 形式化验证**(纯计算 <1µs):单调性 + 反奖励黑客 + 有界性
    /// 3. **CiGate 回归门**(昂贵,前两关通过后才执行):烟雾/回归测试
    ///
    /// # 错误
    /// - `GsoeError::EvolutionFailed`:CiGate 执行本身失败(如 cargo 不可达)
    /// - `GsoeError::FormalVerificationFailed`:M0 守卫验证失败
    pub async fn select(
        &mut self,
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

            // 守护 1.5: M0 形式化验证守卫（单调性 + 反奖励黑客 + 有界性）
            // WHY 在 CiGate 之前: M0 是纯计算(<1µs)，提前过滤可节省 CiGate 子进程开销
            if let Err(e) = self.run_m0_guard(&candidate.spec, base_spec) {
                verdict.rejected.push(RejectedCandidate {
                    spec_name: name,
                    version,
                    reason: format!("M0 形式化验证失败: {e}"),
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
                // 记录接受分数到历史（M0 守卫下次验证使用）
                self.record_accepted(&candidate.spec);
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

    /// 运行 M0 形式化验证守卫
    ///
    /// 在 CiGate 回归门之前，对候选运行 CriticMonotonicityChecker 的三项验证。
    /// 若 M0 守卫关闭（`enable_formal_verification=false`），直接返回 Ok。
    ///
    /// # 可见性说明
    ///
    /// `pub` 而非 `pub(crate)`: 基准测试文件 `benches/formal_gate_eval.rs` 编译为独立
    /// 目标（非 crate lib），需要 `pub` 可见性才能调用。benchmark 测量 `run_m0_guard`
    /// 的延迟（SLO < 1µs），是性能可证伪的必要验证。
    pub fn run_m0_guard(
        &self,
        candidate: &HarnessSpec,
        base_spec: &HarnessSpec,
    ) -> Result<(), GsoeError> {
        if !self.enable_formal_verification {
            return Ok(());
        }

        let candidate_fitness = derive_fitness(candidate);
        let candidate_score = derive_fitness_score(candidate);
        let base_fitness = derive_fitness(base_spec);
        let base_score = derive_fitness_score(base_spec);

        // 构造验证序列: 历史 + 基线 + 候选
        let mut fitness_seq: Vec<f64> = Vec::new();
        let mut score_seq: Vec<f64> = Vec::new();

        // 如果历史为空，用基线作为起点
        if self.score_history.is_empty() {
            fitness_seq.push(base_fitness);
            score_seq.push(base_score);
        } else {
            // 使用 fitness_history 和 score_history 分别构造两个序列
            // WHY 分离: 适应度是"客观质量"（max_attempts 的单调函数），
            // 分数是"评分"（归一化后的复合值），两个序列独立检测单调性
            fitness_seq.extend(self.fitness_history.iter().copied());
            score_seq.extend(self.score_history.iter().copied());
        }
        fitness_seq.push(candidate_fitness);
        score_seq.push(candidate_score);

        // 1. 单调性验证
        let mono_result = self
            .m0_checker
            .verify_monotonicity(&fitness_seq, &score_seq);
        if let VerificationResult::Violated { counterexample, .. } = &mono_result {
            return Err(GsoeError::FormalVerificationFailed {
                property: "critic-monotonicity".into(),
                detail: counterexample.clone(),
            });
        }

        // 2. 反奖励黑客验证
        let hack_result = self.m0_checker.verify_no_reward_hacking(
            &fitness_seq,
            &score_seq,
            self.reward_hacking_tolerance,
        );
        if let VerificationResult::Violated { counterexample, .. } = &hack_result {
            return Err(GsoeError::FormalVerificationFailed {
                property: "anti-reward-hacking".into(),
                detail: counterexample.clone(),
            });
        }

        // 3. 有界性验证
        let bound_result = self.m0_checker.verify_score_bounded(&score_seq, 0.0, 1.0);
        if let VerificationResult::Violated { counterexample, .. } = &bound_result {
            return Err(GsoeError::FormalVerificationFailed {
                property: "score-bounded".into(),
                detail: counterexample.clone(),
            });
        }

        Ok(())
    }

    /// 记录已接受候选的分数和适应度（在接受后调用，更新历史）
    ///
    /// WHY 同时记录分数和适应度: M0 单调性验证需要两个独立序列（fitness 和 score）,
    /// 分别代表"客观质量"和"主观评分"，分离可检测适应度下降但评分上升的异常。
    fn record_accepted(&mut self, spec: &HarnessSpec) {
        let score = derive_fitness_score(spec);
        let fitness = derive_fitness(spec);
        self.score_history.push(score);
        self.fitness_history.push(fitness);
    }

    /// 返回当前分数历史长度（用于测试断言）
    #[cfg(test)]
    fn score_history_len(&self) -> usize {
        self.score_history.len()
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

/// 最大重试次数绝对值（与 MAX_ATTEMPTS_ABSOLUTE_LIMIT 一致，用于分数归一化）
const MAX_ATTEMPTS_FOR_SCORE: u32 = 20;

/// 从 HarnessSpec 的 retry 参数派生出复合适应度分数
///
/// # 分数公式
///
/// `score = 1.0 / (1.0 + max_attempts_norm)`
/// 其中 `max_attempts_norm = max_attempts / MAX_ATTEMPTS_FOR_SCORE`
///
/// # 语义
///
/// - 低重试次数 → 高分 → 好变体（说明系统更稳定，不需要多次重试）
/// - 高重试次数 → 低分 → 差变体（过度依赖重试而非修复根因）
/// - 分数范围: (0.0, 1.0]，1.0 表示 max_attempts=0（理想）
fn derive_fitness_score(spec: &HarnessSpec) -> f64 {
    let norm = spec.retry.max_attempts as f64 / MAX_ATTEMPTS_FOR_SCORE as f64;
    1.0 / (1.0 + norm)
}

/// 从 HarnessSpec 的 retry 参数派生出"适应度"（用于 M0 单调性验证的 fitness 序列）
fn derive_fitness(spec: &HarnessSpec) -> f64 {
    1.0 - (spec.retry.max_attempts as f64 / MAX_ATTEMPTS_FOR_SCORE as f64)
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
        let mut critic = AegisCritic::with_config(true, 2.0); // M0 启用
        let gate = MockCiGate::with_passing_result();
        let verdict = critic
            .select(vec![candidate_with(3, 2000)], &base_spec(), &gate) // 3 < 5: 改进
            .await
            .expect("裁决不应报错");
        assert!(verdict.accepted.is_some());
        assert!(verdict.rejected.is_empty());
    }

    #[tokio::test]
    async fn test_critic_rejects_amplify_limit_violation() {
        let mut critic = AegisCritic::new();
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
        let mut critic = AegisCritic::new();
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
        let mut critic = AegisCritic::with_config(false, 2.0); // M0 关闭: 测试 CiGate 失败路径
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
        let mut critic = AegisCritic::with_config(false, 2.0); // M0 关闭: 测试单 lineage 约束
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

    // ============================================================
    // P1-2: M0 形式化验证守卫集成测试
    // ============================================================

    #[tokio::test]
    async fn test_m0_guard_accepts_improving_candidate() {
        let mut critic = AegisCritic::with_config(true, 2.0);
        let gate = MockCiGate::with_passing_result();

        // 基线: max_attempts=5, 分数=1/(1+0.25)=0.8
        // 候选: max_attempts=2, 分数=1/(1+0.1)≈0.909
        // 分数提升 → M0 单调性满足
        let base = base_spec(); // max_attempts=5
        let candidate = candidate_with(2, 1000); // max_attempts=2, 分数更高

        let verdict = critic
            .select(vec![candidate], &base, &gate)
            .await
            .expect("裁决不应报错");

        assert!(verdict.accepted.is_some(), "改进候选应通过 M0 守卫");
        assert_eq!(critic.score_history_len(), 1, "应记录 1 个接受分数");
    }

    #[tokio::test]
    async fn test_m0_guard_accepts_consistent_worsening() {
        // M0 守卫验证的是"分数-适应度"关系的一致性，而非候选质量。
        // 当适应度和分数同步下降时（一致性行为），M0 应允许通过。
        let mut critic = AegisCritic::with_config(true, 2.0);
        let gate = MockCiGate::with_passing_result();

        // 基线: max_attempts=5, fitness=0.75, score=0.8
        // 候选: max_attempts=10, fitness=0.5, score=0.667
        // 两者同向下降 → 一致性行为 → M0 通过
        let base = base_spec(); // max_attempts=5
        let candidate = candidate_with(10, 1000); // max_attempts=10, 分数更低但一致

        let verdict = critic
            .select(vec![candidate], &base, &gate)
            .await
            .expect("裁决不应报错");

        // 候选通过 M0（一致性验证），但 CiGate 可能拒绝
        // 注意：MockCiGate 返回 passing，所以最终应接受
        assert!(verdict.accepted.is_some(), "一致性退步候选应通过 M0 守卫");
    }

    #[tokio::test]
    async fn test_m0_guard_rejects_reward_hacking_via_tolerance() {
        // 使用严格 tolerance=0.5: 评分提升不能超过适应度提升的 0.5 倍
        // 从 max_attempts=5 (fitness=0.75, score=0.8) 到 max_attempts=1 (fitness=0.95, score=0.952)
        // Δfitness=0.2, Δscore=0.152, tolerance=0.5 → 0.152 > 0.5×0.2=0.1 → 违反!
        let mut critic = AegisCritic::with_config(true, 0.5);
        let gate = MockCiGate::with_passing_result();

        let base = base_spec(); // max_attempts=5
        let candidate = candidate_with(1, 1000); // max_attempts=1, 大幅改进

        let verdict = critic
            .select(vec![candidate], &base, &gate)
            .await
            .expect("裁决不应报错");

        // tolerance=0.5 时 Δscore/Δfitness=0.76 > 0.5 → 反奖励黑客检测到违规
        assert!(verdict.accepted.is_none(), "奖励黑客应被检测");
        assert!(
            verdict.rejected[0].reason.contains("anti-reward-hacking"),
            "拒绝原因应包含反奖励黑客: {}",
            verdict.rejected[0].reason
        );
    }

    #[tokio::test]
    async fn test_m0_guard_disabled_allows_any_candidate() {
        // 关闭 M0 守卫后，即使退步候选也能通过（回到原始行为）
        let mut critic = AegisCritic::with_config(false, 2.0);
        let gate = MockCiGate::with_passing_result();

        let base = base_spec();
        let candidate = candidate_with(15, 1000); // 大幅退步

        let verdict = critic
            .select(vec![candidate], &base, &gate)
            .await
            .expect("裁决不应报错");

        // M0 关闭 + CiGate 通过 + 变异幅度合规 → 应接受
        // 注意: max_attempts=15, 基线=5, 3×5=15, 不超过 amplify limit
        assert!(verdict.accepted.is_some(), "M0 关闭时退步候选应通过");
    }

    #[tokio::test]
    async fn test_m0_guard_tracks_history_across_multiple_selections() {
        // 使用严格 tolerance=0.5 测试历史序列中的反奖励黑客检测
        let mut critic = AegisCritic::with_config(true, 0.5);
        let gate = MockCiGate::with_passing_result();

        let base = base_spec(); // max_attempts=5, fitness=0.75, score=0.8

        // 第一轮: 候选 max_attempts=3 (fitness=0.85, score=0.870)
        // Δfitness=0.1, Δscore=0.070, ratio=0.70 > 0.5 → 反奖励黑客! 被拒
        let c1 = candidate_with(3, 1000);
        let v1 = critic.select(vec![c1], &base, &gate).await.unwrap();
        assert!(v1.accepted.is_none(), "严格 tolerance 下首候选应被拒");
        assert_eq!(critic.score_history_len(), 0, "被拒不应记录历史");

        // 第二轮: 使用宽松 tolerance=2.0 接受一个候选
        let mut critic = AegisCritic::with_config(true, 2.0);
        let c2 = candidate_with(3, 1000);
        let v2 = critic.select(vec![c2], &base, &gate).await.unwrap();
        assert!(v2.accepted.is_some(), "宽松 tolerance 下候选应通过");
        assert_eq!(critic.score_history_len(), 1, "应记录 1 个分数");

        // 第三轮: 尝试更优候选 max_attempts=2 (fitness=0.9, score=0.909)
        // 历史=[0.85, 0.870], 新候选=[0.9, 0.909]
        // 序列: fitness=[0.85, 0.9], score=[0.870, 0.909]
        // 两者同向上升 → 单调性通过, 反奖励黑客: 0.039/0.05=0.78 < 2.0 → 通过
        let c3 = candidate_with(2, 1000);
        let v3 = critic.select(vec![c3], &base, &gate).await.unwrap();
        assert!(v3.accepted.is_some(), "改进候选应通过");
        assert_eq!(critic.score_history_len(), 2, "历史应增长到 2");
    }

    #[tokio::test]
    async fn test_m0_guard_detects_reward_hacking() {
        // 使用严格 tolerance=0.5: 评分提升不能超过适应度提升的 0.5 倍
        let mut critic = AegisCritic::with_config(true, 0.5);
        let gate = MockCiGate::with_passing_result();

        // 基线: max_attempts=5, fitness=0.75, score=0.8
        // 候选: max_attempts=4, fitness=0.80, score=0.833
        // Δfitness=0.05, Δscore=0.033, tolerance=0.5 → 0.033 ≤ 0.5×0.05=0.025? No!
        let base = base_spec(); // max_attempts=5
        let candidate = candidate_with(4, 1000);

        let verdict = critic
            .select(vec![candidate], &base, &gate)
            .await
            .expect("裁决不应报错");

        // tolerance=0.5 时 Δscore=0.033 ≤ 0.5×0.05=0.025 → 违反!
        assert!(verdict.accepted.is_none(), "奖励黑客应被检测");
        assert!(
            verdict.rejected[0].reason.contains("anti-reward-hacking"),
            "拒绝原因应包含反奖励黑客: {}",
            verdict.rejected[0].reason
        );
    }
}
