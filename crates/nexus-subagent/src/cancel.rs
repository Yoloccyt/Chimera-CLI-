//! 取消令牌 — 四因取消传播（P3-T9，v4.0 WI-25）
//!
//! 对应架构层: L7 Execution（nexus-subagent，ADR-148）
//!
//! 取消经 CancellationToken 四因传播（WI-25）:
//! 用户取消 / 超时 / 配额耗尽 / 父级撤销。令牌为 `Arc` 共享,
//! `cancel()` 原子置位 + Notify 唤醒等待者（无自旋——Notify 延续红线）。

use std::sync::atomic::{AtomicU8, Ordering};

use tokio::sync::Notify;

/// 取消原因 — 四因（WI-25）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    /// 用户取消
    UserCancelled,
    /// 超时
    Timeout,
    /// 配额耗尽
    QuotaExhausted,
    /// 父级撤销
    ParentRevoked,
}

impl CancelReason {
    /// 诊断文案
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            CancelReason::UserCancelled => "user_cancelled",
            CancelReason::Timeout => "timeout",
            CancelReason::QuotaExhausted => "quota_exhausted",
            CancelReason::ParentRevoked => "parent_revoked",
        }
    }
}

/// 取消令牌 — 原子置位 + Notify 唤醒
#[derive(Debug)]
pub struct CancellationToken {
    /// 状态:0 = 活跃,1 = 已取消（原因见 reason）
    cancelled: AtomicU8,
    /// 取消原因（cancelled=1 时有效）
    reason: AtomicU8,
    /// 取消唤醒（无自旋）
    notify: Notify,
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancellationToken {
    /// 新建活跃令牌
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancelled: AtomicU8::new(0),
            reason: AtomicU8::new(0),
            notify: Notify::new(),
        }
    }

    /// 是否已取消
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire) == 1
    }

    /// 当前取消原因（未取消 = None）
    #[must_use]
    pub fn reason(&self) -> Option<CancelReason> {
        if !self.is_cancelled() {
            return None;
        }
        Some(match self.reason.load(Ordering::Acquire) {
            1 => CancelReason::UserCancelled,
            2 => CancelReason::Timeout,
            3 => CancelReason::QuotaExhausted,
            _ => CancelReason::ParentRevoked,
        })
    }

    /// 发起取消（幂等:首因生效,后续忽略）
    pub fn cancel(&self, reason: CancelReason) {
        // 先占先得:唯一成功者 fetch 到旧值 0
        if self.cancelled.swap(1, Ordering::AcqRel) == 1 {
            return; // 已取消（幂等）
        }
        let code = match reason {
            CancelReason::UserCancelled => 1,
            CancelReason::Timeout => 2,
            CancelReason::QuotaExhausted => 3,
            CancelReason::ParentRevoked => 4,
        };
        self.reason.store(code, Ordering::Release);
        self.notify.notify_waiters();
    }

    /// 等待取消（无自旋;已取消立即返回原因）
    pub async fn cancelled(&self) -> CancelReason {
        loop {
            if let Some(r) = self.reason() {
                return r;
            }
            self.notify.notified().await;
        }
    }

    /// 取消检查 + 原因（轮询语义,供执行循环定期检查）
    #[must_use]
    pub fn poll(&self) -> Option<CancelReason> {
        self.reason()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use super::*;

    /// 四因取消 — 首因生效,幂等
    #[test]
    fn four_reasons_first_wins() {
        for reason in [
            CancelReason::UserCancelled,
            CancelReason::Timeout,
            CancelReason::QuotaExhausted,
            CancelReason::ParentRevoked,
        ] {
            let t = CancellationToken::new();
            assert!(!t.is_cancelled());
            t.cancel(reason);
            assert!(t.is_cancelled());
            assert_eq!(t.reason(), Some(reason));
            // 幂等:二次取消不改原因
            t.cancel(CancelReason::UserCancelled);
            assert_eq!(t.reason(), Some(reason), "首因必须保留");
        }
    }

    /// 并发取消 — 恰一原因生效
    #[test]
    fn concurrent_cancel_single_reason() {
        let t = Arc::new(CancellationToken::new());
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let t = Arc::clone(&t);
                std::thread::spawn(move || {
                    let reason = if i % 2 == 0 { CancelReason::Timeout } else { CancelReason::ParentRevoked };
                    t.cancel(reason);
                })
            })
            .collect();
        for h in handles {
            h.join().expect("线程正常");
        }
        assert!(t.is_cancelled());
        // 原因必为四因之一（首因竞态不确定,但必有一个）
        assert!(t.reason().is_some());
    }

    /// 异步等待 — cancel 唤醒等待者
    #[tokio::test]
    async fn cancelled_await_wakes() {
        let t = Arc::new(CancellationToken::new());
        let t2 = Arc::clone(&t);
        let waiter = tokio::spawn(async move { t2.cancelled().await });
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        t.cancel(CancelReason::UserCancelled);
        let reason = waiter.await.expect("等待者必须唤醒");
        assert_eq!(reason, CancelReason::UserCancelled);
        // 已取消:立即返回
        let again = t.cancelled().await;
        assert_eq!(again, CancelReason::UserCancelled);
    }

    /// 轮询 — 活跃时 None,取消后有原因
    #[tokio::test]
    async fn poll_semantics() {
        let t = CancellationToken::new();
        assert_eq!(t.poll(), None);
        t.cancel(CancelReason::Timeout);
        assert_eq!(t.poll(), Some(CancelReason::Timeout));
    }
}
