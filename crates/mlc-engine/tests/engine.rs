//! SubTask 1.16:MlcEngine 集成测试
//!
//! 验证 MlcEngine 统一接口的 store/recall/promote/demote 流程,
//! 以及 `MemoryMetricsReported`/`MemoryTiered` 事件正确发布。

use event_bus::{EventBus, NexusEvent};
use mlc_engine::{
    MemoryEntry, MemoryTier, MlcConfig, MlcEngine, MlcError, PatternSignature, ProceduralEntry,
};
use nexus_core::CLV;

/// 构造测试用记忆条目(无 CLV)
fn make_entry(id: &str, tier: MemoryTier) -> MemoryEntry {
    MemoryEntry::new(id, format!("content-{id}"), tier)
}

/// 构造测试用记忆条目(携带 CLV)
fn make_entry_with_clv(id: &str, tier: MemoryTier) -> MemoryEntry {
    let clv = CLV::zero();
    MemoryEntry::new(id, format!("content-{id}"), tier).with_clv(clv)
}

// ============================================================
// 基础存储与召回测试
// ============================================================

#[tokio::test]
async fn test_engine_store_and_recall_l0() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let entry = make_entry("m-1", MemoryTier::L0Working);
    engine.store(entry).await.unwrap();

    let recalled = engine.recall("m-1").await.unwrap();
    assert!(recalled.is_some());
    assert_eq!(recalled.unwrap().id.as_str(), "m-1");
}

#[tokio::test]
async fn test_engine_store_and_recall_l1() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let entry = make_entry("m-1", MemoryTier::L1Episodic);
    engine.store(entry).await.unwrap();

    let recalled = engine.recall("m-1").await.unwrap();
    assert!(recalled.is_some());
    assert_eq!(recalled.unwrap().id.as_str(), "m-1");
}

#[tokio::test]
async fn test_engine_store_and_recall_l2() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let entry = make_entry_with_clv("m-1", MemoryTier::L2Semantic);
    engine.store(entry).await.unwrap();

    let recalled = engine.recall("m-1").await.unwrap();
    assert!(recalled.is_some());
    assert_eq!(recalled.unwrap().id.as_str(), "m-1");
}

#[tokio::test]
async fn test_engine_store_l2_without_clv_returns_error() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let entry = make_entry("m-1", MemoryTier::L2Semantic);
    let result = engine.store(entry).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_engine_store_l3_memory_entry_returns_error() {
    // L3 不支持 MemoryEntry,应使用 store_procedural
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let entry = make_entry("m-1", MemoryTier::L3Procedural);
    let result = engine.store(entry).await;
    assert!(matches!(result, Err(MlcError::InvalidConfig(_))));
}

#[tokio::test]
async fn test_engine_recall_nonexistent_returns_none() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let recalled = engine.recall("nonexistent").await.unwrap();
    assert!(recalled.is_none());
}

#[tokio::test]
async fn test_engine_recall_cross_layer() {
    // 验证跨层查找:L0/L1/L2 各存一个条目,recall 应能找到所有
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    engine
        .store(make_entry("m-l0", MemoryTier::L0Working))
        .await
        .unwrap();
    engine
        .store(make_entry("m-l1", MemoryTier::L1Episodic))
        .await
        .unwrap();
    engine
        .store(make_entry_with_clv("m-l2", MemoryTier::L2Semantic))
        .await
        .unwrap();

    assert!(engine.recall("m-l0").await.unwrap().is_some());
    assert!(engine.recall("m-l1").await.unwrap().is_some());
    assert!(engine.recall("m-l2").await.unwrap().is_some());
    assert!(engine.recall("m-nonexistent").await.unwrap().is_none());
}

// ============================================================
// CLV 召回测试
// ============================================================

#[tokio::test]
async fn test_engine_recall_by_clv() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let mut v = vec![0.0_f32; CLV::DIMENSION];
    v[0] = 1.0;
    let query = CLV::from_vec(v).unwrap();

    engine
        .store(make_entry_with_clv("m-1", MemoryTier::L2Semantic))
        .await
        .unwrap();

    let results = engine.recall_by_clv(&query, 10).await.unwrap();
    assert!(!results.is_empty());
}

// ============================================================
// promote/demote 迁移测试
// ============================================================

#[tokio::test]
async fn test_engine_promote_l1_to_l0() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    engine
        .store(make_entry("m-1", MemoryTier::L1Episodic))
        .await
        .unwrap();
    assert_eq!(engine.l1().len().unwrap(), 1);
    assert_eq!(engine.l0().len(), 0);

    engine
        .promote("m-1", MemoryTier::L1Episodic, MemoryTier::L0Working)
        .await
        .unwrap();

    assert_eq!(engine.l1().len().unwrap(), 0);
    assert_eq!(engine.l0().len(), 1);

    let recalled = engine.recall("m-1").await.unwrap().unwrap();
    assert_eq!(recalled.tier, MemoryTier::L0Working);
}

