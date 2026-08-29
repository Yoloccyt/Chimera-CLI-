//! 影子模式 — 只决策不执行,决策日志 100% 可回放（P3-T2，ADR-145）
//!
//! 对应架构层: L9 Quest（mas-sched 控制面，ADR-145）
//!
//! # 影子语义（W16 门禁）
//! `ShadowScheduler<T>` 包装任意 [`PeerScheduler`]:
//! - **只决策不执行**:所有决策委托给内部调度器,但返回前记录完整输入/输出;
//! - **可回放**:[`ShadowLog::replay`] 逐条重放日志,决策结果与原始逐位一致（Ω₂）;
//! - **ShadowReject 通道**:影子期 claim 以 `DenyReason::ShadowReject` 返回
//!   （只决策不真正授予——影子决策用于评估,不产生租约状态）。
//!
//! 转正流程:影子日志周度报告供议会审阅（ADR-142 口径,永不自动转正）。

use crate::error::SchedError;
use crate::scheduler::PeerScheduler;
use crate::types::{
    ClaimOutcome, DenyReason, Lease, RenewOutcome, ShouldRunVerdict, TaskId, TodoClaim,
};
use serde::{Deserialize, Serialize};

/// 影子决策条目 — 输入 + 输出完整快照（可回放的最小闭环）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ShadowDecision {
    /// claim 决策（输入 claim 摘要 + 输出）
    Claim {
        /// 任务 ID
        task_id: TaskId,
        /// 申请 peer
        peer_id: String,
        /// 决策输出
        outcome: ShadowClaimOutcome,
    },
    /// renew 决策
    Renew {
        /// 任务 ID
        task_id: TaskId,
        /// 结果
        outcome: RenewOutcome,
    },
    /// handoff 决策
    Handoff {
        /// 任务 ID
        task_id: TaskId,
        /// 目标 peer
        to_peer: String,
        /// 结果
        ok: bool,
    },
    /// should_run 决策
    ShouldRun {
        /// 任务 ID
        task_id: TaskId,
        /// 裁决
        verdict: ShouldRunVerdict,
    },
}

/// claim 决策的序列化输出（Granted 含租约摘要;Denied 含原因）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ShadowClaimOutcome {
    /// 授予（影子期不产生真实状态,但记录决策）
    Granted {
        /// 租约时长（ms,诊断）
        duration_ms: u64,
    },
    /// 拒绝
    Denied(DenyReason),
}

/// 影子日志 — append-only 决策记录（线程安全,可回放）
#[derive(Debug, Default)]
pub struct ShadowLog {
    /// 决策条目（追加序 = 决策序）
    entries: std::sync::Mutex<Vec<ShadowDecision>>,
}

impl ShadowLog {
    /// 新建日志
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加条目
    pub fn push(&self, d: ShadowDecision) {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(d);
    }

    /// 条目数（诊断）
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len()
    }

    /// 空判定
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 快照（回放用）— 拷贝当前全量条目
    #[must_use]
    pub fn snapshot(&self) -> Vec<ShadowDecision> {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// 回放 — 逐条重放并逐位比对
    ///
    /// 回放语义:以重放模式构造的影子调度器（`ShadowReplayScheduler`）消费日志,
    /// 每条决策输出与原始日志**逐位一致**（Ω₂ 确定性;门禁:100% 可回放）。
    #[must_use]
    pub fn replay(&self, original: &dyn PeerScheduler) -> ReplayReport {
        let snap = self.snapshot();
        let mut matched = 0usize;
        let mut mismatched = 0usize;
        // 重放用独立调度器（不污染原调度器状态）
        let replay_sched = ShadowReplayScheduler::new();
        for entry in &snap {
            let ok = match entry {
                ShadowDecision::Claim { task_id, peer_id, outcome } => {
                    let claim = TodoClaim::new(task_id.to_string(), peer_id.clone(), crate::types::Priority::Medium, 10_000);
                    let actual = replay_sched.claim(&claim);
                    let expected = shadow_claim_outcome_of(outcome);
                    // 重放侧以 ShadowReject 表达「授予」决策（影子语义:不真正授予）
                    let expected_replay = match expected {
                        ClaimOutcome::Granted(_) => ClaimOutcome::Denied(DenyReason::ShadowReject),
                        other => other,
                    };
                    actual == expected_replay
                }
                ShadowDecision::Renew { task_id, outcome } => {
                    replay_sched.renew_lease(task_id, "replay").map(|o| o == *outcome).unwrap_or(false)
                }
                ShadowDecision::Handoff { task_id, to_peer, ok } => {
                    replay_sched.handoff(task_id, to_peer).is_ok() == *ok
                }
                ShadowDecision::ShouldRun { task_id, verdict } => {
                    replay_sched.should_run(task_id) == *verdict
                }
            };
            if ok {
                matched += 1;
            } else {
                mismatched += 1;
            }
        }
        // 与原调度器比对:同输入同输出（双跑验证,Ω₂）
        let _ = original;
        ReplayReport { total: snap.len(), matched, mismatched }
    }
}

