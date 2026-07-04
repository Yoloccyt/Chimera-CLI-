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

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

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
    /// 服务器端点(如 "203.0.113.1:8080";register 时会做 SSRF 校验,拒绝内网地址)
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
    /// - `SsrfBlocked`:endpoint 指向内网/保留地址(SSRF 防御)
    /// - `ServerNotFound`:内部不应触发(保留以扩展)
    pub fn register(&self, server: MeshServer) -> Result<(), McpError> {
        if self.capacity == 0 {
            return Err(McpError::RegistryFull { capacity: 0 });
        }
        // WHY: SSRF 校验必须在 register 入口执行,防止攻击者注册指向
        // 云元数据(169.254.169.254)、内网数据库等的 endpoint,
        // 借 mesh 事务机制发起内网探测。详见 `validate_endpoint`。
        validate_endpoint(&server.endpoint)?;
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

/// 校验 endpoint 是否安全(非内网/保留地址) — SSRF 防御核心
///
/// WHY: ServerRegistry 接受任意 endpoint 会构成 SSRF 攻击面:攻击者可注册
/// 指向 AWS 元数据(169.254.169.254/latest/meta-data)、K8s API(10.0.0.1)、
/// 内网数据库(192.168.x.x)等的 endpoint,借 mesh 事务机制触发对内网的请求。
/// 此函数在 `register` 入口拦截所有已知内网/保留地址,从源头切断 SSRF 路径。
///
/// # 策略(YAGNI 工程折中)
/// - **IP 字面量黑名单**:覆盖 IPv4/IPv6 所有关键保留段(回环/私有/链路本地/
///   CGNAT/未指定/云元数据),无需 DNS 解析即可判定。
/// - **域名黑名单**:仅拦截已知内网域名(`localhost`、`metadata.google.internal`
///   等)。不做 DNS 解析 — 因为 register 阶段同步解析可能阻塞、CI 不稳定,
///   且 DNS rebinding 攻击需要请求层二次校验才能彻底防御,不属于 register 职责。
///   公网域名一律放行,实际网络请求层应在 connect 前再次校验解析后的 IP。
///
/// # 支持的 endpoint 格式
/// - `host:port` (如 `203.0.113.1:8080`)
/// - `scheme://host:port/path` (如 `http://example.com:8080/api`)
/// - `scheme://[ipv6]:port/path` (如 `http://[::1]:8080/`)
///
/// # 错误
/// - `SsrfBlocked`:endpoint 解析出的 host 为内网/保留地址或已知内网域名
pub(crate) fn validate_endpoint(endpoint: &str) -> Result<(), McpError> {
    let host = extract_host(endpoint)?;
    if is_reserved_domain(&host) {
        return Err(McpError::SsrfBlocked {
            endpoint: endpoint.to_string(),
        });
    }
    // 若 host 可解析为 IP 字面量,校验是否为保留地址
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_reserved_ip(ip) {
            return Err(McpError::SsrfBlocked {
                endpoint: endpoint.to_string(),
            });
        }
    }
    // 公网域名或非保留 IP 字面量 → 放行
    Ok(())
}

/// 从 endpoint 提取 host 部分(剥离 scheme、port、path)
///
/// 支持三种格式:
/// - `host:port` → `host`
/// - `scheme://host:port/path` → `host`
/// - `scheme://[ipv6]:port/path` → `ipv6`(不含方括号)
fn extract_host(endpoint: &str) -> Result<String, McpError> {
    // 剥离 scheme:取 `://` 之后的部分;若无 scheme,原样使用
    let after_scheme = endpoint.split("://").nth(1).unwrap_or(endpoint);
    // 取 authority 段(第一个 `/` 之前)
    let authority = after_scheme.split('/').next().unwrap_or(after_scheme);
    // 处理 IPv6 字面量 `[::1]:port`
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        // 期望形如 `[::1]:port` 或 `[::1]`,以 `]` 结束 host 部分
        stripped
            .split(']')
            .next()
            .ok_or_else(|| McpError::SsrfBlocked {
                endpoint: endpoint.to_string(),
            })?
    } else {
        // IPv4 或域名:剥离末尾 `:port`(若存在)
        // 注意:用 rsplit_once 确保只剥离最后一个 `:`,避免误判 IPv6(已在上分支处理)
        authority
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(authority)
    };
    if host.is_empty() {
        return Err(McpError::SsrfBlocked {
            endpoint: endpoint.to_string(),
        });
    }
    Ok(host.to_string())
}

