//! 因果归因台账 — diff 事件 5s 窗口因果链回溯（P4-T6，ADR-132 落档配套）
//!
//! 对应架构层: **L1 Core**（event-bus,ADR-033 豁免圈延续）
//! 状态(ADR-181):EXPERIMENTAL-UNWIRED —— 零生产消费方;接线路径与
//!             退役条件见 ADR-181 决策 2(遥测批次核对)。
//! 对应任务: **P4-T6**（W22:ADR-132 CausalGraph 归因管道）
//!
//! # 设计（ADR-132 核心决策）
//! - **基座复用**:因果判定直接使用 [`VectorClock`](crate::causal::VectorClock)
//!   （Lamport 时空,P2-W7.2.1,71 测试基线）——不重造偏序算法;
//! - **归因语义**:给定 diff 事件,在 `[now - WINDOW, now]` 时间窗内收集
//!   「因果前驱」（`happens_before(diff)`）与「并发」（`concurrent_with(diff)`）
//!   事件,按时钟序输出因果链——双跑 diff 的归因入口（Ch12 W23:任何 diff
//!   可在 5s 窗口内归因到事件链）;
//! - **环形台账**:容量上限环形缓冲（默认 4096 条）,长跑不膨胀;
//! - **时钟注入**:`now_unix_ms` 由调用方传入（生产真实时钟,测试注入时钟,
//!   RK-P20 双轨同款纪律）。
//!
//! # 降级路径
//! 归因 P95 > 5s 或链路为空 → 人工归因兜底（Ch12 原文;见 closure 报告）。

use std::collections::VecDeque;

use crate::causal::{CausalRelation, VectorClock};

/// 归因时间窗（毫秒;Ch12 W23 原文:5s 窗口）
pub const ATTRIBUTION_WINDOW_MS: u64 = 5_000;

/// 台账默认容量（环形上限;与 shadow_diff MAX_ENTRIES 同量级）
pub const DEFAULT_ATTRIBUTION_CAPACITY: usize = 4_096;

/// 单条归因记录 — 指纹 + 因果时钟 + 墙钟时间
#[derive(Debug, Clone)]
struct AttributionRecord {
    /// 事件指纹（shadow_diff::event_fingerprint 输出）
    fingerprint: u64,
    /// 事件因果时钟（发布方随事件记录）
    clock: VectorClock,
    /// 记录时刻（Unix 毫秒）
    at_unix_ms: u64,
}

/// 因果链节点 — 归因结果条目
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributionNode {
    /// 事件指纹
    pub fingerprint: u64,
    /// 与 diff 事件的因果关系（Before = 因,Concurrent = 并发观察者）
    pub relation: CausalRelation,
    /// 距 diff 事件的记录时差（毫秒;负 = 先于 diff）
    pub delta_ms: i64,
}

/// 归因结果 — 因果链 + 窗口判定
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributionResult {
    /// 因果链（时钟序;不含 diff 自身）
    pub chain: Vec<AttributionNode>,
    /// diff 事件是否在台账中（false = diff 未观测,链必空）
    pub diff_found: bool,
    /// 窗口大小（恒 [`ATTRIBUTION_WINDOW_MS`],自描述）
    pub window_ms: u64,
}

/// 因果归因台账 — 双跑 diff 的归因入口（W22-W24 主链）
#[derive(Debug)]
pub struct CausalAttributionLedger {
    records: VecDeque<AttributionRecord>,
    capacity: usize,
}

impl Default for CausalAttributionLedger {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_ATTRIBUTION_CAPACITY)
    }
}

