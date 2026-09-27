//! CSN 替代器集成测试 — 验证能力缺失 → 替代查询 → 降级链触发 → 事件发布全链路
//!
//! 对应 SubTask 2.6:集成测试
//!
//! # 验证场景
//! 1. 能力注册 + 替代查询返回正确候选(Top-K + tier 分配)
//! 2. 能力缺失 → trigger_substitution → 创建降级链 → 发布事件
//! 3. 降级链逐级推进(next_level → ChainExhausted)
//! 4. 降级链重置(reset → level 0)
//! 5. EventBus 集成:CsnSubstitutionTriggered 事件字段正确性
//! 6. MCP Mesh 事务失败 → 订阅任务推进降级链
//! 7. 性能验证:单次替代查询 p95 ≤ 30ms(#[ignore],需手动运行)

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use csn_substitutor::{
    CapabilityDescriptor, CapabilityMetadata, CsnConfig, CsnError, CsnSubstitutor,
    SubstitutionCandidate,
};
use event_bus::{EventBus, EventMetadata, EventSeverity, NexusEvent};
use std::time::{Duration, Instant};

// === 辅助函数 ===

/// 创建 50 维全 1.0 向量(用于高相似度场景)
fn make_uniform_vector(dim: usize, value: f32) -> Vec<f32> {
    vec![value; dim]
}

/// 创建前 `active` 维为 1.0、其余为 0.0 的向量(用于可控相似度场景)
///
/// 与全 1.0 向量的余弦相似度 = sqrt(active/dim),
/// 例如 make_partial_vector(50, 30) 与 [1.0; 50] 的余弦 ≈ 0.775
fn make_partial_vector(dim: usize, active: usize) -> Vec<f32> {
    let mut v = vec![0.0; dim];
    for item in v.iter_mut().take(active.min(dim)) {
        *item = 1.0;
    }
    v
}

/// 注册多个能力到替代器
fn register_caps(sub: &CsnSubstitutor, caps: Vec<(&str, Vec<f32>)>) {
    for (id, v) in caps {
        sub.register_capability(CapabilityDescriptor::new(id, v))
            .expect("注册失败");
    }
}

// === 1. 能力注册 + 替代查询(Top-K + tier 分配)===

#[test]
fn test_register_and_find_substitutes_top_k() {
    let sub = CsnSubstitutor::new(CsnConfig::default());
    register_caps(
        &sub,
        vec![
            ("cap-1", make_uniform_vector(50, 1.0)),
            ("cap-2", make_uniform_vector(50, 0.99)), // 与 cap-1 极相似(余弦=1.0)
            ("cap-3", make_uniform_vector(50, 0.9)),  // 与 cap-1 极相似(余弦=1.0)
            // v2.9.0-omega: cap-4 改用 partial 向量,确保余弦 > 0.5(通过 similarity_threshold 过滤)
            // make_partial_vector(50, 30) 与 [1.0;50] 的余弦 ≈ 0.775 > 0.5
            ("cap-4", make_partial_vector(50, 30)),
        ],
    );

    let candidates = sub.find_substitutes("cap-1", 3);
    assert_eq!(
        candidates.len(),
        3,
        "应返回 Top-3 候选(所有候选相似度 > 0.5)"
    );

    // 验证降序排列
    assert!(candidates[0].similarity_score >= candidates[1].similarity_score);
    assert!(candidates[1].similarity_score >= candidates[2].similarity_score);

    // 验证 tier 分配:rank 0→tier 0, rank 1→tier 1, rank 2→tier 2
    assert_eq!(candidates[0].tier, 0, "rank 0 → tier 0 (primary)");
    assert_eq!(candidates[1].tier, 1, "rank 1 → tier 1 (secondary)");
    assert_eq!(candidates[2].tier, 2, "rank 2 → tier 2 (tertiary)");
}

