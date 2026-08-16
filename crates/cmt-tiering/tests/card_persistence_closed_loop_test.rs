//! L1→L3 消费接线闭环集成测试 — ExperienceCardBus → with_card_persistence（Phase 3 D-4）
//!
//! 覆盖: L1 经验卡片总线 → L3 持久化端到端闭环 / 分级投递持久化策略 /
//! TokenLedger 训练证据落盘闭环 / 孤儿发布者消除验证

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use cmt_tiering::{CmtConfig, CmtCoordinator, ExperienceCardStorage};
use event_bus::{EventBus, ExperienceCardBus, TokenLedger};
use nexus_contracts::experience_card::{CardMetadata, ExecutionStatus, ThreeFactorScore};
use nexus_contracts::token_evidence::TokenLedgerEntry;
use nexus_contracts::{AtomicOperator, ExperienceCard};

fn card(id: &str, task: &str, score: f32) -> ExperienceCard {
    ExperienceCard {
        card_id: Box::from(id),
        task_id: Box::from(task),
        node_id: Box::from(format!("node-{id}")),
        parent_id: None,
        created_at: Utc::now(),
        operator: AtomicOperator::Debug,
        score,
        delta_vs_parent: 0.0,
        method_family: Box::from("debug_pipeline"),
        error_signature: None,
        three_factor: ThreeFactorScore {
            quality: score,
            progress: 0.1,
            novelty: 0.3,
        },
        execution_status: ExecutionStatus::Success,
        token_evidence_ids: Vec::new(),
        segment_id: None,
        metadata: CardMetadata::default(),
    }
}

// ----------------------------------------------------------
// L1 → L3 端到端持久化闭环
// ----------------------------------------------------------

#[tokio::test]
async fn card_bus_to_storage_closed_loop() {
    // L1 经验卡片总线
    let card_bus = ExperienceCardBus::new();
    // L3 存储
    let storage = Arc::new(
        ExperienceCardStorage::new_in_memory(64)
            .await
            .expect("创建成功"),
    );
    // CMT 协调器接入持久化（D-4 接线）
    let event_bus = EventBus::new();
    let coordinator =
        CmtCoordinator::new_in_memory(CmtConfig::default(), event_bus).expect("CMT 创建成功");
    let _coordinator = coordinator.with_card_persistence(&card_bus, Arc::clone(&storage));

    // 发布中分卡片（0.5-0.8 走 broadcast → 被持久化任务消费）
    card_bus.publish(card("c1", "t1", 0.7));
    card_bus.publish(card("c2", "t1", 0.6));

    // 等待后台持久化任务处理
    tokio::time::sleep(Duration::from_millis(300)).await;

    // L3 存储应已持久化 2 张卡片（闭环验证）
    let count = storage.card_count().await.expect("查询成功");
    assert_eq!(count, 2, "L1 总线中分卡片应持久化到 L3（闭环）");
}

#[tokio::test]
async fn high_score_cards_not_persisted_via_broadcast() {
    // 高分卡片（>0.8）走 Critical mpsc，不进 broadcast → 不被 broadcast 消费路径持久化
    let card_bus = ExperienceCardBus::new();
    let storage = Arc::new(
        ExperienceCardStorage::new_in_memory(64)
            .await
            .expect("创建成功"),
    );
    let event_bus = EventBus::new();
    let coordinator =
        CmtCoordinator::new_in_memory(CmtConfig::default(), event_bus).expect("CMT 创建成功");
    let _coordinator = coordinator.with_card_persistence(&card_bus, Arc::clone(&storage));

    // 发布高分卡片（>0.8 → Critical，不走 broadcast）
    card_bus.publish(card("c-high", "t1", 0.95));
    tokio::time::sleep(Duration::from_millis(200)).await;

    // broadcast 消费路径不应持久化高分卡片
    let count = storage.card_count().await.expect("查询成功");
    assert_eq!(count, 0, "高分卡走 Critical，broadcast 路径不持久化");
}

