//! MCP Mesh 配置定义
//!
//! 控制 2PC 事务超时、心跳探活阈值与注册表容量。
//! 配置项默认值经过权衡,适合大多数 L10 Interface 层分布式事务场景。

use std::path::PathBuf;

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

    /// 是否启用 WAL 持久化(Task 0.7 v2.9.0-omega)
    ///
    /// 默认 true。false 时禁用 WAL,2PC 协调者崩溃后无法恢复未完成事务,
    /// 适合纯内存测试场景或可容忍数据丢失的开发环境。
    ///
    /// WHY 默认 true:生产环境必须启用,确保 2PC 事务持久化。
    /// 测试场景可通过 `MeshConfig { durable: false, .. }` 禁用以避免文件 IO。
    pub durable: bool,

    /// WAL 文件路径(Task 0.7 v2.9.0-omega)
    ///
    /// 默认 `~/.chimera/mcp_mesh.wal`(由 `WalStore::default_path()` 解析)。
    /// 仅在 `durable = true` 时生效。
    ///
    /// WHY Option<PathBuf>:默认值依赖运行时环境变量(HOME/USERPROFILE),
    /// 无法在 const 上下文构造,故用 Option。`None` 时由 mesh.rs 启动时解析。
    #[serde(default)]
    pub wal_path: Option<PathBuf>,

    /// 后台探活任务周期(毫秒,Task 0.7 v2.9.0-omega)
    ///
    /// 默认 60000(60s)。McpMesh 启动时 spawn 后台任务,周期遍历 `list_alive`
    /// 清理僵尸服务器(超过 `heartbeat_timeout_ms` 未心跳)。
    /// 设为 0 则禁用后台探活(仅适合测试场景)。
    pub background_probe_interval_ms: u64,
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            transaction_timeout_ms: 200,
            heartbeat_timeout_ms: 5000,
            max_retries: 2,
            max_participants: 32,
            registry_capacity: 256,
            durable: true,
            wal_path: None,
            background_probe_interval_ms: 60_000,
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
        // Task 0.7 v2.9.0-omega 新增字段
        assert!(config.durable, "durable 默认应为 true");
        assert!(config.wal_path.is_none(), "wal_path 默认应为 None");
        assert_eq!(config.background_probe_interval_ms, 60_000);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = MeshConfig {
            transaction_timeout_ms: 500,
            heartbeat_timeout_ms: 10000,
            max_retries: 3,
            max_participants: 64,
            registry_capacity: 512,
            durable: false,
            wal_path: Some(PathBuf::from("/tmp/test.wal")),
            background_probe_interval_ms: 30_000,
        };
        let json = serde_json::to_string(&config).expect("序列化失败");
        let restored: MeshConfig = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(restored.transaction_timeout_ms, 500);
        assert_eq!(restored.heartbeat_timeout_ms, 10000);
        assert_eq!(restored.max_retries, 3);
        assert_eq!(restored.max_participants, 64);
        assert_eq!(restored.registry_capacity, 512);
        assert!(!restored.durable);
        assert_eq!(restored.wal_path, Some(PathBuf::from("/tmp/test.wal")));
        assert_eq!(restored.background_probe_interval_ms, 30_000);
    }

    #[test]
    fn test_config_serde_backward_compatible_wal_path_default() {
        // 旧配置文件不含 wal_path 字段,反序列化时应使用默认值 None
        let old_json = r#"{
            "transaction_timeout_ms": 200,
            "heartbeat_timeout_ms": 5000,
            "max_retries": 2,
            "max_participants": 32,
            "registry_capacity": 256,
            "durable": true,
            "background_probe_interval_ms": 60000
        }"#;
        let restored: MeshConfig = serde_json::from_str(old_json).expect("反序列化失败");
        assert!(restored.wal_path.is_none(), "缺失 wal_path 应回退为 None");
    }

    #[test]
    fn test_config_clone() {
        let config = MeshConfig::default();
        let cloned = config.clone();
        assert_eq!(config.transaction_timeout_ms, cloned.transaction_timeout_ms);
        assert_eq!(config.heartbeat_timeout_ms, cloned.heartbeat_timeout_ms);
    }
}
