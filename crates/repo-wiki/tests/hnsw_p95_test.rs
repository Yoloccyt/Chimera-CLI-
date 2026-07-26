//! HnswStore 延迟红线守护测试(P2-W8.1.3 + P2-W8.3.1)
//!
//! 对应架构层: L5 Knowledge
//! 对应任务: P2-W8.1.3(10K) + P2-W8.1.4(100K OOM) + P2-W8.3.1(100K p95)
//! 对应 spec 红线: 10K/100K entry KNN p95 < 50ms
//!
//! # 设计
//! 此测试为性能红线守护,遵循 hcw-window 性能测试惯例标记 `#[ignore]`,
//! 默认不在 `cargo test` 中执行,需显式:
//! ```bash
//! # 10K p95 红线
//! cargo test --release -p repo-wiki --test hnsw_p95_test -- --ignored --nocapture test_hnsw_10k_p95
//! # 100K p95 红线(P2-W8.3.1,spec 索引 SLO)
//! cargo test --release -p repo-wiki --test hnsw_p95_test -- --ignored --nocapture test_hnsw_100k_p95
//! # 100K 不 OOM 红线(P2-W8.1.4)
//! cargo test --release -p repo-wiki --test hnsw_p95_test -- --ignored --nocapture test_hnsw_100k_no_oom
//! ```
//!
//! # 为什么需要 release 模式
//! debug build 下 HNSW search 未优化,p95 可能 > 50ms;
//! release build 下优化后 p95 应远低于 50ms(预期 < 5ms)。
//! 红线 50ms 是生产环境(release)的性能契约,debug 下断言会误报。
//!
//! # 样本数选择
//! 1000 次 search 足以稳定估算 p95(统计学上 n≥100 即可,
//! 1000 提供更高置信度并平滑抖动)。预填充 10K entry 约 1-3 秒,
//! 100K entry 约 30-60 秒(release)。1000 次 search 约 1-2 秒。
//!
//! # 与 bench 的分工
//! - **本测试**(CI 守护):严格断言 p95 < 50ms,失败阻塞 CI
//! - **vector_bench.rs::hnsw_{10k,100k}_p95_search_latency**(人工核验):
//!   输出 p50/p95/p99/mean 供性能分析,不阻塞 CI

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use nexus_contracts::{VectorStore, VectorStoreExt};
use repo_wiki::HnswStore;

/// 向量维度(与 CLV 一致,512-dim)
const VECTOR_DIM: usize = 512;

/// KNN 返回的 Top-K 数量(与 bench 一致)
const TOP_K: usize = 5;

/// 预填充条目数(10K entry,spec P2-W8.1.3 红线规模)
const ENTRY_COUNT: usize = 10_000;

/// 延迟样本数(1000 次保证 p95 稳定)
const SAMPLE_COUNT: usize = 1000;

/// p95 延迟红线:50ms(spec P2-W8.1.3)
const P95_THRESHOLD_MS: u64 = 50;

/// 生成确定性伪随机向量(与 bench 中 `make_vector` 一致)
///
/// WHY 确定性:避免引入 rand 依赖,且消除随机性导致的抖动干扰测量。
/// 每个分量基于 `(id, dim_index)` 派生,质数模数 100003 保证周期 > 100K,
/// 覆盖 10K-100K entry 规模无向量重复(早期版本模数 1000 周期仅 1000,
/// 10K 规模下 id=0 与 id=4000 产生相同向量导致 HNSW 命中不确定)。
fn make_vector(id: u64, dim: usize) -> Vec<f32> {
    (0..dim)
        .map(|j| {
            let h = id
                .wrapping_mul(7)
                .wrapping_add((j as u64).wrapping_mul(13))
                .wrapping_mul(31);
            let v = (h % 100003) as f32 / 100003.0;
            v + 0.001 // 避免零向量
        })
        .collect()
}

