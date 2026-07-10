//! L1 存储层 trait 抽象 — 方案 E(PragmaCapable trait + 泛型函数)
//!
//! 对应架构层:L1 Core
//!
//! # 设计决策(WHY 方案 E)
//! v1.1.0 路线图 F2 要求将 `rusqlite` 依赖从 nexus-core(L1)下沉到下游 crate。
//! 原因:L1 Core 是基础层,不应直接依赖具体存储后端(`rusqlite::Connection`),
//! 否则上层 crate 通过 L1 间接耦合 rusqlite,违反"最小依赖"原则。
//!
//! 方案 E 的核心思想:**L1 仅定义 trait 契约,不引用 rusqlite 任何类型**。
//! 下游 crate(如 cmt-tiering、mlc-engine)在自己代码中定义 newtype wrapper
//! (`pub struct PragmaConn<'a>(pub &'a rusqlite::Connection)`)并 impl
//! `PragmaCapable for PragmaConn<'a>`,从而把 rusqlite 依赖下沉到真正使用它的层。
//! WHY newtype wrapper:Rust coherence 规则禁止两个 crate 同时 impl 同一 trait
//! for 同一 type(详见 ADR-006 方案 E 实施修正)。
//!
//! # 历史说明
//! 原 `sqlite_pragma.rs` 已在 F2.3 阶段删除,逻辑全部迁移到本文件的
//! `apply_performance_pragmas<T: PragmaCapable>` 泛型函数。L1 不再依赖 rusqlite。
//!
//! # 不引用 rusqlite 的硬约束
//! 本文件**绝不**出现 `use rusqlite::...` 或 `rusqlite::` 任何引用。
//! 这是方案 E 的核心 —— L1 trait 不依赖 rusqlite 类型,错误类型用
//! `NexusError`(L1 自有类型,F2.3 已移除 `SqliteError` 变体,L1 不再依赖 rusqlite)。

use crate::error::NexusError;

/// PRAGMA 能力抽象 — 由下游 crate 为具体存储连接实现
///
/// WHY trait 抽象:L1 Core 不能依赖 `rusqlite::Connection` 具体类型,
/// 通过 trait 把"能执行 PRAGMA"这一能力抽象出来,L1 仅消费 trait 对象。
///
/// 下游实现示例(在 cmt-tiering / mlc-engine 等使用 rusqlite 的 crate 中):
/// WHY newtype wrapper:Rust coherence 规则禁止两个 crate 同时 impl
/// 同一 trait for 同一 type,故各 crate 定义独立 newtype(详见 ADR-006)。
/// ```ignore
/// use nexus_core::{NexusError, PragmaCapable};
///
/// pub struct PragmaConn<'a>(pub &'a rusqlite::Connection);
///
/// impl<'a> PragmaCapable for PragmaConn<'a> {
///     fn pragma_update_string(&self, key: &str, value: &str) -> Result<(), NexusError> {
///         self.0.pragma_update(None, key, value)
///             .map_err(|e| NexusError::SerializationError(format!("SQLite PRAGMA {key}={value} 失败: {e}")))
///     }
///     fn pragma_update_int(&self, key: &str, value: i64) -> Result<(), NexusError> {
///         self.0.pragma_update(None, key, value)
///             .map_err(|e| NexusError::SerializationError(format!("SQLite PRAGMA {key}={value} 失败: {e}")))
///     }
/// }
/// ```
pub trait PragmaCapable {
    /// 以字符串值更新 PRAGMA(如 `synchronous=NORMAL`)
    fn pragma_update_string(&self, key: &str, value: &str) -> Result<(), NexusError>;

    /// 以整数值更新 PRAGMA(如 `cache_size=-65536`)
    fn pragma_update_int(&self, key: &str, value: i64) -> Result<(), NexusError>;
}

