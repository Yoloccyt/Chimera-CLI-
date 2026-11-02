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
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::broker_guardrail::ClaimRateGovernor;
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
#[derive(Debug)]
pub struct SimplePeerScheduler {
    /// 租约表（task_id → Lease）
    leases: RwLock<HashMap<TaskId, Lease>>,
    /// 每 peer 并发计数（peer_id → 活跃租约数）
    peer_concurrency: RwLock<HashMap<String, usize>>,
    /// v2.3 §(b.4)(2) 认领频率护栏（默认 `None`=不限流，保既有 claim 测试不受影响；
    /// 经 [`SimplePeerScheduler::with_rate_limit`] 启用，`claim` 取 `&self` 故用 `Mutex` 内部可变）
    rate_governor: Option<Mutex<ClaimRateGovernor>>,
    /// 限流窗口时钟基准（`claim` 内换算 `now_ms` 注入 governor，无忙等/无自旋）
    claim_clock: Instant,
}

impl Default for SimplePeerScheduler {
    fn default() -> Self {
        Self {
            leases: RwLock::new(HashMap::new()),
            peer_concurrency: RwLock::new(HashMap::new()),
            rate_governor: None,
            claim_clock: Instant::now(),
        }
    }
}

impl SimplePeerScheduler {
    /// 新建调度器
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 新建**带认领频率硬上限**的调度器（v2.3 §(b.4)(2) broker 护栏）。
    ///
    /// 单 peer 在一个 `window_ms` 窗口内至多 `max_claims_per_window` 次认领，
    /// 超频即以 [`DenyReason::RateLimited`] 拒绝（防轮询风暴）。默认 [`new`](Self::new)
    /// 不启用限流，此构造器为认领制 feature-gated 试点的显式接入点。
    #[must_use]
    pub fn with_rate_limit(max_claims_per_window: u32, window_ms: u64) -> Self {
        Self {
            rate_governor: Some(Mutex::new(ClaimRateGovernor::new(
                max_claims_per_window,
                window_ms,
            ))),
            ..Self::default()
        }
    }

    /// 频率护栏校验（仅在启用限流时）：放行返回 `Ok(())`，超频返回 `Err(RateLimited)`。
    fn check_rate_limit(&self, peer_id: &str) -> Result<(), DenyReason> {
        if let Some(governor) = &self.rate_governor {
            let now_ms = u64::try_from(self.claim_clock.elapsed().as_millis()).unwrap_or(u64::MAX);
            let admitted = governor
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .try_admit(peer_id, now_ms);
            if !admitted {
                return Err(DenyReason::RateLimited);
            }
        }
        Ok(())
    }
}

