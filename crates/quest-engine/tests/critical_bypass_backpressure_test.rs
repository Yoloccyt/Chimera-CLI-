//! Critical 事件 mpsc 旁路背压契约测试(P0-1 修复)
//!
//! 背景:审计报告 `docs/audit/quad-perspective-progress-audit-v2.25.0-omega.md` SYN-P0-1
//! 指出:quest-engine 的 ambient_mode 双通道订阅(broadcast Normal + mpsc 旁路 Critical)
//! 仅测试了"资源恢复 → Quest 恢复"闭环,未覆盖 mpsc 通道背压满时 16 个 `[Critical]`
//! 事件在 §3.4 "Critical 事件必须可靠投递"红线下的契约行为(阻塞 vs 失败 vs 丢弃)。
//!
//! 本文件端到端验证 event-bus 的 mpsc 旁路投递契约(不修改任何 src 代码):
//!
//! ## 契约语义(基于 bus.rs 实际实现)
//! - **投递方式**: `subscribe_critical_events()` 创建有界 `tokio::sync::mpsc::channel`,
//!   容量 `CRITICAL_CHANNEL_CAPACITY = 4096`(bus.rs L33,编译时常量,不可配置);
//!   发布侧经 `send_critical_mpsc` 用 **`try_send`**(同步非阻塞)投递(bus.rs L611-650)。
//! - **容量内**: `try_send` 返回 `Ok(())`,事件可靠投递,`critical_dropped_count` 不增长。
//! - **溢出时**: `try_send` 返回 `Err(Full)`,事件被**显式丢弃并递增
//!   `critical_dropped_count`**(优先级采样丢弃策略)——即"显式失败(可观测计数)"语义:
//!   **非阻塞**(publish 仍立即返回 Ok,无背压等待)、**非静默丢弃**(丢弃数可观测)。
//!   `Err(Closed)` 表示订阅者已 drop,对应 Sender 被移除(不属于本测试契约范围)。
//!
//! ## 19 个 `[Critical]` 事件清单(审计报告 §7.2 line 518 + P2-1 补充 3 个旁路事件)
//! CheckpointSaved / ConsensusReached / BudgetExceeded / CapabilityFrozen /
//! SandboxViolation / SlowConsumerDropped / OrphanCallDetected / SkepticVeto /
//! VetoOverridden / RedTeamAudit / AsaIntervention / AgentTaskFailed /
//! AgentContextOverflow / ResourceRecovered / FormalViolation / RewardSignalReported /
//! AffinityQuotaExhausted / R2FreezeViolation / R2FreezeRollbackFailed(P2-1 补充)
//!
//! 注:该清单与 `NexusEvent::severity()` 的显式 Critical 分支(15 个)不完全重合
//! (审计清单另含 CapabilityFrozen/SandboxViolation/AgentContextOverflow/
//! ResourceRecovered/RewardSignalReported),本测试以审计清单为准;
//! mpsc 旁路触发集(`bus.rs is_critical_mpsc_event`)与审计 19 清单的交集为 9 个:
//! BudgetExceeded / SkepticVeto / RedTeamAudit / AsaIntervention / AgentTaskFailed /
//! AffinityQuotaExhausted / R2FreezeViolation / R2FreezeRollbackFailed / FormalViolation。

#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::time::Duration;

use event_bus::{EventBus, EventMetadata, EventReceiver, NexusEvent};
use nexus_contracts::behavior_contract::ContractContext;
use nexus_contracts::reward::RewardSignal;

/// 构造审计报告定义的 16 个 `[Critical]` 事件(顺序与清单一致)
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
    ]
}

