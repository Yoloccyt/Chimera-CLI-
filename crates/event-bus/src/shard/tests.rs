use super::*;
use crate::bus::{CRITICAL_TOTAL, LANE_FORBIDDEN_SHARD};
use crate::types::EventMetadata;

/// 无会话键的普通事件(Unordered 代表)
fn unordered_event() -> NexusEvent {
    NexusEvent::QuestCreated {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: "q-1".into(),
        title: "测试".into(),
        task_count: 1,
    }
}

fn critical_event() -> NexusEvent {
    NexusEvent::CheckpointSaved {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: "q".into(),
        checkpoint_id: "c".into(),
        memory_snapshot_hash: "h".into(),
    }
}

// ============================================================
// Lane 判定:21 Critical → Critical / 会话键 → OrderSensitive / 其余 → Unordered
// ============================================================

#[test]
fn test_lane_forbidden_shard_list_covers_all_critical() {
    // 红线:LANE_FORBIDDEN_SHARD 清单与 severity() Critical 一一对应
    // (全量 22 个逐一构造断言)—— helper 复用 bus.rs 测试模块的
    // all_severity_critical_variants(pub(crate),D-8 口径)
    let critical_variants = super::super::bus::tests_helpers::all_severity_critical_variants();
    assert_eq!(critical_variants.len(), crate::bus::CRITICAL_TOTAL);
    for event in &critical_variants {
        assert_eq!(
            event_lane(event),
            Lane::Critical,
            "{} 必须判定为 Critical 车道(永不进分片)",
            event.type_name()
        );
    }
}

#[test]
fn test_lane_forbidden_shard_names_are_critical() {
    // LANE_FORBIDDEN_SHARD 清单自身:与 CRITICAL_TOTAL 常量锚定(防漂移),
    // 全部命中 Critical 车道
    assert_eq!(LANE_FORBIDDEN_SHARD.len(), CRITICAL_TOTAL);
    for name in LANE_FORBIDDEN_SHARD {
        let event = super::super::bus::tests_helpers::critical_variant_by_name(name)
            .expect("清单中的名字必须可构造事件");
        assert_eq!(
            event_lane(&event),
            Lane::Critical,
            "{name} 必须为 Critical 车道"
        );
    }
}

#[test]
fn test_lane_session_variants_are_order_sensitive() {
    // 白名单 5 个带 session_id 变体 → OrderSensitive(走单流保序)
    let events = [
        NexusEvent::TuiChatSubmitted {
            metadata: EventMetadata::new("t"),
            session_id: "s1".into(),
            query: "q".into(),
            slash_command: None,
        },
        NexusEvent::TuiChatResponseChunk {
            metadata: EventMetadata::new("t"),
            session_id: "s1".into(),
            delta: "d".into(),
            cursor_hint: 0,
        },
        NexusEvent::TuiChatCompleted {
            metadata: EventMetadata::new("t"),
            session_id: "s1".into(),
            tool_use: None,
        },
        NexusEvent::TuiChatStatusChanged {
            metadata: EventMetadata::new("t"),
            session_id: "s1".into(),
            status: crate::types::ChatStatus::Thinking,
        },
        NexusEvent::CrossVendorNegotiation {
            metadata: EventMetadata::new("t"),
            session_id: "s1".into(),
            quest_id: "q".into(),
            producer_provider: "zhipu/glm-5.2".into(),
            verifier_provider: "zhipu/glm-5.2".into(),
            skeptic_provider: "zhipu/glm-5.2".into(),
            cross_vendor_enforced: true,
            decorrelation_status: "enforced".into(),
        },
    ];
    for event in &events {
        assert!(
            matches!(event_lane(event), Lane::OrderSensitive(SessionKey::Session(s)) if s == "s1"),
            "{} 应判定为 OrderSensitive(Session)",
            event.type_name()
        );
    }
}

