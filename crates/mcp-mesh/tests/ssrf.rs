//! SSRF 防御集成测试 — 验证 `ServerRegistry::register` 拒绝内网/保留地址
//!
//! 对应 F-004 修复:在 register 入口校验 endpoint,拦截指向内网/云元数据的请求。
//!
//! # 测试矩阵
//! - **拒绝列表**:IPv4/IPv6 回环、私有网络、链路本地(含 AWS 元数据)、CGNAT、
//!   未指定地址、已知内网域名
//! - **接受列表**:公网域名、公网 IP(TEST-NET-3 文档地址)

#![forbid(unsafe_code)]

use mcp_mesh::{McpError, MeshServer, ServerRegistry};

/// 构造一个最小的 MeshServer,只关注 endpoint 字段
fn server_with_endpoint(id: &str, endpoint: &str) -> MeshServer {
    MeshServer::new(id, endpoint, vec![])
}

/// 断言给定 endpoint 会被 SSRF 校验拦截(返回 `SsrfBlocked`)
fn assert_blocked(endpoint: &str) {
    let registry = ServerRegistry::new(16);
    let err = registry
        .register(server_with_endpoint("s-1", endpoint))
        .expect_err("期望被 SSRF 校验拦截,但实际接受了");
    assert!(
        matches!(err, McpError::SsrfBlocked { .. }),
        "期望 SsrfBlocked,实际得到: {err:?} (endpoint={endpoint:?})"
    );
}

/// 断言给定 endpoint 会被接受(注册成功)
fn assert_accepted(endpoint: &str) {
    let registry = ServerRegistry::new(16);
    registry
        .register(server_with_endpoint("s-1", endpoint))
        .expect("期望注册成功,但被拒绝");
}

// === 1. IPv4 内网/保留地址 — 全部拒绝 ===

#[test]
fn test_ssrf_blocks_ipv4_loopback() {
    // 127.0.0.0/8 回环 — 经典本地服务地址
    assert_blocked("http://127.0.0.1:8080");
    assert_blocked("127.0.0.1:8080");
    assert_blocked("http://127.255.255.254:80");
}

#[test]
fn test_ssrf_blocks_aws_metadata() {
    // 169.254.169.254 — AWS/GCP/Azure 云元数据,SSRF 经典目标
    assert_blocked("http://169.254.169.254/latest/meta-data/");
    assert_blocked("http://169.254.169.254/computeMetadata/v1/");
    assert_blocked("169.254.170.2"); // ECS task metadata
    assert_blocked("http://169.254.169.254"); // 无路径
}

#[test]
fn test_ssrf_blocks_rfc1918_private() {
    // 10.0.0.0/8 — RFC 1918 私有
    assert_blocked("http://10.0.0.1:8080");
    assert_blocked("http://10.255.255.255:80");
    // 172.16.0.0/12 — RFC 1918 私有
    assert_blocked("http://172.16.0.1:8080");
    assert_blocked("http://172.31.255.255:80");
    // 192.168.0.0/16 — RFC 1918 私有
    assert_blocked("http://192.168.1.1:8080");
    assert_blocked("http://192.168.0.0:80");
}

#[test]
fn test_ssrf_blocks_cgnat() {
    // 100.64.0.0/10 — RFC 6598 CGNAT(运营商级 NAT)
    assert_blocked("http://100.64.0.1:8080");
    assert_blocked("http://100.127.255.255:80");
}

#[test]
fn test_ssrf_blocks_unspecified_ipv4() {
    // 0.0.0.0/8 — 本机网络(含 0.0.0.0 未指定)
    assert_blocked("http://0.0.0.0:8080");
    assert_blocked("http://0.0.0.1:80");
}

#[test]
fn test_ssrf_blocks_multicast_and_broadcast() {
    // 224.0.0.0/4 — 组播
    assert_blocked("http://224.0.0.1:8080");
    assert_blocked("http://239.255.255.255:80");
    // 255.255.255.255 — 广播
    assert_blocked("http://255.255.255.255:80");
}

