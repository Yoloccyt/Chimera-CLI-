//! ImmuneSystem facade 性能基准 — criterion 基准测试（P5.3.6 / ADR-046 决策 8）
//!
//! 对应 ADR:ADR-046 决策 8（KPI-03 探针延迟优化 — <100ms 实现路径）
//! 对应任务:P5.3.6（criterion 基准 + KPI-03 验证）
//!
//! # 基准清单
//! 1. `memory_paradox_probe_latency` — MemoryParadox 单探针延迟
//! 2. `reasoning_trap_probe_latency` — ReasoningTrap 单探针延迟
//! 3. `evolution_hack_probe_latency` — EvolutionHack 单探针延迟
//! 4. `full_immune_system_assessment` — 三探针并行扫描 + 级联风险评估（ADR-046 决策 8
//!    `immune_system_scan` 验收基准,p95 < 100ms）
//!
//! # KPI-03（§9.5 SLO）
//! 适应性免疫层 < 100ms。`full_immune_system_assessment` 即 ADR-046 决策 6 要求的
//! `immune_system_scan` 验收基准。三探针异步并行（FuturesUnordered）+
//! 复用既有熔断状态镜像（AtomicU8 load ~1ns）,目标 p95 < 100ms。
//!
//! # 基准配置
//! - sample_size: 100（与 debate.rs 对齐,统计显著性足够）
//! - warm_up_time: 500ms（避免冷启动干扰）

use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion};
use event_bus::{EventBus, EventMetadata, NexusEvent};
// WHY 从 parliament 顶层导入：lib.rs 已 re-export,子模块内部 use 是私有的。
//   若从 `parliament::immune_system::` 导入会触发 E0603（private struct import）。
use parliament::{
    EvolutionHackProbe, ImmuneSystem, MemoryParadoxProbe, ParadoxProbe, ReasoningTrapProbe,
    StabilityMirror,
};
use std::time::Duration;

// ============================================================
// 测试夹具 — 向 mirror 推入完整信号集
// ============================================================

/// 向 mirror 推入完整信号集,模拟"有信号"状态（避免 insufficient_data 快路径）
///
/// WHY 预填充：探针在 mirror 为空时返回 insufficient_data,走快路径无法反映真实
/// 探针算法延迟。预填充使探针执行完整算法路径,测量真实生产延迟。
fn prefill_mirror(mirror: &StabilityMirror) {
    // 推入 6 次 SkepticVeto（>5 阈值,触发 ReasoningTrap 高模式告警）
    for ts in [1000u64, 2000, 3000, 4000, 5000, 6000] {
        let event = NexusEvent::SkepticVeto {
            metadata: EventMetadata::new("parliament"),
            quest_id: "q-bench".into(),
            veto_reason: "benchmark".into(),
            frozen_capabilities: vec![],
        };
        mirror.update_from_event(&event, ts);
    }

    // 推入 3 次 VetoOverridden（触发 ReasoningTrap override_rate 项）
    for ts in [1500u64, 2500, 3500] {
        let event = NexusEvent::VetoOverridden {
            metadata: EventMetadata::new("parliament"),
            quest_id: "q-bench".into(),
            proposal_id: "p-bench".into(),
            veto_reason: "benchmark".into(),
            override_reason: "false positive".into(),
            override_by: "admin".into(),
        };
        mirror.update_from_event(&event, ts);
    }

    // 推入 1 次 CsnSubstitutionTriggered（degradation_level=3,触发 MemoryParadox 信号）
    let csn_event = NexusEvent::CsnSubstitutionTriggered {
        metadata: EventMetadata::new("csn-substitutor"),
        original_capability_id: "cap-1".into(),
        substitute_id: "cap-sub-1".into(),
        similarity_score: 0.9,
        degradation_level: 3,
    };
    mirror.update_from_event(&csn_event, 1000);

    // 推入 4 次 BudgetExceeded（触发 MemoryParadox + EvolutionHack budget 项）
    for ts in [1000u64, 2000, 3000, 4000] {
        let event = NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("acb-governor"),
            budget_type: "token".into(),
            current: 1000,
            limit: 1000,
        };
        mirror.update_from_event(&event, ts);
    }

    // 推入 5 次 CapabilityFrozen（>3 阈值,触发 EvolutionHack Critical 路径）
    for ts in [1000u64, 2000, 3000, 4000, 5000] {
        let event = NexusEvent::CapabilityFrozen {
            metadata: EventMetadata::new("parliament"),
            capability_id: "cap-1".into(),
            reason: "benchmark".into(),
        };
        mirror.update_from_event(&event, ts);
    }

    // 推入 2 次 AgentTaskFailed（触发 CircuitBreaker Open,影响 circuit_open_ratio）
    for (i, from) in ["agent-1", "agent-2"].iter().enumerate() {
        let event = NexusEvent::AgentTaskFailed {
            metadata: EventMetadata::new("chimera-mas"),
            from: (*from).into(),
            to: "agent-0".into(),
            task_id: format!("t-fail-{i}"),
            error: "boom".into(),
            retry_count: 0,
        };
        mirror.update_from_event(&event, 1000 + i as u64);
    }
}

