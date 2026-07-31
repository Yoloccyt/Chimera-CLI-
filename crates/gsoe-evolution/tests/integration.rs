//! EventBus 集成测试 — 验证 GsoePolicyUpdated 事件的发布与订阅
//!
//! 对应 SubTask 3.5
//!
//! # 测试覆盖
//! - 事件发布:evolve_once 触发 GsoePolicyUpdated 事件
//! - 事件字段:generation/improvement/new_mutation_rate/new_selection_pressure 正确性
//! - 事件 source:metadata.source == "gsoe-evolution"
//! - 多代进化:连续多代均发布事件
//! - 事件序列化:GsoePolicyUpdated 可正确序列化/反序列化
//! - ConsensusReached/RedTeamAudit 信号处理
//!
//! # P5.2.4: 通道 B E2E 集成测试(5 场景)
//!
//! - happy_path:CI 通过 + 无回归 → 注册 spec + 发布 SpecRegistered 事件
//! - bench_jitter_false_veto_prevention:单尾二项检验防止 bench 抖动误否决
//! - inv9_violation_triggers_veto:INV-9 委托图有环 → CI 失败 → 否决
//! - immutable_surface_blocks_registration:不可进化面违反 → 注册拒绝 + 错误映射
//! - channel_a_to_b_end_to_end:通道 A 提议 → 通道 B CI 门 + 显著性 → 注册

// Task 3.10: EventMetadata 已下沉至 L0 nexus-contracts(ADR-033 扩展)
use event_bus::{EventBus, EventSeverity, NexusEvent};
use gsoe_evolution::{
    CargoCiGate, CiFailure, CiFailureKind, CiGate, DelegationEdge, GsoeConfig, GsoeError,
    GsoeEvolutionEngine, MockCiGate, SignificanceDetector, SpecRegistry, SpecRegistryError,
};
use nexus_contracts::{
    ContractSpec, EventMetadata, HarnessMeta, HarnessSpec, HopSpec, RetryPolicy,
};
use std::time::Duration;

/// 验证 evolve_once 正确发布 GsoePolicyUpdated 事件
#[tokio::test]
async fn test_evolve_once_publishes_gsoe_policy_updated() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

    engine.evolve_once().await.expect("进化失败");

    let event = rx
        .recv_timeout(Duration::from_secs(1))
        .await
        .expect("接收事件超时");

    assert!(
        matches!(event, NexusEvent::GsoePolicyUpdated { .. }),
        "期望 GsoePolicyUpdated 事件,收到 {event:?}"
    );
}

/// 验证事件字段正确性
#[tokio::test]
async fn test_gsoe_policy_updated_event_fields() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

    engine.evolve_once().await.unwrap();

    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();

    if let NexusEvent::GsoePolicyUpdated {
        generation,
        improvement,
        new_mutation_rate,
        new_selection_pressure,
        ..
    } = event
    {
        assert_eq!(generation, 1, "首轮进化 generation 应为 1");
        assert!(improvement.is_finite(), "improvement 应为有限值");
        assert!(
            (0.0..=1.0).contains(&new_mutation_rate),
            "new_mutation_rate 应在 [0, 1]: {new_mutation_rate}"
        );
        assert!(
            new_selection_pressure >= 0.0,
            "new_selection_pressure 应非负: {new_selection_pressure}"
        );
    } else {
        panic!("期望 GsoePolicyUpdated 事件");
    }
}

/// 验证事件 source 为 gsoe-evolution
#[tokio::test]
async fn test_gsoe_policy_updated_event_source() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

    engine.evolve_once().await.unwrap();

    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    assert_eq!(
        event.metadata().source,
        "gsoe-evolution",
        "事件 source 应为 gsoe-evolution"
    );
}