#[test]
fn test_find_substitutes_excludes_self() {
    let sub = CsnSubstitutor::new(CsnConfig::default());
    register_caps(
        &sub,
        vec![
            ("cap-1", make_uniform_vector(50, 1.0)),
            ("cap-2", make_uniform_vector(50, 1.0)),
        ],
    );

    let candidates = sub.find_substitutes("cap-1", 5);
    assert_eq!(candidates.len(), 1, "仅 cap-2 是候选(排除自身)");
    assert_eq!(candidates[0].candidate_id, "cap-2");
}

#[test]
fn test_find_substitutes_unregistered_returns_empty() {
    let sub = CsnSubstitutor::new(CsnConfig::default());
    let candidates = sub.find_substitutes("missing", 5);
    assert!(candidates.is_empty(), "未注册能力应返回空候选列表");
}

// === 2. trigger_substitution 全链路 ===

#[tokio::test]
async fn test_trigger_substitution_full_flow() {
    let sub = CsnSubstitutor::new(CsnConfig::default());
    register_caps(
        &sub,
        vec![
            ("cap-1", make_uniform_vector(50, 1.0)),
            ("cap-2", make_uniform_vector(50, 0.9)),
        ],
    );

    // 触发替代
    let candidate = sub.trigger_substitution("cap-1").await.expect("应找到替代");

    // 验证候选
    assert_eq!(candidate.candidate_id, "cap-2");
    assert!(candidate.similarity_score > 0.0);

    // 验证降级链已创建
    assert_eq!(sub.chain_count(), 1, "应创建 1 条降级链");
    assert!(sub.degradation_level("cap-1").is_some());
}

#[tokio::test]
async fn test_trigger_substitution_no_candidate_error() {
    let sub = CsnSubstitutor::new(CsnConfig::default());
    // 仅注册 1 个能力,无替代候选
    register_caps(&sub, vec![("cap-1", make_uniform_vector(50, 1.0))]);

    let result = sub.trigger_substitution("cap-1").await;
    assert!(matches!(result, Err(CsnError::NoSubstituteFound { .. })));
}

#[tokio::test]
async fn test_trigger_substitution_unregistered_error() {
    let sub = CsnSubstitutor::new(CsnConfig::default());
    let result = sub.trigger_substitution("missing").await;
    assert!(matches!(result, Err(CsnError::NoSubstituteFound { .. })));
}

// === 3. 降级链逐级推进 ===

#[tokio::test]
async fn test_degradation_chain_progression() {
    let sub = CsnSubstitutor::new(CsnConfig::default());
    register_caps(
        &sub,
        vec![
            ("cap-1", make_uniform_vector(50, 1.0)),
            ("cap-2", make_uniform_vector(50, 0.9)),
        ],
    );

    // 首次触发:创建降级链
    sub.trigger_substitution("cap-1").await.unwrap();
    let initial_level = sub.degradation_level("cap-1").expect("降级链应存在");

    // 推进降级链
    sub.advance_degradation("cap-1").expect("应推进到下一级");
    let advanced_level = sub.degradation_level("cap-1").expect("降级链应存在");
    assert!(
        advanced_level > initial_level,
        "推进后层级应增加: {initial_level} → {advanced_level}"
    );
}

#[tokio::test]
async fn test_degradation_chain_exhausted() {
    let sub = CsnSubstitutor::new(CsnConfig::default());
    register_caps(
        &sub,
        vec![
            ("cap-1", make_uniform_vector(50, 1.0)),
            ("cap-2", make_uniform_vector(50, 0.9)),
        ],
    );

    // 创建降级链
    sub.trigger_substitution("cap-1").await.unwrap();

    // 推进到末端(默认 3 级:level 0 → 1 → 2)
    sub.advance_degradation("cap-1").expect("推进到 level 1");
    sub.advance_degradation("cap-1").expect("推进到 level 2");

    // 已耗尽,应返回错误
    let result = sub.advance_degradation("cap-1");
    assert!(
        matches!(result, Err(CsnError::ChainExhausted { .. })),
        "末端推进应返回 ChainExhausted"
    );
}

