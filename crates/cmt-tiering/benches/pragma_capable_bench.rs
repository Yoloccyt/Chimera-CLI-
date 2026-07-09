//! PragmaCapable trait + apply_performance_pragmas 泛型函数性能基准
//!
//! 对应任务:F2.4.4(rusqlite 下沉方案 E 的性能验收基准)
//! 架构层归属:L3 Storage(基准测试位于下游 crate,因 nexus-core 不依赖 rusqlite)
//!
//! # 验收标准
//! - C1: 重构后 p95 延迟 ≤ 重构前 +5%(方案 E 静态分发理论零开销)
//!
//! # 基准组
//! - `apply_pragmas`: 单次 `apply_performance_pragmas` 调用延迟(泛型 + 真实 rusqlite::Connection)
//! - `pragma_query_baseline`: 未设置 PRAGMA 时的查询延迟基线
//! - `pragma_query_optimized`: 设置 PRAGMA 后的查询延迟(对比基线)
//!
//! # 设计要点
//! 使用 `iter_batched` 确保每次迭代使用新连接,避免 PRAGMA 状态污染。
//! 真实 rusqlite::Connection(非 mock),验证泛型函数在生产路径的性能特征。

use cmt_tiering::PragmaConn;
use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use nexus_core::apply_performance_pragmas;

/// 打开一个 in-memory rusqlite::Connection 并设置 journal_mode=WAL
///
/// WHY WAL 前置:`apply_performance_pragmas` 文档要求必须在 WAL 模式之后调用
/// (部分 PRAGMA 依赖 WAL 模式,如 synchronous=NORMAL 的安全性保证)。
/// in-memory 数据库的 WAL 不持久化但调用不报错,满足基准测试前置条件。
///
/// 失败时 panic:基准 setup 失败应立即报告,而非静默跳过(§4.1 系统边界处理)。
fn make_wal_connection() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory()
        .unwrap_or_else(|e| panic!("打开 in-memory SQLite 连接失败: {e}"));
    conn.pragma_update(None, "journal_mode", "WAL")
        .unwrap_or_else(|e| panic!("设置 journal_mode=WAL 失败: {e}"));
    conn
}

/// 打开一个 in-memory rusqlite::Connection(不设置任何 PRAGMA,作为基线)
fn make_plain_connection() -> rusqlite::Connection {
    rusqlite::Connection::open_in_memory()
        .unwrap_or_else(|e| panic!("打开 in-memory SQLite 连接失败: {e}"))
}

/// 基准组 1:apply_performance_pragmas 单次调用延迟
///
/// 测量泛型函数 `apply_performance_pragmas::<PragmaConn>` 的单次调用延迟。
/// 方案 E 静态分发理论零开销,本基准验证重构后实际延迟符合预期。
/// 使用 `iter_batched` 每次迭代新建连接,避免 PRAGMA 状态污染。
fn bench_apply_performance_pragmas(c: &mut Criterion) {
    let mut group = c.benchmark_group("apply_pragmas");
    group.bench_function("apply_performance_pragmas_single_call", |b| {
        b.iter_batched(
            make_wal_connection,
            |conn| {
                let wrapper = PragmaConn(&conn);
                apply_performance_pragmas(&wrapper)
                    .unwrap_or_else(|e| panic!("apply_performance_pragmas 应当成功: {e}"));
                // black_box 防止优化器消除整个调用
                black_box(&conn);
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

/// 基准组 2:未设置 PRAGMA 时的查询延迟基线
///
/// 测量 plain in-memory 连接执行 `SELECT 1` 的延迟,作为对比基线。
/// 不应用任何性能 PRAGMA,反映默认 SQLite 配置下的查询开销。
fn bench_pragma_query_baseline(c: &mut Criterion) {
    let mut group = c.benchmark_group("pragma_query_baseline");
    group.bench_function("select_1_no_pragma", |b| {
        b.iter_batched(
            make_plain_connection,
            |conn| {
                let result: i64 = conn
                    .query_row("SELECT 1", [], |row| row.get(0))
                    .unwrap_or_else(|e| panic!("SELECT 1 查询失败: {e}"));
                black_box(result);
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

/// 基准组 3:设置 PRAGMA 后的查询延迟(对比基线)
///
/// 先应用 `apply_performance_pragmas`,再执行 `SELECT 1`,测量优化后查询延迟。
/// 与 `pragma_query_baseline` 对比,验证 PRAGMA 设置对查询延迟的影响。
///
/// 注意:in-memory 数据库的 PRAGMA 效果有限(无磁盘 I/O),本基准主要验证
/// PRAGMA 设置不引入显著查询开销(符合 C1: p95 ≤ 重构前 +5%)。
fn bench_pragma_query_optimized(c: &mut Criterion) {
    let mut group = c.benchmark_group("pragma_query_optimized");
    group.bench_function("select_1_with_pragma", |b| {
        b.iter_batched(
            || {
                // setup:WAL 前置 + 应用全部性能 PRAGMA,复用生产路径初始化流程
                let conn = make_wal_connection();
                let wrapper = PragmaConn(&conn);
                apply_performance_pragmas(&wrapper)
                    .unwrap_or_else(|e| panic!("apply_performance_pragmas 应当成功: {e}"));
                conn
            },
            |conn| {
                let result: i64 = conn
                    .query_row("SELECT 1", [], |row| row.get(0))
                    .unwrap_or_else(|e| panic!("SELECT 1 查询失败: {e}"));
                black_box(result);
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_apply_performance_pragmas,
    bench_pragma_query_baseline,
    bench_pragma_query_optimized,
);
criterion_main!(benches);