/// 预填充 HNSW 存储
fn prefill_hnsw(store: &HnswStore, count: usize) {
    for i in 0..count {
        let vec = make_vector(i as u64, VECTOR_DIM);
        store
            .upsert(&format!("hnsw-vec-{i}"), &vec, ())
            .unwrap_or_else(|e| panic!("预填充 hnsw upsert 失败 @#{i}: {e}"));
    }
}

/// 计算排序后向量的百分位
///
/// `p` ∈ [0, 1],如 0.50 / 0.95 / 0.99。
/// 使用 nearest-rank 方法(向上取整),适用于延迟分布分析。
fn percentile(sorted: &[Duration], p: f64) -> Duration {
    assert!(!sorted.is_empty(), "百分位计算需非空样本");
    let n = sorted.len();
    // nearest-rank: idx = ceil(p * n) - 1,clamp 到 [0, n-1]
    let idx = ((p * n as f64).ceil() as usize)
        .saturating_sub(1)
        .min(n - 1);
    sorted[idx]
}

/// HnswStore 10K entry p95 延迟红线守护
///
/// # 红线
/// p95 < 50ms(spec P2-W8.1.3)
///
/// # 失败处置
/// 若 p95 ≥ 50ms,测试 panic 并输出完整延迟分布(p50/p95/p99/mean/max),
/// 便于定位性能瓶颈(ef_construction/ef_search/M 参数调优、维度过高、
/// DistCosine 计算开销等)。
///
/// # 运行方式
/// ```bash
/// cargo test --release -p repo-wiki --test hnsw_p95_test -- --ignored --nocapture
/// ```
#[test]
#[ignore = "性能红线测试:需 release 模式运行,见模块文档"]
fn test_hnsw_10k_p95_below_50ms() {
    // 1. 预填充 10K entry
    let store = HnswStore::with_dim(VECTOR_DIM);
    let fill_start = Instant::now();
    prefill_hnsw(&store, ENTRY_COUNT);
    let fill_elapsed = fill_start.elapsed();

    // 验证预填充正确
    let stats = store.stats().expect("stats 失败");
    assert_eq!(
        stats.entry_count, ENTRY_COUNT,
        "预填充后条目数应为 {ENTRY_COUNT}"
    );

    // 2. 收集 1000 次 search 延迟样本
    let query = make_vector(0, VECTOR_DIM);
    let mut latencies: Vec<Duration> = Vec::with_capacity(SAMPLE_COUNT);

    // WHY 先做一次 warmup:HNSW 首次 search 可能触发缓存填充,
    // 不计入样本避免冷启动偏高
    let _ = store.top_k(&query, TOP_K, "").expect("warmup search 失败");

    for _ in 0..SAMPLE_COUNT {
        let start = Instant::now();
        let results = store.top_k(&query, TOP_K, "").expect("search 失败");
        let elapsed = start.elapsed();
        assert_eq!(
            results.len(),
            TOP_K,
            "top_k 应返回 {TOP_K} 条结果(10K entry 足够)"
        );
        latencies.push(elapsed);
    }

    // 3. 排序并取百分位
    latencies.sort_unstable();
    let p50 = percentile(&latencies, 0.50);
    let p95 = percentile(&latencies, 0.95);
    let p99 = percentile(&latencies, 0.99);
    let mean = latencies.iter().sum::<Duration>() / latencies.len() as u32;
    let max = latencies[latencies.len() - 1];

    let threshold = Duration::from_millis(P95_THRESHOLD_MS);

    // 4. 输出完整延迟分布(无论成功失败都输出,便于性能分析)
    eprintln!(
        "[hnsw_10k_p95] entries={ENTRY_COUNT}, samples={}, fill_time={fill_elapsed:?}\n\
         [hnsw_10k_p95] mean={mean:?}, p50={p50:?}, p95={p95:?}, p99={p99:?}, max={max:?}\n\
         [hnsw_10k_p95] threshold={threshold:?}, p95<threshold={}",
        latencies.len(),
        p95 < threshold
    );

    // 5. 红线断言
    assert!(
        p95 < threshold,
        "P2-W8.1.3 红线违规:HnswStore 10K entry p95={p95:?} ≥ {threshold:?}\n\
         延迟分布:mean={mean:?}, p50={p50:?}, p95={p95:?}, p99={p99:?}, max={max:?}\n\
         可能原因:① ef_search 过低(召回率不足触发回退);② 维度过高(512-dim DistCosine 计算重);\n\
         ③ HNSW 参数 M/max_layer 不当;④ release 优化未启用(请用 --release 运行)"
    );
}

