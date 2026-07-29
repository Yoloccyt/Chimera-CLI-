#![forbid(unsafe_code)]

//! ImmuneSystem facade 命名对齐测试（ADR-046 P1-5）
//!
//! 对应架构层:L9 chimera-mas(重新导出 L8 parliament 的 ImmuneSystem facade)
//! 对应 ADR:ADR-046 决策 1/5/9 + 深度分析报告 P1-Dev-4 / P1-5
//!
//! # 背景
//!
//! ADR-046 已批准 ImmuneSystem facade 落地于 parliament crate(L8),通过 event-bus
//! 订阅 chimera-mas StabilityGuard 事件维护镜像状态(方案 A 事件订阅镜像)。
//! 但 `crates/chimera-mas/src/lib.rs` 仅导出 `StabilityGuard`,未导出 `ImmuneSystem`,
//! 导致 `chimera_mas::ImmuneSystem` 不可访问 — 命名未对齐 ADR-046 设计。
//!
//! 本测试验证命名对齐后:
//! 1. `chimera_mas::ImmuneSystem` 及关联类型可访问(编译期 + 运行期)
//! 2. ImmuneSystem 功能正常(三探针 + 级联风险 + 膜厚控制)
//! 3. INV-10 不可变量(探针数量恒为 3)
//! 4. 既有 StabilityGuard 仍可访问(append-only 不破坏既有 API)
//!
//! ## 红线对齐
//!
//! - §2.2 依赖铁律:L9→L8 向下依赖允许(chimera-mas → parliament)
//! - §4.4 反模式 3:ImmuneSystem::new() 内部 subscribe 先于 spawn(由 parliament 保证)
//! - `#![forbid(unsafe_code)]`:纯测试,无 unsafe 需求

use std::sync::Arc;

// === P1-5 命名对齐核心断言:类型可访问性(编译期验证)===
//
// WHY 这些 use 语句本身即测试:若 chimera-mas 未导出这些类型,编译失败(RED 阶段)。
// 导出清单遵循"适度导出"原则:facade 主类型 + 公开方法签名涉及的关联类型,
// 不导出内部实现细节(StabilityMirror / compute_cascade_risk / 三探针具体类型)。
use chimera_mas::{
    ImmuneSystem, ImmuneSystemError, ParadoxProbe, ParadoxReport, ParadoxRiskReport,
};
// 既有 StabilityGuard 仍可访问(append-only 兼容性验证)
use chimera_mas::StabilityGuard;
use event_bus::EventBus;

// ============================================================
// 测试 1:ImmuneSystem 类型可从 chimera_mas 命名空间访问(P1-5 核心)
// ============================================================

/// 验证 `chimera_mas::ImmuneSystem` 可构造且探针数量固定为 3(INV-10)
///
/// # INV-10 不可变量(ADR-046 决策 9)
/// ImmuneSystem 探针数量恒为 3(MemoryParadox / ReasoningTrap / EvolutionHack),
/// 禁止运行时动态注册新探针。
#[tokio::test]
async fn test_immune_system_accessible_from_chimera_mas() {
    let bus = Arc::new(EventBus::new());
    let immune = ImmuneSystem::new(bus)
        .await
        .expect("ImmuneSystem::new should succeed with fresh EventBus");

    // INV-10:探针数量固定为 3
    assert_eq!(
        immune.probes().len(),
        3,
        "INV-10: probes count must be 3 (MemoryParadox/ReasoningTrap/EvolutionHack)"
    );
}

// ============================================================
// 测试 2:assess_paradox_risk 返回 ParadoxRiskReport(关联类型可访问性)
// ============================================================

/// 验证 ImmuneSystem::assess_paradox_risk 返回 ParadoxRiskReport,
/// 且报告内容符合 INV-11(膜厚度 ∈ [0,7])与 INV-12(级联风险 ∈ [0.0,1.0])。
///
/// # 不变量(ADR-046 决策 9)
/// - INV-11:membrane_thickness ∈ [0, 7]
/// - INV-12:cascade_risk ∈ [0.0, 1.0]
#[tokio::test]
async fn test_assess_paradox_risk_returns_report() {
    let bus = Arc::new(EventBus::new());
    let immune = ImmuneSystem::new(bus)
        .await
        .expect("ImmuneSystem::new should succeed");

    // 调用 assess_paradox_risk — 返回类型必须为 ParadoxRiskReport(命名对齐验证)
    let report: ParadoxRiskReport = immune.assess_paradox_risk().await;

    // 验证报告包含 3 个探针报告(与 INV-10 一致)
    assert_eq!(
        report.reports.len(),
        3,
        "ParadoxRiskReport must contain 3 probe reports"
    );

    // INV-12:级联风险 ∈ [0.0, 1.0]
    assert!(
        report.cascade_risk >= 0.0 && report.cascade_risk <= 1.0,
        "INV-12: cascade_risk must be in [0.0, 1.0], got {}",
        report.cascade_risk
    );

    // INV-11:膜厚度 ∈ [0, 7]
    assert!(
        report.membrane_thickness <= 7,
        "INV-11: membrane_thickness must be in [0, 7], got {}",
        report.membrane_thickness
    );

    // 验证 ParadoxReport 关联类型可访问(显式类型标注 + 字段访问)
    let reports: &[ParadoxReport] = &report.reports;
    for probe_report in reports {
        let _probe_type = probe_report.probe_type;
        let _paradox_rate = probe_report.paradox_rate;
    }
}

