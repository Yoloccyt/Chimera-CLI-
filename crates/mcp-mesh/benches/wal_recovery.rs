//! WAL 恢复性能基准 — 验证崩溃恢复延迟 < 100ms
//!
//! 对应 Task 0.7 v2.9.0-omega SubTask 0.7.15
//!
//! # 验收标准
//! - WAL 恢复(读取 + 解析 100 条 entry)< 100ms
//! - WAL 单条 append(含 fsync)< 10ms
//!
//! # 运行
//! ```bash
//! cargo bench -p mcp-mesh --bench wal_recovery
//! ```

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mcp_mesh::{TransactionState, WalEntry, WalStore};
use tempfile::TempDir;
use tokio::runtime::Runtime;

/// 构造 N 条 WAL entry(模拟 N 个事务的 Prepare + Commit)
fn make_entries(n: usize) -> Vec<WalEntry> {
    (0..n)
        .map(|i| {
            let tx_id = format!("tx-{i:04}");
            WalEntry::new(
                tx_id,
                TransactionState::Prepare,
                vec!["s-1".into(), "s-2".into(), "s-3".into()],
            )
        })
        .collect()
}

/// 基准:WAL 单条 append(含 fsync)
fn bench_wal_append(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime 创建失败");

    c.bench_function("wal_append_single_fsync", |b| {
        b.to_async(&rt).iter(|| {
            let temp = TempDir::new().expect("临时目录创建失败");
            let store = WalStore::new(temp.path().join("bench.wal"));
            let entry = WalEntry::new("tx-bench", TransactionState::Prepare, vec!["s-1".into()]);

            async move {
                black_box(store.append(&entry).await).expect("append 失败");
                // TempDir 析构时清理
                drop(temp);
            }
        });
    });
}

/// 基准:WAL 批量写入(N 条)后读取全部(恢复场景)
///
/// 模拟协调者崩溃前写入了 N 条 WAL entry,重启时读取并解析全部 entry。
/// 验收标准:100 条 entry 的恢复(读取 + 解析)< 100ms。
fn bench_wal_recovery(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime 创建失败");

    let mut group = c.benchmark_group("wal_recovery");
    group.sample_size(10); // 减少 sample 数,因每轮需写 N 条 + fsync
    group.measurement_time(std::time::Duration::from_secs(15));

    for n in [10, 50, 100, 500] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let entries = make_entries(n);

            b.to_async(&rt).iter(|| {
                let entries = entries.clone();
                async move {
                    let temp = TempDir::new().expect("临时目录创建失败");
                    let store = WalStore::new(temp.path().join("recovery.wal"));

                    // 1. 写入 N 条 entry(模拟崩溃前的 WAL)
                    for entry in &entries {
                        store.append(entry).await.expect("append 失败");
                    }

                    // 2. 读取全部(模拟崩溃恢复)
                    let recovered = store.read_all().await.expect("read_all 失败");
                    assert_eq!(recovered.len(), n, "恢复的 entry 数应匹配");

                    black_box(recovered);
                    // TempDir 析构时清理
                    drop(temp);
                }
            });
        });
    }

    group.finish();
}

/// 基准:WAL 读取空文件(首次启动场景)
fn bench_wal_read_empty(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime 创建失败");

    c.bench_function("wal_read_empty", |b| {
        b.to_async(&rt).iter(|| {
            let temp = TempDir::new().expect("临时目录创建失败");
            let store = WalStore::new(temp.path().join("nonexistent.wal"));

            async move {
                let entries = store.read_all().await.expect("read_all 应成功");
                assert!(entries.is_empty(), "不存在的 WAL 应返回空 Vec");
                black_box(entries);
                drop(temp);
            }
        });
    });
}

/// 基准:WAL truncate(恢复后清理)
fn bench_wal_truncate(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime 创建失败");

    c.bench_function("wal_truncate_after_recovery", |b| {
        b.to_async(&rt).iter(|| {
            let temp = TempDir::new().expect("临时目录创建失败");
            let store = WalStore::new(temp.path().join("truncate.wal"));
            let entry = WalEntry::new("tx-trunc", TransactionState::Commit, vec!["s-1".into()]);

            async move {
                // 先写入一条 entry
                store.append(&entry).await.expect("append 失败");
                // 然后 truncate(模拟恢复后清理)
                store.truncate().await.expect("truncate 失败");
                drop(temp);
            }
        });
    });
}

criterion_group!(
    benches,
    bench_wal_append,
    bench_wal_recovery,
    bench_wal_read_empty,
    bench_wal_truncate
);
criterion_main!(benches);
