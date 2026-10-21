//! 遥测源契约 — 依赖倒置的接缝（由 `nexus-core` 实现，本 crate 消费）
//!
//! WHY 需要这个 trait：`nexus-core` 的 `KindCounters` 引用 `TaskKind`/`hts`，
//! 若本 crate 直接读这些类型就会与 `nexus-core -> nexus-telemetry` 形成 crate 环。
//! 把“有状态实现”留在 `nexus-core`、把“编排”放在本 crate，依赖方向即可单向。

use event_bus::EventBus;

/// 单 `TaskKind` 的校准证据（纯 DTO）
///
/// WHY 用字符串而非 `TaskKind`/`Decided` 枚举：本 crate 不得依赖 `nexus-core`，
/// 否则形成 crate 环。字符串只在低频证据输出路径分配，不在采样热路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalibrationEvidence {
    /// 任务类型名（`TaskKind` 的 `Debug` 形）
    pub kind: String,
    /// Inline 桶样本数
    pub inline_samples: u64,
    /// 池桶样本数
    pub pool_samples: u64,
    /// 判定状态名（`Pending` / `Promote` / `Reject`）
    pub decided: String,
    /// Promote 候选阈值
    pub candidate_threshold: usize,
    /// 是否已写表
    pub applied: bool,
    /// 校准模式名（`Off` / `Observe` / `Apply`）
    pub mode: String,
}

/// 有状态遥测源 —— 由 L0-B 的 `nexus-core` 实现
///
/// 实现方负责采样、去抖与事件发布；本 crate 只负责周期驱动与证据输出。
/// 对象安全：上报器以 `Arc<dyn SaturationSource>` 持有，避免泛型扩散到装配层。
pub trait SaturationSource: Send + Sync {
    /// 采样并发布 `ComputePoolSaturated`（去抖在实现方内部完成）
    fn sample_and_report(&self, bus: &EventBus);

    /// 把 SPRT 校准结果写入 HTS 阈值表，返回实际写入行数
    fn apply_calibration(&self) -> usize;

    /// 导出逐 kind 的校准证据（低频调用，允许分配）
    fn calibration_evidence(&self) -> Vec<CalibrationEvidence>;
}
