//! NexusEvent / EventMetadata 集成测试 — 自 types.rs 内联 #[cfg(test)] 下沉(M1 任务 5)
//!
//! 对应任务: types.rs 上帝文件治理(types.rs 4317 行中测试占 1508 行/35%)
//! 架构层: L1 Core (event-bus)
//!
//! # 迁移说明
//! 原 types.rs 内联 `mod tests`(60 个 #[test] + 2 个构造辅助)整体下沉至本
//! 集成测试文件:测试用例总数不减少,全部改经 crate 公开 API 驱动
//! (`event_bus::` 根重导出 + `event_bus::payloads` 公开模块)。
//! 守护性质不变:序列化线格式(msgpack/json 往返)、severity 分级、
//! type_name 稳定性、topic 映射与 metadata() 契约。

#![forbid(unsafe_code)]

use event_bus::payloads::*;
use event_bus::types::*;

#[test]
fn test_metadata_creation() {
    let meta = EventMetadata::new("osa-coordinator");
    assert_eq!(meta.source, "osa-coordinator");
    assert!(!meta.event_id.to_string().is_empty());
}

#[test]
fn test_severity_classification() {
    let critical = NexusEvent::CheckpointSaved {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: "q1".into(),
        checkpoint_id: "c1".into(),
        memory_snapshot_hash: "abc".into(),
    };
    assert_eq!(critical.severity(), EventSeverity::Critical);

    let normal = NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: "k1".into(),
    };
    assert_eq!(normal.severity(), EventSeverity::Normal);
}

/// ADR-029:TUI 交互式动作协议事件的 severity 分级验证
///
/// 请求/终态(Requested/Completed/Failed/ChatSubmitted/ChatCompleted)为 Info;
/// 高频流式(Progressed/ResponseChunk/StatusChanged)为 Normal——
/// 确保高频事件不占用仅为稀有安全告警保留的 mpsc 旁路通道。
#[test]
fn test_tui_action_protocol_severity() {
    let requested = NexusEvent::TuiActionRequested {
        metadata: EventMetadata::new("chimera-tui"),
        request_id: "tui-1".into(),
        action_id: "quest.pause".into(),
        payload: "{\"quest_id\":\"q1\"}".into(),
        source: ActionSource::Palette,
    };
    assert_eq!(requested.severity(), EventSeverity::Info);

    let chunk = NexusEvent::TuiChatResponseChunk {
        metadata: EventMetadata::new("chimera-cli"),
        session_id: "s1".into(),
        delta: "hello".into(),
        cursor_hint: 0,
    };
    assert_eq!(
        chunk.severity(),
        EventSeverity::Normal,
        "高频 token 流必须为 Normal,避免冲垮 mpsc 旁路"
    );

    let submitted = NexusEvent::TuiChatSubmitted {
        metadata: EventMetadata::new("chimera-tui"),
        session_id: "s1".into(),
        query: "实现登录".into(),
        slash_command: None,
    };
    assert_eq!(submitted.severity(), EventSeverity::Info);
}

/// ADR-029:新增事件的 type_name 稳定性与 metadata 可取性验证
#[test]
fn test_tui_action_protocol_type_name_and_metadata() {
    let events = [
        NexusEvent::TuiActionRequested {
            metadata: EventMetadata::new("chimera-tui"),
            request_id: "tui-1".into(),
            action_id: "a".into(),
            payload: "{}".into(),
            source: ActionSource::Chat,
        },
        NexusEvent::TuiActionProgressed {
            metadata: EventMetadata::new("chimera-cli"),
            action_id: "a".into(),
            delta: "d".into(),
        },
        NexusEvent::TuiChatStatusChanged {
            metadata: EventMetadata::new("chimera-cli"),
            session_id: "s".into(),
            status: ChatStatus::Thinking,
        },
    ];
    // metadata() 对所有新变体可取,source 非空;type_name 以 "Tui" 前缀一致
    for e in &events {
        assert!(!e.metadata().source.is_empty());
        assert!(e.type_name().starts_with("Tui"));
    }
}

#[test]
fn test_type_name_stable() {
    let e = NexusEvent::VoteCast {
        metadata: EventMetadata::new("parliament"),
        proposal_id: "p1".into(),
        voter: "v1".into(),
        vote: true,
    };
    assert_eq!(e.type_name(), "VoteCast");
}

// ============================================================
// Week 4 扩展测试:验证新增 16 个事件变体的行为
// ============================================================

#[test]
fn test_week4_event_orphan_call_critical() {
    let e = NexusEvent::OrphanCallDetected {
        metadata: EventMetadata::new("gqep-executor"),
        operation_id: "op-1".into(),
        spawn_location: "gatherer.rs:42".into(),
    };
    assert_eq!(e.severity(), EventSeverity::Critical);
    assert_eq!(e.type_name(), "OrphanCallDetected");
}

#[test]
fn test_week4_event_expert_activated_normal() {
    let e = NexusEvent::ExpertActivated {
        metadata: EventMetadata::new("gea-activator"),
        activated_experts: vec!["e1".into(), "e2".into()],
        suppressed_experts: vec!["e3".into()],
        top_gate_value: 0.85,
    };
    assert_eq!(e.severity(), EventSeverity::Normal);
    assert_eq!(e.type_name(), "ExpertActivated");
    assert_eq!(e.metadata().source, "gea-activator");
}

#[test]
fn test_week4_event_gather_completed() {
    let e = NexusEvent::GatherCompleted {
        metadata: EventMetadata::new("gqep-executor"),
        total: 10,
        succeeded: 8,
        failed: 2,
        latency_ms: 50.0,
    };
    assert_eq!(e.type_name(), "GatherCompleted");
    assert_eq!(e.severity(), EventSeverity::Normal);
}

#[test]
fn test_week4_event_serialization() {
    let e = NexusEvent::CachePrefetched {
        metadata: EventMetadata::new("scc-cache"),
        prefetched_ids: vec!["ctx-1".into(), "ctx-2".into()],
    };
    let json = serde_json::to_string(&e).unwrap();
    let restored: NexusEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(e, restored);
}

// ============================================================
// Week 5 扩展测试(SubTask 37.1):验证新增 8 个事件变体 +
// ThinkingModeSwitched 扩展字段的行为
// ============================================================

// --- severity() 正确性测试 ---

#[test]
fn test_week5_event_critical_severity() {
    // SkepticVeto 行使否决权,Critical
    let skeptic_veto = NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-1".into(),
        veto_reason: "unsafe shell injection".into(),
        frozen_capabilities: vec!["shell_exec".into()],
    };
    assert_eq!(skeptic_veto.severity(), EventSeverity::Critical);

    // RedTeamAudit 红队审计发现漏洞,Critical
    let red_team = NexusEvent::RedTeamAudit {
        metadata: EventMetadata::new("parliament"),
        vulnerability_type: "prompt_injection".into(),
        failed_probes: 5,
        total_probes: 20,
        detection_rate: 0.25,
        remediation_suggestion: "add input sanitization".into(),
    };
    assert_eq!(red_team.severity(), EventSeverity::Critical);
}

