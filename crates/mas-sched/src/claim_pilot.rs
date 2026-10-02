//! 认领制 feature-gated 试点 — 控制面消费者目标池 + broker 护栏 + 回滚判据
//! （v2.3 §(b.4)(2) D#1；默认 off，仅 `--features claim_pilot` 编译）
//!
//! 对应架构层：L9 Quest（mas-sched 控制面，ADR-145）
//! 对应设计源：`Chimera架构重构与优化方案_v2.3` §(b.4)(2)「认领制 feature-gated 试点」
//!
//! # WHY（决策#1：认领制降级为试点）
//! 现状派单制**任务面零轮询、无回归病灶**（v2.3 §(a) D1 实锤），故认领制
//! **不作为全量 Wave 2 核心**，仅在默认 off 的 feature 后做**最小三层接线**：
//! 1. **消费者目标池**：`ClaimBroker` 维护试点 peer 池，仅池内 peer 的认领进入
//!    认领面，池外一律 `OutsidePilotPool`（保留派单兜底，不强制认领）；
//! 2. **broker 护栏**：委托的 `SimplePeerScheduler` 经 [`with_rate_limit`] 注入
//!    [`ClaimRateGovernor`] 频率硬上限（防轮询风暴，§(f) 新红线）+ 既有配额校验；
//! 3. **转正/回滚双判据预演**：[`ClaimBroker::rollback_verdict`] 依据试点期累计
//!    统计给出 `Proceed`（真实多消费者需求）或 `Rollback`（无需求/分发回归 → 回派单）。
//!
//! # 依赖纪律（不欺骗 machete 与读者）
//! 本试点为**纯控制面**消费者，仅复用 in-crate [`SimplePeerScheduler`] +
//! [`ClaimRateGovernor`] 与 L0 契约类型，**不新增任何外部依赖**；claim/renew/handoff
//! 的**事件面发布**（`event-bus` L1）留待试点确认真实消费者后，再按依赖铁律声明
//! `optional = true` + feature 门控接入（见 Cargo.toml 注 / lib.rs TODO(P3-T2)）。
//!
//! [`with_rate_limit`]: crate::SimplePeerScheduler::with_rate_limit
//! [`ClaimRateGovernor`]: crate::ClaimRateGovernor

use std::collections::HashSet;

use crate::scheduler::{PeerScheduler, SimplePeerScheduler};
use crate::types::{ClaimOutcome, DenyReason, Lease, TodoClaim};

/// 委托调度器后的 broker 决策（比 [`ClaimOutcome`] 多一维「池外」语义，
/// 从而无需为试点在 L0 [`DenyReason`] 追加变体——保持 L0 契约稳定）。
#[derive(Debug, Clone, PartialEq)]
pub enum BrokerDecision {
    /// 池内 peer 且调度器授予租约
    Granted(Lease),
    /// 池内 peer 但被调度器拒绝（配额/频率/独占，附原因）
    Denied(DenyReason),
    /// peer 不在试点目标池 → 不进入认领面（回派单兜底，§(d) 转正/回滚）
    OutsidePilotPool,
}

/// 转正/回滚判据结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackVerdict {
    /// 试点达成真实多消费者需求 → 可评估转正
    Proceed,
    /// 未达成或出现分发回归 → 回派单（附原因）
    Rollback(&'static str),
}

/// 认领制试点 broker —— 消费者目标池 + 频率护栏 + 回滚判据。
#[derive(Debug)]
pub struct ClaimBroker {
    /// 内层调度器（构造即带频率硬上限护栏，默认 new 不限流不适用试点）
    scheduler: SimplePeerScheduler,
    /// 试点消费者目标池
    pool: HashSet<String>,
    /// 试点窗口内实际授予过租约的 distinct peer（转正判据：需 > 1）
    served_peers: HashSet<String>,
    /// 进入认领面的认领总数（池内）
    routed_claims: u32,
    /// 被频率硬上限拒绝的次数（分发回归判据分子）
    rate_limited: u32,
}

