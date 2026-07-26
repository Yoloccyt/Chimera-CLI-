//! # severity 守护测试 — Critical 事件严重级别回归防护
//!
//! 对应 spec：`.trae/specs/nexus-omega-v5-implementation-plan/spec.md`
//! - Scenario: BudgetExceeded severity 守护（spec.md:192-196）
//! - Scenario: Critical 通道背压改造（spec.md:182-190）列出 6 个 Critical 事件
//!
//! ## 6 个 Critical 事件清单（spec.md:186）
//! `CheckpointSaved / SkepticVeto / RedTeamAudit / AsaIntervention /
//!  AgentTaskFailed / BudgetExceeded`
//!
//! ## 红线来源
//! - `nuxus规则.md §6.2`：Critical 安全事件用 mpsc 确保送达
//! - `nuxus规则.md §5.3`：BudgetExceeded severity = Critical（types.rs:1158 历史行号）
//! - 代码权威源：`crates/event-bus/src/types.rs:2071-2100` `severity()` 方法
//!
//! ## 测试状态说明
//! - 5 个事件（CheckpointSaved / SkepticVeto / RedTeamAudit / AgentTaskFailed /
//!   BudgetExceeded）代码已返回 Critical → **GREEN**（守护测试，防降级回归）
//! - AsaIntervention：spec.md:186 红线要求 Critical，但代码 `severity()` 返回
//!   Normal（`types.rs:1187-1207` 注释说明设计理由）→ **RED**（TDD，暴露
//!   spec/code 不一致，待后续 ADR 裁决）
//!
//! 断言一律符合 spec 红线（`== EventSeverity::Critical`），不因想让测试 RED
//! 而写错断言。

use event_bus::{EventMetadata, EventSeverity, NexusEvent};

// ============================================================
// 辅助函数 — 构造 6 个 Critical 事件变体
// ============================================================

/// 构造 `CheckpointSaved` 事件（spec.md:186 Critical 事件之一）
fn make_checkpoint_saved() -> NexusEvent {
    NexusEvent::CheckpointSaved {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: "quest-test-001".to_string(),
        checkpoint_id: "ckpt-test-001".to_string(),
        memory_snapshot_hash: "sha256:deadbeef".to_string(),
    }
}

/// 构造 `BudgetExceeded` 事件（types.rs:1158 红线 / spec.md:192-196 守护场景）
fn make_budget_exceeded() -> NexusEvent {
    NexusEvent::BudgetExceeded {
        metadata: EventMetadata::new("decb-governor"),
        budget_type: "token".to_string(),
        current: 120_000,
        limit: 100_000,
    }
}

/// 构造 `SkepticVeto` 事件（§6.2 红线：Critical 安全事件用 mpsc）
fn make_skeptic_veto() -> NexusEvent {
    NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("parliament"),
        quest_id: "quest-test-001".to_string(),
        veto_reason: "unsafe shell injection detected".to_string(),
        frozen_capabilities: vec!["cap-shell-exec".to_string()],
    }
}

/// 构造 `RedTeamAudit` 事件（§6.2 红线：Critical 安全事件用 mpsc）
fn make_red_team_audit() -> NexusEvent {
    NexusEvent::RedTeamAudit {
        metadata: EventMetadata::new("parliament"),
        vulnerability_type: "prompt_injection".to_string(),
        failed_probes: 3,
        total_probes: 10,
        detection_rate: 0.3,
        remediation_suggestion: "add input sanitization".to_string(),
    }
}

/// 构造 `AsaIntervention` 事件（spec.md:186 列为 Critical 事件之一）
///
/// 注意：`action="Block"` 在语义上等价于 Critical
/// （`types.rs:1192-1193` 注释），故此处构造 Block 级别干预。
fn make_asa_intervention() -> NexusEvent {
    NexusEvent::AsaIntervention {
        metadata: EventMetadata::new("seccore"),
        operation_id: "op-test-001".to_string(),
        action: "Block".to_string(),
        safety_score: 0.2,
        block_reason: Some("malicious intent detected".to_string()),
        alternative_suggestion: Some("use sandboxed tool X".to_string()),
    }
}

/// 构造 `AgentTaskFailed` 事件（v2.x 新增 Critical 事件，ADR-026）
fn make_agent_task_failed() -> NexusEvent {
    NexusEvent::AgentTaskFailed {
        metadata: EventMetadata::new("chimera-mas"),
        from: "agent-001".to_string(),
        to: "agent-002".to_string(),
        task_id: "task-test-001".to_string(),
        error: "tool call timeout".to_string(),
        retry_count: 3,
    }
}

// ============================================================
// 守护测试 — 6 个 Critical 事件 severity 断言
// ============================================================

/// 守护测试（Guard Test）— BudgetExceeded severity 必须为 Critical
///
/// 红线来源：`nuxus规则.md §5.3 / §6.2` + `spec.md:192-196`
/// 代码权威源：`types.rs` `severity()` 方法 Critical 分支
/// 防护目标：防止 BudgetExceeded 被误降级为 Normal/Info，导致背压策略丢弃。
#[test]
fn test_budget_exceeded_severity_is_critical() {
    let event = make_budget_exceeded();
    assert_eq!(
        event.severity(),
        EventSeverity::Critical,
        "BudgetExceeded severity 必须为 Critical（types.rs 红线 / spec.md:192-196 守护场景）"
    );
}

