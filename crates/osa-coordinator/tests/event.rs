//! OmniSparseMasksComputed 事件集成测试 — 验证事件正确发布
//!
//! 对应 SubTask 4.12:验证事件正确发布(通过 EventBus 订阅验证),
//! mask_hash 与 sparsity 字段正确
//!
//! V1 违规修正验证:OSA 不持有 HCW 引用,仅通过 EventBus 传递 context_mask

use event_bus::{EventBus, NexusEvent};
use osa_coordinator::{
    AffectedScope, FileId, MemoryId, OmniSparseCoordinator, OperationId, RiskLevel, TaskId,
    TaskProfile, TaskType, TimePressure, ToolId,
};
use std::time::Duration;

/// 构造测试用 TaskProfile
fn make_profile(complexity: f32) -> TaskProfile {
    TaskProfile {
        task_id: TaskId::new(format!("task-{complexity}")),
        task_type: TaskType::Read,
        complexity_score: complexity,
        risk_level: RiskLevel::Medium,
        time_pressure: TimePressure::Low,
        affected_scope: AffectedScope::Local,
        available_tools: (0..50).map(|i| ToolId::new(format!("tool-{i}"))).collect(),
        available_files: (0..200).map(|i| FileId::new(format!("file-{i}"))).collect(),
        available_memories: (0..50).map(|i| MemoryId::new(format!("mem-{i}"))).collect(),
        recent_operations: (0..100)
            .map(|i| OperationId::new(format!("op-{i}")))
            .collect(),
        active_tasks: (0..10).map(|i| TaskId::new(format!("task-{i}"))).collect(),
    }
}

#[tokio::test]
async fn test_omni_sparse_masks_computed_event_published() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.5);

    // 计算掩码(内部发布事件)
    let masks = coord
        .compute_all_masks(&profile)
        .await
        .expect("掩码计算失败");

    // 接收事件
    let event = rx
        .recv_timeout(Duration::from_secs(1))
        .await
        .expect("接收事件超时");

    match event {
        NexusEvent::OmniSparseMasksComputed {
            mask_hash,
            sparsity,
            context_mask,
            ..
        } => {
            // mask_hash 应为非空字符串(SHA-256 hex = 64 字符)
            assert!(!mask_hash.is_empty(), "mask_hash 不应为空");
            assert_eq!(mask_hash.len(), 64, "SHA-256 hex 应为 64 字符");
            // sparsity 应在 [0.0, 1.0] 范围内
            assert!((0.0..=1.0).contains(&sparsity), "sparsity 应在 [0.0, 1.0]");
            // mask_hash 应与掩码自身计算的哈希一致
            let expected_hash = masks.mask_hash();
            assert_eq!(mask_hash, expected_hash, "事件中的 mask_hash 应与掩码一致");
            // sparsity 应与掩码的平均稀疏度一致
            let expected_sparsity = masks.average_sparsity();
            assert!(
                (sparsity - expected_sparsity).abs() < 1e-6,
                "事件中的 sparsity 应与掩码一致"
            );
            // SubTask 14.3:验证 context_mask 字段非空且与掩码一致
            // complexity=0.5 → Complex 档位,context 保留 100 文件
            assert!(!context_mask.is_empty(), "context_mask 不应为空");
            assert_eq!(
                context_mask.len(),
                masks.context.active_count(),
                "context_mask 长度应与掩码活跃数一致"
            );
            // 验证 context_mask 内容与 masks.context.active_ids 一致(字符串形式)
            let expected: Vec<String> = masks
                .context
                .active_ids
                .iter()
                .map(|f| f.to_string())
                .collect();
            assert_eq!(
                context_mask, expected,
                "context_mask 内容应与掩码活跃 ID 一致"
            );
        }
        other => panic!("期望 OmniSparseMasksComputed 事件,收到 {other:?}"),
    }
}

#[tokio::test]
async fn test_event_metadata_source_is_osa_coordinator() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.3);

    coord.compute_all_masks(&profile).await.unwrap();

    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    assert_eq!(
        event.metadata().source,
        "osa-coordinator",
        "事件 source 应为 osa-coordinator"
    );
}

#[tokio::test]
async fn test_different_profiles_produce_different_mask_hashes() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let coord = OmniSparseCoordinator::new(bus);

    // 两个不同复杂度的 profile
    let profile1 = make_profile(0.1);
    let profile2 = make_profile(0.9);

    let masks1 = coord.compute_all_masks(&profile1).await.unwrap();
    let masks2 = coord.compute_all_masks(&profile2).await.unwrap();

    // 接收两个事件
    let event1 = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    let event2 = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();

    let hash1 = match event1 {
        NexusEvent::OmniSparseMasksComputed { mask_hash, .. } => mask_hash,
        _ => panic!("期望 OmniSparseMasksComputed 事件"),
    };
    let hash2 = match event2 {
        NexusEvent::OmniSparseMasksComputed { mask_hash, .. } => mask_hash,
        _ => panic!("期望 OmniSparseMasksComputed 事件"),
    };

    // 不同复杂度应产生不同掩码哈希
    assert_ne!(hash1, hash2, "不同复杂度的掩码哈希应不同");
    assert_ne!(masks1.mask_hash(), masks2.mask_hash());
}

