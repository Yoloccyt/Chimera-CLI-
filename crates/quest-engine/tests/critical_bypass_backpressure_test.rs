//! Critical 事件 mpsc 旁路背压契约测试(P0-1 修复)
//!
//! 背景:审计报告 `docs/audit/quad-perspective-progress-audit-v2.25.0-omega.md` SYN-P0-1
//! 指出:quest-engine 的 ambient_mode 双通道订阅(broadcast Normal + mpsc 旁路 Critical)
//! 仅测试了"资源恢复 → Quest 恢复"闭环,未覆盖 mpsc 通道背压满时旁路触发集
//! (现 `CRITICAL_MPSC_VARIANTS` = 18)的 `[Critical]`
//! 事件在 §3.4 "Critical 事件必须可靠投递"红线下的契约行为(阻塞 vs 失败 vs 丢弃)。
//!
//! 本文件端到端验证 event-bus 的 mpsc 旁路投递契约(不修改任何 src 代码):
//!
//! ## 契约语义(基于 bus.rs 实际实现)
//! - **投递方式**: `subscribe_critical_events()` 创建有界 `tokio::sync::mpsc::channel`,
//!   容量 `CRITICAL_CHANNEL_CAPACITY = 4096`(bus.rs 编译时常量,不可配置);
//!   发布侧经 `send_critical_mpsc` 用 **`try_send`**(同步非阻塞)投递(bus.rs `send_critical_mpsc`)。
//! - **容量内**: `try_send` 返回 `Ok(())`,事件可靠投递,`critical_dropped_count` 不增长。
//! - **溢出时**: `try_send` 返回 `Err(Full)`,事件被**显式丢弃并递增
//!   `critical_dropped_count`**(优先级采样丢弃策略)——即"显式失败(可观测计数)"语义:
//!   **非阻塞**(publish 仍立即返回 Ok,无背压等待)、**非静默丢弃**(丢弃数可观测)。
//!   `Err(Closed)` 表示订阅者已 drop,对应 Sender 被移除(不属于本测试契约范围)。
//!
//! ## 构造清单构成(26 个事件 + 1 个 Normal 对照 = 注入 27)
//!
//! 构造清单 = **mpsc 旁路触发集全量 18 个** + **非旁路对照 8 个**:
//!
//! - **旁路 18 个**(权威清单 = `bus.rs::is_critical_mpsc_event` 的 matches! 臂,
//!   规模锚定常量 `CRITICAL_MPSC_VARIANTS`):SkepticVeto / RedTeamAudit /
//!   BudgetExceeded / AgentTaskFailed / AsaIntervention / AffinityQuotaExhausted /
//!   R2FreezeViolation / R2FreezeRollbackFailed / FormalViolation / VetoOverridden /
//!   R1ShadowRollbackFailed / StopRulingIssued / ErrorSignatureMatched /
//!   FormalVerificationFailed / CertifyIssued / FailureRegistered /
//!   ShadowBreakerTripped / SandboxViolation
//! - **非旁路对照 8 个**:4 个 severity Critical 但只走 broadcast
//!   (CheckpointSaved / ConsensusReached / SlowConsumerDropped / OrphanCallDetected,
//!   即 `CRITICAL_TOTAL - CRITICAL_MPSC_VARIANTS` 的差集) + 4 个 Normal
//!   (CapabilityFrozen / AgentContextOverflow / ResourceRecovered / RewardSignalReported)
//!
//! ## 两张清单的关系(双清单同步红线)
//!
//! - **旁路集 ⊆ severity Critical 集**:18 个旁路变体在定级表
//!   (`event-bus/src/registry.rs` 的 `define_event_registry!`)中**全部**同时定级 Critical。
//!   故旁路集从 10 扩到 18 **不改变** broadcast 侧的期望集。
//! - **两通道是并集不是二选一**:`bus.rs` 派发宏先走 mpsc 旁路后**照发 broadcast**
//!   ("旁路不减 broadcast,两通道是并集不是二选一"),故 broadcast 侧收到**全部**注入事件
//!   (26 清单 + 1 Normal),而非"severity Critical ∩ 非旁路"子集。
//!   旁路扩容只影响 mpsc 侧的期望集(10 → 18)。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::time::Duration;

