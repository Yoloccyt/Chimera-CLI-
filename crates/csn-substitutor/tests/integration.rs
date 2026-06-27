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

#![forbid(unsafe_code)]

use csn_substitutor::{
    CapabilityDescriptor, CapabilityMetadata, CsnConfig, CsnError, CsnSubstitutor,
    SubstitutionCandidate,
};
use event_bus::{EventBus, EventMetadata, EventSeverity, NexusEvent};
use std::time::{Duration, Instant};

// === 辅助函数 ===

/// 创建 50 维向量,索引 i 处为 1.0,其余为 0.0(用于正交场景)
fn make_unit_vector(dim: usize, idx: usize) -> Vec<f32> {
    let mut v = vec![0.0; dim];
    if idx < dim {
        v[idx] = 1.0;
    }
    v
}

/// 创建 50 维全 1.0 向量(用于高相似度场景)
fn make_uniform_vector(dim: usize, value: f32) -> Vec<f32> {
    vec![value; dim]
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
            ("cap-2", make_uniform_vector(50, 0.99)), // 与 cap-1 极相似
            ("cap-3", make_uniform_vector(50, 0.9)),
            ("cap-4", make_unit_vector(50, 0)), // 与 cap-1 正交
        ],
    );

    let candidates = sub.find_substitutes("cap-1", 3);
    assert_eq!(candidates.len(), 3, "应返回 Top-3 候选");

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
    let event = rx.recv().await.expect("应收到事件");
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

    let event = rx.recv().await.expect("应收到事件");
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
        let event = rx.recv().await.expect("应收到事件");
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
