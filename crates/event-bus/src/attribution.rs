//! 因果归因台账 — diff 事件 5s 窗口因果链回溯（P4-T6，ADR-132 落档配套）
//!
//! 对应架构层: **L1 Core**（event-bus,ADR-033 豁免圈延续）
//! 状态(ADR-200 D1/D2 终裁,2026-10-07):PRODUCTION-WIRED —— AttributionTap 订阅
//!             侧接线(experience_loop 组合根启动);即 ADR-181 决策 2 的到点复核。
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
//!
//! # AttributionTap（ADR-200 D1,2026-10-07 接线）
//! [`spawn_attribution_tap`] —— **订阅侧**记录 + 触发集自动归因:
//! - 先 subscribe 后 spawn（G-79/G-80 红线）,零发布路径改动;
//! - 时钟语义（诚实登记）:单进程 per-source 时钟（`metadata().source` 计数）
//!   = 源内偏序 + 跨源并发;链 = 窗口内同源前驱 + 跨源并发观察者。全局因果
//!   需 L0 事件携时钟字段,登记不做（ADR-200 §3 局限①）;
//! - 触发集（[`ATTRIBUTION_TRIGGER_VARIANTS`]）事件到达时自动调用
//!   [`CausalAttributionLedger::attribute`],因果链写 `tracing::warn!` 诊断;
//!   可选 JSONL 追加导出（[`ATTRIBUTION_DUMP_ENV`] 路径门控,默认关）。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::task::JoinHandle;
use tracing::warn;

use crate::bus::EventBus;
use crate::causal::{CausalRelation, VectorClock};
use crate::error::EventBusError;
use crate::shadow_diff::event_fingerprint;
use crate::types::NexusEvent;

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

/// 归因触发集 — 观测到即自动执行因果链归因的变体名清单（ADR-200 D1）
///
/// 初始定格 = `SlowConsumerDropped` / `BudgetExceeded`（ADR-200 §2 D1 原文）;
/// 扩展须评审（显式常量清单 + 测试锁,不得静默扩表）。
pub const ATTRIBUTION_TRIGGER_VARIANTS: [&str; 2] = ["SlowConsumerDropped", "BudgetExceeded"];

/// JSONL 导出路径环境变量 — 设置则触发集归因时追加导出（默认关）
///
/// 对齐 `CHIMERA_METRICS_DUMP_PATH` 先例:未设 = 零行为;append 语义每行一 JSON
/// （触发集事件稀疏,仅在异常面写盘,不在发布热路径）。
pub const ATTRIBUTION_DUMP_ENV: &str = "CHIMERA_ATTRIBUTION_DUMP_PATH";

/// 判定事件是否属于归因触发集（ADR-200 D1 自动归因消费点）
///
/// 与 [`ATTRIBUTION_TRIGGER_VARIANTS`] 恒等（测试锁双向:常量表 ↔ matches! 臂）。
#[must_use]
pub fn is_attribution_trigger(event: &NexusEvent) -> bool {
    matches!(
        event,
        NexusEvent::SlowConsumerDropped { .. } | NexusEvent::BudgetExceeded { .. }
    )
}

/// tap 共享状态 — 台账 + 诊断计数（组合根句柄与后台任务共同持有）
struct AttributionTapInner {
    /// 实时归因台账（订阅侧记录;锁只做同步临界区,不跨 await）
    ledger: Mutex<CausalAttributionLedger>,
    /// 已记录事件数（诊断）
    recorded: AtomicU64,
    /// recv 侧慢消费者丢弃次数（仅 tap 自身;broadcast 语义保证不向他方传导背压）
    lagged: AtomicU64,
    /// 触发集事件自动归因执行次数（诊断）
    trigger_firings: AtomicU64,
}

/// AttributionTap 句柄 — 只读诊断查询口 + 后台任务生命周期管理
///
/// ADR-200 D1:首个生产消费方 = tap 自身（常驻自动归因 + 诊断导出）;
/// 本句柄供组合根 join 管理,并为后续消费者（错误签名检索链 / parliament
/// 归因 / doctor 诊断）提供**按真实需求**接入的只读查询口（ADR-200 §5 开放项 1）。
pub struct AttributionTapHandle {
    inner: Arc<AttributionTapInner>,
    task: JoinHandle<()>,
}

impl AttributionTapHandle {
    /// 已记录事件数（诊断）
    #[must_use]
    pub fn recorded(&self) -> u64 {
        self.inner.recorded.load(Ordering::Relaxed)
    }

    /// recv 侧慢消费者丢弃次数（诊断）
    #[must_use]
    pub fn lagged(&self) -> u64 {
        self.inner.lagged.load(Ordering::Relaxed)
    }

