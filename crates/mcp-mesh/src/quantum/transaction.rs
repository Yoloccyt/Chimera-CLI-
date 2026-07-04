//! 量子事务状态机 — 2PC 占位实现
//!
//! 对应架构层:L10 Interface
//!
//! ## 状态机
//! ```text
//! Init ──► Prepare ──► Commit    (全部参与者 ACK)
//!            │
//!            └──► Abort ──► Rollback  (任一参与者失败)
//! ```
//! 非法转换(如 Commit → Prepare)返回 `InvalidStateTransition`。
//!
//! ## 2PC 占位说明
//! 当前实现为 in-process mock:`prepare_phase` / `commit_phase` / `rollback_phase`
//! 用 `tokio::time::sleep` 模拟网络往返延迟,不涉及真实网络 IO。
//! 未来可替换为真实 MCP 协议调用,状态机契约保持不变。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::McpError;

/// 2PC 事务状态
///
/// 状态转换规则:
/// - `Init → Prepare`:事务开始,进入 prepare 阶段
/// - `Prepare → Commit`:所有参与者 prepare ACK
/// - `Prepare → Abort`:任一参与者 prepare 失败
/// - `Abort → Rollback`:回滚完成
/// - 其他转换均非法
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransactionState {
    /// 初始状态:事务已创建,未开始 prepare
    Init,
    /// Prepare 阶段:正在向所有参与者发送 prepare 请求
    Prepare,
    /// Commit 阶段:所有参与者已 ACK,正在提交
    Commit,
    /// Abort 阶段:prepare 失败,正在中止
    Abort,
    /// Rollback 阶段:已回滚,事务终结
    Rollback,
}

impl TransactionState {
    /// 状态名称(用于日志与错误信息)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Init => "Init",
            Self::Prepare => "Prepare",
            Self::Commit => "Commit",
            Self::Abort => "Abort",
            Self::Rollback => "Rollback",
        }
    }

    /// 判断是否为终态(不可再转换)
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Commit | Self::Rollback)
    }
}

impl std::fmt::Display for TransactionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 量子事务 — 跨多服务器的原子提交单元
///
/// 携带事务 ID、参与者列表、当前状态与创建时间。
/// 状态转换通过 `transition` 方法,自动校验合法性。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuantumTransaction {
    /// 事务 ID(UUIDv7 字符串,时间有序便于追溯)
    pub transaction_id: String,
    /// 参与者服务器 ID 列表
    pub participant_servers: Vec<String>,
    /// 当前事务状态
    pub state: TransactionState,
    /// 事务创建时刻(UTC)
    pub created_at: DateTime<Utc>,
}

impl QuantumTransaction {
    /// 创建新事务(Init 状态),自动生成 UUIDv7 事务 ID
    ///
    /// - `participant_servers`:参与者服务器 ID 列表
    pub fn new(participant_servers: Vec<String>) -> Self {
        Self::with_id(Uuid::now_v7().to_string(), participant_servers)
    }

    /// 使用指定事务 ID 创建事务(主要用于测试与跨进程恢复)
    pub fn with_id(transaction_id: impl Into<String>, participant_servers: Vec<String>) -> Self {
        Self {
            transaction_id: transaction_id.into(),
            participant_servers,
            state: TransactionState::Init,
            created_at: Utc::now(),
        }
    }

    /// 状态转换 — 校验合法性,非法转换返回 `InvalidStateTransition`
    ///
    /// 合法转换路径:
    /// - `Init → Prepare`
    /// - `Prepare → Commit`
    /// - `Prepare → Abort`
    /// - `Abort → Rollback`
    pub fn transition(&mut self, new_state: TransactionState) -> Result<(), McpError> {
        let allowed = matches!(
            (self.state, new_state),
            (TransactionState::Init, TransactionState::Prepare)
                | (TransactionState::Prepare, TransactionState::Commit)
                | (TransactionState::Prepare, TransactionState::Abort)
                | (TransactionState::Abort, TransactionState::Rollback)
        );

        if !allowed {
            return Err(McpError::InvalidStateTransition {
                from: self.state.to_string(),
                to: new_state.to_string(),
            });
        }

        self.state = new_state;
        Ok(())
    }

    /// 判断事务是否已终结(Commit 或 Rollback)
    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }

    /// 参与者数量
    pub fn participant_count(&self) -> usize {
        self.participant_servers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_new_init_state() {
        let tx = QuantumTransaction::new(vec!["s-1".into(), "s-2".into()]);
        assert_eq!(tx.state, TransactionState::Init);
        assert_eq!(tx.participant_count(), 2);
        assert!(!tx.transaction_id.is_empty());
        assert!(!tx.is_terminal());
    }

    #[test]
    fn test_state_machine_legal_transitions() {
        let mut tx = QuantumTransaction::new(vec!["s-1".into()]);

        tx.transition(TransactionState::Prepare)
            .expect("Init→Prepare");
        assert_eq!(tx.state, TransactionState::Prepare);

        tx.transition(TransactionState::Commit)
            .expect("Prepare→Commit");
        assert_eq!(tx.state, TransactionState::Commit);
        assert!(tx.is_terminal());
    }

    #[test]
    fn test_state_machine_abort_rollback_path() {
        let mut tx = QuantumTransaction::new(vec!["s-1".into()]);

        tx.transition(TransactionState::Prepare)
            .expect("Init→Prepare");
        tx.transition(TransactionState::Abort)
            .expect("Prepare→Abort");
        tx.transition(TransactionState::Rollback)
            .expect("Abort→Rollback");

        assert_eq!(tx.state, TransactionState::Rollback);
        assert!(tx.is_terminal());
    }

    #[test]
    fn test_state_machine_illegal_transition_rejected() {
        let mut tx = QuantumTransaction::new(vec!["s-1".into()]);
        // Init → Commit 非法(跳过 Prepare)
        let err = tx.transition(TransactionState::Commit).unwrap_err();
        assert!(matches!(err, McpError::InvalidStateTransition { .. }));

        // Init → Abort 非法
        let err = tx.transition(TransactionState::Abort).unwrap_err();
        assert!(matches!(err, McpError::InvalidStateTransition { .. }));

        // Init → Rollback 非法
        let err = tx.transition(TransactionState::Rollback).unwrap_err();
        assert!(matches!(err, McpError::InvalidStateTransition { .. }));
    }

    #[test]
    fn test_state_machine_terminal_cannot_transition() {
        let mut tx = QuantumTransaction::new(vec!["s-1".into()]);
        tx.transition(TransactionState::Prepare)
            .expect("Init→Prepare");
        tx.transition(TransactionState::Commit)
            .expect("Prepare→Commit");

        // Commit 是终态,不能再转换
        let err = tx.transition(TransactionState::Prepare).unwrap_err();
        assert!(matches!(err, McpError::InvalidStateTransition { .. }));
    }

    #[test]
    fn test_state_serde_roundtrip() {
        let tx = QuantumTransaction::with_id("tx-1", vec!["s-1".into(), "s-2".into()]);
        let json = serde_json::to_string(&tx).expect("序列化失败");
        let restored: QuantumTransaction = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(tx, restored);
    }

    #[test]
    fn test_state_as_str() {
        assert_eq!(TransactionState::Init.as_str(), "Init");
        assert_eq!(TransactionState::Prepare.as_str(), "Prepare");
        assert_eq!(TransactionState::Commit.as_str(), "Commit");
        assert_eq!(TransactionState::Abort.as_str(), "Abort");
        assert_eq!(TransactionState::Rollback.as_str(), "Rollback");
    }
}