#[tokio::test]
async fn test_same_profile_produces_same_mask_hash() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let coord = OmniSparseCoordinator::new(bus);

    let profile1 = make_profile(0.5);
    let profile2 = make_profile(0.5);

    let masks1 = coord.compute_all_masks(&profile1).await.unwrap();
    let masks2 = coord.compute_all_masks(&profile2).await.unwrap();

    // 接收两个事件
    let _event1 = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    let _event2 = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();

    // 相同 profile 应产生相同掩码哈希
    // mask_hash() 返回 &str(构造时预计算),无需 unwrap
    assert_eq!(
        masks1.mask_hash(),
        masks2.mask_hash(),
        "相同 profile 的掩码哈希应相同"
    );
}

#[tokio::test]
async fn test_invalid_complexity_returns_error() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let mut profile = make_profile(0.5);
    profile.complexity_score = 1.5; // 非法值

    let result = coord.compute_all_masks(&profile).await;
    assert!(result.is_err(), "非法 complexity_score 应返回错误");
    let err = result.unwrap_err();
    assert!(
        matches!(err, osa_coordinator::OsaError::InvalidTaskProfile(_)),
        "应为 InvalidTaskProfile 错误"
    );
}

#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_event_published_within_performance_budget() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.5);

    // 性能基准:OSA 掩码计算 < 10ms
    let start = std::time::Instant::now();
    coord.compute_all_masks(&profile).await.unwrap();
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 10,
        "OSA 掩码计算应 < 10ms,实际 {:?}",
        elapsed
    );

    // 确保事件也能在合理时间内接收
    let _event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
}

#[tokio::test]
async fn test_v1_violation_fix_no_direct_hcw_dependency() {
    // V1 违规修正验证:OSA 不持有 HCW 引用,仅通过 EventBus 传递 context_mask
    // 此测试验证 OSA 可以独立工作,无需 HCW 存在
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus.clone());

    // 验证 OSA 不依赖 HCW(无 HCW 订阅也能正常计算掩码并发布事件)
    let profile = make_profile(0.5);
    let masks = coord.compute_all_masks(&profile).await.unwrap();

    // 验证 context_mask 已正确计算(将通过事件传递给 HCW)
    assert!(masks.context.active_count() > 0, "context_mask 应有活跃项");
    assert_eq!(bus.subscriber_count(), 0, "OSA 不应依赖 HCW 订阅才能工作");
}

/// SubTask 14.3:验证事件携带的 context_mask 与掩码活跃 ID 一致
///
/// 不同复杂度档位产生不同数量的 context 活跃文件,
/// 事件中的 context_mask 应与 masks.context.active_ids(字符串形式)完全一致
#[tokio::test]
async fn test_event_context_mask_matches_masks() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let coord = OmniSparseCoordinator::new(bus);

    // Simple 档位(complexity=0.1)→ context 保留 1 文件
    let profile = make_profile(0.1);
    let masks = coord.compute_all_masks(&profile).await.unwrap();
    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();

    let context_mask = match event {
        NexusEvent::OmniSparseMasksComputed { context_mask, .. } => context_mask,
        _ => panic!("期望 OmniSparseMasksComputed 事件"),
    };
    // Simple 档位应保留 1 文件
    assert_eq!(
        context_mask.len(),
        1,
        "Simple 档位 context_mask 应有 1 个文件"
    );
    let expected: Vec<String> = masks
        .context
        .active_ids
        .iter()
        .map(|f| f.to_string())
        .collect();
    assert_eq!(context_mask, expected, "context_mask 应与掩码活跃 ID 一致");
}

/// SubTask 14.3:验证 UltraComplex 档位 context_mask 携带大量文件 ID
#[tokio::test]
async fn test_event_context_mask_ultra_complex_band() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let coord = OmniSparseCoordinator::new(bus);

    // UltraComplex 档位(complexity=0.9)→ context 保留 1000 文件
    let profile = make_profile(0.9);
    let masks = coord.compute_all_masks(&profile).await.unwrap();
    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();

    let context_mask = match event {
        NexusEvent::OmniSparseMasksComputed { context_mask, .. } => context_mask,
        _ => panic!("期望 OmniSparseMasksComputed 事件"),
    };
    // UltraComplex 档位应保留 1000 文件(profile 中有 200 个文件,全保留)
    // WHY:make_profile 提供 200 文件,UltraComplex 档位 context_scope=1000,
    // select_top_k 当 k >= total 时返回全掩码,故 active_count = 200
    assert_eq!(
        context_mask.len(),
        masks.context.active_count(),
        "context_mask 长度应与掩码活跃数一致"
    );
    assert!(
        !context_mask.is_empty(),
        "UltraComplex 档位 context_mask 不应为空"
    );
}