use event_bus::payloads::{RollbackDiagnosticContext, RollbackTriggerType};
use event_bus::{
    EventBus, EventMetadata, EventReceiver, EventSeverity, NexusEvent, CRITICAL_MPSC_VARIANTS,
    CRITICAL_TOTAL,
};
use nexus_contracts::behavior_contract::ContractContext;
use nexus_contracts::reward::RewardSignal;

/// 构造 Critical 事件清单(26 个:旁路 18 + 非旁路对照 8)
///
/// 每个变体携带唯一可辨识字段,便于端到端断言"按序投递、无一丢失"。
fn audit_critical_events() -> Vec<NexusEvent> {
    let meta = || EventMetadata::new("critical-bypass-test");
    vec![
        NexusEvent::CheckpointSaved {
            metadata: meta(),
            quest_id: "q-1".into(),
            checkpoint_id: "cp-1".into(),
            memory_snapshot_hash: "hash-1".into(),
        },
        NexusEvent::ConsensusReached {
            metadata: meta(),
            quest_id: "q-1".into(),
            decision_hash: "dec-1".into(),
            dpo_pair_id: None,
        },
        NexusEvent::BudgetExceeded {
            metadata: meta(),
            budget_type: "memory".into(),
            current: 120,
            limit: 100,
        },
        NexusEvent::CapabilityFrozen {
            metadata: meta(),
            capability_id: "cap-1".into(),
            reason: "decay-score-below-threshold".into(),
        },
        NexusEvent::SandboxViolation {
            metadata: meta(),
            violation_type: "fs_write".into(),
            detail: "sandbox-deny".into(),
        },
        NexusEvent::SlowConsumerDropped {
            metadata: meta(),
            subscriber_id: "sub-1".into(),
            lag: 10,
            dropped_count: 5,
        },
        NexusEvent::OrphanCallDetected {
            metadata: meta(),
            operation_id: "op-1".into(),
            spawn_location: "gatherer.rs:42".into(),
        },
        NexusEvent::SkepticVeto {
            metadata: meta(),
            quest_id: "q-1".into(),
            veto_reason: "unsafe-shell-injection".into(),
            frozen_capabilities: vec!["shell_exec".into()],
        },
        NexusEvent::VetoOverridden {
            metadata: meta(),
            quest_id: "q-1".into(),
            proposal_id: "p-1".into(),
            veto_reason: "unsafe-shell-injection".into(),
            override_reason: "legitimate-script".into(),
            override_by: "admin:alice".into(),
        },
        NexusEvent::RedTeamAudit {
            metadata: meta(),
            vulnerability_type: "prompt_injection".into(),
            failed_probes: 2,
            total_probes: 10,
            detection_rate: 0.2,
            remediation_suggestion: "add-input-sanitization".into(),
        },
        NexusEvent::AsaIntervention {
            metadata: meta(),
            operation_id: "op-2".into(),
            action: "Block".into(),
            safety_score: 0.2,
            block_reason: Some("unsafe-operation".into()),
            alternative_suggestion: None,
        },
        NexusEvent::AgentTaskFailed {
            metadata: meta(),
            from: "agent-a".into(),
            to: "root".into(),
            task_id: "t-1".into(),
            error: "timeout".into(),
            retry_count: 2,
        },
        NexusEvent::AgentContextOverflow {
            metadata: meta(),
            agent_id: "agent-a".into(),
            current_tokens: 120_000,
            max_tokens: 100_000,
        },
        NexusEvent::ResourceRecovered {
            metadata: meta(),
            resource_type: "memory".into(),
        },
        NexusEvent::FormalViolation {
            metadata: meta(),
            contract_id: "bc-1".into(),
            target_type: "event_bus::EventBus".into(),
            violations: vec!["v1".into()],
            context: ContractContext::Runtime,
        },
        NexusEvent::RewardSignalReported {
            metadata: meta(),
            signal: RewardSignal {
                spec_id: "spec-1".into(),
                raw_reward: 1.0,
                weighted_reward: 1.0,
                timestamp_ms: 0,
                is_security_observation: false,
            },
        },
        // P2-1 补充:旁路触发集内、原审计 16 清单未覆盖的 3 个事件,
        // 使审计清单与 mpsc 旁路触发集交集完整可测(4→9)。
        NexusEvent::AffinityQuotaExhausted {
            metadata: meta(),
            route_key: "deepseek/deepseek-v4".into(),
            reason: "429 quota exhausted for this month".into(),
        },
        NexusEvent::R2FreezeViolation {
            metadata: meta(),
            violation_type: "CiDetection".into(),
            evidence: "gsoe-evolution/src/r2_path.rs:42".into(),
        },
        NexusEvent::R2FreezeRollbackFailed {
            metadata: meta(),
            reason: "git revert conflict".into(),
        },
        // ── 旁路全集补齐(在途基线扩展的 7 个旁路变体)──────────────────
        // 权威清单 = bus.rs `is_critical_mpsc_event` 的 matches! 臂,
        // 规模锚定 `CRITICAL_MPSC_VARIANTS`(现取 18)。此前本清单只覆盖 11 个,
        // 漏 7 个 ⇒ 旁路侧期望集与实际投递集长期不一致(本测试红)。
        NexusEvent::R1ShadowRollbackFailed {
            metadata: meta(),
            reason: "r1 shadow rollback conflict".into(),
            trigger_type: RollbackTriggerType::Unknown,
            triggered_at: None,
            details: "capability registry rollback failed".into(),
            diagnostic: RollbackDiagnosticContext::default(),
        },
        NexusEvent::StopRulingIssued {
            metadata: meta(),
            quest_id: "q-1".into(),
            reason: "stagnation".into(),
            preserve_best: true,
        },
        NexusEvent::ErrorSignatureMatched {
            metadata: meta(),
            error_hash: "sig-1".into(),
            matched_card_ids: vec!["card-1".into()],
        },
        NexusEvent::FormalVerificationFailed {
            metadata: meta(),
            property: "property-1".into(),
            counterexample: "ce-1".into(),
            generation: 0u64,
        },
        NexusEvent::CertifyIssued {
            metadata: meta(),
            claim_ref: "claim-1".into(),
            subject: "subject-1".into(),
            verifier: "verifier-1".into(),
        },
        NexusEvent::FailureRegistered {
            metadata: meta(),
            signature: "sig-1".into(),
            affected_component: "gsoe-evolution".into(),
            fix_status: "Open".into(),
        },
        NexusEvent::ShadowBreakerTripped {
            metadata: meta(),
            reason: "formal property violation".into(),
        },
    ]
}