#[test]
fn test_lane_correlation_id_is_order_sensitive() {
    // correlation_id 作为广义会话键 → OrderSensitive
    let mut meta = EventMetadata::new("t");
    meta.correlation_id = Some("corr-1".into());
    let event = NexusEvent::QuestProgressUpdated {
        metadata: meta,
        quest_id: "q".into(),
        completed: 1,
        total: 2,
    };
    assert_eq!(
        event_lane(&event),
        Lane::OrderSensitive(SessionKey::Correlation("corr-1".into()))
    );
}

#[test]
fn test_lane_alert_event_is_order_sensitive() {
    // 告警类事件(EfficiencyAlertTriggered)→ OrderSensitive(Alert 占位键)
    // 不走 Unordered 分片:告警的即时性即顺序敏感语义(Ω₈ 可观测性,根 e2e
    // 协调闭环测试在 record_event 后立即 try_recv —— 直投单流保立即可读)
    let event = NexusEvent::EfficiencyAlertTriggered {
        metadata: EventMetadata::new("efficiency-monitor"),
        rule_id: "paradox-risk-coordination-ratio".into(),
        metric_name: "coordination_to_gain_ratio".into(),
        triggered_value: 2.0,
        threshold: 0.9,
    };
    assert_eq!(
        event_lane(&event),
        Lane::OrderSensitive(SessionKey::Alert("paradox-risk-coordination-ratio".into()))
    );
}

#[test]
fn test_lane_unordered_default() {
    // 无会话键、非 Critical → Unordered
    assert_eq!(event_lane(&unordered_event()), Lane::Unordered);
    // Critical 事件即使无会话键也优先 Critical(判定顺序:Critical > 会话键)
    assert_eq!(event_lane(&critical_event()), Lane::Critical);
}

#[test]
fn test_lane_priority_critical_over_correlation() {
    // 判定顺序守护:Critical 事件即使带 correlation_id 也必须是 Critical
    // (红线:Critical 永不进分片,correlation_id 不能把它拉进 OrderSensitive)
    let mut meta = EventMetadata::new("t");
    meta.correlation_id = Some("corr".into());
    let event = NexusEvent::BudgetExceeded {
        metadata: meta,
        budget_type: "token".into(),
        current: 2,
        limit: 1,
    };
    assert_eq!(event_lane(&event), Lane::Critical);
}

// ============================================================
// FNV-1a 确定性 + 分片一致性 + 无序事件分散
// ============================================================

#[test]
fn test_fnv1a_known_vector() {
    // FNV-1a 标准测试向量:空串 → 偏移基值;已知向量校验(防实现漂移)
    assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
    // 64 位 FNV-1a("foobar")= 0x85944171f73967e8(公开参考值)
    assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
}

#[test]
fn test_shard_index_deterministic_same_type() {
    // 同类型事件(不同 quest_id)恒入同片(FNV-1a 确定性)
    let bus = ShardedEventBus::new(DEFAULT_SHARD_COUNT, SHARD_CAPACITY);
    let e1 = NexusEvent::QuestCreated {
        metadata: EventMetadata::new("a"),
        quest_id: "q-1".into(),
        title: "t1".into(),
        task_count: 1,
    };
    let e2 = NexusEvent::QuestCreated {
        metadata: EventMetadata::new("b"),
        quest_id: "q-999".into(),
        title: "t2".into(),
        task_count: 2,
    };
    let idx1 = bus.shard_index(&e1);
    let idx2 = bus.shard_index(&e2);
    assert_eq!(idx1, idx2, "同类型事件应确定性同片");
}

#[test]
fn test_session_shard_deterministic() {
    // 会话键 FNV-1a 确定性:同键同片、不同键可不同片
    let s1a = ShardedEventBus::session_shard("session-abc", DEFAULT_SHARD_COUNT);
    let s1b = ShardedEventBus::session_shard("session-abc", DEFAULT_SHARD_COUNT);
    assert_eq!(s1a, s1b, "同会话键恒同片(FNV-1a 确定性)");
    let s2 = ShardedEventBus::session_shard("session-xyz", DEFAULT_SHARD_COUNT);
    assert!(
        s1a < DEFAULT_SHARD_COUNT && s2 < DEFAULT_SHARD_COUNT,
        "分片索引越界"
    );
}

