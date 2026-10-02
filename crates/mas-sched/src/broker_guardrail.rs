//! Broker 频率护栏 — 认领制接线的「防轮询风暴」硬上限（v2.3 §(b.4)(2) / §(f) 新红线）
//!
//! 对应架构层：L9 Quest（mas-sched）
//! 对应设计源：`Chimera架构重构与优化方案_v2.3` §(b.4)(2)「broker 护栏（与接线同批）」
//!
//! # WHY
//! v2.3 新红线：现状派单制**无轮询病灶**，但「认领制接线与后续改造**不得引入高频轮询**」。
//! 决策#1 把认领制降为 feature-gated 试点，护栏须与接线同批落地。本模块提供可注入时钟
//! （`now_ms`）的**定窗计数硬上限**：单 agent 在一个窗口内认领数超上限即拒绝，杜绝轮询风暴；
//! 与背压滞回同思想但落在任务认领面。纯逻辑、无 IO、无 `Instant` 忙等（§红线：无自旋）。

use std::collections::HashMap;

/// 单个 agent 的当前窗口计数。
#[derive(Debug, Clone, Copy)]
struct Window {
    idx: u64,
    count: u32,
}

/// 认领频率护栏：每 `window_ms` 毫秒窗口内，单 agent 至多 `max_claims_per_window` 次认领。
#[derive(Debug, Clone)]
pub struct ClaimRateGovernor {
    max_claims_per_window: u32,
    window_ms: u64,
    state: HashMap<String, Window>,
}

impl ClaimRateGovernor {
    /// 新建护栏。`window_ms` 至少为 1（避免除零）。
    #[must_use]
    pub fn new(max_claims_per_window: u32, window_ms: u64) -> Self {
        Self {
            max_claims_per_window,
            window_ms: window_ms.max(1),
            state: HashMap::new(),
        }
    }

    /// 某 agent 在 `now_ms` 尝试认领一次：返回 `true`=放行，`false`=超频拒绝（防轮询风暴）。
    ///
    /// 时间由调用方注入（不内部读时钟），故可确定性测试且无忙等。
    #[must_use]
    pub fn try_admit(&mut self, agent: &str, now_ms: u64) -> bool {
        let idx = now_ms / self.window_ms;
        let e = self
            .state
            .entry(agent.to_string())
            .or_insert(Window { idx, count: 0 });
        if e.idx != idx {
            e.idx = idx;
            e.count = 0;
        }
        if e.count >= self.max_claims_per_window {
            return false;
        }
        e.count += 1;
        true
    }

    /// 当前被跟踪的 agent 数（可观测性/测试）。
    #[must_use]
    pub fn tracked_agents(&self) -> usize {
        self.state.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admits_up_to_cap_then_rejects() {
        let mut g = ClaimRateGovernor::new(3, 1000);
        // 同一窗口(0..1000ms)内前 3 次放行,第 4 次拒绝
        assert!(g.try_admit("a", 0));
        assert!(g.try_admit("a", 100));
        assert!(g.try_admit("a", 200));
        assert!(!g.try_admit("a", 300), "超上限应拒绝(防轮询风暴)");
    }

    #[test]
    fn new_window_resets_counter() {
        let mut g = ClaimRateGovernor::new(2, 1000);
        assert!(g.try_admit("a", 0));
        assert!(g.try_admit("a", 500));
        assert!(!g.try_admit("a", 900));
        // 进入下一窗口(idx 不同)→ 计数重置
        assert!(g.try_admit("a", 1000));
        assert!(g.try_admit("a", 1500));
        assert!(!g.try_admit("a", 1900));
    }

    #[test]
    fn per_agent_isolation() {
        let mut g = ClaimRateGovernor::new(1, 1000);
        assert!(g.try_admit("a", 0));
        assert!(!g.try_admit("a", 0));
        assert!(g.try_admit("b", 0), "b 有独立配额");
        assert_eq!(g.tracked_agents(), 2);
    }

    #[test]
    fn zero_window_ms_is_clamped_not_panicking() {
        let mut g = ClaimRateGovernor::new(1, 0); // window_ms→1
        assert!(g.try_admit("a", 0));
        assert!(g.try_admit("a", 5)); // 新窗口
    }
}