/// mpsc 旁路触发集全集(bus.rs `is_critical_mpsc_event`)= 18 个
///
/// 权威清单 = `bus.rs::is_critical_mpsc_event` 的 matches! 臂,规模锚定常量
/// `CRITICAL_MPSC_VARIANTS`(现取 18):SkepticVeto/RedTeamAudit/BudgetExceeded/
/// AgentTaskFailed/AsaIntervention/AffinityQuotaExhausted/R2FreezeViolation/
/// R2FreezeRollbackFailed/FormalViolation/VetoOverridden/R1ShadowRollbackFailed/
/// StopRulingIssued/ErrorSignatureMatched/FormalVerificationFailed/CertifyIssued/
/// FailureRegistered/ShadowBreakerTripped/SandboxViolation。
///
/// 此集合与 `severity()` 是两张独立清单(双清单同步红线),但当前 18 项**全部**
/// 同时定级 Critical(定级表 = registry.rs 的 `define_event_registry!`),
/// 故"旁路 ∩ severity Critical"= 旁路全集本身。
fn is_audit_bypass_event(event: &NexusEvent) -> bool {
    matches!(
        event,
        NexusEvent::SkepticVeto { .. }
            | NexusEvent::RedTeamAudit { .. }
            | NexusEvent::BudgetExceeded { .. }
            | NexusEvent::AgentTaskFailed { .. }
            | NexusEvent::AsaIntervention { .. }
            | NexusEvent::AffinityQuotaExhausted { .. }
            | NexusEvent::R2FreezeViolation { .. }
            | NexusEvent::R2FreezeRollbackFailed { .. }
            | NexusEvent::FormalViolation { .. }
            | NexusEvent::VetoOverridden { .. }
            | NexusEvent::R1ShadowRollbackFailed { .. }
            | NexusEvent::StopRulingIssued { .. }
            | NexusEvent::ErrorSignatureMatched { .. }
            | NexusEvent::FormalVerificationFailed { .. }
            | NexusEvent::CertifyIssued { .. }
            | NexusEvent::FailureRegistered { .. }
            | NexusEvent::ShadowBreakerTripped { .. }
            | NexusEvent::SandboxViolation { .. }
    )
}