/// HnswStore 10K entry 功能正确性验证
///
/// 验证 10K entry 规模下 HnswStore 功能正常(不 OOM、search 返回正确结果)。
///
/// # 为什么标记 `#[ignore]`
/// debug 模式下 HNSW 构建 10K entry × 512-dim 是 CPU 密集型操作
/// (每次 `insert` 涉及 DistCosine 距离计算 + 图结构更新,未优化),
/// 单次预填充可能 > 60 秒导致 `cargo test` 默认超时。
/// release 模式下构建约 1-3 秒,可通过 `--ignored` 显式运行:
/// ```bash
/// cargo test --release -p repo-wiki --test hnsw_p95_test -- --ignored --nocapture test_hnsw_10k_functional
/// ```
///
/// 此测试作为 P2-W8.1.4(100K 不 OOM)的前置功能验证。
#[test]
#[ignore = "性能测试:debug 模式 HNSW 10K 构建超 60s,需 release 模式运行"]
fn test_hnsw_10k_functional_correctness() {
    let store = HnswStore::with_dim(VECTOR_DIM);
    prefill_hnsw(&store, ENTRY_COUNT);

    // 验证条目数
    let stats = store.stats().expect("stats 失败");
    assert_eq!(stats.entry_count, ENTRY_COUNT);
    assert_eq!(stats.dimension, VECTOR_DIM);
    assert_eq!(stats.backend, nexus_contracts::VectorBackend::Hnsw);

    // 验证 search 返回正确 top1(查询 vec-0 应命中 hnsw-vec-0)
    let query = make_vector(0, VECTOR_DIM);
    let results = store.top_k(&query, 1, "").expect("search 失败");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "hnsw-vec-0");
    // 相同向量余弦相似度应接近 1.0
    assert!(
        (results[0].score - 1.0).abs() < 1e-3,
        "top1 score 应接近 1.0,实际: {}",
        results[0].score
    );

    // 验证 top_k 返回正确数量
    let results = store.top_k(&query, TOP_K, "").expect("search 失败");
    assert_eq!(results.len(), TOP_K);

    // 验证结果按 score 降序
    for i in 1..results.len() {
        assert!(
            results[i - 1].score >= results[i].score,
            "结果应按 score 降序,但 [{}]={} < [{}]={}",
            i - 1,
            results[i - 1].score,
            i,
            results[i].score
        );
    }
}

// ============================================================
// P2-W8.1.4: 100K entry 不 OOM 测试
// ============================================================

/// 100K entry 预填充条目数(spec P2-W8.1.4 红线规模)
const ENTRY_COUNT_100K: usize = 100_000;

/// 100K entry 内存占用上限:1GB
///
/// WHY 1GB:100K entry × 512-dim × 4 byte = 195MB 向量数据;
/// HNSW 图结构开销约 1.5-3x = 293-586MB;总 < 800MB。
/// 1GB 为安全上限,超过则判定 OOM 风险。
const MEMORY_LIMIT_BYTES: u64 = 1024 * 1024 * 1024;

