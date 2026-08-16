//! 经验卡片契约集成测试 — ExperienceCard 与 Token 证据链协同（v3.4.0 §5.2 + §5.3）
//!
//! 覆盖: 顶层 API 可达性 / 证据链闭环（卡片 ↔ TokenLedgerEntry）/
//! 三因子归一化 proptest 属性 / 执行状态判定互斥完备性

#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use nexus_contracts::{
    AtomicOperator, ErrorSignature, ExecutionStatus, ExperienceCard, ThreeFactorScore,
    TokenLedgerEntry,
};
use proptest::prelude::*;

/// 构造样例经验卡片（含错误签名 + 证据链 ID）
fn sample_card_with_evidence() -> (ExperienceCard, TokenLedgerEntry) {
    let ledger = TokenLedgerEntry::new(
        "ledger-001",
        0,
        "session-1",
        "instance-1",
        vec![101, 102],
        vec![201, 202],
        vec![0.9, 0.8],
        vec![true, true],
        "v2.26.0-omega",
        vec![],
        None,
        1_700_000_000_000,
    );
    let card = ExperienceCard {
        card_id: Box::from("card-001"),
        task_id: Box::from("task-1"),
        node_id: Box::from("node-1"),
        parent_id: None,
        created_at: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("合法时间戳"),
        operator: AtomicOperator::Debug,
        score: 0.72,
        delta_vs_parent: 0.12,
        method_family: Box::from("two_pass_debug"),
        error_signature: Some(ErrorSignature {
            error_type: Box::from("compile_error"),
            error_location: Box::from("src/lib.rs:42"),
            error_summary: Box::from("未定义标识符"),
            error_hash: Box::from("0123456789abcdef"),
        }),
        three_factor: ThreeFactorScore {
            quality: 0.72,
            progress: 0.12,
            novelty: 0.5,
        },
        execution_status: ExecutionStatus::Error,
        token_evidence_ids: vec![Box::from("ledger-001")],
        segment_id: Some(Box::from("seg-1")),
        metadata: Default::default(),
    };
    (card, ledger)
}

// ----------------------------------------------------------
// 顶层 API 可达性（依赖方直接 import 路径验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    // 依赖方可直接 `use nexus_contracts::ExperienceCard`（顶层导出验证）
    let (card, _) = sample_card_with_evidence();
    assert_eq!(card.card_id.as_ref(), "card-001");
    assert_eq!(card.operator, AtomicOperator::Debug);
    assert_eq!(card.execution_status, ExecutionStatus::Error);
    let sig = card.error_signature.expect("错误签名存在");
    assert_eq!(sig.error_hash.len(), 16);
}

#[test]
fn prelude_api_accessible() {
    use nexus_contracts::prelude::*;
    let score = ThreeFactorScore::default_root();
    assert_eq!(score.novelty, 1.0);
    let _ = ExperienceCard::updated_assessment;
    let _ = AtomicOperator::Draft;
    let _ = ExecutionStatus::Success;
    let _ = ErrorSignature {
        error_type: Box::from("t"),
        error_location: Box::from("l"),
        error_summary: Box::from("s"),
        error_hash: Box::from("h"),
    };
}

// ----------------------------------------------------------
// 证据链闭环（v3.4.0 §5.2: 卡片关联 TokenLedgerEntry，形成经验-证据闭环）
// ----------------------------------------------------------

#[test]
fn evidence_chain_closure() {
    let (card, ledger) = sample_card_with_evidence();
    // 卡片的 token_evidence_ids 必须能解析到对应账本条目
    assert_eq!(card.token_evidence_ids.len(), 1);
    assert_eq!(
        card.token_evidence_ids[0].as_ref(),
        ledger.entry_id.as_ref()
    );
    // 账本证据完整性: 三序列等长（构造器已断言，此处验证语义）
    assert_eq!(ledger.output_len(), 2);
    assert_eq!(ledger.loss_mask.len(), ledger.output_len());
}

