//! ModelRegistry 并发读写基准
//!
//! 对应 Task III-2 [B3]:验证 DashMap → `Arc<RwLock<HashMap>>` 优化效果。
//!
//! # 设计背景
//! `ModelRegistry` 面向小规模注册表(≤10 模型)。RwLock 读锁可并发、
//! 写锁互斥,在低基数场景下整体开销低于 DashMap 分片锁(后者需哈希分片
//! 与独立锁获取,小基数时无法摊薄分片开销)。本基准通过三个维度量化
//! 该优化的收益与边界:
//!
//! # 基准项
//! - `single_get_latency`:单线程 get 延迟基线(3/10 模型)
//! - `concurrent_get_throughput`:10 并发读吞吐,验证 RwLock 读锁并发优势
//! - `register_under_read_load`:读负载下 register 写延迟,验证写锁降级可接受性
//!
//! # 运行
//! ```bash
//! cargo bench -p model-router --bench registry_bench
//! # 快速验证(减少样本数)
//! cargo bench -p model-router --bench registry_bench -- --quick
//! ```

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use criterion::{
    black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput,
};
use model_router::{ModelInfo, ModelRegistry};
use tokio::runtime::Runtime;

/// 并发读任务数(模拟典型 L1 Core 被多模块并发查询的场景)。
const CONCURRENCY: usize = 10;

/// 每个并发任务执行的 get 次数。
/// WHY:单个 get 仅 ~50ns,而 tokio::spawn 调度开销约 1-2μs;若每任务只做
/// 一次 get,测量将被 spawn 调度主导而非锁竞争。每任务做 100 次 get
/// 把调度开销摊薄到 ~1%,使测量聚焦于 RwLock 读锁并发性能本身。
const GETS_PER_TASK: usize = 100;

/// 构造测试模型元信息。
///
/// 字段与 `types.rs` 中 `ModelInfo` 定义严格对齐:
/// `cost_per_1k_tokens: f64`、`avg_latency_ms: u64`(注意类型,非 f32/u32)。
fn make_model(id: &str) -> ModelInfo {
    ModelInfo {
        model_id: id.to_string(),
        provider: "bench".to_string(),
        cost_per_1k_tokens: 0.001,
        avg_latency_ms: 100,
        max_context: 8192,
        quality_score: 0.8,
    }
}

/// 预注册 N 个模型,返回 (registry, 已注册 id 列表)。
fn registry_with(n: usize) -> (ModelRegistry, Vec<String>) {
    let registry = ModelRegistry::new();
    let ids: Vec<String> = (0..n).map(|i| format!("model-{i}")).collect();
    for id in &ids {
        registry.register(make_model(id)).expect("预注册失败");
    }
    (registry, ids)
}

/// bench 1: 单线程 get 延迟基线。
///
/// WHY:建立无并发竞争时的读延迟基线。RwLock 读锁在无竞争时获取约
/// 20-50ns,是后续并发场景的对照参照。若并发场景延迟显著高于此基线,
/// 说明锁竞争成为瓶颈;若接近基线,说明 RwLock 并发读优化生效。
///
/// 分别测量 3 模型(典型 RouterConfig 默认配置)与 10 模型(上限配置),
/// 验证 HashMap 查找 O(1) 在小规模下不随基数显著增长。
fn single_get_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("single_get_latency");

    for size in [3usize, 10] {
        let (registry, ids) = registry_with(size);
        let target = ids[0].clone();
        group.bench_with_input(BenchmarkId::from_parameter(size), &target, |b, id| {
            b.iter(|| {
                let model = registry.get(black_box(id.as_str()));
                // debug_assert 在 release bench 下编译为空,不产生分支开销,
                // 既保证 debug 构建校验正确性,又不污染延迟测量。
                debug_assert!(model.is_some(), "get 必须命中预注册模型");
            });
        });
    }
    group.finish();
}

