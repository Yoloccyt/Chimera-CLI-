//! 形式化验证器 CI 门禁 — R2 解冻阶段③ 前置 2(验证器 CI 门禁化)
//!
//! 对应架构层: L5 Knowledge(gsoe-evolution,与既有 CiGate 整合)
//! 对应 ADR: ADR-052 待办 2(验证器 CI 门禁化)+ ADR-042(R2 冻结)
//! 对应计划: R2 解冻阶段③ 前置 2
//!
//! # 职责:把 7 个 FormalVerifier 验证器聚合成 CI 强制门
//!
//! ADR-052 待办 2 要求"7 个验证器接入 CI 作为每次进化提交的强制门"。本模块
//! 将 7 个验证器的 `VerificationResult` 聚合为 `CiGateResult`,复用既有 CI 门禁
//! 基础设施(`CiFailure`/`CiFailureKind::FormalPropertyViolated`),使形式化属性
//! 违规与 test/lint/INV 失败一样阻断合并。
//!
//! # WHY 消费切片而非持有验证器实例(依赖铁律)
//!
//! 7 个验证器分布于 L1(event-bus)/L4(decay-engine)/L5(auto-dpo, gsoe 自身)/
//! L6(omega-learner)。本门禁落 gsoe-evolution(L5),若持有验证器实例需
//! `gsoe → omega-learner`(**L5→L6 向上依赖,§2.2 铁律禁止**)。因此本门禁
//! **只消费已算好的 `NamedPropertyResult` 切片**,验证器的调用与结果采集由
//! 更高层编排器(或 CI/E2E 测试层,dev-dep 可依赖任意 crate)完成后向下喂送。
//! 这与 decay-engine `ShadowModeCircuitBreaker`(前置 3)的"消费切片"模式一致。
//!
//! # fail-closed 聚合语义(与前置 3 熔断器对齐)
//!
//! - **任一 `Violated`** → 门禁失败,每个违规映射为一条 `CiFailure`
//! - **Satisfied 数 < `require_min_satisfied`** → 门禁失败(正面证据不足)
//! - **否则** → 门禁通过
//!
//! # R2 冻结声明(ADR-042)
//!
//! 纯聚合逻辑,无 RL 训练,不含 5 个 R2 扫描关键词。是解冻前的质量门基建,
//! 门禁本身**收紧**安全约束(违规即阻断),不解冻。

use nexus_contracts::formal_props::VerificationResult;

use crate::ci_gate::{CiFailure, CiFailureKind, CiGateResult};

/// 默认要求的最小 Satisfied 属性数 — 至少一个正面验证证据才放行
///
/// WHY ≥1:与前置 3 熔断器的"正面证据原则"一致——"没发现违规"(全 Skipped)
/// 不等于"通过验证",fail-closed 下需至少一个 Satisfied 才算有效验证。
pub const DEFAULT_MIN_SATISFIED: usize = 1;

/// 具名属性验证结果 — CI 门禁的输入单元
///
/// 由上层编排器采集单个验证器输出后构造:`property` 为属性标识
/// (如 "decay-consistency" / "invariant-closure"),`result` 为其验证结果。
#[derive(Debug, Clone, PartialEq)]
pub struct NamedPropertyResult {
    /// 属性标识(人类可读,进入失败诊断)
    pub property: String,
    /// 该属性的验证结果
    pub result: VerificationResult,
}

impl NamedPropertyResult {
    /// 构造具名属性结果
    pub fn new(property: impl Into<String>, result: VerificationResult) -> Self {
        Self {
            property: property.into(),
            result,
        }
    }
}

/// 形式化验证器聚合摘要 — 三态计数(供报告与审计)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormalGateSummary {
    /// Satisfied 属性数
    pub satisfied: usize,
    /// Violated 属性数
    pub violated: usize,
    /// Skipped 属性数
    pub skipped: usize,
    /// 总属性数
    pub total: usize,
}

/// 形式化验证器 CI 门禁 — 聚合 N 个属性验证结果为 CI 裁决
///
/// 同步聚合器(区别于 async 的 `CargoCiGate`):输入已算好的验证结果切片,
/// 产出 `CiGateResult`,可与 `CargoCiGate` 的结果合并为完整 CI 裁决。
#[derive(Debug, Clone, Copy)]
pub struct FormalVerifierGate {
    /// 放行要求的最小 Satisfied 属性数(正面证据门槛)
    require_min_satisfied: usize,
}

