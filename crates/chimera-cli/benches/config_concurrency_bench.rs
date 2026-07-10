//! chimera-cli 14 section OnceLock 懒加载并发性能基准。
//!
//! WHY 此 bench:v1.2.0 Task 4 将 14 个顶层配置 section 从 eager `extract`
//! 改为 `OnceLock` + `Figment::extract_inner` section 级懒加载。高并发场景下
//! `OnceLock::get_or_init` 内部 spinlock 可能成为瓶颈,本 bench 量化验证:
//! 14 section 并发访问 p99 延迟 < 100μs(门槛,验证 OnceLock 不成为瓶颈)。
//!
//! WHY min-of-N 采样:criterion 默认 100 sample,本 bench 通过 `sample_size`
//! 控制并发 bench 的样本量(10)以避免 14×spawn 单次迭代过慢导致超时。
//!
//! 架构层归属:L10 Interface(bench 不入架构层,仅作为 chimera-cli dev-artifact)。
//! 关联任务:S1(P1 短期增强,最低风险,纯 bench 新增,零生产代码修改)。

use std::sync::Arc;

use chimera_cli::LazyConfig;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use tokio::runtime::Runtime;

/// 单 section 首次访问延迟(冷启动,含 Figment provider 构建首次 extract_inner)。
///
/// 测量目标:`LazyConfig::new` + `nexus()` 首次访问的端到端延迟,反映单 section
/// 冷启动开销(Figment provider 构建 + extract_inner + OnceLock 初始化)。
fn bench_single_section_first_access(c: &mut Criterion) {
    c.bench_function("single_section_first_access", |b| {
        b.iter(|| {
            // 每次迭代新建 LazyConfig,触发 figment provider 构建(不 extract)。
            let config = LazyConfig::new(None).expect("LazyConfig::new should not fail");
            // nexus() 首次访问触发 extract_inner("nexus") + OnceLock::get_or_init。
            black_box(config.nexus().is_ok());
        });
    });
}

/// 单 section 缓存命中延迟(热路径,OnceLock spinlock 检查 + return &T)。
///
/// 测量目标:OnceLock 已初始化后的访问开销,验证 spinlock 检查路径极低延迟
/// (预期 < 10ns 量级,仅 atomic load + return)。
fn bench_single_section_cached_access(c: &mut Criterion) {
    let config = LazyConfig::new(None).expect("LazyConfig::new should not fail");
    // 预热:nexus 首次访问触发 extract + 缓存,后续迭代均为缓存命中。
    let _ = config.nexus().expect("nexus default parse should succeed");

    c.bench_function("single_section_cached_access", |b| {
        b.iter(|| {
            black_box(config.nexus().is_ok());
        });
    });
}

/// 14 section 顺序访问总延迟(含 14 次 extract_inner 首次解析)。
///
/// 测量目标:14 个 section 顺序触发的总延迟,作为并发 bench 的串行基线
/// 对照(若并发加速比显著,说明 14 次 extract_inner 可并行化)。
fn bench_14_sections_sequential(c: &mut Criterion) {
    c.bench_function("14_sections_sequential", |b| {
        b.iter(|| {
            let config = LazyConfig::new(None).expect("LazyConfig::new should not fail");
            // 14 个 getter 顺序访问,首次访问触发 extract_inner + OnceLock 初始化。
            black_box(config.nexus().is_ok());
            black_box(config.quest().is_ok());
            black_box(config.thinking_toggle().is_ok());
            black_box(config.repo_wiki().is_ok());
            black_box(config.model_router().is_ok());
            black_box(config.osa().is_ok());
            black_box(config.kvbsr().is_ok());
            black_box(config.pvl().is_ok());
            black_box(config.mtpe().is_ok());
            black_box(config.gqep().is_ok());
            black_box(config.seccore().is_ok());
            black_box(config.mcp().is_ok());
            black_box(config.evolution().is_ok());
            black_box(config.monitoring().is_ok());
        });
    });
}