#[tokio::test]
async fn low_score_cards_not_broadcast_not_persisted() {
    // 低分卡片（≤0.5）静默丢弃，不进 broadcast → 不持久化
    let card_bus = ExperienceCardBus::new();
    let storage = Arc::new(
        ExperienceCardStorage::new_in_memory(64)
            .await
            .expect("创建成功"),
    );
    let event_bus = EventBus::new();
    let coordinator =
        CmtCoordinator::new_in_memory(CmtConfig::default(), event_bus).expect("CMT 创建成功");
    let _coordinator = coordinator.with_card_persistence(&card_bus, Arc::clone(&storage));

    card_bus.publish(card("c-low", "t1", 0.4)); // 低分 → 丢弃
    tokio::time::sleep(Duration::from_millis(200)).await;

    let count = storage.card_count().await.expect("查询成功");
    assert_eq!(count, 0, "低分卡静默丢弃，不持久化");
}

// ----------------------------------------------------------
// 持久化数据可查询（闭环完整性）
// ----------------------------------------------------------

#[tokio::test]
async fn persisted_cards_queryable_by_three_factor() {
    let card_bus = ExperienceCardBus::new();
    let storage = Arc::new(
        ExperienceCardStorage::new_in_memory(64)
            .await
            .expect("创建成功"),
    );
    let event_bus = EventBus::new();
    let coordinator =
        CmtCoordinator::new_in_memory(CmtConfig::default(), event_bus).expect("CMT 创建成功");
    let _coordinator = coordinator.with_card_persistence(&card_bus, Arc::clone(&storage));

    // 发布多张中分卡片
    for i in 0..3 {
        let score = 0.6 + i as f32 * 0.05;
        card_bus.publish(card(&format!("c{i}"), "t1", score));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    // 持久化后可通过三因子查询（闭环完整性）
    let results = storage
        .query_by_three_factor("t1", 0.0, 10)
        .await
        .expect("查询");
    assert_eq!(results.len(), 3, "持久化的卡片应可查询");
}

// ----------------------------------------------------------
// TokenLedger 训练证据落盘闭环（补 Phase 1 WAL 遗留）
// ----------------------------------------------------------

#[tokio::test]
async fn token_ledger_evidence_persistence_closed_loop() {
    // L1 Token 账本
    let ledger = TokenLedger::new();
    ledger
        .append(TokenLedgerEntry::new(
            "e1",
            0,
            "s1",
            "i1",
            vec![101, 102],
            vec![201],
            vec![0.9],
            vec![true],
            "v1",
            vec![],
            None,
            1_700_000_000_000,
        ))
        .expect("账本追加成功");
    ledger
        .append(TokenLedgerEntry::new(
            "e2",
            1,
            "s1",
            "i1",
            vec![103],
            vec![202, 203],
            vec![0.8, 0.7],
            vec![true, true],
            "v1",
            vec![],
            None,
            1_700_000_001_000,
        ))
        .expect("账本追加成功");

    // 导出 → L3 落盘（闭环）
    let entries = ledger.export_entries();
    assert_eq!(entries.len(), 2);
    let storage = ExperienceCardStorage::new_in_memory(16)
        .await
        .expect("创建成功");
    let inserted = storage
        .store_evidence_batch(&entries)
        .await
        .expect("落盘成功");
    assert_eq!(inserted, 2);
    assert_eq!(storage.evidence_count().await.expect("查询"), 2);

    // 完整性审计包含证据行数
    let report = storage.integrity_check().await.expect("审计");
    assert_eq!(report.evidence_rows, 2);
}

// ----------------------------------------------------------
// 孤儿发布者消除验证
// ----------------------------------------------------------

#[tokio::test]
async fn no_orphan_publisher_card_bus_has_subscriber() {
    // with_card_persistence 订阅后，card_bus 的 broadcast 有订阅者（非孤儿）
    let card_bus = ExperienceCardBus::new();
    let storage = Arc::new(
        ExperienceCardStorage::new_in_memory(16)
            .await
            .expect("创建成功"),
    );
    let event_bus = EventBus::new();
    let coordinator =
        CmtCoordinator::new_in_memory(CmtConfig::default(), event_bus).expect("CMT 创建成功");
    // 接线前: 手动验证 subscribe 增加订阅者
    let _rx = card_bus.subscribe();
    // 接线: with_card_persistence 内部再 subscribe
    let _coordinator = coordinator.with_card_persistence(&card_bus, Arc::clone(&storage));
    // 发布中分卡片，验证有消费者（不丢失）
    card_bus.publish(card("c1", "t1", 0.7));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let count = storage.card_count().await.expect("查询");
    assert_eq!(count, 1, "接线后卡片有消费者，非孤儿发布者");
}