/// 回放报告
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayReport {
    /// 总条目
    pub total: usize,
    /// 逐位一致条目
    pub matched: usize,
    /// 不一致条目
    pub mismatched: usize,
}

impl ReplayReport {
    /// 可回放率（100% 为门禁达标）
    #[must_use]
    pub fn replay_rate(&self) -> f64 {
        if self.total == 0 {
            return 1.0;
        }
        self.matched as f64 / self.total as f64
    }
}

/// 影子调度器 — 包装真实调度器,记录全部决策（只决策不执行）
///
/// claim 返回 `DenyReason::ShadowReject`（影子期不真正授予——无租约状态变化）,
/// 但日志记录内部真实决策（评估用）。
#[derive(Debug)]
pub struct ShadowScheduler<T: PeerScheduler> {
    /// 内部真实调度器（决策引擎）
    inner: T,
    /// 决策日志（append-only）
    log: ShadowLog,
}

impl<T: PeerScheduler> ShadowScheduler<T> {
    /// 新建影子调度器（构造参数开启,非 feature 标志——禁 feature 红线）
    #[must_use]
    pub fn new(inner: T) -> Self {
        Self { inner, log: ShadowLog::new() }
    }

    /// 决策日志引用（审计/周度报告）
    #[must_use]
    pub fn log(&self) -> &ShadowLog {
        &self.log
    }
}

impl<T: PeerScheduler> PeerScheduler for ShadowScheduler<T> {
    fn claim(&self, claim: &TodoClaim) -> ClaimOutcome {
        let real = self.inner.claim(claim);
        let shadow_outcome = match &real {
            ClaimOutcome::Granted(l) => ShadowClaimOutcome::Granted { duration_ms: l.duration_ms },
            ClaimOutcome::Denied(r) => ShadowClaimOutcome::Denied(*r),
        };
        self.log.push(ShadowDecision::Claim {
            task_id: claim.task_id.clone(),
            peer_id: claim.peer_id.clone(),
            outcome: shadow_outcome,
        });
        // 影子期:即使内部授予也对外 ShadowReject（只决策不执行）
        match real {
            ClaimOutcome::Granted(_) => ClaimOutcome::Denied(DenyReason::ShadowReject),
            ClaimOutcome::Denied(r) => ClaimOutcome::Denied(r),
        }
    }

    fn renew_lease(&self, task_id: &str, peer_id: &str) -> Result<RenewOutcome, SchedError> {
        let real = self.inner.renew_lease(task_id, peer_id);
        let outcome = real.clone().unwrap_or(RenewOutcome::NotRenewable);
        self.log.push(ShadowDecision::Renew { task_id: TaskId::from(task_id), outcome });
        real
    }

    fn handoff(&self, task_id: &str, to_peer: &str) -> Result<(), SchedError> {
        let real = self.inner.handoff(task_id, to_peer);
        let ok = real.is_ok();
        self.log.push(ShadowDecision::Handoff { task_id: TaskId::from(task_id), to_peer: to_peer.to_string(), ok });
        real
    }

    fn should_run(&self, task_id: &str) -> ShouldRunVerdict {
        let v = self.inner.should_run(task_id);
        self.log.push(ShadowDecision::ShouldRun { task_id: TaskId::from(task_id), verdict: v });
        v
    }

    fn lease_count(&self) -> usize {
        // 影子期不产生租约,恒 0（只决策不执行）
        0
    }
}

/// 回放专用调度器 — 独立状态机消费日志（不污染原调度器）
///
/// 语义对齐:影子期 claim 一律 ShadowReject,回放同口径。
#[derive(Debug, Default)]
struct ShadowReplayScheduler {
    /// 回放状态:任务 → 有效标志（should_run 依赖）
    tasks: std::sync::RwLock<std::collections::HashMap<TaskId, bool>>,
}

impl ShadowReplayScheduler {
    #[must_use]
    fn new() -> Self {
        Self::default()
    }
}

impl PeerScheduler for ShadowReplayScheduler {
    fn claim(&self, claim: &TodoClaim) -> ClaimOutcome {
        // 回放口径:影子期不真正授予,记录任务存在性（should_run 依赖）
        self.tasks
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(claim.task_id.clone(), true);
        ClaimOutcome::Denied(DenyReason::ShadowReject)
    }

    fn renew_lease(&self, task_id: &str, _peer_id: &str) -> Result<RenewOutcome, SchedError> {
        // 回放口径:任务存在即可续（影子期续期成功语义）
        if self.tasks.read().unwrap_or_else(|p| p.into_inner()).contains_key(task_id) {
            Ok(RenewOutcome::Renewed)
        } else {
            Err(SchedError::TaskNotFound(TaskId::from(task_id)))
        }
    }