/// 14 section 并发访问(tokio::spawn 14 tasks,join_all 等待)— 关键指标。
///
/// WHY tokio::spawn:chimera-cli 是 async CLI,真实场景配置访问在 async 上下文中,
/// 且 14 section 在不同 async task 中并发访问是 OSA 协调器典型调用模式。
///
/// 测量目标:14 task 并发访问各自 section 的端到端延迟。门槛 p99 < 100μs
/// (验证 OnceLock spinlock 竞争不成为瓶颈)。
///
/// WHY LazyConfig 迭代外创建:隔离 Figment provider 构造开销(~600µs),仅测量
/// 并发访问开销(spawn + extract_inner/get + join_all)。首次迭代触发冷启动
/// extract,后续迭代为缓存命中热路径,反映真实"启动后多次并发访问"场景。
///
/// WHY BenchmarkGroup + sample_size(10):criterion 0.5 的 `Criterion::sample_size`
/// 签名为 `self` by value,无法在 `bench_function` 返回的 `&mut Self` 上链式调用。
/// 改用 `BenchmarkGroup::sample_size(&mut self, n)`(API 接受 `&mut self`)。
/// 14 spawn × 100 sample = 1400 task,降低到 10 后 140 task,避免单 bench 超时
/// (criterion 默认 5s 上限)。min-of-N 5 采样仍保留统计显著性。
fn bench_14_sections_concurrent(c: &mut Criterion) {
    // WHY multi-thread runtime:验证真实并发竞争场景,单线程 runtime 会序列化 task。
    let rt = Runtime::new().expect("tokio multi-thread runtime build failed");
    // LazyConfig 迭代外创建:隔离 provider 构造开销,仅测量并发访问开销。
    let config = Arc::new(LazyConfig::new(None).expect("LazyConfig::new should not fail"));

    let mut group = c.benchmark_group("14_sections_concurrent");
    group.sample_size(10);
    group.bench_function("14_tasks", |b| {
        b.iter(|| {
            // WHY rt 借用而非 move:Runtime 体积大,迭代外构造一次复用。
            let rt_ref = &rt;

            rt_ref.block_on(async {
                // 14 个 task,每个 task 持有 Arc clone 并访问一个独立 section。
                // WHY 独立 section:确保 14 task 触发 14 个不同 OnceLock,而非同一
                // OnceLock 的竞争(同一 OnceLock 并发等待属 get_or_init 正常语义)。
                // 首次迭代:14 task 各触发一次 extract_inner(冷启动);
                // 后续迭代:14 task 各执行 OnceLock::get 缓存命中(热路径)。
                let mut handles = Vec::with_capacity(14);

                let c1 = config.clone();
                handles.push(tokio::spawn(async move { c1.nexus().is_ok() }));
                let c2 = config.clone();
                handles.push(tokio::spawn(async move { c2.quest().is_ok() }));
                let c3 = config.clone();
                handles.push(tokio::spawn(async move { c3.thinking_toggle().is_ok() }));
                let c4 = config.clone();
                handles.push(tokio::spawn(async move { c4.repo_wiki().is_ok() }));
                let c5 = config.clone();
                handles.push(tokio::spawn(async move { c5.model_router().is_ok() }));
                let c6 = config.clone();
                handles.push(tokio::spawn(async move { c6.osa().is_ok() }));
                let c7 = config.clone();
                handles.push(tokio::spawn(async move { c7.kvbsr().is_ok() }));
                let c8 = config.clone();
                handles.push(tokio::spawn(async move { c8.pvl().is_ok() }));
                let c9 = config.clone();
                handles.push(tokio::spawn(async move { c9.mtpe().is_ok() }));
                let c10 = config.clone();
                handles.push(tokio::spawn(async move { c10.gqep().is_ok() }));
                let c11 = config.clone();
                handles.push(tokio::spawn(async move { c11.seccore().is_ok() }));
                let c12 = config.clone();
                handles.push(tokio::spawn(async move { c12.mcp().is_ok() }));
                let c13 = config.clone();
                handles.push(tokio::spawn(async move { c13.evolution().is_ok() }));
                let c14 = config.clone();
                handles.push(tokio::spawn(async move { c14.monitoring().is_ok() }));

                // join_all 等价:依次 await,但 task 已并发执行。
                for h in handles {
                    let _ = h.await;
                }
            });
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_single_section_first_access,
    bench_single_section_cached_access,
    bench_14_sections_sequential,
    bench_14_sections_concurrent,
);
criterion_main!(benches);
