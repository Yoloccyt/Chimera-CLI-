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

use crate::event_payload::TaskPriority;

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
    /// 认领频率超硬上限（v2.3 §(b.4)(2) broker 护栏：防轮询风暴,仅在启用限流时出现）
    RateLimited,
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

/// 3 档优先级 → 4 档任务优先级（**无损单射**）
///
/// WHY 方向唯一：`Priority` 是 `TaskPriority` 的真子集，Low/Medium/High 逐档
/// 同名同义，正向单射无损；反向 `From<TaskPriority> for Priority` **不可实现**
/// —— `Critical` 无对应像。它携带抢占权且仅由任务域表达：总线信用流侧的
/// `Priority`（`event-bus/src/credit_flow.rs:80-91`，仅 High/Normal）已明文将
/// Critical 排除在外（走 mpsc 旁路无条件投递，属红线）。反向若有需求须走
/// `TryFrom` 并另立 ADR（§13-bis #9 完整统一批）。
///
/// 本 impl 是 v2.0 §6-bis B.0.1「以 `TaskPriority`（4 档）为任务域唯一真值源」
/// 的最小地基：**不改动任一枚举变体**，故 serde 线上值零变更。
///
/// match 不写 catch-all：`Priority` 未来若新增变体即编译错提醒同步此处映射
///（沿本仓穷举守卫纪律，避免静默走默认分支造成优先级语义漂移）。
impl From<Priority> for TaskPriority {
    fn from(p: Priority) -> Self {
        match p {
            Priority::Low => TaskPriority::Low,
            Priority::Medium => TaskPriority::Medium,
            Priority::High => TaskPriority::High,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
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

    /// 单射逐档映射 — Low/Medium/High 逐档同名同义，不得跨档接错
    #[test]
    fn priority_injects_into_task_priority_variant_by_variant() {
        assert_eq!(TaskPriority::from(Priority::Low), TaskPriority::Low);
        assert_eq!(TaskPriority::from(Priority::Medium), TaskPriority::Medium);
        assert_eq!(TaskPriority::from(Priority::High), TaskPriority::High);
    }

    /// 单射性 — 像互异（三档输入必得三个不同的 4 档像）
    ///
    /// WHY 单独断：若未来有人把两档映射到同一像（合并档位），本仓调度
    /// 语义会静默降级为更粗的粒度而无人察觉；本测试把该退化钉红。
    #[test]
    fn priority_injection_is_injective() {
        let mapped = [
            TaskPriority::from(Priority::Low),
            TaskPriority::from(Priority::Medium),
            TaskPriority::from(Priority::High),
        ];
        assert_eq!(mapped[0], TaskPriority::Low);
        assert_ne!(mapped[0], mapped[1], "Low/Medium 不得塌缩到同一像");
        assert_ne!(mapped[1], mapped[2], "Medium/High 不得塌缩到同一像");
        assert_ne!(mapped[0], mapped[2], "Low/High 不得塌缩到同一像");
    }

    /// 保序性 — 映射后的相对优先级不得反转
    ///
    /// `TaskPriority` 未派生 `Ord`（避免 L1 承载调度语义，见
    /// chimera-mas/src/scheduler.rs:97 注释），故此处按任务域秩函数
    /// （Critical=3 > High=2 > Medium=1 > Low=0）同式比较，不依赖 `Ord`。
    #[test]
    fn priority_injection_preserves_relative_order() {
        // 任务侧秩：与 chimera-mas::priority_rank 同定义（Critical=3 … Low=0）
        let task_rank = |p: TaskPriority| match p {
            TaskPriority::Low => 0,
            TaskPriority::Medium => 1,
            TaskPriority::High => 2,
            TaskPriority::Critical => 3,
        };
        let lo = task_rank(TaskPriority::from(Priority::Low));
        let med = task_rank(TaskPriority::from(Priority::Medium));
        let hi = task_rank(TaskPriority::from(Priority::High));
        assert!(lo < med, "Low→Medium 相对序不得反转");
        assert!(med < hi, "Medium→High 相对序不得反转");
        // 3 档枚举均不得映射到 Critical（Critical 携带抢占权，仅任务域可给）
        for p in [Priority::Low, Priority::Medium, Priority::High] {
            assert_ne!(
                TaskPriority::from(p),
                TaskPriority::Critical,
                "调度侧不得凭空抬升出 Critical 抢占权"
            );
        }
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