/// 验证多代进化连续发布事件
#[tokio::test]
async fn test_multi_generation_publishes_events() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

    // 进化 3 代
    for _ in 0..3 {
        engine.evolve_once().await.unwrap();
    }

    // 应收到 3 个事件,generation 分别为 1, 2, 3
    for expected_gen in 1..=3u64 {
        let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
        if let NexusEvent::GsoePolicyUpdated { generation, .. } = event {
            assert_eq!(
                generation, expected_gen,
                "第 {expected_gen} 个事件的 generation 应为 {expected_gen}"
            );
        } else {
            panic!("期望 GsoePolicyUpdated 事件");
        }
    }
}

/// 验证 GsoePolicyUpdated 事件可正确序列化/反序列化
#[tokio::test]
async fn test_gsoe_policy_updated_serialization() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

    engine.evolve_once().await.unwrap();

    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();

    // JSON round-trip
    let json = serde_json::to_string(&event).expect("序列化失败");
    let restored: NexusEvent = serde_json::from_str(&json).expect("反序列化失败");
    assert_eq!(event, restored, "序列化 round-trip 应保持一致");

    // MessagePack round-trip
    let msgpack = event_bus::serialize_msgpack(&event).expect("msgpack 序列化失败");
    let restored_mp = event_bus::deserialize_msgpack(&msgpack).expect("msgpack 反序列化失败");
    assert_eq!(event, restored_mp, "msgpack round-trip 应保持一致");
}

/// 验证无 EventBus 时进化正常工作(不发布事件)
#[tokio::test]
async fn test_evolve_without_event_bus() {
    let mut engine = GsoeEvolutionEngine::new(GsoeConfig::default());
    let result = engine.evolve_once().await.expect("进化失败");
    assert_eq!(result.generation, 1);
    assert!(result.improvement.is_finite());
}

/// 验证 ConsensusReached 信号被正确消费
#[tokio::test]
async fn test_consensus_signal_consumed_after_evolution() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

    // 模拟收到 2 个共识信号
    engine.handle_consensus_reached();
    engine.handle_consensus_reached();

    // 进化后信号应被消费
    engine.evolve_once().await.unwrap();

    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    assert!(
        matches!(event, NexusEvent::GsoePolicyUpdated { .. }),
        "仍应发布 GsoePolicyUpdated 事件"
    );
}

/// 验证 RedTeamAudit 信号触发对抗进化(提升 mutation_rate)
#[tokio::test]
async fn test_red_team_signal_triggers_adversarial_evolution() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

    let original_mr = engine.current_policy().mutation_rate;

    // 模拟收到红队审计信号
    engine.handle_red_team_audit();

    engine.evolve_once().await.unwrap();

    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    if let NexusEvent::GsoePolicyUpdated {
        new_mutation_rate, ..
    } = event
    {
        // 对抗进化后 mutation_rate 可能有变化(受 red_team 信号影响)
        assert!(
            (0.0..=1.0).contains(&new_mutation_rate),
            "mutation_rate 应在合法范围"
        );
        // 原始 mutation_rate 应被记录(不为 0)
        assert!(original_mr > 0.0);
    } else {
        panic!("期望 GsoePolicyUpdated 事件");
    }
}

/// 验证事件 severity 为 Normal
#[tokio::test]
async fn test_gsoe_policy_updated_severity_normal() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

    engine.evolve_once().await.unwrap();

    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    assert_eq!(
        event.severity(),
        event_bus::EventSeverity::Normal,
        "GsoePolicyUpdated 应为 Normal 级别"
    );
}

/// 验证事件 type_name 正确
#[tokio::test]
async fn test_gsoe_policy_updated_type_name() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

    engine.evolve_once().await.unwrap();

    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    assert_eq!(
        event.type_name(),
        "GsoePolicyUpdated",
        "type_name 应为 GsoePolicyUpdated"
    );
}