/// 审计 19 清单 ∩ mpsc 旁路触发集(bus.rs `is_critical_mpsc_event`)= 9 个
///
/// 旁路触发集:SkepticVeto/RedTeamAudit/BudgetExceeded/AgentTaskFailed(P1-2 纳入)/
/// AsaIntervention/AffinityQuotaExhausted/R2FreezeViolation/R2FreezeRollbackFailed/
/// FormalViolation(P1-5 升级)。此集合与 `severity()` 是两张独立清单
/// (双清单同步红线),本测试仅锁定审计 19 清单 ∩ 旁路集的交集。
fn is_audit_bypass_event(event: &NexusEvent) -> bool {
    matches!(
        event,
        NexusEvent::BudgetExceeded { .. }
            | NexusEvent::SkepticVeto { .. }
            | NexusEvent::RedTeamAudit { .. }
            | NexusEvent::AsaIntervention { .. }
            | NexusEvent::AgentTaskFailed { .. }
            | NexusEvent::AffinityQuotaExhausted { .. }
            | NexusEvent::R2FreezeViolation { .. }
            | NexusEvent::R2FreezeRollbackFailed { .. }
            | NexusEvent::FormalViolation { .. }
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

/// 测试 1(容量内投递):注入 20 个事件(19 Critical + 1 Normal),
/// 断言全部投递成功 —— broadcast 保序收到全部 20 个,mpsc 旁路收到且仅收到
/// 旁路触发集内 9 个 Critical 事件,全程 `critical_dropped_count == 0`。
#[tokio::test]
async fn critical_events_within_capacity_all_delivered() {
    let bus = EventBus::new();
    let mut broadcast_rx = bus.subscribe(); // 先 subscribe 再发布(异步红线)
    let mut critical_rx = bus.subscribe_critical_events(); // 先订阅旁路再发布

    let mut events = audit_critical_events(); // 19 Critical
    assert_eq!(events.len(), 19, "审计 19 清单应构造齐全");
    // 1 个 Normal 事件(severity Normal,不走旁路,作为对照)
    events.push(NexusEvent::QuestCreated {
        metadata: EventMetadata::new("critical-bypass-test"),
        quest_id: "q-20".into(),
        title: "普通事件".into(),
        task_count: 1,
    });
    assert_eq!(events.len(), 20, "共注入 20 个事件");

    let expected: Vec<&str> = events.iter().map(|e| e.type_name()).collect();
    for event in events {
        let result = bus.publish(event).await;
        assert!(result.is_ok(), "容量内 publish 应成功,实际: {result:?}");
    }

    // broadcast 侧:按注入顺序收到全部 20 个事件(容量内 broadcast 无丢失)
    let mut received: Vec<String> = Vec::with_capacity(20);
    for _ in 0..20 {
        match recv_broadcast(&mut broadcast_rx, Duration::from_secs(2)).await {
            Ok(event) => received.push(event.type_name().to_string()),
            Err(msg) => panic!("broadcast 应收到全部 20 个事件: {msg}"),
        }
    }
    assert_eq!(
        received, expected,
        "broadcast 应保序送达全部 20 个事件(19 Critical + 1 Normal)"
    );
    // 无多余事件
    let extra = tokio::time::timeout(Duration::from_millis(100), broadcast_rx.recv()).await;
    assert!(extra.is_err(), "broadcast 不应有多余事件");

    // mpsc 旁路侧:收到且仅收到旁路触发集 ∩ 审计 19 清单的 9 个事件
    let mut bypass_received: HashSet<String> = HashSet::new();
    for _ in 0..9 {
        match recv_critical(&mut critical_rx, Duration::from_secs(2)).await {
            Some(event) => {
                bypass_received.insert(event.type_name().to_string());
            }
            None => panic!("旁路应收到 9 个 mpsc Critical 事件"),
        }
    }
    let expected_bypass: HashSet<String> = [
        "BudgetExceeded",
        "SkepticVeto",
        "RedTeamAudit",
        "AsaIntervention",
        "AgentTaskFailed",
        "AffinityQuotaExhausted",
        "R2FreezeViolation",
        "R2FreezeRollbackFailed",
        "FormalViolation",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
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
    let overflow = 20usize; // 超出容量的注入数(与 19 Critical + 1 Normal 对齐)

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

/// 测试 3(逐个投递验证,加分):19 个 `[Critical]` 事件逐个发布,
/// 确认每一个都经 broadcast 送达、旁路触发集内的 9 个经 mpsc 旁路送达,
/// 全程 `critical_dropped_count == 0` —— 无一被静默丢弃。
#[tokio::test]
async fn each_critical_variant_delivered_individually() {
    let bus = EventBus::new();
    let mut broadcast_rx = bus.subscribe();
    let mut critical_rx = bus.subscribe_critical_events();

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
        if bypass {
            match recv_critical(&mut critical_rx, Duration::from_secs(2)).await {
                Some(received) => {
                    assert_eq!(received.type_name(), type_name, "旁路应收到 {type_name}");
                }
                None => panic!("{type_name} 应经 mpsc 旁路送达"),
            }
        }
    }

    // 全部 19 个变体投递完毕,不得产生任何丢弃
    assert_eq!(
        bus.critical_dropped_count(),
        0,
        "19 个 Critical 事件逐个投递不得被丢弃(禁止静默丢弃)"
    );
}
