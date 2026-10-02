//! 人机一致率校准契约 — 评测层「一致率锚点」（v2.3 §(b.6)）
//!
//! 对应架构层: **L0 Contracts**（纯类型 + 无 IO 纯函数，ADR-033 先例）
//! 对应设计源: `Chimera架构重构与优化方案_v2.3` §(b.6)「校准动作」
//!
//! # WHY
//! §(b.6) 主线④「评测即基础设施」：机器评测须以**人机一致率**为锚点。抽样门判定
//! 结果与人审对照，持续度量一致率（阈值参考美团 ≥90%）；**低于阈值时相关门的自动化
//! 结果降级为 Claim 而非 Certify**（与 §(b.1) 裁决权分离闭环：没被验证的不得当作已认证）。
//! 本模块提供该一致率的**数据形状 + 纯计算 + 阈值判定**契约，供 efficiency-monitor
//! RuntimeAuditor 与评测月报消费；不含 IO/采样调度（那是 owner crate 的运行面）。
//!
//! 纯类型 + 纯函数，零 crate 依赖（serde derive 除外）。

use serde::{Deserialize, Serialize};

/// 自动化门/审查器对某样本的判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateVerdict {
    /// 门判定「通过/无问题」
    Pass,
    /// 门判定「失败/有问题」
    Fail,
}

/// 人工对同一样本的标注（视为真值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HumanLabel {
    /// 人审：门的「通过」是对的
    Correct,
    /// 人审：门判错了
    Incorrect,
}

/// 一个对照样本：门判定 vs 人工标注。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationSample {
    /// 自动化门判定
    pub gate: GateVerdict,
    /// 人工标注真值
    pub human: HumanLabel,
}

impl CalibrationSample {
    /// 门与人工是否一致：Pass↔Correct、Fail↔Incorrect 视为一致。
    #[must_use]
    pub fn agrees(&self) -> bool {
        matches!(
            (self.gate, self.human),
            (GateVerdict::Pass, HumanLabel::Correct) | (GateVerdict::Fail, HumanLabel::Incorrect)
        )
    }
}

/// 默认可信阈值：一致率千分比 ≥ 900（= 90%，参考美团评测锚点）。
pub const DEFAULT_AGREEMENT_THRESHOLD_MILLI: u32 = 900;

/// 一致率（千分比，0–1000）。空样本返回 0（无数据 ⇒ 不可信，保守降级为 Claim）。
#[must_use]
pub fn agreement_rate_milli(samples: &[CalibrationSample]) -> u32 {
    if samples.is_empty() {
        return 0;
    }
    let agreed = samples.iter().filter(|s| s.agrees()).count();
    (agreed as u64 * 1000 / samples.len() as u64) as u32
}

// ============================================================
// 样本库与 FailureRegistry 共库契约（§(b.6)「样本库与 FailureRegistry 共库」）
// 统一记录形状：两类样本同一 schema 落库，Bad Case 转化率表（月报表二）
// 直接复用 FailureRegistered 既有的 `fix_status` 字段，不另造状态机。
// ============================================================

/// 共库样本记录的来源判别：同一存储行承载两类评测样本。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvalSampleKind {
    /// 门判定 vs 人审的校准样本（一致率/可认证性证据）
    GateCalibration,
    /// FailureRegistry 踩坑样本（修复状态 = Bad Case 转化链证据）
    FailureRegistry,
}

/// 共库统一记录——由 `CalibrationSample` 或 FailureRegistered 事件字段归一而来。
///
/// WHY 单形而非双表：§(b.6) 要求两者共库；评测月报的「一致率趋势」（表一）
/// 与「Bad Case 转化率」（表二）在同一条落库流上聚合，避免双源漂移。
/// `status` 按 kind 取域：GateCalibration 存 `"correct"`/`"incorrect"`，
/// FailureRegistry 存 [`FailureFixStatus`] 变体名（`"Open"`/`"Mitigated"`/
/// `"Resolved"`/`"WontFix"`，与既有字段取值域严格同源，不另造状态机）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvalRecord {
    /// 判别键：校准样本用门/命题标识；失败样本用失败签名
    pub key: String,
    /// 受影响组件（crate 名/模块路径；校准样本可为空串）
    pub component: String,
    /// 来源类型
    pub kind: EvalSampleKind,
    /// 状态字符串（按 kind 取域，见类型文档）
    pub status: String,
}

