//! VectorIndex RwLock 优化基准(B1)
//!
//! 对应 Task III-1:验证 VectorIndex 从 Mutex 改为 RwLock 后的并发读收益。
//!
//! # 基准项
//! - `single_thread_knn_latency`:100/1000 条目单线程 KNN 延迟基线
//! - `concurrent_knn_search_throughput`:10 并发 search 吞吐(验证 RwLock 多读并发)
//! - `search_under_write_load`:写负载下 search 延迟(验证 RwLock 读写竞争降级)
//!
//! # 运行
//! ```bash
//! cargo bench -p repo-wiki --bench vector_bench
//! # 快速验证(不精确测量,仅验证编译与基本运行)
//! cargo bench -p repo-wiki --bench vector_bench -- --quick
//! ```
//!
//! # 设计说明
//! VectorIndex 为纯内存结构(`RwLock<HashMap>`),无文件 IO,因此:
//! - 不使用 `tempfile::tempdir()`。`WikiStore` bench 需要它创建 SQLite 文件,
//!   而 VectorIndex 不持久化任何数据,强行使用只会产生无用代码。
//! - `search`/`upsert`/`delete` 均为同步方法。并发 bench 通过 `spawn_blocking`
//!   在阻塞线程池执行同步 search,避免阻塞 tokio async runtime(§4.4 反模式:
//!   同步阻塞调用必须 spawn_blocking,不可直接在 async task 中执行)。

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use repo_wiki::VectorIndex;
use tokio::runtime::Runtime;

/// 向量维度(与 CLV/NexusState 一致,512-dim 潜在语言向量)
const VECTOR_DIM: usize = 512;

/// KNN 返回的 Top-K 数量
const TOP_K: usize = 5;

/// 并发 search 任务数(验证 RwLock 多读并发的核心场景)
const CONCURRENT_TASKS: usize = 10;

/// 小规模预填充条目数(并发/写负载场景基线)
const SMALL_SIZE: usize = 100;

/// 大规模预填充条目数(单线程延迟扩展性验证)
const LARGE_SIZE: usize = 1000;

/// 生成确定性伪随机向量(避免引入 rand 依赖与 RNG 开销干扰测量)
///
/// WHY:每个分量基于 `(id, dim_index)` 派生,保证:
/// 1. 不同 id 产生不同向量(余弦相似度有意义,非全 1.0)
/// 2. 无零向量(避免除零导致相似度 NaN)
/// 3. 可复现(每次运行结果一致,消除随机性导致的 bench 抖动)
fn make_vector(id: u64, dim: usize) -> Vec<f32> {
    (0..dim)
        .map(|j| {
            let v = ((id.wrapping_mul(7).wrapping_add(j as u64 * 13)) % 1000) as f32 / 1000.0;
            v + 0.001
        })
        .collect()
}

/// 预填充向量索引
///
/// 在 setup 阶段执行,不计入测量时间。返回传入的索引便于链式调用。
fn prefill(index: &VectorIndex, count: usize) {
    for i in 0..count {
        let vec = make_vector(i as u64, VECTOR_DIM);
        index
            .upsert(&format!("vec-{i}"), &vec)
            .expect("预填充 upsert 失败");
    }
}

/// bench 1:单线程 KNN 延迟基线
///
/// WHY:建立 100/1000 两种规模的单线程 search 延迟基线。
/// 后续并发 bench 的吞吐提升需对照此基线判断:
/// - 若并发吞吐 ≈ 单线程 × N → RwLock 多读并发生效
/// - 若并发吞吐 ≈ 单线程 → RwLock 退化为串行(读锁未真正并发)
///
/// 此 bench 为纯同步测量(search 是同步方法),无需 tokio runtime。
fn single_thread_knn_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("single_thread_knn_latency");

    for &size in &[SMALL_SIZE, LARGE_SIZE] {
        let index = VectorIndex::new(VECTOR_DIM);
        prefill(&index, size);
        // 查询向量与 vec-0 相同,保证命中 top1 且结果稳定
        let query = make_vector(0, VECTOR_DIM);

        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| {
                let results = index
                    .search(black_box(&query), black_box(TOP_K))
                    .expect("search 失败");
                black_box(results);
            });
        });
    }
    group.finish();
}

