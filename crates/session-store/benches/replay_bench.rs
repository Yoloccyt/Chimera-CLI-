//! replay_bench — k-way 归并回放基准（P2-T3 / ADR-109）
//!
//! 对应任务: **P2-T3**（ADR-109 k-way 归并时间线回放 / v4.0 WI-18 会话树）
//!
//! # 指标
//!
//! - **wal_replay_seconds（门禁口径）**:写 1 万事件（微批,多段）→ 重开
//!   段文件 + 树索引 → `replay` 全量回放的墙钟耗时（T8/T9 固定 n 单次采样
//!   模式,打印到 stdout 供指标登记）。
//! - **replay_order_consistency_pct（门禁:100%）**:回放输出顺序与写入顺序
//!   逐项一致率——任意事件序列写入后 replay 必须与写入顺序逐项一致。
//! - **fork_latency_ms（门禁:<100ms）**:fork 会话耗时（fork 现有实现,
//!   bench 记录延迟;断言 <100ms 红线）。
//!
//! # 模式
//!
//! `probe_replay_10000` / `probe_fork_latency` 为固定 n 单次采样
//! （T8/T9 模式,打印到 stdout）,criterion 其余组为迭代基准。

use criterion::{criterion_group, criterion_main, Criterion};
use session_store::{
    replay, CbmrWriter, Offset, SessionEvent, SessionId, StoreConfig, TreeIndex,
};
use std::time::Instant;

/// 回放规模（门禁口径:1 万事件）
const REPLAY_N: u64 = 10_000;
/// 回放基准的段大小（1000 条/段 → 10 段,k=10 归并）
const SEGMENT_ROWS: u64 = 1_000;
/// fork 基准的父会话规模
const FORK_N: u64 = 1_024;
/// fork 门禁（毫秒）
const FORK_LATENCY_MS_LIMIT: f64 = 100.0;

fn ev(i: u64) -> SessionEvent {
    SessionEvent::with_payload(format!("ev-{i}"), vec![i as u8])
}

fn current_thread_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("构建 tokio runtime")
}

/// 写 n 条事件（微批,多段）到临时目录,返回 (dir, tree, sid)
///
/// 模拟真实生命周期:微批写 → drop（重开段 + 树索引）——replay 与
/// rebuild 的输入场景（段文件为权威源）。
fn seed(n: u64, segment_rows: u64) -> (tempfile::TempDir, TreeIndex, SessionId) {
    let dir = tempfile::tempdir().expect("临时目录");
    let mut cfg = StoreConfig::with_dir(dir.path());
    cfg.spawn_flush_loop = false; // 确定性:仅满批/显式 flush
    cfg.max_rows_per_segment = segment_rows;
    cfg.batch_size = 64;
    let writer = CbmrWriter::new(cfg).expect("CbmrWriter");
    let rt = current_thread_runtime();
    let sid = SessionId::new("bench-replay");
    rt.block_on(async {
        for i in 0..n {
            writer.append(&sid, ev(i)).await.expect("append");
        }
        writer.flush().await.expect("flush");
    });
    drop(rt);
    drop(writer); // 模拟重开:replay 只依赖段文件 + 树索引
    let tree =
        TreeIndex::open(&dir.path().join("sessions.sqlite3")).expect("重开树索引");
    (dir, tree, sid)
}

/// T8/T9 固定 n 单次采样:replay 1 万事件墙钟 + 顺序一致率（门禁 100%）
fn probe_replay_10000() -> (f64, f64) {
    let (dir, tree, sid) = seed(REPLAY_N, SEGMENT_ROWS);
    let start = Instant::now();
    let stream = replay(&tree, dir.path(), &sid, Offset::new(0, 0)).expect("replay");
    let items = stream.collect().expect("collect");
    let elapsed = start.elapsed().as_secs_f64();

    // 顺序一致率:逐项比对回放输出与写入顺序（门禁 100%）
    let total = items.len();
    let consistent = items
        .iter()
        .enumerate()
        .filter(|(i, item)| {
            item.offset.seq == *i as u64 && item.event.event_type == format!("ev-{i}")
        })
        .count();
    let pct = consistent as f64 / total as f64 * 100.0;

    println!("\n================ wal_replay_seconds 采样 (n={REPLAY_N}, k={}) ================",
        REPLAY_N / SEGMENT_ROWS);
    println!("wal_replay_seconds = {elapsed:.6}  (写 1 万 + 重开 + k-way 归并回放,墙钟)");
    println!("replay_order_consistency_pct = {pct:.2}%  (门禁: 100%)");
    println!("=====================================================================\n");
    assert_eq!(total, REPLAY_N as usize, "回放条数必须完整");
    assert_eq!(pct, 100.0, "门禁:回放顺序一致率必须 100%");
    (elapsed, pct)
}

/// T8/T9 固定 n 单次采样:fork 会话耗时（断言 <100ms 门禁）
fn probe_fork_latency() -> f64 {
    let (dir, _tree, parent) = seed(FORK_N, 256);
    let rt = current_thread_runtime();
    let start = Instant::now();
    let fork_point = FORK_N / 2;
    let child = {
        let cfg = StoreConfig::with_dir(dir.path());
        let w = CbmrWriter::new(cfg).expect("重开 writer");
        rt.block_on(async { w.fork_session(&parent, fork_point).await.expect("fork") })
    };
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    drop(rt);
    drop(child);

    println!("\n================ fork_latency_ms 采样 (父会话 {FORK_N} 条, fork 点 {fork_point}) ================");
    println!("fork_latency_ms = {ms:.3}  (门禁: <{FORK_LATENCY_MS_LIMIT}ms)");
    println!("=========================================================================\n");
    assert!(
        ms < FORK_LATENCY_MS_LIMIT,
        "fork 门禁失败: {ms:.3}ms >= {FORK_LATENCY_MS_LIMIT}ms"
    );
    ms
}

/// 固定 n 单次采样组（T8/T9 模式,非迭代基准）——门禁指标登记入口
fn probe_metrics(_c: &mut Criterion) {
    probe_replay_10000();
    probe_fork_latency();
}

/// k-way 归并回放迭代基准:1 万事件（10 段）全量回放耗时
fn bench_replay_10000(c: &mut Criterion) {
    let (dir, tree, sid) = seed(REPLAY_N, SEGMENT_ROWS);
    c.bench_function("replay_10000_events_kway_merge", |b| {
        b.iter(|| {
            let stream = replay(&tree, dir.path(), &sid, Offset::new(0, 0)).expect("replay");
            let items = stream.collect().expect("collect");
            assert_eq!(items.len(), REPLAY_N as usize);
            items.len()
        })
    });
}

/// fork 会话迭代基准:父会话 1024 条事件,记录 fork 延迟
fn bench_fork(c: &mut Criterion) {
    let (dir, _tree, parent) = seed(FORK_N, 256);
    let rt = current_thread_runtime();
    c.bench_function("fork_session_1024_events", |b| {
        b.iter(|| {
            let cfg = StoreConfig::with_dir(dir.path());
            let w = CbmrWriter::new(cfg).expect("writer");
            let sid = rt.block_on(async {
                w.fork_session(&parent, FORK_N / 2).await.expect("fork")
            });
            sid
        })
    });
    drop(rt);
}

criterion_group!(
    benches,
    probe_metrics,
    bench_replay_10000,
    bench_fork
);
criterion_main!(benches);