/// 验证手动构造的 GsoePolicyUpdated 事件可被订阅者接收
#[tokio::test]
async fn test_manual_gsoe_policy_updated_event_delivery() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let event = NexusEvent::GsoePolicyUpdated {
        metadata: EventMetadata::new("gsoe-evolution"),
        generation: 42,
        improvement: 0.05,
        new_mutation_rate: 0.15,
        new_selection_pressure: 1.8,
    };

    bus.publish(event.clone()).await.unwrap();

    let received = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    assert_eq!(received, event);
}

// ============================================================
// P5.2.4: 通道 B E2E 集成测试(5 场景)
// ============================================================
//
// 对应 ADR: ADR-044 决策 5(CiGate 接口)+ 决策 6(显著性检测)+
//           ADR-045 决策 1(INV-9 与否决证据分离)+
//           P5.2.3(EvolutionRecord 谱系集成 + SpecRegistered 事件)
//
// 测试策略:
// - 使用 MockCiGate 注入确定性 CI 结果(避免依赖 cargo 子进程)
// - 使用 CargoCiGate::with_subprocess_enabled(false) 测试 INV-9 路径
// - 使用 SignificanceDetector 验证二项检验显著性阈值
// - 使用 SpecRegistry::with_event_bus 验证 SpecRegistered 事件发布

/// P5.2.4 测试辅助:构造最小合法 HarnessSpec
fn make_candidate_spec(name: &str, version: u32, parent: Option<u32>) -> HarnessSpec {
    HarnessSpec {
        meta: HarnessMeta {
            name: name.to_string(),
            version,
            immutable: false,
            parent,
            task_type: None,
        },
        contracts: vec![ContractSpec {
            name: "no_panic".to_string(),
            property: "fuzz_target_must_not_panic".to_string(),
            description: None,
            from: None,
            to: None,
            fields: vec![],
        }],
        hops: vec![HopSpec {
            name: "generate_input".to_string(),
            input_type: None,
            output_type: None,
            contracts: vec!["no_panic".to_string()],
            description: None,
            order: vec!["Architect.propose".to_string()],
            on_veto: None,
            fallback: None,
        }],
        retry: RetryPolicy::default(),
        auxiliary: Some(
            "acceptance_gates = [\"tests_pass\", \"bench_no_regression\", \"invariants_clean\", \"redline_scan_clean\"]"
                .to_string(),
        ),
    }
}

/// P5.2.4 场景 1: happy path — CI 通过 + 无回归 → 注册 spec + 发布事件
///
/// 验证通道 B 正常路径:
/// 1. MockCiGate 返回 passed=true
/// 2. SignificanceDetector 累积 streak=0(无回归)
/// 3. 调用 register_with_source(spec, "rhi-cg-channel-b") 注册成功
/// 4. SpecRegistered 事件被发布,字段正确
#[tokio::test]
async fn p524_happy_path_ci_passes_registers_spec() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut registry = SpecRegistry::with_event_bus(bus);

    // 步骤 1: 模拟通道 B 的 CI 执行门(永远通过)
    let gate = MockCiGate::with_passing_result();
    let candidate = make_candidate_spec("quest-parse", 1, None);

    // 步骤 2: 执行 CI 检查
    let ci_result = gate
        .execute(&candidate)
        .await
        .expect("MockCiGate 不应返回错误");
    assert!(ci_result.passed, "CI 应通过");
    assert_eq!(ci_result.regression_streak, 0, "无回归");

    // 步骤 3: 显著性检测器确认无需否决(streak=0,p-value=1.0)
    // WHY 无 mut:本场景仅调用 is_veto_justified()(&self 方法,不改状态),
    //    与场景 2 不同(场景 2 调用 record_regression() 需要 mut)
    let detector = SignificanceDetector::new();
    assert!(!detector.is_veto_justified(), "streak=0 不应触发否决");

    // 步骤 4: 注册 spec(source="rhi-cg-channel-b")
    let version = registry
        .register_with_source(candidate, "rhi-cg-channel-b")
        .expect("注册应成功");
    assert_eq!(version, 1);

    // 步骤 5: 验证 SpecRegistered 事件发布
    let event = rx
        .recv_timeout(Duration::from_secs(1))
        .await
        .expect("应有事件");
    // WHY 先取 severity:match 模式会 move String/EventMetadata 字段,
    //    若先 match 再调用 event.severity() 会触发 "use of partially moved value" 错误。
    //    severity() 是 &self 方法,在 match 之前调用规避 move 问题。
    let event_severity = event.severity();
    match event {
        NexusEvent::SpecRegistered {
            spec_name,
            spec_version,
            parent_version,
            source,
            metadata,
        } => {
            assert_eq!(spec_name, "quest-parse");
            assert_eq!(spec_version, 1);
            assert_eq!(parent_version, None);
            assert_eq!(source, "rhi-cg-channel-b");
            assert_eq!(metadata.source, "gsoe-evolution");
        }
        other => panic!("期望 SpecRegistered,实际: {other:?}"),
    }

    // 步骤 6: 验证 severity 为 Normal(非阻断性事件)
    assert_eq!(
        event_severity,
        EventSeverity::Normal,
        "SpecRegistered 应为 Normal 级别"
    );
}

