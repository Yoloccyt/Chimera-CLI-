//! bus 模块测试 — 整体自 `bus/mod.rs` 下沉(R4.4,L1-R4 批)
//!
//! 纯移动:tests_helpers(D-8 变体构造辅助)+ tests(发布/订阅/分片/
//! 双清单守护)。本模块仍是 `bus` 的子模块,子模块内的 `use super::*`
//! 以本文件为父节点;文件级再做一次 `use super::*`,即可链式访问
//! `bus/mod.rs` 的私有项(EventBus 私有字段、`is_critical_mpsc_event` 等),
//! 与移动前逐字等价。
#![allow(clippy::unwrap_used, clippy::expect_used)] // 原 mod.rs 上两个测试 mod 声明的 allow,随下沉整体保留

// WHY 文件级 glob:tests_helpers / tests 内的 `use super::*` 以本模块为父;
// 此处从 bus/mod.rs 做一次 glob 导入,子模块的 `use super::*` 即可链式拿到
// EventBus / EventMetadata / serialize_* 等 —— 纯移动,不改任何用例的导入写法。
use super::*;

// ============================================================
// P1-T12:测试辅助 — Critical 变体构造(D-8 口径,双清单同步红线)
// ============================================================

/// Critical 变体构造测试辅助(D-8 口径)
///
/// WHY pub(crate):shard.rs 的 Lane 判定全量断言(21 Critical → Critical)
/// 复用本模块构造事件,避免两处维护 22 个变体构造代码漂移(新增 Critical
/// 事件时只需改此处一处,三处清单同步守护见 test_critical_double_list_d8_counts)。
/// 仅测试构建存在(cfg(test)),生产零足迹。
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod tests_helpers {
    use super::*;

    /// 全量 18 个 mpsc 旁路变体构造(D-8 口径,双清单同步红线)
    pub fn all_mpsc_critical_variants() -> Vec<NexusEvent> {
        vec![
            // C-64b（M49 后半）：沙箱违规升 Critical 后进 mpsc 旁路
            NexusEvent::SandboxViolation {
                metadata: EventMetadata::new("t"),
                violation_type: "path_traversal".into(),
                detail: "attempted /etc/passwd".into(),
            },
            // v2.3 (b.1)/(b.4): Certify 与 FailureRegistry 进 mpsc 旁路清单(D-8 口径)
            NexusEvent::CertifyIssued {
                metadata: EventMetadata::new("t"),
                claim_ref: "c".into(),
                subject: "s".into(),
                verifier: "GovernanceGate".into(),
            },
            NexusEvent::FailureRegistered {
                metadata: EventMetadata::new("t"),
                signature: "sig".into(),
                affected_component: "crate".into(),
                fix_status: "Open".into(),
            },
            NexusEvent::SkepticVeto {
                metadata: EventMetadata::new("t"),
                quest_id: "q".into(),
                veto_reason: "r".into(),
                frozen_capabilities: vec![],
            },
            NexusEvent::RedTeamAudit {
                metadata: EventMetadata::new("t"),
                vulnerability_type: "prompt_injection".into(),
                failed_probes: 1,
                total_probes: 2,
                detection_rate: 0.5,
                remediation_suggestion: "s".into(),
            },
            NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new("t"),
                budget_type: "token".into(),
                current: 2,
                limit: 1,
            },
            NexusEvent::AgentTaskFailed {
                metadata: EventMetadata::new("t"),
                from: "agent-a".into(),
                to: "root".into(),
                task_id: "t-1".into(),
                error: "timeout".into(),
                retry_count: 0,
            },
            NexusEvent::AsaIntervention {
                metadata: EventMetadata::new("t"),
                operation_id: "op-1".into(),
                action: "Block".into(),
                safety_score: 0.1,
                block_reason: Some("unsafe".into()),
                alternative_suggestion: None,
            },
            NexusEvent::AffinityQuotaExhausted {
                metadata: EventMetadata::new("t"),
                route_key: "zhipu/glm-5.2".into(),
                reason: "quota".into(),
            },
            NexusEvent::R2FreezeViolation {
                metadata: EventMetadata::new("t"),
                violation_type: "CiDetection".into(),
                evidence: "ev".into(),
            },
            NexusEvent::R2FreezeRollbackFailed {
                metadata: EventMetadata::new("t"),
                reason: "git revert conflict".into(),
            },
            NexusEvent::FormalViolation {
                metadata: EventMetadata::new("t"),
                contract_id: "bc-1".into(),
                target_type: "event_bus::EventBus".into(),
                violations: vec!["v1".into()],
                context: nexus_contracts::behavior_contract::ContractContext::Runtime,
            },
            NexusEvent::VetoOverridden {
                metadata: EventMetadata::new("t"),
                quest_id: "q".into(),
                proposal_id: "p".into(),
                veto_reason: "r".into(),
                override_reason: "o".into(),
                override_by: "admin".into(),
            },
            NexusEvent::R1ShadowRollbackFailed {
                metadata: EventMetadata::new("t"),
                reason: "rollback conflict".into(),
                trigger_type: crate::types::RollbackTriggerType::Unknown,
                triggered_at: None,
                details: String::new(),
                diagnostic: crate::types::RollbackDiagnosticContext::default(),
            },
            NexusEvent::StopRulingIssued {
                metadata: EventMetadata::new("t"),
                quest_id: "q".into(),
                reason: "stagnation".into(),
                preserve_best: true,
            },
            NexusEvent::ErrorSignatureMatched {
                metadata: EventMetadata::new("t"),
                error_hash: "h".into(),
                matched_card_ids: vec![],
            },
            // L4 深度优化 P1-1:验证器失败升入 mpsc 旁路(双清单同步)
            NexusEvent::FormalVerificationFailed {
                metadata: EventMetadata::new("t"),
                property: "inv-1".into(),
                counterexample: "s".into(),
                generation: 1,
            },
            // C-13(M09): ShadowBreakerTripped 升入 mpsc 旁路(16 → 17,双清单同步)
            NexusEvent::ShadowBreakerTripped {
                metadata: EventMetadata::new("t"),
                reason: "r".into(),
            },
        ]
    }

    /// 全量 22 个 severity() Critical 变体构造(D-8 口径)
    ///
    /// 22 = 18(mpsc 旁路,见 [`all_mpsc_critical_variants`])
    ///   + 4(历史 Critical 只走 broadcast:CheckpointSaved/ConsensusReached/
    ///     SlowConsumerDropped/OrphanCallDetected,按既定设计不回退通道归属)。
    pub fn all_severity_critical_variants() -> Vec<NexusEvent> {
        let mut variants = all_mpsc_critical_variants();
        variants.extend([
            NexusEvent::CheckpointSaved {
                metadata: EventMetadata::new("t"),
                quest_id: "q".into(),
                checkpoint_id: "c".into(),
                memory_snapshot_hash: "h".into(),
            },
            NexusEvent::ConsensusReached {
                metadata: EventMetadata::new("t"),
                quest_id: "q".into(),
                decision_hash: "h".into(),
                dpo_pair_id: None,
            },
            NexusEvent::SlowConsumerDropped {
                metadata: EventMetadata::new("t"),
                subscriber_id: "sub".into(),
                lag: 1,
                dropped_count: 1,
            },
            NexusEvent::OrphanCallDetected {
                metadata: EventMetadata::new("t"),
                operation_id: "op".into(),
                spawn_location: "bus.rs".into(),
            },
        ]);
        variants
    }

    /// 按变体名构造 Critical 事件(供 LANE_FORBIDDEN_SHARD 逐名断言)
    ///
    /// 名字不在 22 清单中返回 None(调用方断言消息定位漂移名字)。
    pub fn critical_variant_by_name(name: &str) -> Option<NexusEvent> {
        all_severity_critical_variants()
            .into_iter()
            .find(|e| e.type_name() == name)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
// 命名说明：本模块（suite）所在的文件由 bus/mod.rs 以 `mod tests;` 挂载，
// 若此处仍叫 `tests` 会触发 clippy module_inception（同名父子模块）。
mod suite {
    use super::*;
    use crate::shard::{event_lane, Lane, ShadowStats, DEFAULT_SHARD_COUNT};
    use crate::types::EventMetadata;

    fn make_test_event() -> NexusEvent {
        NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-001".into(),
            title: "测试任务".into(),
            task_count: 3,
        }
    }

    #[tokio::test]
    async fn test_publish_subscribe_basic() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let event = make_test_event();
        bus.publish(event.clone()).await.unwrap();
        let received = rx.recv().await.unwrap();
        assert_eq!(received, event);
    }

    // ============================================================
    // 方向 3:单事件发布路径去重 — 两变体共享 dispatch_one 的等价守卫
    // ============================================================

    fn make_critical_event() -> NexusEvent {
        NexusEvent::SkepticVeto {
            metadata: EventMetadata::new("parliament"),
            quest_id: "q-veto".into(),
            veto_reason: "等价性守卫".into(),
            frozen_capabilities: vec!["cap-x".into()],
        }
    }

    /// `publish` 与 `publish_blocking` 必须行为等价(共享同一实现)
    ///
    /// # WHY 需要该用例
    /// 重构前这两条路径是约 88 行**逐行复制粘贴**的独立实现,日志文案的
    /// "(同步发布)" 后缀是唯一差异。任何发布逻辑改动都必须同步改两处,漏改即
    /// **静默分叉** —— 两条路径行为不再等价却无人察觉。本用例把"两路径等价"
    /// 固化为契约:若未来有人再次各写一份并只改其中一处,这里会红。
    #[tokio::test]
    async fn test_publish_and_blocking_are_equivalent() {
        let bus_async = EventBus::new();
        let mut rx_async = bus_async.subscribe();
        let bus_sync = EventBus::new();
        let mut rx_sync = bus_sync.subscribe();

        let before_async = bus_async.published_total();
        let before_sync = bus_sync.published_total();

        bus_async.publish(make_test_event()).await.unwrap();
        bus_sync.publish_blocking(make_test_event()).unwrap();

        // 1) 吞吐计数点一致:两条路径递增同一计数器
        assert_eq!(bus_async.published_total(), before_async + 1);
        assert_eq!(bus_sync.published_total(), before_sync + 1);

        // 2) 两条路径都实际投递到 broadcast 订阅者
        assert!(rx_async.recv().await.is_ok(), "async 路径应投递事件");
        assert!(rx_sync.recv().await.is_ok(), "blocking 路径应投递事件");
    }

    /// Critical 事件在两条路径上都必须走 mpsc 旁路(§6.2 红线双通道)
    ///
    /// 这是"共享实现"最容易被分叉破坏的可观测点:旁路投递若只在其中一条路径上
    /// 被漏掉,Critical 治理事件会在 broadcast Lagged 时永久丢失。
    #[tokio::test]
    async fn test_critical_bypass_present_on_both_publish_paths() {
        let bus_async = EventBus::new();
        let mut crit_async = bus_async.subscribe_critical_events();
        let bus_sync = EventBus::new();
        let mut crit_sync = bus_sync.subscribe_critical_events();

        bus_async.publish(make_critical_event()).await.unwrap();
        bus_sync.publish_blocking(make_critical_event()).unwrap();

        let got_async = tokio::time::timeout(Duration::from_millis(200), crit_async.recv())
            .await
            .expect("async 路径不应超时")
            .expect("async 路径应走 mpsc 旁路");
        let got_sync = tokio::time::timeout(Duration::from_millis(200), crit_sync.recv())
            .await
            .expect("blocking 路径不应超时")
            .expect("blocking 路径应走 mpsc 旁路");

        assert_eq!(got_async.type_name(), "SkepticVeto");
        assert_eq!(got_sync.type_name(), "SkepticVeto");
    }

    #[tokio::test]
    async fn test_no_subscribers_ok() {
        let bus = EventBus::new();
        // 无订阅者时发布应返回 Ok(()),非错误
        bus.publish(make_test_event()).await.unwrap();
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus = EventBus::new();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();
        let event = make_test_event();
        bus.publish(event.clone()).await.unwrap();
        assert_eq!(rx1.recv().await.unwrap(), event);
        assert_eq!(rx2.recv().await.unwrap(), event);
    }

    #[tokio::test]
    async fn test_recv_timeout() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let result = rx.recv_timeout(Duration::from_millis(50)).await;
        assert!(matches!(result, Err(EventBusError::RecvTimeout(_))));
    }

    // ============================================================
    // M1-T1.3:publish_batch 批量发布测试
    // ============================================================

    fn make_indexed_event(i: usize) -> NexusEvent {
        NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: format!("q-{i}"),
            title: format!("任务 {i}"),
            task_count: 1,
        }
    }

    #[tokio::test]
    async fn test_publish_batch_delivers_all_events() {
        // 批量发布后所有事件均被订阅者接收(计数与内容完整)
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let events: Vec<NexusEvent> = (0..5).map(make_indexed_event).collect();

        bus.publish_batch(events.clone()).await.unwrap();

        for expected in &events {
            let received = rx.recv().await.unwrap();
            assert_eq!(&received, expected, "批量发布应保留逐条内容与顺序");
        }
    }

    #[tokio::test]
    async fn test_publish_batch_empty_early_return() {
        // 空 Vec 早退:不发布任何事件,不 panic
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.publish_batch(Vec::new()).await.unwrap();
        // 超时无事件可收
        let result = rx.recv_timeout(Duration::from_millis(50)).await;
        assert!(matches!(result, Err(EventBusError::RecvTimeout(_))));
    }

    #[tokio::test]
    async fn test_publish_batch_critical_mpsc_bypass_preserved() {
        // 批量中含 Critical 安全事件(SkepticVeto)时,mpsc 旁路仍投递
        let bus = EventBus::new();
        let mut critical_rx = bus.subscribe_critical_events();
        let events = vec![
            make_indexed_event(0),
            NexusEvent::SkepticVeto {
                metadata: EventMetadata::new("parliament"),
                quest_id: "q-veto".into(),
                veto_reason: "恶意意图".into(),
                frozen_capabilities: vec!["cap-x".into()],
            },
            make_indexed_event(1),
        ];

        bus.publish_batch(events).await.unwrap();

        // Critical 旁路应收到 SkepticVeto(不被普通事件淹没)
        let received = tokio::time::timeout(Duration::from_millis(200), critical_rx.recv())
            .await
            .expect("不应超时")
            .expect("Critical 旁路应收到 SkepticVeto");
        assert_eq!(received.type_name(), "SkepticVeto");
    }

    #[tokio::test]
    async fn test_affinity_quota_exhausted_mpsc_delivery() {
        // MCA M0(ADR-065):AffinityQuotaExhausted 必须走 mpsc 旁路确保投递
        // (丢失导致降级链无人触发,请求持续打向死通道)
        let bus = EventBus::new();
        let mut critical_rx = bus.subscribe_critical_events();
        let event = NexusEvent::AffinityQuotaExhausted {
            metadata: EventMetadata::new("mca-gateway"),
            route_key: "deep_seek/deepseek-v4-flash".into(),
            reason: "429 quota exceeded for this month".into(),
        };

        bus.publish(event).await.unwrap();

        let received = tokio::time::timeout(Duration::from_millis(200), critical_rx.recv())
            .await
            .expect("不应超时")
            .expect("Critical 旁路应收到 AffinityQuotaExhausted");
        assert_eq!(received.type_name(), "AffinityQuotaExhausted");
        assert_eq!(received.severity(), crate::EventSeverity::Critical);
    }

    #[test]
    fn test_mpsc_required_events_hit_both_lists() {
        // 双清单同步守护(MCA M0 起):§6.2 红线要求的安全/资源类 Critical
        // 事件必须同时在 severity() 与 is_critical_mpsc_event() 两张清单中。
        // WHY 不断言全部 Critical 变体:CheckpointSaved/ConsensusReached 等
        // 历史 Critical 事件按既定设计只走 broadcast(背压保护级别),
        // 本测试只锁定"必须确保投递"的安全/资源事件子集不回退。
        let mpsc_required = [
            NexusEvent::SkepticVeto {
                metadata: EventMetadata::new("t"),
                quest_id: "q".into(),
                veto_reason: "r".into(),
                frozen_capabilities: vec![],
            },
            NexusEvent::RedTeamAudit {
                metadata: EventMetadata::new("t"),
                vulnerability_type: "prompt_injection".into(),
                failed_probes: 1,
                total_probes: 2,
                detection_rate: 0.5,
                remediation_suggestion: "s".into(),
            },
            NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new("t"),
                budget_type: "token".into(),
                current: 2,
                limit: 1,
            },
            NexusEvent::AgentTaskFailed {
                metadata: EventMetadata::new("t"),
                from: "agent-a".into(),
                to: "root".into(),
                task_id: "t-1".into(),
                error: "timeout".into(),
                retry_count: 0,
            },
            NexusEvent::AsaIntervention {
                metadata: EventMetadata::new("t"),
                operation_id: "op-1".into(),
                action: "Block".into(),
                safety_score: 0.1,
                block_reason: Some("unsafe".into()),
                alternative_suggestion: None,
            },
            NexusEvent::AffinityQuotaExhausted {
                metadata: EventMetadata::new("t"),
                route_key: "zhipu/glm-5.2".into(),
                reason: "quota".into(),
            },
            NexusEvent::R2FreezeViolation {
                metadata: EventMetadata::new("t"),
                violation_type: "CiDetection".into(),
                evidence: "ev".into(),
            },
            NexusEvent::R2FreezeRollbackFailed {
                metadata: EventMetadata::new("t"),
                reason: "git revert conflict".into(),
            },
            NexusEvent::FormalViolation {
                metadata: EventMetadata::new("t"),
                contract_id: "bc-1".into(),
                target_type: "event_bus::EventBus".into(),
                violations: vec!["v1".into()],
                context: nexus_contracts::behavior_contract::ContractContext::Runtime,
            },
            // Phase 10 Wave 5 双清单对齐(+4):否决覆盖审计/影子回滚失败/
            // 停止裁决/错误签名匹配(均 severity() Critical,必须确保投递)
            NexusEvent::VetoOverridden {
                metadata: EventMetadata::new("t"),
                quest_id: "q".into(),
                proposal_id: "p".into(),
                veto_reason: "r".into(),
                override_reason: "o".into(),
                override_by: "admin".into(),
            },
            NexusEvent::R1ShadowRollbackFailed {
                metadata: EventMetadata::new("t"),
                reason: "rollback conflict".into(),
                trigger_type: crate::types::RollbackTriggerType::Unknown,
                triggered_at: None,
                details: String::new(),
                diagnostic: crate::types::RollbackDiagnosticContext::default(),
            },
            NexusEvent::StopRulingIssued {
                metadata: EventMetadata::new("t"),
                quest_id: "q".into(),
                reason: "stagnation".into(),
                preserve_best: true,
            },
            NexusEvent::ErrorSignatureMatched {
                metadata: EventMetadata::new("t"),
                error_hash: "h".into(),
                matched_card_ids: vec![],
            },
            // L4 深度优化 P1-1(+1):验证器失败升入旁路(与 registry.rs 定级同步)
            NexusEvent::FormalVerificationFailed {
                metadata: EventMetadata::new("t"),
                property: "inv-1".into(),
                counterexample: "s".into(),
                generation: 1,
            },
        ];
        assert_eq!(
            mpsc_required.len(),
            14,
            "本安全/资源子集 14 个必投事件须齐全(权威全量 16 见 test_critical_double_list_d8_counts;\n             此为 MCA M0 子集不回退锁,非全量清单——第三轮 T1.3 注:旧误称'全量')"
        );
        for event in &mpsc_required {
            assert!(
                is_critical_mpsc_event(event),
                "{} 必须在 mpsc 旁路清单中(双清单同步红线)",
                event.type_name()
            );
            assert_eq!(
                event.severity(),
                crate::EventSeverity::Critical,
                "{} 的 severity() 必须为 Critical(双清单一致)",
                event.type_name()
            );
        }
    }

    /// D-8 双清单计数守护(P1-T11):CRITICAL_MPSC_VARIANTS、
    /// CRITICAL_TOTAL、mpsc ⊆ Critical、LANE_FORBIDDEN_SHARD 一一对应
    /// (常量真值见 :62/:69 两侧定义——注释不再抄数字,防 M44 同型漂移;2026-11-08 X2-N5 订正)
    /// (第三轮 T1.3 纠漂:旧注 14/18 为 v2.3 补入 CertifyIssued/FailureRegistered
    ///  前的滞后值;断言体以常量为准故恒绿,漂移仅存于此注 —— I8 盲区见上方常量注)
    #[test]
    fn test_critical_double_list_d8_counts() {
        // 口径断言:两清单规模与常量一致(常量是 D-8 裁决的编译期锚点)
        // WHY 引用 tests_helpers:变体构造已上移 pub(crate) 模块(D-8 口径唯一
        // 来源,shard.rs Lane 全量断言复用;本模块不再维护副本,防漂移)
        let mpsc_variants = tests_helpers::all_mpsc_critical_variants();
        let critical_variants = tests_helpers::all_severity_critical_variants();
        assert_eq!(
            mpsc_variants.len(),
            CRITICAL_MPSC_VARIANTS,
            "mpsc 旁路变体数必须等于 CRITICAL_MPSC_VARIANTS({CRITICAL_MPSC_VARIANTS})"
        );
        assert_eq!(
            critical_variants.len(),
            CRITICAL_TOTAL,
            "severity() Critical 变体数必须等于 CRITICAL_TOTAL({CRITICAL_TOTAL})"
        );

        // 18 个 mpsc 变体:is_critical_mpsc_event 必中 + severity() Critical
        for event in &mpsc_variants {
            assert!(
                is_critical_mpsc_event(event),
                "{} 必须在 mpsc 旁路清单中(双清单同步红线)",
                event.type_name()
            );
            assert_eq!(
                event.severity(),
                EventSeverity::Critical,
                "{} 的 severity() 必须为 Critical(双清单一致)",
                event.type_name()
            );
        }
        // 22 个 Critical 变体:severity() 全部为 Critical(21 口径)
        for event in &critical_variants {
            assert_eq!(
                event.severity(),
                EventSeverity::Critical,
                "{} 应命中 severity() Critical 清单(21 口径)",
                event.type_name()
            );
        }

        // 17 ⊆ 21:每个 mpsc 变体的名字必须在 severity() Critical 清单中
        let mpsc_names: Vec<&str> = mpsc_variants.iter().map(|e| e.type_name()).collect();
        let critical_names: Vec<&str> = critical_variants.iter().map(|e| e.type_name()).collect();
        for name in &mpsc_names {
            assert!(
                critical_names.contains(name),
                "mpsc 变体 {name} 不在 severity() Critical 清单中(17 ⊆ 21 违反)"
            );
        }

        // LANE_FORBIDDEN_SHARD 分片禁区声明:22 个名字与 Critical 清单一一对应
        // WHY 双向断言:T12 分片实现将按此清单禁止切片 Critical 单流;
        // 任一方向缺失都意味着声明与实际清单漂移
        assert_eq!(
            LANE_FORBIDDEN_SHARD.len(),
            CRITICAL_TOTAL,
            "LANE_FORBIDDEN_SHARD 必须恰好包含 {CRITICAL_TOTAL} 个名字"
        );
        for name in LANE_FORBIDDEN_SHARD {
            assert!(
                critical_names.contains(name),
                "LANE_FORBIDDEN_SHARD 含未知 Critical 变体名: {name}"
            );
        }
        for name in &critical_names {
            assert!(
                LANE_FORBIDDEN_SHARD.contains(name),
                "Critical 变体 {name} 未声明在 LANE_FORBIDDEN_SHARD(T12 分片禁区缺失)"
            );
        }
    }

    // ============================================================
    // R7-Critical 清单反向断言(bus.rs:1580/1674 手抄清单零反向断言治理)
    // ============================================================
    // WHY 替代"枚举反射"方案:本 crate 至少在 R7 时点 NexusEvent 无 all()/ALL/
    // iter() 全量遍历辅助,且 Rust 无反射,无法从 144 变体"枚举定义"可靠穷举
    // severity()-Critical 全集。故采用**三清单互锁 + 双向 is_critical_mpsc_event
    // 交叉断言**:把行为判定(severity() / is_critical_mpsc_event)与声明清单
    // (tests_helpers 双清单 + 常量注释记载的历史广播级)绑成一张闭环网,任何一环
    // 漂移都触发红灯。权威源:severity() 显式臂(registry.rs) + L44-49 注释
    // 记载的 4 个历史广播级 Critical。

    /// R7-正向反向断言:手抄双清单每一项必须与 severity() 权威判定一致,
    /// 且规模锚定(d8 常量)不回退。防"清单声称为 Critical 但 registry.rs
    /// 判为 Normal"(被通配 `_ => Normal` 吞掉的静默降级)。
    #[test]
    fn test_critical_lists_are_all_severity_critical() {
        let mpsc_variants = tests_helpers::all_mpsc_critical_variants();
        let severity_variants = tests_helpers::all_severity_critical_variants();

        // 现实值核验:16(mpsc 旁路)/ 20(severity Critical,含 4 个历史广播级)。
        // WHY 锚定常量而非硬编码 16/20:常量是 D-8 裁决的编译期锚点,清单规模
        // 一旦回退(误删)此测试即红。
        assert_eq!(
            mpsc_variants.len(),
            CRITICAL_MPSC_VARIANTS,
            "mpsc 旁路清单规模异常"
        );
        assert_eq!(
            severity_variants.len(),
            CRITICAL_TOTAL,
            "severity() Critical 清单规模异常"
        );

        // 正向反向:并集内每个声称为 Critical 的事件,severity() 必须真返回 Critical。
        // 一旦 registry.rs severity() 加显式臂时漏在此清单登记,或清单登记了
        // 某个实际被判 Normal 的事件,此断言红灯 —— 反向捕获两只清单漂移方向。
        for event in mpsc_variants.iter().chain(severity_variants.iter()) {
            assert_eq!(
                event.severity(),
                EventSeverity::Critical,
                "{} 声明为 Critical(手抄清单)但 severity() 返回 {:?} → 与 registry.rs 权威判定漂移",
                event.type_name(),
                event.severity()
            );
        }
    }

    /// R7-核心反向断言:severity-Critical 全集(清单)与 mpsc 旁路判定反查互锁。
    ///
    /// 覆盖任务约定的互锁断言 (c)/(d)+反向交叉:
    /// - (c) mpsc_set ⊆ severity_set:每个 mpsc 变体必须在 severity 清单中;
    /// - (d) severity_set − mpsc_set 恰为 4 个历史广播级 Critical
    ///   (CheckpointSaved/ConsensusReached/SlowConsumerDropped/OrphanCallDetected,
    ///   唯一权威源 = bus.rs L44-49 常量注释)——语义:除既定历史广播级 4 项外,
    ///   **任何 severity() Critical 变体都必须走 mpsc 旁路**。若未来新增 Critical
    ///   只看 severity 清单、漏接 mpsc,差值集合会出现非历史项 → 红。
    /// - 反向交叉 is_critical_mpsc_event:`mpsc 清单项必须判定 true`,历史广播级
    ///   4 项必须判定 false,锁定清单与判定函数不漂移(含 4 项通道归属不被误回退)。
    #[test]
    fn test_critical_severity_census_matches_lists() {
        use std::collections::{HashMap, HashSet};

        // 历史广播级 Critical(不走 mpsc 旁路)的唯一权威来源:bus.rs L44-49 常量注释。
        // 写死于此以独立于 tests_helpers,形成对"注释声明"的落点核对。
        let historical_broadcast_critical: [&str; 4] = [
            "CheckpointSaved",
            "ConsensusReached",
            "SlowConsumerDropped",
            "OrphanCallDetected",
        ];

        let mpsc_variants = tests_helpers::all_mpsc_critical_variants();
        let severity_variants = tests_helpers::all_severity_critical_variants();

        let mpsc_names: HashSet<&str> = mpsc_variants.iter().map(|e| e.type_name()).collect();
        let severity_by_name: HashMap<&str, &NexusEvent> = severity_variants
            .iter()
            .map(|e| (e.type_name(), e))
            .collect();

        // (c) mpsc_set ⊆ severity_set
        for name in &mpsc_names {
            assert!(
                severity_by_name.contains_key(name),
                "mpsc 变体 {name} 不在 severity() Critical 清单中(17 ⊆ 21 违反)"
            );
        }

        // (d) severity_set − mpsc_set 恰为 4 个历史广播级 Critical
        let broadcast_only: Vec<&str> = severity_by_name
            .keys()
            .copied()
            .filter(|n| !mpsc_names.contains(n))
            .collect();
        assert_eq!(
            broadcast_only.len(),
            historical_broadcast_critical.len(),
            "severity Critical − mpsc Critical 差值必须恰为 4 个历史广播级事件,实际 {}: {broadcast_only:?}",
            broadcast_only.len()
        );
        for name in &broadcast_only {
            assert!(
                historical_broadcast_critical.contains(name),
                "新增 Critical 变体 {name} 仅登记在 severity 清单(漏接 mpsc 旁路)或通道归属异常;历史广播级 4 项为 {historical_broadcast_critical:?}"
            );
        }

        // 反向交叉 is_critical_mpsc_event:
        // (i) mpsc 清单每一项判定必须 true(清单与判定函数漂移 → 红);
        // (ii) 历史广播级 4 项判定必须 false(4 项通道归属不得被误回退 → 红)。
        for event in &mpsc_variants {
            assert!(
                is_critical_mpsc_event(event),
                "{} 在 mpsc 清单但 is_critical_mpsc_event 判定 false → 清单与判定函数漂移",
                event.type_name()
            );
        }
        for name in historical_broadcast_critical {
            let event = severity_by_name
                .get(name)
                .expect("历史广播级 Critical 应存在于 severity() 清单");
            assert!(
                !is_critical_mpsc_event(event),
                "{name} 为历史广播级 Critical(只走 broadcast)但 is_critical_mpsc_event 判定 true → 通道归属被误回退"
            );
        }
    }

    #[test]
    fn test_msgpack_roundtrip() {
        let event = make_test_event();
        let bytes = serialize_msgpack(&event).unwrap();
        let decoded = deserialize_msgpack(&bytes).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn test_json_roundtrip() {
        let event = make_test_event();
        let s = serialize_json(&event).unwrap();
        let decoded = deserialize_json(&s).unwrap();
        assert_eq!(decoded, event);
    }

    #[tokio::test]
    async fn test_subscriber_count() {
        let bus = EventBus::new();
        assert_eq!(bus.subscriber_count(), 0);
        let _rx1 = bus.subscribe();
        let _rx2 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);
    }

    // ============================================================
    // P1-1: 事件主题过滤测试
    // ============================================================

    #[tokio::test]
    async fn test_recv_matching_filters_events() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        // 发布不同类型的事件
        let quest_event = make_test_event();
        let progress_event = NexusEvent::QuestProgressUpdated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-001".into(),
            completed: 1,
            total: 3,
        };

        bus.publish(quest_event.clone()).await.unwrap();
        bus.publish(progress_event.clone()).await.unwrap();

        // recv_matching 只接收 QuestProgressUpdated
        let received = rx
            .recv_matching(|e| matches!(e, NexusEvent::QuestProgressUpdated { .. }))
            .await
            .unwrap();
        assert_eq!(received, progress_event);
        // QuestCreated 事件被消费但未返回
    }

    #[tokio::test]
    async fn test_recv_matching_skips_non_matching() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        // 发布 3 个事件,只有最后 1 个匹配
        for i in 0..2 {
            bus.publish(NexusEvent::QuestProgressUpdated {
                metadata: EventMetadata::new("quest-engine"),
                quest_id: format!("q-{i}"),
                completed: i as u32,
                total: 3,
            })
            .await
            .unwrap();
        }
        let target = make_test_event(); // QuestCreated
        bus.publish(target.clone()).await.unwrap();

        // 只匹配 QuestCreated — 前两个 ProgressUpdated 被跳过
        let received = rx
            .recv_matching(|e| matches!(e, NexusEvent::QuestCreated { .. }))
            .await
            .unwrap();
        assert_eq!(received, target);
    }

    #[tokio::test]
    async fn test_recv_matching_by_quest_id() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        bus.publish(NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-other".into(),
            title: "其他任务".into(),
            task_count: 1,
        })
        .await
        .unwrap();

        let target = NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-target".into(),
            title: "目标任务".into(),
            task_count: 5,
        };
        bus.publish(target.clone()).await.unwrap();

        // 按 quest_id 过滤
        let received = rx
            .recv_matching(|e| {
                matches!(e, NexusEvent::QuestCreated { quest_id, .. } if quest_id == "q-target")
            })
            .await
            .unwrap();
        assert_eq!(received, target);
    }

    #[test]
    fn test_try_recv_matching_finds_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        // 同步发布多个事件
        bus.publish_blocking(NexusEvent::QuestProgressUpdated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-1".into(),
            completed: 1,
            total: 3,
        })
        .unwrap();
        let target = make_test_event();
        bus.publish_blocking(target.clone()).unwrap();

        // 只匹配 QuestCreated
        let result = rx
            .try_recv_matching(|e| matches!(e, NexusEvent::QuestCreated { .. }))
            .unwrap();
        assert_eq!(result, Some(target));
    }

    #[test]
    fn test_try_recv_matching_empty_buffer() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        // 缓冲区为空
        let result = rx.try_recv_matching(|_| true).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_try_recv_matching_no_match_in_buffer() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        // 只有 ProgressUpdated 事件
        bus.publish_blocking(NexusEvent::QuestProgressUpdated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-1".into(),
            completed: 1,
            total: 3,
        })
        .unwrap();

        // 寻找 QuestCreated — 不应找到
        let result = rx
            .try_recv_matching(|e| matches!(e, NexusEvent::QuestCreated { .. }))
            .unwrap();
        assert_eq!(result, None, "缓冲区中没有匹配事件");
    }

    // ============================================================
    // A3: 背压监控测试
    // ============================================================

    #[test]
    fn test_backpressure_monitor_initial_zero() {
        // 验证初始计数器为 0
        let bus = EventBus::new();
        let (lagged, warnings) = bus.backpressure_stats();
        assert_eq!(lagged, 0, "初始 lagged_count 应为 0");
        assert_eq!(warnings, 0, "初始 backpressure_warning_count 应为 0");
    }

    #[tokio::test]
    async fn test_backpressure_monitor_tracks_lagged() {
        // 使用极小容量(4)模拟 broadcast 满容量场景
        let bus = EventBus::with_capacity(4);
        let mut rx = bus.subscribe();

        // 发布超过容量的事件,使 receiver 的缓冲区溢出
        // broadcast::channel(4) 最多保留 4 条,发布 10 条后 receiver 会 Lagged
        for i in 0..10 {
            bus.publish(NexusEvent::QuestProgressUpdated {
                metadata: EventMetadata::new("test"),
                quest_id: format!("q-{i}"),
                completed: i as u32,
                total: 10,
            })
            .await
            .unwrap();
        }

        // 接收端应触发 Lagged(慢消费者丢弃)
        let result = rx.recv().await;
        assert!(
            matches!(result, Err(EventBusError::SlowConsumerDropped { .. })),
            "应触发 SlowConsumerDropped,实际: {result:?}"
        );

        // 验证 lagged_count 已递增(丢弃的事件数 > 0)
        let (lagged, _warnings) = bus.backpressure_stats();
        assert!(lagged > 0, "lagged_count 应递增,实际: {lagged}");
    }

    #[tokio::test]
    async fn test_backpressure_monitor_tracks_warnings() {
        // 使用小容量(8)触发背压告警(> 75% = 6)
        let bus = EventBus::with_capacity(8);
        let _rx = bus.subscribe(); // 保持订阅者,防止 send 返回 Err

        // 发布 8 个事件(第 8 次 publish 时,send 前 sender.len() == 7 > 6 = 8*3/4)
        // 注意:背压检查在 send() 之前,所以需要 queued > threshold 即 queued >= 7
        for i in 0..8 {
            bus.publish(NexusEvent::QuestProgressUpdated {
                metadata: EventMetadata::new("test"),
                quest_id: format!("q-{i}"),
                completed: i as u32,
                total: 8,
            })
            .await
            .unwrap();
        }

        let (_lagged, warnings) = bus.backpressure_stats();
        assert!(
            warnings > 0,
            "backpressure_warning_count 应递增,实际: {warnings}"
        );
    }

    // T-4(ADR-191「日志即契约」):补齐 §7 四可靠性站点中的背压 WARN 站点
    // (bus.rs:416-425)的日志断言。`test_backpressure_monitor_tracks_warnings`
    // 仅断言 counter(背压告警次数),此处断言“缓冲区 >75% 必发 WARN”本身是
    // 运维契约——日志文案漂移即回归。行为触发条件与上测完全一致(容量 8,订
    // 阅者不消费,第 8 次 publish 前 sender.len()==7 > 6=8*3/4)。
    #[tracing_test::traced_test]
    #[tokio::test]
    async fn test_backpressure_warn_emits_log_contract() {
        let bus = EventBus::with_capacity(8);
        let _rx = bus.subscribe(); // 保持订阅者,防止 send 返回 Err
        for i in 0..8 {
            bus.publish(NexusEvent::QuestProgressUpdated {
                metadata: EventMetadata::new("test"),
                quest_id: format!("q-{i}"),
                completed: i as u32,
                total: 8,
            })
            .await
            .unwrap();
        }
        assert!(
            logs_contain("broadcast 通道背压告警"),
            "缓冲区占用 >75% 时应记录背压 WARN 日志(T-4 日志契约)"
        );
    }

    // ============================================================
    // P1-T12: ShardedBus 分片灰度 —— EventBus 级集成测试
    // ============================================================
    // 断言口径:漏发率 = 0(sharded_total == merged_total,影子双跑前哨硬门禁)、
    // 17 Critical 永不进分片、OrderSensitive 单流保序、灰度默认关闭零回归。

    /// 带超时批量接收(测试辅助:漏发率 0 的前提是能收满事件)
    async fn drain_events(
        rx: &mut EventReceiver,
        expect: usize,
        timeout_ms: u64,
    ) -> Vec<NexusEvent> {
        let mut received = Vec::with_capacity(expect);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        while received.len() < expect {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
                Ok(Ok(event)) => received.push(event),
                _ => break,
            }
        }
        received
    }

    /// 轮询等待前哨统计收敛(sharded == merged;T13 漏发率=0 口径的测试版)
    async fn wait_merged_equals_sharded(bus: &EventBus, timeout_ms: u64) -> ShadowStats {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let stats = bus.shadow_stats();
            if stats.sharded_total > 0 && stats.merged_total >= stats.sharded_total {
                return stats;
            }
            if tokio::time::Instant::now() >= deadline {
                return stats;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// 第三轮 T1.2 结构判据：`event_lane` 在每条发布路径**只求值一次**。
    ///
    /// WHY 结构断言而非行为断言：`event_lane` 是纯函数，双份求值与单份求值
    /// **行为完全等价**（`recv==sent` 抓不到），回归只会表现为热路径多做的
    /// OrderSensitive `SessionKey` String clone（F-P2）——故只能以源码形态锁定。
    /// 不变式：全文件禁止在 `matches!` 里内联求值 `event_lane(&event)`（计数 0）。
    ///
    /// R4.2 收敛后记数调整（2 → 1）：单发宏与批量循环已共用同一 `@body` 宏体
    /// （publish.rs `dispatch_one_impl!`），lane 提升点唯一 —— 收敛前的「双份」
    /// 正是手写并行副本的漂移面；若未来新增**独立**发布路径（不经该宏体），
    /// 须把计数改回并按路径显性复核。
    #[test]
    fn dispatch_paths_hoist_lane_exactly_once() {
        // 只扫描生产区（`mod tests {` 之前）——否则 include_str! 会把本测试自己的
        // 字面量（包含待匹配串）也数进去，造成自引用假阳性。
        //
        // WHY 扫「整个 bus 模块的全部子文件」而非单一文件：W6(ADR-314/BC-01)把发布
        // 路径拆到 `bus/publish.rs`（单发宏 + 批量循环），两条路径现在同处该文件；
        // 若未来新增发布路径落在别的子模块，单文件扫描会漏检。逐文件扫描 + 拼接后
        // 统一计数，不变式的覆盖面与拆分前等价（拆分前全在 bus.rs 一个文件里）。
        const BUS_MODULE_FILES: [&str; 5] = [
            "mod.rs",
            "codec.rs",
            "publish.rs",
            "subscribe.rs",
            "shard_ctl.rs",
        ];
        let sources = [
            include_str!("mod.rs"),
            include_str!("codec.rs"),
            include_str!("publish.rs"),
            include_str!("subscribe.rs"),
            include_str!("shard_ctl.rs"),
        ];
        debug_assert_eq!(sources.len(), BUS_MODULE_FILES.len());
        let prod = sources
            .iter()
            .map(|src| src.split("mod tests {").next().unwrap_or(src))
            .collect::<Vec<_>>()
            .join("\n");
        let inline_eval = prod.matches("matches!(event_lane(&event)").count();
        let hoisted = prod.matches("let lane = event_lane(&event);").count();
        assert_eq!(
            inline_eval, 0,
            "禁止在 matches! 内联求值 event_lane（双份求值 = F-P2 热路径 clone 回归）"
        );
        assert_eq!(
            hoisted, 1,
            "R4.2 收敛后全部发布路径共用唯一 @body 宏体，lane 应恰好提升一次；实测 {hoisted}（新增独立发布路径请显式复核本计数）"
        );
    }

    #[test]
    fn test_sharding_default_off_zero_regression() {
        // 灰度默认关:EventBus::new() 行为与 v2.27.1 完全一致(零回归)
        let bus = EventBus::new();
        assert!(!bus.sharding_enabled(), "灰度默认必须关闭分片");
        let stats = bus.shadow_stats();
        assert_eq!(stats.published_total, 0);
        assert_eq!(stats.sharded_total, 0);
        assert_eq!(stats.merged_total, 0);
        assert_eq!(stats.shed_total, 0);
        assert_eq!(stats.critical_total, 0);
        // 无 runtime 上下文:enable_sharding 返回 Err 而非 panic(调用方 let _ 忽略即降级)
        assert!(matches!(
            bus.enable_sharding(64),
            Err(EventBusError::ShardingRequiresRuntime)
        ));
        assert!(!bus.sharding_enabled(), "降级后分片仍必须保持关闭");
    }

    #[tokio::test]
    async fn test_enable_sharding_idempotent() {
        let bus = EventBus::new();
        bus.enable_sharding(64).unwrap();
        assert!(bus.sharding_enabled());
        // 重复调用幂等拒绝(不重复 spawn worker,防泄漏)
        assert!(matches!(
            bus.enable_sharding(64),
            Err(EventBusError::ShardingAlreadyEnabled)
        ));
        assert!(matches!(
            bus.enable_sharding(128),
            Err(EventBusError::ShardingAlreadyEnabled)
        ));
        assert!(bus.sharding_enabled(), "幂等拒绝后分片状态不变");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_enable_sharding_concurrent_no_duplicate_spawn() {
        // 评审 Issue 1 回归守卫:并发双 enable_sharding 不重复 spawn worker
        // - 并发 8 个调用(barrier 同时进入):恰好 1 个 Ok,其余 Err(ShardingAlreadyEnabled);
        // - 失败路径不得 spawn worker:以 credit_flow 强引用计数观测 —— 每个 worker
        //   持有 Arc<CreditFlow> 一份,成功组 64 个 + 总线自身 1 份 = 65;
        //   修复前(锁外 spawn)失败者也 spawn 64 个 worker → 引用数 > 65,断言失败
        //   (64 个 worker 持有未注册总线永久空转,评审发现的资源泄漏)。
        let bus = EventBus::new();
        const CALLS: usize = 8;
        let barrier = Arc::new(tokio::sync::Barrier::new(CALLS));
        let handles: Vec<_> = (0..CALLS)
            .map(|_| {
                let b = bus.clone();
                let barrier = Arc::clone(&barrier);
                tokio::spawn(async move {
                    barrier.wait().await; // 全部就绪后同时进入,最大化并发交错
                    b.enable_sharding(DEFAULT_SHARD_COUNT)
                })
            })
            .collect();
        let mut ok = 0;
        for h in handles {
            match h.await.expect("并发 task panic") {
                Ok(()) => ok += 1,
                Err(EventBusError::ShardingAlreadyEnabled) => {}
                Err(e) => panic!("并发启用分片只应出现幂等拒绝: {e:?}"),
            }
        }
        assert_eq!(ok, 1, "并发启用分片必须恰好一个成功");
        assert!(bus.sharding_enabled(), "启用后分片标志必须为 true");
        // worker 不翻倍:credit_flow 强引用 = 总线 1 + 每片 worker 1(65)
        // (断言时 8 个 task 已结束、其 EventBus clone 已 drop;worker 任务仍挂载
        //  在测试 runtime 上持有引用 —— 修复前失败路径的 worker 亦在,计数 > 65)
        assert_eq!(
            Arc::strong_count(&bus.credit_flow),
            1 + DEFAULT_SHARD_COUNT,
            "并发失败路径不得 spawn worker(引用计数翻倍 = 重复 spawn 泄漏)"
        );
        // 安装的分片总线:worker 恰好一组(评审 Issue 1 的守护本意 = 不重复 spawn)。
        //
        // 性能重构 D2 断言语义调整(2026-10-22):worker 现改持 `Weak<ShardedEventBus>`
        // (根治旧设计"worker 持强引用且无退出条件"的内存泄漏 —— 即 composition_root_bench
        // OOM 根因)。故分片总线的**强引用计数不再随 worker 数增长**:此刻仅 `load_full()`
        // 返回的临时 guard(+ arc-swap 内部至多 1)持有强引用,worker 持 0 份。"不重复
        // spawn" 的精确守护已由上方 `credit_flow == 1 + DEFAULT_SHARD_COUNT` 承担(每个
        // spawn 出的 worker 恰持一份 `Arc<CreditFlow>`,失败路径不 spawn → 计数不翻倍)。
        // 本断言相应反转:强引用计数必须**远小于** worker 数(旧断言是 `> worker 数`),
        // 若未来 worker 重新持强引用(回退泄漏设计)即判红。
        let guard = bus.shard_bus.load_full();
        let sb = guard.as_ref().expect("分片总线必须已安装");
        let sb_count = Arc::strong_count(sb);
        assert!(
            sb_count < DEFAULT_SHARD_COUNT,
            "worker 应只持 Weak 引用,分片总线强计数不得随 worker 数增长(实测 {sb_count})"
        );
    }

    #[tokio::test]
    async fn test_sharded_all_delivered_no_loss() {
        // 汇入完整性:100 个 Unordered 事件(< 256 片容量,无 shed)全部送达
        let bus = EventBus::new();
        bus.enable_sharding(64).unwrap();
        let mut rx = bus.subscribe();
        const N: usize = 100;
        for _ in 0..N {
            bus.publish(make_test_event()).await.unwrap();
        }
        let received = drain_events(&mut rx, N, 5000).await;
        assert_eq!(received.len(), N, "漏发率必须为 0:订阅者应收到全部事件");
        let stats = wait_merged_equals_sharded(&bus, 2000).await;
        assert_eq!(stats.sharded_total, N as u64, "容量内事件应全部入分片");
        assert_eq!(
            stats.merged_total, stats.sharded_total,
            "入片数 == 汇入数(影子双跑漏发率 = 0)"
        );
        assert_eq!(stats.shed_total, 0, "容量内发布不应触发 shed");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn without_sharding_blocks_opt_in_and_propagates_to_clones() {
        // 性能重构 D1：without_sharding 后 leaf 的 try_enable_sharding 应早退
        // （即使有 runtime，否则 Handle::try_current 会成功 → 真启用）。对照总线可启用。
        let bus = EventBus::new().without_sharding();
        let clone = bus.clone();
        assert!(
            !bus.try_enable_sharding("test-blocked"),
            "without_sharding 应屏蔽 leaf opt-in"
        );
        assert!(!bus.sharding_enabled());
        assert!(
            !clone.sharding_enabled(),
            "开关应跨 Clone 共享（同一 Arc<AtomicBool>）"
        );
        // 对照：未关闭的总线在同 runtime 下 try_enable 应成功
        let open = EventBus::new();
        assert!(open.try_enable_sharding("test-open"), "默认允许 opt-in");
        assert!(open.sharding_enabled());
        open.shutdown_sharding().await; // 清理：避免 worker 泄漏到测试 runtime
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shard_workers_terminable_after_bus_drop() {
        // 性能重构 D2（漏发内存泄漏回归守卫）：worker 仅持 Weak<ShardedEventBus>，
        // 故最后一个 EventBus clone drop 后，分片总线必须被回收（旧设计 worker 持强
        // 引用 → 永不回收 → composition_root_bench OOM）。用 Weak 直接观测，不依赖
        // 全局计数，对并行测试鲁棒。
        let weak_sb;
        {
            let bus = EventBus::new();
            bus.enable_sharding(8).unwrap();
            weak_sb = Arc::downgrade(bus.shard_bus.load_full().as_ref().expect("已安装分片总线"));
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            assert!(weak_sb.upgrade().is_some(), "投递期总线必须存活");
        } // bus drop → 最后一个强引用释放（worker 持 Weak，不阻止回收）
        for _ in 0..100 {
            if weak_sb.upgrade().is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(
            weak_sb.upgrade().is_none(),
            "EventBus drop 后 ShardedEventBus 必须被回收（worker 应仅持 Weak）"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_sharding_drains_queued_events_no_loss() {
        // 性能重构 D2 优雅关闭：shutdown_sharding 应排空已入片但未汇入的事件，
        // 漏发率 = 0，且订阅者能收到全部事件。
        // 注：shutdown 会 store(None) 释放分片总线，故计数器必须在 shutdown **前**读 sharded_total，
        // 汇入完整性在 shutdown **后**经订阅者侧验证（而非读已释放的计数器）。
        let bus = EventBus::new();
        bus.enable_sharding(4).unwrap();
        let mut rx = bus.subscribe();
        const N: usize = 50;
        for _ in 0..N {
            bus.publish(make_test_event()).await.unwrap();
        }
        // 入片计数在 publish 路径同步递增（try_push 内 fetch_add）
        assert_eq!(
            bus.shadow_stats().sharded_total,
            N as u64,
            "容量内事件应全部入片"
        );
        // 优雅排空：唤醒 worker 将其片 drain-to-empty 汇入 broadcast 后退出
        bus.shutdown_sharding().await;
        let received = drain_events(&mut rx, N, 1000).await;
        assert_eq!(
            received.len(),
            N,
            "shutdown 后订阅者应收到全部已入片事件（漏发率 = 0）"
        );
    }

    /// P1-1 并发排空：`shutdown_sharding()` 与持续 `publish(Unordered)` 重叠时，
    /// **不得**出现「入片成功但永不汇入」的静默丢事件。
    ///
    /// 判定口径选「订阅者实收数 == 发布成功数」而非计数器：
    /// shutdown 后 `shadow_stats()` 随 `store(None)` 归零，计数器不可读；
    /// 而丢事件必然表现为订阅者少收（未汇入 broadcast）。
    ///
    /// 改前失效链：worker 见 `draining && is_empty()` 即 break，而 `try_push`
    /// 无 draining 判定、槽位仍在（`store(None)` 在 await 之后）—— break 后入片的
    /// 事件返回 Ok 却永不汇入。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_publish_and_shutdown_loses_nothing() {
        const ROUNDS: usize = 10;
        const PRODUCERS: usize = 8;
        const N: usize = 400;

        for round in 0..ROUNDS {
            // 容量远大于 N：避免慢消费者 Lagged 造成假丢
            let bus = EventBus::with_capacity(16_384);
            bus.enable_sharding(8).expect("启用分片应成功");
            let mut rx = bus.subscribe();

            let mut handles = Vec::new();
            for _ in 0..PRODUCERS {
                let b = bus.clone();
                handles.push(tokio::spawn(async move {
                    for _ in 0..(N / PRODUCERS) {
                        b.publish(make_test_event()).await.expect("publish 应成功");
                    }
                }));
            }

            // 让部分发布先入片，再与排空并发（关键竞态窗口）
            tokio::task::yield_now().await;
            bus.shutdown_sharding().await;

            for h in handles {
                h.await.expect("producer 不应 panic");
            }

            let received = drain_events(&mut rx, N, 3000).await;
            assert_eq!(
                received.len(),
                N,
                "round {round}: shutdown 与发布并发时不得丢事件（实收 {} / {N}）",
                received.len()
            );
        }
    }

    /// P2-5：`CHIMERA_BUS_SHARDS` 严格解析——非法值等同未设置，
    /// 不得绕过 `without_sharding()` 宿主角色的 Off 策略。
    #[test]
    fn env_shard_override_parsing_is_strict() {
        assert_eq!(EventBus::resolve_shard_override(Some("1")), Some(1));
        assert_eq!(EventBus::resolve_shard_override(Some("8")), Some(8));
        assert_eq!(EventBus::resolve_shard_override(Some("256")), Some(256));
        assert_eq!(EventBus::resolve_shard_override(Some("0")), None);
        assert_eq!(EventBus::resolve_shard_override(Some("257")), None);
        assert_eq!(EventBus::resolve_shard_override(Some("abc")), None);
        assert_eq!(EventBus::resolve_shard_override(Some("")), None);
        assert_eq!(EventBus::resolve_shard_override(None), None);
    }

    /// P2-4：`shutdown_sharding` 后 `shard_bus=None` 但 guard 已 set（无法重启），
    /// `try_enable_sharding` 必须诚实返回 false，不得谎报「分片可用」。
    #[tokio::test]
    async fn try_enable_after_shutdown_reports_false() {
        let bus = EventBus::new();
        assert!(bus.try_enable_sharding("test"), "首次启用应成功");
        bus.shutdown_sharding().await;
        assert!(!bus.sharding_enabled(), "shutdown 后分片应为关闭态");
        assert!(
            !bus.try_enable_sharding("test"),
            "shutdown 后不得谎报分片可用（guard 已 set，无法重启用）"
        );
    }

    #[tokio::test]
    async fn test_zero_subscriber_merged_total_counts_entry_not_delivery() {
        // 第三轮 T1.4/F-A2 显式锁测:零订阅者时 worker `send` 恒返 Err(Closed),
        // 但 `merged_total` 仍 == `sharded_total`——该等式的口径是「入汇入通道」
        // 而非「送达」;「入通道即丢弃」全部归因到 T1.1 新增的 `merge_closed_total`。
        // 本测同锁两件事:① merged==sharded 等式不因零订阅而破;② 零订阅下
        // merge_closed_total==merged_total(逐条都丢)。
        let bus = EventBus::new();
        bus.enable_sharding(64).unwrap();
        // 故意不 subscribe:broadcast 无订阅者 → worker send 恒 Err(Closed)
        const N: usize = 50;
        for _ in 0..N {
            bus.publish(make_test_event()).await.unwrap();
        }
        let stats = wait_merged_equals_sharded(&bus, 2000).await;
        assert_eq!(stats.sharded_total, N as u64, "零订阅不影响入分片");
        assert_eq!(
            stats.merged_total, stats.sharded_total,
            "零订阅下 merged==sharded 等式仍成立(证「入通道」非「送达」)"
        );
        assert_eq!(
            stats.merge_closed_total, stats.merged_total,
            "T1.1:零订阅者时全部汇入丢弃,merge_closed_total==merged_total"
        );
    }

    #[tokio::test]
    async fn test_order_sensitive_single_stream_preserves_order() {
        // 顺序敏感:correlation_id 为广义会话键 → OrderSensitive 车道,
        // 单流保序、不分片(E8-4:顺序敏感通道保持单流)
        let bus = EventBus::new();
        bus.enable_sharding(64).unwrap();
        let mut rx = bus.subscribe();
        for i in 0..5 {
            bus.publish(NexusEvent::QuestCreated {
                metadata: EventMetadata::with_correlation("test", format!("corr-{i}")),
                quest_id: format!("q-{i}"),
                title: format!("任务 {i}"),
                task_count: 1,
            })
            .await
            .unwrap();
        }
        let received = drain_events(&mut rx, 5, 5000).await;
        assert_eq!(received.len(), 5);
        for (i, ev) in received.iter().enumerate() {
            let NexusEvent::QuestCreated { quest_id, .. } = ev else {
                panic!("应收到 QuestCreated");
            };
            assert_eq!(
                quest_id,
                &format!("q-{i}"),
                "OrderSensitive 事件必须保持发布顺序"
            );
        }
        assert_eq!(
            bus.shadow_stats().sharded_total,
            0,
            "OrderSensitive 通道不分片(sharded_total 恒 0)"
        );
    }

    #[tokio::test]
    async fn test_shard_chaos_shed_and_critical_survive() {
        // 混沌:分片满 + 信用耗尽 → shed 计数增长;Critical 事件仍全送达
        // (current_thread runtime 下测试 task 无 yield,worker 无机会消费,
        //  同 type 事件确定性地填满同片 + 耗尽 256 信用池 → 确定性 shed)
        let bus = EventBus::new();
        bus.enable_sharding(64).unwrap();
        let mut rx = bus.subscribe();
        let mut critical_rx = bus.subscribe_critical_events();
        const FLOOD: usize = 400; // 256 入片 + 144 回退(< 1024 broadcast 容量,无 Lagged)
        for _ in 0..FLOOD {
            bus.publish(make_test_event()).await.unwrap();
        }
        // Critical 混发(3 个):永不进分片、永不丢弃
        let critical_events = tests_helpers::all_mpsc_critical_variants();
        for ev in critical_events.iter().take(3) {
            bus.publish(ev.clone()).await.unwrap();
        }
        // Critical 必须经 mpsc 旁路即时送达(分片/信用/背压均不影响)
        for _ in 0..3 {
            let critical = critical_rx.recv().await.unwrap();
            assert_eq!(
                critical.severity(),
                EventSeverity::Critical,
                "Critical 事件必须全送达(红线)"
            );
        }
        // shed 计数增长(片满 + 信用耗尽)
        assert!(
            bus.shadow_stats().shed_total > 0,
            "分片满 + 信用耗尽必须触发 shed 计数"
        );
        // 漏发率 0:订阅者收到全部(256 入片由 worker 汇入 + 144 回退直接投递)
        let received = drain_events(&mut rx, FLOOD + 3, 10000).await;
        assert_eq!(received.len(), FLOOD + 3, "混沌下漏发率必须为 0");
        let stats = wait_merged_equals_sharded(&bus, 3000).await;
        assert_eq!(stats.sharded_total, 256, "片容量 256:确定性入片数");
        assert_eq!(
            stats.merged_total, stats.sharded_total,
            "入片数 == 汇入数(漏发率 = 0)"
        );
        assert_eq!(stats.critical_total, 3, "Critical 车道前哨计数");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_sharded_concurrent_publish_no_loss() {
        // 不用 loom 的理由是**取舍**而非能力限制（原注「Windows GNU 下 loom 编译失败」
        // 已被 R2 C-102 实测推翻：loom 0.7.2 在本工具链编过且模型检验有牙，
        // 证据见 `tmp/loom_probe` 与 ADR-226 E1）。
        // 本测试锁吞吐/守恒：8 线程 × 100 次并发 publish_blocking，
        // 验证 Mutex/Atomic/ArrayQueue/CAS 竞争安全 + 漏发率 0。
        let bus = EventBus::new();
        bus.enable_sharding(64).unwrap();
        let mut rx = bus.subscribe();
        const THREADS: usize = 8;
        const PER_THREAD: usize = 100;
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let b = bus.clone();
                std::thread::spawn(move || {
                    for _ in 0..PER_THREAD {
                        b.publish_blocking(make_test_event()).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let received = drain_events(&mut rx, THREADS * PER_THREAD, 15000).await;
        assert_eq!(
            received.len(),
            THREADS * PER_THREAD,
            "多线程并发下漏发率必须为 0"
        );
        let stats = wait_merged_equals_sharded(&bus, 5000).await;
        assert_eq!(
            stats.merged_total, stats.sharded_total,
            "统计一致性:入片数 == 汇入数"
        );
    }

    // ============================================================
    // T16 EventSink 契约不变式(手册 §11.2)+ T17 Critical-13 顺序红线
    // ============================================================

    /// T16 EventSink 契约不变式(§11.2):shed 事件总数 = credit_shed_total 指标 —
    /// 交付面(EventBus.credit_stats)与观测面(shadow_stats.shed_total)必须同源一致,
    /// 防「shed 无指标 / 双处计数漂移」。
    #[test]
    fn event_sink_invariant_shed_metric_consistent() {
        // 基线:分片未启用 → 无 shed,两观测面一致为 0
        let bus = EventBus::new();
        assert_eq!(bus.credit_stats().shed_total, 0);
        assert_eq!(bus.shadow_stats().shed_total, 0);
        // 一致性不变量:任一时刻两观测面对 shed 计数的读取必然相等(同源 AtomicU64)
        assert_eq!(
            bus.credit_stats().shed_total,
            bus.shadow_stats().shed_total,
            "shed 计数两观测面必须同源一致(§11.2 EventSink 不变式)"
        );
    }

    /// T17(a) 顺序红线守护:is_critical_mpsc_event() 必须**恰好命中 17 个** mpsc 旁路
    /// 变体(规模由 `CRITICAL_MPSC_VARIANTS` 锁定,广播专属 Critical 计数由
    /// `CRITICAL_TOTAL` 锁定)。防「全 Critical 一律进旁路」旧口径回潮:
    /// 4 个广播专属 Critical(CheckpointSaved/ConsensusReached/SlowConsumerDropped/
    /// OrphanCallDetected)按既定设计只走 broadcast,误捕即回潮破裂。
    #[test]
    // 函数名 exact_13_not_17 为历史标签(T17 立测时的旧规模),现值以
    // CRITICAL_MPSC_VARIANTS=18 为准(C-64b 后;改名留 T3)
    fn mpsc_bypass_exact_13_not_17() {
        let mpsc = tests_helpers::all_mpsc_critical_variants();
        assert_eq!(mpsc.len(), CRITICAL_MPSC_VARIANTS, "18 名单规模锁定");
        for ev in &mpsc {
            assert!(
                is_critical_mpsc_event(ev),
                "{} 必须在 mpsc 旁路清单中",
                ev.type_name()
            );
        }

        // 4 个广播专属 Critical 不得被 mpsc 旁路误捕(17 ⊆ 21,差集恒为 4)
        let severity_critical = tests_helpers::all_severity_critical_variants();
        let mpsc_names: std::collections::HashSet<&str> =
            mpsc.iter().map(|e| e.type_name()).collect();
        let mut broadcast_only = 0;
        for ev in &severity_critical {
            if mpsc_names.contains(ev.type_name()) {
                continue;
            }
            assert!(
                !is_critical_mpsc_event(ev),
                "{} 属广播专属 Critical,不得被 mpsc 误捕(全 Critical 进旁路的旧口径回潮)",
                ev.type_name()
            );
            broadcast_only += 1;
        }
        assert_eq!(
            broadcast_only,
            CRITICAL_TOTAL - CRITICAL_MPSC_VARIANTS,
            "差集 21-17 = {CRITICAL_TOTAL}-{CRITICAL_MPSC_VARIANTS} = 4 个广播专属 Critical"
        );
    }

    /// T17(b) 分片红线:18 个 mpsc 旁路变体全部判定为 Critical 车道(永不进分片,
    /// 单流旁路投递)—— 分片(shard)仅服务非 Critical,与 event_lane/§11.2 对齐。
    #[test]
    fn mpsc_critical_variants_are_single_stream_never_sharded() {
        let mpsc = tests_helpers::all_mpsc_critical_variants();
        for ev in &mpsc {
            assert_eq!(
                event_lane(ev),
                Lane::Critical,
                "{} 必须为 Critical 车道(单流旁路,分片仅服务非 Critical)",
                ev.type_name()
            );
        }
    }
}
