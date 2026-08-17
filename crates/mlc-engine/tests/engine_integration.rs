//! MlcEngine 集成测试 — 从 src/engine.rs 内联测试模块外移(L2-P2-1)
//!
//! 外移说明:原 `#[cfg(test)] mod tests` 混在生产文件(710 行,占文件 44%),
//! 外移后 engine.rs 仅保留生产代码(约 900 行)。测试仅使用公共 API:
//! - 存储/召回/迁移(demote 走 L0 assert_archive_monotonicity 校验)
//! - 事件发布(MemoryMetricsReported / MemoryTiered)
//! - TemporalMeta 召回过滤(P3-W11.1.2)

use event_bus::{EventBus, NexusEvent};
use mlc_engine::types::{PatternSignature, ProceduralEntry};
use mlc_engine::{MemoryEntry, MemoryTier, MlcConfig, MlcEngine, MlcError};
use nexus_core::CLV;

fn make_entry(id: &str, tier: MemoryTier) -> MemoryEntry {
    MemoryEntry::new(id, format!("content-{id}"), tier)
}

fn make_entry_with_clv(id: &str, tier: MemoryTier) -> MemoryEntry {
    let clv = CLV::zero();
    MemoryEntry::new(id, format!("content-{id}"), tier).with_clv(clv)
}

#[tokio::test]
async fn test_store_and_recall_l0() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let entry = make_entry("m-1", MemoryTier::L0Working);
    engine.store(entry).await.unwrap();

    let recalled = engine.recall("m-1").await.unwrap();
    assert!(recalled.is_some());
    assert_eq!(recalled.unwrap().id.as_str(), "m-1");
}

#[tokio::test]
async fn test_store_and_recall_l1() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let entry = make_entry("m-1", MemoryTier::L1Episodic);
    engine.store(entry).await.unwrap();

    let recalled = engine.recall("m-1").await.unwrap();
    assert!(recalled.is_some());
    assert_eq!(recalled.unwrap().id.as_str(), "m-1");
}

#[tokio::test]
async fn test_store_and_recall_l2() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let entry = make_entry_with_clv("m-1", MemoryTier::L2Semantic);
    engine.store(entry).await.unwrap();

    let recalled = engine.recall("m-1").await.unwrap();
    assert!(recalled.is_some());
    assert_eq!(recalled.unwrap().id.as_str(), "m-1");
}

#[tokio::test]
async fn test_store_l2_without_clv_returns_error() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let entry = make_entry("m-1", MemoryTier::L2Semantic);
    let result = engine.store(entry).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_store_l3_memory_entry_returns_error() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let entry = make_entry("m-1", MemoryTier::L3Procedural);
    let result = engine.store(entry).await;
    assert!(matches!(result, Err(MlcError::InvalidConfig(_))));
}

#[tokio::test]
async fn test_recall_nonexistent_returns_none() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let recalled = engine.recall("nonexistent").await.unwrap();
    assert!(recalled.is_none());
}

#[tokio::test]
async fn test_recall_cross_layer() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    // 在不同层存储不同条目
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

    // 跨层查找应找到所有
    assert!(engine.recall("m-l0").await.unwrap().is_some());
    assert!(engine.recall("m-l1").await.unwrap().is_some());
    assert!(engine.recall("m-l2").await.unwrap().is_some());
}

#[tokio::test]
async fn test_recall_by_clv() {
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

#[tokio::test]
async fn test_promote_l1_to_l0() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    // 存储到 L1
    engine
        .store(make_entry("m-1", MemoryTier::L1Episodic))
        .await
        .unwrap();
    assert!(engine.l1().len().unwrap() == 1);
    assert!(engine.l0().is_empty());

    // 提升到 L0
    engine
        .promote("m-1", MemoryTier::L1Episodic, MemoryTier::L0Working)
        .await
        .unwrap();

    // L1 应为空,L0 应有 1 个
    assert_eq!(engine.l1().len().unwrap(), 0);
    assert_eq!(engine.l0().len(), 1);

    // 验证条目存在
    let recalled = engine.recall("m-1").await.unwrap().unwrap();
    assert_eq!(recalled.tier, MemoryTier::L0Working);
}

