//! 向量检索基准套件(VectorIndex RwLock 优化 + HnswStore 生产路径)
//!
//! 对应 Task III-1:验证 VectorIndex 从 Mutex 改为 RwLock 后的并发读收益。
//! 对应 Task P2-W8.1.3:HnswStore 10K entry KNN p95 < 50ms 红线验证。
//! 对应 Task P2-W8.3.1:HnswStore 100K entry KNN p95 < 50ms 红线验证(spec.md 索引 SLO)。
//!
//! # 基准项
//! ## VectorIndex(内存 KNN,≤1000 entry)
//! - `single_thread_knn_latency`:100/1000 条目单线程 KNN 延迟基线
//! - `concurrent_knn_search_throughput`:10 并发 search 吞吐(验证 RwLock 多读并发)
//! - `search_under_write_load`:写负载下 search 延迟(验证 RwLock 读写竞争降级)
//!
//! ## HnswStore 10K(HNSW 生产路径,P2-W8.1.3)
//! - `hnsw_10k_search_latency`:10K entry 单线程 KNN 延迟(均值/中位数基线)
//! - `hnsw_10k_p95_search_latency`:10K entry p95 延迟(iter_custom 收集样本计算)
//!
//! ## HnswStore 100K(HNSW 生产路径,P2-W8.3.1,spec 索引 SLO 红线)
//! - `hnsw_100k_search_latency`:100K entry 单线程 KNN 延迟(均值/中位数基线)
//! - `hnsw_100k_p95_search_latency`:100K entry p95 延迟(iter_custom 收集样本计算)
//!
//! # 运行
//! ```bash
//! # 全部基准
//! cargo bench -p repo-wiki --bench vector_bench
//! # 仅 HnswStore 10K 基准(快速验证)
//! cargo bench -p repo-wiki --bench vector_bench -- "hnsw_10k"
//! # 仅 HnswStore 100K 基准(生产规模验证,release 模式推荐)
//! cargo bench --release -p repo-wiki --bench vector_bench -- "hnsw_100k"
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
//!
//! HnswStore 同样为纯内存结构,但基于 HNSW 图算法,支持 10K-100K entry 规模。
//! - `top_k`/`upsert` 均为同步方法(`&self`),无需 spawn_blocking
//! - 10K entry × 512 dim ≈ 20MB 向量数据 + ~60MB HNSW 图 ≈ 80MB 内存,安全
//! - 100K entry × 512 dim ≈ 195MB 向量数据 + ~390MB HNSW 图 ≈ 585MB 内存,安全
//! - 预填充在 setup 阶段执行(不计入测量):10K 约 1-3 秒,100K 约 30-60 秒(release)
//! - 100K 基准建议在 release 模式运行(debug 模式预填充可能 >5 分钟)

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
// VectorStore trait 必须在 scope 内才能调用 HnswStore 的 top_k/upsert 方法
// (Rust trait 方法解析要求 trait 在作用域内,即使 impl 已写明)
use nexus_contracts::VectorStore;
use repo_wiki::{HnswStore, VectorIndex};
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
///
/// # 周期分析
/// 模数 100003 为质数,`id * 7 % 100003` 周期 = 100003(因 gcd(7, 100003)=1)。
/// 故 id ∈ [0, 100003) 内向量唯一,覆盖 100K entry 规模(P2-W8.1.4)无重复。
/// 早期版本用模数 1000,周期仅 1000,10K 规模下 id=0 与 id=1000/4000 产生
/// 完全相同向量,导致 HNSW top1 命中不确定条目。
fn make_vector(id: u64, dim: usize) -> Vec<f32> {
    (0..dim)
        .map(|j| {
            // 混合 id 和 dim_index,质数模数 100003 保证周期 > 100K
            let h = id
                .wrapping_mul(7)
                .wrapping_add((j as u64).wrapping_mul(13))
                .wrapping_mul(31);
            let v = (h % 100003) as f32 / 100003.0;
            v + 0.001 // 避免零向量
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

/// 预填充 HNSW 向量存储
///
/// 在 setup 阶段执行(不计入测量)。逐条 `upsert` 而非 `insert_batch`,
/// 因为 `insert_batch` 内部仍是逐条 upsert 循环,无性能差异。
/// 预填充 10K 条目约需 1-3 秒(HNSW 构建开销)。
fn prefill_hnsw(store: &HnswStore, count: usize) {
    for i in 0..count {
        let vec = make_vector(i as u64, VECTOR_DIM);
        store
            .upsert(&format!("hnsw-vec-{i}"), &vec, ())
            .unwrap_or_else(|e| panic!("预填充 hnsw upsert 失败 @#{i}: {e}"));
    }
}

// ============================================================
// HnswStore 基准(P2-W8.1.3:10K entry p95 < 50ms)
// ============================================================

/// HNSW 10K entry 规模的 HnswStore 引用与查询向量,供 bench 共用 setup
struct Hnsw10KFixture {
    store: HnswStore,
    query: Vec<f32>,
}

impl Hnsw10KFixture {
    fn new() -> Self {
        let store = HnswStore::with_dim(VECTOR_DIM);
        prefill_hnsw(&store, 10_000);
        // 查询向量与 vec-0 相同,保证命中 top1 且结果稳定
        let query = make_vector(0, VECTOR_DIM);
        Self { store, query }
    }
}

/// HnswStore 10K entry KNN 延迟基准
///
/// 测量 10K entry 规模下 `top_k(query, 5, "")` 的平均延迟。
/// 预期:均值 < 5ms(HNSW 10K 规模下 search 极快)。
///
/// 此基准用于建立延迟基线,具体 p95 阈值断言由
/// `hnsw_10k_p95_search_latency` 通过 `iter_custom` 收集样本计算。
fn hnsw_10k_search_latency(c: &mut Criterion) {
    let fixture = Hnsw10KFixture::new();

    let mut group = c.benchmark_group("hnsw_10k_search_latency");
    // 100 样本保证统计稳定(HNSW search 单次微秒级,100 次总耗时 < 1s)
    group.sample_size(100);

    group.bench_function("knn_top5", |b| {
        b.iter(|| {
            let results = fixture
                .store
                .top_k(black_box(&fixture.query), black_box(TOP_K), "")
                .expect("hnsw search 失败");
            black_box(results);
        });
    });
    group.finish();
}

/// HnswStore 10K entry p95 延迟基准(iter_custom 收集单次延迟样本)
///
/// 验证 spec.md P2-W8.1.3 红线:**10K entry p95 < 50ms**。
///
/// # 设计
/// `iter_custom` 让我们手动控制每次迭代的计时,收集每单次 `top_k` 调用的
/// 延迟样本。criterion 调用 `iters` 次(由 --measurement-time 决定,通常 100-3000),
/// 收集后我们排序取 p95,通过 `eprintln!` 输出供人工核验。
///
/// # 红线断言
/// 真正的"p95 < 50ms"红线断言不在此 bench 中(因 `iter_custom` 返回 `Duration`
/// 总耗时而非单次,且 bench 不应 panic 失败 CI)。红线断言由独立测试
/// `tests/hnsw_p95_test.rs::test_hnsw_10k_p95_below_50ms` 守护,在 CI 中可执行。
///
/// # 输出示例
/// ```text
/// [hnsw_10k_p95] samples=100, mean=1.2ms, p50=1.1ms, p95=2.3ms, p99=3.8ms, p95<50ms=true
/// ```
fn hnsw_10k_p95_search_latency(c: &mut Criterion) {
    let fixture = Hnsw10KFixture::new();

    let mut group = c.benchmark_group("hnsw_10k_p95_search_latency");
    group.sample_size(100);

    group.bench_function("p95_latency", |b| {
        b.iter_custom(|iters| {
            let mut latencies: Vec<Duration> = Vec::with_capacity(iters as usize);
            let start_total = Instant::now();
            for _ in 0..iters {
                let start = Instant::now();
                black_box(
                    fixture
                        .store
                        .top_k(black_box(&fixture.query), black_box(TOP_K), "")
                        .expect("hnsw search 失败"),
                );
                latencies.push(start.elapsed());
            }
            let total = start_total.elapsed();

            // 排序后取百分位
            latencies.sort_unstable();
            let n = latencies.len();
            // WHY 括号包裹 cast:`as usize` 优先级低于方法调用,
            // 不加括号会被解析为 `n as (f64 * 0.95 as usize).min(...)`,编译失败
            let p50 = latencies[((n as f64 * 0.50) as usize).min(n.saturating_sub(1))];
            let p95 = latencies[((n as f64 * 0.95) as usize).min(n.saturating_sub(1))];
            let p99 = latencies[((n as f64 * 0.99) as usize).min(n.saturating_sub(1))];
            let mean = latencies.iter().sum::<Duration>() / n.max(1) as u32;

            eprintln!(
                "[hnsw_10k_p95] samples={n}, mean={mean:?}, p50={p50:?}, p95={p95:?}, p99={p99:?}, \
                 p95<50ms={}",
                p95 < Duration::from_millis(50)
            );

            total
        })
    });
    group.finish();
}

// ============================================================
// HnswStore 100K 基准(P2-W8.3.1:100K entry p95 < 50ms,spec 索引 SLO)
// ============================================================

/// HNSW 100K entry 规模的 HnswStore 引用与查询向量,供 bench 共用 setup
///
/// 100K entry 预填充约需 30-60 秒(release 模式),debug 模式可能 >5 分钟。
/// 建议在 release 模式运行:
/// ```bash
/// cargo bench --release -p repo-wiki --bench vector_bench -- "hnsw_100k"
/// ```
struct Hnsw100KFixture {
    store: HnswStore,
    query: Vec<f32>,
}

impl Hnsw100KFixture {
    fn new() -> Self {
        let store = HnswStore::with_dim(VECTOR_DIM);
        prefill_hnsw(&store, 100_000);
        // 查询向量与 vec-0 相同,保证命中 top1 且结果稳定
        let query = make_vector(0, VECTOR_DIM);
        Self { store, query }
    }
}

/// HnswStore 100K entry KNN 延迟基准
///
/// 测量 100K entry 规模下 `top_k(query, 5, "")` 的平均延迟。
/// 预期:均值 < 10ms(HNSW O(log N) 搜索,100K 规模仍极快)。
///
/// 此基准用于建立延迟基线,具体 p95 阈值断言由
/// `hnsw_100k_p95_search_latency` 通过 `iter_custom` 收集样本计算,
/// 真正的"p95 < 50ms"红线断言由独立测试守护(见 `tests/hnsw_p95_test.rs`)。
fn hnsw_100k_search_latency(c: &mut Criterion) {
    let fixture = Hnsw100KFixture::new();

    let mut group = c.benchmark_group("hnsw_100k_search_latency");
    // 100 样本保证统计稳定(HNSW search 单次微秒级,100 次总耗时 < 1s)
    group.sample_size(100);

    group.bench_function("knn_top5", |b| {
        b.iter(|| {
            let results = fixture
                .store
                .top_k(black_box(&fixture.query), black_box(TOP_K), "")
                .expect("hnsw search 失败");
            black_box(results);
        });
    });
    group.finish();
}

/// HnswStore 100K entry p95 延迟基准(iter_custom 收集单次延迟样本)
///
/// 验证 spec.md P2-W8.3.1 红线:**100K entry p95 < 50ms**(索引 SLO)。
///
/// # 设计
/// 与 `hnsw_10k_p95_search_latency` 结构一致,`iter_custom` 手动控制每次迭代的计时,
/// 收集每单次 `top_k` 调用的延迟样本。criterion 调用 `iters` 次,
/// 收集后排序取 p95,通过 `eprintln!` 输出供人工核验。
///
/// # 红线断言
/// 真正的"p95 < 50ms"红线断言不在此 bench 中(因 `iter_custom` 返回 `Duration`
/// 总耗时而非单次,且 bench 不应 panic 失败 CI)。红线断言由独立测试
/// `tests/hnsw_p95_test.rs::test_hnsw_100k_p95_below_50ms` 守护。
///
/// # 输出示例
/// ```text
/// [hnsw_100k_p95] samples=100, mean=2.5ms, p50=2.2ms, p95=5.1ms, p99=8.3ms, p95<50ms=true
/// ```
fn hnsw_100k_p95_search_latency(c: &mut Criterion) {
    let fixture = Hnsw100KFixture::new();

    let mut group = c.benchmark_group("hnsw_100k_p95_search_latency");
    group.sample_size(100);

    group.bench_function("p95_latency", |b| {
        b.iter_custom(|iters| {
            let mut latencies: Vec<Duration> = Vec::with_capacity(iters as usize);
            let start_total = Instant::now();
            for _ in 0..iters {
                let start = Instant::now();
                black_box(
                    fixture
                        .store
                        .top_k(black_box(&fixture.query), black_box(TOP_K), "")
                        .expect("hnsw search 失败"),
                );
                latencies.push(start.elapsed());
            }
            let total = start_total.elapsed();

            // 排序后取百分位
            latencies.sort_unstable();
            let n = latencies.len();
            let p50 = latencies[((n as f64 * 0.50) as usize).min(n.saturating_sub(1))];
            let p95 = latencies[((n as f64 * 0.95) as usize).min(n.saturating_sub(1))];
            let p99 = latencies[((n as f64 * 0.99) as usize).min(n.saturating_sub(1))];
            let mean = latencies.iter().sum::<Duration>() / n.max(1) as u32;

            eprintln!(
                "[hnsw_100k_p95] samples={n}, mean={mean:?}, p50={p50:?}, p95={p95:?}, p99={p99:?}, \
                 p95<50ms={}",
                p95 < Duration::from_millis(50)
            );

            total
        })
    });
    group.finish();
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
    search_under_write_load,
    hnsw_10k_search_latency,
    hnsw_10k_p95_search_latency,
    hnsw_100k_search_latency,
    hnsw_100k_p95_search_latency
);
criterion_main!(benches);
