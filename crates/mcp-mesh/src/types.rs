//! MCP Mesh 核心类型定义 — 事务结果与辅助类型
//!
//! 对应架构层:L10 Interface
//!
//! ## 类型关系
//! - `TransactionResult`:量子事务执行结果,携带成功标志、延迟与已提交服务器列表
//! - 此模块仅放跨模块共享的聚合类型;各子系统的核心类型(如 `MeshServer`、
//!   `QuantumTransaction`)定义在对应功能模块中,通过 `lib.rs` 重导出统一访问

use serde::{Deserialize, Serialize};

/// 量子事务执行结果 — 描述一次 2PC 事务的最终产出
///
/// 由 `McpMesh::execute_transaction` 返回,记录事务成功与否、总耗时与
/// 实际提交的服务器列表(失败时为空)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransactionResult {
    /// 事务 ID(UUIDv7 字符串,与 `QuantumTransaction::transaction_id` 一致)
    pub transaction_id: String,
    /// 事务是否成功提交(true=Commit,false=Abort+Rollback)
    pub success: bool,
    /// 事务总耗时(毫秒,从创建到 Commit/Rollback 完成)
    pub latency_ms: u64,
    /// 实际提交的服务器 ID 列表(success=false 时为空 Vec)
    pub committed_servers: Vec<String>,
}

impl TransactionResult {
    /// 创建事务结果
    pub fn new(
        transaction_id: impl Into<String>,
        success: bool,
        latency_ms: u64,
        committed_servers: Vec<String>,
    ) -> Self {
        Self {
            transaction_id: transaction_id.into(),
            success,
            latency_ms,
            committed_servers,
        }
    }

    /// 失败事务结果的便捷构造(committed_servers 为空)
    pub fn failed(transaction_id: impl Into<String>, latency_ms: u64) -> Self {
        Self {
            transaction_id: transaction_id.into(),
            success: false,
            latency_ms,
            committed_servers: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_result_new_success() {
        let r = TransactionResult::new("tx-1", true, 50, vec!["s-1".into(), "s-2".into()]);
        assert_eq!(r.transaction_id, "tx-1");
        assert!(r.success);
        assert_eq!(r.latency_ms, 50);
        assert_eq!(r.committed_servers.len(), 2);
    }

    #[test]
    fn test_transaction_result_failed_helper() {
        let r = TransactionResult::failed("tx-2", 30);
        assert!(!r.success);
        assert!(r.committed_servers.is_empty());
        assert_eq!(r.latency_ms, 30);
    }

    #[test]
    fn test_transaction_result_serde_roundtrip() {
        let r = TransactionResult::new("tx-3", true, 100, vec!["s-a".into()]);
        let json = serde_json::to_string(&r).expect("序列化失败");
        let restored: TransactionResult = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(r, restored);
    }
}