// === 2. IPv6 内网/保留地址 — 全部拒绝 ===

#[test]
fn test_ssrf_blocks_ipv6_loopback() {
    // ::1 — IPv6 回环
    assert_blocked("http://[::1]:8080");
    assert_blocked("[::1]:8080");
}

#[test]
fn test_ssrf_blocks_ipv6_unspecified() {
    // :: — 未指定
    assert_blocked("http://[::]:8080");
}

#[test]
fn test_ssrf_blocks_ipv6_unique_local() {
    // fc00::/7 — 唯一本地地址(ULA,IPv6 私有)
    assert_blocked("http://[fc00::1]:8080");
    assert_blocked("http://[fd00::1]:8080");
    assert_blocked("http://[fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff]:80");
}

#[test]
fn test_ssrf_blocks_ipv6_link_local() {
    // fe80::/10 — 链路本地
    assert_blocked("http://[fe80::1]:8080");
    assert_blocked("http://[febf::]:8080");
}

// === 3. 已知内网域名 — 全部拒绝 ===

#[test]
fn test_ssrf_blocks_localhost_domain() {
    assert_blocked("http://localhost:8080");
    assert_blocked("http://Localhost:80"); // 大小写不敏感
    assert_blocked("localhost:8080");
}

#[test]
fn test_ssrf_blocks_cloud_metadata_domains() {
    // GCP 元数据服务域名
    assert_blocked("http://metadata.google.internal/computeMetadata/v1/");
    // K8s 内部服务域名
    assert_blocked("https://kubernetes.default.svc:443");
    assert_blocked("https://kubernetes.default.svc.cluster.local:443");
}

// === 4. 合法公网 endpoint — 全部接受 ===

#[test]
fn test_ssrf_accepts_public_domains() {
    // 公网域名(不做 DNS 解析,直接放行)
    assert_accepted("http://example.com:8080");
    assert_accepted("https://api.openai.com");
    assert_accepted("https://github.com:443");
    assert_accepted("example.com:8080");
}

#[test]
fn test_ssrf_accepts_test_net_3() {
    // RFC 5737 TEST-NET-3(203.0.113.0/24)— 文档/测试用途,不在保留范围
    assert_accepted("203.0.113.1:8080");
    assert_accepted("http://203.0.113.1:8080");
    // TEST-NET-1/2 同样不在 SSRF 黑名单
    assert_accepted("http://192.0.2.1:8080"); // TEST-NET-1
    assert_accepted("http://198.51.100.1:8080"); // TEST-NET-2
}

#[test]
fn test_ssrf_accepts_public_ipv4() {
    // 公网 IPv4(如 8.8.8.8 Google DNS、1.1.1.1 Cloudflare DNS)
    assert_accepted("http://8.8.8.8:53");
    assert_accepted("http://1.1.1.1:443");
}

#[test]
fn test_ssrf_accepts_public_ipv6() {
    // 2001:4860:4860::8888 — Google Public DNS IPv6
    assert_accepted("http://[2001:4860:4860::8888]:53");
    // 2606:4700:4700::1111 — Cloudflare Public DNS IPv6
    assert_accepted("http://[2606:4700:4700::1111]:443");
}

// === 5. 边界情况 ===

#[test]
fn test_ssrf_blocks_empty_host() {
    // 空 host 应被拒绝(解析失败)
    assert_blocked("http://:8080");
}

#[test]
fn test_ssrf_error_carries_endpoint() {
    // 错误变体应携带原样 endpoint,便于排查
    let registry = ServerRegistry::new(16);
    let endpoint = "http://169.254.169.254/latest/meta-data/";
    let err = registry
        .register(server_with_endpoint("s-x", endpoint))
        .unwrap_err();
    match err {
        McpError::SsrfBlocked { endpoint: e } => {
            assert_eq!(e, endpoint, "错误应携带原样 endpoint");
        }
        other => panic!("期望 SsrfBlocked,得到 {other:?}"),
    }
}