/// 带超时的 broadcast 接收(避免测试挂起;显式错误处理,禁止 unwrap/expect)
async fn recv_broadcast(rx: &mut EventReceiver, timeout: Duration) -> Result<NexusEvent, String> {
    match tokio::time::timeout(timeout, rx.recv()).await {
        Ok(Ok(event)) => Ok(event),
        Ok(Err(e)) => Err(format!("broadcast 接收错误: {e:?}")),
        Err(_) => Err("broadcast 接收超时".to_string()),
    }
}

/// 带超时的 mpsc 旁路接收(超时/关闭均返回 None;禁止 unwrap/expect)
async fn recv_critical(
    rx: &mut tokio::sync::mpsc::Receiver<NexusEvent>,
    timeout: Duration,
) -> Option<NexusEvent> {
    match tokio::time::timeout(timeout, rx.recv()).await {
        Ok(Some(event)) => Some(event),
        Ok(None) | Err(_) => None,
    }
}

/// 测试 1(容量内投递):注入 27 个事件(26 构造清单 + 1 Normal 对照),
/// 断言全部投递成功 —— broadcast 保序收到全部 27 个,mpsc 旁路收到且仅收到
/// 旁路触发集全集(`CRITICAL_MPSC_VARIANTS` 现取 18),
/// 全程 `critical_dropped_count == 0`。
#[tokio::test]
async fn critical_events_within_capacity_all_delivered() {
    let bus = EventBus::new();
    let mut broadcast_rx = bus.subscribe(); // 先 subscribe 再发布(异步红线)
    let mut critical_rx = bus.subscribe_critical_events(); // 先订阅旁路再发布

    let mut events = audit_critical_events(); // 18 旁路 + 8 非旁路对照
    assert_eq!(events.len(), 26, "构造清单应构造齐全(18 旁路 + 8 非旁路)");
    // 构造清单覆盖 severity Critical 全集(18 旁路 + 4 只走 broadcast 的历史 Critical)
    let critical_in_list = events
        .iter()
        .filter(|e| e.severity() == EventSeverity::Critical)
        .count();
    assert_eq!(
        critical_in_list, CRITICAL_TOTAL,
        "构造清单的 severity Critical 覆盖数应与 CRITICAL_TOTAL 一致"
    );
    // 1 个 Normal 事件(severity Normal,不走旁路,作为对照)
    events.push(NexusEvent::QuestCreated {
        metadata: EventMetadata::new("critical-bypass-test"),
        quest_id: "q-27".into(),
        title: "普通事件".into(),
        task_count: 1,
    });
    assert_eq!(events.len(), 27, "共注入 27 个事件");

    let expected: Vec<&str> = events.iter().map(|e| e.type_name()).collect();
    for event in events {
        let result = bus.publish(event).await;
        assert!(result.is_ok(), "容量内 publish 应成功,实际: {result:?}");
    }

    // broadcast 侧:按注入顺序收到全部 27 个事件(容量内 broadcast 无丢失)
    // WHY 旁路不减 broadcast:bus.rs 派发宏先走 mpsc 旁路后照发 broadcast
    // (两通道是并集不是二选一),故 broadcast 侧期望集 = 全部注入事件。
    let mut received: Vec<String> = Vec::with_capacity(27);
    for _ in 0..27 {
        match recv_broadcast(&mut broadcast_rx, Duration::from_secs(2)).await {
            Ok(event) => received.push(event.type_name().to_string()),
            Err(msg) => panic!("broadcast 应收到全部 27 个事件: {msg}"),
        }
    }
    assert_eq!(
        received, expected,
        "broadcast 应保序送达全部 27 个事件(26 构造清单 + 1 Normal)"
    );
    // 无多余事件
    let extra = tokio::time::timeout(Duration::from_millis(100), broadcast_rx.recv()).await;
    assert!(extra.is_err(), "broadcast 不应有多余事件");

    // mpsc 旁路侧:收到且仅收到旁路触发集全集(18 个)
    let mut bypass_received: HashSet<String> = HashSet::new();
    for _ in 0..CRITICAL_MPSC_VARIANTS {
        match recv_critical(&mut critical_rx, Duration::from_secs(2)).await {
            Some(event) => {
                bypass_received.insert(event.type_name().to_string());
            }
            None => panic!("旁路应收到 {CRITICAL_MPSC_VARIANTS} 个 mpsc Critical 事件"),
        }
    }
    let expected_bypass: HashSet<String> = [
        "SkepticVeto",
        "RedTeamAudit",
        "BudgetExceeded",
        "AgentTaskFailed",
        "AsaIntervention",
        "AffinityQuotaExhausted",
        "R2FreezeViolation",
        "R2FreezeRollbackFailed",
        "FormalViolation",
        "VetoOverridden",
        "R1ShadowRollbackFailed",
        "StopRulingIssued",
        "ErrorSignatureMatched",
        "FormalVerificationFailed",
        "CertifyIssued",
        "FailureRegistered",
        "ShadowBreakerTripped",
        "SandboxViolation",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(
        expected_bypass.len(),
        CRITICAL_MPSC_VARIANTS,
        "旁路期望集规模应与 CRITICAL_MPSC_VARIANTS 一致"
    );
    assert_eq!(
        bypass_received, expected_bypass,
        "旁路应仅收到 mpsc 触发集内的 Critical 事件"
    );
    let extra_critical = tokio::time::timeout(Duration::from_millis(100), critical_rx.recv()).await;
    assert!(extra_critical.is_err(), "旁路不应有多余事件");

    // 容量内投递:不得产生任何丢弃
    assert_eq!(
        bus.critical_dropped_count(),
        0,
        "容量内投递 Critical 事件不得被丢弃(禁止静默丢弃)"
    );
}

/// 测试 2(溢出契约):mpsc 容量固定为 `CRITICAL_CHANNEL_CAPACITY = 4096`
/// (实现级常量,`subscribe_critical_events` 不可配置容量),模拟慢消费者
/// (不消费 critical_rx),注入 4096 + 20 个 Critical 事件,断言:
/// - **显式失败而非静默丢弃**: `critical_dropped_count` 精确等于溢出数 20;
/// - **非阻塞**: 每次 publish 均立即返回 Ok(无背压等待、无报错);
/// - 容量内的 4096 个事件全部按 FIFO 送达(顺序完整,无内容损坏)。
#[tokio::test]
async fn overflow_fails_explicitly_not_silently_dropped() {
    let bus = EventBus::new();
    let mut critical_rx = bus.subscribe_critical_events(); // 先订阅再注入
    let capacity = bus.critical_channel_capacity();
    assert_eq!(capacity, 4096, "旁路容量应为实现常量 4096(bus.rs)");
    let overflow = 20usize; // 超出容量的注入数(与旁路清单规模无关,仅需为正)

    // 慢消费者场景:不消费 critical_rx,注入 容量 + 溢出 个 Critical 事件
    for i in 0..(capacity + overflow) {
        let event = NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("critical-bypass-test"),
            budget_type: format!("overflow-{i}"),
            current: 120,
            limit: 100,
        };
        let result = bus.publish(event).await;
        assert!(
            result.is_ok(),
            "第 {i} 次 publish 应返回 Ok(溢出语义为显式失败,非阻塞非报错)"
        );
    }

    // 契约断言 1:溢出事件被"显式失败"计数(可观测),而非静默丢弃
    let dropped = bus.critical_dropped_count();
    assert_eq!(
        dropped, overflow as u64,
        "溢出必须显式计入 critical_dropped_count(禁止静默丢弃): 期望 {overflow}, 实际 {dropped}"
    );

    // 契约断言 2:容量内的 4096 个事件全部按 FIFO 送达(未被溢出破坏)
    let mut received: Vec<String> = Vec::with_capacity(capacity);
    loop {
        match critical_rx.try_recv() {
            Ok(event) => {
                let NexusEvent::BudgetExceeded { budget_type, .. } = event else {
                    panic!("旁路应仅收到 BudgetExceeded");
                };
                received.push(budget_type);
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                panic!("旁路通道不应关闭");
            }
        }
    }
    assert_eq!(
        received.len(),
        capacity,
        "容量内 {capacity} 个事件应全部投递"
    );
    for (i, tag) in received.iter().enumerate() {
        assert_eq!(tag, &format!("overflow-{i}"), "FIFO 顺序应保持");
    }
}