#[tokio::test]
async fn test_demote_l0_to_l1() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    // 存储到 L0
    engine
        .store(make_entry("m-1", MemoryTier::L0Working))
        .await
        .unwrap();

    // 降级到 L1
    engine
        .demote("m-1", MemoryTier::L0Working, MemoryTier::L1Episodic)
        .await
        .unwrap();

    assert_eq!(engine.l0().len(), 0);
    assert_eq!(engine.l1().len().unwrap(), 1);
}

#[tokio::test]
async fn test_promote_nonexistent_returns_error() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let result = engine
        .promote("nonexistent", MemoryTier::L1Episodic, MemoryTier::L0Working)
        .await;
    assert!(matches!(result, Err(MlcError::EntryNotFound(_))));
}

#[tokio::test]
async fn test_store_procedural_and_match() {
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let sig = PatternSignature::new(vec!["tool_a".into()], "hash-1");
    let entry = ProceduralEntry::new(sig.clone(), "output-1");
    engine.store_procedural(entry).await.unwrap();

    let matched = engine.match_procedural(&sig).await.unwrap();
    assert!(matched.is_some());
    assert_eq!(matched.unwrap().output, "output-1");
}

#[tokio::test]
async fn test_memory_metrics_reported_event() {
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
let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("5s 内未收到事件(资源竞争或事件丢失)")
        .unwrap();
    match event {
        NexusEvent::MemoryMetricsReported {
            hit_rate,
            evictions,
            ..
        } => {
            // hit_rate 可能为 0.0(仅 store 未 recall)
            assert!((0.0..=1.0).contains(&hit_rate));
            assert_eq!(evictions, 0);
        }
        other => panic!("expected MemoryMetricsReported, got {other:?}"),
    }
}

#[tokio::test]
async fn test_memory_tiered_event_on_promote() {
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
let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("5s 内未收到事件(资源竞争或事件丢失)")
        .unwrap();
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
async fn test_report_metrics_manual() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    // 手动上报指标
    engine.report_metrics().await.unwrap();

let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("5s 内未收到事件(资源竞争或事件丢失)")
        .unwrap();
    assert!(matches!(event, NexusEvent::MemoryMetricsReported { .. }));
}

#[tokio::test]
async fn test_hit_rate_calculation() {
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
let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("5s 内未收到事件(资源竞争或事件丢失)")
        .unwrap();
    if let NexusEvent::MemoryMetricsReported { hit_rate, .. } = event {
        // hit_rate = hits / (hits + misses)
        // 至少有 1 次命中和 1 次未命中
        assert!(hit_rate > 0.0 && hit_rate < 1.0);
    }
}

// ============================================================
// P3-W11.1.2 D12 修复验收测试(spec.md:293-295 召回按 TransitionType 过滤)
// ============================================================

/// 测试辅助:创建带 TemporalMeta 的条目
fn make_entry_with_temporal(
    id: &str,
    tier: MemoryTier,
    meta: nexus_contracts::TemporalMeta,
) -> MemoryEntry {
    MemoryEntry::new(id, format!("content-{id}"), tier).with_temporal_meta(meta)
}

#[tokio::test]
async fn test_p3_w11_1_2_recall_current_returns_current_entry() {
    // P3-W11.1.2: recall_current 返回 Current 状态条目(含 None 向后兼容)
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    // Current 条目(默认 None temporal_meta,视为 Current)
    engine
        .store(make_entry("m-current", MemoryTier::L0Working))
        .await
        .unwrap();

    // recall_current 应返回条目
    let result = engine.recall_current("m-current").await.unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap().id.as_str(), "m-current");
}

#[tokio::test]
async fn test_p3_w11_1_2_recall_current_filters_historical() {
    // P3-W11.1.2: recall_current 对 Historical 条目返回 None
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let historical_meta = nexus_contracts::TemporalMeta {
        valid_from: 1000,
        valid_until: Some(2000),
        transition_type: nexus_contracts::TransitionType::Historical,
        confidence: 0.5,
    };
    engine
        .store(make_entry_with_temporal(
            "m-historical",
            MemoryTier::L0Working,
            historical_meta,
        ))
        .await
        .unwrap();

    // recall_current 对 Historical 返回 None
    let result = engine.recall_current("m-historical").await.unwrap();
    assert!(result.is_none());

    // 但 recall(向后兼容)仍返回条目
    let result = engine.recall("m-historical").await.unwrap();
    assert!(result.is_some());
}