/// HnswStore 100K entry 不 OOM 红线守护(P2-W8.1.4)
///
/// # 红线
/// 100K entry 规模下 HnswStore 不 OOM,且 search 功能正常。
///
/// # 设计
/// 100K entry × 512-dim 是 spec P2-W8.1.4 规定的生产规模上限。
/// 此测试验证:
/// 1. 预填充 100K entry 能完成(不 OOM)
/// 2. stats().entry_count == 100000
/// 3. search 返回正确结果(top1 命中预期条目)
/// 4. 内存占用 < 1GB(VectorStoreStats.memory_bytes 仅向量数据,不含图结构)
///
/// # 运行方式
/// ```bash
/// cargo test --release -p repo-wiki --test hnsw_p95_test -- --ignored --nocapture test_hnsw_100k
/// ```
///
/// # 为什么需要 release 模式
/// debug build 下 HNSW::insert 未优化,100K 预填充可能需要 10+ 分钟;
/// release build 下约 30-60 秒。且 OOM 测试应在生产环境(release)下验证。
#[test]
#[ignore = "性能红线测试:需 release 模式运行,100K 预填充约 30-60s"]
fn test_hnsw_100k_no_oom() {
    // 1. 预填充 100K entry
    let store = HnswStore::with_dim(VECTOR_DIM);
    let fill_start = Instant::now();
    prefill_hnsw(&store, ENTRY_COUNT_100K);
    let fill_elapsed = fill_start.elapsed();

    // 2. 验证条目数(不 OOM 的直接证据:预填充完成且条目数正确)
    let stats = store.stats().expect("stats 失败");
    assert_eq!(
        stats.entry_count, ENTRY_COUNT_100K,
        "预填充后条目数应为 {ENTRY_COUNT_100K}"
    );
    assert_eq!(stats.dimension, VECTOR_DIM);
    assert_eq!(stats.backend, nexus_contracts::VectorBackend::Hnsw);

    // 3. 验证内存占用(stats.memory_bytes 仅向量数据,不含 HNSW 图结构)
    // 100K × 512 × 4 = 204,800,000 bytes ≈ 195MB
    let expected_min_memory = (ENTRY_COUNT_100K * VECTOR_DIM * std::mem::size_of::<f32>()) as u64;
    assert!(
        stats.memory_bytes >= expected_min_memory,
        "内存占用应 ≥ {expected_min_memory} bytes(100K × 512 × 4),实际: {}",
        stats.memory_bytes
    );
    assert!(
        stats.memory_bytes < MEMORY_LIMIT_BYTES,
        "内存占用应 < 1GB,实际: {} bytes ({:.2} MB)",
        stats.memory_bytes,
        stats.memory_bytes as f64 / 1024.0 / 1024.0
    );

    // 4. 验证 search 功能正常(warmup + 正式 search)
    let query = make_vector(0, VECTOR_DIM);
    let _ = store.top_k(&query, TOP_K, "").expect("warmup search 失败");

    let search_start = Instant::now();
    let results = store.top_k(&query, TOP_K, "").expect("100K search 失败");
    let search_elapsed = search_start.elapsed();

    assert_eq!(
        results.len(),
        TOP_K,
        "top_k 应返回 {TOP_K} 条结果(100K entry 足够)"
    );
    // top1 应为 hnsw-vec-0(查询向量与 vec-0 相同)
    assert_eq!(results[0].id, "hnsw-vec-0");
    assert!(
        (results[0].score - 1.0).abs() < 1e-3,
        "top1 score 应接近 1.0,实际: {}",
        results[0].score
    );

    // 5. 输出完整诊断信息
    eprintln!(
        "[hnsw_100k_no_oom] entries={}, fill_time={fill_elapsed:?}, search_time={search_elapsed:?}\n\
         [hnsw_100k_no_oom] vector_memory={:.2} MB (limit: {:.2} MB), within_limit={}\n\
         [hnsw_100k_no_oom] top1={}, top1_score={:.6}",
        stats.entry_count,
        stats.memory_bytes as f64 / 1024.0 / 1024.0,
        MEMORY_LIMIT_BYTES as f64 / 1024.0 / 1024.0,
        stats.memory_bytes < MEMORY_LIMIT_BYTES,
        results[0].id,
        results[0].score
    );
}

// ============================================================
// P2-W8.3.1: 100K entry p95 延迟红线守护(spec.md 索引 SLO)
// ============================================================