#[tokio::test]
async fn test_degradation_chain_reset() {
    let sub = CsnSubstitutor::new(CsnConfig::default());
    register_caps(
        &sub,
        vec![
            ("cap-1", make_uniform_vector(50, 1.0)),
            ("cap-2", make_uniform_vector(50, 0.9)),
        ],
    );

    // 创建并推进降级链
    sub.trigger_substitution("cap-1").await.unwrap();
    sub.advance_degradation("cap-1").expect("推进到 level 1");
    assert_eq!(sub.degradation_level("cap-1"), Some(1));

    // 重置
    sub.reset_chain("cap-1").expect("重置应成功");
    assert_eq!(
        sub.degradation_level("cap-1"),
        Some(0),
        "重置后应回到 level 0"
    );
}

// === 4. EventBus 集成:CsnSubstitutionTriggered 事件 ===

#[tokio::test]
async fn test_trigger_substitution_publishes_event() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let sub = CsnSubstitutor::with_event_bus(CsnConfig::default(), bus);
    register_caps(
        &sub,
        vec![
            ("cap-1", make_uniform_vector(50, 1.0)),
            ("cap-2", make_uniform_vector(50, 0.9)),
        ],
    );

    // 触发替代
    let candidate = sub.trigger_substitution("cap-1").await.expect("应找到替代");

    // 验证事件
    let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("5s 内未收到事件(资源竞争或事件丢失)")
        .expect("应收到事件");
    match event {
        NexusEvent::CsnSubstitutionTriggered {
            original_capability_id,
            substitute_id,
            similarity_score,
            degradation_level,
            ..
        } => {
            assert_eq!(original_capability_id, "cap-1");
            assert_eq!(substitute_id, candidate.candidate_id);
            assert!((similarity_score - candidate.similarity_score).abs() < 1e-5);
            // 首次触发:degradation_level 应为 0(primary)
            assert_eq!(degradation_level, 0);
        }
        _ => panic!(
            "期望 CsnSubstitutionTriggered 事件,得到 {:?}",
            event.type_name()
        ),
    }
}

#[tokio::test]
async fn test_event_severity_normal() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let sub = CsnSubstitutor::with_event_bus(CsnConfig::default(), bus);
    register_caps(
        &sub,
        vec![
            ("cap-1", make_uniform_vector(50, 1.0)),
            ("cap-2", make_uniform_vector(50, 0.9)),
        ],
    );

    sub.trigger_substitution("cap-1").await.unwrap();

    let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("5s 内未收到事件(资源竞争或事件丢失)")
        .expect("应收到事件");
    assert_eq!(
        event.severity(),
        EventSeverity::Normal,
        "CsnSubstitutionTriggered 应为 Normal 级别"
    );
    assert_eq!(event.type_name(), "CsnSubstitutionTriggered");
    assert_eq!(event.metadata().source, "csn-substitutor");
}

#[tokio::test]
async fn test_no_event_without_bus() {
    // 无 EventBus 时,trigger_substitution 应正常完成,不 panic
    let sub = CsnSubstitutor::new(CsnConfig::default());
    register_caps(
        &sub,
        vec![
            ("cap-1", make_uniform_vector(50, 1.0)),
            ("cap-2", make_uniform_vector(50, 0.9)),
        ],
    );

    let result = sub.trigger_substitution("cap-1").await;
    assert!(result.is_ok(), "无 EventBus 也应成功触发替代");
}

#[tokio::test]
async fn test_multiple_triggers_publish_multiple_events() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let sub = CsnSubstitutor::with_event_bus(CsnConfig::default(), bus);
    register_caps(
        &sub,
        vec![
            ("cap-1", make_uniform_vector(50, 1.0)),
            ("cap-2", make_uniform_vector(50, 0.9)),
            ("cap-3", make_uniform_vector(50, 1.0)),
            ("cap-4", make_uniform_vector(50, 0.9)),
        ],
    );

    // 连续触发 3 次替代
    for _ in 0..3 {
        sub.trigger_substitution("cap-1").await.expect("触发失败");
    }

    // 应收到 3 个 CsnSubstitutionTriggered 事件
    let mut count = 0;
    for _ in 0..3 {
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("5s 内未收到事件(资源竞争或事件丢失)")
            .expect("应收到事件");
        if let NexusEvent::CsnSubstitutionTriggered { .. } = event {
            count += 1;
        }
    }
    assert_eq!(count, 3, "应收到 3 个 CsnSubstitutionTriggered 事件");
}