impl PeerScheduler for SimplePeerScheduler {
    /// 申请任务租约 — **单一原子事务**消除两处 TOCTOU（v2.0 §7-bis P1 前置 0）
    ///
    /// WHY 合并持锁:旧实现把「任务独占检查」与「插入 + 并发自增」分处两次
    /// `leases.write()`（双锁间隙 → 同 `task_id` 可双授予）,且并发上限判定读
    /// `peer_concurrency.read()` 与自增的 `peer_concurrency.write()` 分离（跨 peer
    /// 可破 `max_concurrent`）。现把「独占检查 + 配额检查 + 插入 + 自增」放入
    /// **同一次临界区**:按固定锁序 `leases → peer_concurrency` 先后取写锁并同持,
    /// 两表要么都不改（提前拒绝）、要么一起生效（授予）,不存在中间窗口。
    ///
    /// 锁序不变量:`leases` 写锁恒**先于** `peer_concurrency` 写锁取得,任何路径不得
    /// 反向（`handoff`/`renew_lease` 均先取 `leases`、对 `peer` 的取用在其释放之后）,
    /// 故与既有方法不会构成死锁环。本模块无 async,不存在持锁跨 await（红线 §4.1）。
    fn claim(&self, claim: &TodoClaim) -> ClaimOutcome {
        // 0. 频率硬上限护栏（feature-gated 试点默认 off：仅 with_rate_limit 构造时启用）
        if let Err(reason) = self.check_rate_limit(&claim.peer_id) {
            return ClaimOutcome::Denied(reason);
        }
        // 1. 时长上限（纯字段比较,无锁先决条件,等价旧 check_quota 的时长分支）
        if claim.est_duration_ms > claim.quota.max_duration_ms {
            return ClaimOutcome::Denied(DenyReason::DurationExceedsQuota);
        }
        // 2. 原子事务:独占检查 → 配额检查 → 插入 → 自增,全程同持两写锁
        let mut leases = self.leases.write().unwrap_or_else(|p| p.into_inner());
        if leases.contains_key(&claim.task_id) {
            return ClaimOutcome::Denied(DenyReason::TaskClaimed);
        }
        let mut peers = self
            .peer_concurrency
            .write()
            .unwrap_or_else(|p| p.into_inner());
        let active = peers.get(&claim.peer_id).copied().unwrap_or(0);
        if active >= claim.quota.max_concurrent {
            // peers/leases 写锁随作用域释放,两表均未被改动 → 无半成品授予
            return ClaimOutcome::Denied(DenyReason::QuotaExceeded);
        }
        let lease = Lease {
            task_id: claim.task_id.clone(),
            peer_id: claim.peer_id.clone(),
            lease_until: Instant::now() + Duration::from_millis(claim.quota.max_duration_ms),
            renewable: true,
            duration_ms: claim.quota.max_duration_ms,
        };
        leases.insert(claim.task_id.clone(), lease.clone());
        *peers.entry(claim.peer_id.clone()).or_insert(0) += 1;
        // guards 于函数结束时依次释放(后取先放:peers→leases),与获取序相反不构成环
        ClaimOutcome::Granted(lease)
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
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试码 unwrap 为 Rust 惯用法；E-5 lint 意在治理生产码
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

    /// 同 task_id 并发 claim 恰一个授予 — 锁住旧实现 claim 的"独占检查/插入"双锁间隙
    ///
    /// WHY 该测试此前不存在（`duplicate_claim_denied` 单线程串行、`concurrent_claims`
    /// 各线程用**不同** task_id）,故双锁间隙长期潜伏。用 Barrier 对齐起跑放大竞态窗口,
    /// 各线程用 **distinct peer + 放宽配额**,把"任务独占"与配额约束隔离:旧实现会在
    /// 步骤 2 释放写锁、步骤 3 重取之间双双通过独占检查 → 双授予;新实现独占检查与
    /// 插入同处一次 `leases.write()` → 恰一个成功。本测在旧实现必红、新实现必绿。
    #[test]
    fn concurrent_same_task_claim_grants_exactly_one() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier};
        const N: usize = 16;
        let s = Arc::new(SimplePeerScheduler::new());
        let gate = Arc::new(Barrier::new(N));
        let granted = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..N)
            .map(|i| {
                let (s, gate, granted) = (Arc::clone(&s), Arc::clone(&gate), Arc::clone(&granted));
                std::thread::spawn(move || {
                    let mut c =
                        TodoClaim::new("t-race", format!("peer-{i}"), Priority::Medium, 10_000);
                    // 放宽配额:隔离"任务独占"竞态,拒绝只因 TaskClaimed 非 QuotaExceeded
                    c.quota.max_concurrent = 100;
                    gate.wait();
                    if matches!(s.claim(&c), ClaimOutcome::Granted(_)) {
                        granted.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("并发 claim 线程应正常退出");
        }
        assert_eq!(
            granted.load(Ordering::SeqCst),
            1,
            "同 task_id 并发 claim 必须恰一个 Granted"
        );
        assert_eq!(s.lease_count(), 1, "租约表不得出现双授予");
    }

    /// 同 peer 并发 claim 不超配额 — 锁住旧实现 check_quota 读/自增写分离的配额 TOCTOU
    ///
    /// WHY:`max_concurrent=1` + distinct task_id + 同一 peer——旧实现各线程在独立的
    /// `peer_concurrency.read()` 窗口都看到 active=0 而放行,再到分离的写锁各自 +1 →
    /// N 个授予(超配额);新实现把并发上限判定与自增并入同一次 `peer_concurrency.write()`
    /// 临界区 → 首个进入者把 active 置 1 后释放锁,其余看到 active=1≥1 遭拒。本测在旧
    /// 实现必红(granted==N)、新实现必绿(granted==1),且与 task 独占维度正交(各线程 task_id 唯一)。
    #[test]
    fn concurrent_distinct_task_respects_per_peer_quota() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier};
        const N: usize = 16;
        let s = Arc::new(SimplePeerScheduler::new());
        let gate = Arc::new(Barrier::new(N));
        let granted = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..N)
            .map(|i| {
                let (s, gate, granted) = (Arc::clone(&s), Arc::clone(&gate), Arc::clone(&granted));
                std::thread::spawn(move || {
                    // 同一 peer、各用 distinct task_id → 只隔离配额维度,不触发 TaskClaimed
                    let mut c =
                        TodoClaim::new(format!("t-{i}"), "peer-shared", Priority::Medium, 10_000);
                    c.quota.max_concurrent = 1;
                    gate.wait();
                    if matches!(s.claim(&c), ClaimOutcome::Granted(_)) {
                        granted.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("并发 claim 线程应正常退出");
        }
        assert_eq!(
            granted.load(Ordering::SeqCst),
            1,
            "同 peer max_concurrent=1 并发下必须恰一个授予"
        );
        assert_eq!(s.lease_count(), 1, "并发计数不得被突破");
    }

    /// 默认 new() 不限流 — 同一 peer 多次认领均成功（只要不超并发配额）
    #[test]
    fn default_new_has_no_rate_limit() {
        let s = SimplePeerScheduler::new();
        for i in 0..50 {
            let mut c = TodoClaim::new(format!("t{i}"), "peer-a", Priority::Medium, 1_000);
            c.quota.max_concurrent = 100; // 放宽并发,隔离出“无频率限制”
            assert!(
                matches!(s.claim(&c), ClaimOutcome::Granted(_)),
                "默认调度器不得引入频率限制（§(b.4)(2) 默认 off）"
            );
        }
    }

    /// with_rate_limit 超频拒绝 — 同窗口内第 3 次认领遭 RateLimited
    #[test]
    fn rate_limit_denies_after_cap() {
        // 窗口 100s（足以覆盖测试执行时长）→ 同一窗口内上限 2 次
        let s = SimplePeerScheduler::with_rate_limit(2, 100_000);
        let mut c1 = claim("t1", "peer-a");
        c1.quota.max_concurrent = 100;
        let mut c2 = claim("t2", "peer-a");
        c2.quota.max_concurrent = 100;
        let mut c3 = claim("t3", "peer-a");
        c3.quota.max_concurrent = 100;
        assert!(matches!(s.claim(&c1), ClaimOutcome::Granted(_)));
        assert!(matches!(s.claim(&c2), ClaimOutcome::Granted(_)));
        assert_eq!(
            s.claim(&c3),
            ClaimOutcome::Denied(DenyReason::RateLimited),
            "超频率硬上限必须拒绝（防轮询风暴）"
        );
    }

    /// 频率护栏按 peer 隔离 — a 超频不影响 b 的独立配额
    #[test]
    fn rate_limit_is_per_peer() {
        let s = SimplePeerScheduler::with_rate_limit(1, 100_000);
        let mut ca = claim("t1", "peer-a");
        ca.quota.max_concurrent = 100;
        let mut cb = claim("t2", "peer-b");
        cb.quota.max_concurrent = 100;
        assert!(matches!(s.claim(&ca), ClaimOutcome::Granted(_)));
        let mut ca2 = claim("t3", "peer-a");
        ca2.quota.max_concurrent = 100;
        assert_eq!(s.claim(&ca2), ClaimOutcome::Denied(DenyReason::RateLimited));
        assert!(
            matches!(s.claim(&cb), ClaimOutcome::Granted(_)),
            "peer-b 有独立频率配额,不因 a 超频受连累"
        );
    }
}
