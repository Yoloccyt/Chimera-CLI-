//! L4→L0→L3 错误签名闭环集成测试 — 提取→契约→持久化索引协同（Phase 4 Wave 4）
//!
//! 覆盖: L4 ErrorSignatureCollector 提取 → L0 ExperienceCard.error_signature 契约 →
//! L3 ExperienceCardStorage.query_by_error_signature（idx_error_hash 索引）全链路闭环。
//! 验证 D-3 哈希一致性（collector.compute_error_hash 与 L3 存储 error_hash 对齐）。

#![forbid(unsafe_code)]

use chrono::Utc;
use cmt_tiering::ExperienceCardStorage;
use nexus_contracts::experience_card::{
    AtomicOperator, CardMetadata, ExecutionStatus, ThreeFactorScore,
};
use nexus_contracts::ExperienceCard;
use seccore::ErrorSignatureCollector;

/// 构造带错误签名的经验卡片（从 collector 提取的签名注入 L0 契约）
fn card_with_signature(
    id: &str,
    sig: nexus_contracts::experience_card::ErrorSignature,
) -> ExperienceCard {
    ExperienceCard {
        card_id: id.into(),
        task_id: "task-err".into(),
        node_id: format!("node-{id}").into(),
        parent_id: None,
        created_at: Utc::now(),
        operator: AtomicOperator::Debug,
        score: 0.3,
        delta_vs_parent: -0.1,
        method_family: "debug_pipeline".into(),
        error_signature: Some(sig),
        three_factor: ThreeFactorScore {
            quality: 0.3,
            progress: -0.1,
            novelty: 0.2,
        },
        execution_status: ExecutionStatus::Error,
        token_evidence_ids: Vec::new(),
        segment_id: None,
        metadata: CardMetadata::default(),
    }
}

// ----------------------------------------------------------
// L4 → L0 → L3 全链路闭环
// ----------------------------------------------------------

#[tokio::test]
async fn error_signature_l4_to_l3_closed_loop() {
    // 1. L4: 从执行输出提取错误签名
    let mut collector = ErrorSignatureCollector::new();
    let sig = collector
        .extract("error[E0308]: mismatched types", "src/main.rs:42")
        .expect("L4 应提取错误签名");
    let extracted_hash = sig.error_hash.to_string();

    // 2. L0: 签名注入 ExperienceCard 契约
    let card = card_with_signature("card-err-1", sig);
    assert!(card.error_signature.is_some());

    // 3. L3: 持久化到 ExperienceCardStorage
    let storage = ExperienceCardStorage::new_in_memory(16)
        .await
        .expect("创建存储");
    storage.store(&card).await.expect("持久化成功");

    // 4. L3: 通过 idx_error_hash 索引查询（用 L4 提取的哈希）
    let results = storage
        .query_by_error_signature(&extracted_hash, 10)
        .await
        .expect("查询成功");
    assert_eq!(results.len(), 1, "应通过 error_hash 索引查到 1 张卡片");
    assert_eq!(results[0].card_id.as_ref(), "card-err-1");
    // 闭环验证: 查询出的卡片错误签名哈希与 L4 提取的一致
    let queried_sig = results[0].error_signature.as_ref().expect("有签名");
    assert_eq!(
        queried_sig.error_hash.as_ref(),
        extracted_hash,
        "哈希应全链路一致"
    );
}

// ----------------------------------------------------------
// D-3 哈希一致性（L4 提取 == L3 索引）
// ----------------------------------------------------------

#[tokio::test]
async fn hash_consistency_l4_extraction_matches_l3_index() {
    let mut collector = ErrorSignatureCollector::new();
    // 同一错误提取两次 → 相同哈希 → L3 聚类
    let sig1 = collector
        .extract("error[E0308]: mismatched types", "a.rs:1")
        .expect("提取");
    let sig2 = collector
        .extract("error[E0308]: mismatched types", "b.rs:2")
        .expect("提取");
    assert_eq!(
        sig1.error_hash, sig2.error_hash,
        "同错误应同哈希（去重聚类）"
    );
    // 先保存哈希（sig1 即将被 move 入卡片）
    let sig1_hash = sig1.error_hash.to_string();

    // 两张卡片同哈希 → L3 查询返回 2 张
    let storage = ExperienceCardStorage::new_in_memory(16)
        .await
        .expect("创建存储");
    storage
        .store(&card_with_signature("c1", sig1))
        .await
        .expect("存储");
    storage
        .store(&card_with_signature("c2", sig2))
        .await
        .expect("存储");
    let results = storage
        .query_by_error_signature(&sig1_hash, 10)
        .await
        .expect("查询");
    assert_eq!(results.len(), 2, "同哈希两张卡片都应查到");
}

// ----------------------------------------------------------
// 不同错误类型隔离
// ----------------------------------------------------------

#[tokio::test]
async fn different_error_types_isolated_in_index() {
    let mut collector = ErrorSignatureCollector::new();
    let compile_sig = collector
        .extract("error[E0308]: mismatched types", "a.rs")
        .expect("提取");
    let panic_sig = collector
        .extract("thread 'main' panicked at x.rs:1, boom", "b.rs")
        .expect("提取");
    assert_ne!(
        compile_sig.error_hash, panic_sig.error_hash,
        "不同错误应不同哈希"
    );

    let storage = ExperienceCardStorage::new_in_memory(16)
        .await
        .expect("创建存储");
    storage
        .store(&card_with_signature("c-compile", compile_sig.clone()))
        .await
        .expect("存储");
    storage
        .store(&card_with_signature("c-panic", panic_sig.clone()))
        .await
        .expect("存储");

    // 按编译错误哈希查询 → 只返回编译错误卡片
    let compile_results = storage
        .query_by_error_signature(&compile_sig.error_hash, 10)
        .await
        .expect("查询");
    assert_eq!(compile_results.len(), 1);
    assert_eq!(compile_results[0].card_id.as_ref(), "c-compile");
    // 按 panic 哈希查询 → 只返回 panic 卡片
    let panic_results = storage
        .query_by_error_signature(&panic_sig.error_hash, 10)
        .await
        .expect("查询");
    assert_eq!(panic_results.len(), 1);
    assert_eq!(panic_results[0].card_id.as_ref(), "c-panic");
}

// ----------------------------------------------------------
// 无错误卡片不影响错误索引
// ----------------------------------------------------------

#[tokio::test]
async fn success_card_without_signature_not_in_error_index() {
    let storage = ExperienceCardStorage::new_in_memory(16)
        .await
        .expect("创建存储");
    // 无错误签名的成功卡片
    let success_card = ExperienceCard {
        card_id: "card-ok".into(),
        task_id: "task-ok".into(),
        node_id: "node-ok".into(),
        parent_id: None,
        created_at: Utc::now(),
        operator: AtomicOperator::Draft,
        score: 0.9,
        delta_vs_parent: 0.1,
        method_family: "draft".into(),
        error_signature: None,
        three_factor: ThreeFactorScore {
            quality: 0.9,
            progress: 0.1,
            novelty: 0.5,
        },
        execution_status: ExecutionStatus::Success,
        token_evidence_ids: Vec::new(),
        segment_id: None,
        metadata: CardMetadata::default(),
    };
    storage.store(&success_card).await.expect("存储");
    // 任意哈希查询不应返回无签名卡片
    let results = storage
        .query_by_error_signature("anyhash00000000", 10)
        .await
        .expect("查询");
    assert!(results.is_empty(), "无签名卡片不在错误索引中");
}