#[test]
fn test_unordered_events_spread_across_shards() {
    // 无序事件分散(第三轮 T1.4:取齐 TEST_UNORDERED_TYPE_NAMES 真实规模 = 15,
    // 旧注误写"32";断言从"不塌缩到 1 片"弱判据升级为倾斜度上限)
    // 设计口径 kind() % 64 的等价验证:type_name FNV-1a 哈希分散。
    let bus = ShardedEventBus::new(DEFAULT_SHARD_COUNT, SHARD_CAPACITY);
    assert_eq!(
        TEST_UNORDERED_TYPE_NAMES.len(),
        15,
        "测试清单规模与注释(15)必须一致(T1.4 取齐,防再次漂移)"
    );
    let mut occupied = std::collections::HashSet::new();
    let mut per_shard: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for (i, name) in TEST_UNORDERED_TYPE_NAMES.iter().enumerate() {
        let event = make_event_by_type_name(name, i);
        let idx = bus.shard_index(&event);
        occupied.insert(idx);
        *per_shard.entry(idx).or_insert(0) += 1;
    }
    let max_skew = *per_shard.values().max().unwrap_or(&0);
    // 倾斜度上限:15 个类型名经 FNV-1a % 64 实测落 11 片、单片最多 3 个;
    // 判据留裕度(≥10 片、单片 ≤4)——既放过确定性哈希的正常分布,又能在
    // 键被改成"多数类型撞同片"的病态倾斜时判红(旧 >1 弱判据抓不到 2-3 片塌缩)。
    assert!(
        occupied.len() >= 10,
        "无序事件分片分布塌缩:仅占 {} 片(15 类型应分散到 ≥10 片)",
        occupied.len()
    );
    assert!(
        max_skew <= 4,
        "分片倾斜度过高:单片最多承载 {max_skew} 个类型(上限 4;热点片由单类型独占,非多类型堆积)"
    );
    // 深度/计数初始为 0(新总线无残留)
    assert_eq!(bus.sharded_total(), 0);
    assert_eq!(bus.merged_total(), 0);
}

/// 测试用 Unordered 事件类型名清单(15 个非 Critical、无会话键、非告警变体;
/// EfficiencyAlertTriggered 已移出 —— 告警类事件归 OrderSensitive 车道)
const TEST_UNORDERED_TYPE_NAMES: &[&str] = &[
    "QuestCreated",
    "QuestProgressUpdated",
    "QuestListUpdated",
    "QuestCompleted",
    "ThinkingModeSwitched",
    "VoteCast",
    "CapabilityFrozen",
    "ModelRouteSelected",
    "NexusStateChanged",
    "UserIntentEncoded",
    "CacheHit",
    "MemoryMetricsReported",
    "WikiUpdated",
    "MemConStrategyAdjusted",
    "BudgetMetricsUpdated",
];