#[tokio::test]
async fn test_engine_demote_l0_to_l1() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    engine
        .store(make_entry("m-1", MemoryTier::L0Working))
        .await
        .unwrap();

    engine
        .demote("m-1", MemoryTier::L0Working, MemoryTier::L1Episodic)
        .await
        .unwrap();

    assert_eq!(engine.l0().len(), 0);
    assert_eq!(engine.l1().len().unwrap(), 1);
}

#[tokio::test]
async fn test_engine_promote_nonexistent_returns_error() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let result = engine
        .promote("nonexistent", MemoryTier::L1Episodic, MemoryTier::L0Working)
        .await;
    assert!(matches!(result, Err(MlcError::EntryNotFound(_))));
}

#[tokio::test]
async fn test_engine_promote_l2_to_l0() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    engine
        .store(make_entry_with_clv("m-1", MemoryTier::L2Semantic))
        .await
        .unwrap();
    assert_eq!(engine.l2().len().unwrap(), 1);

    engine
        .promote("m-1", MemoryTier::L2Semantic, MemoryTier::L0Working)
        .await
        .unwrap();

    assert_eq!(engine.l2().len().unwrap(), 0);
    assert_eq!(engine.l0().len(), 1);
}

#[tokio::test]
async fn test_engine_demote_l0_to_l2_with_clv() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    // L0 不强制 CLV,但降级到 L2 需要 CLV
    // 此处先存储带 CLV 的条目到 L0
    let entry = make_entry_with_clv("m-1", MemoryTier::L0Working);
    engine.store(entry).await.unwrap();

    engine
        .demote("m-1", MemoryTier::L0Working, MemoryTier::L2Semantic)
        .await
        .unwrap();

    assert_eq!(engine.l0().len(), 0);
    assert_eq!(engine.l2().len().unwrap(), 1);
}

// ============================================================
// L3 程序记忆测试
// ============================================================

#[tokio::test]
async fn test_engine_store_procedural_and_match() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let sig = PatternSignature::new(vec!["tool_a".into()], "hash-1");
    let entry = ProceduralEntry::new(sig.clone(), "output-1");
    engine.store_procedural(entry).await.unwrap();

    let matched = engine.match_procedural(&sig).await.unwrap();
    assert!(matched.is_some());
    assert_eq!(matched.unwrap().output, "output-1");
}

// ============================================================
// 事件发布测试
// ============================================================

#[tokio::test]
async fn test_engine_memory_metrics_reported_event() {
    // 验证达到阈值时发布 MemoryMetricsReported 事件
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let config = MlcConfig::default().with_metrics_interval(3);
    let engine = MlcEngine::new_in_memory_with_config(config, bus).unwrap();

    // 执行 3 次存储操作,应触发指标上报
    engine
        .store(make_entry("m-1", MemoryTier::L0Working))
        .await
        .unwrap();
    engine
        .store(make_entry("m-2", MemoryTier::L0Working))
        .await
        .unwrap();
    engine
        .store(make_entry("m-3", MemoryTier::L0Working))
        .await
        .unwrap();

    // 应收到 MemoryMetricsReported 事件
    let event = rx.recv().await.unwrap();
    match event {
        NexusEvent::MemoryMetricsReported {
            hit_rate,
            evictions,
            ..
        } => {
            assert!((0.0..=1.0).contains(&hit_rate));
            assert_eq!(evictions, 0);
        }
        other => panic!("expected MemoryMetricsReported, got {other:?}"),
    }
}

#[tokio::test]
async fn test_engine_memory_tiered_event_on_promote() {
    // 验证层级迁移时发布 MemoryTiered 事件
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    engine
        .store(make_entry("m-1", MemoryTier::L1Episodic))
        .await
        .unwrap();
    engine
        .promote("m-1", MemoryTier::L1Episodic, MemoryTier::L0Working)
        .await
        .unwrap();

    // 应收到 MemoryTiered 事件
    let event = rx.recv().await.unwrap();
    match event {
        NexusEvent::MemoryTiered {
            tier,
            item_count,
            memory_id,
            ..
        } => {
            assert_eq!(tier, "L0");
            assert_eq!(item_count, 1);
            // SubTask 17.4:单条迁移应填充 memory_id
            assert_eq!(
                memory_id,
                Some("m-1".to_string()),
                "单条迁移的 memory_id 应为被迁移条目的 ID"
            );
        }
        other => panic!("expected MemoryTiered, got {other:?}"),
    }
}

