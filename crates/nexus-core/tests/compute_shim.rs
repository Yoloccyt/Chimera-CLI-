//! ADR-309 (W4-1) compute shim 编译期同一性守卫
//!
//! `nexus-core/src/compute/` 已迁出为独立 crate `nexus-compute`（L0-B）；
//! `nexus_core::compute::*` 旧路径经 `pub mod compute { pub use nexus_compute::*; }`
//! shim 再导出。本测试用 `Option<T>` 赋值断言锁定两侧类型同一
//! （模式同 `types.rs` 的编译期等价测试先例），并断言 `compute::bridge` 等
//! 子模块路径经 glob 再导出可达。shim 删除窗口（ADR-309）到时本文件随删。

#![allow(clippy::unwrap_used)]

/// Option<T> 型别同一性：只有两侧是同一类型，赋值才成立（编译期判据）。
#[test]
fn shim_types_are_identical_to_nexus_compute() {
    let shim: Option<nexus_core::compute::ComputeBridge> = None;
    let direct: Option<nexus_compute::ComputeBridge> = shim;
    let _ = direct;

    let shim: Option<nexus_core::compute::ComputeError> = None;
    let direct: Option<nexus_compute::ComputeError> = shim;
    let _ = direct;

    let shim: Option<nexus_core::compute::DispatchPlan> = None;
    let direct: Option<nexus_compute::DispatchPlan> = shim;
    let _ = direct;

    let shim: Option<nexus_core::compute::TaskKind> = None;
    let direct: Option<nexus_compute::TaskKind> = shim;
    let _ = direct;

    let shim: Option<nexus_core::compute::HtsTable> = None;
    let direct: Option<nexus_compute::HtsTable> = shim;
    let _ = direct;

    let shim: Option<nexus_core::compute::KindCounters> = None;
    let direct: Option<nexus_compute::KindCounters> = shim;
    let _ = direct;
}

/// 子模块路径可达：glob 再导出须把 `nexus_compute` 的 `pub mod` 一并带过来。
#[test]
fn shim_submodule_paths_reachable() {
    // 值面：bridge() 单例入口经 shim 顶层可达（函数名与模块名同存于不同命名空间）
    let _f: fn() -> &'static nexus_compute::ComputeBridge = nexus_core::compute::bridge;

    let shim: Option<nexus_core::compute::bridge::ComputeBridge> = None;
    let direct: Option<nexus_compute::bridge::ComputeBridge> = shim;
    let _ = direct;

    let shim: Option<nexus_core::compute::hts::HtsTable> = None;
    let direct: Option<nexus_compute::hts::HtsTable> = shim;
    let _ = direct;

    let shim: Option<nexus_core::compute::reduce::ReduceMode> = None;
    let direct: Option<nexus_compute::reduce::ReduceMode> = shim;
    let _ = direct;

    let shim: Option<nexus_core::compute::utilization::UtilizationSampler> = None;
    let direct: Option<nexus_compute::utilization::UtilizationSampler> = shim;
    let _ = direct;
}