#[test]
fn test_week5_event_normal_severity() {
    let meta = EventMetadata::new("test-source");
    let debate = NexusEvent::DebateStarted {
        metadata: meta.clone(),
        quest_id: "q-1".into(),
        proposal_id: "p-1".into(),
        participant_count: 5,
    };
    assert_eq!(debate.severity(), EventSeverity::Normal);

    let budget_adj = NexusEvent::BudgetAdjusted {
        metadata: meta.clone(),
        quest_id: "q-1".into(),
        old_tier: "High".into(),
        new_tier: "Medium".into(),
        coefficient: 0.5,
        reason: "consumption > 0.8".into(),
    };
    assert_eq!(budget_adj.severity(), EventSeverity::Normal);

    let asa = NexusEvent::AsaIntervention {
        metadata: meta.clone(),
        operation_id: "op-1".into(),
        action: "Block".into(),
        safety_score: 0.2,
        block_reason: Some("unsafe".into()),
        alternative_suggestion: None,
    };
    // P1-W2.1.4 修复:AsaIntervention 统一返回 Critical(对齐 spec.md L186 红线)。
    // 历史设计曾返回 Normal,W1.2 TDD 测试暴露 spec/code 偏差后修复。
    // 详见 severity() 方法中 AsaIntervention 分支注释。
    assert_eq!(asa.severity(), EventSeverity::Critical);

    let ahirt = NexusEvent::AhirtProbeCompleted {
        metadata: meta.clone(),
        probe_type: "prompt_injection".into(),
        total: 20,
        passed: 15,
        failed: 5,
        detection_rate: 0.25,
    };
    assert_eq!(ahirt.severity(), EventSeverity::Normal);

    let role = NexusEvent::RoleRegistered {
        metadata: meta.clone(),
        role_id: "visionary-01".into(),
        role_name: "Visionary".into(),
        voting_weight: 0.4,
    };
    assert_eq!(role.severity(), EventSeverity::Normal);

    let stats = NexusEvent::BudgetStatsReported {
        metadata: meta,
        total_consumption: 5000.0,
        remaining_budget: 5000.0,
        utilization_rate: 0.5,
    };
    assert_eq!(stats.severity(), EventSeverity::Normal);
}

// --- type_name() 正确性测试 ---

#[test]
fn test_week5_event_type_names() {
    let meta = EventMetadata::new("test");
    assert_eq!(
        NexusEvent::DebateStarted {
            metadata: meta.clone(),
            quest_id: "q".into(),
            proposal_id: "p".into(),
            participant_count: 1,
        }
        .type_name(),
        "DebateStarted"
    );
    assert_eq!(
        NexusEvent::SkepticVeto {
            metadata: meta.clone(),
            quest_id: "q".into(),
            veto_reason: "r".into(),
            frozen_capabilities: vec![],
        }
        .type_name(),
        "SkepticVeto"
    );
    assert_eq!(
        NexusEvent::RedTeamAudit {
            metadata: meta.clone(),
            vulnerability_type: "t".into(),
            failed_probes: 0,
            total_probes: 0,
            detection_rate: 0.0,
            remediation_suggestion: "s".into(),
        }
        .type_name(),
        "RedTeamAudit"
    );
    assert_eq!(
        NexusEvent::BudgetAdjusted {
            metadata: meta.clone(),
            quest_id: "q".into(),
            old_tier: "H".into(),
            new_tier: "M".into(),
            coefficient: 1.0,
            reason: "r".into(),
        }
        .type_name(),
        "BudgetAdjusted"
    );
    assert_eq!(
        NexusEvent::AsaIntervention {
            metadata: meta.clone(),
            operation_id: "o".into(),
            action: "Allow".into(),
            safety_score: 1.0,
            block_reason: None,
            alternative_suggestion: None,
        }
        .type_name(),
        "AsaIntervention"
    );
    assert_eq!(
        NexusEvent::AhirtProbeCompleted {
            metadata: meta.clone(),
            probe_type: "t".into(),
            total: 0,
            passed: 0,
            failed: 0,
            detection_rate: 0.0,
        }
        .type_name(),
        "AhirtProbeCompleted"
    );
    assert_eq!(
        NexusEvent::RoleRegistered {
            metadata: meta.clone(),
            role_id: "r".into(),
            role_name: "n".into(),
            voting_weight: 1.0,
        }
        .type_name(),
        "RoleRegistered"
    );
    assert_eq!(
        NexusEvent::BudgetStatsReported {
            metadata: meta,
            total_consumption: 0.0,
            remaining_budget: 0.0,
            utilization_rate: 0.0,
        }
        .type_name(),
        "BudgetStatsReported"
    );
}

// --- 序列化 round-trip 测试(每个新变体) ---

#[test]
fn test_week5_event_debate_started_serialization() {
    let e = NexusEvent::DebateStarted {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-1".into(),
        proposal_id: "p-1".into(),
        participant_count: 5,
    };
    let json = serde_json::to_string(&e).unwrap();
    let restored: NexusEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(e, restored);
}

#[test]
fn test_week5_event_skeptic_veto_serialization() {
    let e = NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-1".into(),
        veto_reason: "unsafe shell injection".into(),
        frozen_capabilities: vec!["shell_exec".into(), "fs_write".into()],
    };
    let json = serde_json::to_string(&e).unwrap();
    let restored: NexusEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(e, restored);
}

#[test]
fn test_week5_event_red_team_audit_serialization() {
    let e = NexusEvent::RedTeamAudit {
        metadata: EventMetadata::new("parliament"),
        vulnerability_type: "prompt_injection".into(),
        failed_probes: 5,
        total_probes: 20,
        detection_rate: 0.25,
        remediation_suggestion: "add input sanitization".into(),
    };
    let json = serde_json::to_string(&e).unwrap();
    let restored: NexusEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(e, restored);
}

#[test]
fn test_week5_event_budget_adjusted_serialization() {
    let e = NexusEvent::BudgetAdjusted {
        metadata: EventMetadata::new("decb-governor"),
        quest_id: "q-1".into(),
        old_tier: "High".into(),
        new_tier: "Medium".into(),
        coefficient: 0.5,
        reason: "consumption > 0.8".into(),
    };
    let json = serde_json::to_string(&e).unwrap();
    let restored: NexusEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(e, restored);
}

