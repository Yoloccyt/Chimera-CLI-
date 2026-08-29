//! session_store_bench — 单条直写 vs CBMR 微批写 + WAL 崩溃恢复回放基准
//!
//! 对应任务: **P2-T2**（手册 W9 T-07 / ADR-108;T8/T9 固定 n 单次采样模式）
//!
//! # 指标
//!
//! - **syscall_reduction_pct（Windows 代理指标 = 写操作计数对比）**:
//!   单条直写 N 条 = N 次 SQLite 写事务;微批写 N 条 = ceil(N/64) 次。
//!   `syscall_reduction_pct = (1 - batch_count/single_count) * 100`。
//!   门禁要求 ≥80%（64 批下理论值 = 1 - 16/1024 ≈ 98.4%）。
//! - **wal_replay_seconds（预研指标）**:微批写 → 重开恢复（截断校验）
//!   → 全量回放的墙钟耗时。
//!
//! # 模式
//!
//! `syscall_reduction_probe` / `wal_replay_probe` 为固定 n 单次采样
//! （T8/T9 模式,打印到 stdout）,criterion 其余组为迭代基准。

use criterion::{criterion_group, criterion_main, Criterion};
use session_store::{
    EventRow, SegmentId, SegmentWriter, SessionEvent, SessionId, StoreConfig, TreeIndex,
    CbmrWriter,
};
use std::time::Instant;

/// 固定采样规模（T8/T9 固定 n 单次采样）
const PROBE_N: usize = 1024;
/// 微批上限（与 StoreConfig::default().batch_size 一致）
const BATCH_SIZE: usize = 64;

fn ev(i: u64) -> SessionEvent {
    SessionEvent::with_payload(format!("ev-{i}"), vec![i as u8])
}

fn current_thread_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("构建 tokio runtime")
}

/// T8/T9 固定 n 单次采样:打印 syscall_reduction_pct（门禁 ≥80%）
fn probe_syscall_reduction() -> (u64, u64, f64) {
    let rt = current_thread_runtime();
    let n = PROBE_N;

    // --- 单条直写:N 条 = N 次 SQLite 写事务 ---
    let tree = TreeIndex::open_in_memory().expect("内存树索引");
    let sid = SessionId::new("probe-single");
    let seg_id = SegmentId::new("probe-seg");
    for i in 0..n {
        let row = EventRow {
            offset: i as u64,
            session_id: sid.clone(),
            segment_id: seg_id.clone(),
            event: ev(i as u64),
        };
        tree.insert_events(&[row]).expect("单条直写");
    }
    let single_count = tree.transaction_count();

    // --- 微批写:N 条 = ceil(N/64) 次 SQLite 写事务 ---
    let dir = tempfile::tempdir().expect("临时目录");
    let mut cfg = StoreConfig::with_dir(dir.path());
    cfg.spawn_flush_loop = false; // 确定性:仅满批触发
    let writer = CbmrWriter::new(cfg).expect("CbmrWriter");
    rt.block_on(async {
        for i in 0..n {
            writer
                .append(&sid, ev(i as u64))
                .await
                .expect("微批 append");
        }
    });
    let batch_count = writer.transactions();

    let pct = (1.0 - batch_count as f64 / single_count as f64) * 100.0;
    println!("\n================ syscall_reduction_pct 采样 (n={n}) ================");
    println!("single_write_count = {single_count}   (单条直写: N 条 = N 次 SQLite 写)");
    println!("batch_write_count  = {batch_count}   (CBMR 微批: ceil(N/{BATCH_SIZE}) 次)");
    println!("syscall_reduction_pct = {pct:.2}%  (门禁: ≥80%)");
    println!("=====================================================================\n");
    (single_count, batch_count, pct)
}

