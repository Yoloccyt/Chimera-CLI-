//! OSA 掩码计算基准测试
//!
//! 对应 SubTask 11.1:引入 criterion 基准测试框架
//!
//! 基准场景:构造 TaskProfile(50 工具 + 2000 文件 + 50 记忆 + 100 操作 + 10 任务),
//! 测量 `OmniSparseCoordinator::compute_all_masks` 延迟。
//!
//! WHY 使用 block_on:`compute_all_masks` 为 async fn, criterion 默认同步,
//! 通过 `Runtime::new().block_on()` 在同步上下文中调用 async 方法。

use criterion::{criterion_group, criterion_main, Criterion};
use event_bus::EventBus;
use osa_coordinator::{
    AffectedScope, FileId, MemoryId, OmniSparseCoordinator, OperationId, RiskLevel, TaskId,
    TaskProfile, TaskType, TimePressure, ToolId,
};

/// 构造测试用 TaskProfile(模拟真实任务规模)
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
    }
}

/// 基准:OSA 五维度掩码计算
fn bench_compute_all_masks(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建应成功");
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    c.bench_function("compute_all_masks", |b| {
        b.iter(|| {
            let profile = make_profile();
            rt.block_on(coord.compute_all_masks(&profile))
                .expect("掩码计算应成功");
        });
    });
}

criterion_group!(benches, bench_compute_all_masks);
criterion_main!(benches);