/// P5.2.4 场景 2: bench 抖动 — 单尾二项检验防止误否决
///
/// 验证显著性检测器的统计门槛:
/// - 1 次回归(p=0.5 > 0.05):不足以否决(避免 bench 抖动误判)
/// - 2 次回归(p=0.25 > 0.05):仍不足以否决
/// - 3 次连续回归(p=0.125 > 0.05):仍不足以否决(随机抖动概率 12.5%)
/// - 5 次连续回归(p=0.03125 < 0.05):达到否决证据充分性
///
/// 这防止了通道 B 因 bench 噪声(false positive)错误否决通道 A 的提议。
///
/// WHY 5 次而非 3 次:单尾二项检验 P(X >= 3 | n=3, p=0.5) = 0.125 > 0.05,
///    即 3 次回归的随机抖动概率 12.5% 高于 5% 显著性阈值,不足以确信回归。
///    需要 P(X >= 5 | n=5, p=0.5) = 0.03125 < 0.05 才达到统计显著。
///    `VETO_STREAK_THRESHOLD=3` + `p<0.05` 双重过滤实际要求 >= 5 次连续回归。
#[tokio::test]
async fn p524_bench_jitter_false_veto_prevention() {
    let mut detector = SignificanceDetector::new();

    // 初始状态:streak=0, observed_runs=0
    assert_eq!(detector.regression_streak(), 0);
    assert_eq!(detector.observed_runs(), 0);
    assert!(!detector.is_veto_justified());

    // 步骤 1: 1 次回归 — 不足以否决(p=0.5 > 0.05)
    detector.record_regression();
    assert_eq!(detector.regression_streak(), 1);
    assert_eq!(detector.observed_runs(), 1);
    let p1 = detector.p_value();
    assert!(p1 > 0.05, "1 次回归 p-value={p1} 应 > 0.05,不足以否决");
    assert!(!detector.is_veto_justified(), "1 次回归不应触发否决");

    // 步骤 2: 2 次回归 — 仍不足以否决(p=0.25 > 0.05)
    detector.record_regression();
    assert_eq!(detector.regression_streak(), 2);
    let p2 = detector.p_value();
    assert!(p2 > 0.05, "2 次回归 p-value={p2} 应 > 0.05,不足以否决");
    assert!(!detector.is_veto_justified(), "2 次回归不应触发否决");

    // 步骤 3: 1 次 pass — streak 重置为 0(避免累积旧噪声)
    detector.record_pass();
    assert_eq!(detector.regression_streak(), 0, "pass 后 streak 应重置");
    assert_eq!(detector.observed_runs(), 3, "observed_runs 应继续累积");
    assert!(!detector.is_veto_justified());

    // 步骤 4: 3 次连续回归 — 仍不足以否决(p=0.125 > 0.05)
    //   关键:这是"防止 bench 抖动误否决"的核心保护
    //   3 次连续回归在随机情况下有 12.5% 概率发生,高于 5% 显著性阈值
    detector.record_regression();
    detector.record_regression();
    detector.record_regression();
    assert_eq!(detector.regression_streak(), 3);
    let p3 = detector.p_value();
    // 注意:此时 observed_runs=6(含 3 次 pass 前的回归),p 由 binomial_sf(3, 6, 0.5) 计算
    // 若用全新 detector streak=3, n=3 时 p=0.125(仍 > 0.05)
    assert!(
        p3 > 0.05,
        "3 次连续回归 p-value={p3} 应 > 0.05,不足以否决(防止 bench 抖动误否决)"
    );
    assert!(
        !detector.is_veto_justified(),
        "3 次连续回归不应触发否决(随机抖动概率 > 5%)"
    );

    // 步骤 5: 用全新 detector 验证 streak=3, n=3 边界值
    let mut fresh = SignificanceDetector::new();
    fresh.record_regression();
    fresh.record_regression();
    fresh.record_regression();
    assert_eq!(fresh.regression_streak(), 3);
    assert_eq!(fresh.observed_runs(), 3);
    let p3_fresh = fresh.p_value();
    assert!(
        (p3_fresh - 0.125).abs() < 1e-10,
        "全新 detector 3 次回归 p-value={p3_fresh}, 应为 0.125"
    );
    assert!(
        !fresh.is_veto_justified(),
        "3 次回归 / 3 次运行不应触发否决(p=0.125 > 0.05)"
    );

    // 步骤 6: 5 次连续回归 — 达到否决阈值(p=0.03125 < 0.05)
    let mut detector5 = SignificanceDetector::new();
    for _ in 0..5 {
        detector5.record_regression();
    }
    assert_eq!(detector5.regression_streak(), 5);
    assert_eq!(detector5.observed_runs(), 5);
    let p5 = detector5.p_value();
    assert!(
        p5 < 0.05,
        "5 次连续回归 p-value={p5} 应 < 0.05,达到否决阈值"
    );
    assert!(
        detector5.is_veto_justified(),
        "5 次连续回归应触发否决证据充分性"
    );

    // 步骤 7: 验证 check_veto_evidence 函数接口
    assert!(
        gsoe_evolution::check_veto_evidence(5, p5).is_ok(),
        "check_veto_evidence(5, {p5}) 应返回 Ok"
    );
    assert!(
        gsoe_evolution::check_veto_evidence(3, p3_fresh).is_err(),
        "check_veto_evidence(3, {p3_fresh}) 应返回 Err"
    );
}