/// T8/T9 固定 n 单次采样:打印 WAL 崩溃恢复回放耗时（预研指标）
fn probe_wal_replay() -> f64 {
    let rt = current_thread_runtime();
    let dir = tempfile::tempdir().expect("临时目录");
    let n = 4096usize;
    let sid = SessionId::new("probe-replay");

    let start = Instant::now();
    // 阶段 1:微批写 n 条
    let mut cfg = StoreConfig::with_dir(dir.path());
    cfg.spawn_flush_loop = false;
    let writer = CbmrWriter::new(cfg).expect("CbmrWriter");
    rt.block_on(async {
        for i in 0..n {
            writer
                .append(&sid, ev(i as u64))
                .await
                .expect("微批 append");
        }
    });
    // 阶段 2:drop 模拟崩溃后重启,重开段(截断校验)+ 树索引
    drop(writer);
    let tree = TreeIndex::open(&dir.path().join("sessions.sqlite3")).expect("重开树索引");
    let _recovered = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0).expect("重开段");
    // 阶段 3:全量回放(树索引读回)
    let stored = tree.read_events(&sid, None).expect("全量回放");
    assert_eq!(stored.len(), n, "崩溃恢复后事件数必须完整");
    let elapsed = start.elapsed();

    println!("\n================ wal_replay_seconds 采样 (n={n}) ================");
    println!("wal_replay_seconds = {:.6}  (微批写 + 重开恢复 + 全量回放,墙钟)",
        elapsed.as_secs_f64());
    println!("==================================================================\n");
    elapsed.as_secs_f64()
}

/// 固定 n 单次采样组（T8/T9 模式,非迭代基准）
fn probe_metrics(_c: &mut Criterion) {
    probe_syscall_reduction();
    probe_wal_replay();
}

/// 单条直写基准:每事件一次 SQLite 事务的耗时（写路径基线）
///
/// # WHY 每迭代新建内存树
/// events 主键 = (session_id, offset);criterion 的 `iter` 测量机制会
/// 重复调用闭包,复用同一树索引必然第二次迭代起 UNIQUE 冲突。
/// 每次迭代新建内存树保证数据隔离（建表开销两边基准对称,对比公平）。
fn bench_single_write(c: &mut Criterion) {
    let rt = current_thread_runtime();
    let sid = SessionId::new("bench-single");
    let seg_id = SegmentId::new("bench-seg");
    c.bench_function("single_write_per_event_txn", |b| {
        b.iter(|| {
            let tree = TreeIndex::open_in_memory().expect("内存树索引");
            for i in 0..BATCH_SIZE {
                let row = EventRow {
                    offset: i as u64,
                    session_id: sid.clone(),
                    segment_id: seg_id.clone(),
                    event: ev(i as u64),
                };
                tree.insert_events(&[row]).expect("单条直写");
            }
            tree.transaction_count()
        })
    });
    drop(rt);
}

/// 微批写基准:64 条攒批 + 1 次 flush 的耗时（写路径优化后基线）
///
/// # WHY 每迭代新会话
/// 事件 offset 按会话从 0 起,复用同一会话每次迭代会写相同
/// (session_id, offset) → UNIQUE 冲突;新会话隔离迭代间数据。
fn bench_microbatch(c: &mut Criterion) {
    let rt = current_thread_runtime();
    let dir = tempfile::tempdir().expect("临时目录");
    let mut cfg = StoreConfig::with_dir(dir.path());
    cfg.spawn_flush_loop = false;
    let writer = CbmrWriter::new(cfg).expect("CbmrWriter");
    c.bench_function("microbatch_write_64_in_one_flush", |b| {
        let mut iter = 0u64;
        b.iter(|| {
            let sid = SessionId::new(format!("bench-micro-{iter}"));
            iter += 1;
            rt.block_on(async {
                for i in 0..BATCH_SIZE {
                    writer
                        .append(&sid, ev(i as u64))
                        .await
                        .expect("微批 append");
                }
            });
            writer.transactions()
        })
    });
    drop(rt);
}

/// WAL 崩溃恢复回放基准:写 → 重开 → 全量读回的完整循环
fn bench_wal_replay(c: &mut Criterion) {
    let rt = current_thread_runtime();
    c.bench_function("wal_replay_cycle", |b| {
        b.iter(|| {
            let dir = tempfile::tempdir().expect("临时目录");
            let sid = SessionId::new("bench-replay");
            let mut cfg = StoreConfig::with_dir(dir.path());
            cfg.spawn_flush_loop = false;
            let writer = CbmrWriter::new(cfg).expect("CbmrWriter");
            rt.block_on(async {
                for i in 0..64 {
                    writer
                        .append(&sid, ev(i as u64))
                        .await
                        .expect("append");
                }
            });
            drop(writer);
            let tree = TreeIndex::open(&dir.path().join("sessions.sqlite3")).expect("重开");
            let stored = tree.read_events(&sid, None).expect("回放");
            assert_eq!(stored.len(), 64);
            stored.len()
        })
    });
    drop(rt);
}

criterion_group!(
    benches,
    probe_metrics,
    bench_single_write,
    bench_microbatch,
    bench_wal_replay
);
criterion_main!(benches);