impl Default for FormalVerifierGate {
    fn default() -> Self {
        Self::new()
    }
}

impl FormalVerifierGate {
    /// 创建门禁(默认要求 ≥1 Satisfied)
    pub fn new() -> Self {
        Self {
            require_min_satisfied: DEFAULT_MIN_SATISFIED,
        }
    }

    /// 创建门禁并指定最小 Satisfied 门槛
    ///
    /// WHY 可配置:解冻不同阶段可要求不同数量的正面证据
    /// (如影子模式初期要求全部 7 属性 Satisfied)。
    pub fn with_min_satisfied(min_satisfied: usize) -> Self {
        Self {
            require_min_satisfied: min_satisfied,
        }
    }

    /// 统计三态摘要
    #[must_use]
    pub fn summarize(&self, results: &[NamedPropertyResult]) -> FormalGateSummary {
        let mut satisfied = 0;
        let mut violated = 0;
        let mut skipped = 0;
        for r in results {
            match r.result {
                VerificationResult::Satisfied { .. } => satisfied += 1,
                VerificationResult::Violated { .. } => violated += 1,
                VerificationResult::Skipped { .. } => skipped += 1,
            }
        }
        FormalGateSummary {
            satisfied,
            violated,
            skipped,
            total: results.len(),
        }
    }

