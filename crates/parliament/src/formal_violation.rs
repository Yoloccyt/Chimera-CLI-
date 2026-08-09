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

use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_contracts::behavior_contract::{BehaviorContract, ContractCheckOutcome};
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

/// 行为契约前置闸门 — 审议前强制校验（Milestone B-3c 生产接线，P1-4）
///
/// # 语义（违反即否决，九层防御 L0）
/// 任一契约 Violated → ① 调用 `handle_formal_violation`（否决建议 + warn 日志）
/// ② 发布 `FormalViolation` 事件（审计载体，P1-5 起为 Critical + mpsc 旁路投递）
/// ③ 返回否决建议文本。全部 Satisfied → 返回 `None`（候选可进入审议）。
///
/// # 为何 publish_blocking
/// 本函数是同步编排函数（`deliberate_with_contract_guard` 调用链起点），
/// 遵循 §4.4 红线 8（sync 用 publish_blocking），与 seccore asa.rs 模式一致。
pub fn enforce_and_audit(
    bus: &EventBus,
    contracts: &[BehaviorContract],
    observed: &[String],
) -> Option<String> {
    for contract in contracts {
        if let ContractCheckOutcome::Violated { missing } = contract.enforce(observed) {
            let verdict = handle_formal_violation(&contract.contract_id, &contract.target_type);
            let event = NexusEvent::FormalViolation {
                metadata: EventMetadata::new("parliament"),
                contract_id: contract.contract_id.clone(),
                target_type: contract.target_type.clone(),
                violations: missing,
                context: contract.context,
            };
            if let Err(e) = bus.publish_blocking(event) {
                warn!(error = %e, "发布 FormalViolation 事件失败");
            }
            return Some(verdict);
        }
    }
    None
}

/// 事件订阅消费 — 处理外部来源（efficiency-monitor/gsoe-evolution）发布的
/// `FormalViolation`，使 `handle_formal_violation` 在生产路径被调用（P1-4 修复
/// "handle_formal_violation 生产调用者 0"）。
///
/// # 订阅时序
/// 本函数内部先 `subscribe` 再 `tokio::spawn`（§4.4 反模式 3 红线）。
pub fn spawn_formal_violation_subscriber(bus: EventBus) {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(NexusEvent::FormalViolation {
                    contract_id,
                    target_type,
                    violations,
                    ..
                }) => {
                    let text = handle_formal_violation(&contract_id, &target_type);
                    warn!(
                        contract_id = %contract_id,
                        target_type = %target_type,
                        violation_count = violations.len(),
                        "{text}"
                    );
                }
                Ok(_) => {}
                Err(_) => break, // 通道关闭,退出
            }
        }
    });
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