/// bench 2:10 并发 KNN search 吞吐
///
/// WHY:这是验证 RwLock 核心优化点的基准——多个 search(读锁)可同时执行。
/// 用 `spawn_blocking` 在阻塞线程池并发执行同步 search,避免阻塞 async runtime。
/// `Throughput::Elements(10)` 让 criterion 报告 ops/sec,直观反映并发收益。
///
/// 注:spawn_blocking 有固定调度开销,但它是 tokio 处理同步阻塞调用的正确方式
/// (§4.4 反模式:禁止在 async task 直接执行同步阻塞调用)。
fn concurrent_knn_search_throughput(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");

    let index = Arc::new(VectorIndex::new(VECTOR_DIM));
    prefill(&index, SMALL_SIZE);
    let query = Arc::new(make_vector(0, VECTOR_DIM));

    let mut group = c.benchmark_group("concurrent_knn_search_throughput");
    // 每次 iter 执行 10 个并发 search,吞吐按 10 个操作计
    group.throughput(Throughput::Elements(CONCURRENT_TASKS as u64));

    group.bench_function("10_concurrent_search", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut handles = Vec::with_capacity(CONCURRENT_TASKS);
                for _ in 0..CONCURRENT_TASKS {
                    let idx = Arc::clone(&index);
                    let q = Arc::clone(&query);
                    // WHY spawn_blocking:search 是同步阻塞调用,
                    // 直接在 async task 中执行会占用 worker thread 阻塞 runtime
                    handles.push(tokio::task::spawn_blocking(move || {
                        idx.search(black_box(q.as_slice()), black_box(TOP_K))
                            .expect("search 失败")
                    }));
                }
                for h in handles {
                    let _ = h.await.expect("并发 search task panic");
                }
            });
        });
    });
    group.finish();
}

/// bench 3:写负载下 KNN search 延迟
///
/// WHY:验证 RwLock 在写锁持有时的降级表现。
/// 后台持续 upsert(写锁互斥),同时测量 search 延迟。
/// 预期:search 延迟略高于基线(写锁竞争导致读等待),
/// 但不应数量级恶化(RwLock 读优先/公平性)。
///
/// 后台 writer 在 multi-thread runtime 的 worker thread 上独立运行,
/// 与主线程的同步 search 产生真实读写竞争。
fn search_under_write_load(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");

    let index = Arc::new(VectorIndex::new(VECTOR_DIM));
    prefill(&index, SMALL_SIZE);
    let query = make_vector(0, VECTOR_DIM);

    // 后台写入任务:持续 upsert 新向量模拟写负载
    let write_index = Arc::clone(&index);
    let write_counter = Arc::new(AtomicU64::new(SMALL_SIZE as u64));
    let writer = rt.spawn(async move {
        loop {
            let id = write_counter.fetch_add(1, Ordering::Relaxed);
            let vec = make_vector(id, VECTOR_DIM);
            // upsert 是同步调用(HashMap insert 极快,微秒级),
            // 此处直接执行无需 spawn_blocking
            if write_index.upsert(&format!("write-{id}"), &vec).is_err() {
                break;
            }
            // 让出执行权,允许 runtime 调度,避免 writer 独占 worker 饿死读
            tokio::task::yield_now().await;
        }
    });

    let read_index = Arc::clone(&index);
    let mut group = c.benchmark_group("search_under_write_load");
    // 显式保证样本数 ≥ 100,即使单次迭代较慢也不降级
    group.sample_size(100);
    group.bench_function("knn_under_write_load", |b| {
        b.iter(|| {
            // search 同步执行,writer 在另一 worker thread 并发 upsert 产生竞争
            let results = read_index
                .search(black_box(&query), black_box(TOP_K))
                .expect("search 失败");
            black_box(results);
        });
    });
    group.finish();

    // 清理后台写入任务,避免进程残留
    writer.abort();
}

criterion_group!(
    benches,
    single_thread_knn_latency,
    concurrent_knn_search_throughput,
    search_under_write_load
);
criterion_main!(benches);
