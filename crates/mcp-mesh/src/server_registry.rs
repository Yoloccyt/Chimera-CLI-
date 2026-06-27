//! 服务器注册表 — DashMap-based 并发安全注册与心跳探活
//!
//! 对应架构层:L10 Interface
//!
//! ## 设计要点
//! - 基于 `DashMap<String, MeshServer>` 实现 O(1) 分片查找
//! - 心跳探活通过比较 `last_heartbeat` 与当前时间差,无锁原子读
//! - `MeshServer::is_alive` 方法下沉探活逻辑,使调用方可独立判断单服务器存活
//! - 注册表容量不强制上限(避免 register 时持锁遍历),由调用方管理;
//!   `capacity()` 仅作为指标暴露,`RegistryFull` 错误由调用方判断后抛出

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::error::McpError;

/// MCP 网格服务器 — 注册表中的一条服务器记录
///
/// `last_heartbeat` 由 `ServerRegistry::heartbeat` 周期性更新,
/// `is_alive` 据此判断服务器是否在线。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MeshServer {
    /// 服务器唯一标识
    pub server_id: String,
    /// 服务器端点(如 "127.0.0.1:8080")
    pub endpoint: String,
    /// 服务器声明的能力 ID 列表(如 ["shell-exec", "code-gen"])
    pub capabilities: Vec<String>,
    /// 最近一次心跳时刻(UTC),注册时初始化为当前时间
    pub last_heartbeat: DateTime<Utc>,
}

impl MeshServer {
    /// 创建新服务器记录,`last_heartbeat` 默认为当前 UTC 时间
    pub fn new(
        server_id: impl Into<String>,
        endpoint: impl Into<String>,
        capabilities: Vec<String>,
    ) -> Self {
        Self {
            server_id: server_id.into(),
            endpoint: endpoint.into(),
            capabilities,
            last_heartbeat: Utc::now(),
        }
    }

    /// 判断服务器是否存活(心跳未超时)
    ///
    /// - `timeout_ms`:心跳超时阈值(毫秒),超过此时间未心跳视为离线
    pub fn is_alive(&self, timeout_ms: u64) -> bool {
        let elapsed = (Utc::now() - self.last_heartbeat).num_milliseconds();
        elapsed >= 0 && elapsed as u64 <= timeout_ms
    }

    /// 更新心跳时刻为当前 UTC 时间
    pub fn touch_heartbeat(&mut self) {
        self.last_heartbeat = Utc::now();
    }
}

/// 服务器注册表 — 并发安全的 MCP 服务器注册与心跳管理
///
/// 基于 `DashMap<String, MeshServer>`,支持 O(1) 查找与并发读写。
/// 不同 `server_id` 的读写互不阻塞(DashMap 分片锁)。
pub struct ServerRegistry {
    servers: DashMap<String, MeshServer>,
    capacity: usize,
}

impl ServerRegistry {
    /// 创建指定容量的注册表
    ///
    /// `capacity` 用于调用方判断是否已满(`RegistryFull`),不强制上限。
    pub fn new(capacity: usize) -> Self {
        Self {
            servers: DashMap::new(),
            capacity,
        }
    }

    /// 注册服务器 — 若 server_id 已存在则覆盖(更新)
    ///
    /// # 错误
    /// - `RegistryFull`:容量为 0
    /// - `ServerNotFound`:内部不应触发(保留以扩展)
    pub fn register(&self, server: MeshServer) -> Result<(), McpError> {
        if self.capacity == 0 {
            return Err(McpError::RegistryFull { capacity: 0 });
        }
        let key = server.server_id.clone();
        self.servers.insert(key, server);
        Ok(())
    }

    /// 注销服务器,返回是否成功移除
    pub fn unregister(&self, server_id: &str) -> Result<(), McpError> {
        if self.servers.remove(server_id).is_none() {
            return Err(McpError::ServerNotFound {
                server_id: server_id.to_string(),
            });
        }
        Ok(())
    }

    /// 更新服务器心跳时刻 — 失败返回 `ServerNotFound`
    pub fn heartbeat(&self, server_id: &str) -> Result<(), McpError> {
        match self.servers.get_mut(server_id) {
            Some(mut entry) => {
                entry.value_mut().touch_heartbeat();
                Ok(())
            }
            None => Err(McpError::ServerNotFound {
                server_id: server_id.to_string(),
            }),
        }
    }

    /// 查询服务器(clone),返回 `None` 表示未注册
    pub fn get(&self, server_id: &str) -> Option<MeshServer> {
        self.servers.get(server_id).map(|r| r.clone())
    }

    /// 列出所有存活服务器 ID(心跳未超时)
    pub fn list_alive(&self, timeout_ms: u64) -> Vec<String> {
        self.servers
            .iter()
            .filter(|r| r.value().is_alive(timeout_ms))
            .map(|r| r.key().clone())
            .collect()
    }

