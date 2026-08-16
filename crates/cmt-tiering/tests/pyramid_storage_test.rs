//! 金字塔存储映射集成测试 — 四层映射 + 分层采样 + INV-8（v3.4.0 §8.1）
//!
//! 覆盖: 顶层 API 可达性 / MemoryPyramidLevel→热温冷冰端到端存储 /
//! 分层采样比例 / INV-8 迁移单调性 / 与 rl_replay_pool 采样比例一致性

#![forbid(unsafe_code)]

use std::sync::Arc;

use cmt_tiering::{CmtConfig, CmtCoordinator, PyramidStorageMapper, Tier, PYRAMID_SAMPLE_RATIOS};
use event_bus::EventBus;
use nexus_contracts::MemoryPyramidLevel;

async fn make_mapper() -> PyramidStorageMapper {
    let bus = EventBus::new();
    let coordinator =
        CmtCoordinator::new_in_memory(CmtConfig::default(), bus).expect("CMT 协调器创建成功");
    PyramidStorageMapper::new(Arc::new(coordinator))
}

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    use cmt_tiering::prelude::*;
    // 映射纯函数可通过顶层访问
    assert_eq!(
        PyramidStorageMapper::pyramid_to_tier(MemoryPyramidLevel::L3Persona),
        Tier::Hot
    );
    let ratios = PYRAMID_SAMPLE_RATIOS;
    assert_eq!(ratios, (0.25, 0.25, 0.5));
}

// ----------------------------------------------------------
// 四层映射端到端存储
// ----------------------------------------------------------

#[tokio::test]
async fn store_pyramid_level_routes_to_correct_tier() {
    let mapper = make_mapper().await;
    // 四层各存一条（content 为序列化字符串占位）
    mapper
        .store_pyramid_level(MemoryPyramidLevel::L0RawLog, "raw-1", "raw log content")
        .await
        .expect("L0RawLog 存储成功");
    mapper
        .store_pyramid_level(
            MemoryPyramidLevel::L1AtomicMemory,
            "atomic-1",
            "atomic card",
        )
        .await
        .expect("L1AtomicMemory 存储成功");
    mapper
        .store_pyramid_level(MemoryPyramidLevel::L2SceneBlock, "scene-1", "scene block")
        .await
        .expect("L2SceneBlock 存储成功");
    mapper
        .store_pyramid_level(
            MemoryPyramidLevel::L3Persona,
            "persona-1",
            "persona summary",
        )
        .await
        .expect("L3Persona 存储成功");

    // 存储记录索引按层级分布
    let counts = mapper.stored_counts();
    assert_eq!(counts.get(&Tier::Ice), Some(&1), "L0RawLog → Ice");
    assert_eq!(counts.get(&Tier::Cold), Some(&1), "L1AtomicMemory → Cold");
    assert_eq!(counts.get(&Tier::Warm), Some(&1), "L2SceneBlock → Warm");
    assert_eq!(counts.get(&Tier::Hot), Some(&1), "L3Persona → Hot");
}

#[tokio::test]
async fn store_pyramid_level_content_retrievable() {
    let mapper = make_mapper().await;
    mapper
        .store_pyramid_level(
            MemoryPyramidLevel::L3Persona,
            "persona-1",
            "persona content",
        )
        .await
        .expect("存储成功");
    // 通过采样能取回存储的条目 ID
    let samples = mapper.sample_pyramid(10);
    assert!(
        samples.contains(&"persona-1".to_string()),
        "Hot 层条目应可采样"
    );
}

// ----------------------------------------------------------
// 分层采样
// ----------------------------------------------------------

#[tokio::test]
async fn sample_pyramid_respects_tier_distribution() {
    let mapper = make_mapper().await;
    // 填充 Cold 层 10 条（L1AtomicMemory → Cold）
    for i in 0..10 {
        mapper
            .store_pyramid_level(
                MemoryPyramidLevel::L1AtomicMemory,
                &format!("atomic-{i}"),
                "content",
            )
            .await
            .expect("存储成功");
    }
    // 采样 8: Hot 2 + Warm 2 + Cold 4（Hot/Warm 为空，只有 Cold 有结果）
    let samples = mapper.sample_pyramid(8);
    assert!(
        samples.len() <= 4,
        "Hot/Warm 空层，仅 Cold 有样本（实际 {}）",
        samples.len()
    );
    // 所有样本都是 atomic-*（Cold 层）
    for s in &samples {
        assert!(s.starts_with("atomic-"), "样本应来自 Cold 层");
    }
}

#[tokio::test]
async fn sample_pyramid_zero_batch_empty() {
    let mapper = make_mapper().await;
    let samples = mapper.sample_pyramid(0);
    assert!(samples.is_empty(), "batch_size=0 应返回空");
}

// ----------------------------------------------------------
// INV-8 迁移单调性
// ----------------------------------------------------------

#[test]
fn validate_migration_end_to_end() {
    // 合法降级链: L3Persona → L2SceneBlock → L1AtomicMemory → L0RawLog
    assert!(PyramidStorageMapper::validate_migration(
        MemoryPyramidLevel::L3Persona,
        MemoryPyramidLevel::L2SceneBlock
    )
    .is_ok());
    assert!(PyramidStorageMapper::validate_migration(
        MemoryPyramidLevel::L2SceneBlock,
        MemoryPyramidLevel::L0RawLog
    )
    .is_ok());
    // 回升拒绝: L0RawLog → L1AtomicMemory（Ice → Cold 回升）
    assert!(PyramidStorageMapper::validate_migration(
        MemoryPyramidLevel::L0RawLog,
        MemoryPyramidLevel::L1AtomicMemory
    )
    .is_err());
}

// ----------------------------------------------------------
// 采样比例一致性（Wave 4 防漂移）
// ----------------------------------------------------------

#[test]
fn sample_ratios_consistent_with_replay_pool() {
    // 金字塔采样比例必须与 rl_replay_pool 分层回放池语义一致（防漂移）
    assert_eq!(
        PYRAMID_SAMPLE_RATIOS,
        cmt_tiering::rl_replay_pool::SAMPLE_RATIOS,
        "金字塔采样与回放池采样比例必须一致"
    );
    // 比例总和为 1.0
    let sum = PYRAMID_SAMPLE_RATIOS.0 + PYRAMID_SAMPLE_RATIOS.1 + PYRAMID_SAMPLE_RATIOS.2;
    assert!((sum - 1.0).abs() < 1e-6);
}
