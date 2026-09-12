//! CBF 信用流原语 — 订阅者按消费速率获信用、发布者无信用挂起的背压控制
//!
//! 对应架构层:L1 Core(event-bus)
//! 对应任务:P1-T11(Phase 1 地基波次,手册 §8.5 / T-06 / v4.0 WI-08)
//!
//! # 核心思想(手册 §8.5 CBF 信用流)
//! - 信用池初值 256:发布普通事件时 `acquire(1)` 扣减信用;
//! - 信用耗尽时**不丢弃事件**:回退既有 broadcast 语义(broadcast 自身有
//!   Lagged 保护 + SlowConsumerDropped 告警),累计 `credit_shed_total` 指标;
//! - 归还由订阅者按消费速率批量归还(`release_many`,ADR-125 批提交语义),
//!   归还后通过 `Notify` 唤醒高优等待者;
//! - 高优事件(`Priority::High`)在信用不足时**异步等待 ≤100ms 窗口**
//!   (`tokio::sync::Notify`),窗口内被归还唤醒则成功,超时返回 `CreditError::Timeout`;
//! - 普通事件(`Priority::Normal`)`try_acquire` 失败立即返回,由调用方决定
//!   shed 或走既有 broadcast 兜底。
//!
//! # 红线:Critical 事件不经过信用流(豁免)
//! Critical 事件(severity() == Critical,17 个变体)由 mpsc 旁路通道保证投递
//! (容量 4096,`is_critical_mpsc_event` 判定 13 个 + broadcast),**绝不进入信用流**。
//!
//! WHY 豁免(推演 9):**Critical 背压 = 死锁源**。若 Critical 事件也参与信用
//! 扣减/等待,则 Critical 订阅者(如 SecCore/Parliament)在自身消费慢时会让
//! 信用池枯竭,而发布方等待归还 —— 归还依赖订阅者消费,订阅者消费依赖
//! 事件到达 —— 形成发布方 ↔ 订阅方的循环等待,任何超时窗口都无法解开的
//! 死锁。Critical 通道必须无条件可投(有界 4096 内),背压只作用于可重试的
//! 普通事件。
//!
//! # 实现选择理由(AtomicU64 vs Mutex)
//! 信用池选择 `AtomicU64` 而非 `Mutex<u64>`:
//! - publish 是热路径(门禁 > 100K msg/s),Mutex 加解锁 ~25ns 且存在竞争
//!   队列,AtomicU64 CAS 无锁 ~1ns,热路径零锁竞争;
//! - 扣减是"读-改-写"操作,`fetch_update` 原子完成比较并更新,不超发
//!   (并发 acquire 不会把信用扣成负数 —— 见并发测试守护);
//! - 与 bus.rs `lagged_count`/`published_total` 的选型一致(§4.4 红线 1
//!   "持锁跨 await",AtomicU64 无锁且 store/load 不跨 await)。
//!
//! # ADR-129:无自旋
//! 所有等待路径唯一原语是 `tokio::sync::Notify`,不存在自旋循环。
//! 等待者挂起至 `notify_waiters` 唤醒(归还触发)或超时窗口到期。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use thiserror::Error;
use tokio::sync::Notify;

/// 默认信用池大小 — 初始 256 信用(手册 §8.5)
///
/// WHY 256:与 broadcast 默认容量 1024 量级匹配,可吸收约 1/4 容量的
/// 短时突发后开始 shed(broadcast 兜底),同时不至于过早触发 shed。
pub const DEFAULT_CREDITS: u64 = 256;

/// 高优先级等待窗口 — 默认 100ms(手册 §8.5 / T-06)
///
/// WHY 100ms:足够短以容忍订阅者消费一批事件的正常归还延迟,又足够长
/// 以覆盖一次批量归还(ADR-125 批提交)的到达时间;超过则视为背压,
/// 高优事件也回退 shed(调用方决定),不无限挂起(架构红线"void Promise
/// 无 await"教训)。
pub const HIGH_PRIORITY_WAIT_WINDOW: Duration = Duration::from_millis(100);