/// 按类型名构造测试事件(仅覆盖 TEST_UNORDERED_TYPE_NAMES 中的变体;
/// 若变体字段签名变化,此 helper 需同步更新 —— 编译期强制)
fn make_event_by_type_name(name: &str, i: usize) -> NexusEvent {
    let meta = || EventMetadata::new("test");
    match name {
        "QuestCreated" => NexusEvent::QuestCreated {
            metadata: meta(),
            quest_id: format!("q-{i}"),
            title: "t".into(),
            task_count: 1,
        },
        "QuestProgressUpdated" => NexusEvent::QuestProgressUpdated {
            metadata: meta(),
            quest_id: format!("q-{i}"),
            completed: 1,
            total: 2,
        },
        "QuestListUpdated" => NexusEvent::QuestListUpdated {
            metadata: meta(),
            quests: vec![],
            source: "s".into(),
        },
        "QuestCompleted" => NexusEvent::QuestCompleted {
            metadata: meta(),
            quest_id: format!("q-{i}"),
            status: crate::types::QuestStatus::Completed,
        },
        "ThinkingModeSwitched" => NexusEvent::ThinkingModeSwitched {
            metadata: meta(),
            quest_id: format!("q-{i}"),
            from_mode: "fast".into(),
            to_mode: "deep".into(),
            reason: String::new(),
        },
        "VoteCast" => NexusEvent::VoteCast {
            metadata: meta(),
            proposal_id: format!("p-{i}"),
            voter: "v".into(),
            vote: true,
        },
        "CapabilityFrozen" => NexusEvent::CapabilityFrozen {
            metadata: meta(),
            capability_id: format!("c-{i}"),
            reason: "r".into(),
        },
        "ModelRouteSelected" => NexusEvent::ModelRouteSelected {
            metadata: meta(),
            quest_id: format!("q-{i}"),
            model_id: "m".into(),
            route_reason: "r".into(),
        },
        "NexusStateChanged" => NexusEvent::NexusStateChanged {
            metadata: meta(),
            state_hash: format!("h-{i}"),
            prev_hash: "p".into(),
        },
        "UserIntentEncoded" => NexusEvent::UserIntentEncoded {
            metadata: meta(),
            intent_id: format!("i-{i}"),
            raw_text: "r".into(),
            risk_level: 1,
        },
        "CacheHit" => NexusEvent::CacheHit {
            metadata: meta(),
            cache_key: format!("k-{i}"),
        },
        "MemoryMetricsReported" => NexusEvent::MemoryMetricsReported {
            metadata: meta(),
            hit_rate: 0.5,
            evictions: 0,
        },
        "WikiUpdated" => NexusEvent::WikiUpdated {
            metadata: meta(),
            wiki_hash: format!("w-{i}"),
            delta: 1,
        },
        "MemConStrategyAdjusted" => NexusEvent::MemConStrategyAdjusted {
            metadata: meta(),
            from_strategy: "StandardTopK".into(),
            to_strategy: "TimeFocused".into(),
            reason: "r".into(),
            ghost_rate: None,
        },
        "BudgetMetricsUpdated" => NexusEvent::BudgetMetricsUpdated {
            metadata: meta(),
            metrics: crate::types::BudgetMetricsPayload {
                total_consumption: 1.0,
                remaining_budget: 2.0,
                utilization_rate: 0.5,
                current_tier: "High".into(),
                coefficient: 1.0,
                is_exceeded: false,
                alert: None,
            },
        },
        _ => panic!("测试事件名未覆盖: {name}"),
    }
}

// ============================================================
// ShardedEventBus 入片/汇入基本语义(EventBus 集成测试在 bus.rs)
// ============================================================

#[test]
fn test_try_push_pop_roundtrip_and_depth() {
    let bus = ShardedEventBus::new(DEFAULT_SHARD_COUNT, SHARD_CAPACITY);
    let event = unordered_event();
    // 入片成功,深度 +1,sharded_total +1
    assert!(bus.try_push(event).is_ok());
    assert_eq!(bus.sharded_total(), 1);
    // 找到事件所在片,pop 后深度归零
    let idx = bus.shard_index(&unordered_event());
    assert_eq!(bus.shard_depth(idx), 1, "深度应反映队列中未汇入事件数");
    assert!(bus.shards[idx].pop().is_some(), "入片事件应可被 pop");
    // 模拟 worker 汇入:深度计数仅在 shard_worker 的 pop 路径递减
    // (pop + fetch_sub 是 worker 原子的两步,直接 pop 不更新 depth)
    bus.depths[idx].fetch_sub(1, Ordering::Relaxed);
    assert_eq!(bus.shard_depth(idx), 0, "汇入后深度应归零");
}