#[test]
fn test_week5_event_asa_intervention_serialization() {
    // 测试 Block 场景(带 block_reason 和 alternative_suggestion)
    let e_block = NexusEvent::AsaIntervention {
        metadata: EventMetadata::new("seccore"),
        operation_id: "op-1".into(),
        action: "Block".into(),
        safety_score: 0.2,
        block_reason: Some("unsafe operation".into()),
        alternative_suggestion: Some("use sandboxed tool".into()),
    };
    let json = serde_json::to_string(&e_block).unwrap();
    let restored: NexusEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(e_block, restored);

    // 测试 Allow 场景(block_reason 和 alternative_suggestion 为 None)
    let e_allow = NexusEvent::AsaIntervention {
        metadata: EventMetadata::new("seccore"),
        operation_id: "op-2".into(),
        action: "Allow".into(),
        safety_score: 0.95,
        block_reason: None,
        alternative_suggestion: None,
    };
    let json = serde_json::to_string(&e_allow).unwrap();
    let restored: NexusEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(e_allow, restored);
}

#[test]
fn test_week5_event_ahirt_probe_completed_serialization() {
    let e = NexusEvent::AhirtProbeCompleted {
        metadata: EventMetadata::new("parliament"),
        probe_type: "tool_abuse".into(),
        total: 100,
        passed: 95,
        failed: 5,
        detection_rate: 0.05,
    };
    let json = serde_json::to_string(&e).unwrap();
    let restored: NexusEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(e, restored);
}

#[test]
fn test_week5_event_role_registered_serialization() {
    let e = NexusEvent::RoleRegistered {
        metadata: EventMetadata::new("parliament"),
        role_id: "skeptic-01".into(),
        role_name: "Skeptic".into(),
        voting_weight: 0.3,
    };
    let json = serde_json::to_string(&e).unwrap();
    let restored: NexusEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(e, restored);
}

#[test]
fn test_week5_event_budget_stats_reported_serialization() {
    let e = NexusEvent::BudgetStatsReported {
        metadata: EventMetadata::new("decb-governor"),
        total_consumption: 7500.0,
        remaining_budget: 2500.0,
        utilization_rate: 0.75,
    };
    let json = serde_json::to_string(&e).unwrap();
    let restored: NexusEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(e, restored);
}

// --- ThinkingModeSwitched 扩展字段测试 ---

#[test]
fn test_week5_thinking_mode_switched_with_reason() {
    let e = NexusEvent::ThinkingModeSwitched {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: "q-1".into(),
        from_mode: "fast".into(),
        to_mode: "deep".into(),
        reason: "complexity threshold exceeded".into(),
    };
    let json = serde_json::to_string(&e).unwrap();
    let restored: NexusEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(e, restored);
    assert_eq!(e.type_name(), "ThinkingModeSwitched");
    assert_eq!(e.severity(), EventSeverity::Normal);
}

#[test]
fn test_week5_thinking_mode_switched_backward_compat() {
    // WHY:旧格式数据(无 reason 字段)必须能反序列化为新结构,
    // reason 字段通过 #[serde(default)] 填充为空字符串。
    // 这确保 Week 1/2 已序列化的 ThinkingModeSwitched 数据
    // 仍能被 Week 5 的新消费者正确读取。
    let old_json = r#"{"type":"ThinkingModeSwitched","data":{"metadata":{"event_id":"01901234-5678-7abc-def0-123456789abc","timestamp":"2025-01-01T00:00:00Z","source":"quest-engine"},"quest_id":"q-1","from_mode":"fast","to_mode":"deep"}}"#;
    let restored: NexusEvent = serde_json::from_str(old_json).unwrap();
    match restored {
        NexusEvent::ThinkingModeSwitched {
            quest_id,
            from_mode,
            to_mode,
            reason,
            ..
        } => {
            assert_eq!(quest_id, "q-1");
            assert_eq!(from_mode, "fast");
            assert_eq!(to_mode, "deep");
            // 旧格式数据无 reason 字段,反序列化为空字符串
            assert_eq!(reason, "");
        }
        _ => panic!("expected ThinkingModeSwitched variant"),
    }
}

// ============================================================
// Week 6 扩展测试:验证 NmcEncoded 事件变体的行为
// ============================================================

#[test]
fn test_week6_event_nmc_encoded_normal_severity() {
    let e = NexusEvent::NmcEncoded {
        metadata: EventMetadata::new("nmc-encoder"),
        modality: "Text".into(),
        content_hash: "abc123".into(),
        clv_dimension: 512,
    };
    assert_eq!(e.severity(), EventSeverity::Normal);
    assert_eq!(e.type_name(), "NmcEncoded");
    assert_eq!(e.metadata().source, "nmc-encoder");
}

#[test]
fn test_week6_event_nmc_encoded_serialization() {
    let e = NexusEvent::NmcEncoded {
        metadata: EventMetadata::new("nmc-encoder"),
        modality: "Desktop".into(),
        content_hash: "deadbeef".into(),
        clv_dimension: 512,
    };
    let json = serde_json::to_string(&e).unwrap();
    let restored: NexusEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(e, restored);
}

#[test]
fn test_week6_event_nmc_encoded_msgpack_roundtrip() {
    let e = NexusEvent::NmcEncoded {
        metadata: EventMetadata::new("nmc-encoder"),
        modality: "Image".into(),
        content_hash: "cafebabe".into(),
        clv_dimension: 512,
    };
    let bytes = event_bus::serialize_msgpack(&e).unwrap();
    let decoded = event_bus::deserialize_msgpack(&bytes).unwrap();
    assert_eq!(e, decoded);
}

// ============================================================
// F-001 回归测试:验证 BudgetExceeded severity == Critical
// Hard Constraint 第 10 条:BudgetExceeded 必须标记为 Critical
// WHY:预算耗尽是系统红线,若被通配符误判为 Normal,在背压场景下
// 可能被丢弃,导致预算超限无人响应、Quest 持续消耗资源直至 OOM。
// 此测试守护 severity() 显式分支,防止未来重构时意外回退。
// ============================================================

#[test]
fn test_budget_exceeded_severity_is_critical() {
    let e = NexusEvent::BudgetExceeded {
        metadata: EventMetadata::new("decb-governor"),
        budget_type: "token".into(),
        current: 10_000,
        limit: 8_000,
    };
    assert_eq!(
        e.severity(),
        EventSeverity::Critical,
        "BudgetExceeded 必须为 Critical (Hard Constraint 第 10 条)"
    );
    assert_eq!(e.type_name(), "BudgetExceeded");
}

// ============================================================
// P1-3 扩展测试:验证 VetoOverridden 事件变体
// ============================================================

