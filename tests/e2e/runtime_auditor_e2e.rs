//! polish-v2.7 P1-6:RuntimeAuditor 自我评估 E2E 测试
//!
//! 对应架构层:L9 Quest(efficiency-monitor)+ L1 Core(event-bus)
//! 对应 ADR:ADR-049 决策 1(runtime-auditor 落点)
//! 对应 KPI:KPI-P2(五维度报告可产出,Finding 证据 = 运行时事件)
//!
//! # 测试覆盖
//!
//! 1. **证据纪律端到端**:静态登记能力 → 无运行时证据 → UnusedCapability(static_only);
//!    有运行时证据 → VerifiedCapability(runtime_events)
//! 2. **事件链路**:AuditFindingRaised / HarnessReportGenerated 经 EventBus
//!    发布且订阅者可收到,severity = Normal(观察性事件不占用 Critical 通道)
//! 3. **五维度评分**:从真实事件流(QuestCreated/QuestCompleted/CheckpointSaved)
//!    计算的评分与预期比率一致
//! 4. **TUI 消费端**:SelfAssessment 面板能从事件流派生渲染内容(L10 消费闭环)

use efficiency_monitor::{FindingCategory, RuntimeAuditor};
use event_bus::{EventBus, EventMetadata, EventSeverity, NexusEvent, QuestStatus};

/// 证据纪律 E2E:配置的能力无运行时证据时必须产出 UnusedCapability
#[test]
fn e2e_evidence_discipline_static_config_is_not_verified() {
    let bus = EventBus::new();
    // §4.4 红线 3:先 subscribe 再触发发布
    let mut rx = bus.subscribe();
    let auditor = RuntimeAuditor::with_event_bus(bus);

    // 模拟启动阶段:静态配置登记 3 个能力,仅 1 个实际使用
    auditor.register_capability("cap-search");
    auditor.register_capability("cap-sandbox");
    auditor.register_capability("cap-ghost");
    auditor.record_capability_use("cap-search");

    let findings = auditor.audit_all_capabilities();
    assert_eq!(findings.len(), 3);

    let verified = findings
        .iter()
        .filter(|f| f.category == FindingCategory::VerifiedCapability)
        .count();
    let unused = findings
        .iter()
        .filter(|f| f.category == FindingCategory::UnusedCapability)
        .count();
    // 证据纪律:1 个已验证(有运行时证据),2 个未使用(仅静态配置)
    assert_eq!((verified, unused), (1, 2));

    // 事件链路:3 条 AuditFindingRaised 全部可被订阅者收到
    let mut received = 0;
    while let Ok(Some(event)) = rx.try_recv() {
        if let NexusEvent::AuditFindingRaised { evidence_kind, .. } = &event {
            // 观察性事件走 Normal 广播,不挤占 Critical mpsc 旁路
            assert_eq!(event.severity(), EventSeverity::Normal);
            assert!(evidence_kind == "static_only" || evidence_kind == "runtime_events");
            received += 1;
        }
    }
    assert_eq!(received, 3, "3 条审计发现应全部经 EventBus 投递");
}

/// 五维度报告 E2E:真实事件流 → 评分计算 → HarnessReportGenerated 发布
#[test]
fn e2e_five_dimension_report_from_event_stream() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let auditor = RuntimeAuditor::with_event_bus(bus);

    // 模拟一轮任务流:2 个 Quest 创建,1 个完成,1 次检查点沉淀
    for i in 0..2 {
        auditor.record_event(&NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: format!("q{i}"),
            title: format!("quest {i}"),
            task_count: 1,
        });
    }
    auditor.record_event(&NexusEvent::QuestCompleted {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: "q0".into(),
        status: QuestStatus::Completed,
    });
    auditor.record_event(&NexusEvent::CheckpointSaved {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: "q0".into(),
        checkpoint_id: "c0".into(),
        memory_snapshot_hash: "hash".into(),
    });

    let report = auditor.generate_report();
    // 可靠交付 = 1 完成 / 2 创建 = 0.5
    assert!((report.reliable_delivery - 0.5).abs() < f32::EPSILON);
    // 经验沉淀 = 1 检查点 / 1 完成 = 1.0
    assert_eq!(report.experience_accumulation, 1.0);
    // 无执行事件观测 → 可控执行为中性 0.5
    assert_eq!(report.controllable_execution, 0.5);

    // 事件链路:HarnessReportGenerated 可被订阅者收到且字段一致
    let mut report_seen = false;
    while let Ok(Some(event)) = rx.try_recv() {
        if let NexusEvent::HarnessReportGenerated {
            reliable_delivery,
            experience_accumulation,
            ..
        } = event
        {
            assert!((reliable_delivery - 0.5).abs() < f32::EPSILON);
            assert_eq!(experience_accumulation, 1.0);
            report_seen = true;
        }
    }
    assert!(report_seen, "HarnessReportGenerated 应经 EventBus 投递");
}

/// L10 消费闭环 E2E:SelfAssessment 面板从事件流派生渲染内容
#[test]
fn e2e_tui_self_assessment_panel_consumes_report() {
    use chimera_tui::panels::self_assessment::SelfAssessmentPanel;
    use chimera_tui::types::TuiState;

    let mut state = TuiState::new();
    // 模拟 DataPipeline 已将审计事件送入 latest_events
    state
        .latest_events
        .push_back(NexusEvent::HarnessReportGenerated {
            metadata: EventMetadata::new("efficiency-monitor"),
            task_comprehension: 0.9,
            controllable_execution: 0.7,
            change_verification: 0.5,
            reliable_delivery: 0.6,
            experience_accumulation: 0.4,
            findings_count: 1,
        });
    state
        .latest_events
        .push_back(NexusEvent::AuditFindingRaised {
            metadata: EventMetadata::new("efficiency-monitor"),
            finding_severity: "medium".into(),
            category: "unused_capability".into(),
            message: "capability 'cap-ghost' configured but never used".into(),
            evidence_kind: "static_only".into(),
            fix_hint: "remove or wire up".into(),
        });

    let content = SelfAssessmentPanel::content(&state).to_string();
    // 五维评分渲染(90% 为最新报告的任务理解维度)
    assert!(content.contains("90%"), "面板应渲染五维评分");
    // 审计发现渲染(含严重度标签与消息)
    assert!(content.contains("[medium]"), "面板应渲染发现严重度");
    assert!(content.contains("cap-ghost"), "面板应渲染发现消息");
}