// ============================================================
// 基准 1: MemoryParadox 单探针延迟
// ============================================================

/// 测量 MemoryParadox 探针 `detect()` 延迟
///
/// # KPI-03 期望
/// 单探针 < 33ms（100ms / 3 探针并行后的预算）
fn bench_memory_paradox_probe_latency(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mirror = Arc::new(StabilityMirror::new());
    prefill_mirror(&mirror);
    let probe = MemoryParadoxProbe::new(mirror);

    c.bench_function("memory_paradox_probe_latency", |b| {
        b.iter(|| {
            rt.block_on(probe.detect());
        });
    });
}

// ============================================================
// 基准 2: ReasoningTrap 单探针延迟
// ============================================================

/// 测量 ReasoningTrap 探针 `detect()` 延迟
fn bench_reasoning_trap_probe_latency(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mirror = Arc::new(StabilityMirror::new());
    prefill_mirror(&mirror);
    let probe = ReasoningTrapProbe::new(mirror);

    c.bench_function("reasoning_trap_probe_latency", |b| {
        b.iter(|| {
            rt.block_on(probe.detect());
        });
    });
}

// ============================================================
// 基准 3: EvolutionHack 单探针延迟
// ============================================================

/// 测量 EvolutionHack 探针 `detect()` 延迟
fn bench_evolution_hack_probe_latency(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mirror = Arc::new(StabilityMirror::new());
    prefill_mirror(&mirror);
    let probe = EvolutionHackProbe::new(mirror);

    c.bench_function("evolution_hack_probe_latency", |b| {
        b.iter(|| {
            rt.block_on(probe.detect());
        });
    });
}

// ============================================================
// 基准 4: full_immune_system_assessment（ADR-046 决策 8 验收基准）
// ============================================================

/// 测量 ImmuneSystem::assess_paradox_risk() 完整扫描延迟
///
/// # ADR-046 决策 8 验收
/// 这是 ADR-046 决策 6 要求的 `immune_system_scan` 验收基准。
/// 三探针异步并行（FuturesUnordered）+ 级联风险评估 + 膜厚调节。
/// 目标 p95 < 100ms（KPI-03,§9.5 SLO）。
///
/// # 设计
/// - 使用 `ImmuneSystem::new()` 真实构造（含 event-bus 订阅 + 后台任务）
/// - 预填充 mirror 以避免 insufficient_data 快路径
fn bench_full_immune_system_assessment(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    // 在 async 上下文中初始化 ImmuneSystem（subscribe + spawn 后台任务）
    // WHY 直接构造 ImmuneSystem 而非手动组装探针：测量真实生产路径,
    //   含 FuturesUnordered 并行调度 + 报告排序 + 级联风险计算 + 膜厚调节全流程
    let immune_system = rt.block_on(async {
        let bus = Arc::new(EventBus::new());
        ImmuneSystem::new(bus)
            .await
            .expect("ImmuneSystem init failed")
    });

    // 预填充 ImmuneSystem 内部的 mirror 以触发完整探针算法路径
    // WHY 通过 stability_mirror() 访问器获取并预填充：ImmuneSystem::new 构造的 mirror 为空,
    //   探针会走 insufficient_data 快路径,无法测量真实算法延迟。
    let mirror = immune_system.stability_mirror();
    prefill_mirror(mirror);

    c.bench_function("full_immune_system_assessment", |b| {
        b.iter(|| {
            rt.block_on(immune_system.assess_paradox_risk());
        });
    });
}

// ============================================================
// criterion group 配置
// ============================================================

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(100)
        .warm_up_time(Duration::from_millis(500));
    targets = bench_memory_paradox_probe_latency,
        bench_reasoning_trap_probe_latency,
        bench_evolution_hack_probe_latency,
        bench_full_immune_system_assessment
}

criterion_main!(benches);
