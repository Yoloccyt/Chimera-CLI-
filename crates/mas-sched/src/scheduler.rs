//! 调度器核心 — PeerScheduler trait + SimplePeerScheduler（P3-T2，v4.0 WI-29）
//!
//! 对应架构层: L9 Quest（mas-sched 控制面，ADR-145）
//!
//! # 契约（v4.0 WI-29 接口）
//! ```text
//! PeerScheduler{claim / renew_lease / handoff / should_run}
//! ```
//! 控制面纯调度不碰工具执行。状态:租约表（task_id → Lease）+ 每 peer 并发计数。
//!
//! # 并发
//! `RwLock<HashMap>` 承载（claim/renew/handoff 低频,读多写少）;
//! 单实例 `Arc<dyn PeerScheduler>` 共享,无自旋、无持锁跨 await（本模块无 async）。

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use crate::error::SchedError;
use crate::types::{
    ClaimOutcome, DenyReason, Lease, Priority, RenewOutcome, ShouldRunVerdict, TaskId, TodoClaim,
    HANDOFF,
};

/// 调度器 trait — 控制面四原语（v4.0 WI-29 契约）
pub trait PeerScheduler: Send + Sync {
    /// 申请任务租约（长任务 claim）
    fn claim(&self, claim: &TodoClaim) -> ClaimOutcome;
    /// 续期租约（持有者调用）
    fn renew_lease(&self, task_id: &str, peer_id: &str) -> Result<RenewOutcome, SchedError>;
    /// 移交任务（给指定 peer 或回编排层 HANDOFF）
    fn handoff(&self, task_id: &str, to_peer: &str) -> Result<(), SchedError>;
    /// 是否应运行（Loop 记分卡联动入口,WI-32 第四因子输入）
    fn should_run(&self, task_id: &str) -> ShouldRunVerdict;
    /// 当前租约数（诊断/遥测）
    fn lease_count(&self) -> usize;
}

/// 内存调度器 — 租约表 + 每 peer 并发计数 + 配额校验
#[derive(Debug, Default)]
pub struct SimplePeerScheduler {
    /// 租约表（task_id → Lease）
    leases: RwLock<HashMap<TaskId, Lease>>,
    /// 每 peer 并发计数（peer_id → 活跃租约数）
    peer_concurrency: RwLock<HashMap<String, usize>>,
}

impl SimplePeerScheduler {
    /// 新建调度器
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 校验配额 — 时长上限 + 并发上限（控制面硬约束）
    fn check_quota(&self, claim: &TodoClaim) -> Result<(), DenyReason> {
        if claim.est_duration_ms > claim.quota.max_duration_ms {
            return Err(DenyReason::DurationExceedsQuota);
        }
        let peers = self
            .peer_concurrency
            .read()
            .unwrap_or_else(|p| p.into_inner());
        let active = peers.get(&claim.peer_id).copied().unwrap_or(0);
        if active >= claim.quota.max_concurrent {
            return Err(DenyReason::QuotaExceeded);
        }
        Ok(())
    }

    /// 内部授予 — 写租约表 + 并发计数（锁内完成,无跨锁 await）
    fn grant_locked(&self, claim: &TodoClaim) -> Lease {
        let lease = Lease {
            task_id: claim.task_id.clone(),
            peer_id: claim.peer_id.clone(),
            lease_until: Instant::now() + Duration::from_millis(claim.quota.max_duration_ms),
            renewable: true,
            duration_ms: claim.quota.max_duration_ms,
        };
        {
            let mut leases = self.leases.write().unwrap_or_else(|p| p.into_inner());
            leases.insert(claim.task_id.clone(), lease.clone());
        }
        {
            let mut peers = self
                .peer_concurrency
                .write()
                .unwrap_or_else(|p| p.into_inner());
            *peers.entry(claim.peer_id.clone()).or_insert(0) += 1;
        }
        lease
    }
}

impl PeerScheduler for SimplePeerScheduler {
    fn claim(&self, claim: &TodoClaim) -> ClaimOutcome {
        // 1. 时长/并发配额校验（读锁,先决条件）
        if let Err(reason) = self.check_quota(claim) {
            return ClaimOutcome::Denied(reason);
        }
        // 2. 任务独占检查（写锁:已存在即拒绝）
        {
            let leases = self.leases.write().unwrap_or_else(|p| p.into_inner());
            if leases.contains_key(&claim.task_id) {
                return ClaimOutcome::Denied(DenyReason::TaskClaimed);
            }
        }
        // 3. 授予（锁内完成,无跨锁 await）
        ClaimOutcome::Granted(self.grant_locked(claim))
    }