// === 5. MCP Mesh 事务失败 → 推进降级链 ===

#[tokio::test]
async fn test_mcp_mesh_failure_advances_chain() {
    let bus = EventBus::new();
    let sub = CsnSubstitutor::with_event_bus(CsnConfig::default(), bus.clone());
    register_caps(
        &sub,
        vec![
            ("cap-1", make_uniform_vector(50, 1.0)),
            ("cap-2", make_uniform_vector(50, 0.9)),
        ],
    );

    // 创建降级链
    sub.trigger_substitution("cap-1").await.unwrap();
    let initial_level = sub.degradation_level("cap-1").expect("降级链应存在");

    // 启动订阅任务
    let handle = sub.start_degradation_listener().expect("应启动订阅");

    // 发布 MCP Mesh 事务失败事件
    bus.publish(NexusEvent::McpMeshTransactionCompleted {
        metadata: EventMetadata::new("mcp-mesh"),
        transaction_id: "tx-1".into(),
        participant_count: 3,
        latency_ms: 100,
        success: false,
        capability_id: None,
    })
    .await
    .expect("发布失败");

    // 等待后台任务处理
    tokio::time::sleep(Duration::from_millis(150)).await;

    // 验证降级链已推进
    let advanced_level = sub.degradation_level("cap-1").expect("降级链应存在");
    assert!(
        advanced_level > initial_level,
        "MCP 事务失败应推进降级链: {initial_level} → {advanced_level}"
    );

    handle.abort();
}

#[tokio::test]
async fn test_mcp_mesh_success_does_not_advance_chain() {
    let bus = EventBus::new();
    let sub = CsnSubstitutor::with_event_bus(CsnConfig::default(), bus.clone());
    register_caps(
        &sub,
        vec![
            ("cap-1", make_uniform_vector(50, 1.0)),
            ("cap-2", make_uniform_vector(50, 0.9)),
        ],
    );

    // 创建降级链
    sub.trigger_substitution("cap-1").await.unwrap();
    let initial_level = sub.degradation_level("cap-1").expect("降级链应存在");

    let handle = sub.start_degradation_listener().expect("应启动订阅");

    // 发布 MCP Mesh 事务成功事件
    bus.publish(NexusEvent::McpMeshTransactionCompleted {
        metadata: EventMetadata::new("mcp-mesh"),
        transaction_id: "tx-1".into(),
        participant_count: 3,
        latency_ms: 50,
        success: true,
        capability_id: None,
    })
    .await
    .expect("发布失败");

    tokio::time::sleep(Duration::from_millis(150)).await;

    // 验证降级链未推进(事务成功不应触发降级)
    let level_after = sub.degradation_level("cap-1").expect("降级链应存在");
    assert_eq!(level_after, initial_level, "事务成功不应推进降级链");

    handle.abort();
}

// === 6. 能力元数据传递 ===

#[test]
fn test_capability_metadata_preserved() {
    let sub = CsnSubstitutor::new(CsnConfig::default());
    let meta = CapabilityMetadata::new("shell", "1.0.0").with_critical(true);
    let cap = CapabilityDescriptor::new("cap-1", make_uniform_vector(50, 1.0)).with_metadata(meta);
    sub.register_capability(cap).expect("注册失败");

    let found = sub.registry().get("cap-1").expect("应找到能力");
    assert_eq!(found.metadata.category, "shell");
    assert_eq!(found.metadata.version, "1.0.0");
    assert!(found.metadata.critical);
}

// === 7. 性能验证(#[ignore],手动运行: cargo test --ignored -p csn-substitutor) ===