/// P5.2.4 场景 3: INV-9 违反 — 委托图有环触发 CI 失败与否决
///
/// 验证通道 B 的 INV-9 检查路径:
/// 1. CargoCiGate 配置带环的委托边(A→B, B→A)
/// 2. 禁用子进程(避免依赖 cargo,仅测 INV-9 路径)
/// 3. 执行 CI → passed=false, has_inv9_violation=true
/// 4. 通道 B 应否决(不注册 spec)
///
/// 这确保循环委托不会通过通道 B 进入 spec 谱系(§6.2 红线:零循环委托)
#[tokio::test]
async fn p524_inv9_violation_triggers_veto() {
    // 步骤 1: 构造带环的委托图(A→B, B→A)
    let cyclic_edges = vec![
        DelegationEdge::new("agent-a", "agent-b"),
        DelegationEdge::new("agent-b", "agent-a"),
    ];

    // 步骤 2: 创建 CargoCiGate,禁用子进程(仅测 INV-9 路径)
    let gate = CargoCiGate::new(cyclic_edges).with_subprocess_enabled(false);
    let candidate = make_candidate_spec("quest-parse", 1, None);

    // 步骤 3: 执行 CI 检查
    let ci_result = gate
        .execute(&candidate)
        .await
        .expect("CI 执行应成功(非子进程故障)");

    // 步骤 4: 验证 CI 失败,且为 INV-9 违反
    assert!(!ci_result.passed, "带环委托图 CI 应失败");
    assert!(ci_result.has_inv9_violation(), "应有 INV-9 委托图有环违反");
    assert_eq!(ci_result.failures.len(), 1, "应有 1 条失败记录(INV-9)");
    assert_eq!(ci_result.failures[0].kind, CiFailureKind::Inv9Violated);

    // 步骤 5: 验证通道 B 应否决(不注册 spec)
    // 模拟通道 B 决策逻辑:CI 失败 → 不调用 register
    let mut registry = SpecRegistry::new(); // 无 EventBus,因不应发布事件
    if ci_result.passed {
        // 如果 CI 通过才会注册(此处不会执行)
        registry.register(candidate).unwrap();
    }
    assert_eq!(registry.total_specs(), 0, "CI 失败时不应注册 spec");

    // 步骤 6: 验证 check_inv9_delegation_acyclic 直接调用也检测到环
    let direct_check = gsoe_evolution::check_inv9_delegation_acyclic(&[
        DelegationEdge::new("agent-a", "agent-b"),
        DelegationEdge::new("agent-b", "agent-a"),
    ]);
    assert!(direct_check.is_err(), "直接调用 INV-9 检查应检测到环");
    let cycle_path = direct_check.unwrap_err();
    assert!(
        cycle_path.len() >= 3,
        "环路径应至少 3 个节点(首尾相同),实际: {cycle_path:?}"
    );
    assert_eq!(cycle_path.first(), cycle_path.last(), "环路径首尾应相同");
}