    fn renew_lease(&self, task_id: &str, peer_id: &str) -> Result<RenewOutcome, SchedError> {
        let mut leases = self.leases.write().unwrap_or_else(|p| p.into_inner());
        let lease = leases
            .get_mut(task_id)
            .ok_or_else(|| SchedError::TaskNotFound(TaskId::from(task_id)))?;
        // 主体校验:仅持有者可续期
        if lease.peer_id != peer_id {
            return Err(SchedError::LeaseHolderMismatch(
                TaskId::from(task_id),
                lease.peer_id.clone(),
                peer_id.to_string(),
            ));
        }
        if !lease.renewable {
            return Ok(RenewOutcome::NotRenewable);
        }
        if lease.lease_until <= Instant::now() {
            return Ok(RenewOutcome::Expired);
        }
        // 续期:延长一个租约时长
        lease.lease_until += Duration::from_millis(lease.duration_ms);
        Ok(RenewOutcome::Renewed)
    }

    fn handoff(&self, task_id: &str, to_peer: &str) -> Result<(), SchedError> {
        let mut leases = self.leases.write().unwrap_or_else(|p| p.into_inner());
        let lease = leases
            .get_mut(task_id)
            .ok_or_else(|| SchedError::TaskNotFound(TaskId::from(task_id)))?;
        // 回编排层:释放租约（HANDOFF 哨兵）
        if to_peer == HANDOFF {
            let old_peer = lease.peer_id.clone();
            leases.remove(task_id);
            drop(leases);
            self.decrement_peer(&old_peer);
            return Ok(());
        }
        // 移交:更新持有者（peer 并发计数转移）
        let old_peer = lease.peer_id.clone();
        lease.peer_id = to_peer.to_string();
        drop(leases);
        self.decrement_peer(&old_peer);
        {
            let mut peers = self
                .peer_concurrency
                .write()
                .unwrap_or_else(|p| p.into_inner());
            *peers.entry(to_peer.to_string()).or_insert(0) += 1;
        }
        Ok(())
    }

    fn should_run(&self, task_id: &str) -> ShouldRunVerdict {
        let leases = self.leases.read().unwrap_or_else(|p| p.into_inner());
        match leases.get(task_id) {
            // 无租约 = 无已受理工作 → 无可行工作（记分卡语义）
            None => ShouldRunVerdict::NoActionableWork,
            // 有租约且未过期 = 应运行
            Some(l) if l.lease_until > Instant::now() => ShouldRunVerdict::Run,
            // 有租约但已过期 = 延后（需重新 claim）
            Some(_) => ShouldRunVerdict::Defer,
        }
    }

    fn lease_count(&self) -> usize {
        self.leases.read().unwrap_or_else(|p| p.into_inner()).len()
    }
}

impl SimplePeerScheduler {
    /// 递减 peer 并发计数（handoff/释放内部辅助,无任务时移除条目）
    fn decrement_peer(&self, peer_id: &str) {
        let mut peers = self
            .peer_concurrency
            .write()
            .unwrap_or_else(|p| p.into_inner());
        match peers.get_mut(peer_id) {
            Some(n) if *n > 1 => *n -= 1,
            Some(_) => {
                peers.remove(peer_id);
            }
            None => {}
        }
    }

