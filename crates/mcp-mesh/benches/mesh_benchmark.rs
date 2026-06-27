//! MCP Mesh 性能基准 — 测量 5 服务器事务延迟
//!
//! 对应 SubTask 1.7:性能基准测试
//!
//! # 验收标准
//! - p95 ≤ 100ms(设计目标)
//! - 目标 ≤ 80ms(留 20% 余量)
//!
//! # 运行
//! ```bash
//! cargo bench -p mcp-mesh --bench mesh_benchmark
//! # 或带 --ignored 运行完整压测(被 #[ignore] 标记)
//! cargo bench -p mcp-mesh --bench mesh_benchmark -- --ignored
//! ```

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mcp_mesh::{McpMesh, MeshConfig, MeshServer};
use tokio::runtime::Runtime;

/// 构建带 N 个服务器的 Mesh(无 EventBus,纯事务测量)
///
/// # WHY: 心跳超时配置
/// `MeshConfig::default()` 的 `heartbeat_timeout_ms = 5000`(5s),但 criterion
/// `measurement_time = 10s` + `sample_size = 50`,5s 后所有服务器会被 `is_alive`
/// 判定为离线,导致 `execute_transaction` 返回 `ServerUnreachable` 而 panic。
///
/// 修复策略:参考 `tests/integration.rs::test_1000_transactions_all_publish_events`
/// 的模式,将 `heartbeat_timeout_ms` 延长至 300_000ms(5 分钟),覆盖整个基准
/// 运行周期(单事务基准约 30s + 并发基准约 15s)。in-process mock 无真实网络
/// 心跳,延长超时仅影响"是否判定离线"的阈值,不影响事务本身的延迟测量精度
/// (2PC 的 prepare/commit/rollback 仍用 `tokio::time::sleep` 模拟 1-2ms 网络往返)。
fn make_mesh_with_n_servers(n: usize) -> McpMesh {
    // 心跳超时延长至 5 分钟,确保基准全程服务器被视为 alive
    let config = MeshConfig {
        heartbeat_timeout_ms: 300_000,
        ..MeshConfig::default()
    };
    let mesh = McpMesh::new(config);
    for i in 0..n {
        let sid = format!("s-{i}");
        let _ = mesh.register_server(MeshServer::new(sid, format!("127.0.0.1:{i}"), vec![]));
    }
    mesh
}

/// 构建 N 服务器参与者列表
fn make_participants(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("s-{i}")).collect()
}

/// 基准:测量 1/3/5 服务器事务延迟(单事务串行)
fn bench_transaction(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");
    let sizes: &[usize] = &[1, 3, 5];

    let mut group = c.benchmark_group("mcp_mesh_transaction");
    group.sample_size(50); // 50 次采样,降低方差
    group.measurement_time(std::time::Duration::from_secs(10));

    for &n in sizes {
        let mesh = make_mesh_with_n_servers(n);
        let participants = make_participants(n);

        group.bench_with_input(BenchmarkId::new("2pc", n), &n, |b, &_| {
            b.iter(|| {
                rt.block_on(async {
                    mesh.execute_transaction(
                        black_box(participants.clone()),
                        black_box("bench".into()),
                    )
                    .await
                    .expect("事务失败")
                })
            });
        });
    }

    group.finish();
}

/// 基准:测量 5 服务器并发 100 事务批量延迟
///
/// 此基准被 `#[ignore]` 标记,需 `--ignored` 显式运行,因为它耗时较长
/// 且会与单事务基准产生噪音。
#[ignore = "perf: run with --ignored"]
fn bench_concurrent_100_transactions(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");
    let mesh = std::sync::Arc::new(make_mesh_with_n_servers(5));
    let participants = make_participants(5);

    let mut group = c.benchmark_group("mcp_mesh_concurrent");
    group.sample_size(10);
    group.measurement_time(std::time::Duration::from_secs(15));

    group.bench_function("100_concurrent_5_servers", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut handles = Vec::with_capacity(100);
                for _ in 0..100 {
                    let mesh = std::sync::Arc::clone(&mesh);
                    let p = participants.clone();
                    handles.push(tokio::spawn(async move {
                        mesh.execute_transaction(p, "bench-concurrent".into()).await
                    }));
                }
                for h in handles {
                    let _ = h.await;
                }
            })
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_transaction,
    bench_concurrent_100_transactions
);
criterion_main!(benches);