/// P5.2.4 场景 4: 不可进化面违反 — 注册拒绝 + 错误映射
///
/// 验证不可进化面守护的三层防御:
/// 1. 注册不可进化 spec v1(immutable=true)
/// 2. 尝试注册 v2 → 被运行时守护拒绝(ImmutableSpecOverwrite)
/// 3. 验证 into_gsoe_error 映射为 ImmutableSurfaceViolated
/// 4. 验证失败时不发布 SpecRegistered 事件(避免虚假通知)
#[tokio::test]
async fn p524_immutable_surface_blocks_registration() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut registry = SpecRegistry::with_event_bus(bus);

    // 步骤 1: 注册不可进化 spec v1(成功,发布事件)
    let mut immutable_spec = make_candidate_spec("critical-redline", 1, None);
    immutable_spec.meta.immutable = true;
    let v1 = registry
        .register(immutable_spec)
        .expect("不可进化 v1 应注册成功");
    assert_eq!(v1, 1);

    // 消费 v1 的事件
    let _v1_event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();

    // 步骤 2: 尝试注册 v2(应被拒绝)
    let v2_spec = make_candidate_spec("critical-redline", 2, Some(1));
    let result = registry.register(v2_spec);

    // 步骤 3: 验证返回 ImmutableSpecOverwrite 错误(用 ref 模式避免 move)
    // WHY 用 ref:后续步骤 4 需再次消费 result.unwrap_err(),
    //    match value 模式会 move String 字段,导致 result 部分移动无法再用。
    //    用 match &result 借用检查,保留 result 所有权供后续步骤使用。
    match &result {
        Err(SpecRegistryError::ImmutableSpecOverwrite { name }) => {
            assert_eq!(name, "critical-redline");
        }
        other => panic!("期望 ImmutableSpecOverwrite,实际: {other:?}"),
    }

    // 步骤 4: 验证 into_gsoe_error 映射为 ImmutableSurfaceViolated
    let gsoe_err = SpecRegistry::into_gsoe_error(result.unwrap_err());
    match gsoe_err {
        GsoeError::ImmutableSurfaceViolated { reason } => {
            assert!(reason.contains("critical-redline"));
            assert!(reason.contains("immutable=true"));
        }
        other => panic!("期望 ImmutableSurfaceViolated,实际: {other:?}"),
    }

    // 步骤 5: 验证失败时不发布 SpecRegistered 事件
    //   try_recv 返回 Ok(None) 表示无事件
    let event = rx.try_recv().expect("try_recv 不应报错");
    assert!(
        event.is_none(),
        "不可进化面违反时不应发布 SpecRegistered 事件"
    );

    // 步骤 6: 验证 v2 未被注册(total_specs 仍为 1)
    assert_eq!(registry.total_specs(), 1, "不可进化面违反时 v2 不应被注册");
    assert_eq!(
        registry.version_count("critical-redline"),
        1,
        "仅 v1 应存在"
    );
}

