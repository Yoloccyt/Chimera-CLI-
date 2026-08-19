//! 沙箱拦截率统计 — §16.5 跨层奖励缺口补齐(Phase 10 Wave 6)
//!
//! 对应架构层: **L4 Security**（seccore 子模块）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范` §16.5
//!
//! # 核心职责
//!
//! 真实采集零信任沙箱的请求/拦截计数,提供拦截率查询与周期报告事件:
//! - `record_request`: 每次 `audit_and_execute` 入口调用(总请求数)
//! - `record_blocked`: 任一级防御层拦截时调用(CommandBlocked/EnvVarBlocked/
//!   EscalateToHuman/ASA Block/Parliament 否决)
//! - `interception_rate`: blocked / total(真实可测指标)
//!
//! # 诚实数据原则
//!
//! **误拦截率不实施假采集**:误拦截需要人工标注真值(哪些拦截是"错误"的),
//! 运行时无法判定——与 §16.5 审计"L10 用户满意度采集"同款原则,标注
//! v4.0 预留,不伪造指标。

use std::sync::atomic::{AtomicU64, Ordering};

/// 拦截率统计器 — 无锁原子计数(Relaxed 序,统计指标非控制流信号)
#[derive(Debug, Default)]
pub struct InterceptorStats {
    /// 总请求数(审计并执行入口)
    total_requests: AtomicU64,
    /// 被拦截请求数(任一防御层拦截)
    blocked_requests: AtomicU64,
}

impl InterceptorStats {
    /// 创建空统计器
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一次请求(入口调用)
    pub fn record_request(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次拦截(任一防御层)
    pub fn record_blocked(&self) {
        self.blocked_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// 拦截率 = blocked / total(无请求时为 0.0,防除零)
    pub fn interception_rate(&self) -> f64 {
        let total = self.total_requests.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        self.blocked_requests.load(Ordering::Relaxed) as f64 / total as f64
    }

    /// 计数快照 `(total_requests, blocked_requests)`(可观测性/周期报告)
    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.total_requests.load(Ordering::Relaxed),
            self.blocked_requests.load(Ordering::Relaxed),
        )
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stats_zero_rate() {
        let stats = InterceptorStats::new();
        assert_eq!(stats.interception_rate(), 0.0, "无请求拦截率为 0");
        assert_eq!(stats.snapshot(), (0, 0));
    }

    #[test]
    fn rate_is_blocked_over_total() {
        let stats = InterceptorStats::new();
        stats.record_request();
        stats.record_request();
        stats.record_blocked();
        assert_eq!(stats.snapshot(), (2, 1));
        assert!(
            (stats.interception_rate() - 0.5).abs() < 1e-9,
            "拦截率应为 0.5(实际 {})",
            stats.interception_rate()
        );
    }

    #[test]
    fn monotonic_counters() {
        let stats = InterceptorStats::new();
        for _ in 0..10 {
            stats.record_request();
        }
        for _ in 0..3 {
            stats.record_blocked();
        }
        assert_eq!(stats.snapshot(), (10, 3));
        assert!(
            (stats.interception_rate() - 0.3).abs() < 1e-9,
            "拦截率应为 0.3(实际 {})",
            stats.interception_rate()
        );
    }
}