impl CausalAttributionLedger {
    /// 新建（默认容量）
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 指定容量（环形上限;超限淘汰最旧）
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            records: VecDeque::with_capacity(capacity.max(1)),
            capacity: capacity.max(1),
        }
    }

    /// 记录事件 — 指纹 + 因果时钟 + 时刻（发布路径挂载点）
    pub fn record(&mut self, fingerprint: u64, clock: VectorClock, at_unix_ms: u64) {
        if self.records.len() >= self.capacity {
            self.records.pop_front();
        }
        self.records.push_back(AttributionRecord {
            fingerprint,
            clock,
            at_unix_ms,
        });
    }

    /// 当前记录数（诊断）
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// 是否为空
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// 归因 — diff 指纹在 5s 窗口内的因果链回溯
    ///
    /// # 语义
    /// 1. 定位 **窗口内最后一次** 同指纹记录（diff 本体;同指纹重复以最近为准）
    /// 2. 收集窗口内其余记录,关系 = `compare(diff.clock, r.clock)`:
    ///    - `Before`（r 因于 diff）→ 归入链
    ///    - `Concurrent` → 归入链（并发观察者,归因价值同因）
    ///    - `After` / `Equal` → 排除（果/自身）
    /// 3. 按 `delta_ms` 升序输出（时间因果直观序）
    #[must_use]
    pub fn attribute(&self, diff_fingerprint: u64, now_unix_ms: u64) -> AttributionResult {
        let window_start = now_unix_ms.saturating_sub(ATTRIBUTION_WINDOW_MS);
        // 定位窗口内最后一次 diff 记录
        let diff = self
            .records
            .iter()
            .rev()
            .find(|r| r.fingerprint == diff_fingerprint && r.at_unix_ms >= window_start);
        let Some(diff_rec) = diff else {
            return AttributionResult {
                chain: Vec::new(),
                diff_found: false,
                window_ms: ATTRIBUTION_WINDOW_MS,
            };
        };
        let mut chain: Vec<AttributionNode> = self
            .records
            .iter()
            .filter(|r| {
                r.fingerprint != diff_fingerprint // 排除自身（Equal 语义兜底）
                    && r.at_unix_ms >= window_start
                    && r.at_unix_ms <= now_unix_ms
            })
            .filter_map(|r| {
                // 关系 = 候选相对 diff:候选.compare(diff) = Before 即候选为因
                let relation = r.clock.compare(&diff_rec.clock);
                let in_chain = matches!(
                    relation,
                    CausalRelation::Before | CausalRelation::Concurrent
                );
                in_chain.then(|| AttributionNode {
                    fingerprint: r.fingerprint,
                    relation,
                    delta_ms: r.at_unix_ms as i64 - diff_rec.at_unix_ms as i64,
                })
            })
            .collect();
        chain.sort_by_key(|n| n.delta_ms);
        AttributionResult {
            chain,
            diff_found: true,
            window_ms: ATTRIBUTION_WINDOW_MS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造节点时钟:node 计数 +1
    fn tick(node: &str, n: u64) -> VectorClock {
        let mut c = VectorClock::new();
        for _ in 0..n {
            c.increment(node);
        }
        c
    }

    /// 接收时钟:self_node 接收 other（合并 + 自增）
    fn recv(self_node: &str, other: &VectorClock) -> VectorClock {
        let mut c = VectorClock::new();
        c.receive(other, self_node);
        c
    }

    /// 合成链路归因 — A → B → C:attribute(C) 返回 [A, B]（因果链完整）
    #[test]
    fn synthetic_chain_attribution() {
        let mut ledger = CausalAttributionLedger::new();
        let now = 1_000_000u64;
        // A 发布;B 接收 A 后发布;C 接收 B 后发布
        let a = tick("node-a", 1);
        let b = recv("node-b", &a);
        let c = recv("node-c", &b);
        ledger.record(100, a, now);
        ledger.record(200, b, now + 10);
        ledger.record(300, c, now + 20);
        let result = ledger.attribute(300, now + 25);
        assert!(result.diff_found);
        assert_eq!(result.window_ms, 5_000);
        assert_eq!(result.chain.len(), 2, "C 的因果前驱 = A, B: {result:?}");
        assert_eq!(result.chain[0].fingerprint, 100, "A 最先（时钟序）");
        assert_eq!(result.chain[0].relation, CausalRelation::Before);
        assert_eq!(result.chain[1].fingerprint, 200);
        assert_eq!(result.chain[1].relation, CausalRelation::Before);
    }

    /// 并发事件入链 — 与 diff 并发的观察者同样归因
    #[test]
    fn concurrent_included() {
        let mut ledger = CausalAttributionLedger::new();
        let now = 2_000_000u64;
        let x = tick("node-x", 3);
        let y = tick("node-y", 2); // 与 X 无交换 → 并发
        ledger.record(10, x, now);
        ledger.record(20, y, now + 5);
        let result = ledger.attribute(10, now + 10);
        assert!(result.diff_found);
        assert_eq!(result.chain.len(), 1);
        assert_eq!(result.chain[0].relation, CausalRelation::Concurrent);
    }

    /// 窗口外排除 — 前驱在 now-6s → 不在 5s 窗口链中
    #[test]
    fn outside_window_excluded() {
        let mut ledger = CausalAttributionLedger::new();
        let now = 3_000_000u64;
        let a = tick("a", 1);
        let c = recv("c", &a);
        ledger.record(1, a, now - 6_000); // 窗口外（5s 窗）
        ledger.record(3, c, now);
        let result = ledger.attribute(3, now);
        assert!(result.diff_found);
        assert!(result.chain.is_empty(), "窗口外前驱不入链: {result:?}");
    }

    /// diff 不存在 → diff_found=false,链空（人工归因兜底入口）
    #[test]
    fn missing_diff_reports_not_found() {
        let ledger = CausalAttributionLedger::new();
        let result = ledger.attribute(99, 1_000);
        assert!(!result.diff_found);
        assert!(result.chain.is_empty());
        assert_eq!(result.window_ms, 5_000);
    }

    /// 果不入链 — After 关系（diff 的下游）排除
    #[test]
    fn effect_excluded_from_chain() {
        let mut ledger = CausalAttributionLedger::new();
        let now = 4_000_000u64;
        let diff = tick("d", 1);
        let downstream = recv("e", &diff);
        ledger.record(50, diff, now);
        ledger.record(60, downstream, now + 5);
        let result = ledger.attribute(50, now + 10);
        assert!(result.diff_found);
        assert!(result.chain.is_empty(), "下游（果）不入因链: {result:?}");
    }

    /// 环形容量 — 超限淘汰最旧（长跑不膨胀）
    #[test]
    fn ring_capacity_evicts_oldest() {
        let mut ledger = CausalAttributionLedger::with_capacity(4);
        for i in 0..8u64 {
            ledger.record(i, tick("n", i + 1), 1_000 + i);
        }
        assert_eq!(ledger.len(), 4, "容量 4 只留最新 4 条");
        assert!(!ledger.attribute(0, 2_000).diff_found, "最旧已淘汰");
        assert!(ledger.attribute(7, 2_000).diff_found, "最新在册");
    }

    /// 同指纹重复 — 以窗口内最近一次为准
    #[test]
    fn duplicate_fingerprint_last_wins() {
        let mut ledger = CausalAttributionLedger::new();
        let now = 5_000_000u64;
        let c1 = tick("n1", 1);
        let c2 = tick("n2", 5);
        ledger.record(77, c1, now);
        ledger.record(77, c2, now + 100);
        let a = tick("x", 1);
        ledger.record(88, a, now + 50);
        let result = ledger.attribute(77, now + 150);
        assert!(result.diff_found);
        // 最近一次（c2,n2=5）与 a（n=1）并发 → 入链
        assert_eq!(result.chain.len(), 1);
        assert_eq!(result.chain[0].relation, CausalRelation::Concurrent);
    }

    /// proptest:因果链单调性 — 若 X happens_before(diff) 且都在窗口,归因必含 X
    #[test]
    fn prop_chain_monotonicity() {
        // 合成 50 组随机链:随机前驱计数 + 确定性接收 → 归因包含全部真前驱
        for seed in 0..50u64 {
            let mut ledger = CausalAttributionLedger::new();
            let now = 10_000_000u64 + seed;
            let pre_n = seed % 5 + 1;
            let pre = tick("pre", pre_n);
            let mid = recv("mid", &pre);
            let diff = recv("diff", &mid);
            ledger.record(1, pre, now - 100);
            ledger.record(2, mid, now - 50);
            ledger.record(3, diff, now);
            let result = ledger.attribute(3, now);
            assert!(result.diff_found, "seed={seed}");
            let fps: Vec<_> = result.chain.iter().map(|n| n.fingerprint).collect();
            assert!(fps.contains(&1), "seed={seed}: 真前驱 pre 必在链: {fps:?}");
            assert!(fps.contains(&2), "seed={seed}: 真前驱 mid 必在链: {fps:?}");
            assert_eq!(result.chain.len(), 2, "seed={seed}: 无多余节点");
        }
    }

    /// 时序直观 — delta_ms 升序（链输出排序保证）
    #[test]
    fn chain_sorted_by_delta() {
        let mut ledger = CausalAttributionLedger::new();
        let now = 20_000_000u64;
        let p1 = tick("p1", 1);
        let p2 = recv("p2", &p1);
        let diff = recv("d", &p2);
        ledger.record(11, p1, now - 200);
        ledger.record(12, p2, now - 100);
        ledger.record(13, diff, now);
        let result = ledger.attribute(13, now);
        let deltas: Vec<i64> = result.chain.iter().map(|n| n.delta_ms).collect();
        assert_eq!(deltas, vec![-200, -100], "delta 升序: {deltas:?}");
    }
}