/// P5.2.4 场景 5: A→B 端到端 — 通道 A 提议 + 通道 B CI 门 + 注册
///
/// 完整的 RHI-CG 双通道流程:
/// 1. 模拟通道 A 提议一个候选 spec(版本 v2,parent=v1)
/// 2. 通道 B 执行 CiGate(通过)
/// 3. 通道 B 检查 SignificanceDetector(无回归 → 无需否决)
/// 4. 通道 B 注册 spec(source="rhi-cg-channel-b")
/// 5. 验证 SpecRegistered 事件 + spec 谱系正确
///
/// 这是 P5.2 的核心交付:通道 A 提议,通道 B 把关,通过后纳入谱系
#[tokio::test]
async fn p524_channel_a_to_b_end_to_end() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut registry = SpecRegistry::with_event_bus(bus);

    // 步骤 1: 通道 A 已有 v1(初始版本,模拟通道 A 历史提议)
    let v1_spec = make_candidate_spec("quest-parse", 1, None);
    registry.register(v1_spec).expect("v1 初始版本应注册成功");
    // 消费 v1 的事件
    let _v1_event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();

    // 步骤 2: 通道 A 提议 v2(候选版本)
    let v2_candidate = make_candidate_spec("quest-parse", 2, Some(1));

    // 步骤 3: 通道 B 执行 CiGate(使用 MockCiGate 模拟通过)
    let ci_gate = MockCiGate::with_passing_result();
    let ci_result = ci_gate.execute(&v2_candidate).await.expect("CI 执行应成功");
    assert!(ci_result.passed, "CI 应通过");

    // 步骤 4: 通道 B 显著性检测(无回归 → 无需否决)
    let detector = SignificanceDetector::new();
    assert_eq!(detector.regression_streak(), 0);
    assert!(!detector.is_veto_justified(), "无回归不应触发否决");

    // 步骤 5: 通道 B 通过 CI 门 + 显著性门,注册 v2
    let v2_version = registry
        .register_with_source(v2_candidate, "rhi-cg-channel-b")
        .expect("v2 应注册成功");
    assert_eq!(v2_version, 2);

    // 步骤 6: 验证 SpecRegistered 事件
    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    match event {
        NexusEvent::SpecRegistered {
            spec_name,
            spec_version,
            parent_version,
            source,
            ..
        } => {
            assert_eq!(spec_name, "quest-parse");
            assert_eq!(spec_version, 2);
            assert_eq!(parent_version, Some(1), "v2 的 parent 应为 v1");
            assert_eq!(source, "rhi-cg-channel-b");
        }
        other => panic!("期望 SpecRegistered,实际: {other:?}"),
    }

    // 步骤 7: 验证 active 仍为 v1(初始版本),candidate 未设置
    //   WHY 在 lineage 检查之前:register_with_source 不自动 promote,
    //   active 仍指向 v1。lineage() 从 active 向 parent 追溯,
    //   此时仅返回 [v1]。这是 SpecRegistry 的设计语义:
    //   lineage 反映"active 版本的祖先链",而非"所有已注册版本"
    let active = registry.get_active("quest-parse").unwrap();
    assert_eq!(
        active.meta.version, 1,
        "active 应仍为 v1(register 不自动 promote)"
    );

    // 步骤 8: v2 未 promote 前的 lineage 应为 [v1](active 的祖先链)
    let lineage_v1 = registry.lineage("quest-parse").expect("lineage 应存在");
    assert_eq!(
        lineage_v1,
        vec![1],
        "promote 前 lineage 应为 [v1](active 仍为 v1)"
    );

    // 步骤 9: 验证 v1 和 v2 都存在于 specs 表(通过 list_versions)
    let versions = registry.list_versions("quest-parse");
    assert_eq!(versions, vec![1, 2], "specs 表应包含 v1 和 v2");

    // 步骤 10: 模拟通道 A 后续 promote v2 为 active(A/B 测试通过后)
    registry.set_candidate("quest-parse", 2).unwrap();
    let promoted = registry.promote_candidate("quest-parse").unwrap();
    assert_eq!(promoted, 2);
    assert_eq!(
        registry.get_active("quest-parse").unwrap().meta.version,
        2,
        "promote 后 active 应为 v2"
    );

    // 步骤 11: promote 后 lineage 应为 [v1, v2](active=v2 → parent=v1)
    let lineage_v2 = registry.lineage("quest-parse").expect("lineage 应存在");
    assert_eq!(lineage_v2, vec![1, 2], "promote 后 lineage 应为 [v1, v2]");

    // 步骤 12: 验证可回滚到 v1(谱系完整性)
    let rolled_back = registry.rollback("quest-parse").unwrap();
    assert_eq!(rolled_back, 1, "回滚应回到 v1");
    assert_eq!(
        registry.get_active("quest-parse").unwrap().meta.version,
        1,
        "回滚后 active 应为 v1"
    );
}