    /// 优先级辅助 — 高优先级任务在配额竞争时优先（未来扩展:priority_weight 加权）
    #[allow(dead_code)]
    fn priority_rank(p: Priority) -> u8 {
        match p {
            Priority::Low => 0,
            Priority::Medium => 1,
            Priority::High => 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(task: &str, peer: &str) -> TodoClaim {
        TodoClaim::new(task, peer, Priority::Medium, 10_000)
    }

    /// claim 授予 — 正常路径返回租约
    #[test]
    fn claim_granted() {
        let s = SimplePeerScheduler::new();
        match s.claim(&claim("t1", "peer-a")) {
            ClaimOutcome::Granted(l) => {
                assert_eq!(l.task_id.as_str(), "t1");
                assert_eq!(l.peer_id, "peer-a");
                assert!(l.renewable);
            }
            ClaimOutcome::Denied(r) => panic!("不应拒绝: {r:?}"),
        }
        assert_eq!(s.lease_count(), 1);
    }

    /// 重复 claim 拒绝 — 任务独占（TaskClaimed）
    #[test]
    fn duplicate_claim_denied() {
        let s = SimplePeerScheduler::new();
        assert!(matches!(
            s.claim(&claim("t1", "peer-a")),
            ClaimOutcome::Granted(_)
        ));
        assert_eq!(
            s.claim(&claim("t1", "peer-b")),
            ClaimOutcome::Denied(DenyReason::TaskClaimed),
            "任务已被 claim 必须拒绝"
        );
    }

    /// 配额拒绝 — 并发超限（QuotaExceeded）
    #[test]
    fn quota_exceeded_denied() {
        let s = SimplePeerScheduler::new();
        let mut c = TodoClaim::new("t1", "peer-a", Priority::Medium, 10_000);
        c.quota.max_concurrent = 2;
        assert!(matches!(s.claim(&c), ClaimOutcome::Granted(_)));
        let mut c2 = TodoClaim::new("t2", "peer-a", Priority::Medium, 10_000);
        c2.quota.max_concurrent = 2;
        assert!(matches!(s.claim(&c2), ClaimOutcome::Granted(_)));
        let mut c3 = TodoClaim::new("t3", "peer-a", Priority::Medium, 10_000);
        c3.quota.max_concurrent = 2;
        assert_eq!(
            s.claim(&c3),
            ClaimOutcome::Denied(DenyReason::QuotaExceeded),
            "并发超限必须拒绝"
        );
    }

    /// 时长超限拒绝 — DurationExceedsQuota
    #[test]
    fn duration_exceeds_quota_denied() {
        let s = SimplePeerScheduler::new();
        let mut c = TodoClaim::new("t1", "peer-a", Priority::Medium, 999_999);
        c.quota.max_duration_ms = 1000;
        assert_eq!(
            s.claim(&c),
            ClaimOutcome::Denied(DenyReason::DurationExceedsQuota)
        );
    }

    /// renew — 持有者可续,非持有者报错,不可续租约拒绝
    #[test]
    fn renew_semantics() {
        let s = SimplePeerScheduler::new();
        let mut c = claim("t1", "peer-a");
        c.quota.max_duration_ms = 60_000;
        let _ = s.claim(&c);
        // 非持有者
        assert!(s.renew_lease("t1", "peer-b").is_err(), "非持有者必须报错");
        // 持有者
        assert_eq!(s.renew_lease("t1", "peer-a"), Ok(RenewOutcome::Renewed));
        // 不存在
        assert!(s.renew_lease("t-x", "peer-a").is_err());
    }

    /// handoff — 移交到 peer 与回编排层（HANDOFF）
    #[test]
    fn handoff_semantics() {
        let s = SimplePeerScheduler::new();
        let _ = s.claim(&claim("t1", "peer-a"));
        // 移交到 peer-b
        s.handoff("t1", "peer-b").expect("移交必须成功");
        let leases = s.leases.read().unwrap();
        assert_eq!(leases.get("t1").map(|l| l.peer_id.as_str()), Some("peer-b"));
        drop(leases);
        // 回编排层:租约释放
        s.handoff("t1", HANDOFF).expect("回编排层必须成功");
        assert_eq!(s.lease_count(), 0);
        // 不存在任务
        assert!(s.handoff("t-x", "peer-a").is_err());
    }

    /// should_run — 无租约=无工作;有效租约=Run;过期=Defer
    #[test]
    fn should_run_verdict() {
        let s = SimplePeerScheduler::new();
        assert_eq!(s.should_run("t1"), ShouldRunVerdict::NoActionableWork);
        let _ = s.claim(&claim("t1", "peer-a"));
        assert_eq!(s.should_run("t1"), ShouldRunVerdict::Run);
        // 过期租约:手动构造过期 lease
        {
            let mut leases = s.leases.write().unwrap();
            let l = leases.get_mut("t1").unwrap();
            l.lease_until = Instant::now() - Duration::from_secs(1);
        }
        assert_eq!(s.should_run("t1"), ShouldRunVerdict::Defer);
    }

    /// 并发 claim — 多线程无 panic 无竞争（RwLock 语义）;默认配额上限生效
    ///
    /// 期望:每 peer 默认 max_concurrent=4 → 8 线程 × 4 = 32 授予,
    /// 其余 48 被 QuotaExceeded 拒绝（并发上限是控制面硬约束,非丢失）
    #[test]
    fn concurrent_claims() {
        let s = std::sync::Arc::new(SimplePeerScheduler::new());
        let mut granted = 0usize;
        let mut denied = 0usize;
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let s = std::sync::Arc::clone(&s);
                std::thread::spawn(move || {
                    let mut g = 0usize;
                    let mut d = 0usize;
                    for j in 0..10usize {
                        let c = TodoClaim::new(
                            format!("t{i}-{j}"),
                            format!("peer-{i}"),
                            Priority::Medium,
                            1_000,
                        );
                        match s.claim(&c) {
                            ClaimOutcome::Granted(_) => g += 1,
                            ClaimOutcome::Denied(DenyReason::QuotaExceeded) => d += 1,
                            ClaimOutcome::Denied(_) => {}
                        }
                    }
                    (g, d)
                })
            })
            .collect();
        for h in handles {
            let (g, d) = h.join().expect("线程应正常退出");
            granted += g;
            denied += d;
        }
        assert_eq!(granted, 8 * 4, "每 peer 恰 4 个授予（配额上限）");
        assert_eq!(denied, 8 * 6, "其余被配额拒绝");
        assert_eq!(s.lease_count(), 32, "全部授予必须可见");
    }
}