#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_perf_substitution_latency_p95_under_30ms() {
    // 注册 100 个能力(设计目标上限)
    let sub = CsnSubstitutor::new(CsnConfig::default());
    for i in 0..100 {
        let id = format!("cap-{i}");
        let vector: Vec<f32> = (0..50)
            .map(|j| (i as f32 + j as f32 * 0.01) * 0.1)
            .collect();
        let cap = CapabilityDescriptor::new(id, vector);
        sub.register_capability(cap).expect("注册失败");
    }

    // 测量 1000 次替代查询延迟
    let mut latencies: Vec<Duration> = Vec::with_capacity(1000);
    for i in 0..1000 {
        let id = format!("cap-{}", i % 100);
        let start = Instant::now();
        let _candidates: Vec<SubstitutionCandidate> = sub.find_substitutes(&id, 5);
        latencies.push(start.elapsed());
    }

    // 计算 p95
    latencies.sort();
    let p95_idx = (latencies.len() as f64 * 0.95) as usize;
    let p95 = latencies[p95_idx];

    assert!(p95.as_millis() <= 30, "p95 延迟应 ≤ 30ms,实际: {:?}", p95);
}

#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_perf_trigger_substitution_latency_under_30ms() {
    // 测量 trigger_substitution 端到端延迟(含降级链创建)
    let sub = CsnSubstitutor::new(CsnConfig::default());
    for i in 0..100 {
        let id = format!("cap-{i}");
        let vector: Vec<f32> = (0..50)
            .map(|j| (i as f32 + j as f32 * 0.01) * 0.1)
            .collect();
        let cap = CapabilityDescriptor::new(id, vector);
        sub.register_capability(cap).expect("注册失败");
    }

    let mut latencies: Vec<Duration> = Vec::with_capacity(100);
    for i in 0..100 {
        let id = format!("cap-{i}");
        let start = Instant::now();
        let _ = sub.trigger_substitution(&id).await;
        latencies.push(start.elapsed());
    }

    latencies.sort();
    let p95_idx = (latencies.len() as f64 * 0.95) as usize;
    let p95 = latencies[p95_idx];

    assert!(
        p95.as_millis() <= 30,
        "trigger_substitution p95 延迟应 ≤ 30ms,实际: {:?}",
        p95
    );
}

// ============================================================
// P1-1: 配额耗尽降级链接线测试(ADR-068 M3)
// ============================================================

use nexus_contracts::affinity::{CapabilitySet, StatePreservationPolicy, ThinkingSupport};

/// 构造能力集(测试辅助,对齐 mca_quota_switch_e2e.rs 的 caps())
fn caps(
    tool_calling: bool,
    thinking: ThinkingSupport,
    window: u32,
    state: StatePreservationPolicy,
) -> CapabilitySet {
    let mut c = CapabilitySet::minimal_text(window, 8192);
    c.tool_calling = tool_calling;
    c.thinking = thinking;
    c.state_preservation = state;
    c
}

/// 通道能力注册表:注册/查询/排除耗尽通道(P1-1)
#[test]
fn test_channel_registry_register_and_exclude() {
    use csn_substitutor::ChannelAffinityRegistry;
    let reg = ChannelAffinityRegistry::new();
    let exhausted = caps(
        true,
        ThinkingSupport::OnOff,
        1_000_000,
        StatePreservationPolicy::None,
    );
    reg.register("deep_seek/deepseek-v4-flash", exhausted);
    reg.register(
        "zhipu/glm-5.2",
        caps(
            true,
            ThinkingSupport::EffortLevels(vec!["low".into()]),
            1_000_000,
            StatePreservationPolicy::BlockPreservation,
        ),
    );
    assert_eq!(reg.len(), 2, "应注册 2 个通道");
    assert!(reg.get("deep_seek/deepseek-v4-flash").is_some());
    // 排除耗尽通道后仅剩 1 个候选
    let candidates = reg.candidates_excluding("deep_seek/deepseek-v4-flash");
    assert_eq!(candidates.len(), 1, "排除耗尽通道后应剩 1 个候选");
    assert_eq!(candidates[0].0, "zhipu/glm-5.2");
    assert!(reg.unregister("zhipu/glm-5.2"), "注销应成功");
    assert_eq!(reg.len(), 1);
}