/// 信用流错误类型(thiserror 库层错误,§4.1 约定)
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CreditError {
    /// 高优等待超时 — 窗口内未等到归还
    #[error("信用等待超时(窗口 {0:?} 内未获得信用)")]
    Timeout(Duration),
    /// 信用不足 — try_acquire 立即失败(请求 n 个单位)
    #[error("信用不足(需 {0} 个信用)")]
    Insufficient(u64),
}

/// 事件优先级 — 信用流分级语义
///
/// # Critical 事件不在此枚举中(红线)
/// Critical 事件**不经过信用流**(豁免),由 mpsc 旁路无条件投递。
/// WHY(Critical 背压 = 死锁源,推演 9):Critical 参与信用等待会形成
/// 发布方 ↔ 订阅方循环等待,详见模块级文档。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    /// 高重要性事件:信用不足时异步等待 ≤[`HIGH_PRIORITY_WAIT_WINDOW`](100ms),
    /// 窗口内被归还唤醒则成功;超时返回 [`CreditError::Timeout`]
    High,
    /// 普通事件:try_acquire 失败立即返回,调用方决定 shed 或走既有 broadcast
    Normal,
}

/// 信用流观测统计 — [`EventBus::credit_stats`] 返回类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreditStats {
    /// 当前可用信用(扣减后剩余)
    pub available: u64,
    /// 信用耗尽回退 broadcast 的累计事件数(单调递增,运维观测)
    pub shed_total: u64,
    /// 高优等待累计次数(High 事件进入等待窗口的次数,运维观测)
    pub high_wait_total: u64,
}

/// CBF 信用流 — 信用池 + 高优等待窗口 + 批量归还
///
/// 线程安全:所有状态为 `AtomicU64` + `Notify`,多线程并发 acquire/release
/// 安全;`Notify` 唤醒为异步等待(无自旋,ADR-129)。
pub struct CreditFlow {
    /// 当前可用信用 — 无锁 CAS 扣减/归还(选型理由见模块文档)
    credits: AtomicU64,
    /// 信用上限(归还封顶,防过度归还导致信用膨胀破坏守恒)
    max_credits: u64,
    /// 等待唤醒原语 — 所有等待路径唯一原语(ADR-129:无自旋)
    notify: Notify,
    /// 高优等待累计次数(进入 acquire_with_wait 等待的次数)
    high_wait_total: AtomicU64,
}

impl std::fmt::Debug for CreditFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // WHY 手动实现:Notify 不实现 Debug,仅输出可观测状态
        f.debug_struct("CreditFlow")
            .field("credit_available", &self.credit_available())
            .field("max_credits", &self.max_credits)
            .field("high_wait_total", &self.high_wait_total())
            .finish()
    }
}

