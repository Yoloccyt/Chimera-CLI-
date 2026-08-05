//! GQEP 聚集性能基准 — criterion 基准测试
//!
//! 对应 SubTask 24.6:测量 10/50/100 操作聚集延迟
//!
//! 运行方式:
//! ```powershell
//! cargo bench -p gqep-executor
//! ```

use criterion::{criterion_group, criterion_main, Criterion};
use event_bus::EventBus;
use gqep_executor::{GqepConfig, GqepExecutor, GqepFuture};
use qeep_protocol::{QeepError, QeepProtocol};

/// 基准测试:不同规模操作聚集的延迟
///
/// 测量 10/50/100 个即时操作(无 sleep)经 GQEP 聚集的耗时,
/// 包含 QEEP entangle 包裹 + FuturesUnordered 流式处理开销。
fn bench_gather(c: &mut Criterion) {
    // 创建 tokio runtime 供异步基准使用
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("gather");
    group.sample_size(30); // 降低样本数以加速基准(默认 100)

    for size in [10usize, 50, 100] {
        group.bench_with_input(format!("{size}_ops"), &size, |b, &size| {
            b.to_async(&rt).iter(|| async {
                let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
                let futures: Vec<GqepFuture<String>> = (0..size)
                    .map(|i| {
                        Box::pin(async move { Ok(format!("result-{i}")) }) as GqepFuture<String>
                    })
                    .collect();
                executor.gather(futures).await
            });
        });
    }

    group.finish();
}

/// 基准测试:QEEP `entangle` 单独注册延迟
///
/// 测量 `QeepProtocol::entangle` 包裹**立即完成** future 的端到端开销
/// (UUID 生成 + DashMap 注册 + Ack 状态更新 + OrphanGuard 创建 +
/// timeout 包裹 + 完成清理),不含任何业务逻辑。
///
/// # WHY(ADR-048 触发条件 3 闭环)
/// ADR-048 复审触发条件 3:entangle 调用延迟 >100μs 即触发跨层渗透复审。
/// 本基准提供 entangle 单独延迟数值,与 `bench_gather` 的 `10_ops` 整体
/// 延迟对比,量化 entangle 在 gather 中的占比——P9-T6 审查建议提前至
/// P9-T9 闭环测量,此基准为闭环提供数据依据。
///
/// # 对比口径说明
/// - `1_op`:单次 entangle 延迟(并行 gather 场景下每个 op 的 entangle 成本)
/// - `10_ops`:10 次串行 entangle(作为上界对比;gather 中 entangle 经
///   FuturesUnordered 并发执行,实际占比低于串行上界)
fn bench_entangle_alone(c: &mut Criterion) {
    // 创建 tokio runtime 供异步基准使用(与 bench_gather 一致)
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
    // 复用协议实例:只测量 entangle 调用本身,排除构造开销
    let protocol = QeepProtocol::new(qeep_protocol::DEFAULT_TIMEOUT);

    let mut group = c.benchmark_group("entangle_alone");
    group.sample_size(30); // 与 bench_gather 一致的样本数

    for size in [1usize, 10] {
        group.bench_with_input(format!("{size}_ops"), &size, |b, &size| {
            b.to_async(&rt).iter(|| async {
                // 立即完成的 future,无 sleep/业务逻辑;
                // 累积返回值防止编译器内联消除 entangle 调用
                let mut last: Result<&'static str, QeepError> = Ok("ok");
                for _ in 0..size {
                    last = protocol.entangle(async { Ok("ok") }).await;
                }
                last
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_gather, bench_entangle_alone);
criterion_main!(benches);