    /// 触发集事件自动归因执行次数（诊断）
    #[must_use]
    pub fn trigger_firings(&self) -> u64 {
        self.inner.trigger_firings.load(Ordering::Relaxed)
    }

    /// 台账当前记录数（诊断）
    #[must_use]
    pub fn ledger_len(&self) -> usize {
        self.inner
            .ledger
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// 只读归因查询 — 在 tap 的实时台账上回溯 diff 指纹的因果链
    ///
    /// 与 tap 内部触发集自动归因同一台账;`now_unix_ms` 由调用方注入
    /// （生产真实时钟,测试注入时钟,与 [`CausalAttributionLedger::attribute`] 同纪律）。
    #[must_use]
    pub fn attribute(&self, diff_fingerprint: u64, now_unix_ms: u64) -> AttributionResult {
        self.inner
            .ledger
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .attribute(diff_fingerprint, now_unix_ms)
    }

    /// 取回后台任务句柄（组合根 join 管理;与 spawn_* 报告器族同款生命周期纪律）
    #[must_use]
    pub fn into_task(self) -> JoinHandle<()> {
        self.task
    }
}

/// 启动归因 tap — 订阅侧记录 + 触发集自动归因（ADR-200 D1,由组合根调用）
///
/// # 语义
/// 1. 先 [`EventBus::subscribe`] 后 `tokio::spawn`（subscribe-before-spawn 红线）;
/// 2. 每事件 `record(event_fingerprint(ev), per_source_clock(ev), now_ms)`
///    ——**零发布路径改动**（不碰 `bus.rs` publish 面）;
/// 3. 触发集事件（[`is_attribution_trigger`]）到达时在 5s 窗口内自动归因,
///    因果链写 `tracing::warn!` 诊断;设 [`ATTRIBUTION_DUMP_ENV`] 时追加 JSONL
///    （`spawn_blocking` 落盘,不阻塞 tap 循环）;
/// 4. `ChannelClosed` → 任务退出;recv 侧 Lagged 仅计数（诊断）,不中断。
///
/// # 并发纪律
/// - 锁（`Mutex<CausalAttributionLedger>`）只做同步临界区,**不跨 await**;
/// - tap 为独立 broadcast 订阅者,lag 只影响自身（broadcast 不传导背压）。
#[must_use]
pub fn spawn_attribution_tap(bus: &EventBus) -> AttributionTapHandle {
    let mut rx = bus.subscribe();
    let inner = Arc::new(AttributionTapInner {
        ledger: Mutex::new(CausalAttributionLedger::new()),
        recorded: AtomicU64::new(0),
        lagged: AtomicU64::new(0),
        trigger_firings: AtomicU64::new(0),
    });
    let dump_path = std::env::var(ATTRIBUTION_DUMP_ENV)
        .ok()
        .filter(|p| !p.trim().is_empty());
    let inner_for_task = Arc::clone(&inner);
    let task = tokio::spawn(async move {
        let mut source_clocks: HashMap<String, VectorClock> = HashMap::new();
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let fingerprint = event_fingerprint(&event);
                    let source = event.metadata().source.clone();
                    let clock = {
                        let entry = source_clocks.entry(source.clone()).or_default();
                        entry.increment(&source);
                        entry.clone()
                    };
                    let now_ms = unix_ms_now();
                    let trigger = is_attribution_trigger(&event);
                    let mut attributed = None;
                    {
                        // 同步临界区:记录 + 触发集归因（锁不跨 await,红线）
                        let mut ledger = inner_for_task
                            .ledger
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        ledger.record(fingerprint, clock, now_ms);
                        if trigger {
                            attributed = Some(ledger.attribute(fingerprint, now_ms));
                        }
                    }
                    if let Some(result) = attributed {
                        inner_for_task
                            .trigger_firings
                            .fetch_add(1, Ordering::Relaxed);
                        warn!(
                            trigger = event.type_name(),
                            fingerprint,
                            chain_len = result.chain.len(),
                            "归因 tap:触发集事件自动归因（输出 5s 窗口因果链）"
                        );
                        for node in &result.chain {
                            warn!(
                                fingerprint = node.fingerprint,
                                relation = ?node.relation,
                                delta_ms = node.delta_ms,
                                "归因 tap:因果链节点（因|并发观察者）"
                            );
                        }
                        if let Some(path) = &dump_path {
                            let line =
                                render_dump_line(now_ms, event.type_name(), fingerprint, &result);
                            let path = path.clone();
                            // WHY spawn_blocking:文件 append 为阻塞 IO,不得占用 tap
                            // 异步循环（阻塞 IO 不入运行时线程,同 rusqlite 红线纪律）。
                            tokio::task::spawn_blocking(move || {
                                if let Err(e) = append_dump_line(&path, &line) {
                                    warn!(
                                        error = %e,
                                        "归因 tap:JSONL 导出失败（仅诊断面,不影响主链）"
                                    );
                                }
                            });
                        }
                    }
                    inner_for_task.recorded.fetch_add(1, Ordering::Relaxed);
                }
                Err(EventBusError::SlowConsumerDropped { .. }) => {
                    // tap 自身 lag 只计数（诊断）;broadcast 不向他方传导背压
                    inner_for_task.lagged.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => break, // ChannelClosed:总线关闭 → tap 退出
            }
        }
    });
    AttributionTapHandle { inner, task }
}