#[test]
fn evidence_chain_json_roundtrip_closure() {
    // 卡片 + 账本整体序列化闭环（EventBus 传输形态）
    let (card, ledger) = sample_card_with_evidence();
    let card_json = serde_json::to_string(&card).expect("卡片 JSON 序列化失败");
    let ledger_json = serde_json::to_string(&ledger).expect("账本 JSON 序列化失败");
    let card_back: ExperienceCard = serde_json::from_str(&card_json).expect("卡片反序列化失败");
    let ledger_back: TokenLedgerEntry =
        serde_json::from_str(&ledger_json).expect("账本反序列化失败");
    assert_eq!(card_back, card);
    assert_eq!(ledger_back, ledger);
}

// ----------------------------------------------------------
// 三因子归一化 proptest 属性
// ----------------------------------------------------------

proptest! {
    /// 归一化属性: 任意非负输入，归一化结果恒为有限值（无 NaN/Inf）
    #[test]
    fn three_factor_normalize_always_finite(
        q in 0.0f32..1.0,
        p in 0.0f32..1.0,
        n in 0.0f32..1.0,
        mq in 0.0f32..1.0,
        mp in 0.0f32..1.0,
        mn in 0.0f32..1.0,
    ) {
        let s = ThreeFactorScore { quality: q, progress: p, novelty: n };
        let norm = s.normalize(mq, mp, mn);
        prop_assert!(norm.quality.is_finite());
        prop_assert!(norm.progress.is_finite());
        prop_assert!(norm.novelty.is_finite());
    }

    /// 归一化边界属性: 归一化值 ≤ 原始值 / max（数学恒等）
    #[test]
    fn three_factor_normalize_identity(
        q in 0.0f32..1.0,
        p in 0.0f32..1.0,
        n in 0.0f32..1.0,
    ) {
        let s = ThreeFactorScore { quality: q, progress: p, novelty: n };
        let norm = s.normalize(1.0, 1.0, 1.0);
        prop_assert!((norm.quality - q).abs() < 1e-6);
        prop_assert!((norm.progress - p).abs() < 1e-6);
        prop_assert!((norm.novelty - n).abs() < 1e-6);
    }
}

// ----------------------------------------------------------
// 执行状态判定互斥完备性
// ----------------------------------------------------------

#[test]
fn execution_status_judgements_exhaustive() {
    // 互斥完备: 任一状态必属可重试或不可重试之一（且仅一）
    for status in [
        ExecutionStatus::Success,
        ExecutionStatus::Error,
        ExecutionStatus::MissingCode,
        ExecutionStatus::NoSubmit,
        ExecutionStatus::ScoreFailed,
        ExecutionStatus::Timeout,
    ] {
        assert_eq!(
            status.is_retryable(),
            !matches!(
                status,
                ExecutionStatus::Success | ExecutionStatus::MissingCode | ExecutionStatus::NoSubmit
            )
        );
        assert_eq!(
            status.generates_meaningful_card(),
            matches!(
                status,
                ExecutionStatus::Success | ExecutionStatus::Error | ExecutionStatus::Timeout
            )
        );
    }
}

// ----------------------------------------------------------
// 版本化更新链（不可变契约 + 证据链延伸）
// ----------------------------------------------------------

#[test]
fn versioned_update_chain_extends_evidence() {
    let (card, _) = sample_card_with_evidence();
    let v2 = card.updated_assessment(
        "card-002",
        0.88,
        ThreeFactorScore {
            quality: 0.88,
            progress: 0.16,
            novelty: 0.4,
        },
        ExecutionStatus::Success,
        None,
        vec![Box::from("ledger-001"), Box::from("ledger-002")],
        Some(Box::from("seg-2")),
    );
    let v3 = v2.updated_assessment(
        "card-003",
        0.93,
        ThreeFactorScore {
            quality: 0.93,
            progress: 0.05,
            novelty: 0.3,
        },
        ExecutionStatus::Success,
        None,
        vec![
            Box::from("ledger-001"),
            Box::from("ledger-002"),
            Box::from("ledger-003"),
        ],
        Some(Box::from("seg-3")),
    );
    // 版本链: v1 → v2 → v3（父指针回溯 + delta 累积语义）
    assert_eq!(v2.parent_id.as_deref(), Some("card-001"));
    assert_eq!(v3.parent_id.as_deref(), Some("card-002"));
    assert_eq!(v3.token_evidence_ids.len(), 3);
    // 原始卡片保持不可变
    assert_eq!(card.card_id.as_ref(), "card-001");
    assert_eq!(card.token_evidence_ids.len(), 1);
}