impl EvalRecord {
    /// 由校准样本构造（verdict 归一为 correct/incorrect）。
    #[must_use]
    pub fn from_calibration(key: &str, component: &str, sample: &CalibrationSample) -> Self {
        Self {
            key: key.to_string(),
            component: component.to_string(),
            kind: EvalSampleKind::GateCalibration,
            status: if sample.agrees() {
                "correct".to_string()
            } else {
                "incorrect".to_string()
            },
        }
    }

    /// 由 FailureRegistered 事件三字段构造（签名/组件/修复状态原样入库）。
    #[must_use]
    pub fn from_failure(signature: &str, affected_component: &str, fix_status: &str) -> Self {
        Self {
            key: signature.to_string(),
            component: affected_component.to_string(),
            kind: EvalSampleKind::FailureRegistry,
            status: fix_status.to_string(),
        }
    }

    /// 由强类型 [`FailureRecord`] 构造（`fix_status` 取变体名，与事件面
    /// `FailureRegistered.fix_status: String` 的发射惯例同源）。
    #[must_use]
    pub fn from_record(record: &crate::adjudication::FailureRecord) -> Self {
        let status = match record.fix_status {
            crate::adjudication::FailureFixStatus::Open => "Open",
            crate::adjudication::FailureFixStatus::Mitigated => "Mitigated",
            crate::adjudication::FailureFixStatus::Resolved => "Resolved",
            crate::adjudication::FailureFixStatus::WontFix => "WontFix",
        };
        Self::from_failure(&record.signature, &record.affected_component, status)
    }

    /// Bad Case 已转化（根因消除）判定：仅对 FailureRegistry 行有意义。
    ///
    /// 只认 `Resolved`（`FailureFixStatus` 唯一 completed 态）；`WontFix` 是
    /// 设计接受边界、`Mitigated` 根因未除，均不计转化（不虚抬转化率）。
    #[must_use]
    pub fn is_resolved(&self) -> bool {
        self.kind == EvalSampleKind::FailureRegistry && self.status == "Resolved"
    }
}

/// 门「可信」判据：一致率达标 ⇒ 其自动化结果可作为 Certify 依据；否则只能作 Claim。
#[must_use]
pub fn certifiable(samples: &[CalibrationSample], threshold_milli: u32) -> bool {
    agreement_rate_milli(samples) >= threshold_milli
}

/// 四级裁判链（v2.3 §(b.6)「四级裁判链预留」：Rubric → Human → LLM-Judge → RM 蒸馏）。
///
/// 顺序 = 可信度/成本递增。本枚举为契约**预留**——只定义数据形状与 RM 触发判据，
/// 不含任何调度/推理 IO（那是 owner 运行面职责）。RM（奖励模型蒸馏裁判）**条件触发**：
/// 仅当一致率长期达标且 LLM-Judge↔Human 对照样本累积充足时，才允许蒸馏出 RM
/// 以替代部分人审（降本）。未达标前不得启用 RM（§(f)「评测无锚点自动化」红线）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JudgeTier {
    /// 量规打分（规则/清单，零模型成本，最廉价）
    Rubric,
    /// 人工标注（真值锚点，见 [`HumanLabel`]）
    Human,
    /// LLM 裁判（可扩展，但须经一致率校准）
    LlmJudge,
    /// 奖励模型蒸馏裁判（由 LLM-Judge + Human 样本蒸馏，条件触发）
    RewardModel,
}

/// RM 蒸馏触发判据（§(b.6)「RM 条件触发」）：一致率(‰)达阈 **且**
/// 累计 LLM-Judge↔Human 对照标注样本数达最小量，方可启用 [`JudgeTier::RewardModel`]。
///
/// 两条件缺一即 `false`（样本不足时蒸馏会固化噪声；一致率未达标时蒸馏无信任基础）。
#[must_use]
pub fn rm_distillation_eligible(
    agreement_milli: u32,
    threshold_milli: u32,
    labeled_samples: usize,
    min_samples: usize,
) -> bool {
    agreement_milli >= threshold_milli && labeled_samples >= min_samples
}