impl Default for CreditFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl CreditFlow {
    /// 创建信用流,使用默认信用池(256)
    #[must_use = "构造的信用流需被持有并调用 acquire/release"]
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CREDITS)
    }

    /// 创建信用流,指定初始信用池大小
    ///
    /// WHY 参数化:测试与 T12 分片场景需要不同规模的信用池
    /// (分片后每片按订阅者消费速率独立授信)。
    #[must_use = "构造的信用流需被持有并调用 acquire/release"]
    pub fn with_capacity(initial: u64) -> Self {
        Self {
            credits: AtomicU64::new(initial),
            max_credits: initial,
            notify: Notify::new(),
            high_wait_total: AtomicU64::new(0),
        }
    }

    /// 尝试扣减 n 个信用(非阻塞,CAS 原子,不超发)
    ///
    /// - 有信用:扣减并返回 `true`
    /// - 信用不足:返回 `false`(不扣减、不等待)
    ///
    /// WHY `fetch_update` 而非 load + compare_exchange:
    /// fetch_update 将"读-比较-更新"收敛为单次原子操作,闭包返回 `None`
    /// 表示不更新(信用不足)并退出,无 ABA 问题。
    ///
    /// # 不超发保证
    /// 并发 acquire 时 CAS 保证同一时刻只有一个线程成功扣减最后一个信用,
    /// 信用永不变成负数(见 `test_concurrent_acquire_no_over_issue`)。
    #[must_use = "acquire 结果决定是否持有信用,忽略将导致信用账目错误"]
    pub fn acquire(&self, n: u64) -> bool {
        if n == 0 {
            return true;
        }
        self.credits
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |c| {
                // 信用足够才扣减,否则返回 None(不更新,立即失败)
                // WHY 分支而非 then_some:then_some 参数急切求值,c < n 时
                // `c - n` 在 debug 模式立即下溢 panic(经典 Rust 陷阱)
                if c >= n {
                    Some(c - n)
                } else {
                    None
                }
            })
            .is_ok()
    }

    /// 尝试扣减 n 个信用(非阻塞),失败返回 [`CreditError::Insufficient`]
    ///
    /// 供 [`Priority::Normal`] 语义使用:调用方失败后决定 shed 或走既有
    /// broadcast 兜底(EventBus::publish 即如此)。
    pub fn try_acquire(&self, n: u64) -> Result<(), CreditError> {
        if self.acquire(n) {
            Ok(())
        } else {
            Err(CreditError::Insufficient(n))
        }
    }

    /// 高优窗口等待扣减 — 信用不足时**异步等待**(Notify,无自旋)
    ///
    /// - 快速路径:信用足够立即扣减返回 `Ok(())`;
    /// - 等待路径:挂起至 [`Notify`] 被归还唤醒(release)或超时窗口到期;
    ///   窗口内被唤醒且信用恢复 → 扣减成功返回 `Ok(())`;
    /// - 超时:返回 [`CreditError::Timeout`](携带窗口时长)。
    ///
    /// # ADR-129 无自旋
    /// 等待路径唯一原语是 Notify:等待者经 `tokio::select!` 挂起,
    /// 由归还方的 `notify_waiters` 唤醒,或由 `sleep_until(deadline)` 超时
    /// 唤醒,不存在忙等循环(见 `test_acquire_with_wait_timeout_no_spin`)。
    ///
    /// # 通知不丢失
    /// `Notified::enable()` 复用同一个 Notified future:每次被唤醒后重新
    /// 挂载到等待队列,避免"检查信用失败 → 注册等待之前归还到达"的
    /// 通知丢失窗口(经典 missed-wakeup 竞态,tokio Notify 官方模式)。
    pub async fn acquire_with_wait(&self, n: u64, timeout: Duration) -> Result<(), CreditError> {
        if n == 0 {
            return Ok(());
        }
        // 快速路径:信用充足,零等待
        if self.acquire(n) {
            return Ok(());
        }
        // 进入等待窗口,累计统计(Relaxed:观测指标,非控制流)
        self.high_wait_total.fetch_add(1, Ordering::Relaxed);
        let deadline = tokio::time::Instant::now() + timeout;
        // enable() 要求 future 至少被 poll 过一次;select! 每次循环都会 poll
        let notified = self.notify.notified();
        tokio::pin!(notified);
        loop {
            // 每次唤醒后先重新尝试(可能单次归还不足以满足 n>1 的请求)
            if self.acquire(n) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(CreditError::Timeout(timeout));
            }
            tokio::select! {
                // 归还唤醒:重新挂载等待(enable),回到循环重新尝试
                _ = &mut notified => {
                    notified.as_mut().enable();
                }
                // 窗口到期:超时失败,调用方决定 shed 或降级
                _ = tokio::time::sleep_until(deadline) => {
                    return Err(CreditError::Timeout(timeout));
                }
            }
        }
    }

    /// 分级语义入口 — 按优先级获取信用
    ///
    /// - [`Priority::High`]:异步等待 ≤[`HIGH_PRIORITY_WAIT_WINDOW`](100ms)
    /// - [`Priority::Normal`]:try_acquire 立即失败
    ///
    /// Critical 事件不进入本方法(模块级红线,见 [`Priority`] 文档)。
    pub async fn acquire_priority(&self, priority: Priority, n: u64) -> Result<(), CreditError> {
        match priority {
            Priority::High => self.acquire_with_wait(n, HIGH_PRIORITY_WAIT_WINDOW).await,
            Priority::Normal => self.try_acquire(n),
        }
    }

    /// 批量归还 n 个信用(非阻塞,原子累加)
    ///
    /// 归还封顶到初始信用池(`max_credits`):过度归还(调用方 bug 或重复归还)
    /// 不会导致信用膨胀超过初始值,保持"信用守恒"不变量可被测试守护。
    /// 归还后调用 `notify_waiters` 唤醒所有等待者。
    ///
    /// WHY `notify_waiters` 而非 `notify_one`:高优请求可能一次等待多个
    /// 信用单位(n>1),单次 `notify_one` 唤醒的等待者可能因信用仍不足而
    /// 再次挂起,造成后续归还无人唤醒(丢失唤醒)。`notify_waiters` 唤醒
    /// 全部等待者各重新尝试一次,以归还低频(批提交)换唤醒完备性。
    pub fn release(&self, n: u64) {
        if n == 0 {
            return;
        }
        self.credits
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |c| {
                // 饱和加 + 封顶:任何并发交错下均不超 max_credits
                Some(c.saturating_add(n).min(self.max_credits))
            })
            // 闭包恒返回 Some,fetch_update 恒成功
            .ok();
        self.notify.notify_waiters();
    }

    /// 批量归还接口(批提交,ADR-125 语义)
    ///
    /// ADR-125 批提交语义:订阅者**按消费批次**归还信用(如每消费 100 个
    /// 事件归还 100),而非逐事件归还。WHY:批量归还降低 Notify 唤醒频率
    /// (唤醒是核外切换,高频逐事件归还会放大调度开销),且与广播消费的
    /// 批处理节奏一致。
    ///
    /// 实现与 [`release`](Self::release) 共享原子累加核心,本方法仅作为
    /// 批提交的显式语义入口(文档层面区分逐次归还与批次提交)。
    pub fn release_many(&self, n: u64) {
        self.release(n);
    }

    /// 当前可用信用(观测)
    #[must_use = "观测信用水位,忽略返回值无意义"]
    pub fn credit_available(&self) -> u64 {
        self.credits.load(Ordering::Relaxed)
    }

    /// 高优等待累计次数(观测,进入等待窗口的次数)
    #[must_use = "观测统计,忽略返回值无意义"]
    pub fn high_wait_total(&self) -> u64 {
        self.high_wait_total.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    // ============================================================
    // 基础语义:扣减 / 耗尽 / 归还恢复 / 边界
    // ============================================================

    #[test]
    fn test_acquire_deducts_credit() {
        let cf = CreditFlow::with_capacity(10);
        assert!(cf.acquire(3));
        assert_eq!(cf.credit_available(), 7);
        assert!(cf.acquire(7));
        assert_eq!(cf.credit_available(), 0);
    }

    #[test]
    fn test_acquire_exhausted_returns_false() {
        let cf = CreditFlow::with_capacity(2);
        assert!(cf.acquire(2));
        // 耗尽后 acquire 失败且不扣成负数
        assert!(!cf.acquire(1));
        assert_eq!(cf.credit_available(), 0);
        // try_acquire 返回 Insufficient
        assert_eq!(cf.try_acquire(1), Err(CreditError::Insufficient(1)));
    }

    #[test]
    fn test_acquire_request_exceeding_pool_fails() {
        // 单次请求超过池容量:永远失败(池最多 max_credits)
        let cf = CreditFlow::with_capacity(5);
        assert!(!cf.acquire(6));
        assert_eq!(cf.credit_available(), 5);
    }

    #[test]
    fn test_release_restores_credit() {
        let cf = CreditFlow::with_capacity(10);
        assert!(cf.acquire(6));
        assert_eq!(cf.credit_available(), 4);
        cf.release(6);
        assert_eq!(cf.credit_available(), 10);
        // 归还后可再次获取
        assert!(cf.acquire(10));
    }

    #[test]
    fn test_release_caps_at_max_credits() {
        // 过度归还不膨胀:信用守恒由 max_credits 封顶守护
        let cf = CreditFlow::with_capacity(10);
        cf.release(100);
        assert_eq!(cf.credit_available(), 10);
        cf.release_many(1000);
        assert_eq!(cf.credit_available(), 10);
    }

    #[test]
    fn test_zero_acquire_release_noop() {
        let cf = CreditFlow::with_capacity(3);
        assert!(cf.acquire(0));
        cf.release(0);
        assert_eq!(cf.credit_available(), 3);
    }

    #[test]
    fn test_release_many_batch_commit() {
        // ADR-125 批提交语义:一次归还整个批次
        let cf = CreditFlow::with_capacity(100);
        for _ in 0..100 {
            assert!(cf.acquire(1));
        }
        assert_eq!(cf.credit_available(), 0);
        cf.release_many(100);
        assert_eq!(cf.credit_available(), 100);
    }

    // ============================================================
    // 并发竞争:8 线程 × 1000 次 acquire 不超发
    // ============================================================

    #[test]
    fn test_concurrent_acquire_no_over_issue() {
        // WHY 用多线程测试替代 loom:loom 需 nightly-only 特性,在 Windows
        // GNU 工具链下编译失败(任务约定:编译失败则用 8 线程 × 1000 次
        // acquire 竞争测试替代,报告说明)。8 线程同时竞争 1000 信用,
        // 验证任何交错下"成功总数 + 剩余可用 == 初始容量"(信用守恒,不超发)。
        let cf = Arc::new(CreditFlow::with_capacity(1000));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let cf = Arc::clone(&cf);
            handles.push(std::thread::spawn(move || {
                let mut acquired = 0u64;
                for _ in 0..1000 {
                    if cf.acquire(1) {
                        acquired += 1;
                    }
                }
                acquired
            }));
        }
        let total_acquired: u64 = handles
            .into_iter()
            .map(|h| h.join().expect("并发线程 panic"))
            .sum();
        // 信用守恒:成功扣减总数 + 剩余可用 == 初始容量(任一交错下恒成立)
        assert_eq!(
            total_acquired + cf.credit_available(),
            1000,
            "并发 acquire 不超发:成功 {total_acquired} + 剩余 {} != 1000",
            cf.credit_available()
        );
        assert!(cf.credit_available() <= 1000, "信用不可为负/不可膨胀");
    }

    #[test]
    fn test_concurrent_release_and_acquire_conservation() {
        // 并发归还 + 扣减交错:available 永不超上限
        let cf = Arc::new(CreditFlow::with_capacity(500));
        let mut handles = Vec::new();
        for t in 0..8u64 {
            let cf = Arc::clone(&cf);
            handles.push(std::thread::spawn(move || {
                for _ in 0..500 {
                    if t % 2 == 0 {
                        // 一半线程扣减
                        let _ = cf.acquire(1);
                    } else {
                        // 一半线程归还(封顶由 max_credits 守护)
                        cf.release(1);
                    }
                }
            }));
        }
        for h in handles {
            h.join().expect("并发线程 panic");
        }
        let available = cf.credit_available();
        assert!(available <= 500, "信用永不超上限,实际 {available}");
    }

    // ============================================================
    // 高优窗口等待:Notify 唤醒成功 / 超时失败 / 无自旋
    // ============================================================

    #[tokio::test]
    async fn test_acquire_with_wait_notify_wakeup() {
        // 窗口内被归还唤醒 → 成功
        let cf = Arc::new(CreditFlow::with_capacity(2));
        assert!(cf.acquire(2)); // 耗尽信用
        let cf2 = Arc::clone(&cf);
        let release_task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            cf2.release(2); // 30ms 后批量归还
        });
        let start = Instant::now();
        let result = cf.acquire_with_wait(1, Duration::from_millis(500)).await;
        assert!(result.is_ok(), "窗口内归还应唤醒等待者: {result:?}");
        // 由 release 唤醒而非超时:应远早于 500ms 返回
        assert!(
            start.elapsed() < Duration::from_millis(400),
            "应由 release 唤醒而非超时,实际 {:?}",
            start.elapsed()
        );
        release_task.await.expect("归还任务 panic");
        // 等待者扣走 1,剩余 1
        assert_eq!(cf.credit_available(), 1);
        // 等待统计已累计
        assert_eq!(cf.high_wait_total(), 1);
    }

    #[tokio::test]
    async fn test_acquire_with_wait_timeout_no_spin() {
        // 无自旋验证:等待路径走 Notify,超时由 sleep_until 触发,
        // elapsed 必须接近超时窗口(自旋实现会立即失败或忙占用 CPU)
        let cf = CreditFlow::with_capacity(2);
        assert!(cf.acquire(2)); // 耗尽且无人归还 → 只能超时
        let start = Instant::now();
        let err = cf.acquire_with_wait(1, Duration::from_millis(60)).await;
        assert!(
            matches!(err, Err(CreditError::Timeout(_))),
            "应超时: {err:?}"
        );
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(45),
            "等待路径应挂起至超时窗口(Notify 睡眠),而非自旋立即返回,实际 {elapsed:?}"
        );
        // 超时后信用不变(未扣减)
        assert_eq!(cf.credit_available(), 0);
        assert_eq!(cf.high_wait_total(), 1);
    }

    #[tokio::test]
    async fn test_acquire_with_wait_fast_path_no_wait() {
        // 快速路径:信用充足,零等待直接成功
        let cf = CreditFlow::with_capacity(10);
        let start = Instant::now();
        assert!(cf
            .acquire_with_wait(2, Duration::from_millis(500))
            .await
            .is_ok());
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "快速路径不应等待"
        );
        assert_eq!(cf.credit_available(), 8);
        // 未进入等待窗口,统计为 0
        assert_eq!(cf.high_wait_total(), 0);
    }

    #[tokio::test]
    async fn test_acquire_priority_normal_immediate_fail() {
        // Normal:try_acquire 失败立即返回,不等待
        let cf = CreditFlow::with_capacity(1);
        assert!(cf.acquire(1));
        let start = Instant::now();
        let err = cf.acquire_priority(Priority::Normal, 1).await;
        assert_eq!(err, Err(CreditError::Insufficient(1)));
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "Normal 不应等待"
        );
        assert_eq!(cf.high_wait_total(), 0);
    }

    #[tokio::test]
    async fn test_acquire_priority_high_wait_then_timeout() {
        // High:进入 ≤100ms 默认窗口,无人归还则超时
        let cf = CreditFlow::with_capacity(1);
        assert!(cf.acquire(1));
        let start = Instant::now();
        let err = cf.acquire_priority(Priority::High, 1).await;
        assert!(matches!(err, Err(CreditError::Timeout(_))));
        // 默认窗口 100ms
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(80),
            "High 应等待满默认窗口 100ms,实际 {elapsed:?}"
        );
        assert_eq!(cf.high_wait_total(), 1);
    }

    #[tokio::test]
    async fn test_acquire_priority_high_wakeup_success() {
        // High:窗口内归还唤醒 → 成功
        let cf = Arc::new(CreditFlow::with_capacity(3));
        assert!(cf.acquire(3)); // 耗尽
        let cf2 = Arc::clone(&cf);
        let release_task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cf2.release_many(3); // 批提交归还
        });
        let start = Instant::now();
        assert!(cf.acquire_priority(Priority::High, 1).await.is_ok());
        assert!(
            start.elapsed() < Duration::from_millis(90),
            "High 应由归还唤醒,而非等满窗口,实际 {:?}",
            start.elapsed()
        );
        release_task.await.expect("归还任务 panic");
    }
}
