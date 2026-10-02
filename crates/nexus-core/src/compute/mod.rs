//! compute 模块 — CPU 卸载统一入口（P1-T8,Phase 1 地基波次,手册 W3）
//!
//! 对应架构层:L1 Core
//!
//! # 模块结构
//! - [`dispatch`]:三态派发计划 `DispatchPlan` + 任务类型登记 `TaskKind`（手册 §8.3/§8.4,ADR-127）
//! - [`bridge`]:`ComputeBridge` L-a 全局 rayon 池 + L-f 三态路由（手册 §8.3/§11.1,v4.0 §7.5.2 L-a）
//! - [`hts`]:HTS-CPU 混合阈值调度 — 动态阈值表 + 序贯检验 + cgroup 核数校正（手册 §8.4,ADR-103）
//! - [`seam`]:Clock / Rng / Fs / Net 四个缝合点（手册 §10.8,Ω₇ 可测试性根基）
//! - [`reduce`]:DetReduce 双模式归约 — 固定分块树 + ReproBLAS 式指数分桶（手册 §10.2,ADR-102/106）
//! - [`utilization`]:WI-34 CPU 利用率测量基座 — tokio 稳定子集 + rayon 池活跃双探针（v4.0 §7.5.4）
//! - [`telemetry`]:ADR-197 计算面遥测与池饱和判定 — 按 TaskKind 归因计数、
//!   千分比占用判定与三态处置策略（env）、饱和观测事件周期上报
//! - [`calibration`]:ADR-198 Phase B1 阈值校准闭环 — 把已有的 SPRT 序贯检验
//!   接到 [`update_threshold`](bridge::ComputeBridge::update_threshold);
//!   默认 `off`（不探测不写表），`observe` 只出证据，`apply` 才写表
//!
//! # 依赖关系（T14 预留）
//! - T14(WI-34):各 crate CPU 热点经 [`bridge::spawn_compute`] 卸载,序贯检验采样接线
//!   （已由 [`calibration`] 于 ADR-198 完成采样→判定→写表闭环，默认 off）;
//!   缝合点注入生产实现;利用率采样器驱动中期验收
//!
//! # 红线（§5.2/§7.5.3）
//! 本模块及全 crate `#![forbid(unsafe_code)]`;rayon 全部 safe API;
//! rayon 闭包内禁 `.await` / IO / 持锁跨闭包边界;禁 feature 标志。

pub mod bridge;
pub mod calibration;
pub mod dispatch;
pub mod hts;
pub mod reduce;
pub mod seam;
pub mod telemetry;
pub mod utilization;

pub use bridge::{bridge, ComputeBridge, ComputeError};
pub use dispatch::{DispatchPlan, TaskKind};
// P1-T9: HTS 动态阈值表 + 序贯检验 + cgroup 校正（手册 §8.4 / ADR-103）
pub use hts::cgroup::{effective_cores, parse_cpu_max, CgroupProbe};
pub use hts::sequential_test::{SequentialTest, SequentialTestConfig, TestDecision};
pub use hts::{HtsTable, ThresholdSource};
// P1-T10: DetReduce 双模式归约(手册 §10.2 / ADR-102/106)
pub use reduce::{reduce, repro_reduce, tree_reduce_fixed, ReduceMode, DEFAULT_CHUNK};
// T1(WI-34):CPU 利用率测量基座(手册 §8.5 / v4.0 §7.5.4)
pub use utilization::{RayonProbe, TokioProbe, UtilizationSampler, UtilizationSnapshot};
// A1/ADR-197:饱和判定与按类型归因遥测(装配层直接调用上报器与判定纯函数)
pub use telemetry::{
    parse_saturation_mode, parse_threshold_bp, report_needed, saturation_mode, saturation_ratio_bp,
    spawn_saturation_reporter, threshold_bp, KindCounters, KindStat, SaturationMode,
};
// B1/ADR-198:阈值校准闭环（装配层/上报器可用 flush 写表、snapshot 出证据）
pub use calibration::{
    snapshot as calibration_snapshot, CalibrationMode, CalibrationStat, Decided,
};