// ============================================================
// 评测月报三表（§(b.6) 验收「评测月报三表：一致率趋势 / Bad Case 转化率 / 门覆盖率」）
// 纯数据形状 + 无 IO 聚合；供评测层运行面填充、供 §(b.6) 验收反查。
// ============================================================

/// 表一：一致率趋势点 — 某评测周期的人机一致率（千分比）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgreementTrendPoint {
    /// 周期标识（如 `2026-10`）
    pub period: String,
    /// 该周期一致率（千分比 0–1000）
    pub agreement_milli: u32,
}

impl AgreementTrendPoint {
    /// 由某周期的校准样本集构造趋势点（复用 [`agreement_rate_milli`]）。
    #[must_use]
    pub fn from_samples(period: &str, samples: &[CalibrationSample]) -> Self {
        Self {
            period: period.to_string(),
            agreement_milli: agreement_rate_milli(samples),
        }
    }
}

/// 表二：Bad Case 转化率 — 检测→修复→回退的漏斗计数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BadCaseConversion {
    /// 检出的 Bad Case 总数
    pub detected: u32,
    /// 已转化为修复的数量
    pub fixed: u32,
    /// 修复后回退的数量
    pub regressed: u32,
}

impl BadCaseConversion {
    /// 转化率（千分比）= fixed / detected；detected=0 ⇒ 0（无数据不可称高转化）。
    #[must_use]
    pub fn conversion_rate_milli(&self) -> u32 {
        if self.detected == 0 {
            return 0;
        }
        (self.fixed as u64 * 1000 / self.detected as u64) as u32
    }
}

/// 表三：门覆盖率 — 承诺写点中已插桩敏感扫描的比例（§(b.5) 交叉度量）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateCoverage {
    /// 承诺写点总数
    pub required: u32,
    /// 已插桩写点数
    pub instrumented: u32,
}

impl GateCoverage {
    /// 覆盖率（千分比）= instrumented / required；required=0 ⇒ 0。
    #[must_use]
    pub fn coverage_rate_milli(&self) -> u32 {
        if self.required == 0 {
            return 0;
        }
        (self.instrumented as u64 * 1000 / self.required as u64) as u32
    }
}

/// 评测月报三表（§(b.6)）。数据形状，不含采样调度 IO。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonthlyEvalReport {
    /// 表一：一致率趋势（按周期升序）
    pub agreement_trend: Vec<AgreementTrendPoint>,
    /// 表二：Bad Case 转化率
    pub bad_case: BadCaseConversion,
    /// 表三：门覆盖率
    pub gate_coverage: GateCoverage,
}

