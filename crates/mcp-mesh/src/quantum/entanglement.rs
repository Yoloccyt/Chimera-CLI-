//! 纠缠链接 — 服务器间状态同步策略
//!
//! 对应架构层:L10 Interface
//!
//! ## 量子纠缠语义
//! 两个服务器建立"纠缠链接"后,一端状态变更需按 `SyncStrategy` 同步至另一端:
//! - `Eager`:立即同步(强一致,延迟高)
//! - `Lazy`:周期同步(最终一致,延迟低)
//! - `BestEffort`:尽力同步(失败不重试,适合低优先级状态)
//!
//! 当前实现为 in-process 注册表,真实同步逻辑由调用方根据 strategy 决定。

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::error::McpError;

/// 同步策略 — 控制纠缠链接两端的状态同步时机
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SyncStrategy {
    /// 立即同步:状态变更后立刻推送至对端(强一致,延迟高)
    Eager,
    /// 周期同步:按固定周期批量同步(最终一致,延迟低)
    Lazy,
    /// 尽力同步:推送失败不重试(适合低优先级状态)
    BestEffort,
}

impl SyncStrategy {
    /// 策略名称(用于日志与序列化)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Eager => "Eager",
            Self::Lazy => "Lazy",
            Self::BestEffort => "BestEffort",
        }
    }
}

impl std::fmt::Display for SyncStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 纠缠链接 — 描述两个服务器间的状态同步关系
///
/// `linked_servers` 的两个元素顺序无关(对称链接),但构造时会规范化为
/// 字典序较小的在前,便于 `EntanglementManager` 去重。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntanglementLink {
    /// 链接的两端服务器 ID(字典序规范化:(min, max))
    pub linked_servers: (String, String),
    /// 同步策略
    pub sync_strategy: SyncStrategy,
}

impl EntanglementLink {
    /// 创建纠缠链接 — 自动规范化服务器顺序(字典序小的在前)
    ///
    /// # 错误
    /// - `InvalidEntanglement`:两端服务器 ID 相同
    pub fn new(
        a: impl Into<String>,
        b: impl Into<String>,
        sync_strategy: SyncStrategy,
    ) -> Result<Self, McpError> {
        let a = a.into();
        let b = b.into();
        if a == b {
            return Err(McpError::InvalidEntanglement {
                reason: format!("两端服务器不能相同: {a}"),
            });
        }
        // 规范化:字典序小的在前,确保 (a,b) 与 (b,a) 视为同一条链接
        let linked_servers = if a <= b { (a, b) } else { (b, a) };
        Ok(Self {
            linked_servers,
            sync_strategy,
        })
    }

    /// 判断指定服务器是否为链接的一端
    pub fn involves(&self, server_id: &str) -> bool {
        self.linked_servers.0 == server_id || self.linked_servers.1 == server_id
    }

    /// 获取对端服务器 ID(若指定服务器在链接中)
    pub fn partner_of(&self, server_id: &str) -> Option<&str> {
        if self.linked_servers.0 == server_id {
            Some(&self.linked_servers.1)
        } else if self.linked_servers.1 == server_id {
            Some(&self.linked_servers.0)
        } else {
            None
        }
    }
}

/// 纠缠链接管理器 — 基于 DashMap 的并发安全注册表
///
/// 链接以 `(min_id, max_id)` 元组为 key,确保同一对服务器的链接只注册一次。
/// `register` 时若链接已存在,更新 sync_strategy 并返回旧值。
pub struct EntanglementManager {
    links: DashMap<(String, String), SyncStrategy>,
}

impl EntanglementManager {
    /// 创建空管理器
    pub fn new() -> Self {
        Self {
            links: DashMap::new(),
        }
    }

    /// 注册纠缠链接 — 若已存在则更新策略,返回旧策略(若有)
    pub fn register(&self, link: EntanglementLink) -> Result<Option<SyncStrategy>, McpError> {
        let key = link.linked_servers.clone();
        // DashMap::insert 返回旧值
        let old = self.links.insert(key, link.sync_strategy);
        Ok(old)
    }

    /// 注销纠缠链接,返回被移除的策略(若存在)
    pub fn unregister(&self, a: &str, b: &str) -> Option<SyncStrategy> {
        let key = if a <= b {
            (a.to_string(), b.to_string())
        } else {
            (b.to_string(), a.to_string())
        };
        self.links.remove(&key).map(|(_, v)| v)
    }

    /// 查询指定服务器参与的所有链接策略
    pub fn strategies_for(&self, server_id: &str) -> Vec<SyncStrategy> {
        self.links
            .iter()
            .filter(|entry| {
                let (a, b) = entry.key();
                a == server_id || b == server_id
            })
            .map(|entry| *entry.value())
            .collect()
    }