#[test]
fn test_veto_overridden_severity_is_critical() {
    let e = NexusEvent::VetoOverridden {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-1".into(),
        proposal_id: "p-1".into(),
        veto_reason: "command_injection detected".into(),
        override_reason: "false positive: legitimate shell script".into(),
        override_by: "admin:alice".into(),
    };
    assert_eq!(
        e.severity(),
        EventSeverity::Critical,
        "VetoOverridden 必须为 Critical(否决覆盖审计)"
    );
    assert_eq!(e.type_name(), "VetoOverridden");
    assert_eq!(e.metadata().source, "parliament");
}

#[test]
fn test_veto_overridden_serialization_roundtrip() {
    let e = NexusEvent::VetoOverridden {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-1".into(),
        proposal_id: "p-1".into(),
        veto_reason: "Skeptic 否决:DataExfiltration 'curl'".into(),
        override_reason: "legitimate API call to github.com".into(),
        override_by: "system:auto-review".into(),
    };
    let json = serde_json::to_string(&e).unwrap();
    let restored: NexusEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(e, restored);
}

#[test]
fn test_veto_overridden_msgpack_roundtrip() {
    let e = NexusEvent::VetoOverridden {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-2".into(),
        proposal_id: "p-2".into(),
        veto_reason: "sandbox_escape /proc/".into(),
        override_reason: "monitoring use case".into(),
        override_by: "admin:bob".into(),
    };
    let bytes = event_bus::serialize_msgpack(&e).unwrap();
    let decoded = event_bus::deserialize_msgpack(&bytes).unwrap();
    assert_eq!(e, decoded);
}

// ============================================================
// M4 扩展测试:验证 TUI 双向控制事件
// ============================================================

#[test]
fn test_m4_control_events_normal_severity() {
    let meta = EventMetadata::new("chimera-tui");
    let pause = NexusEvent::QuestPauseRequested {
        metadata: meta.clone(),
        quest_id: "q-1".into(),
        requested_by: "operator".into(),
    };
    let resume = NexusEvent::QuestResumeRequested {
        metadata: meta.clone(),
        quest_id: "q-1".into(),
        requested_by: "operator".into(),
    };
    let vote = NexusEvent::VoteCastRequested {
        metadata: meta.clone(),
        proposal_id: "p-1".into(),
        voter: "operator".into(),
        vote: VoteValue::Abstain,
    };
    let refresh = NexusEvent::RefreshStateRequested {
        metadata: meta,
        requested_by: "operator".into(),
    };

    for e in [pause, resume, vote, refresh] {
        assert_eq!(e.severity(), EventSeverity::Normal);
    }
}

#[test]
fn test_m4_control_events_type_names() {
    let meta = EventMetadata::new("chimera-tui");
    assert_eq!(
        NexusEvent::QuestPauseRequested {
            metadata: meta.clone(),
            quest_id: "q-1".into(),
            requested_by: "operator".into(),
        }
        .type_name(),
        "QuestPauseRequested"
    );
    assert_eq!(
        NexusEvent::QuestResumeRequested {
            metadata: meta.clone(),
            quest_id: "q-1".into(),
            requested_by: "operator".into(),
        }
        .type_name(),
        "QuestResumeRequested"
    );
    assert_eq!(
        NexusEvent::VoteCastRequested {
            metadata: meta.clone(),
            proposal_id: "p-1".into(),
            voter: "operator".into(),
            vote: VoteValue::Yes,
        }
        .type_name(),
        "VoteCastRequested"
    );
    assert_eq!(
        NexusEvent::RefreshStateRequested {
            metadata: meta.clone(),
            requested_by: "operator".into(),
        }
        .type_name(),
        "RefreshStateRequested"
    );
    assert_eq!(
        NexusEvent::QuestPaused {
            metadata: meta.clone(),
            quest_id: "q-1".into(),
            requested_by: "operator".into(),
        }
        .type_name(),
        "QuestPaused"
    );
    assert_eq!(
        NexusEvent::QuestResumed {
            metadata: meta,
            quest_id: "q-1".into(),
            requested_by: "operator".into(),
        }
        .type_name(),
        "QuestResumed"
    );
}

#[test]
fn test_m4_control_events_serialization_roundtrip() {
    let cases = vec![
        NexusEvent::QuestPauseRequested {
            metadata: EventMetadata::new("chimera-tui"),
            quest_id: "q-1".into(),
            requested_by: "operator".into(),
        },
        NexusEvent::QuestResumeRequested {
            metadata: EventMetadata::new("chimera-tui"),
            quest_id: "q-2".into(),
            requested_by: "operator".into(),
        },
        NexusEvent::VoteCastRequested {
            metadata: EventMetadata::new("chimera-tui"),
            proposal_id: "p-1".into(),
            voter: "operator".into(),
            vote: VoteValue::No,
        },
        NexusEvent::RefreshStateRequested {
            metadata: EventMetadata::new("chimera-tui"),
            requested_by: "operator".into(),
        },
        NexusEvent::QuestPaused {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-1".into(),
            requested_by: "operator".into(),
        },
        NexusEvent::QuestResumed {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-1".into(),
            requested_by: "operator".into(),
        },
    ];

    for e in cases {
        let json = serde_json::to_string(&e).unwrap();
        let restored: NexusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, restored);
    }
}

#[test]
fn test_m4_vote_value_serialization() {
    for value in [VoteValue::Yes, VoteValue::No, VoteValue::Abstain] {
        let json = serde_json::to_string(&value).unwrap();
        let restored: VoteValue = serde_json::from_str(&json).unwrap();
        assert_eq!(value, restored);
    }
}

// ============================================================
// TUI v1.8-omega 扩展测试:验证 ClvSnapshotReported 事件变体
// ============================================================

#[test]
fn test_clv_snapshot_reported_normal_severity() {
    let summary = ClvSummary {
        block_means: vec![0.1; 8],
        l2_norm: 2.5,
        top_dims: vec![(0, 0.8), (64, 0.6)],
    };
    let e = NexusEvent::ClvSnapshotReported {
        metadata: EventMetadata::new("nmc-encoder"),
        modality: "Text".into(),
        content_hash: "abc123".into(),
        clv_summary: summary,
    };
    assert_eq!(e.severity(), EventSeverity::Normal);
    assert_eq!(e.type_name(), "ClvSnapshotReported");
    assert_eq!(e.metadata().source, "nmc-encoder");
}

#[test]
fn test_clv_snapshot_reported_serialization_json() {
    let summary = ClvSummary {
        block_means: vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7],
        l2_norm: 1.234,
        top_dims: vec![(0, 0.9), (128, 0.7), (256, 0.5)],
    };
    let e = NexusEvent::ClvSnapshotReported {
        metadata: EventMetadata::new("nmc-encoder"),
        modality: "Image".into(),
        content_hash: "deadbeef".into(),
        clv_summary: summary,
    };
    let json = serde_json::to_string(&e).unwrap();
    let restored: NexusEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(e, restored);
}

