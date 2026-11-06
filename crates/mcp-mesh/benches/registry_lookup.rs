//! MCP Mesh 注册表查找基准 — 命中 / 未命中 / `ServerNotFound` 快速失败
//!
//! 覆盖用户指定的第 4 类测点：
//! - `get_hit_100` / `get_hit_1000`：DashMap 分片命中 + `MeshServer` 整体 clone
//! - `get_miss`：未命中路径（`None`，零 clone）
//! - `heartbeat_missing`：`ServerNotFound` 错误构造（含 `server_id` String 分配）
//! - `unregister_missing`：同上，注销路径
//! - `list_alive_100`：全量存活扫描（后台探活周期性调用）
//!
//! 运行：
//! ```powershell
//! cargo bench -p mcp-mesh --bench registry_lookup
//! ```
#![allow(clippy::unwrap_used, clippy::expect_used)] // bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use criterion::{criterion_group, criterion_main, Criterion};
use mcp_mesh::{MeshServer, ServerRegistry};

/// 构造 N 台服务器的注册表（endpoint 用 RFC 5737 文档地址，天然绕过 SSRF 校验）
fn registry_with(n: usize) -> ServerRegistry {
    let registry = ServerRegistry::new(n.max(1));
    for i in 0..n {
        let server = MeshServer::new(
            format!("s-{i}"),
            format!("203.0.113.1:{}", 10000 + i),
            vec!["cap-a".to_string(), "cap-b".to_string()],
        );
        registry.register(server).expect("注册失败");
    }
    registry
}

fn bench_registry(c: &mut Criterion) {
    let mut group = c.benchmark_group("mcp_registry");
    group.sample_size(50);

    for n in [100usize, 1000] {
        let registry = registry_with(n);
        let hit_id = format!("s-{}", n / 2);
        group.bench_function(format!("get_hit_{n}"), |b| {
            b.iter(|| {
                let server = registry.get(&hit_id);
                assert!(server.is_some());
            });
        });
    }

    let registry = registry_with(100);
    // 零 clone 存在性检查（优化后路径）：与 `get_hit_100` 是同一语义的两种实现
    group.bench_function("contains_hit_100", |b| {
        b.iter(|| {
            assert!(registry.contains("s-50"));
        });
    });
    group.bench_function("contains_miss", |b| {
        b.iter(|| {
            assert!(!registry.contains("s-absent"));
        });
    });

    group.bench_function("get_miss", |b| {
        b.iter(|| {
            let server = registry.get("s-absent");
            assert!(server.is_none());
        });
    });

    group.bench_function("heartbeat_missing", |b| {
        b.iter(|| {
            let err = registry.heartbeat("s-absent").expect_err("未注册应报错");
            assert!(matches!(err, mcp_mesh::McpError::ServerNotFound { .. }));
        });
    });

    group.bench_function("unregister_missing", |b| {
        b.iter(|| {
            let err = registry.unregister("s-absent").expect_err("未注册应报错");
            assert!(matches!(err, mcp_mesh::McpError::ServerNotFound { .. }));
        });
    });

    group.bench_function("list_alive_100", |b| {
        b.iter(|| {
            let alive = registry.list_alive(300_000);
            assert_eq!(alive.len(), 100);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_registry);
criterion_main!(benches);