#[tokio::test]
async fn test_engine_report_metrics_manual() {
    // 验证手动上报指标
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    engine.report_metrics().await.unwrap();

    let event = rx.recv().await.unwrap();
    assert!(matches!(event, NexusEvent::MemoryMetricsReported { .. }));
}

#[tokio::test]
async fn test_engine_hit_rate_calculation() {
    // 验证命中率计算:命中次数 / (命中次数 + 未命中次数)
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let config = MlcConfig::default().with_metrics_interval(5);
    let engine = MlcEngine::new_in_memory_with_config(config, bus).unwrap();

    // 存储 1 个条目
    engine
        .store(make_entry("m-1", MemoryTier::L0Working))
        .await
        .unwrap();

    // 命中 1 次
    engine.recall("m-1").await.unwrap();
    // 未命中 1 次
    engine.recall("nonexistent").await.unwrap();

    // 继续操作直到触发指标上报(共 5 次 store,达到阈值 5)
    for i in 0..4 {
        engine
            .store(make_entry(&format!("m-{i}"), MemoryTier::L0Working))
            .await
            .unwrap();
    }

    // 应收到 MemoryMetricsReported 事件
    let event = rx.recv().await.unwrap();
    if let NexusEvent::MemoryMetricsReported { hit_rate, .. } = event {
        // hit_rate = hits / (hits + misses)
        // 至少有 1 次命中和 1 次未命中
        assert!(hit_rate > 0.0 && hit_rate < 1.0);
    }
}

// ============================================================
// 配置测试
// ============================================================

#[tokio::test]
async fn test_engine_new_in_memory() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    // 默认配置:L0=64, L1=1024, L2=4096
    assert_eq!(engine.config().l0_capacity, 64);
    assert_eq!(engine.config().l1_capacity, 1024);
    assert_eq!(engine.config().l2_capacity, 4096);
}

#[tokio::test]
async fn test_engine_new_in_memory_with_config() {
    let bus = EventBus::new();
    let config = MlcConfig::default()
        .with_l0_capacity(32)
        .with_l1_capacity(512)
        .with_l2_capacity(2048)
        .with_metrics_interval(10);
    let engine = MlcEngine::new_in_memory_with_config(config, bus).unwrap();

    assert_eq!(engine.config().l0_capacity, 32);
    assert_eq!(engine.config().l1_capacity, 512);
    assert_eq!(engine.config().l2_capacity, 2048);
    assert_eq!(engine.config().metrics_report_interval, 10);
}

// ============================================================
// 端到端流程测试
// ============================================================

#[tokio::test]
async fn test_engine_e2e_store_recall_promote_demote() {
    // 端到端流程:store → recall → promote → recall → demote → recall
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    // 1. 存储到 L1
    engine
        .store(make_entry("m-1", MemoryTier::L1Episodic))
        .await
        .unwrap();
    assert!(engine.recall("m-1").await.unwrap().is_some());

    // 2. 提升到 L0
    engine
        .promote("m-1", MemoryTier::L1Episodic, MemoryTier::L0Working)
        .await
        .unwrap();
    assert_eq!(engine.l0().len(), 1);
    assert_eq!(engine.l1().len().unwrap(), 0);
    assert!(engine.recall("m-1").await.unwrap().is_some());

    // 3. 降级回 L1
    engine
        .demote("m-1", MemoryTier::L0Working, MemoryTier::L1Episodic)
        .await
        .unwrap();
    assert_eq!(engine.l0().len(), 0);
    assert_eq!(engine.l1().len().unwrap(), 1);
    assert!(engine.recall("m-1").await.unwrap().is_some());
}

#[tokio::test]
async fn test_engine_l0_eviction_triggers_on_overflow() {
    // 验证 L0 容量满时触发 LRU 驱逐
    let bus = EventBus::new();
    let config = MlcConfig::default().with_l0_capacity(3);
    let engine = MlcEngine::new_in_memory_with_config(config, bus).unwrap();

    // 插入 4 个条目,应驱逐 1 个
    engine
        .store(make_entry("m-1", MemoryTier::L0Working))
        .await
        .unwrap();
    engine
        .store(make_entry("m-2", MemoryTier::L0Working))
        .await
        .unwrap();
    engine
        .store(make_entry("m-3", MemoryTier::L0Working))
        .await
        .unwrap();
    let evicted = engine
        .store(make_entry("m-4", MemoryTier::L0Working))
        .await
        .unwrap();
    assert!(evicted.is_some());
    assert_eq!(engine.l0().len(), 3); // 容量不变
}

