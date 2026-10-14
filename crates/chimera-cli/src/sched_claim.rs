//! sched claim 准入 — P1 候选③（ADR-211 Accepted 2026-10-12，见其 §7）
//!
//! `agent spawn` 派单前向 mas-sched 控制面申请任务租约（claim/lease/quota）：
//! 把 mas-sched 从 DECORATIVE（零生产调用方）翻为有真实消费者的生产边，
//! 并为 `ClaimIssued`/`ClaimRetracted` 建立首批生产发出点（v2.0 §7-bis P1 候选③）。
//!
//! # 开关（env 三态，沿 `nexus-core::compute::calibration` 形制）
//!
//! | `CHIMERA_SCHED_CLAIM` | 效果 |
//! |---|---|
//! | `off`（**默认**） | 不调 claim、不发事件 —— 与接线前逐位等价 |
//! | `observe` | 调 claim；被拒仅 warn 不阻断（只出证据） |
//! | `apply` | 被拒阻断派单（`EngineError`）；授予则发 `ClaimIssued` |
//!
//! 未知值 fail-safe 按 `off`（绝不默认放开准入）。env 读取经 [`mode()`] 的
//! `OnceLock` 只发生一次；判定逻辑全部收在纯函数（[`parse_sched_claim_env`]）里便于表驱动单测。
//!
//! # 事件面（ADR-211 §7.3）
//!
//! 发布方 = 组合根本层（持 `ctx.bus`；**mas-sched 保持零 event-bus 依赖**）：
//! claim Granted → [`EventBus::publish_claim`]（`ClaimIssued`，Normal/Parliament Topic）；
//! delegate 失败 → [`publish_retract`]（`ClaimRetracted`）。Certify 不涉及（G-83 白名单锁）。
//! 两变体的生产构造点即 `event_ownership_baseline` 两行的消除依据（台账只减）。
//!
//! # 语义裁量（同 ADR-211 §7.3，供后续批复核）
//!
//! spawn 成功路径不显式 retract（ephemeral 进程退出即释放租约表）；retract 仅出现在
//! delegate 失败路径。`PeerScheduler::claim` 是 total function（`-> ClaimOutcome`），
//! **无「调用异常」面** —— 唯一失败面是 `Denied`；off 档零调用即逐位等价。

use std::sync::{Arc, OnceLock};

use event_bus::{EventBus, EventMetadata, NexusEvent};
use mas_sched::{
    ClaimOutcome, DenyReason, PeerScheduler, Priority, SimplePeerScheduler, TodoClaim,
};

/// 事件源标识（沿 `action_orchestrator::SOURCE` 惯例，chimera-cli 根事件）
const SOURCE: &str = "chimera-cli";

/// claim 准入模式（与 `CHIMERA_SCHED_CLAIM` 字符串一一对应）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchedClaimMode {
    /// 关闭（默认）：不调用、不发事件 —— 零行为变更
    Off,
    /// 观察：调用 claim 并记录判定，被拒不阻断（只出证据）
    Observe,
    /// 应用：被拒阻断派单（严格准入）
    Apply,
}

/// 解析 env 字符串 → 模式（纯函数，供表驱动单测；未知值 fail-safe 按 `Off`）
#[must_use]
pub(crate) fn parse_sched_claim_env(raw: Option<&str>) -> SchedClaimMode {
    match raw {
        Some("observe") => SchedClaimMode::Observe,
        Some("apply") => SchedClaimMode::Apply,
        // "off"、未设置、未知值一律 Off：fail-safe 方向是「不放开准入」，
        // 而非「拼错就默认启用」。
        _ => SchedClaimMode::Off,
    }
}

/// 进程级缓存读取生产模式（`OnceLock`：env 读取只发生一次，沿 calibration.rs 形制）
pub(crate) fn mode() -> SchedClaimMode {
    static CACHED: OnceLock<SchedClaimMode> = OnceLock::new();
    *CACHED
        .get_or_init(|| parse_sched_claim_env(std::env::var("CHIMERA_SCHED_CLAIM").ok().as_deref()))
}

/// 准入结果（`Skipped` 显式表征「逐位等价旧路径」，I1 判据的载体）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaimGate {
    /// off 档：未触发任何 claim/事件（逐位等价旧路径）
    Skipped,
    /// 授予：已发布 `ClaimIssued`（observe/apply 通用）
    Granted,
    /// observe 档被拒：warn 记录后继续（不阻断）
    DeniedObserved(DenyReason),
}

