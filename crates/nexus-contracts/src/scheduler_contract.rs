//! 调度契约 — mas-sched 控制面类型先移 L0（P3-T2 补，ADR-033 先例）
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts，ADR-145 裁决：strangler 渐进第一步）
//! 对应任务: **P3-T2**（手册 W16，WI-29：TodoClaim/Lease/Quota 类型先移 L0，
//! 后续 chimera-mas 拆出依赖时经本契约承接——类型先移、接口后接）
//!
//! # 迁移说明
//! 原定义于 mas-sched/src/types.rs（Phase 3 初版）;按 strangler 渐进路径迁至
//! L0 纯类型契约层（零依赖 + serde），mas-sched 经 re-export 保持公开 API 不变。

use std::time::Instant;

use serde::{Deserialize, Serialize};

/// 任务 ID — 复用 L0 既有契约（ids.rs,五维度稀疏化 ID 之一）
pub use crate::ids::TaskId;

/// 移交目标哨兵 — `handoff(task, HANDOFF)` 表示移交回编排层
pub const HANDOFF: &str = "*handoff*";

/// 任务优先级 — 配额/竞争裁决依据
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    /// 低（后台/可延后）
    Low,
    /// 中（默认）
    Medium,
    /// 高（关键路径）
    High,
}

/// 配额声明 — 调度器据此拒绝超限 claim（控制面硬约束）
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Quota {
    /// 单 peer 最大并发租约数
    pub max_concurrent: usize,
    /// 单任务最大租约时长（ms）
    pub max_duration_ms: u64,
    /// 优先级权重（>1 高优,<1 低优,=1 中性）
    pub priority_weight: f64,
}

impl Default for Quota {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            max_duration_ms: 300_000,
            priority_weight: 1.0,
        }
    }
}

/// 任务 claim 请求 — 长任务租约申请（控制面输入）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TodoClaim {
    /// 任务 ID
    pub task_id: TaskId,
    /// 申请 peer ID
    pub peer_id: String,
    /// 优先级
    pub priority: Priority,
    /// 预估时长（ms,配额校验依据）
    pub est_duration_ms: u64,
    /// 配额声明
    pub quota: Quota,
}

impl TodoClaim {
    /// 新建 claim（默认配额）
    #[must_use]
    pub fn new(
        task_id: impl Into<String>,
        peer_id: impl Into<String>,
        priority: Priority,
        est_duration_ms: u64,
    ) -> Self {
        Self {
            // 先具体化为 String（impl Into<String>）,再 TaskId::from(String)
            task_id: TaskId::from(task_id.into()),
            peer_id: peer_id.into(),
            priority,
            est_duration_ms,
            quota: Quota::default(),
        }
    }
}

/// 租约 — claim 成功后的持有凭证
#[derive(Debug, Clone, PartialEq)]
pub struct Lease {
    /// 任务 ID
    pub task_id: TaskId,
    /// 持有 peer ID
    pub peer_id: String,
    /// 租约到期时刻（Instant,判定用）
    pub lease_until: Instant,
    /// 是否可续期
    pub renewable: bool,
    /// 租约原始时长（ms,诊断）
    pub duration_ms: u64,
}

/// claim 结果
#[derive(Debug, Clone, PartialEq)]
pub enum ClaimOutcome {
    /// 授予租约
    Granted(Lease),
    /// 拒绝（附原因）
    Denied(DenyReason),
}

/// 拒绝原因 — 控制面硬约束枚举（可观测,ADR-145）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DenyReason {
    /// 该 peer 并发租约已达配额上限
    QuotaExceeded,
    /// 任务已被其他 peer claim
    TaskClaimed,
    /// 预估时长超配额上限
    DurationExceedsQuota,
    /// 影子模式拒绝（只决策不执行,影子期不真正授予）
    ShadowReject,
}

/// 续期结果
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenewOutcome {
    /// 续期成功
    Renewed,
    /// 租约不可续（超最大续期次数/不可续类型）
    NotRenewable,
    /// 租约已过期（需重新 claim）
    Expired,
}

/// should_run 裁决 — 与 Loop 记分卡联动（WI-32:收敛分×边际收益×配额余量）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShouldRunVerdict {
    /// 应运行
    Run,
    /// 延后（附原因——当前仅表达延后语义,细节由调用方记录）
    Defer,
    /// 已收敛（记分卡:Gate 满足度达标,无需再跑）
    AlreadyConverged,
    /// 无可行工作（记分卡:边际收益 < 阈值）
    NoActionableWork,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认配额 — 保守合理（4 并发 / 5min / 中性权重）
    #[test]
    fn default_quota_sane() {
        let q = Quota::default();
        assert_eq!(q.max_concurrent, 4);
        assert_eq!(q.max_duration_ms, 300_000);
        assert!((q.priority_weight - 1.0).abs() < 1e-9);
    }

    /// TodoClaim 构造 — 便捷入口默认配额
    #[test]
    fn claim_construction() {
        let c = TodoClaim::new("t1", "peer-a", Priority::High, 1_000);
        assert_eq!(c.task_id.as_str(), "t1");
        assert_eq!(c.peer_id, "peer-a");
        assert_eq!(c.priority, Priority::High);
        assert_eq!(c.est_duration_ms, 1_000);
        assert_eq!(c.quota, Quota::default());
    }

    /// 优先级序 — High > Medium > Low（竞争裁决依赖）
    #[test]
    fn priority_ordering() {
        assert!(Priority::High > Priority::Medium);
        assert!(Priority::Medium > Priority::Low);
    }

    /// 序列化 — 可编解码类型（Quota/DenyReason/RenewOutcome/ShouldRunVerdict）
    #[test]
    fn serde_roundtrip() {
        let q = Quota::default();
        let json = serde_json::to_string(&q).expect("编码成功");
        let back: Quota = serde_json::from_str(&json).expect("解码成功");
        assert_eq!(back, q);
        let d = DenyReason::QuotaExceeded;
        let json2 = serde_json::to_string(&d).expect("编码成功");
        let back2: DenyReason = serde_json::from_str(&json2).expect("解码成功");
        assert_eq!(back2, d);
    }
}