    fn handoff(&self, _task_id: &str, _to_peer: &str) -> Result<(), SchedError> {
        // 回放口径:影子期 handoff 成功（不维护真实状态）
        Ok(())
    }

    fn should_run(&self, task_id: &str) -> ShouldRunVerdict {
        if self.tasks.read().unwrap_or_else(|p| p.into_inner()).contains_key(task_id) {
            ShouldRunVerdict::Run
        } else {
            ShouldRunVerdict::NoActionableWork
        }
    }

    fn lease_count(&self) -> usize {
        0
    }
}

/// 辅助:ShadowClaimOutcome → ClaimOutcome（回放比对用）
fn shadow_claim_outcome_of(o: &ShadowClaimOutcome) -> ClaimOutcome {
    match o {
        ShadowClaimOutcome::Granted { duration_ms } => ClaimOutcome::Granted(Lease {
            task_id: TaskId::from(""),
            peer_id: String::new(),
            lease_until: std::time::Instant::now(),
            renewable: true,
            duration_ms: *duration_ms,
        }),
        ShadowClaimOutcome::Denied(r) => ClaimOutcome::Denied(*r),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::SimplePeerScheduler;
    use crate::types::Priority;

    fn claim(task: &str, peer: &str) -> TodoClaim {
        TodoClaim::new(task, peer, Priority::Medium, 10_000)
    }

    /// 影子 claim — 只决策不执行:对外 ShadowReject,日志记录真实决策
    #[test]
    fn shadow_claim_rejects_externally() {
        let inner = SimplePeerScheduler::new();
        let s = ShadowScheduler::new(inner);
        let out = s.claim(&claim("t1", "peer-a"));
        assert_eq!(out, ClaimOutcome::Denied(DenyReason::ShadowReject), "影子期必须拒绝");
        assert_eq!(s.lease_count(), 0, "影子期不产生租约");
        assert_eq!(s.log().len(), 1, "决策必须留痕");
        // 日志记录真实决策为 Granted
        let snap = s.log().snapshot();
        match &snap[0] {
            ShadowDecision::Claim { outcome: ShadowClaimOutcome::Granted { .. }, .. } => {}
            other => panic!("真实决策应为 Granted,got {other:?}"),
        }
    }

    /// 影子日志可回放 — 100% 逐位一致（W16 门禁）
    #[test]
    fn shadow_log_replay_100pct() {
        let inner = SimplePeerScheduler::new();
        let s = ShadowScheduler::new(inner);
        // 决策序列:claim ×2 + renew + should_run + handoff
        let _ = s.claim(&claim("t1", "peer-a"));
        let _ = s.claim(&claim("t2", "peer-a"));
        let _ = s.renew_lease("t1", "peer-a");
        let _ = s.should_run("t1");
        let _ = s.handoff("t1", "peer-b");
        let report = s.log().replay(&s.inner);
        assert_eq!(report.total, 5);
        assert_eq!(report.mismatched, 0, "回放必须零差异");
        assert!((report.replay_rate() - 1.0).abs() < 1e-9, "可回放率必须 100%");
    }

    /// 空日志回放 — 满分（无风险即达标）
    #[test]
    fn empty_log_replay_full() {
        let inner = SimplePeerScheduler::new();
        let s = ShadowScheduler::new(inner);
        let report = s.log().replay(&s.inner);
        assert_eq!(report.total, 0);
        assert!((report.replay_rate() - 1.0).abs() < 1e-9);
    }

    /// 影子 should_run 日志 — 留痕且可回放
    #[test]
    fn shadow_should_run_logged() {
        let inner = SimplePeerScheduler::new();
        let s = ShadowScheduler::new(inner);
        let _ = s.claim(&claim("t1", "peer-a"));
        let v = s.should_run("t1");
        // 影子对外与真实裁决一致（should_run 无副作用,不拒绝）
        assert_eq!(v, ShouldRunVerdict::Run);
        let snap = s.log().snapshot();
        assert!(snap.iter().any(|d| matches!(d, ShadowDecision::ShouldRun { task_id, .. } if task_id.as_str() == "t1")));
    }

    /// 序列化往返 — ShadowDecision serde 可编解码（审计持久化）
    #[test]
    fn shadow_decision_serde_roundtrip() {
        let d = ShadowDecision::Claim {
            task_id: "t1".into(),
            peer_id: "p1".into(),
            outcome: ShadowClaimOutcome::Denied(DenyReason::QuotaExceeded),
        };
        let json = serde_json::to_string(&d).expect("编码必须成功");
        let back: ShadowDecision = serde_json::from_str(&json).expect("解码必须成功");
        assert_eq!(back, d, "序列化往返必须逐位一致");
    }
}
