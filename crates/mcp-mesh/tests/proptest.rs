//! mcp-mesh 属性测试 — 服务注册查询不变量
//!
//! 对应架构层:L10 Interface
//! 对应 SubTask 13.3:v1.5.0-omega 发布就绪差距闭合
//!
//! # 测试覆盖的不变量
//! 1. ServerRegistry register 后 get 返回注册的服务
//! 2. register 后 unregister,get 返回 None
//! 3. capacity == 0 拒绝所有注册(RegistryFull)
//! 4. SSRF 防御:私有/保留 IP 段被拒绝(127.x/10.x/192.168.x/169.254.x/172.16-31.x)
//! 5. SSRF 放行:TEST-NET-3(203.0.113.x)公网文档地址被接受
//!
//! # 设计要点
//! - 使用 RFC 5737 TEST-NET-3(203.0.113.0/24)绕过 SSRF 校验
//! - block-named 语法(§4.1 规范)
//! - 256 cases(proptest 默认)

#![forbid(unsafe_code)]

use mcp_mesh::{McpError, MeshServer, ServerRegistry};
use proptest::prelude::*;

/// 构造合法的 TEST-NET-3 endpoint(绕过 SSRF 校验)
/// WHY TEST-NET-3:RFC 5737 文档用途地址,不会被 SSRF 黑名单拦截
fn make_test_endpoint(port: u16) -> String {
    format!("203.0.113.1:{port}")
}

/// 构造合法的 TEST-NET-3 服务器
fn make_test_server(id: &str, port: u16) -> MeshServer {
    MeshServer::new(id, make_test_endpoint(port), vec!["cap-1".into()])
}

proptest! {
    /// 不变量 1:register 后 get 返回注册的服务
    ///
    /// 注册 N 个不同 server_id 的服务器,get 应返回对应记录。
    /// 验证 DashMap 插入与查询的一致性。
    #[test]
    fn prop_register_then_get_returns_server(
        server_count in 1u16..=20,
        start_port in 1000u16..=60000,
    ) {
        let registry = ServerRegistry::new(256);
        let mut ids = Vec::new();
        for i in 0..server_count {
            let id = format!("s-{i}");
            let server = make_test_server(&id, start_port + i);
            let result = registry.register(server);
            prop_assert!(result.is_ok(), "注册 s-{} 应成功: {:?}", i, result);
            ids.push(id);
        }
        prop_assert_eq!(registry.len(), server_count as usize);

        // 逐个 get 验证
        for id in &ids {
            let got = registry.get(id);
            prop_assert!(got.is_some(), "get({}) 应返回 Some", id);
        }
    }

    /// 不变量 2:register 后 unregister,get 返回 None
    ///
    /// 注销后服务器应从注册表移除,再次 get 返回 None。
    /// 重复注销应返回 ServerNotFound 错误。
    #[test]
    fn prop_unregister_removes_server(
        port in 1000u16..=60000,
    ) {
        let registry = ServerRegistry::new(16);
        registry.register(make_test_server("s-1", port)).ok();
        prop_assert_eq!(registry.len(), 1);

        // 注销
        let result = registry.unregister("s-1");
        prop_assert!(result.is_ok(), "注销应成功");
        prop_assert!(registry.get("s-1").is_none(), "注销后 get 应返回 None");
        prop_assert_eq!(registry.len(), 0);

        // 重复注销应失败
        let dup = registry.unregister("s-1");
        prop_assert!(
            matches!(dup, Err(McpError::ServerNotFound { .. })),
            "重复注销应返回 ServerNotFound"
        );
    }

    /// 不变量 3:capacity == 0 拒绝所有注册(RegistryFull)
    ///
    /// 零容量注册表无法接受任何服务器,所有 register 返回 RegistryFull。
    #[test]
    fn prop_zero_capacity_rejects_all(port in 1000u16..=60000) {
        let registry = ServerRegistry::new(0);
        let result = registry.register(make_test_server("s-1", port));
        prop_assert!(
            matches!(result, Err(McpError::RegistryFull { capacity: 0 })),
            "零容量注册应返回 RegistryFull {{ capacity: 0 }},实际 {:?}",
            result
        );
        prop_assert_eq!(registry.len(), 0);
        prop_assert!(registry.is_empty());
    }

    /// 不变量 4:SSRF 防御 — 私有/保留 IPv4 段被拒绝
    ///
    /// 覆盖关键保留段:
    /// - 127.x.x.x(回环)
    /// - 10.x.x.x(RFC 1918 私有)
    /// - 192.168.x.x(RFC 1918 私有)
    /// - 169.254.x.x(链路本地,含云元数据)
    /// - 172.16-31.x.x(RFC 1918 私有)
    /// - 0.x.x.x(本机网络)
    ///
    /// WHY SSRF 是安全红线:攻击者可注册内网 endpoint 借 mesh 事务探测内网,
    /// 必须 register 入口拦截(§6.2 红线)。
    #[test]
    fn prop_ssrf_rejects_private_ipv4(
        second_octet in 0u8..=255,
        third_octet in 0u8..=255,
        port in 1u16..=65535,
    ) {
        // 构造各保留段的 endpoint
        let private_endpoints = vec![
            format!("127.{second_octet}.{third_octet}.1:{port}"),     // 回环
            format!("10.{second_octet}.{third_octet}.1:{port}"),      // RFC 1918
            format!("192.168.{third_octet}.1:{port}"),                // RFC 1918
            format!("169.254.{third_octet}.1:{port}"),                // 链路本地
            format!("0.{second_octet}.{third_octet}.1:{port}"),       // 本机网络
        ];

        // 172.16-31.x.x 需要特殊构造(仅 16-31 被拦截)
        let endpoint_172 = format!("172.{}.{}.1:{port}", 16 + (second_octet % 16), third_octet);

        let registry = ServerRegistry::new(16);

        for ep in &private_endpoints {
            let server = MeshServer::new("s-test", ep.clone(), vec![]);
            let result = registry.register(server);
            prop_assert!(
                matches!(result, Err(McpError::SsrfBlocked { .. })),
                "私有地址 {} 应被 SSRF 拦截,实际 {:?}",
                ep,
                result
            );
        }

        let server_172 = MeshServer::new("s-172", endpoint_172.clone(), vec![]);
        let result_172 = registry.register(server_172);
        prop_assert!(
            matches!(result_172, Err(McpError::SsrfBlocked { .. })),
            "172.16-31 地址 {} 应被 SSRF 拦截,实际 {:?}",
            endpoint_172,
            result_172
        );
    }

    /// 不变量 5:SSRF 放行 — TEST-NET-3(203.0.113.x)公网文档地址被接受
    ///
    /// RFC 5737 TEST-NET-3(203.0.113.0/24)是文档用途公网地址,
    /// 不在 SSRF 黑名单中,应被接受。
    /// WHY 测试放行路径:确保 SSRF 校验不会误拦合法公网地址。
    #[test]
    fn prop_ssrf_accepts_test_net_3(port in 1u16..=65535) {
        let registry = ServerRegistry::new(16);
        let endpoint = format!("203.0.113.1:{port}");
        let server = MeshServer::new("s-public", endpoint.clone(), vec!["cap-1".into()]);
        let result = registry.register(server);
        prop_assert!(
            result.is_ok(),
            "TEST-NET-3 地址 {} 应被接受,实际 {:?}",
            endpoint,
            result
        );
        prop_assert_eq!(registry.len(), 1);
    }
}