#[test]
fn test_clv_snapshot_reported_msgpack_roundtrip() {
    let summary = ClvSummary {
        block_means: vec![-0.5; 8],
        l2_norm: 0.0,
        top_dims: vec![],
    };
    let e = NexusEvent::ClvSnapshotReported {
        metadata: EventMetadata::new("nmc-encoder"),
        modality: "Audio".into(),
        content_hash: "cafebabe".into(),
        clv_summary: summary,
    };
    let bytes = event_bus::serialize_msgpack(&e).unwrap();
    let decoded = event_bus::deserialize_msgpack(&bytes).unwrap();
    assert_eq!(e, decoded);
}

#[test]
fn test_clv_summary_partial_eq() {
    let s1 = ClvSummary {
        block_means: vec![0.1; 8],
        l2_norm: 1.0,
        top_dims: vec![(1, 0.5)],
    };
    let s2 = ClvSummary {
        block_means: vec![0.1; 8],
        l2_norm: 1.0,
        top_dims: vec![(1, 0.5)],
    };
    assert_eq!(s1, s2);

    let s3 = ClvSummary {
        block_means: vec![0.2; 8],
        l2_norm: 1.0,
        top_dims: vec![(1, 0.5)],
    };
    assert_ne!(s1, s3);
}

#[test]
fn test_clv_snapshot_reported_metadata_extraction() {
    let summary = ClvSummary {
        block_means: vec![0.0; 8],
        l2_norm: 0.0,
        top_dims: vec![],
    };
    let metadata = EventMetadata::new("test-source");
    let expected_id = metadata.event_id;
    let e = NexusEvent::ClvSnapshotReported {
        metadata,
        modality: "Text".into(),
        content_hash: "test".into(),
        clv_summary: summary,
    };
    assert_eq!(e.metadata().event_id, expected_id);
    assert_eq!(e.metadata().source, "test-source");
}

// ============================================================
// P2-13: R1ShadowRollbackFailed 结构化理由记录测试
// ============================================================

/// 验证 `RollbackTriggerType` 默认值为 Unknown
///
/// WHY 默认 Unknown:确保未显式设置 trigger_type 的旧版本事件反序列化后
/// 不会误归类为某一具体触发条件。
#[test]
fn test_p2_13_rollback_trigger_type_default_is_unknown() {
    let default_trigger: RollbackTriggerType = Default::default();
    assert_eq!(default_trigger, RollbackTriggerType::Unknown);
}

/// 验证 `RollbackTriggerType::description()` 返回非空人类可读描述
///
/// 每个变体应有唯一的描述字符串,用于日志与 TUI 展示。
#[test]
fn test_p2_13_rollback_trigger_type_description() {
    let cases = [
        (
            RollbackTriggerType::ConsecutiveRegression,
            "R1 significantly worse than L3 for 3 consecutive days",
        ),
        (
            RollbackTriggerType::AsaIntervention,
            "ASA intervention triggered on R1 seam",
        ),
        (
            RollbackTriggerType::EwmaCollapse,
            "EWMA collapsed by >=0.3 within 24h",
        ),
        (
            RollbackTriggerType::RecallRateDrop,
            "Recall rate dropped >=5% vs L3 baseline",
        ),
        (RollbackTriggerType::Unknown, "Unknown rollback trigger"),
    ];
    for (trigger, expected_desc) in cases {
        assert_eq!(
            trigger.description(),
            expected_desc,
            "RollbackTriggerType::{:?} description mismatch",
            trigger
        );
    }
}

/// 验证 `RollbackTriggerType` 序列化为 snake_case(ADR-043 决策 4 对齐)
///
/// WHY snake_case:对齐 Rust serde 惯例与 JSON 字段命名规范,
/// 便于审计日志解析与 efficiency-monitor 告警规则匹配。
#[test]
fn test_p2_13_rollback_trigger_type_serialization_snake_case() {
    let cases = [
        (
            RollbackTriggerType::ConsecutiveRegression,
            "consecutive_regression",
        ),
        (RollbackTriggerType::AsaIntervention, "asa_intervention"),
        (RollbackTriggerType::EwmaCollapse, "ewma_collapse"),
        (RollbackTriggerType::RecallRateDrop, "recall_rate_drop"),
        (RollbackTriggerType::Unknown, "unknown"),
    ];
    for (trigger, expected_json) in cases {
        let json = serde_json::to_string(&trigger).unwrap();
        assert_eq!(json, format!("\"{}\"", expected_json));
        let restored: RollbackTriggerType = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, trigger);
    }
}

/// 验证 `RollbackDiagnosticContext::default()` 所有字段为 None
#[test]
fn test_p2_13_rollback_diagnostic_context_default_all_none() {
    let ctx = RollbackDiagnosticContext::default();
    assert_eq!(ctx.ewma_level, None);
    assert_eq!(ctx.observation_days, None);
    assert_eq!(ctx.regression_streak, None);
    assert_eq!(ctx.recall_rate_drop, None);
    assert_eq!(ctx.rollback_target_version, None);
}

/// 验证 `RollbackDiagnosticContext` builder 模式正确设置字段
///
/// builder 模式用于 R1 回滚失败时构造诊断快照,便于专家团队复盘根因。
#[test]
fn test_p2_13_rollback_diagnostic_context_builder() {
    let ctx = RollbackDiagnosticContext::empty()
        .with_ewma_level(0.35)
        .with_observation_days(7)
        .with_regression_streak(3)
        .with_recall_rate_drop(0.08)
        .with_rollback_target_version(42);
    assert_eq!(ctx.ewma_level, Some(0.35));
    assert_eq!(ctx.observation_days, Some(7));
    assert_eq!(ctx.regression_streak, Some(3));
    assert_eq!(ctx.recall_rate_drop, Some(0.08));
    assert_eq!(ctx.rollback_target_version, Some(42));
}

/// 验证 `RollbackDiagnosticContext` 序列化/反序列化往返一致
#[test]
fn test_p2_13_rollback_diagnostic_context_serialization() {
    let ctx = RollbackDiagnosticContext::empty()
        .with_ewma_level(0.42)
        .with_observation_days(14)
        .with_regression_streak(5);
    let json = serde_json::to_string(&ctx).unwrap();
    let restored: RollbackDiagnosticContext = serde_json::from_str(&json).unwrap();
    assert_eq!(ctx, restored);
}