/// claim 准入前置 —— 在 `orchestrator.delegate(agent_task)` **之前**调用。
///
/// # 参数
/// - `mode`：准入模式（**注入参数**便于单测三态；生产入口传 [`mode()`]）
/// - `sched` / `bus`：组合根装配句柄（[`crate::composition::AppContext`] 字段）
/// - `task_id` / `subject` / `peer_id`：claim 语义三元组（`claim_id` 复用 `task_id`，
///   下游消费者为 Parliament Topic 订阅者，见 ADR-211 §7.3 裁量）
/// - `est_duration_ms`：预估时长（配额校验依据；spawn 场景 = acceptable_latency）
///
/// # 返回
/// - `Ok(ClaimGate::Skipped)`：off 档（未调用 claim，零行为变更）
/// - `Ok(ClaimGate::Granted)`：授予并已发布 `ClaimIssued`
/// - `Ok(ClaimGate::DeniedObserved(reason))`：observe 档被拒（已 warn）
/// - `Err(DenyReason)`：apply 档被拒 —— 调用方转
///   `ChimeraCliError::EngineError`（不新增错误变体，保 ADR-060 7 变体矩阵）
pub(crate) async fn preflight(
    mode: SchedClaimMode,
    sched: &Arc<SimplePeerScheduler>,
    bus: &EventBus,
    task_id: &str,
    subject: &str,
    peer_id: &str,
    est_duration_ms: u64,
) -> Result<ClaimGate, DenyReason> {
    if mode == SchedClaimMode::Off {
        return Ok(ClaimGate::Skipped);
    }
    // Medium 优先级 + 默认配额（max_concurrent=4 / max_duration_ms=300_000）：
    // spawn 的 est(60_000) 恒小于时长上限，时长门在此场景透明。
    let claim = TodoClaim::new(task_id, peer_id, Priority::Medium, est_duration_ms);
    match sched.claim(&claim) {
        ClaimOutcome::Granted(_lease) => {
            publish_claim_issued(bus, task_id, subject, peer_id).await;
            Ok(ClaimGate::Granted)
        }
        ClaimOutcome::Denied(reason) => match mode {
            SchedClaimMode::Apply => Err(reason),
            // Observe：DenyReason 是实测准入证据，warn 记录后不阻断主流程。
            _ => {
                tracing::warn!(
                    task_id,
                    peer_id,
                    reason = ?reason,
                    "sched claim 被拒（observe 档：不阻断派单）"
                );
                Ok(ClaimGate::DeniedObserved(reason))
            }
        },
    }
}

/// 发布 `ClaimIssued`（Granted 后）。
///
/// WHY 独立函数：`metadata:` 冒号形态是 `check_event_ownership.py` 生产构造点的
/// 静态判据（字段简写不算 ctor），收拢到单点避免散落失守。
/// 发布失败不阻断（事件面是证据通道；准入结果已由 [`preflight`] 返回值表达）。
async fn publish_claim_issued(bus: &EventBus, claim_id: &str, subject: &str, claimant: &str) {
    let event = NexusEvent::ClaimIssued {
        metadata: EventMetadata::new(SOURCE),
        claim_id: claim_id.to_string(),
        subject: subject.to_string(),
        claimant: claimant.to_string(),
    };
    if let Err(e) = bus.publish_claim(event).await {
        tracing::warn!(error = %e, claim_id, "ClaimIssued 发布失败（不阻断派单）");
    }
}

