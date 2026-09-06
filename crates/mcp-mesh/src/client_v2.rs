//! MCP 客户端闭环 — 发现(Discovery) + Schema 缓存 + 路由注册(WI-22 / ADR-150)
//!
//! 对应任务:WI-22 MCP client_v2 客户端闭环(发现 + schema 缓存 + 路由注册,ADR-150)
//! 对应架构层:L10 Interface
//!
//! # 重建说明
//! ⚠️ 本文件于 **2026-08-28 磁盘 ENOSPC 事故中数据丢失**,现按 WI-22/ADR-150
//! 语义做**最小但真实**的三项骨架重建:
//! 1. **发现(Discovery)**:[`DiscoveryResult`] 承载一次端点发现的结果结构
//!    (服务器标识 + 可用工具 + 端点 + 实测延迟 + 发现时刻);
//! 2. **Schema 缓存**:[`SchemaCache`] 以并发安全 `DashMap` 缓存
//!    "Schema 摘要 → 发现结果",提供 `insert` / `get` 基础操作;
//! 3. **路由注册**:[`RouteRegistry`] 维护 "Schema 摘要 → 服务器" 的路由表,
//!    提供 `register`(注册)与 `query`(查路由)基础操作。
//!
//! > 说明:本重建仅覆盖三项骨架的基础数据结构与方法,未恢复丢失前的完整
//! > 连接器/协议握手逻辑;后续如需完整功能请按 ADR-150 增量补齐。

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

/// 单次发现的元数据快照 — 记录发现目标、可用工具与实测延迟
///
/// WHY 独立结构:发现结果需在 Schema 缓存中被引用,且能被序列化持久化
/// (serde),故与"发现流程"解耦为纯数据。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveryResult {
    /// 被发现的服务器标识(如 "s-1" / "mcp-fileserver")
    pub server_id: String,
    /// 服务器暴露的工具名列表
    pub tools: Vec<String>,
    /// 目标端点(host:port 或 URI)
    pub endpoint: String,
    /// 本次发现实测延迟(微秒,0 表示未测量)
    pub latency_us: u64,
    /// 是否承载 schema(能力预告)信息
    pub has_schema: bool,
}

impl DiscoveryResult {
    /// 构造一次发现结果
    #[must_use]
    pub fn new(server_id: String, tools: Vec<String>, endpoint: String) -> Self {
        Self {
            server_id,
            tools,
            endpoint,
            latency_us: 0,
            has_schema: false,
        }
    }

    /// 记录实测延迟(链式,便于在发现完成后回填)
    #[must_use]
    pub fn with_latency(mut self, latency_us: u64) -> Self {
        self.latency_us = latency_us;
        self
    }
}

/// Schema 缓存 — 并发安全的 "Schema 摘要 → 发现结果" 映射
///
/// WHY `DashMap`:发现/接入同一 schema 可能来自并发连接器线程,`DashMap`
/// 分片锁提供无阻塞读与低争用写(与项目其余注册表同哲学)。
#[derive(Debug, Default)]
pub struct SchemaCache {
    /// schema 摘要 → 发现结果(schema 摘要见 [`RouteRegistry`] 注释)
    inner: DashMap<String, DiscoveryResult>,
}

impl SchemaCache {
    /// 创建空缓存
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 插入一条缓存记录(已存在则覆盖)
    pub fn insert(&self, schema_key: String, result: DiscoveryResult) {
        self.inner.insert(schema_key, result);
    }

    /// 查询缓存记录 — 未命中返回 `None`
    #[must_use]
    pub fn get(&self, schema_key: &str) -> Option<DiscoveryResult> {
        self.inner.get(schema_key).map(|r| r.clone())
    }

    /// 当前缓存条目数
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// 缓存是否为空
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// 路由注册表 — 并发安全 "Schema 摘要 → 服务器" 路由表
///
/// WHY 独立于 [`SchemaCache`]:缓存仅加速本地访问,而路由表决定"该 schema
/// 请求应转发到哪个服务器";两者语义不同,故分开展架。
#[derive(Debug, Default)]
pub struct RouteRegistry {
    /// schema 摘要 → 目标服务器 id
    routes: DashMap<String, String>,
}

impl RouteRegistry {
    /// 创建空路由表
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一条路由(schema 摘要 → 服务器;已存在则覆盖)
    pub fn register(&self, schema_key: String, server_id: String) {
        self.routes.insert(schema_key, server_id);
    }

    /// 查询路由 — 未注册返回 `None`
    #[must_use]
    pub fn query(&self, schema_key: &str) -> Option<String> {
        self.routes.get(schema_key).map(|s| s.clone())
    }

    /// 当前路由条数
    #[must_use]
    pub fn len(&self) -> usize {
        self.routes.len()
    }

    /// 路由表是否为空
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{DiscoveryResult, RouteRegistry, SchemaCache};

    /// DiscoveryResult 构造 + 链式回填延迟
    #[test]
    fn discovery_result_builds_with_latency() {
        let r = DiscoveryResult::new("s-1".into(), vec!["tool-a".into()], "127.0.0.1:8080".into())
            .with_latency(42);
        assert_eq!(r.server_id, "s-1");
        assert_eq!(r.tools.len(), 1);
        assert_eq!(r.latency_us, 42);
        assert!(!r.has_schema);
    }

    /// SchemaCache insert + get 基础语义(命中/未命中/覆盖/空)
    #[test]
    fn schema_cache_insert_get() {
        let cache = SchemaCache::new();
        assert!(cache.is_empty());
        let r = DiscoveryResult::new("s-1".into(), vec!["t".into()], "ep".into());
        cache.insert("sum/{a,b}".into(), r.clone());
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get("sum/{a,b}"), Some(r.clone()));
        assert_eq!(cache.get("missing"), None);
        // 覆盖语义
        cache.insert("sum/{a,b}".into(), r.with_latency(7));
        assert_eq!(cache.get("sum/{a,b}").unwrap().latency_us, 7);
        assert_eq!(cache.len(), 1, "覆盖不增条目");
    }

    /// RouteRegistry register + query 基础语义
    #[test]
    fn route_registry_register_query() {
        let routes = RouteRegistry::new();
        assert!(routes.is_empty());
        routes.register("sum/{a,b}".into(), "compute-1".into());
        assert_eq!(routes.len(), 1);
        assert_eq!(routes.query("sum/{a,b}"), Some("compute-1".into()));
        assert_eq!(routes.query("no-route"), None);
        // 覆盖语义
        routes.register("sum/{a,b}".into(), "compute-2".into());
        assert_eq!(routes.query("sum/{a,b}"), Some("compute-2".into()));
        assert_eq!(routes.len(), 1);
    }
}
