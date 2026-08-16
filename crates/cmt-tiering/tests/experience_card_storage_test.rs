//! 经验卡片持久化集成测试 — SQLite 复合索引 + 完整性审计（v3.4.0 §8.2）
//!
//! 覆盖: 顶层 API 可达性 / 持久化全链路 / 三因子查询 / 完整性审计 /
//! 热缓存一致性 / proptest 持久化不变量

#![forbid(unsafe_code)]

use std::sync::Arc;

use chrono::Utc;
use cmt_tiering::ExperienceCardStorage;
use nexus_contracts::experience_card::{CardMetadata, ExecutionStatus, ThreeFactorScore};
use nexus_contracts::{AtomicOperator, ExperienceCard};
use proptest::prelude::*;

fn card(id: &str, task: &str, score: f32, quality: f32) -> ExperienceCard {
    ExperienceCard {
        card_id: Box::from(id),
        task_id: Box::from(task),
        node_id: Box::from(format!("node-{id}")),
        parent_id: None,
        created_at: Utc::now(),
        operator: AtomicOperator::Improve,
        score,
        delta_vs_parent: 0.05,
        method_family: Box::from("improve_pipeline"),
        error_signature: None,
        three_factor: ThreeFactorScore {
            quality,
            progress: 0.15,
            novelty: 0.4,
        },
        execution_status: ExecutionStatus::Success,
        token_evidence_ids: Vec::new(),
        segment_id: None,
        metadata: CardMetadata::default(),
    }
}

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[tokio::test]
async fn top_level_api_accessible() {
    use cmt_tiering::prelude::*;
    let storage = ExperienceCardStorage::new_in_memory(32)
        .await
        .expect("创建成功");
    storage
        .store(&card("c1", "t1", 0.8, 0.8))
        .await
        .expect("存储");
    let report = storage.integrity_check().await.expect("审计");
    assert!(report.consistent);
    let _ = CardStorageIntegrityReport {
        sqlite_rows: 0,
        hot_cache_len: 0,
        evidence_rows: 0,
        empty_payload: 0,
        consistent: true,
    };
}

// ----------------------------------------------------------
// 持久化全链路（顶层 API）
// ----------------------------------------------------------

#[tokio::test]
async fn persistence_full_lifecycle() {
    let storage = Arc::new(
        ExperienceCardStorage::new_in_memory(32)
            .await
            .expect("创建成功"),
    );
    // 存储多张卡片
    for i in 0..5 {
        let score = 0.5 + i as f32 * 0.1;
        storage
            .store(&card(&format!("c{i}"), "t1", score, score))
            .await
            .expect("存储");
    }
    assert_eq!(storage.card_count().await.expect("查询"), 5);
    // 三因子查询（顶层 API）
    let results = storage
        .query_by_three_factor("t1", 0.0, 3)
        .await
        .expect("查询");
    assert_eq!(results.len(), 3);
    // 完整性审计
    let report = storage.integrity_check().await.expect("审计");
    assert_eq!(report.sqlite_rows, 5);
    assert!(report.consistent);
    assert_eq!(report.empty_payload, 0);
}

// ----------------------------------------------------------
// 完整性审计
// ----------------------------------------------------------

#[tokio::test]
async fn integrity_check_empty_storage() {
    let storage = ExperienceCardStorage::new_in_memory(16)
        .await
        .expect("创建成功");
    let report = storage.integrity_check().await.expect("审计");
    assert_eq!(report.sqlite_rows, 0);
    assert_eq!(report.hot_cache_len, 0);
    assert_eq!(report.evidence_rows, 0);
    assert_eq!(report.empty_payload, 0);
    assert!(report.consistent, "空库应一致");
}

#[tokio::test]
async fn integrity_check_after_stores() {
    let storage = ExperienceCardStorage::new_in_memory(16)
        .await
        .expect("创建成功");
    storage
        .store(&card("c1", "t1", 0.8, 0.8))
        .await
        .expect("存储");
    storage
        .store(&card("c2", "t1", 0.6, 0.6))
        .await
        .expect("存储");
    let report = storage.integrity_check().await.expect("审计");
    assert_eq!(report.sqlite_rows, 2);
    assert!(report.hot_cache_len >= 1, "高分卡片应入热缓存");
    assert!(report.consistent);
}

// ----------------------------------------------------------
// 热缓存与 SQLite 一致性
// ----------------------------------------------------------

#[tokio::test]
async fn hot_cache_sqlite_consistency() {
    let storage = ExperienceCardStorage::new_in_memory(4)
        .await
        .expect("创建成功");
    // 存储超过热缓存容量的卡片
    for i in 0..6 {
        storage
            .store(&card(&format!("c{i}"), "t1", 0.8, 0.8))
            .await
            .expect("存储");
    }
    // 热缓存不超容量
    assert!(storage.hot_cache_len() <= 4, "热缓存应不超容量");
    // SQLite 保存全部
    assert_eq!(storage.card_count().await.expect("查询"), 6);
    // 查询仍能返回全部（热缓存 + SQLite 合并）
    let results = storage
        .query_by_three_factor("t1", 0.0, 10)
        .await
        .expect("查询");
    assert_eq!(results.len(), 6, "应合并热缓存与 SQLite 结果");
}

// ----------------------------------------------------------
// proptest: 持久化不变量
// ----------------------------------------------------------

proptest! {
    /// 任意卡片序列持久化后，card_count 恒等于存储数，完整性一致
    #[test]
    fn persistence_count_invariant(
        n in 1usize..15,
        score in 0.0f32..1.0,
    ) {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        // async block 返回 Result 以使 prop_assert! 的 early-return 生效
        let result: Result<(), proptest::test_runner::TestCaseError> = rt.block_on(async {
            let storage = ExperienceCardStorage::new_in_memory(64).await.expect("创建");
            for i in 0..n {
                storage.store(&card(&format!("c{i}"), "t1", score, score)).await.expect("存储");
            }
            let count = storage.card_count().await.expect("查询");
            prop_assert_eq!(count as usize, n);
            let report = storage.integrity_check().await.expect("审计");
            prop_assert!(report.consistent);
            prop_assert_eq!(report.sqlite_rows as usize, n);
            Ok(())
        });
        result?;
    }
}
