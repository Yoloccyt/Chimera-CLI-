//! 存储层 trait 抽象（方案 E）的集成契约 — `PragmaCapable` + `apply_performance_pragmas`
//!
//! 对应 `src/storage_traits.rs`:L1 只定义 trait,L3 下游用 newtype 适配真实连接。
//! 本文件用**纯内存 mock**（零 rusqlite 依赖）验证三件事:
//! ① 泛型函数按固定顺序下发 5 条 PRAGMA 且参数逐位正确;
//! ② 任一 PRAGMA 失败立即短路（错误传播,不继续写后续项）;
//! ③ trait 可作 `&dyn` 使用（对象安全）,即 L1 消费面无需知道具体后端类型。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code

use std::cell::RefCell;

use nexus_core::{apply_performance_pragmas, NexusError, PragmaCapable};

/// 记录调用顺序的 mock 连接 — 可按 key 注入失败（模拟下游 PRAGMA 执行错误）
struct MockConn {
    calls: RefCell<Vec<(String, String)>>,
    fail_on: Option<String>,
}

impl MockConn {
    fn new() -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            fail_on: None,
        }
    }

    fn failing_on(key: &str) -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            fail_on: Some(key.to_owned()),
        }
    }

    fn record(&self, key: &str, value: String) -> Result<(), NexusError> {
        if self.fail_on.as_deref() == Some(key) {
            return Err(NexusError::SerializationError(format!(
                "PRAGMA {key} 执行失败"
            )));
        }
        self.calls.borrow_mut().push((key.to_owned(), value));
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        self.calls.borrow().iter().map(|(k, _)| k.clone()).collect()
    }
}

impl PragmaCapable for MockConn {
    fn pragma_update_string(&self, key: &str, value: &str) -> Result<(), NexusError> {
        self.record(key, value.to_owned())
    }

    fn pragma_update_int(&self, key: &str, value: i64) -> Result<(), NexusError> {
        self.record(key, value.to_string())
    }
}

#[test]
fn apply_performance_pragmas_emits_all_five_in_documented_order() {
    // 契约:五条 PRAGMA 的键/值/顺序是实现文档承诺面（调换任意两条都可能改变
    // SQLite 的行为,例如 temp_store 必须在 WAL 之后）
    let conn = MockConn::new();
    apply_performance_pragmas(&conn).expect("全成功路径必须 Ok");

    let calls = conn.calls.borrow().clone();
    let expected: [(&str, &str); 5] = [
        ("synchronous", "NORMAL"),
        ("cache_size", "-65536"),
        ("mmap_size", "268435456"),
        ("temp_store", "MEMORY"),
        ("wal_autocheckpoint", "1000"),
    ];
    assert_eq!(calls.len(), expected.len(), "必须恰好下发 5 条 PRAGMA");
    for (i, (want_key, want_val)) in expected.iter().enumerate() {
        assert_eq!(calls[i].0, *want_key, "第 {i} 条 key 不符");
        assert_eq!(calls[i].1, *want_val, "第 {i} 条 value 不符");
    }
}

#[test]
fn apply_performance_pragmas_short_circuits_on_middle_failure() {
    // 契约:失败即短路 —— 前序已下发、后序一律不再触碰（连接状态不可预测,不得续写）
    let conn = MockConn::failing_on("mmap_size");
    let err = apply_performance_pragmas(&conn).expect_err("注入失败必须返回 Err");
    assert!(
        matches!(err, NexusError::SerializationError(_)),
        "错误必须原样透传后端变体: {err:?}"
    );
    assert_eq!(
        conn.keys(),
        vec!["synchronous".to_string(), "cache_size".to_string()],
        "短路前应只下发前两条"
    );
}

#[test]
fn apply_performance_pragmas_short_circuits_on_first_failure() {
    // 契约:第一条就失败时调用记录为空（不做任何"尽力而为"的补偿写）
    let conn = MockConn::failing_on("synchronous");
    assert!(apply_performance_pragmas(&conn).is_err());
    assert!(conn.keys().is_empty(), "首个失败时不得留下任何调用");
}

#[test]
fn pragma_capable_is_object_safe_and_usable_through_dyn_reference() {
    // 契约:trait 方法只取 `&self` 且无泛型参数 → 可作 `&dyn PragmaCapable` 使用,
    // 即 L1 调用方可以完全不感知具体存储后端类型（方案 E 的核心前提）
    let conn = MockConn::new();
    let dynamic: &dyn PragmaCapable = &conn;
    dynamic
        .pragma_update_string("journal_mode", "WAL")
        .expect("dyn 调用应成功");
    dynamic
        .pragma_update_int("page_size", 4096)
        .expect("dyn 调用应成功");
    assert_eq!(
        conn.calls.borrow().as_slice(),
        &[
            ("journal_mode".to_string(), "WAL".to_string()),
            ("page_size".to_string(), "4096".to_string()),
        ]
    );
}

#[test]
fn apply_performance_pragmas_succeeds_on_repeat_calls() {
    // 契约:函数无内部状态,重复调用等价下发（可安全地在每次建连后调用）
    let conn = MockConn::new();
    apply_performance_pragmas(&conn).expect("首次调用应成功");
    apply_performance_pragmas(&conn).expect("再次调用应成功");
    assert_eq!(conn.calls.borrow().len(), 10, "两次调用应各下发 5 条");
}