/// 渲染单条 JSONL 导出记录（纯转换,便于单测锁定字段面）
fn render_dump_line(
    ts_unix_ms: u64,
    trigger: &str,
    fingerprint: u64,
    result: &AttributionResult,
) -> String {
    serde_json::json!({
        "ts_unix_ms": ts_unix_ms,
        "trigger": trigger,
        "fingerprint": fingerprint,
        "chain_len": result.chain.len(),
        "window_ms": result.window_ms,
        "chain": result
            .chain
            .iter()
            .map(|n| serde_json::json!({
                "fingerprint": n.fingerprint,
                "relation": format!("{:?}", n.relation),
                "delta_ms": n.delta_ms,
            }))
            .collect::<Vec<_>>(),
    })
    .to_string()
}

/// 追加单行 JSONL（阻塞 IO;调用方必须在 `spawn_blocking` 内调用）
fn append_dump_line(path: &str, line: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()
}

/// 当前 Unix 毫秒（生产真实时钟;测试注入时钟经 `attribute` 直调,双轨同纪律）
fn unix_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
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

    /// 触发集常量锁 — 常量表 ↔ matches! 臂 ↔ type_name 三方恒等（ADR-200 D1;扩表须经评审）
    #[test]
    fn trigger_variants_locked() {
        use crate::types::EventMetadata;
        assert_eq!(
            ATTRIBUTION_TRIGGER_VARIANTS,
            ["SlowConsumerDropped", "BudgetExceeded"],
            "触发集定格;扩展须评审（ADR-200 D1）"
        );
        let dropped = NexusEvent::SlowConsumerDropped {
            metadata: EventMetadata::new("src"),
            subscriber_id: "s-1".into(),
            lag: 1,
            dropped_count: 1,
        };
        let budget = NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("src"),
            budget_type: "token".into(),
            current: 2,
            limit: 1,
        };
        assert!(is_attribution_trigger(&dropped));
        assert!(is_attribution_trigger(&budget));
        assert_eq!(dropped.type_name(), ATTRIBUTION_TRIGGER_VARIANTS[0]);
        assert_eq!(budget.type_name(), ATTRIBUTION_TRIGGER_VARIANTS[1]);
        // 负对照:非触发集事件不得进入（红线:不制造高频归因）
        let other = NexusEvent::QuestCreated {
            metadata: EventMetadata::new("src"),
            quest_id: "q-1".into(),
            title: "t".into(),
            task_count: 1,
        };
        assert!(!is_attribution_trigger(&other));
    }

    /// JSONL 追加 — append 语义 + 渲染字段面锁定（render + append 往返）
    #[test]
    fn dump_line_append_roundtrip() {
        let path = std::env::temp_dir().join(format!(
            "chimera-attribution-dump-test-{}.jsonl",
            std::process::id()
        ));
        let path_s = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);
        let result = AttributionResult {
            chain: vec![AttributionNode {
                fingerprint: 42,
                relation: CausalRelation::Before,
                delta_ms: -3,
            }],
            diff_found: true,
            window_ms: ATTRIBUTION_WINDOW_MS,
        };
        let line = render_dump_line(1_700_000_000_000, "SlowConsumerDropped", 7, &result);
        append_dump_line(&path_s, &line).expect("append #1");
        append_dump_line(&path_s, &line).expect("append #2");
        let text = std::fs::read_to_string(&path).expect("read back");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "append 语义:两次追加各一行");
        assert_eq!(lines[0], lines[1], "同输入同输出（纯函数渲染）");
        let parsed: serde_json::Value = serde_json::from_str(lines[0]).expect("JSONL 可解析");
        assert_eq!(parsed["ts_unix_ms"], 1_700_000_000_000u64);
        assert_eq!(parsed["trigger"], "SlowConsumerDropped");
        assert_eq!(parsed["fingerprint"], 7);
        assert_eq!(parsed["chain_len"], 1);
        assert_eq!(parsed["chain"][0]["fingerprint"], 42);
        assert_eq!(parsed["chain"][0]["relation"], "Before");
        assert_eq!(parsed["chain"][0]["delta_ms"], -3);
        let _ = std::fs::remove_file(&path);
    }
}