/// HnswStore 100K entry p95 延迟红线守护(P2-W8.3.1)
///
/// # 红线
/// p95 < 50ms(spec.md KPI 表格"索引:wiki_knn @100K p95<50ms(新)")
///
/// # 设计
/// 与 `test_hnsw_10k_p95_below_50ms` 结构一致,但规模升级至 100K entry。
/// 100K entry 是 spec.md 定义的生产规模上限(P2-W8.1.4 OOM 红线 + P2-W8.3.1 延迟红线)。
/// HNSW 搜索复杂度为 O(log N),100K 与 10K 的 p95 差距应在 2-5x 以内,
/// 仍远低于 50ms 红线。
///
/// # 运行方式
/// ```bash
/// cargo test --release -p repo-wiki --test hnsw_p95_test -- --ignored --nocapture test_hnsw_100k_p95
/// ```
///
/// # 为什么需要 release 模式
/// debug build 下 HNSW::insert 未优化,100K 预填充可能需要 10+ 分钟;
/// release build 下约 30-60 秒。且性能红线应在生产环境(release)下验证。
#[test]
#[ignore = "性能红线测试:需 release 模式运行,100K 预填充约 30-60s"]
fn test_hnsw_100k_p95_below_50ms() {
    // 1. 预填充 100K entry
    let store = HnswStore::with_dim(VECTOR_DIM);
    let fill_start = Instant::now();
    prefill_hnsw(&store, ENTRY_COUNT_100K);
    let fill_elapsed = fill_start.elapsed();

    // 验证预填充正确
    let stats = store.stats().expect("stats 失败");
    assert_eq!(
        stats.entry_count, ENTRY_COUNT_100K,
        "预填充后条目数应为 {ENTRY_COUNT_100K}"
    );

    // 2. 收集 1000 次 search 延迟样本
    let query = make_vector(0, VECTOR_DIM);
    let mut latencies: Vec<Duration> = Vec::with_capacity(SAMPLE_COUNT);

    // WHY 先做一次 warmup:HNSW 首次 search 可能触发缓存填充,
    // 不计入样本避免冷启动偏高
    let _ = store.top_k(&query, TOP_K, "").expect("warmup search 失败");

    for _ in 0..SAMPLE_COUNT {
        let start = Instant::now();
        let results = store.top_k(&query, TOP_K, "").expect("search 失败");
        let elapsed = start.elapsed();
        assert_eq!(
            results.len(),
            TOP_K,
            "top_k 应返回 {TOP_K} 条结果(100K entry 足够)"
        );
        latencies.push(elapsed);
    }

    // 3. 排序并取百分位
    latencies.sort_unstable();
    let p50 = percentile(&latencies, 0.50);
    let p95 = percentile(&latencies, 0.95);
    let p99 = percentile(&latencies, 0.99);
    let mean = latencies.iter().sum::<Duration>() / latencies.len() as u32;
    let max = latencies[latencies.len() - 1];

    let threshold = Duration::from_millis(P95_THRESHOLD_MS);

    // 4. 输出完整延迟分布(无论成功失败都输出,便于性能分析)
    eprintln!(
        "[hnsw_100k_p95] entries={ENTRY_COUNT_100K}, samples={}, fill_time={fill_elapsed:?}\n\
         [hnsw_100k_p95] mean={mean:?}, p50={p50:?}, p95={p95:?}, p99={p99:?}, max={max:?}\n\
         [hnsw_100k_p95] threshold={threshold:?}, p95<threshold={}",
        latencies.len(),
        p95 < threshold
    );

    // 5. 红线断言
    assert!(
        p95 < threshold,
        "P2-W8.3.1 红线违规:HnswStore 100K entry p95={p95:?} ≥ {threshold:?}\n\
         延迟分布:mean={mean:?}, p50={p50:?}, p95={p95:?}, p99={p99:?}, max={max:?}\n\
         可能原因:① ef_search 过低(召回率不足触发回退);② 维度过高(512-dim DistCosine 计算重);\n\
         ③ HNSW 参数 M/max_layer 不当;④ release 优化未启用(请用 --release 运行);\n\
         ⑤ 100K 规模图遍历开销超出预期(检查 ef_construction 是否充分)"
    );
}
