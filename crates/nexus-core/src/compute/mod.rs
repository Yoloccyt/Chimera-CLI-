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
//!
//! # 依赖关系（T14 预留）
//! - T14(WI-34):各 crate CPU 热点经 [`bridge::spawn_compute`] 卸载,序贯检验采样接线
//!   （`hts::sequential_test` 状态机注入）;缝合点注入生产实现;利用率采样器驱动中期验收
//!
//! # 红线（§5.2/§7.5.3）
//! 本模块及全 crate `#![forbid(unsafe_code)]`;rayon 全部 safe API;
//! rayon 闭包内禁 `.await` / IO / 持锁跨闭包边界;禁 feature 标志。

pub mod bridge;
pub mod dispatch;
pub mod hts;
pub mod reduce;
pub mod seam;
pub mod utilization;

pub use bridge::{bridge, ComputeBridge, ComputeError};
pub use dispatch::{DispatchPlan, TaskKind};
// P1-T9: HTS 动态阈值表 + 序贯检验 + cgroup 校正（手册 §8.4 / ADR-103）
pub use hts::cgroup::{effective_cores, parse_cpu_max, CgroupProbe};
pub use hts::sequential_test::{SequentialTest, SequentialTestConfig, TestDecision};
pub use hts::{HtsTable, ThresholdSource};
// P1-T10: DetReduce 双模式归约(手册 §10.2 / ADR-102/106)
pub use reduce::{reduce, repro_reduce, tree_reduce_fixed, ReduceMode, DEFAULT_CHUNK};
// T1(WI-34): CPU 利用率测量基座(手册 §8.5 / v4.0 §7.5.4)
pub use utilization::{
    RayonProbe, TokioProbe, UtilizationSampler, UtilizationSnapshot,
};