/// 验证 `R1ShadowRollbackFailed` 事件可构造且新字段正确(P2-13 结构化字段)
///
/// 覆盖完整字段构造,模拟真实回滚失败场景:
/// - trigger_type = EwmaCollapse(EWMA 24h 内下降 ≥ 0.3)
/// - triggered_at = 精确时间戳
/// - details = CapabilityTokenRegistry 内部错误消息
/// - diagnostic = EWMA 水平 0.35 + 观察期 7 天
#[test]
fn test_p2_13_r1_shadow_rollback_failed_with_structured_fields() {
    let triggered_at = chrono::Utc::now();
    let diagnostic = RollbackDiagnosticContext::empty()
        .with_ewma_level(0.35)
        .with_observation_days(7);
    let event = NexusEvent::R1ShadowRollbackFailed {
        metadata: EventMetadata::new("omega-learner"),
        reason: "EWMA collapsed from 0.7 to 0.35 within 24h".to_string(),
        trigger_type: RollbackTriggerType::EwmaCollapse,
        triggered_at: Some(triggered_at),
        details: "CapabilityTokenRegistry::trigger_asa_intervention failed: internal error"
            .to_string(),
        diagnostic,
    };
    assert_eq!(event.severity(), EventSeverity::Critical);
    // 使用模式匹配解构枚举变体字段
    match &event {
        NexusEvent::R1ShadowRollbackFailed {
            trigger_type,
            triggered_at: ta,
            details,
            diagnostic,
            ..
        } => {
            assert_eq!(*trigger_type, RollbackTriggerType::EwmaCollapse);
            assert_eq!(*ta, Some(triggered_at));
            assert!(details.contains("CapabilityTokenRegistry"));
            assert_eq!(diagnostic.ewma_level, Some(0.35));
            assert_eq!(diagnostic.observation_days, Some(7));
        }
        other => panic!(
            "Expected R1ShadowRollbackFailed, got {:?}",
            other.type_name()
        ),
    }
}

/// 验证 `R1ShadowRollbackFailed` 事件序列化/反序列化往返一致
///
/// 确保结构化字段(trigger_type / triggered_at / details / diagnostic)
/// 在 JSON 序列化后能完整恢复。
#[test]
fn test_p2_13_r1_shadow_rollback_failed_serialization() {
    let triggered_at = chrono::Utc::now();
    let diagnostic = RollbackDiagnosticContext::empty()
        .with_regression_streak(3)
        .with_rollback_target_version(10);
    let event = NexusEvent::R1ShadowRollbackFailed {
        metadata: EventMetadata::new("omega-learner"),
        reason: "ConsecutiveRegression detected".to_string(),
        trigger_type: RollbackTriggerType::ConsecutiveRegression,
        triggered_at: Some(triggered_at),
        details: "R1 worse than L3 for 3 consecutive days".to_string(),
        diagnostic,
    };
    let json = serde_json::to_string(&event).unwrap();
    let restored: NexusEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(event, restored);
}

/// 验证向后兼容性:旧格式 JSON(仅有 reason 字段)能被反序列化
///
/// P2-13 之前的事件只有 `metadata` + `reason` 字段。新增的 4 个字段
/// (trigger_type / triggered_at / details / diagnostic)都有 `#[serde(default)]`,
/// 确保旧格式 JSON 能被反序列化为默认值。
///
/// 这是 SemVer minor 兼容性的关键验证。
///
/// NOTE: `NexusEvent` 使用 `#[serde(tag = "type", content = "data")]` internally
/// tagged 表示,JSON 格式为 `{"type": "VariantName", "data": {fields}}`。
#[test]
fn test_p2_13_r1_shadow_rollback_failed_backward_compatibility() {
    // 模拟旧格式 JSON(无 trigger_type / triggered_at / details / diagnostic 字段)
    let old_json = r#"{
        "type": "R1ShadowRollbackFailed",
        "data": {
            "metadata": {
                "event_id": "550e8400-e29b-41d4-a716-446655440000",
                "source": "omega-learner",
                "timestamp": "2026-07-25T10:00:00Z"
            },
            "reason": "ConsecutiveRegression"
        }
    }"#;
    let restored: NexusEvent = serde_json::from_str(old_json).unwrap();
    match restored {
        NexusEvent::R1ShadowRollbackFailed {
            reason,
            trigger_type,
            triggered_at,
            details,
            diagnostic,
            ..
        } => {
            assert_eq!(reason, "ConsecutiveRegression");
            // 新字段应有默认值
            assert_eq!(trigger_type, RollbackTriggerType::Unknown);
            assert_eq!(triggered_at, None);
            assert_eq!(details, "");
            assert_eq!(diagnostic, RollbackDiagnosticContext::default());
        }
        other => panic!(
            "Expected R1ShadowRollbackFailed, got {:?}",
            other.type_name()
        ),
    }
}

/// 验证 `R1ShadowRollbackFailed` 的 type_name 稳定性(序列化兼容性)
///
/// type_name 必须保持 "R1ShadowRollbackFailed",不允许因 P2-13 扩展而变更,
/// 否则会破坏 efficiency-monitor 的告警规则匹配与 TUI 事件分类。
#[test]
fn test_p2_13_r1_shadow_rollback_failed_type_name_stable() {
    let event = NexusEvent::R1ShadowRollbackFailed {
        metadata: EventMetadata::new("test"),
        reason: "test".to_string(),
        trigger_type: RollbackTriggerType::Unknown,
        triggered_at: None,
        details: String::new(),
        diagnostic: RollbackDiagnosticContext::default(),
    };
    assert_eq!(event.type_name(), "R1ShadowRollbackFailed");
}

// ============================================================
// P2-1 后续增强:CoordinationRatioReported 事件测试
// ============================================================

/// 验证 `CoordinationRatioReported` 的 type_name 稳定性
///
/// type_name 必须保持 "CoordinationRatioReported",不允许变更,
/// 否则会破坏 efficiency-monitor 的告警规则匹配与 TUI 事件分类。
#[test]
fn test_p2_1_coordination_ratio_reported_type_name_stable() {
    let event = NexusEvent::CoordinationRatioReported {
        metadata: EventMetadata::new("quest-engine"),
        coordination_cost_ms: 500.0,
        inference_gain: 0.8,
        cost_index: 0.5,
        gain_index: 0.8,
        ratio: 0.625,
        is_paradox_risk: false,
        threshold: 1.0,
        sample_count: 10,
    };
    assert_eq!(event.type_name(), "CoordinationRatioReported");
}

/// 验证 `CoordinationRatioReported` 的 metadata 可取性
///
/// metadata.source 必须与构造时传入的 "quest-engine" 一致,
/// 确保事件溯源信息不丢失。
#[test]
fn test_p2_1_coordination_ratio_reported_metadata_accessible() {
    let event = NexusEvent::CoordinationRatioReported {
        metadata: EventMetadata::new("quest-engine"),
        coordination_cost_ms: 500.0,
        inference_gain: 0.8,
        cost_index: 0.5,
        gain_index: 0.8,
        ratio: 0.625,
        is_paradox_risk: false,
        threshold: 1.0,
        sample_count: 10,
    };
    assert_eq!(event.metadata().source, "quest-engine");
}