// ============================================================
// 测试 3:膜厚度与级联风险访问器(决策 1 接口完整性)
// ============================================================

/// 验证 ImmuneSystem 的 membrane_thickness() 与 cascade_risk() 访问器可调用,
/// 且初始值符合 ADR-046 决策 1 设计(初始膜厚 0,初始级联风险 0.0)。
#[tokio::test]
async fn test_immune_system_accessors() {
    let bus = Arc::new(EventBus::new());
    let immune = ImmuneSystem::new(bus)
        .await
        .expect("ImmuneSystem::new should succeed");

    // 初始膜厚度应为 0(ADR-046 决策 1:AtomicU8::new(0))
    assert_eq!(
        immune.membrane_thickness(),
        0,
        "initial membrane_thickness should be 0"
    );

    // 初始级联风险应为 0.0(ADR-046 决策 1:AtomicU32::new(0.0f32.to_bits()))
    let initial_risk = immune.cascade_risk();
    assert!(
        (initial_risk - 0.0).abs() < 1e-6,
        "initial cascade_risk should be 0.0, got {}",
        initial_risk
    );
}

// ============================================================
// 测试 4:trip_circuit 接口(决策 1 熔断触发)
// ============================================================

/// 验证 ImmuneSystem::trip_circuit 可调用,且触发后膜厚度不变(非级联触发)。
///
/// # 设计(ADR-046 决策 1)
/// trip_circuit 仅在镜像中标记 breaker 为 Open,不直接调用 chimera-mas StabilityGuard
/// (依赖铁律:parliament 不向上依赖 chimera-mas)。
#[tokio::test]
async fn test_trip_circuit_interface() {
    let bus = Arc::new(EventBus::new());
    let immune = ImmuneSystem::new(bus)
        .await
        .expect("ImmuneSystem::new should succeed");

    let thickness_before = immune.membrane_thickness();
    immune.trip_circuit("breaker-test-1");
    let thickness_after = immune.membrane_thickness();

    // trip_circuit 不触发膜厚变化(仅级联风险 >0.7 才增厚)
    assert_eq!(
        thickness_before, thickness_after,
        "trip_circuit should not change membrane_thickness (only cascade_risk > 0.7 does)"
    );
}

// ============================================================
// 测试 5:既有 StabilityGuard 仍可访问(append-only 兼容性)
// ============================================================

/// 验证命名对齐修改是 append-only:既有 StabilityGuard 及其方法不受影响。
///
/// # SemVer 兼容性
/// 本次修改仅新增 `pub use parliament::ImmuneSystem`,不修改既有 stability 模块的
/// 任何类型或方法。此测试守护既有 API 不被破坏。
#[test]
fn test_stability_guard_still_accessible() {
    let guard = StabilityGuard::new();

    // 既有方法仍可用
    assert_eq!(
        guard.terminal_count(),
        0,
        "StabilityGuard::new should have 0 terminals"
    );
    assert_eq!(
        guard.isolated_count(),
        0,
        "StabilityGuard::new should have 0 isolated subtrees"
    );
    assert!(
        !guard.is_isolated("any-subtree"),
        "no subtree should be isolated initially"
    );
}

// ============================================================
// 测试 6:ImmuneSystemError 类型可访问(错误类型完整性)
// ============================================================

/// 验证 `chimera_mas::ImmuneSystemError` 类型可访问。
///
/// WHY 此测试:ImmuneSystem::new() 与 trigger_cascade() 返回 Result<_, ImmuneSystemError>,
/// 消费者需能访问错误类型以编写完整的错误处理代码。
#[test]
fn test_immune_system_error_type_accessible() {
    // 仅验证类型可被命名空间引用(编译期验证)
    // 运行期无简单方式构造 ImmuneSystemError(其变体由内部逻辑产生),故用 Option 占位
    let _err_placeholder: Option<ImmuneSystemError> = None;
}

// ============================================================
// 测试 7:ParadoxProbe trait 可访问(trait 完整性)
// ============================================================

/// 验证 `chimera_mas::ParadoxProbe` trait 可访问。
///
/// WHY 此测试:ImmuneSystem::probes() 返回 `&[Box<dyn ParadoxProbe>]`,
/// 消费者需能访问 ParadoxProbe trait 才能对探针切片进行类型操作。
#[tokio::test]
async fn test_paradox_probe_trait_accessible() {
    let bus = Arc::new(EventBus::new());
    let immune = ImmuneSystem::new(bus)
        .await
        .expect("ImmuneSystem::new should succeed");

    // probes() 返回 &[Box<dyn ParadoxProbe>],验证 trait 可访问
    let probes: &[Box<dyn ParadoxProbe>] = immune.probes();

    // 验证可遍历探针并调用 trait 方法(probe_type)
    for probe in probes {
        let _probe_type = probe.probe_type();
    }
}