    /// 聚合验证结果 → `CiGateResult`(CI 强制门裁决)
    ///
    /// # 判定(fail-closed)
    /// 1. 每个 `Violated` 属性 → 一条 `CiFailure(FormalPropertyViolated)`
    /// 2. Satisfied 数 < `require_min_satisfied` → 追加一条"证据不足"失败
    /// 3. failures 为空 → `passed=true`;否则 `passed=false`
    ///
    /// # 参数
    /// - `results`: 具名属性验证结果切片(7 验证器输出)
    ///
    /// # 返回
    /// `CiGateResult`(可与 CargoCiGate 结果合并)。`regression_streak` 恒 0
    /// (形式化门禁非回归性质,streak 由 bench 回归维护)。
    #[must_use]
    pub fn evaluate(&self, results: &[NamedPropertyResult]) -> CiGateResult {
        let summary = self.summarize(results);
        let mut failures: Vec<CiFailure> = Vec::new();

        // 1. 每个违规属性映射为一条 CI 失败(携带属性名 + 反例)
        for r in results {
            if let VerificationResult::Violated { counterexample, .. } = &r.result {
                failures.push(CiFailure::new(
                    CiFailureKind::FormalPropertyViolated,
                    format!("属性 '{}' 被违反: {counterexample}", r.property),
                ));
            }
        }

        // 2. 正面证据门槛:Satisfied 数不足则门禁失败(fail-closed)
        if summary.satisfied < self.require_min_satisfied {
            failures.push(CiFailure::new(
                CiFailureKind::FormalPropertyViolated,
                format!(
                    "正面验证证据不足: Satisfied {} < 要求 {}(共 {} 属性,{} skipped)",
                    summary.satisfied, self.require_min_satisfied, summary.total, summary.skipped
                ),
            ));
        }

        CiGateResult::failed_with(failures, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn satisfied() -> VerificationResult {
        VerificationResult::Satisfied { samples_tested: 10 }
    }

    fn violated(msg: &str) -> VerificationResult {
        VerificationResult::Violated {
            counterexample: msg.to_string(),
            samples_tested: 3,
        }
    }

    fn skipped() -> VerificationResult {
        VerificationResult::Skipped {
            reason: "前置不满足".to_string(),
        }
    }

    fn named(property: &str, result: VerificationResult) -> NamedPropertyResult {
        NamedPropertyResult::new(property, result)
    }

    #[test]
    fn test_all_satisfied_passes() {
        let gate = FormalVerifierGate::new();
        let results = [
            named("decay-consistency", satisfied()),
            named("invariant-closure", satisfied()),
            named("learning-monotonicity", satisfied()),
        ];
        let verdict = gate.evaluate(&results);
        assert!(verdict.passed, "全 Satisfied 应通过门禁");
        assert!(verdict.failures.is_empty());
    }

    #[test]
    fn test_any_violated_fails_with_property_name() {
        let gate = FormalVerifierGate::new();
        let results = [
            named("decay-consistency", satisfied()),
            named("invariant-closure", violated("检测到有向环")),
        ];
        let verdict = gate.evaluate(&results);
        assert!(!verdict.passed, "任一违规应门禁失败");
        assert_eq!(verdict.failures.len(), 1);
        assert_eq!(
            verdict.failures[0].kind,
            CiFailureKind::FormalPropertyViolated
        );
        assert!(verdict.failures[0].message.contains("invariant-closure"));
        assert!(verdict.failures[0].message.contains("检测到有向环"));
    }

    #[test]
    fn test_multiple_violations_each_reported() {
        let gate = FormalVerifierGate::new();
        let results = [
            named("p1", violated("v1")),
            named("p2", satisfied()),
            named("p3", violated("v3")),
        ];
        let verdict = gate.evaluate(&results);
        assert!(!verdict.passed);
        // 两条违规各一条失败(证据充足,不追加证据不足)
        assert_eq!(verdict.failures.len(), 2);
    }

    #[test]
    fn test_all_skipped_fails_insufficient_evidence() {
        let gate = FormalVerifierGate::new();
        let results = [named("p1", skipped()), named("p2", skipped())];
        let verdict = gate.evaluate(&results);
        // 无违规但无正面证据 → fail-closed 失败
        assert!(!verdict.passed, "全 Skipped 证据不足应失败");
        assert_eq!(verdict.failures.len(), 1);
        assert!(verdict.failures[0].message.contains("证据不足"));
    }

    #[test]
    fn test_empty_fails_insufficient_evidence() {
        let gate = FormalVerifierGate::new();
        let verdict = gate.evaluate(&[]);
        assert!(!verdict.passed, "空输入 fail-closed 失败");
    }

    #[test]
    fn test_satisfied_with_skipped_passes() {
        let gate = FormalVerifierGate::new();
        // 有正面证据(1 Satisfied)且无违规 → 通过
        let results = [named("p1", satisfied()), named("p2", skipped())];
        assert!(gate.evaluate(&results).passed);
    }

    #[test]
    fn test_min_satisfied_threshold() {
        // 要求 ≥3 Satisfied,但只有 2 个 → 失败
        let gate = FormalVerifierGate::with_min_satisfied(3);
        let results = [
            named("p1", satisfied()),
            named("p2", satisfied()),
            named("p3", skipped()),
        ];
        let verdict = gate.evaluate(&results);
        assert!(!verdict.passed, "Satisfied 2 < 要求 3 应失败");
    }

    #[test]
    fn test_violation_and_insufficient_both_reported() {
        // 要求 ≥2 Satisfied:1 违规 + 1 Satisfied + 1 Skipped → 违规失败 + 证据不足失败
        let gate = FormalVerifierGate::with_min_satisfied(2);
        let results = [
            named("p1", violated("bad")),
            named("p2", satisfied()),
            named("p3", skipped()),
        ];
        let verdict = gate.evaluate(&results);
        assert!(!verdict.passed);
        // 1 条违规 + 1 条证据不足(Satisfied 1 < 2)
        assert_eq!(verdict.failures.len(), 2);
    }

    #[test]
    fn test_summarize_counts() {
        let gate = FormalVerifierGate::new();
        let results = [
            named("p1", satisfied()),
            named("p2", satisfied()),
            named("p3", violated("x")),
            named("p4", skipped()),
        ];
        let s = gate.summarize(&results);
        assert_eq!(s.satisfied, 2);
        assert_eq!(s.violated, 1);
        assert_eq!(s.skipped, 1);
        assert_eq!(s.total, 4);
    }

    #[test]
    fn test_seven_property_matrix_all_pass() {
        // 模拟 7 属性全 Satisfied(M0 谱系/Critic + M1 偏好/事件/学习 + M2 衰减/闭包)
        let gate = FormalVerifierGate::new();
        let props = [
            "lineage-dag",
            "critic-monotonicity",
            "preference-consistency",
            "causal-consistency",
            "learning-monotonicity",
            "decay-consistency",
            "invariant-closure",
        ];
        let results: Vec<NamedPropertyResult> =
            props.iter().map(|p| named(p, satisfied())).collect();
        let verdict = gate.evaluate(&results);
        assert!(verdict.passed, "7 属性全 Satisfied 应通过 CI 门禁");
        assert_eq!(gate.summarize(&results).satisfied, 7);
    }
}
