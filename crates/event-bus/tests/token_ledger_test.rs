//! Token 账本集成测试 — 证据完整性与训练数据面协同（v3.4.0 §6.1 + §5.3）
//!
//! 覆盖: 顶层 API / 证据完整性红线 / 导出通道（JSON + MsgPack）/
//! 跨 session/instance 回溯 / proptest 并发不变量

#![forbid(unsafe_code)]

use event_bus::{LedgerError, TokenLedger};
use nexus_contracts::token_evidence::TokenLedgerEntry;
use proptest::prelude::*;

/// 构造账本条目
fn entry(id: &str, session: &str, instance: &str, ts: u64) -> TokenLedgerEntry {
    TokenLedgerEntry::new(
        id,
        0,
        session,
        instance,
        vec![101, 102],
        vec![201, 202],
        vec![0.9, 0.8],
        vec![true, true],
        "v2.26.0-omega",
        vec![],
        None,
        ts,
    )
}

// ----------------------------------------------------------
// 顶层 API 可达性
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    let ledger = TokenLedger::new();
    assert!(ledger.is_empty());
    assert_eq!(ledger.len(), 0);
}

// ----------------------------------------------------------
// 证据完整性红线（"Token Ledger 不可丢失"）
// ----------------------------------------------------------

#[test]
fn evidence_integrity_duplicate_rejected() {
    let ledger = TokenLedger::new();
    ledger
        .append(entry("e1", "s1", "i1", 1_000))
        .expect("首次追加");
    // 重复 ID 重放拒绝（防篡改）
    let err = ledger
        .append(entry("e1", "s1", "i1", 1_000))
        .expect_err("重复拒绝");
    assert!(matches!(err, LedgerError::DuplicateEntry(_)));
    // 完整性报告: 唯一 ID = 条目数，零重复
    let report = ledger.integrity_check();
    assert_eq!(report.total_entries, report.unique_ids);
    assert_eq!(report.duplicate_ids, 0);
}

#[test]
fn evidence_integrity_multiple_sessions_instances() {
    let ledger = TokenLedger::new();
    ledger.append(entry("e1", "s1", "i1", 1_000)).expect("追加");
    ledger.append(entry("e2", "s1", "i2", 2_000)).expect("追加");
    ledger.append(entry("e3", "s2", "i1", 3_000)).expect("追加");
    let report = ledger.integrity_check();
    assert_eq!(report.sessions, 2);
    assert_eq!(report.instances, 2);
    assert_eq!(report.total_entries, 3);
}

// ----------------------------------------------------------
// 导出通道（WAL 落盘 / v4.0 训练数据面）
// ----------------------------------------------------------

#[test]
fn export_json_and_msgpack_roundtrip() {
    let ledger = TokenLedger::new();
    ledger.append(entry("e1", "s1", "i1", 1_000)).expect("追加");
    ledger.append(entry("e2", "s1", "i1", 2_000)).expect("追加");
    let exported = ledger.export_entries();

    // JSON 导出（可读审计形态）
    let json = serde_json::to_string(&exported).expect("JSON 序列化失败");
    let back_json: Vec<TokenLedgerEntry> = serde_json::from_str(&json).expect("JSON 反序列化失败");
    assert_eq!(back_json, exported);

    // MsgPack 导出（训练数据面二进制形态，体积更小）
    let bytes = rmp_serde::to_vec(&exported).expect("MsgPack 序列化失败");
    let back_msg: Vec<TokenLedgerEntry> =
        rmp_serde::from_slice(&bytes).expect("MsgPack 反序列化失败");
    assert_eq!(back_msg, exported);
    assert!(bytes.len() < json.len(), "MsgPack 应小于 JSON");
}

// ----------------------------------------------------------
// 跨 session/instance 回溯
// ----------------------------------------------------------

#[test]
fn cross_session_instance_traceability() {
    let ledger = TokenLedger::new();
    // 同一 session 跨 instance + 同一 instance 跨 session
    ledger.append(entry("e1", "s1", "i1", 1_000)).expect("追加");
    ledger.append(entry("e2", "s1", "i2", 2_000)).expect("追加");
    ledger.append(entry("e3", "s2", "i1", 3_000)).expect("追加");
    // session 回溯
    assert_eq!(ledger.session_entry_ids("s1"), vec!["e1", "e2"]);
    // instance 回溯
    assert_eq!(ledger.instance_entry_ids("i1"), vec!["e1", "e3"]);
}

// ----------------------------------------------------------
// proptest: 唯一 ID 追加不变量
// ----------------------------------------------------------

proptest! {
    /// 任意唯一 ID 序列追加后，账本完整性恒成立
    #[test]
    fn unique_ids_append_preserves_integrity(
        n in 1usize..50,
        ts in 0u64..10_000,
    ) {
        let ledger = TokenLedger::new();
        for i in 0..n {
            ledger.append(entry(&format!("e{i}"), "s1", "i1", ts + i as u64))
                .expect("唯一 ID 追加成功");
        }
        let report = ledger.integrity_check();
        prop_assert_eq!(report.total_entries, n as u64);
        prop_assert_eq!(report.unique_ids, n as u64);
        prop_assert_eq!(report.duplicate_ids, 0);
        // 时间序导出长度一致
        prop_assert_eq!(ledger.export_entries().len(), n);
    }
}

// ----------------------------------------------------------
// 并发追加（多线程）
// ----------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_append_integrity_holds() {
    let ledger = TokenLedger::new();
    let mut handles = Vec::new();
    for w in 0..6 {
        let ledger = ledger.clone();
        handles.push(tokio::spawn(async move {
            for i in 0..30 {
                ledger
                    .append(entry(
                        &format!("w{w}-e{i}"),
                        &format!("s{w}"),
                        "i-main",
                        i as u64,
                    ))
                    .expect("唯一 ID 追加成功");
            }
        }));
    }
    for h in handles {
        h.await.expect("并发任务不失败");
    }
    assert_eq!(ledger.len(), 180);
    let report = ledger.integrity_check();
    assert_eq!(report.total_entries, 180);
    assert_eq!(report.unique_ids, 180);
    assert_eq!(report.duplicate_ids, 0);
    assert_eq!(report.sessions, 6);
}