/// SubTask 12.6:验证 migrate 操作顺序安全性 — 目标层写入失败时源层仍保留条目。
///
/// WHY:原实现先从源层删除再写入目标层,中间失败时数据丢失。
/// 改为"先写入目标层 → 确认成功 → 再从源层删除"后,目标层写入失败时源层条目保留。
///
/// 测试方案:L1 中的条目(不带 CLV)promote 到 L2 会失败(L2 要求 CLV),
/// 断言 L1 仍保留条目(无数据丢失)。
#[tokio::test]
async fn test_migrate_order_safety() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    // 在 L1 存储一个不带 CLV 的条目
    let entry = make_entry("m-1", MemoryTier::L1Episodic);
    engine.store(entry).await.unwrap();
    assert_eq!(engine.l1().len().unwrap(), 1);

    // 尝试 promote 到 L2(应失败,因为 L2 要求 CLV)
    let result = engine
        .promote("m-1", MemoryTier::L1Episodic, MemoryTier::L2Semantic)
        .await;
    assert!(result.is_err(), "L2 要求 CLV,promote 应失败");

    // 验证源层(L1)仍保留条目(无数据丢失)
    assert_eq!(
        engine.l1().len().unwrap(),
        1,
        "目标层写入失败时,源层应保留条目(无数据丢失)"
    );
    let recalled = engine.recall("m-1").await.unwrap();
    assert!(recalled.is_some(), "源层条目应仍可召回");
    assert_eq!(recalled.unwrap().id.as_str(), "m-1");
}

// ============================================================
// SubTask 18.1:并发迁移安全测试
// ============================================================

/// SubTask 18.1:验证 10 线程并发 migrate 同一 MemoryId 时无数据重复。
///
/// WHY:无迁移锁时,fetch_from_tier → insert → remove_from_tier 过程存在 TOCTOU 窗口,
/// 多线程并发迁移同一 ID 可能导致目标层出现重复条目或源层数据丢失。
/// 引入 `migration_locks` 条目级锁后,同一 ID 的迁移串行化,消除竞态。
///
/// 断言:
/// - 目标层(L0)条目数 ≤ 1(无数据重复)
/// - 源层(L1)条目数 ≤ 1(无数据残留)
/// - 至少 1 个线程成功迁移
#[tokio::test]
async fn test_concurrent_migrate_same_id_no_duplication() {
    use std::sync::Arc;
    use tokio::task::JoinSet;

    let bus = EventBus::new();
    let engine = Arc::new(MlcEngine::new_in_memory(bus).unwrap());

    // 在 L1 存储条目
    engine
        .store(make_entry("m-concurrent", MemoryTier::L1Episodic))
        .await
        .unwrap();
    assert_eq!(engine.l1().len().unwrap(), 1);

    // 10 线程同时 migrate 同一 MemoryId(L1 → L0)
    let mut tasks = JoinSet::new();
    for _ in 0..10 {
        let engine = engine.clone();
        tasks.spawn(async move {
            engine
                .promote(
                    "m-concurrent",
                    MemoryTier::L1Episodic,
                    MemoryTier::L0Working,
                )
                .await
        });
    }

    // 收集结果:统计成功与失败次数
    let mut success_count = 0u32;
    let mut not_found_count = 0u32;
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(())) => success_count += 1,
            Ok(Err(MlcError::EntryNotFound(_))) => not_found_count += 1,
            Ok(Err(e)) => panic!("并发迁移出现非预期错误: {e:?}"),
            Err(e) => panic!("任务 join 失败: {e}"),
        }
    }

    // 断言:目标层(L0)条目数 ≤ 1(无数据重复)
    let l0_count = engine.l0().len();
    assert!(
        l0_count <= 1,
        "目标层 L0 条目数应 ≤ 1(无数据重复),实际 = {l0_count}"
    );

    // 断言:恰好 1 个线程成功迁移(第一个获取锁的线程)
    assert_eq!(
        success_count, 1,
        "应有恰好 1 个线程成功迁移,实际成功 = {success_count}"
    );

    // 断言:其余 9 个线程收到 EntryNotFound(条目已被迁移)
    assert_eq!(
        not_found_count, 9,
        "应有 9 个线程收到 EntryNotFound,实际 = {not_found_count}"
    );

    // 断言:源层(L1)条目数 ≤ 1(被迁移后应删除)
    let l1_count = engine.l1().len().unwrap();
    assert!(l1_count <= 1, "源层 L1 条目数应 ≤ 1,实际 = {l1_count}");
    assert_eq!(
        l1_count, 0,
        "源层 L1 应为空(条目已迁移到 L0),实际 = {l1_count}"
    );
}
