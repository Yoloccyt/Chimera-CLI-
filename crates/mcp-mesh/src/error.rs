//! MCP Mesh 错误类型定义
//!
//! 遵循 §4.1 规范:库层用自定义 `thiserror` enum,不使用 `anyhow`。
//! 这些错误覆盖 MCP Mesh 在事务执行、服务器注册与超位查询中的所有失败场景。

use thiserror::Error;

/// MCP Mesh 执行错误枚举
///
/// 每个变体对应一种分布式事务或网格通信失败模式。
/// 所有错误均不可恢复(需调用者决定重试或降级)。
#[derive(Debug, Clone, Error)]
pub enum McpError {
    /// 服务器未找到:指定的 server_id 未在 registry 中注册
    ///
    /// 事务或查询的参与者列表中存在未注册的服务器 ID。
    /// 调用者应先注册缺失的服务器,或从参与者列表中移除。
    #[error("服务器未找到: {server_id}")]
    ServerNotFound {
        /// 未找到的服务器 ID
        server_id: String,
    },

    /// 服务器不可达:心跳超时,服务器已被判定离线
    ///
    /// 服务器虽然注册过,但超过 `heartbeat_timeout_ms` 未心跳,
    /// 注册表已将其标记为离线,事务无法继续。
    #[error("服务器不可达(心跳超时): {server_id}")]
    ServerUnreachable {
        /// 不可达的服务器 ID
        server_id: String,
    },

    /// 事务超时:2PC 事务超过 `transaction_timeout_ms` 未完成
    ///
    /// 对应架构红线:所有异步操作必须有超时处理,避免死锁。
    /// 超时后已自动触发 Abort+Rollback,调用方无需再回滚。
    #[error("事务超时: {transaction_id} (超时 {timeout_ms}ms)")]
    TransactionTimeout {
        /// 超时的事务 ID
        transaction_id: String,
        /// 超时阈值(毫秒)
        timeout_ms: u64,
    },

    /// 参与者过多:事务参与者数量超过 `max_participants`
    ///
    /// 防止单事务过大导致 2PC 阻塞。
    #[error("参与者过多: {actual} > {limit}")]
    TooManyParticipants {
        /// 实际参与者数量
        actual: usize,
        /// 配置的最大参与者数量
        limit: usize,
    },

    /// 事务状态转换非法:2PC 状态机不允许此转换
    ///
    /// 例如从 `Commit` 转回 `Prepare`。
    /// 通常表示内部实现错误,应记录告警并修复。
    #[error("非法状态转换: {from} -> {to}")]
    InvalidStateTransition {
        /// 源状态
        from: String,
        /// 目标状态
        to: String,
    },

    /// 配置错误:无效的配置参数
    ///
    /// 例如 `registry_capacity == 0` 或 `max_participants == 0`。
    #[error("配置错误: {reason}")]
    ConfigError {
        /// 错误原因(人类可读描述)
        reason: String,
    },

    /// 注册表已满:服务器注册时注册表已达容量上限
    ///
    /// 调用者应先注销离线服务器或增大 `registry_capacity`。
    #[error("注册表已满(capacity={capacity})")]
    RegistryFull {
        /// 当前注册表容量
        capacity: usize,
    },

    /// 纠缠链接无效:两端服务器相同或不存在
    #[error("无效纠缠链接: {reason}")]
    InvalidEntanglement {
        /// 错误原因
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_server_not_found() {
        let e = McpError::ServerNotFound {
            server_id: "s-1".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("服务器未找到"));
        assert!(msg.contains("s-1"));
    }

    #[test]
    fn test_error_display_transaction_timeout() {
        let e = McpError::TransactionTimeout {
            transaction_id: "tx-1".into(),
            timeout_ms: 200,
        };
        let msg = format!("{e}");
        assert!(msg.contains("事务超时"));
        assert!(msg.contains("tx-1"));
        assert!(msg.contains("200"));
    }

    #[test]
    fn test_error_display_too_many_participants() {
        let e = McpError::TooManyParticipants {
            actual: 64,
            limit: 32,
        };
        let msg = format!("{e}");
        assert!(msg.contains("64"));
        assert!(msg.contains("32"));
    }

    #[test]
    fn test_error_display_invalid_state_transition() {
        let e = McpError::InvalidStateTransition {
            from: "Commit".into(),
            to: "Prepare".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("Commit"));
        assert!(msg.contains("Prepare"));
    }

    #[test]
    fn test_error_clone() {
        let e = McpError::ServerNotFound {
            server_id: "s-1".into(),
        };
        let _cloned = e.clone();
    }
}