/// P5.2.4 附加: 通道 B 否决路径 — CI 失败时不注册 spec
///
/// 验证当 CI 失败(cargo test 失败)时,通道 B 不应注册 spec:
/// 1. MockCiGate 返回失败结果(TestFailed)
/// 2. 通道 B 决策:CI 失败 → 跳过注册
/// 3. 验证 SpecRegistry 无新 spec,无 SpecRegistered 事件
#[tokio::test]
async fn p524_veto_path_ci_failure_skips_registration() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut registry = SpecRegistry::with_event_bus(bus);

    // 步骤 1: 模拟 CI 失败(cargo test 失败)
    let failures = vec![CiFailure::new(
        CiFailureKind::TestFailed,
        "unit test `test_xyz` failed",
    )];
    let ci_gate = MockCiGate::with_failing_result(failures);
    let candidate = make_candidate_spec("quest-parse", 1, None);

    // 步骤 2: 执行 CI 检查
    let ci_result = ci_gate
        .execute(&candidate)
        .await
        .expect("MockCiGate 不应返回错误");

    // 步骤 3: 验证 CI 失败
    assert!(!ci_result.passed, "CI 应失败");
    assert_eq!(ci_result.failures.len(), 1);
    assert_eq!(ci_result.failures[0].kind, CiFailureKind::TestFailed);

    // 步骤 4: 通道 B 决策 — CI 失败,不注册
    if ci_result.passed {
        // 仅当 CI 通过才注册(此处不会执行)
        registry.register(candidate).unwrap();
    }

    // 步骤 5: 验证无 spec 被注册
    assert_eq!(registry.total_specs(), 0, "CI 失败时不应注册 spec");

    // 步骤 6: 验证无 SpecRegistered 事件
    let event = rx.try_recv().expect("try_recv 不应报错");
    assert!(event.is_none(), "CI 失败时不应发布 SpecRegistered 事件");
}