/// 验证 `CoordinationRatioReported` 为 Normal 严重级别
///
/// WHY Normal:这是周期性指标报告,非阻断性事件。推理悖论风险告警
/// 由 efficiency-monitor 订阅后通过 EfficiencyAlertTriggered 二次发布,
/// 不走 mpsc 旁路通道(§6.2 红线 5 仅适用于 Critical 安全事件)。
#[test]
fn test_p2_1_coordination_ratio_reported_severity_normal() {
    let event = NexusEvent::CoordinationRatioReported {
        metadata: EventMetadata::new("quest-engine"),
        coordination_cost_ms: 1000.0,
        inference_gain: 0.1,
        cost_index: 1.0,
        gain_index: 0.1,
        ratio: 10.0,
        is_paradox_risk: true, // 即使触发推理悖论风险,事件本身仍为 Normal
        threshold: 1.0,
        sample_count: 5,
    };
    assert_eq!(
        event.severity(),
        event_bus::EventSeverity::Normal,
        "CoordinationRatioReported 必须为 Normal,告警由订阅者处理"
    );
}

/// 验证 `CoordinationRatioReported` 归入 Quest 主题
///
/// 该事件由 L9 quest-engine 发布,归入 Quest 主题组,
/// 与 ThinkingModeSwitched / QuestCompleted 等同级。
#[test]
fn test_p2_1_coordination_ratio_reported_topic_quest() {
    let event = NexusEvent::CoordinationRatioReported {
        metadata: EventMetadata::new("quest-engine"),
        coordination_cost_ms: 300.0,
        inference_gain: 0.9,
        cost_index: 0.3,
        gain_index: 0.9,
        ratio: 0.333,
        is_paradox_risk: false,
        threshold: 1.0,
        sample_count: 1,
    };
    assert_eq!(
        event.topic(),
        event_bus::topic::EventTopic::Quest,
        "CoordinationRatioReported 应归入 Quest 主题"
    );
}

/// 验证 `CoordinationRatioReported` 的序列化/反序列化往返
///
/// 确保事件的 serde tag="type" content="data" 格式正确,
/// 且所有字段(包括 f64 的 ratio / INFINITY 边界)都能正确往返。
#[test]
fn test_p2_1_coordination_ratio_reported_serialization_roundtrip() {
    let event = NexusEvent::CoordinationRatioReported {
        metadata: EventMetadata::new("quest-engine"),
        coordination_cost_ms: 750.0,
        inference_gain: 0.65,
        cost_index: 0.75,
        gain_index: 0.65,
        ratio: 1.153846,
        is_paradox_risk: true,
        threshold: 1.0,
        sample_count: 42,
    };
    let json = serde_json::to_string(&event).expect("序列化失败");
    assert!(
        json.contains("CoordinationRatioReported"),
        "JSON 应包含 type tag: {json}"
    );
    let decoded: NexusEvent = serde_json::from_str(&json).expect("反序列化失败");
    match decoded {
        NexusEvent::CoordinationRatioReported {
            coordination_cost_ms,
            inference_gain,
            cost_index,
            gain_index,
            ratio,
            is_paradox_risk,
            threshold,
            sample_count,
            ..
        } => {
            assert!((coordination_cost_ms - 750.0).abs() < 1e-6);
            assert!((inference_gain - 0.65).abs() < 1e-6);
            assert!((cost_index - 0.75).abs() < 1e-6);
            assert!((gain_index - 0.65).abs() < 1e-6);
            assert!((ratio - 1.153846).abs() < 1e-6);
            assert!(is_paradox_risk);
            assert!((threshold - 1.0).abs() < 1e-6);
            assert_eq!(sample_count, 42);
        }
        other => panic!(
            "Expected CoordinationRatioReported, got {:?}",
            other.type_name()
        ),
    }
}

/// 验证 `CoordinationRatioReported` 能承载 INFINITY ratio(增益为零的边界)
///
/// 当 gain_index = 0.0 时 ratio = f64::INFINITY,必须能正确构造与访问。
/// 这是推理悖论的极端场景:有协调成本但无推理增益。
///
/// WHY 不测试 JSON 序列化往返:serde_json 将 f64::INFINITY 序列化为 null,
/// 反序列化时 null 无法还原为 f64::INFINITY(JSON 规范不支持 Infinity)。
/// 生产环境使用 MessagePack(rmp-serde,ADR-004)序列化,支持 INFINITY。
/// 此处仅验证事件构造与字段访问,序列化兼容性由 MessagePack 保证。
#[test]
fn test_p2_1_coordination_ratio_reported_infinity_ratio() {
    let event = NexusEvent::CoordinationRatioReported {
        metadata: EventMetadata::new("quest-engine"),
        coordination_cost_ms: 500.0,
        inference_gain: 0.0,
        cost_index: 0.5,
        gain_index: 0.0,
        ratio: f64::INFINITY,
        is_paradox_risk: true,
        threshold: 1.0,
        sample_count: 3,
    };
    // 验证事件可正确构造与字段访问
    match event {
        NexusEvent::CoordinationRatioReported {
            ratio,
            is_paradox_risk,
            gain_index,
            ..
        } => {
            assert!(ratio.is_infinite(), "ratio 应为 INFINITY");
            assert!(ratio.is_sign_positive(), "ratio 应为正无穷");
            assert!(is_paradox_risk, "增益为零时必然触发推理悖论风险");
            assert_eq!(gain_index, 0.0);
        }
        _ => panic!("Expected CoordinationRatioReported"),
    }
}

// ============================================================
// L8 协调度量接线闭环:DebateCompleted / DelegationCompleted 事件测试
// ============================================================

/// 构造测试用 DebateCompleted 事件(Full 策略共识达成场景)
fn make_debate_completed() -> NexusEvent {
    NexusEvent::DebateCompleted {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-1".into(),
        proposal_id: "p-1".into(),
        debate_latency_ms: 42.5,
        strategy: "full".into(),
        weighted_approval_rate: Some(0.85),
        participation_rate: Some(1.0),
        divergence: Some(0.2),
        abstention_rate: Some(0.1),
        consensus_margin: Some(0.25),
        outcome: "Reached".into(),
    }
}

/// 构造测试用 DelegationCompleted 事件(4 子任务 3 成功场景)
fn make_delegation_completed() -> NexusEvent {
    NexusEvent::DelegationCompleted {
        metadata: EventMetadata::new("chimera-mas:DelegationExecutor"),
        parent_id: "root-1".into(),
        quest_id: Some("q-1".into()),
        total_overhead_ms: 120.0,
        sub_task_count: 4,
        success_count: 3,
    }
}

