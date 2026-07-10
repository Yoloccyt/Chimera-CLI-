//! MCP Mesh 配置定义
//!
//! 控制 2PC 事务超时、心跳探活阈值与注册表容量。
//! 配置项默认值经过权衡,适合大多数 L10 Interface 层分布式事务场景。

use serde::{Deserialize, Serialize};

/// MCP Mesh 配置 — 控制事务与心跳行为
///
/// 所有字段均有合理默认值,可通过 `Default::default()` 快速创建。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshConfig {
    /// 2PC 事务总超时(毫秒)
    ///
    /// 默认 200,超过此时间未完成则触发 Abort+Rollback。
    /// WHY 200:5 服务器并发事务 p95 设计目标 ≤ 100ms,200ms 留 2x 余量;
    /// 超时即认为发生死锁或网络分区,必须回滚避免资源占用。
    pub transaction_timeout_ms: u64,

    /// 心跳探活超时(毫秒)
    ///
    /// 默认 5000,服务器超过此时间未心跳则视为离线。
    /// WHY 5000:典型分布式心跳周期 1-3s,5s 容忍 2-3 次心跳丢失;
    /// 过短导致误判,过长导致僵尸服务器占用注册表。
    pub heartbeat_timeout_ms: u64,

    /// 2PC 单阶段(prepare/commit/rollback)单服务器最大重试次数
    ///
    /// 默认 2,失败后放弃并触发回滚。
    pub max_retries: u32,

    /// 单次事务最大参与者数量
    ///
    /// 默认 32,防止过大事务导致 2PC 阻塞。
    pub max_participants: usize,

    /// 服务器注册表容量上限
    ///
    /// 默认 256,平衡内存占用与典型 MCP 网格规模。
    pub registry_capacity: usize,

    /// JSON-RPC 单请求超时(毫秒)
    ///
    /// 默认 5000,覆盖 prepare/commit/rollback/query 单次 RPC 调用。
    pub json_rpc_timeout_ms: u64,

    /// 是否启用 JSON-RPC Mock 模式(跳过真实网络,直接返回成功)
    ///
    /// 默认 true(兼容既有测试与 CI 环境)。生产环境应设为 false 启用真实网络。
    pub json_rpc_mock: bool,
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            transaction_timeout_ms: 200,
            heartbeat_timeout_ms: 5000,
            max_retries: 2,
            max_participants: 32,
            registry_capacity: 256,
            json_rpc_timeout_ms: 5000,
            json_rpc_mock: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MeshConfig::default();
        assert_eq!(config.transaction_timeout_ms, 200);
        assert_eq!(config.heartbeat_timeout_ms, 5000);
        assert_eq!(config.max_retries, 2);
        assert_eq!(config.max_participants, 32);
        assert_eq!(config.registry_capacity, 256);
        assert_eq!(config.json_rpc_timeout_ms, 5000);
        assert!(config.json_rpc_mock);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = MeshConfig {
            transaction_timeout_ms: 500,
            heartbeat_timeout_ms: 10000,
            max_retries: 3,
            max_participants: 64,
            registry_capacity: 512,
            json_rpc_timeout_ms: 10000,
            json_rpc_mock: false,
        };
        let json = serde_json::to_string(&config).expect("序列化失败");
        let restored: MeshConfig = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(restored.transaction_timeout_ms, 500);
        assert_eq!(restored.heartbeat_timeout_ms, 10000);
        assert_eq!(restored.max_retries, 3);
        assert_eq!(restored.max_participants, 64);
        assert_eq!(restored.registry_capacity, 512);
        assert_eq!(restored.json_rpc_timeout_ms, 10000);
        assert!(!restored.json_rpc_mock);
    }

    #[test]
    fn test_config_clone() {
        let config = MeshConfig::default();
        let cloned = config.clone();
        assert_eq!(config.transaction_timeout_ms, cloned.transaction_timeout_ms);
        assert_eq!(config.heartbeat_timeout_ms, cloned.heartbeat_timeout_ms);
    }
}
