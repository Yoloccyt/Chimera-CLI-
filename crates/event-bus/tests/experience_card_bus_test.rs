//! 经验卡片总线集成测试 — 与 L0 契约类型全链路协同（v3.4.0 §6.1）
//!
//! 覆盖: 顶层 API 可达性 / 证据链关联（卡片 ↔ Token 账本）/
//! 四索引跨模块查询 / 分级投递全路径（含无订阅者场景）

#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use event_bus::{ExperienceCardBus, TokenLedger};
use nexus_contracts::token_evidence::TokenLedgerEntry;
use nexus_contracts::{
    AtomicOperator, ErrorSignature, ExecutionStatus, ExperienceCard, ThreeFactorScore,
};

/// 构造样例卡片（score/status/错误签名可定制）
fn card(id: &str, task: &str, score: f32, status: ExecutionStatus) -> ExperienceCard {
    ExperienceCard {
        card_id: Box::from(id),
        task_id: Box::from(task),
        node_id: Box::from(format!("node-{id}")),
        parent_id: None,
        created_at: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("合法时间戳"),
        operator: AtomicOperator::Debug,
        score,
        delta_vs_parent: 0.05,
        method_family: Box::from("two_pass_debug"),
        error_signature: None,
        three_factor: ThreeFactorScore {
            quality: score,
            progress: 0.05,
            novelty: 0.4,
        },
        execution_status: status,
        token_evidence_ids: Vec::new(),
        segment_id: None,
        metadata: Default::default(),
    }
}

/// 构造带证据链的卡片（token_evidence_ids → TokenLedger 账本条目）
fn card_with_evidence(id: &str, task: &str, ledger_id: &str) -> ExperienceCard {
    let mut c = card(id, task, 0.85, ExecutionStatus::Success);
    c.token_evidence_ids = vec![Box::from(ledger_id)];
    c
}

// ----------------------------------------------------------
// 顶层 API 可达性（依赖方直接 import 路径验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    let bus = ExperienceCardBus::new();
    bus.publish(card("c1", "t1", 0.9, ExecutionStatus::Success));
    assert_eq!(bus.get_global_stats().total_cards, 1);
    // TokenLedger 顶层可达
    let ledger = TokenLedger::new();
    assert!(ledger.is_empty());
}

// ----------------------------------------------------------
// 证据链闭环: 卡片 ↔ Token 账本（Dressage 经验-证据闭环）
// ----------------------------------------------------------