/// 核心:发布 AffinityQuotaExhausted → listener 消费 → 发布 CsnSubstitutionTriggered(P1-1)
#[tokio::test]
async fn test_quota_exhausted_triggers_channel_switch() {
    let bus = EventBus::new();
    let sub = CsnSubstitutor::with_event_bus(CsnConfig::default(), bus.clone());
    // 装配通道能力(组合根角色,与 mca_quota_switch_e2e 对齐)
    sub.register_channel(
        "deep_seek/deepseek-v4-flash",
        caps(
            true,
            ThinkingSupport::OnOff,
            1_000_000,
            StatePreservationPolicy::None,
        ),
    );
    sub.register_channel(
        "zhipu/glm-5.2",
        caps(
            true,
            ThinkingSupport::EffortLevels(vec!["low".into(), "high".into()]),
            1_000_000,
            StatePreservationPolicy::BlockPreservation,
        ),
    );
    sub.register_channel(
        "step_fun/step-3.5-flash-2603",
        caps(
            false,
            ThinkingSupport::OnOff,
            262_144,
            StatePreservationPolicy::None,
        ),
    );

    // 订阅 CsnSubstitutionTriggered(先订阅再发布,§4.4 反模式 3)
    let mut triggered_rx = bus.subscribe();
    let handle = sub.start_degradation_listener().expect("应启动 listener");

    // 发布配额耗尽事件(通道 A)
    bus.publish(NexusEvent::AffinityQuotaExhausted {
        metadata: EventMetadata::new("test"),
        route_key: "deep_seek/deepseek-v4-flash".into(),
        reason: "429 quota".into(),
    })
    .await
    .unwrap();

    // 等待消费:断言 CsnSubstitutionTriggered 送达且替代为能力最相似的 GLM
    // (广播通道全量可达,循环跳过非目标事件——原 AffinityQuotaExhausted 会先到达)
    let triggered = loop {
        let event = tokio::time::timeout(Duration::from_secs(2), triggered_rx.recv())
            .await
            .expect("超时:降级链未触发")
            .expect("事件流错误");
        if matches!(event, NexusEvent::CsnSubstitutionTriggered { .. }) {
            break event;
        }
    };
    match triggered {
        NexusEvent::CsnSubstitutionTriggered {
            original_capability_id,
            substitute_id,
            degradation_level,
            ..
        } => {
            assert_eq!(original_capability_id, "deep_seek/deepseek-v4-flash");
            assert_eq!(
                substitute_id, "zhipu/glm-5.2",
                "应选能力最相似的 GLM 接管(Step 缺工具 + 小窗口,距离更大)"
            );
            assert_eq!(degradation_level, 0, "首次降级层级应为 0");
        }
        other => panic!("应收到 CsnSubstitutionTriggered: {other:?}"),
    }
    handle.abort();
}

/// 未注册通道的配额耗尽事件 → no-op(不 panic、不建链)
#[tokio::test]
async fn test_quota_exhausted_unknown_route_key_noop() {
    let bus = EventBus::new();
    let sub = CsnSubstitutor::with_event_bus(CsnConfig::default(), bus.clone());
    sub.register_channel(
        "zhipu/glm-5.2",
        caps(
            true,
            ThinkingSupport::OnOff,
            1_000_000,
            StatePreservationPolicy::None,
        ),
    );
    let handle = sub.start_degradation_listener().expect("应启动 listener");

    bus.publish(NexusEvent::AffinityQuotaExhausted {
        metadata: EventMetadata::new("test"),
        route_key: "unknown/channel".into(),
        reason: "quota".into(),
    })
    .await
    .unwrap();

    // 等待处理完成:不应 panic、不应建链
    tokio::time::sleep(Duration::from_millis(100)).await;
    handle.abort();
}

// ============================================================
// P1(§8.1): 降级链耗尽路径不得卡死 degradation listener
// ============================================================