    /// 列出所有已注册服务器 ID(不关心心跳)
    pub fn list_all(&self) -> Vec<String> {
        self.servers.iter().map(|r| r.key().clone()).collect()
    }

    /// 当前注册服务器数量
    pub fn len(&self) -> usize {
        self.servers.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    /// 注册表容量上限
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration as StdDuration;

    fn make_server(id: &str) -> MeshServer {
        MeshServer::new(id, format!("127.0.0.1:{id}"), vec!["cap-1".into()])
    }

    #[test]
    fn test_server_new_initializes_heartbeat() {
        let server = make_server("1");
        // 刚创建的服务器应该 alive
        assert!(server.is_alive(5000));
    }

    #[test]
    fn test_server_is_alive_after_timeout() {
        let mut server = make_server("1");
        // 模拟过期心跳
        server.last_heartbeat = Utc::now() - chrono::Duration::seconds(60);
        assert!(!server.is_alive(5000));
    }

    #[test]
    fn test_registry_register_and_get() {
        let registry = ServerRegistry::new(16);
        registry.register(make_server("s-1")).expect("注册失败");
        registry.register(make_server("s-2")).expect("注册失败");

        assert_eq!(registry.len(), 2);
        assert!(registry.get("s-1").is_some());
        assert!(registry.get("unknown").is_none());
    }

    #[test]
    fn test_registry_register_overwrites() {
        let registry = ServerRegistry::new(16);
        registry.register(make_server("s-1")).expect("注册失败");
        // 用相同 ID 重新注册(更新 endpoint)
        let updated = MeshServer::new("s-1", "127.0.0.1:9999", vec![]);
        registry.register(updated).expect("注册失败");
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry.get("s-1").expect("应存在").endpoint,
            "127.0.0.1:9999"
        );
    }

    #[test]
    fn test_registry_unregister() {
        let registry = ServerRegistry::new(16);
        registry.register(make_server("s-1")).expect("注册失败");
        registry.unregister("s-1").expect("注销失败");
        assert_eq!(registry.len(), 0);

        // 重复注销应失败
        let err = registry.unregister("s-1").unwrap_err();
        assert!(matches!(err, McpError::ServerNotFound { .. }));
    }

    #[test]
    fn test_registry_heartbeat_updates_timestamp() {
        let registry = ServerRegistry::new(16);
        let mut server = make_server("s-1");
        // 设置为很久以前
        server.last_heartbeat = Utc::now() - chrono::Duration::seconds(60);
        registry.register(server).expect("注册失败");

        // 心跳前:不 alive
        assert!(!registry.get("s-1").expect("应存在").is_alive(5000));

        // 心跳后:alive
        registry.heartbeat("s-1").expect("心跳失败");
        assert!(registry.get("s-1").expect("应存在").is_alive(5000));
    }

    #[test]
    fn test_registry_heartbeat_unknown_server() {
        let registry = ServerRegistry::new(16);
        let err = registry.heartbeat("unknown").unwrap_err();
        assert!(matches!(err, McpError::ServerNotFound { .. }));
    }

    #[test]
    fn test_registry_list_alive_filters_dead() {
        let registry = ServerRegistry::new(16);
        registry.register(make_server("s-alive")).expect("注册失败");

        let mut dead = make_server("s-dead");
        dead.last_heartbeat = Utc::now() - chrono::Duration::seconds(60);
        registry.register(dead).expect("注册失败");

        let alive = registry.list_alive(5000);
        assert_eq!(alive.len(), 1);
        assert!(alive.contains(&"s-alive".to_string()));
        assert!(!alive.contains(&"s-dead".to_string()));
    }

    #[test]
    fn test_registry_capacity_zero_rejected() {
        let registry = ServerRegistry::new(0);
        let err = registry.register(make_server("s-1")).unwrap_err();
        assert!(matches!(err, McpError::RegistryFull { .. }));
    }

    #[test]
    fn test_registry_concurrent_register_safe() {
        // 验证 DashMap 并发安全性:多线程同时注册不同 server_id
        let registry = std::sync::Arc::new(ServerRegistry::new(256));
        let mut handles = Vec::new();
        for i in 0..16 {
            let reg = std::sync::Arc::clone(&registry);
            handles.push(thread::spawn(move || {
                let id = format!("s-{i}");
                reg.register(make_server(&id)).expect("注册失败");
            }));
        }
        for h in handles {
            h.join().expect("线程 panic");
        }
        assert_eq!(registry.len(), 16);

        // 让所有服务器经过短暂时间(但仍在心跳窗口内)
        thread::sleep(StdDuration::from_millis(10));
        let alive = registry.list_alive(5000);
        assert_eq!(alive.len(), 16);
    }

    #[test]
    fn test_server_serde_roundtrip() {
        let server = make_server("s-1");
        let json = serde_json::to_string(&server).expect("序列化失败");
        let restored: MeshServer = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(server, restored);
    }
}