/// 应用 SQLite 性能优化 PRAGMA(连接级,影响所有附加数据库)
///
/// 泛型约束 `T: PragmaCapable` —— L1 不依赖具体存储类型,任何实现该 trait
/// 的连接(rusqlite::Connection / 未来其他后端)都可调用。
///
/// 必须在 `journal_mode=WAL` 设置之后调用(部分 PRAGMA 依赖 WAL 模式)。
///
/// # PRAGMA 列表
/// - `synchronous=NORMAL`:WAL 模式下 NORMAL 足够安全,避免每次提交 fsync
/// - `cache_size=-65536`:负值表示 KB,64MB 页缓存(默认 2MB)
/// - `mmap_size=268435456`:256MB 内存映射 I/O,减少 read 系统调用
/// - `temp_store=MEMORY`:临时表与索引存内存,避免磁盘临时文件
/// - `wal_autocheckpoint=1000`:每 1000 页自动 checkpoint(默认 1000)
///
/// # 错误传播
/// 任一 PRAGMA 设置失败时立即返回错误(短路),不继续执行后续 PRAGMA。
/// 调用方应将错误视为致命错误(数据库连接状态不可预测)。
pub fn apply_performance_pragmas<T: PragmaCapable>(conn: &T) -> Result<(), NexusError> {
    conn.pragma_update_string("synchronous", "NORMAL")?;
    conn.pragma_update_int("cache_size", -65536)?;
    conn.pragma_update_int("mmap_size", 268435456)?;
    conn.pragma_update_string("temp_store", "MEMORY")?;
    conn.pragma_update_int("wal_autocheckpoint", 1000)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Mock 实现 — 记录所有 PRAGMA 调用(key, value 字符串化),支持注入失败
    ///
    /// 不依赖 rusqlite,纯 Rust 结构,验证 trait 抽象的通用性。
    /// 使用 `RefCell` 内部可变性,因为 trait 方法签名是 `&self`(非 `&mut self`)
    /// —— 这与 rusqlite::Connection 的 `pragma_update(&self, ...)` 签名一致。
    struct MockPragmaCapable {
        /// 按顺序记录所有成功调用 (key, value 字符串化)
        calls: RefCell<Vec<(String, String)>>,
        /// 注入失败:匹配此 key 时返回 Err,模拟 PRAGMA 设置失败
        fail_on: Option<String>,
    }

    impl MockPragmaCapable {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                fail_on: None,
            }
        }

        /// 设置在某个 key 上失败(模拟下游 PRAGMA 执行错误)
        fn with_failure(key: &str) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                fail_on: Some(key.to_string()),
            }
        }

        /// 取出调用记录快照(便于断言)
        fn calls_snapshot(&self) -> Vec<(String, String)> {
            self.calls.borrow().clone()
        }
    }

    impl PragmaCapable for MockPragmaCapable {
        fn pragma_update_string(&self, key: &str, value: &str) -> Result<(), NexusError> {
            if self.fail_on.as_deref() == Some(key) {
                return Err(NexusError::SerializationError(format!(
                    "mock failure on pragma: {key}"
                )));
            }
            self.calls
                .borrow_mut()
                .push((key.to_string(), value.to_string()));
            Ok(())
        }

        fn pragma_update_int(&self, key: &str, value: i64) -> Result<(), NexusError> {
            if self.fail_on.as_deref() == Some(key) {
                return Err(NexusError::SerializationError(format!(
                    "mock failure on pragma: {key}"
                )));
            }
            self.calls
                .borrow_mut()
                .push((key.to_string(), value.to_string()));
            Ok(())
        }
    }

    /// 测试 1:apply_performance_pragmas 正确按顺序调用全部 5 个 PRAGMA
    ///
    /// 验证:
    /// - 调用次数 = 5
    /// - 调用顺序与参数完全匹配预期
    #[test]
    fn test_apply_performance_pragmas_calls_all_five_in_order() {
        let mock = MockPragmaCapable::new();

        // 执行泛型函数,传入 mock 实现
        let result = apply_performance_pragmas(&mock);

        // 应当成功
        assert!(result.is_ok(), "expected Ok, got {:?}", result);

        // 取出调用记录,验证顺序与参数
        let calls = mock.calls_snapshot();
        assert_eq!(
            calls.len(),
            5,
            "应当调用 5 个 PRAGMA,实际调用 {} 次",
            calls.len()
        );

        // 顺序与参数严格断言(与函数实现一一对应)
        // WHY: 用数组替代 vec!,避免不必要的堆分配(clippy::useless_vec)
        let expected = [
            ("synchronous", "NORMAL"),
            ("cache_size", "-65536"),
            ("mmap_size", "268435456"),
            ("temp_store", "MEMORY"),
            ("wal_autocheckpoint", "1000"),
        ];
        for (i, (exp_key, exp_val)) in expected.iter().enumerate() {
            assert_eq!(
                calls[i].0, *exp_key,
                "第 {} 次调用的 key 应为 {}",
                i, exp_key
            );
            assert_eq!(
                calls[i].1, *exp_val,
                "第 {} 次调用的 value 应为 {}",
                i, exp_val
            );
        }
    }

    /// 测试 2:错误传播 —— mock 在第 3 个 PRAGMA(mmap_size)失败时,
    /// 泛型函数应短路返回 Err,且不再调用后续 PRAGMA
    #[test]
    fn test_apply_performance_pragmas_propagates_error_and_short_circuits() {
        let mock = MockPragmaCapable::with_failure("mmap_size");

        let result = apply_performance_pragmas(&mock);

        // 应返回 Err
        assert!(result.is_err(), "expected Err, got {:?}", result);
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("mmap_size"),
            "错误信息应包含 mmap_size,实际: {err_msg}"
        );

        // 短路验证:前 2 个 PRAGMA 已调用,后 3 个未调用
        let calls = mock.calls_snapshot();
        assert_eq!(
            calls.len(),
            2,
            "短路语义:应当在 mmap_size 失败前已调用 2 个,实际 {} 个",
            calls.len()
        );
        assert_eq!(calls[0].0, "synchronous");
        assert_eq!(calls[1].0, "cache_size");
    }

    /// 测试 3:第一个 PRAGMA 就失败 —— 验证 0 调用后立即返回 Err
    #[test]
    fn test_apply_performance_pragmas_fails_on_first_pragma() {
        let mock = MockPragmaCapable::with_failure("synchronous");

        let result = apply_performance_pragmas(&mock);

        assert!(result.is_err());

        let calls = mock.calls_snapshot();
        assert!(
            calls.is_empty(),
            "第一个就失败,调用记录应为空,实际 {} 个",
            calls.len()
        );
    }

    /// 测试 4:最后一个 PRAGMA 失败 —— 验证前 4 个已调用,第 5 个失败
    #[test]
    fn test_apply_performance_pragmas_fails_on_last_pragma() {
        let mock = MockPragmaCapable::with_failure("wal_autocheckpoint");

        let result = apply_performance_pragmas(&mock);

        assert!(result.is_err());

        let calls = mock.calls_snapshot();
        assert_eq!(
            calls.len(),
            4,
            "第 5 个 PRAGMA 失败,前 4 个应已调用,实际 {} 个",
            calls.len()
        );
        // 验证前 4 个 key 顺序
        assert_eq!(calls[0].0, "synchronous");
        assert_eq!(calls[1].0, "cache_size");
        assert_eq!(calls[2].0, "mmap_size");
        assert_eq!(calls[3].0, "temp_store");
    }
}