/// 探针等待预算:listener 若在耗尽路径自锁,探针事件永远不会有产出。
const LIVENESS_BUDGET: Duration = Duration::from_secs(2);

/// 构造配额耗尽事件(测试辅助)
fn quota_event(route_key: &str) -> NexusEvent {
    NexusEvent::AffinityQuotaExhausted {
        metadata: EventMetadata::new("test"),
        route_key: route_key.into(),
        reason: "429 quota".into(),
    }
}

/// 单次事件的处理节拍(listener 是顺序 await,逐个发就得逐个等)
const TICK: Duration = Duration::from_millis(100);

/// 场景裁决:`Ok(true)` 耗尽后仍存活 / `Ok(false)` 无产出 / `Err` 前提不成立
///
/// `channels` = 注册通道数。耗尽键固定 `ch-0`,其链深 = `channels - 1`
/// (`handle_quota_exhausted` 以"候选数"作 levels 深度)⇒ 恰好 `channels` 次事件
/// 进入 `ChainExhausted` 分支,之后再加一次不同键的探针。
///
/// 前提为何显式回传而不是 panic:线程内 panic 只会让主线程观察到「超时」,
/// 于是「用例前提没成立」会被误判成「生产码卡死」。
async fn exhaustion_scenario(channels: usize) -> Result<bool, &'static str> {
    if channels < 3 {
        return Err("通道数须 ≥ 3(链深 ≥ 2)才可能进入耗尽分支");
    }
    let bus = EventBus::new();
    let sub = CsnSubstitutor::with_event_bus(CsnConfig::default(), bus.clone());
    for i in 0..channels {
        sub.register_channel(
            format!("ch-{i}"),
            caps(
                i % 2 == 0,
                if i % 3 == 0 {
                    ThinkingSupport::OnOff
                } else {
                    ThinkingSupport::EffortLevels(vec!["low".into(), "high".into()])
                },
                1_000_000,
                if i % 2 == 0 {
                    StatePreservationPolicy::None
                } else {
                    StatePreservationPolicy::BlockPreservation
                },
            ),
        );
    }

    // 先订阅再 spawn(§4.4 反模式 3)
    let mut rx = bus.subscribe();
    let handle = match sub.start_degradation_listener() {
        Some(h) => h,
        None => return Err("listener 未绑定 EventBus,场景不成立"),
    };

    // 第 1 次:建链。链没建起来就谈不上"进耗尽分支",用例会退化成假绿,故自证前提。
    // WHY 只在这里查 chain_count():此后 listener 可能已卡在写守卫上,
    // 而它要取全分片读锁(见 run_exhaustion 的 WHY 注释)。
    bus.publish(quota_event("ch-0")).await.unwrap();
    tokio::time::sleep(TICK).await;
    if sub.chain_count() != 1 {
        handle.abort();
        return Err("首次配额耗尽后未建立降级链,用例前提不成立");
    }

    // 再发 channels-1 次:第 channels 次落到 ChainExhausted 分支
    for _ in 1..channels {
        bus.publish(quota_event("ch-0")).await.unwrap();
        tokio::time::sleep(TICK).await;
    }

    // 反空转前提:走到这里 ch-0 的链必须**已被耗尽路径移除**。
    // WHY 必要:若事件数不足以推进到链尾(例如深度算错),后面的探针照样会成功,
    // 属性版就会在"根本没进 ChainExhausted 分支"的输入上静默通过 —— 绿但无意义。
    if sub.chain_count() != 0 {
        handle.abort();
        return Err("事件数未把 ch-0 推进到 ChainExhausted(链仍在),属性用例会退化成假绿");
    }

    // 探针:换一个 route_key。listener 仍活着 ⇒ 必然再产出一个 Triggered。
    bus.publish(quota_event("ch-1")).await.unwrap();
    let alive = tokio::time::timeout(LIVENESS_BUDGET, async {
        loop {
            match rx.recv().await {
                Ok(NexusEvent::CsnSubstitutionTriggered { .. }) => break true,
                Ok(_) => continue,
                Err(_) => break false,
            }
        }
    })
    .await
    .unwrap_or(false);
    handle.abort();
    Ok(alive)
}