impl ClaimBroker {
    /// 新建试点 broker。
    ///
    /// - `max_claims_per_window` / `window_ms`：broker 频率硬上限护栏参数（防轮询风暴）；
    /// - `pool`：试点消费者目标池 peer_id 集合。
    #[must_use]
    pub fn new(
        max_claims_per_window: u32,
        window_ms: u64,
        pool: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            scheduler: SimplePeerScheduler::with_rate_limit(max_claims_per_window, window_ms),
            pool: pool.into_iter().collect(),
            served_peers: HashSet::new(),
            routed_claims: 0,
            rate_limited: 0,
        }
    }

    /// 目标池大小（可观测/测试）。
    #[must_use]
    pub fn pool_size(&self) -> usize {
        self.pool.len()
    }

    /// 三层接线之「消费者目标池 + broker 护栏」入口：
    /// 池外 peer → [`BrokerDecision::OutsidePilotPool`]（不计数、走派单兜底）；
    /// 池内 peer → 委托 [`PeerScheduler::claim`]（其内含配额 + 频率硬上限护栏）。
    pub fn route_claim(&mut self, claim: &TodoClaim) -> BrokerDecision {
        if !self.pool.contains(&claim.peer_id) {
            return BrokerDecision::OutsidePilotPool;
        }
        self.routed_claims += 1;
        match self.scheduler.claim(claim) {
            ClaimOutcome::Granted(lease) => {
                self.served_peers.insert(claim.peer_id.clone());
                BrokerDecision::Granted(lease)
            }
            ClaimOutcome::Denied(reason) => {
                if reason == DenyReason::RateLimited {
                    self.rate_limited += 1;
                }
                BrokerDecision::Denied(reason)
            }
        }
    }

    /// 续期租约（委托调度器，透传结果；试点不额外限流续期）。
    pub fn renew(
        &self,
        task_id: &str,
        peer_id: &str,
    ) -> Result<crate::types::RenewOutcome, crate::SchedError> {
        self.scheduler.renew_lease(task_id, peer_id)
    }

    /// 移交/释放租约（委托调度器，透传结果）。
    pub fn handoff(&self, task_id: &str, to_peer: &str) -> Result<(), crate::SchedError> {
        self.scheduler.handoff(task_id, to_peer)
    }

    /// 转正/回滚双判据（§(b.4)(2)「回滚判据预演」）：
    ///
    /// - **无真实多消费者需求**：试点期内实际授予过租约的 distinct peer ≤ 1
    ///   → [`RollbackVerdict::Rollback`]（回派单）；
    /// - **分发回归**：频率护栏拒绝占比 ≥ 半数（`rate_limited * 2 >= routed`），
    ///   即认领面出现超频竞争（引入轮询/回退风险）→ [`RollbackVerdict::Rollback`]；
    /// - 否则 → [`RollbackVerdict::Proceed`]（可评估转正）。
    #[must_use]
    pub fn rollback_verdict(&self) -> RollbackVerdict {
        if self.routed_claims == 0 || self.served_peers.len() <= 1 {
            return RollbackVerdict::Rollback("无多消费者需求(distinct served peer ≤ 1)");
        }
        if self.rate_limited.saturating_mul(2) >= self.routed_claims {
            return RollbackVerdict::Rollback("分发回归(频率护栏拒绝占比过高)");
        }
        RollbackVerdict::Proceed
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::types::Priority;

    fn broker_with_pool(peers: &[&str]) -> ClaimBroker {
        // 频率上限放宽到 100，隔离出「目标池」与「回滚判据」两条主线
        ClaimBroker::new(
            100,
            100_000,
            peers.iter().map(|p| (*p).to_string()).collect::<Vec<_>>(),
        )
    }

    fn claim(task: &str, peer: &str) -> TodoClaim {
        let mut c = TodoClaim::new(task, peer, Priority::Medium, 1_000);
        c.quota.max_concurrent = 100;
        c
    }

    #[test]
    fn outside_pool_is_not_routed() {
        let mut b = broker_with_pool(&["peer-a"]);
        assert_eq!(
            b.route_claim(&claim("t1", "peer-x")),
            BrokerDecision::OutsidePilotPool,
            "池外 peer 不应进入认领面(回派单兜底)"
        );
        assert_eq!(b.pool_size(), 1);
    }

    #[test]
    fn pool_peer_grants_and_tracks_demand() {
        let mut b = broker_with_pool(&["peer-a", "peer-b"]);
        assert!(matches!(
            b.route_claim(&claim("t1", "peer-a")),
            BrokerDecision::Granted(_)
        ));
        assert!(matches!(
            b.route_claim(&claim("t2", "peer-b")),
            BrokerDecision::Granted(_)
        ));
        // 两个 distinct peer 服务过 → 达成多消费者需求 → Proceed
        assert_eq!(b.rollback_verdict(), RollbackVerdict::Proceed);
    }

    #[test]
    fn single_consumer_triggers_rollback() {
        let mut b = broker_with_pool(&["peer-a", "peer-b"]);
        // 仅 peer-a 实际认领(peer-b 在池但无需求)
        assert!(matches!(
            b.route_claim(&claim("t1", "peer-a")),
            BrokerDecision::Granted(_)
        ));
        assert!(matches!(
            b.route_claim(&claim("t2", "peer-a")),
            BrokerDecision::Granted(_)
        ));
        assert!(
            matches!(b.rollback_verdict(), RollbackVerdict::Rollback(_)),
            "distinct served peer ≤ 1 应判回滚(无多消费者需求)"
        );
    }

    #[test]
    fn rate_limit_regression_triggers_rollback() {
        // 频率上限 1：同窗口内 peer 第 2 次认领即被拒 → 拒绝占比高 → 分发回归
        let mut b = ClaimBroker::new(1, 100_000, vec!["peer-a".to_string(), "peer-b".to_string()]);
        // 两 peer 各授予一次(满足 distinct>1)，再各撞一次频率上限
        assert!(matches!(
            b.route_claim(&claim("t1", "peer-a")),
            BrokerDecision::Granted(_)
        ));
        assert!(matches!(
            b.route_claim(&claim("t2", "peer-b")),
            BrokerDecision::Granted(_)
        ));
        assert_eq!(
            b.route_claim(&claim("t3", "peer-a")),
            BrokerDecision::Denied(DenyReason::RateLimited)
        );
        assert_eq!(
            b.route_claim(&claim("t4", "peer-b")),
            BrokerDecision::Denied(DenyReason::RateLimited)
        );
        // rate_limited=2, routed=4 → 2*2>=4 → 分发回归回滚
        assert_eq!(
            b.rollback_verdict(),
            RollbackVerdict::Rollback("分发回归(频率护栏拒绝占比过高)")
        );
    }

    #[test]
    fn renew_and_handoff_delegate_to_scheduler() {
        let mut b = broker_with_pool(&["peer-a", "peer-b"]);
        assert!(matches!(
            b.route_claim(&claim("t1", "peer-a")),
            BrokerDecision::Granted(_)
        ));
        // 持有者续期成功
        assert_eq!(
            b.renew("t1", "peer-a").unwrap(),
            crate::types::RenewOutcome::Renewed
        );
        // 移交给 peer-b
        b.handoff("t1", "peer-b").expect("移交应成功");
        // 空认领统计下无需求 → 但已服务过 peer-a(served_peers={peer-a}) len=1 → 回滚
        assert!(matches!(b.rollback_verdict(), RollbackVerdict::Rollback(_)));
    }
}