    /// 获取指定服务器对之间的同步策略(若存在)
    pub fn get(&self, a: &str, b: &str) -> Option<SyncStrategy> {
        let key = if a <= b {
            (a.to_string(), b.to_string())
        } else {
            (b.to_string(), a.to_string())
        };
        self.links.get(&key).map(|r| *r.value())
    }

    /// 当前链接数量
    pub fn len(&self) -> usize {
        self.links.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }
}

impl Default for EntanglementManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_link_new_normalizes_order() {
        let link1 = EntanglementLink::new("s-2", "s-1", SyncStrategy::Eager).expect("创建失败");
        let link2 = EntanglementLink::new("s-1", "s-2", SyncStrategy::Eager).expect("创建失败");
        // 顺序无关,规范化后应相等
        assert_eq!(link1, link2);
        assert_eq!(link1.linked_servers, ("s-1".into(), "s-2".into()));
    }

    #[test]
    fn test_link_rejects_same_servers() {
        let err = EntanglementLink::new("s-1", "s-1", SyncStrategy::Lazy).unwrap_err();
        assert!(matches!(err, McpError::InvalidEntanglement { .. }));
    }

    #[test]
    fn test_link_involves_and_partner() {
        let link = EntanglementLink::new("s-1", "s-2", SyncStrategy::Eager).expect("创建失败");
        assert!(link.involves("s-1"));
        assert!(link.involves("s-2"));
        assert!(!link.involves("s-3"));

        assert_eq!(link.partner_of("s-1"), Some("s-2"));
        assert_eq!(link.partner_of("s-2"), Some("s-1"));
        assert_eq!(link.partner_of("s-3"), None);
    }

    #[test]
    fn test_manager_register_and_get() {
        let mgr = EntanglementManager::new();
        let link = EntanglementLink::new("s-1", "s-2", SyncStrategy::Eager).expect("创建失败");
        assert!(mgr.register(link).expect("注册失败").is_none());

        assert_eq!(mgr.get("s-1", "s-2"), Some(SyncStrategy::Eager));
        assert_eq!(mgr.get("s-2", "s-1"), Some(SyncStrategy::Eager)); // 顺序无关
        assert_eq!(mgr.get("s-1", "s-3"), None);
        assert_eq!(mgr.len(), 1);
    }

    #[test]
    fn test_manager_register_updates_strategy() {
        let mgr = EntanglementManager::new();
        let link1 = EntanglementLink::new("s-1", "s-2", SyncStrategy::Eager).expect("创建失败");
        mgr.register(link1).expect("注册失败");

        let link2 = EntanglementLink::new("s-2", "s-1", SyncStrategy::Lazy).expect("创建失败");
        let old = mgr.register(link2).expect("注册失败");
        assert_eq!(old, Some(SyncStrategy::Eager));
        assert_eq!(mgr.get("s-1", "s-2"), Some(SyncStrategy::Lazy));
        assert_eq!(mgr.len(), 1); // 仍是一条链接
    }

    #[test]
    fn test_manager_unregister() {
        let mgr = EntanglementManager::new();
        let link = EntanglementLink::new("s-1", "s-2", SyncStrategy::BestEffort).expect("创建失败");
        mgr.register(link).expect("注册失败");

        let removed = mgr.unregister("s-2", "s-1"); // 顺序无关
        assert_eq!(removed, Some(SyncStrategy::BestEffort));
        assert!(mgr.is_empty());
    }

    #[test]
    fn test_manager_strategies_for() {
        let mgr = EntanglementManager::new();
        mgr.register(EntanglementLink::new("s-1", "s-2", SyncStrategy::Eager).expect("创建失败"))
            .expect("注册失败");
        mgr.register(EntanglementLink::new("s-1", "s-3", SyncStrategy::Lazy).expect("创建失败"))
            .expect("注册失败");
        mgr.register(
            EntanglementLink::new("s-2", "s-3", SyncStrategy::BestEffort).expect("创建失败"),
        )
        .expect("注册失败");

        let strategies = mgr.strategies_for("s-1");
        assert_eq!(strategies.len(), 2);
        assert!(strategies.contains(&SyncStrategy::Eager));
        assert!(strategies.contains(&SyncStrategy::Lazy));

        let s2_strategies = mgr.strategies_for("s-2");
        assert_eq!(s2_strategies.len(), 2);
    }

    #[test]
    fn test_sync_strategy_serde() {
        let s = SyncStrategy::Eager;
        let json = serde_json::to_string(&s).expect("序列化失败");
        let restored: SyncStrategy = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(s, restored);
    }
}