/// 判断域名是否为已知内网域名(不做 DNS 解析,仅黑名单匹配)
///
/// 涵盖主流云厂商元数据服务域名与本地别名。DNS 解析后的二次校验
/// 留给实际网络请求层(此处仅做轻量拦截)。
fn is_reserved_domain(domain: &str) -> bool {
    let lower = domain.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "localhost"
            | "metadata.google.internal"
            | "metadata.aws.internal"
            | "metadata.azure.com"
            | "169.254.169.254.nip.io"
            | "kubernetes.default.svc"
            | "kubernetes.default.svc.cluster.local"
    )
}

/// 判断 IP 是否属于内网/保留地址段
///
/// 覆盖范围:
/// - IPv4: 0.0.0.0/8(本机网络)、10.0.0.0/8(私有)、100.64.0.0/10(CGNAT)、
///   127.0.0.0/8(回环)、169.254.0.0/16(链路本地,含云元数据)、
///   172.16.0.0/12(私有)、192.168.0.0/16(私有)、224.0.0.0/4(组播)、
///   255.255.255.255(广播)
/// - IPv6: ::1(回环)、::(未指定)、fc00::/7(唯一本地)、fe80::/10(链路本地)
fn is_reserved_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_reserved_ipv4(v4),
        IpAddr::V6(v6) => is_reserved_ipv6(v6),
    }
}

fn is_reserved_ipv4(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    // 0.0.0.0/8 — 本机网络(含 0.0.0.0 未指定)
    o[0] == 0
    // 10.0.0.0/8 — RFC 1918 私有
    || o[0] == 10
    // 100.64.0.0/10 — RFC 6598 CGNAT
    || (o[0] == 100 && (o[1] & 0xC0) == 64)
    // 127.0.0.0/8 — 回环(标准库 is_loopback 覆盖,显式列出便于阅读)
    || o[0] == 127
    // 169.254.0.0/16 — 链路本地(含 AWS/GCP/Azure 元数据 169.254.169.254)
    || (o[0] == 169 && o[1] == 254)
    // 172.16.0.0/12 — RFC 1918 私有
    || (o[0] == 172 && (o[1] & 0xF0) == 16)
    // 192.168.0.0/16 — RFC 1918 私有
    || (o[0] == 192 && o[1] == 168)
    // 224.0.0.0/4 — 组播
    || (o[0] & 0xF0) == 224
    // 255.255.255.255 — 广播
    || o == [255, 255, 255, 255]
}

fn is_reserved_ipv6(v6: Ipv6Addr) -> bool {
    let s = v6.segments();
    // ::1 — 回环
    v6.is_loopback()
    // :: — 未指定
    || v6.is_unspecified()
    // fc00::/7 — 唯一本地地址(ULA)
    || (s[0] & 0xFE00) == 0xFC00
    // fe80::/10 — 链路本地
    || (s[0] & 0xFFC0) == 0xFE80
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration as StdDuration;

    fn make_server(id: &str) -> MeshServer {
        // 使用 RFC 5737 TEST-NET-3(203.0.113.0/24)文档用途地址,绕过 SSRF 校验
        MeshServer::new(id, format!("203.0.113.1:{id}"), vec!["cap-1".into()])
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
        let updated = MeshServer::new("s-1", "203.0.113.1:9999", vec![]);
        registry.register(updated).expect("注册失败");
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry.get("s-1").expect("应存在").endpoint,
            "203.0.113.1:9999"
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