#[test]
fn test_try_push_full_returns_event() {
    // 片满:try_push 返回 Err(事件所有权交还调用方,不丢事件)
    let bus = ShardedEventBus::new(1, 2); // 单片容量 2
    let e1 = unordered_event();
    let e2 = NexusEvent::CacheHit {
        metadata: EventMetadata::new("t"),
        cache_key: "k2".into(),
    };
    let e3 = NexusEvent::CacheHit {
        metadata: EventMetadata::new("t"),
        cache_key: "k3".into(),
    };
    assert!(bus.try_push(e1).is_ok());
    assert!(bus.try_push(e2).is_ok());
    let rejected = bus.try_push(e3).expect_err("容量 2 满后应拒绝");
    assert_eq!(rejected.type_name(), "CacheHit", "拒绝时返回事件本体");
    assert_eq!(bus.sharded_total(), 2, "仅入片成功者计数");
}

#[test]
fn test_same_shard_concurrent_try_push_no_loss_no_dup() {
    // 第三轮 T1.4：同片多线程并发 try_push 不丢不重。这里**不引入 loom** 是取舍
    // （不是能力限制：loom 0.7.2 在本工具链可用且有牙，见 R2 C-102 实测与 ADR-226）：
    // 目标是把 crossbeam ArrayQueue 的 MPSC 语义 + 无锁计数守恒压在**真并发**下看结果，
    // 而不是枚举交错。
    // 单片(n_shards=1)强制所有事件争抢同一 ArrayQueue。
    use std::sync::Barrier;
    const THREADS: usize = 8;
    const PER_THREAD: usize = 24; // 8×24=192 ≤ SHARD_CAPACITY 256,零 shed
    let bus = Arc::new(ShardedEventBus::new(1, SHARD_CAPACITY));
    let barrier = Arc::new(Barrier::new(THREADS));
    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let bus = Arc::clone(&bus);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut ok = 0usize;
                barrier.wait(); // 对齐起跑,最大化并发交错
                for j in 0..PER_THREAD {
                    let ev = NexusEvent::CacheHit {
                        metadata: EventMetadata::new("t"),
                        cache_key: format!("k-{t}-{j}"),
                    };
                    if bus.try_push(ev).is_ok() {
                        ok += 1;
                    }
                }
                ok
            })
        })
        .collect();
    let pushed: usize = handles
        .into_iter()
        .map(|h| h.join().expect("worker thread panic"))
        .sum();
    assert_eq!(
        pushed,
        THREADS * PER_THREAD,
        "并发入片不得因竞争而丢(容量内应全成功)"
    );
    // 计数守恒:sharded_total == 成功入片数(每成功恰一次 +1,无重)
    assert_eq!(
        bus.sharded_total(),
        (THREADS * PER_THREAD) as u64,
        "sharded_total 必须恰等于成功入片数(不重不漏)"
    );
    // 队列实收守恒:排空单片,唯一键集大小 == 入片数(既不重复也不丢失)
    let mut seen = std::collections::HashSet::new();
    let mut drained = 0usize;
    while let Some(ev) = bus.shards[0].pop() {
        if let NexusEvent::CacheHit { cache_key, .. } = &ev {
            seen.insert(cache_key.clone());
        }
        drained += 1;
    }
    assert_eq!(drained, THREADS * PER_THREAD, "排空队列数 == 入片数");
    assert_eq!(seen.len(), drained, "队列内事件互不重复(不重)");
    // 排空后深度归零(入片 fetch_add / pop 不经 worker 时手动不维护,本测试未启 worker)
}

#[test]
fn test_zero_shards_falls_back_to_one() {
    // n_shards = 0 时回退 1 片(取模除零防 panic,灰度安全)
    let bus = ShardedEventBus::new(0, SHARD_CAPACITY);
    assert_eq!(bus.n_shards(), 1);
    assert!(bus.try_push(unordered_event()).is_ok());
}
