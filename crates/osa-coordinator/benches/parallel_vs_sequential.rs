//! Task 6: OSA 五维度并行 vs 顺序延迟对比基准
//!
//! 对应 Task 6:验证 `std::thread::scope` 并行计算相比顺序调用的延迟降低。
//!
//! # 基准场景
//!
//! - 50 工具 + 2000 文件 + 50 记忆 + 100 操作 + 10 任务(模拟真实任务规模)
//! - 顺序基准:直接顺序调用 5 个 `compute_*_mask` 同步方法
//! - 并行版:调用 `compute_all_masks`(内部 std::thread::scope 并行)
//!
//! # WHY 顺序基准用 5 个 compute_*_mask 而非旧版 compute_all_masks
//!
//! `compute_all_masks` 已改为并行(Task 6),无法作为"顺序基准"。
//! 直接顺序调用 5 个 `compute_*_mask` 方法是等价的顺序版(省略事件发布,
//! 因事件发布是副作用,非计算核心,且 bench 关注纯计算延迟)。
//!
//! # 预期结果
//!
//! 并行版延迟应低于顺序版(理论加速比 ≈ 2-5x,取决于 OS 线程调度开销)。
//! 5 个维度计算量差异较大(context 2000 文件最重,routing 50 工具最轻),
//! 并行版受"最重维度"限制(Amdahl 定律),但仍应显著快于顺序版。

use criterion::{criterion_group, criterion_main, Criterion};
use event_bus::EventBus;
use osa_coordinator::{
    AffectedScope, FileId, MemoryId, OmniSparseCoordinator, OperationId, RiskLevel, TaskId,
    TaskProfile, TaskType, TimePressure, ToolId,
};

/// 构造测试用 TaskProfile(50 工具 + 2000 文件 + 50 记忆 + 100 操作 + 10 任务)
fn make_profile() -> TaskProfile {
    TaskProfile {
        task_id: TaskId::new("t-1"),
        task_type: TaskType::Read,
        complexity_score: 0.6,
        risk_level: RiskLevel::Medium,
        time_pressure: TimePressure::Low,
        affected_scope: AffectedScope::Local,
        available_tools: (0..50).map(|i| ToolId::new(format!("tool-{i}"))).collect(),
        available_files: (0..2000)
            .map(|i| FileId::new(format!("file-{i}")))
            .collect(),
        available_memories: (0..50).map(|i| MemoryId::new(format!("mem-{i}"))).collect(),
        recent_operations: (0..100)
            .map(|i| OperationId::new(format!("op-{i}")))
            .collect(),
        active_tasks: (0..10).map(|i| TaskId::new(format!("task-{i}"))).collect(),
        // 评分字段默认 None:基准测试 fallback 到 heuristic_scores 的行为
        routing_scores: None,
        context_scores: None,
        memory_scores: None,
        // task_phase 默认 None:基准测试不涉及 S2 自适应
        task_phase: None,
    }
}

/// 顺序基准:直接顺序调用 5 个 compute_*_mask 同步方法
///
/// 模拟 Task 6 改造前的 compute_all_masks 行为(省略事件发布,
/// 因事件发布是 IO 副作用,非计算核心,bench 关注纯计算延迟)。
fn bench_sequential_compute(c: &mut Criterion) {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    c.bench_function("sequential_compute_5_masks", |b| {
        b.iter(|| {
            let profile = make_profile();
            // 顺序调用 5 个 compute_*_mask 方法(模拟 Task 6 前的 compute_all_masks)
            let _routing = coord.compute_routing_mask(&profile);
            let _context = coord.compute_context_mask(&profile);
            let _memory = coord.compute_memory_mask(&profile);
            let _audit = coord.compute_audit_mask(&profile);
            let _budget = coord.compute_budget_mask(&profile);
        });
    });
}

/// 并行版:调用 compute_all_masks(内部 std::thread::scope 并行计算)
///
/// 包含事件发布开销,但因事件发布在并行计算之后,不影响并行计算本身的延迟测量。
/// WHY 保留事件发布:保持与生产代码一致,bench 测量的是真实 compute_all_masks 延迟。
fn bench_parallel_compute_all_masks(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建应成功");
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    c.bench_function("parallel_compute_all_masks", |b| {
        b.iter(|| {
            let profile = make_profile();
            rt.block_on(coord.compute_all_masks(&profile))
                .expect("掩码计算应成功");
        });
    });
}

/// 并行版(无事件发布):仅测量 std::thread::scope 并行计算延迟
///
/// WHY 单独测量:与 bench_parallel_compute_all_masks 对比,可分离事件发布开销
/// 与并行计算开销,验证并行计算本身的延迟降低。
fn bench_parallel_compute_only(c: &mut Criterion) {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    c.bench_function("parallel_compute_only_no_event", |b| {
        b.iter(|| {
            let profile = make_profile();
            // 复刻 compute_all_masks 的并行计算部分(省略校验/事件发布)
            // 用于纯并行计算延迟测量,与 bench_sequential_compute 直接对比
            std::thread::scope(|s| {
                let r_routing = s.spawn(|| coord.compute_routing_mask(&profile));
                let r_context = s.spawn(|| coord.compute_context_mask(&profile));
                let r_memory = s.spawn(|| coord.compute_memory_mask(&profile));
                let r_audit = s.spawn(|| coord.compute_audit_mask(&profile));
                let r_budget = s.spawn(|| coord.compute_budget_mask(&profile));

                let _routing = r_routing.join().expect("routing join 应成功");
                let _context = r_context.join().expect("context join 应成功");
                let _memory = r_memory.join().expect("memory join 应成功");
                let _audit = r_audit.join().expect("audit join 应成功");
                let _budget = r_budget.join().expect("budget join 应成功");
            });
        });
    });
}

criterion_group!(
    benches,
    bench_sequential_compute,
    bench_parallel_compute_all_masks,
    bench_parallel_compute_only
);
criterion_main!(benches);