#[tokio::test]
async fn test_p3_w11_1_2_recall_current_filters_transition() {
    // P3-W11.1.2: recall_current 对 Transition 条目返回 None
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let transition_meta = nexus_contracts::TemporalMeta {
        valid_from: 1000,
        valid_until: Some(2000),
        transition_type: nexus_contracts::TransitionType::Transition,
        confidence: 0.3,
    };
    engine
        .store(make_entry_with_temporal(
            "m-transition",
            MemoryTier::L0Working,
            transition_meta,
        ))
        .await
        .unwrap();

    // recall_current 对 Transition 返回 None
    let result = engine.recall_current("m-transition").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_p3_w11_1_2_recall_historical_returns_historical_entry() {
    // P3-W11.1.2: recall_historical 返回 Historical 状态条目
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let historical_meta = nexus_contracts::TemporalMeta {
        valid_from: 1000,
        valid_until: Some(2000),
        transition_type: nexus_contracts::TransitionType::Historical,
        confidence: 0.5,
    };
    engine
        .store(make_entry_with_temporal(
            "m-historical",
            MemoryTier::L1Episodic,
            historical_meta,
        ))
        .await
        .unwrap();

    // recall_historical 应返回条目
    let result = engine.recall_historical("m-historical").await.unwrap();
    assert!(result.is_some());
    let entry = result.unwrap();
    assert_eq!(entry.id.as_str(), "m-historical");
    assert!(entry.is_historical());
}

#[tokio::test]
async fn test_p3_w11_1_2_recall_historical_filters_current_and_transition() {
    // P3-W11.1.2: recall_historical 对 Current/Transition 返回 None
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    // Current 条目
    engine
        .store(make_entry("m-current", MemoryTier::L0Working))
        .await
        .unwrap();

    // Transition 条目
    let transition_meta = nexus_contracts::TemporalMeta {
        valid_from: 1000,
        valid_until: Some(2000),
        transition_type: nexus_contracts::TransitionType::Transition,
        confidence: 0.3,
    };
    engine
        .store(make_entry_with_temporal(
            "m-transition",
            MemoryTier::L0Working,
            transition_meta,
        ))
        .await
        .unwrap();

    // recall_historical 对 Current/Transition 返回 None
    assert!(engine
        .recall_historical("m-current")
        .await
        .unwrap()
        .is_none());
    assert!(engine
        .recall_historical("m-transition")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn test_p3_w11_1_2_recall_transition_returns_entry_with_evidence() {
    // P3-W11.1.2: recall_transition 返回 Transition 条目 + 时间证据包(TemporalMeta)
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    let transition_meta = nexus_contracts::TemporalMeta {
        valid_from: 1000,
        valid_until: Some(2000),
        transition_type: nexus_contracts::TransitionType::Transition,
        confidence: 0.4, // 降置信度
    };
    engine
        .store(make_entry_with_temporal(
            "m-transition",
            MemoryTier::L0Working,
            transition_meta,
        ))
        .await
        .unwrap();

    // recall_transition 应返回 (entry, TemporalMeta)
    let result = engine.recall_transition("m-transition").await.unwrap();
    assert!(result.is_some());
    let (entry, meta) = result.unwrap();
    assert_eq!(entry.id.as_str(), "m-transition");
    assert!(entry.is_transition());

    // 时间证据包字段验证
    assert_eq!(meta.valid_from, 1000);
    assert_eq!(meta.valid_until, Some(2000));
    assert_eq!(
        meta.transition_type,
        nexus_contracts::TransitionType::Transition
    );
    // 降置信度
    assert!((meta.confidence - 0.4).abs() < 1e-6);
}

#[tokio::test]
async fn test_p3_w11_1_2_recall_transition_filters_current_and_historical() {
    // P3-W11.1.2: recall_transition 对 Current/Historical 返回 None
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    // Current 条目
    engine
        .store(make_entry("m-current", MemoryTier::L0Working))
        .await
        .unwrap();

    // Historical 条目
    let historical_meta = nexus_contracts::TemporalMeta {
        valid_from: 1000,
        valid_until: Some(2000),
        transition_type: nexus_contracts::TransitionType::Historical,
        confidence: 0.5,
    };
    engine
        .store(make_entry_with_temporal(
            "m-historical",
            MemoryTier::L0Working,
            historical_meta,
        ))
        .await
        .unwrap();

    // recall_transition 对 Current/Historical 返回 None
    assert!(engine
        .recall_transition("m-current")
        .await
        .unwrap()
        .is_none());
    assert!(engine
        .recall_transition("m-historical")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn test_p3_w11_1_2_three_recall_methods_mutual_exclusion() {
    // P3-W11.1.2 验收:三个召回方法对同一 ID 互斥
    // spec.md:293-295 "默认只取 Current;Historical 需显式历史查询;Transition 附时间证据包"
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    // 存储三种状态的条目
    let current_meta = nexus_contracts::TemporalMeta::new(1000, 1.0);
    let historical_meta = nexus_contracts::TemporalMeta {
        valid_from: 1000,
        valid_until: Some(2000),
        transition_type: nexus_contracts::TransitionType::Historical,
        confidence: 0.5,
    };
    let transition_meta = nexus_contracts::TemporalMeta {
        valid_from: 1000,
        valid_until: Some(2000),
        transition_type: nexus_contracts::TransitionType::Transition,
        confidence: 0.3,
    };

    engine
        .store(make_entry_with_temporal(
            "m-current",
            MemoryTier::L0Working,
            current_meta,
        ))
        .await
        .unwrap();
    engine
        .store(make_entry_with_temporal(
            "m-historical",
            MemoryTier::L0Working,
            historical_meta,
        ))
        .await
        .unwrap();
    engine
        .store(make_entry_with_temporal(
            "m-transition",
            MemoryTier::L0Working,
            transition_meta,
        ))
        .await
        .unwrap();

    // Current 条目:recall_current 命中,recall_historical/recall_transition 不命中
    assert!(engine.recall_current("m-current").await.unwrap().is_some());
    assert!(engine
        .recall_historical("m-current")
        .await
        .unwrap()
        .is_none());
    assert!(engine
        .recall_transition("m-current")
        .await
        .unwrap()
        .is_none());

    // Historical 条目:recall_historical 命中,recall_current/recall_transition 不命中
    assert!(engine
        .recall_current("m-historical")
        .await
        .unwrap()
        .is_none());
    assert!(engine
        .recall_historical("m-historical")
        .await
        .unwrap()
        .is_some());
    assert!(engine
        .recall_transition("m-historical")
        .await
        .unwrap()
        .is_none());

    // Transition 条目:recall_transition 命中,recall_current/recall_historical 不命中
    assert!(engine
        .recall_current("m-transition")
        .await
        .unwrap()
        .is_none());
    assert!(engine
        .recall_historical("m-transition")
        .await
        .unwrap()
        .is_none());
    assert!(engine
        .recall_transition("m-transition")
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn test_p3_w11_1_2_recall_preserves_backward_compat() {
    // P3-W11.1.2: recall(向后兼容)仍返回所有状态条目
    // 验证方案 A:recall 不改变行为,新增方法提供过滤
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    // Current 条目(None temporal_meta)
    engine
        .store(make_entry("m-current", MemoryTier::L0Working))
        .await
        .unwrap();

    // Historical 条目
    let historical_meta = nexus_contracts::TemporalMeta {
        valid_from: 1000,
        valid_until: Some(2000),
        transition_type: nexus_contracts::TransitionType::Historical,
        confidence: 0.5,
    };
    engine
        .store(make_entry_with_temporal(
            "m-historical",
            MemoryTier::L0Working,
            historical_meta,
        ))
        .await
        .unwrap();

    // recall(向后兼容)对两种状态都返回条目
    assert!(engine.recall("m-current").await.unwrap().is_some());
    assert!(engine.recall("m-historical").await.unwrap().is_some());
}

#[tokio::test]
async fn test_p3_w11_1_2_recall_nonexistent_returns_none() {
    // P3-W11.1.2: 三个新方法对不存在 ID 返回 None
    let bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(bus).unwrap();

    assert!(engine
        .recall_current("nonexistent")
        .await
        .unwrap()
        .is_none());
    assert!(engine
        .recall_historical("nonexistent")
        .await
        .unwrap()
        .is_none());
    assert!(engine
        .recall_transition("nonexistent")
        .await
        .unwrap()
        .is_none());
}
