//! 行为契约强制层 — Parliament 审议入口（Milestone B-3c，九层防御 L0 补齐）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §7.2）：行为契约不可违反，
//! 违反路径 = L0 `BehaviorContract::enforce()` 检出 → 发布 `FormalViolation`
//! 事件 → 本模块审议（否决候选/记录审计）。
//!
//! # 依赖铁律
//!
//! L8 parliament → L0 nexus-contracts（enforce 纯函数）/ L1 event-bus（事件）
//! 均为向下依赖，合规（§2.2）。

use tracing::{info, warn};

/// 审议建议 — FormalViolation 的处置方向
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationVerdict {
    /// 否决关联候选（进化候选/执行计划）
    RejectCandidate,
    /// 仅记录审计（非阻断场景，如 Runtime 观测）
    RecordOnly,
}

/// 审议 FormalViolation — 契约违反的处置入口
///
/// # 规则（规则式，非学习——R2 冻结面外）
/// - 违反即否决：任何契约违反都拒绝关联候选进入后续阶段（九层防御 L0 语义：
///   "行为契约不可违反"）
/// - 返回人类可读建议文本，供编排器/CLI 呈现
///
/// # 返回
/// `(ViolationVerdict, 建议文本)`
pub fn handle_formal_violation(contract_id: &str, target_type: &str) -> String {
    let verdict = ViolationVerdict::RejectCandidate;
    warn!(
        contract_id = %contract_id,
        target_type = %target_type,
        "行为契约违反:候选被否决"
    );
    format!(
        "reject: 契约 '{contract_id}'（目标 {target_type}）违反，候选否决（verdict={verdict:?}）"
    )
}

/// 记录型审议 — 仅审计不否决（调用方判断非阻断场景时使用）
pub fn record_formal_violation(contract_id: &str, violations: &[String]) -> String {
    info!(contract_id = %contract_id, count = violations.len(), "FormalViolation 已记录审计");
    format!(
        "record: 契约 '{contract_id}' 违反 {n} 项断言，已记录审计",
        n = violations.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_is_reject_for_violation() {
        let text = handle_formal_violation("bc-1", "my::Type");
        assert!(text.contains("reject"), "违反应给出否决: {text}");
    }

    #[test]
    fn record_mentions_violation_count() {
        let text = record_formal_violation("bc-1", &["a".into(), "b".into()]);
        assert!(text.contains("2"), "应包含违反计数: {text}");
    }
}