impl MonthlyEvalReport {
    /// 一致率趋势是否单调不降（供回退告警；不足两点视为不降）。
    #[must_use]
    pub fn agreement_non_decreasing(&self) -> bool {
        self.agreement_trend
            .windows(2)
            .all(|w| w[0].agreement_milli <= w[1].agreement_milli)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试码 unwrap 为 Rust 惯用法；E-5 意在治理生产码
mod tests {
    use super::*;

    fn s(gate: GateVerdict, human: HumanLabel) -> CalibrationSample {
        CalibrationSample { gate, human }
    }

    #[test]
    fn agree_semantics() {
        assert!(s(GateVerdict::Pass, HumanLabel::Correct).agrees());
        assert!(s(GateVerdict::Fail, HumanLabel::Incorrect).agrees());
        assert!(!s(GateVerdict::Pass, HumanLabel::Incorrect).agrees());
        assert!(!s(GateVerdict::Fail, HumanLabel::Correct).agrees());
    }

    #[test]
    fn agreement_rate_and_threshold() {
        // 8 一致 / 10 => 800‰ < 900 ⇒ 不可 Certify
        let mixed: Vec<_> = (0..8)
            .map(|_| s(GateVerdict::Pass, HumanLabel::Correct))
            .chain((0..2).map(|_| s(GateVerdict::Pass, HumanLabel::Incorrect)))
            .collect();
        assert_eq!(agreement_rate_milli(&mixed), 800);
        assert!(!certifiable(&mixed, DEFAULT_AGREEMENT_THRESHOLD_MILLI));
        // 全一致 => 1000‰ ≥ 900 ⇒ 可 Certify
        let good: Vec<_> = (0..5)
            .map(|_| s(GateVerdict::Fail, HumanLabel::Incorrect))
            .collect();
        assert_eq!(agreement_rate_milli(&good), 1000);
        assert!(certifiable(&good, DEFAULT_AGREEMENT_THRESHOLD_MILLI));
    }

    #[test]
    fn empty_is_conservatively_not_certifiable() {
        assert_eq!(agreement_rate_milli(&[]), 0);
        assert!(!certifiable(&[], DEFAULT_AGREEMENT_THRESHOLD_MILLI));
    }

    #[test]
    fn rm_distillation_requires_both_agreement_and_volume() {
        // 一致率达标 + 样本充足 → 可触发 RM
        assert!(rm_distillation_eligible(950, 900, 500, 300));
        // 一致率未达标 → 不触发（即便样本多）
        assert!(!rm_distillation_eligible(850, 900, 10_000, 300));
        // 样本不足 → 不触发（即便一致率满）
        assert!(!rm_distillation_eligible(1000, 900, 10, 300));
        // JudgeTier 可序列化(链序预留)
        let json = serde_json::to_string(&JudgeTier::RewardModel).unwrap();
        let back: JudgeTier = serde_json::from_str(&json).unwrap();
        assert_eq!(back, JudgeTier::RewardModel);
    }

    #[test]
    fn shared_sample_store_records_unify_both_sources() {
        // §(b.6) 共库契约：两类样本归一同一 EvalRecord 形状
        let cal = EvalRecord::from_calibration(
            "gate:G-84",
            "scripts",
            &s(GateVerdict::Pass, HumanLabel::Incorrect),
        );
        assert_eq!(cal.kind, EvalSampleKind::GateCalibration);
        assert_eq!(cal.status, "incorrect");
        assert!(!cal.is_resolved(), "校准行不参与 Bad Case 转化判定");

        let fail = EvalRecord::from_failure("sig-panic-01", "crates/scc-cache", "Resolved");
        assert_eq!(fail.kind, EvalSampleKind::FailureRegistry);
        assert!(fail.is_resolved());
        // 非 completed 态不虚计
        assert!(!EvalRecord::from_failure("s2", "c", "Mitigated").is_resolved());
        assert!(!EvalRecord::from_failure("s3", "c", "WontFix").is_resolved());

        // 强类型 FailureRecord 入口与事件面字符串惯例同源
        let rec = crate::adjudication::FailureRecord {
            signature: "sig-x".into(),
            applicability: "wal-full".into(),
            experiment_basis: "test_y".into(),
            invalidation_boundary: "none".into(),
            fix_status: crate::adjudication::FailureFixStatus::Resolved,
            affected_component: "crates/scc-cache".into(),
        };
        assert!(EvalRecord::from_record(&rec).is_resolved());
    }

    #[test]
    fn monthly_eval_three_tables_aggregate() {
        let good: Vec<_> = (0..10)
            .map(|_| s(GateVerdict::Pass, HumanLabel::Correct))
            .collect();
        let point = AgreementTrendPoint::from_samples("2026-10", &good);
        assert_eq!(point.agreement_milli, 1000);

        // 表二：转化率 fixed/detected
        let bc = BadCaseConversion {
            detected: 8,
            fixed: 6,
            regressed: 1,
        };
        assert_eq!(bc.conversion_rate_milli(), 750);
        assert_eq!(
            BadCaseConversion {
                detected: 0,
                fixed: 0,
                regressed: 0
            }
            .conversion_rate_milli(),
            0
        );

        // 表三：门覆盖率 instrumented/required
        let gc = GateCoverage {
            required: 11,
            instrumented: 11,
        };
        assert_eq!(gc.coverage_rate_milli(), 1000);

        // 单调不降判定
        let report = MonthlyEvalReport {
            agreement_trend: vec![
                AgreementTrendPoint {
                    period: "2026-09".into(),
                    agreement_milli: 820,
                },
                AgreementTrendPoint {
                    period: "2026-10".into(),
                    agreement_milli: 900,
                },
            ],
            bad_case: bc,
            gate_coverage: gc,
        };
        assert!(report.agreement_non_decreasing());
        let mut regressed_report = report.clone();
        regressed_report.agreement_trend[1].agreement_milli = 700;
        assert!(!regressed_report.agreement_non_decreasing());
    }
}