#[test]
fn card_evidence_chain_closure_with_ledger() {
    // 1. 构建 Token 账本并追加证据
    let ledger = TokenLedger::new();
    let entry = TokenLedgerEntry::new(
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
    ledger.append(entry).expect("账本追加成功");

    // 2. 卡片携带证据链 ID 发布到总线
    let bus = ExperienceCardBus::new();
    let mut critical_rx = bus.subscribe_critical();
    bus.publish(card_with_evidence("c-ev", "t1", "ledger-001"));

    // 3. 闭环验证: 卡片 → 证据 ID → 账本条目
    let received = critical_rx.try_recv().expect("高分卡片应达 Critical 通道");
    assert_eq!(received.token_evidence_ids.len(), 1);
    let resolved = ledger
        .get(&received.token_evidence_ids[0])
        .expect("证据 ID 必须可解析到账本条目");
    assert_eq!(resolved.entry_id.as_ref(), "ledger-001");
    assert_eq!(resolved.output_len(), 2);
    // 账本完整性
    let report = ledger.integrity_check();
    assert_eq!(report.total_entries, 1);
    assert_eq!(report.duplicate_ids, 0);
}

// ----------------------------------------------------------
// 四索引跨模块查询
// ----------------------------------------------------------

#[test]
fn four_indexes_cross_module_query() {
    let bus = ExperienceCardBus::new();
    let mut err_card = card("c-err", "t1", 0.7, ExecutionStatus::Error);
    err_card.error_signature = Some(ErrorSignature {
        error_type: Box::from("compile_error"),
        error_location: Box::from("src/lib.rs:1"),
        error_summary: Box::from("E0308"),
        error_hash: Box::from("hash-xyz"),
    });
    bus.publish(card("c-ok", "t1", 0.9, ExecutionStatus::Success));
    bus.publish(err_card);

    // 任务索引 + 因子 Top-K
    let top = bus.get_top_cards_by_factor("t1", 1);
    assert_eq!(top[0].card_id.as_ref(), "c-ok");
    // 节点索引
    assert_eq!(
        bus.get_card_by_node("node-c-err")
            .expect("存在")
            .card_id
            .as_ref(),
        "c-err"
    );
    // 错误聚类索引
    let err_ids = bus.get_card_ids_by_error_hash("hash-xyz");
    assert_eq!(err_ids, vec!["c-err".to_string()]);
}

// ----------------------------------------------------------
// 分级投递: 无订阅者场景（降级路径）
// ----------------------------------------------------------

#[tokio::test]
async fn publish_without_critical_subscriber_no_panic() {
    let bus = ExperienceCardBus::new();
    // 无 Critical 订阅者发布高分卡片——不得 panic（retain 移除空列表为空操作）
    bus.publish(card("c1", "t1", 0.95, ExecutionStatus::Success));
    assert_eq!(bus.get_global_stats().total_cards, 1);
    // 无 broadcast 订阅者发布中分卡片——send 返回 Err 被忽略（尽力投递）
    bus.publish(card("c2", "t1", 0.7, ExecutionStatus::Success));
    assert_eq!(bus.get_global_stats().total_cards, 2);
}

// ----------------------------------------------------------
// 订阅者断开后发布（fan-out retain 清理）
// ----------------------------------------------------------

#[tokio::test]
async fn dropped_critical_subscriber_cleaned_up() {
    let bus = ExperienceCardBus::new();
    {
        let mut rx = bus.subscribe_critical();
        bus.publish(card("c1", "t1", 0.9, ExecutionStatus::Success));
        let _ = rx.try_recv().expect("订阅者存活时应收到");
    } // rx drop
      // 订阅者断开后发布——不得 panic（retain 移除已断开的 sender）
    bus.publish(card("c2", "t1", 0.9, ExecutionStatus::Success));
    assert_eq!(bus.get_global_stats().total_cards, 2);
}

// ----------------------------------------------------------
// 多订阅者 fan-out（EventBus critical_tx 先例）
// ----------------------------------------------------------

#[tokio::test]
async fn multiple_critical_subscribers_all_receive() {
    let bus = ExperienceCardBus::new();
    let mut rx1 = bus.subscribe_critical();
    let mut rx2 = bus.subscribe_critical();
    bus.publish(card("c1", "t1", 0.9, ExecutionStatus::Success));
    assert_eq!(
        rx1.try_recv().expect("订阅者 1 应收到").card_id.as_ref(),
        "c1"
    );
    assert_eq!(
        rx2.try_recv().expect("订阅者 2 应收到").card_id.as_ref(),
        "c1"
    );
}

// ----------------------------------------------------------
// 全链路: 卡片发布 → 索引 → 统计 → 账本导出
// ----------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn end_to_end_card_pipeline() {
    let bus = ExperienceCardBus::new();
    let ledger = TokenLedger::new();
    let mut critical_rx = bus.subscribe_critical();

    // 并发生产: 8 任务 × 50 卡片，每卡附一条 Token 证据
    let mut handles = Vec::new();
    for w in 0..8 {
        let bus = bus.clone();
        let ledger = ledger.clone();
        handles.push(tokio::spawn(async move {
            for i in 0..50 {
                let ledger_id = format!("ledger-w{w}-c{i}");
                let entry = TokenLedgerEntry::new(
                    &ledger_id,
                    i as u32,
                    &format!("session-{w}"),
                    "instance-main",
                    vec![101],
                    vec![201, 202],
                    vec![0.9, 0.8],
                    vec![true, true],
                    "v2.26.0-omega",
                    vec![],
                    None,
                    1_700_000_000_000 + i as u64,
                );
                ledger.append(entry).expect("账本追加成功");
                bus.publish(card_with_evidence(
                    &format!("c-w{w}-i{i}"),
                    &format!("task-{w}"),
                    &ledger_id,
                ));
            }
        }));
    }
    for h in handles {
        h.await.expect("并发任务不失败");
    }

    // Critical 通道应收齐 400 张高分卡片
    let mut received = 0;
    while let Ok(Some(_)) =
        tokio::time::timeout(std::time::Duration::from_millis(100), critical_rx.recv()).await
    {
        received += 1;
    }
    assert_eq!(received, 400, "Critical 通道无丢失");

    // 统计一致性: 卡片总数 = 账本条目数 = 400
    assert_eq!(bus.get_global_stats().total_cards, 400);
    assert_eq!(ledger.len(), 400);
    let ledger_report = ledger.integrity_check();
    assert_eq!(ledger_report.total_entries, 400);
    assert_eq!(ledger_report.unique_ids, 400);
    // 会话索引分片
    assert_eq!(ledger.session_entry_ids("session-3").len(), 50);
    // 卡片任务索引分片
    assert_eq!(bus.get_cards_by_task("task-7").len(), 50);
}