/// 在独立线程 + 自有 runtime 里跑一个场景,主线程只等 `recv_timeout`。
/// 返回 `Err(String)` 时字符串自带判红理由(含预算与通道数),调用方无需再拼。
///
/// WHY 线程化而不是直接在 `#[tokio::test]` 里 await:自锁持有的是**分片写锁**,
/// 而对该图做任何整体读(`DashMap::len()` 一类)都要取**全部分片读锁** —— 会把测试
/// 线程一起锁死。那样用例表现为「挂住」而非「失败」:既拿不到可归档的红色证据,
/// 又会把 CI 的整条 test job 拖死。线程若卡死,主线程超时即判红,进程照常退出。
fn run_exhaustion(channels: usize) -> Result<bool, String> {
    // 预算随深度线性放大:场景自身约 (channels+1) 个 TICK,再加调度余量
    let budget = Duration::from_millis(100 * (channels as u64 + 2) + 4_000);
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("独立 runtime 构建失败");
        // send 失败只可能是主线程已超时退出,无需处理
        let _ = tx.send(rt.block_on(exhaustion_scenario(channels)));
    });
    match rx.recv_timeout(budget) {
        // 已回传裁决 => 线程必然跑完,join 不会阻塞
        Ok(v) => {
            let _ = worker.join();
            v.map_err(String::from)
        }
        // 超时:线程可能正卡死,**不能** join(会把挂起传染给测试进程)
        Err(err) => Err(format!(
            "{} 通道(链深 {})的耗尽场景在 {:?} 内未回传裁决({:?}):降级链耗尽路径把 \
             listener 卡死了。修法见同 crate 的正确模板 \
             CsnSubstitutor::advance_degradation(块作用域出守卫后再 remove)。",
            channels,
            channels - 1,
            budget,
            err
        )),
    }
}

/// 生产路径回归守卫:`handle_quota_exhausted` 的 `ChainExhausted` 分支若在 `get_mut`
/// 守卫仍存活时对同一张 DashMap 调 `remove`,分片写锁不可重入即自锁;而 listener 是
/// **单个顺序 await 的 task**(`start_degradation_listener` 内一处 `recv` + 一处
/// `handle_quota_exhausted(..).await`),所以卡掉的不是一条链,而是之后所有通道的
/// 降级响应。WHY 用符号而非行号锚点:本文件修复过一次即整体位移,行号指针当场腐烂。
///
/// 机制层最小证明见 `tests/lock_reentrancy_mechanism.rs`;任意深度的属性版见本文件末。
#[test]
fn chain_exhaustion_does_not_wedge_degradation_listener() {
    match run_exhaustion(3) {
        Ok(true) => {}
        Ok(false) => panic!(
            "降级链耗尽后 listener 不再产出替代事件(探针超时):同图重入自锁,回归点在 \
             handle_quota_exhausted 的 ChainExhausted 分支;正确模板见 \
             CsnSubstitutor::advance_degradation(块作用域出守卫后再 remove)。"
        ),
        Err(premise) => panic!("{}", premise),
    }
}

/// 属性版:把上面这一条(depth=2 单点)一般化到**任意链深**。
///
/// WHY 值得加:修复的形态是"守卫作用域",它是否随候选数变化而漏掉某个分支,
/// 单点用例证不了。属性 = "对任意通道数 n≥3,把 ch-0 推到耗尽后再来一个键,
/// listener 仍产出替代事件"。案例数刻意压到 6(每例约 (n+1)×100ms),
/// 否则 256 例的默认值会把这条挂进 CI 长尾。
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(6))]

    #[test]
    fn any_chain_depth_exhaustion_keeps_listener_alive(channels in 3usize..=8usize) {
        let verdict = run_exhaustion(channels);
        prop_assert!(
            matches!(verdict, Ok(true)),
            "channels={} 时耗尽路径之后 listener 未恢复响应: {:?}",
            channels,
            verdict
        );
    }
}