/// 发布 `ClaimRetracted`（delegate 失败路径专用入口）。
///
/// 语义：已 Granted 的 claim 因 delegate 失败而撤回（审计完整性）；成功路径不发
/// （ephemeral 进程退出即释放，见模块头「语义裁量」）。
pub(crate) async fn publish_retract(bus: &EventBus, claim_ref: &str, reason: &str) {
    let event = NexusEvent::ClaimRetracted {
        metadata: EventMetadata::new(SOURCE),
        claim_ref: claim_ref.to_string(),
        reason: reason.to_string(),
    };
    if let Err(e) = bus.publish_declaim(event).await {
        tracing::warn!(error = %e, claim_ref, "ClaimRetracted 发布失败（不阻断）");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// 表驱动：三态字符串 + 未知值 + 未设置（fail-safe 全部收敛到 Off）。
    #[test]
    fn parse_sched_claim_env_table_driven() {
        let cases: &[(Option<&str>, SchedClaimMode)] = &[
            (None, SchedClaimMode::Off),
            (Some("off"), SchedClaimMode::Off),
            (Some("observe"), SchedClaimMode::Observe),
            (Some("apply"), SchedClaimMode::Apply),
            // 未知值/大小写偏差/空串：fail-safe 按 Off（不得默认放开准入）
            (Some(""), SchedClaimMode::Off),
            (Some("ON"), SchedClaimMode::Off),
            (Some("enabled"), SchedClaimMode::Off),
            (Some("Apply"), SchedClaimMode::Off),
        ];
        for (raw, want) in cases {
            assert_eq!(
                parse_sched_claim_env(*raw),
                *want,
                "raw={raw:?} 应解析为 {want:?}"
            );
        }
    }

    /// I1 逐位等价：off 档不调 claim、不发事件（订阅者零接收）。
    #[tokio::test]
    async fn off_mode_is_bitwise_equivalent_no_event() {
        let sched = Arc::new(SimplePeerScheduler::new());
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let gate = preflight(
            SchedClaimMode::Off,
            &sched,
            &bus,
            "task-off",
            "subject",
            "Q1",
            1_000,
        )
        .await
        .expect("off 档恒 Ok");
        assert_eq!(gate, ClaimGate::Skipped, "off 档应显式 Skipped");
        assert_eq!(sched.lease_count(), 0, "off 档不得产生租约");
        assert!(matches!(rx.try_recv(), Ok(None)), "off 档不得发任何事件");
    }

    /// Granted：发 ClaimIssued（订阅者收到，claimant/claim_id 语义正确）。
    #[tokio::test]
    async fn apply_mode_granted_publishes_claim_issued() {
        let sched = Arc::new(SimplePeerScheduler::new());
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let gate = preflight(
            SchedClaimMode::Apply,
            &sched,
            &bus,
            "task-a",
            "run tests",
            "Q2",
            1_000,
        )
        .await
        .expect("首次 claim 应授予");
        assert_eq!(gate, ClaimGate::Granted);
        assert_eq!(sched.lease_count(), 1);
        match rx
            .try_recv()
            .expect("bus 正常")
            .expect("应收到 ClaimIssued")
        {
            NexusEvent::ClaimIssued {
                claim_id,
                subject,
                claimant,
                ..
            } => {
                assert_eq!(claim_id, "task-a");
                assert_eq!(subject, "run tests");
                assert_eq!(claimant, "Q2");
            }
            other => panic!("应为 ClaimIssued，得到 {other:?}"),
        }
    }

    /// apply 档被拒 → Err(reason)（阻断面），且不发事件。
    #[tokio::test]
    async fn apply_mode_denied_returns_err_without_event() {
        let sched = Arc::new(SimplePeerScheduler::new());
        let bus = EventBus::new();
        // 先占住 task_id，制造 Denied(TaskClaimed)
        sched.claim(&TodoClaim::new("task-b", "Q1", Priority::Medium, 1_000));
        let mut rx = bus.subscribe();
        let denied = preflight(
            SchedClaimMode::Apply,
            &sched,
            &bus,
            "task-b",
            "subject",
            "Q2",
            1_000,
        )
        .await;
        assert_eq!(denied, Err(DenyReason::TaskClaimed));
        assert!(matches!(rx.try_recv(), Ok(None)), "被拒不得发 ClaimIssued");
    }

    /// observe 档被拒 → DeniedObserved（不阻断），且不发事件。
    #[tokio::test]
    async fn observe_mode_denied_does_not_block() {
        let sched = Arc::new(SimplePeerScheduler::new());
        let bus = EventBus::new();
        sched.claim(&TodoClaim::new("task-c", "Q1", Priority::Medium, 1_000));
        let mut rx = bus.subscribe();
        let gate = preflight(
            SchedClaimMode::Observe,
            &sched,
            &bus,
            "task-c",
            "subject",
            "Q2",
            1_000,
        )
        .await
        .expect("observe 档被拒不得返回 Err");
        assert_eq!(gate, ClaimGate::DeniedObserved(DenyReason::TaskClaimed));
        assert!(matches!(rx.try_recv(), Ok(None)), "被拒不得发 ClaimIssued");
    }

    /// publish_retract：ClaimRetracted 到达（delegate 失败路径的载体）。
    #[tokio::test]
    async fn retract_publishes_claim_retracted() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        publish_retract(&bus, "task-d", "delegate failed").await;
        match rx
            .try_recv()
            .expect("bus 正常")
            .expect("应收到 ClaimRetracted")
        {
            NexusEvent::ClaimRetracted {
                claim_ref, reason, ..
            } => {
                assert_eq!(claim_ref, "task-d");
                assert_eq!(reason, "delegate failed");
            }
            other => panic!("应为 ClaimRetracted，得到 {other:?}"),
        }
    }
}