/// 验证两个新观测事件的 type_name 稳定性与 metadata 可取性
///
/// type_name 不允许变更,否则会破坏 quest-engine 订阅器的事件匹配
/// 与 TUI 事件分类(同 CoordinationRatioReported 稳定性要求)。
#[test]
fn test_debate_delegation_completed_type_name_and_metadata() {
    let debate = make_debate_completed();
    assert_eq!(debate.type_name(), "DebateCompleted");
    assert_eq!(debate.metadata().source, "parliament");

    let delegation = make_delegation_completed();
    assert_eq!(delegation.type_name(), "DelegationCompleted");
    assert_eq!(
        delegation.metadata().source,
        "chimera-mas:DelegationExecutor"
    );
}

/// 验证两个新观测事件为 Normal 严重级别
///
/// WHY Normal:它们是只读延迟/质量观测事件,丢失仅影响单次度量样本
/// (Option 字段保持 None,EWMA 不阻塞),不影响共识/安全决策,
/// 不得占用仅为稀有安全告警保留的 mpsc 旁路通道(§6.2 红线 5)。
#[test]
fn test_debate_delegation_completed_severity_normal() {
    assert_eq!(make_debate_completed().severity(), EventSeverity::Normal);
    assert_eq!(
        make_delegation_completed().severity(),
        EventSeverity::Normal
    );
}

/// 验证两个新观测事件的主题归类
///
/// DebateCompleted 归 Parliament(与 DebateStarted/ConsensusReached 同组),
/// DelegationCompleted 归 Agent(与 AgentTaskCompleted 同组),
/// 使订阅者按主题过滤即可获取完整生命周期事件。
#[test]
fn test_debate_delegation_completed_topic() {
    assert_eq!(
        make_debate_completed().topic(),
        event_bus::topic::EventTopic::Parliament
    );
    assert_eq!(
        make_delegation_completed().topic(),
        event_bus::topic::EventTopic::Agent
    );
}

/// 验证 DebateCompleted 的序列化/反序列化往返(含 Option 字段两态)
#[test]
fn test_debate_completed_serialization_roundtrip() {
    // 态 1:有投票数据(Simplified/Full 路径)
    let json = serde_json::to_string(&make_debate_completed()).expect("序列化失败");
    assert!(
        json.contains("DebateCompleted"),
        "JSON 应含 type tag: {json}"
    );
    let decoded: NexusEvent = serde_json::from_str(&json).expect("反序列化失败");
    match decoded {
        NexusEvent::DebateCompleted {
            quest_id,
            debate_latency_ms,
            strategy,
            weighted_approval_rate,
            participation_rate,
            divergence,
            abstention_rate,
            consensus_margin,
            outcome,
            ..
        } => {
            assert_eq!(quest_id, "q-1");
            assert!((debate_latency_ms - 42.5).abs() < 1e-6);
            assert_eq!(strategy, "full");
            assert!((weighted_approval_rate.expect("应有赞成率") - 0.85).abs() < 1e-6);
            assert!((participation_rate.expect("应有参与率") - 1.0).abs() < 1e-6);
            // M2-T2.2:多维质量字段往返
            assert!((divergence.expect("应有分歧度") - 0.2).abs() < 1e-6);
            assert!((abstention_rate.expect("应有弃权率") - 0.1).abs() < 1e-6);
            assert!((consensus_margin.expect("应有共识裕度") - 0.25).abs() < 1e-6);
            assert_eq!(outcome, "Reached");
        }
        other => panic!("Expected DebateCompleted, got {:?}", other.type_name()),
    }

    // 态 2:无投票数据(FastPath/Vetoed 路径,Option 字段为 None)
    let vetoed = NexusEvent::DebateCompleted {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-2".into(),
        proposal_id: "p-2".into(),
        debate_latency_ms: 3.2,
        strategy: "fast-path".into(),
        weighted_approval_rate: None,
        participation_rate: None,
        divergence: None,
        abstention_rate: None,
        consensus_margin: None,
        outcome: "Vetoed".into(),
    };
    let json = serde_json::to_string(&vetoed).expect("序列化失败");
    let decoded: NexusEvent = serde_json::from_str(&json).expect("反序列化失败");
    match decoded {
        NexusEvent::DebateCompleted {
            weighted_approval_rate,
            participation_rate,
            divergence,
            outcome,
            ..
        } => {
            assert!(weighted_approval_rate.is_none(), "否决路径无投票数据");
            assert!(participation_rate.is_none());
            assert!(divergence.is_none(), "无投票路径多维质量也为 None");
            assert_eq!(outcome, "Vetoed");
        }
        _ => panic!("Expected DebateCompleted"),
    }
}

/// 验证 DelegationCompleted 的序列化/反序列化往返
#[test]
fn test_delegation_completed_serialization_roundtrip() {
    let json = serde_json::to_string(&make_delegation_completed()).expect("序列化失败");
    assert!(
        json.contains("DelegationCompleted"),
        "JSON 应含 type tag: {json}"
    );
    let decoded: NexusEvent = serde_json::from_str(&json).expect("反序列化失败");
    match decoded {
        NexusEvent::DelegationCompleted {
            parent_id,
            quest_id,
            total_overhead_ms,
            sub_task_count,
            success_count,
            ..
        } => {
            assert_eq!(parent_id, "root-1");
            assert_eq!(quest_id.as_deref(), Some("q-1"));
            assert!((total_overhead_ms - 120.0).abs() < 1e-6);
            assert_eq!(sub_task_count, 4);
            assert_eq!(success_count, 3);
        }
        other => panic!("Expected DelegationCompleted, got {:?}", other.type_name()),
    }
}

/// 验证 BenchmarkMetricsCollected 为 Normal 级别（基准模式观测面事件）
#[test]
fn test_benchmark_metrics_collected_severity_normal() {
    let event = NexusEvent::BenchmarkMetricsCollected {
        metadata: EventMetadata::new("efficiency-monitor"),
        equivalent_input_cost_micro: 1250,
        vendor_cache_hit_rate_percent: 66,
        semantic_cache_hit_rate_percent: 40,
        ttft_p95_ms: 320,
        total_output_tokens: 5000,
        task_success_rate_percent: 95,
        per_vendor_snapshot_json: "{}".into(),
    };
    assert_eq!(
        event.severity(),
        EventSeverity::Normal,
        "基准指标采集必须为 Normal（观测面事件，不阻断系统）"
    );
    assert_eq!(event.type_name(), "BenchmarkMetricsCollected");
    assert!(!event.metadata().source.is_empty());
}