/// bench 2: 10 并发任务同时 get 的吞吐。
///
/// WHY:验证 RwLock 读锁可并发这一核心优势。读锁允许多个读者同时持有,
/// 因此 10 并发 get 的单次延迟应接近单线程基线,吞吐接近 10×。
/// 对比 DashMap:≤10 模型时 DashMap 默认 4 分片,每个分片独立锁,
/// 但分片本身有哈希计算 + 索引开销,在小基数下反而劣于单 RwLock。
///
/// 使用 `tokio::spawn` 将 10 个读任务分发到多 worker 线程并行执行。
/// 每任务查询不同模型 key,模拟真实路由场景中各模块分散查询(避免
/// 所有任务查同一 key 造成的不真实热点)。
fn concurrent_get_throughput(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");
    let (registry, ids) = registry_with(CONCURRENCY);
    // ModelRegistry 内部已是 Arc<RwLock<HashMap>>,Clone 廉价;此处显式
    // Arc 包装整个 registry 用于跨 spawn 任务共享(语义更清晰地表达只读共享)。
    let registry = Arc::new(registry);
    // targets 用 Arc 共享,避免每次 spawn clone String 产生分配噪音。
    let targets = Arc::new(ids);

    let mut group = c.benchmark_group("concurrent_get_throughput");
    // 每次迭代共执行 CONCURRENCY * GETS_PER_TASK 次 get,设置吞吐单位
    // 让 criterion 直接报告 ops/sec 与单次延迟。
    group.throughput(Throughput::Elements((CONCURRENCY * GETS_PER_TASK) as u64));

    group.bench_function("10_concurrent_get", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut handles = Vec::with_capacity(CONCURRENCY);
                for i in 0..CONCURRENCY {
                    let reg = Arc::clone(&registry);
                    let keys = Arc::clone(&targets);
                    handles.push(tokio::spawn(async move {
                        // 借用 Arc 内 Vec 的元素,无 String clone 分配。
                        let key: &str = &keys[i % keys.len()];
                        for _ in 0..GETS_PER_TASK {
                            let model = reg.get(black_box(key));
                            debug_assert!(model.is_some(), "并发 get 必须命中");
                        }
                    }));
                }
                for h in handles {
                    h.await.expect("并发任务 panic");
                }
            });
        });
    });

    group.finish();
}

/// bench 3: 读负载下的 register 写延迟。
///
/// WHY:验证 RwLock 在写锁持有期间读请求的降级表现。`register` 需要写锁,
/// 会短暂阻塞所有读请求;但 register 是低频操作(模型注册阶段),且小规模
/// HashMap 写入耗时极短(纳秒级),阻塞窗口可忽略。后台持续 `get` 模拟
/// 真实读负载,主线程测量 register 延迟。若延迟未显著膨胀,说明小规模
/// 注册表下写锁竞争可接受。
///
/// 使用 `iter_batched`:setup 阶段(不计入测量)清理上一次注册的临时模型,
/// routine 阶段(被测量)只执行 register,使测量精确反映写锁路径。
fn register_under_read_load(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");
    let (registry, ids) = registry_with(CONCURRENCY);
    let registry = Arc::new(registry);

    // 后台读负载:持续 get 已注册模型,制造读竞争压力。
    // WHY spawn 而非 spawn_blocking:get 是纯内存操作且持锁极短(纳秒级),
    // 在 async 上下文调用同步方法不会阻塞 runtime 过久;yield_now 让出
    // 执行权使读负载与主线程 register 真正并发交替执行。
    let read_reg = Arc::clone(&registry);
    let read_ids = Arc::new(ids);
    let reader = rt.spawn(async move {
        let mut idx = 0usize;
        loop {
            let id = &read_ids[idx % read_ids.len()];
            let _ = read_reg.get(id);
            idx += 1;
            tokio::task::yield_now().await;
        }
    });

    // 计数器与上次注册 id:用于 iter_batched 的 setup 清理。
    // WHY 用 RefCell:bench 在单线程 criterion 调用,无需原子锁;
    // RefCell 在 setup 闭包内 borrow_mut 修改上次 id,语义清晰。
    let counter = AtomicU64::new(0);
    let prev = std::cell::RefCell::new(None::<String>);

    let mut group = c.benchmark_group("register_under_read_load");
    // register 含写锁,单次迭代略慢;降低样本数减少抖动(默认 100 → 50)。
    group.sample_size(50);

    group.bench_function("register", |b| {
        b.iter_batched(
            || {
                // setup(不计入测量):清理上次注册项,生成新唯一 id。
                let mut p = prev.borrow_mut();
                if let Some(prev_id) = p.take() {
                    // 清理失败不致命(仅导致注册表缓慢增长),忽略错误。
                    let _ = registry.unregister(&prev_id);
                }
                let new_id = format!("bench-new-{}", counter.fetch_add(1, Ordering::Relaxed));
                *p = Some(new_id.clone());
                new_id
            },
            |new_id| {
                // routine(被测量):只测 register 写锁路径。
                registry
                    .register(make_model(&new_id))
                    .expect("register 失败");
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();

    // 清理后台读任务,避免 runtime 残留。
    reader.abort();
}

criterion_group!(
    benches,
    single_get_latency,
    concurrent_get_throughput,
    register_under_read_load
);
criterion_main!(benches);