/// 测试 3(逐个投递验证,加分):26 个构造清单事件逐个发布,
/// 确认每一个都经 broadcast 送达、旁路触发集内的 18 个经 mpsc 旁路送达,
/// 全程 `critical_dropped_count == 0` —— 无一被静默丢弃。
#[tokio::test]
async fn each_critical_variant_delivered_individually() {
    let bus = EventBus::new();
    let mut broadcast_rx = bus.subscribe();
    let mut critical_rx = bus.subscribe_critical_events();

    let mut bypass_seen = 0usize;
    for event in audit_critical_events() {
        let type_name = event.type_name();
        let bypass = is_audit_bypass_event(&event);

        let result = bus.publish(event).await;
        assert!(
            result.is_ok(),
            "publish {type_name} 应成功,实际: {result:?}"
        );

        // broadcast:容量内必须送达(无一被静默丢弃)
        match recv_broadcast(&mut broadcast_rx, Duration::from_secs(2)).await {
            Ok(received) => {
                assert_eq!(
                    received.type_name(),
                    type_name,
                    "broadcast 应收到 {type_name}"
                );
            }
            Err(msg) => panic!("{type_name} 未被 broadcast 送达: {msg}"),
        }

        // mpsc 旁路:仅旁路触发集内的事件应经旁路送达
        // WHY 逐个消费:mpsc 为 FIFO 且与 publish 同序,故"发一个旁路事件即取一个"
        // 是逐变体的顺序断言(而非仅计数)。
        if bypass {
            match recv_critical(&mut critical_rx, Duration::from_secs(2)).await {
                Some(received) => {
                    assert_eq!(received.type_name(), type_name, "旁路应收到 {type_name}");
                }
                None => panic!("{type_name} 应经 mpsc 旁路送达"),
            }
            bypass_seen += 1;
        }
    }

    // 逐个消费后旁路通道应为空(不多投),且旁路事件数 == 旁路清单规模
    assert_eq!(
        bypass_seen, CRITICAL_MPSC_VARIANTS,
        "旁路触发集内的变体数应与 CRITICAL_MPSC_VARIANTS 一致"
    );
    let extra_critical = tokio::time::timeout(Duration::from_millis(100), critical_rx.recv()).await;
    assert!(extra_critical.is_err(), "旁路不应有多余事件");

    // 全部 26 个变体投递完毕,不得产生任何丢弃
    assert_eq!(
        bus.critical_dropped_count(),
        0,
        "26 个构造清单事件逐个投递不得被丢弃(禁止静默丢弃)"
    );
}