/// 守护测试（Guard Test）— CheckpointSaved severity 必须为 Critical
///
/// 红线来源：`types.rs` `severity()` Critical 分支
/// 防护目标：CheckpointSaved 丢失会导致 Quest 无法恢复，必须走 mpsc 确保投递。
#[test]
fn test_checkpoint_saved_severity_is_critical() {
    let event = make_checkpoint_saved();
    assert_eq!(
        event.severity(),
        EventSeverity::Critical,
        "CheckpointSaved severity 必须为 Critical（丢失将导致 Quest 无法恢复）"
    );
}

/// 守护测试（Guard Test）— SkepticVeto severity 必须为 Critical
///
/// 红线来源：`nuxus规则.md §6.2` + `types.rs` `severity()` Critical 分支
/// 防护目标：Skeptic 否决权是红队安全防线，丢失将导致高风险操作继续执行。
#[test]
fn test_skeptic_veto_severity_is_critical() {
    let event = make_skeptic_veto();
    assert_eq!(
        event.severity(),
        EventSeverity::Critical,
        "SkepticVeto severity 必须为 Critical（§6.2 红线：否决权丢失导致安全防线失效）"
    );
}

/// 守护测试（Guard Test）— RedTeamAudit severity 必须为 Critical
///
/// 红线来源：`nuxus规则.md §6.2` + `types.rs` `severity()` Critical 分支
/// 防护目标：AHIRT 红队审计结果丢失将导致已知漏洞被忽略。
#[test]
fn test_red_team_audit_severity_is_critical() {
    let event = make_red_team_audit();
    assert_eq!(
        event.severity(),
        EventSeverity::Critical,
        "RedTeamAudit severity 必须为 Critical（§6.2 红线：审计结果丢失导致漏洞被忽略）"
    );
}

/// TDD RED 测试 — AsaIntervention severity 应为 Critical（spec.md:186 红线）
///
/// 红线来源：
/// - `spec.md:186` 将 AsaIntervention 列为 6 个 Critical 事件之一
/// - `nuxus规则.md §6.2` 也将 AsaIntervention 列为 Critical 安全事件
///
/// 当前代码状态（RED 原因）：
/// `types.rs:1187-1207` 注释明确说明 `severity()` 统一返回 Normal，理由是
/// "severity() 是同步函数且不依赖运行时值"，而 AsaIntervention 的严重性
/// 取决于运行时 `action` 字段（Block 才语义等价 Critical）。因此
/// AsaIntervention 走 `severity()` 的通配符分支返回 Normal。
///
/// 本测试暴露 spec/code 不一致：
/// - spec.md 红线要求 AsaIntervention 是 Critical 事件之一
/// - 代码设计者认为 `severity()` 应返回 Normal（因 action 决定严重性）
/// - 后续需 ADR 裁决：要么修改代码让 AsaIntervention 返回 Critical，
///   要么修改 spec 移除 AsaIntervention 出 Critical 清单
///
/// 断言符合 spec 红线（== Critical），不因想让测试 RED 而写错断言。
#[test]
fn test_asa_intervention_severity_is_critical() {
    let event = make_asa_intervention();
    assert_eq!(
        event.severity(),
        EventSeverity::Critical,
        "AsaIntervention severity 应为 Critical（spec.md:186 红线将其列为 Critical 事件之一）；\
         当前代码返回 Normal（types.rs:1187-1207 设计理由），暴露 spec/code 不一致，待 ADR 裁决"
    );
}

/// 守护测试（Guard Test）— AgentTaskFailed severity 必须为 Critical
///
/// 红线来源：`nuxus规则.md §6.2` + ADR-026（chimera-mas）
/// 代码权威源：`types.rs:2081-2084` 注释 + `severity()` Critical 分支
/// 防护目标：Agent 任务失败丢失会导致 Quest 持续等待已死 Agent 结果。
#[test]
fn test_agent_task_failed_severity_is_critical() {
    let event = make_agent_task_failed();
    assert_eq!(
        event.severity(),
        EventSeverity::Critical,
        "AgentTaskFailed severity 必须为 Critical（ADR-026：任务失败影响 Quest 完整性）"
    );
}

/// 汇总守护测试 — 6 个 Critical 事件全部返回 Critical
///
/// 红线来源：`spec.md:186` 列出 6 个 Critical 事件清单
/// 防护目标：任一事件被降级为 Normal/Info 时本测试失败。
///
/// 注意：本测试当前 RED，因为 AsaIntervention 实际返回 Normal（见
/// `test_asa_intervention_severity_is_critical` 注释）。待 spec/code
/// 不一致解决后，本测试转为 GREEN 成为汇总守护测试。
#[test]
fn test_all_critical_events_collected() {
    let critical_events: Vec<(&str, NexusEvent)> = vec![
        ("CheckpointSaved", make_checkpoint_saved()),
        ("SkepticVeto", make_skeptic_veto()),
        ("RedTeamAudit", make_red_team_audit()),
        ("AsaIntervention", make_asa_intervention()),
        ("AgentTaskFailed", make_agent_task_failed()),
        ("BudgetExceeded", make_budget_exceeded()),
    ];

    let degraded: Vec<String> = critical_events
        .iter()
        .filter_map(|(name, event)| {
            if event.severity() != EventSeverity::Critical {
                Some(format!(
                    "{} 返回 {:?}（期望 Critical）",
                    name,
                    event.severity()
                ))
            } else {
                None
            }
        })
        .collect();

    assert!(
        degraded.is_empty(),
        "以下 Critical 事件被降级（spec.md:186 红线）：\n  - {}\n\
         6 个 Critical 事件必须全部返回 EventSeverity::Critical",
        degraded.join("\n  - ")
    );
}
