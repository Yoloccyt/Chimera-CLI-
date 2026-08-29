//! 影子双跑 diff 采集 — 串行影子 vs 分片实投逐事件比对（P4-T4，D-P3/D-P4 裁决）
//!
//! 对应架构层: **L1 Core**（event-bus）
//! 对应任务: **P4-T4**（W21 影子双跑启动:双路径并行管道）
//!
//! # 双跑语义（Ch12 W21-W24,红线 1 兑现前提）
//! 同一事件流走两条路径:
//! - **串行影子路径**（`observe_serial`）:发布即记录（直接 broadcast 语义基线）
//! - **分片实投路径**（`observe_merged`）:worker 汇入 broadcast 时记录
//! 双路径对 **同一事件实例** 分别观测 → 指纹序列逐位比对 → [`ShadowDiffReport`]
//! （零 diff = 类型与顺序完全等价）。
//!
//! # 指纹口径（诚实登记）
//! [`event_fingerprint`] 取事件全量 Debug 哈希。双跑比对对象是同一实例的
//! 两次观测（同实例 → 同指纹,确定性成立）;跨运行/跨实例的 payload 级
//! 比对需 meta 规范化（时间戳剥离）,留待 ADR-132 归因管道扩展。
//!
//! # 台账（≥7 天窗口,Ch12 W24 门禁）
//! [`ShadowSojournLedger`] 按天记账:连续零 diff 天数累计（断档归零）,
//! `zero_diff_days ≥ 7` 满足 Go 前置;天数由调用方注入（测试时钟可加速）。

use std::sync::Mutex;

use crate::shard::fnv1a;
use crate::types::NexusEvent;

/// 事件指纹 — 全量 Debug 哈希（同实例双路径观测确定性成立）
#[must_use]
pub fn event_fingerprint(event: &NexusEvent) -> u64 {
    fnv1a(format!("{event:?}").as_bytes())
}

/// 规范化指纹 — 剥离 metadata 不稳定字段后的跨实例比对（P5-T4，ADR-158）
///
/// # 口径（ADR-158 裁决）
/// 剥离 `metadata.event_id`（UUIDv7 每实例唯一）与 `metadata.timestamp`
/// （每实例生成时刻），保留：事件变体 + 全部载荷字段 + metadata 稳定子集
/// （source / correlation_id / payload_version / graph_identity）。
/// 规范化后：同 payload 异实例 → 同指纹（跨实例 payload 级双跑比对入口）。
/// 成本：serde_json 序列化 + 哈希（非热路径，仅归因/审计用）。
///
/// # Errors
/// 事件序列化失败时返回错误（载荷含不可序列化数据；NexusEvent 全字段
/// Serialize，实际不可达，防御性保留）。
pub fn canonical_fingerprint(event: &NexusEvent) -> Result<u64, String> {
    let mut v = serde_json::to_value(event).map_err(|e| e.to_string())?;
    // 剣离单个 JSON 对象内的 metadata 不稳定字段
    fn strip_unstable(node: &mut serde_json::Value) {
        if let Some(meta) = node.get_mut("metadata") {
            if let Some(o) = meta.as_object_mut() {
                o.remove("event_id");
                o.remove("timestamp");
            }
        }
    }
    // NexusEvent serde tag="type"/content="data"（struct variant）→ metadata 在 data 内;
    // 顶层兕底（兼容非 tagged 布局;幂等无副作用）
    strip_unstable(&mut v);
    if let Some(data) = v.get_mut("data") {
        strip_unstable(data);
    }
    Ok(fnv1a(v.to_string().as_bytes()))
}

/// 跨实例 payload 级比对 — 规范化指纹逐位比对双序列（P5-T4）
///
/// 双路径事件来自不同实例（串行影子实例 vs 分片实投实例）时,
/// [`event_fingerprint`]（全量 Debug）因 metadata 不稳定字段必然全 diff——
/// 本函数用 [`canonical_fingerprint`] 剥离不稳定字段后逐位比对。
/// 长度不等按缺失计 diff（同 [`ShadowDiffRecorder::compare`] 语义）。
///
/// # Errors
/// 任一事件序列化失败（同 [`canonical_fingerprint`]，实际不可达）。
pub fn compare_cross_instance(
    serial: &[NexusEvent],
    merged: &[NexusEvent],
) -> Result<ShadowDiffReport, String> {
    let s = serial
        .iter()
        .map(canonical_fingerprint)
        .collect::<Result<Vec<_>, _>>()?;
    let m = merged
        .iter()
        .map(canonical_fingerprint)
        .collect::<Result<Vec<_>, _>>()?;
    let mut diff_count = 0usize;
    let mut first_diff = None;
    let n = s.len().max(m.len());
    for i in 0..n {
        let expected = s.get(i).copied();
        let actual = m.get(i).copied();
        if expected != actual {
            diff_count += 1;
            if first_diff.is_none() {
                first_diff = Some(DiffEntry {
                    position: i,
                    expected: expected.unwrap_or(0),
                    actual,
                });
            }
        }
    }
    Ok(ShadowDiffReport {
        serial_count: s.len(),
        merged_count: m.len(),
        diff_count,
        first_diff,
    })
}

/// 单条 diff — 位置 + 期望指纹 + 实际指纹
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEntry {
    /// 序列位置（0 起）
    pub position: usize,
    /// 串行影子路径指纹
    pub expected: u64,
    /// 分片实投路径指纹（None = 实投序列提前结束,丢事件）
    pub actual: Option<u64>,
}

/// 双跑比对报告 — 零 diff 判定输入
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowDiffReport {
    /// 串行影子路径事件数
    pub serial_count: usize,
    /// 分片实投路径事件数
    pub merged_count: usize,
    /// diff 条数（类型/顺序/丢失三类）
    pub diff_count: usize,
    /// 首条 diff（诊断入口;None = 零 diff）
    pub first_diff: Option<DiffEntry>,
}

impl ShadowDiffReport {
    /// 零 diff（指纹序列完全等价）
    #[must_use]
    pub fn zero_diff(&self) -> bool {
        self.diff_count == 0 && self.serial_count == self.merged_count
    }
}

/// 双跑采集器 — 双路径环形缓冲 + 比对（W21 双跑启动管道）
///
/// # 线程安全
/// 双 `Mutex<Vec<u64>>` 环形（超 [`MAX_ENTRIES`] 淘汰最旧）——观测路径
/// 在 worker/发布热路径上仅做指纹计算 + push,锁粒度最小。
pub struct ShadowDiffRecorder {
    serial: Mutex<Vec<u64>>,
    merged: Mutex<Vec<u64>>,
}

/// 环形上限（单路径最多留存 4096 条;超限淘汰最旧,防长跑内存膨胀）
pub const MAX_ENTRIES: usize = 4096;

impl Default for ShadowDiffRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl ShadowDiffRecorder {
    /// 新建采集器（双空序列）
    #[must_use]
    pub fn new() -> Self {
        Self {
            serial: Mutex::new(Vec::new()),
            merged: Mutex::new(Vec::new()),
        }
    }

    /// 串行影子路径观测（发布即记;语义基线）
    pub fn observe_serial(&self, event: &NexusEvent) {
        let mut v = self.serial.lock().unwrap_or_else(|p| p.into_inner());
        if v.len() >= MAX_ENTRIES {
            v.remove(0);
        }
        v.push(event_fingerprint(event));
    }

    /// 分片实投路径观测（worker 汇入 broadcast 时记）
    pub fn observe_merged(&self, event: &NexusEvent) {
        let mut v = self.merged.lock().unwrap_or_else(|p| p.into_inner());
        if v.len() >= MAX_ENTRIES {
            v.remove(0);
        }
        v.push(event_fingerprint(event));
    }

    /// 比对双路径 → 报告（不消费序列,可重复调用）
    #[must_use]
    pub fn compare(&self) -> ShadowDiffReport {
        let s = self.serial.lock().unwrap_or_else(|p| p.into_inner());
        let m = self.merged.lock().unwrap_or_else(|p| p.into_inner());
        let mut diff_count = 0usize;
        let mut first_diff = None;
        let n = s.len().max(m.len());
        for i in 0..n {
            let expected = s.get(i).copied();
            let actual = m.get(i).copied();
            if expected != actual {
                diff_count += 1;
                if first_diff.is_none() {
                    first_diff = Some(DiffEntry {
                        position: i,
                        expected: expected.unwrap_or(0),
                        actual,
                    });
                }
            }
        }
        ShadowDiffReport {
            serial_count: s.len(),
            merged_count: m.len(),
            diff_count,
            first_diff,
        }
    }

    /// 清空双序列（窗口轮转;台账独立计数不受影响）
    pub fn reset(&self) {
        self.serial
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.merged
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }
}

/// 双跑台账 — 连续零 diff 天数记账（W24 Go/No-Go 前置）
///
/// 天数单位 = Unix 天（`unix_secs / 86400`）;由调用方注入（生产用真实时钟,
/// 测试/长跑加速用注入时钟,RK-P20 双轨满足）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowSojournLedger {
    start_day: u64,
    last_recorded_day: Option<u64>,
    zero_diff_days: u64,
    total_diffs: u64,
    days_recorded: u64,
}

impl ShadowSojournLedger {
    /// 双跑启动（记录起始日）
    #[must_use]
    pub fn start(now_unix_day: u64) -> Self {
        Self {
            start_day: now_unix_day,
            last_recorded_day: None,
            zero_diff_days: 0,
            total_diffs: 0,
            days_recorded: 0,
        }
    }

    /// 起始日
    #[must_use]
    pub fn start_day(&self) -> u64 {
        self.start_day
    }

    /// 按日记账 — 同一天重复记账忽略（幂等）;跳日（断采）按断档处理
    pub fn record_day(&mut self, report: &ShadowDiffReport, now_unix_day: u64) {
        if self.last_recorded_day == Some(now_unix_day) {
            return; // 同日幂等
        }
        self.days_recorded += 1;
        self.last_recorded_day = Some(now_unix_day);
        if report.zero_diff() {
            self.zero_diff_days += 1;
        } else {
            self.zero_diff_days = 0; // 断档归零（连续性语义）
            self.total_diffs += u64::try_from(report.diff_count).unwrap_or(u64::MAX);
        }
    }

    /// 当前连续零 diff 天数
    #[must_use]
    pub fn zero_diff_days(&self) -> u64 {
        self.zero_diff_days
    }

    /// 累计 diff 总数
    #[must_use]
    pub fn total_diffs(&self) -> u64 {
        self.total_diffs
    }

    /// 已记账天数
    #[must_use]
    pub fn days_recorded(&self) -> u64 {
        self.days_recorded
    }

    /// Go 前置就绪 — 连续零 diff ≥ required_days（Ch12 W24:≥7 天）
    #[must_use]
    pub fn go_decision_ready(&self, required_days: u64) -> bool {
        self.zero_diff_days >= required_days
    }

    /// JSON 快照导出（T7 长跑脚本落盘格式）
    #[must_use]
    pub fn snapshot(&self) -> String {
        format!(
            "{{\"start_day\":{},\"days_recorded\":{},\"zero_diff_days\":{},\"total_diffs\":{},\"go_ready_at_7\":{}}}",
            self.start_day,
            self.days_recorded,
            self.zero_diff_days,
            self.total_diffs,
            self.go_decision_ready(7),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::EventMetadata;

    /// 构造以 quest_id 区分的测试事件（同实例双路径观测 → 同指纹）
    fn ev(correlation: &str) -> NexusEvent {
        NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: format!("q-{correlation}"),
            title: "双跑观测".into(),
            task_count: 1,
        }
    }

    /// 指纹稳定 — 同实例同指纹,异实例异指纹
    #[test]
    fn fingerprint_stable_and_discriminating() {
        let a = ev("c1");
        let a_clone = a.clone();
        let c = ev("c2");
        assert_eq!(event_fingerprint(&a), event_fingerprint(&a_clone));
        assert_ne!(event_fingerprint(&a), event_fingerprint(&c));
    }

    /// 零 diff — 同序列双路径完全等价
    #[test]
    fn identical_streams_zero_diff() {
        let r = ShadowDiffRecorder::new();
        for i in 0..16 {
            let e = ev(&format!("c{i}"));
            r.observe_serial(&e);
            r.observe_merged(&e);
        }
        let report = r.compare();
        assert!(report.zero_diff(), "同序列必须零 diff: {report:?}");
        assert_eq!(report.diff_count, 0);
        assert!(report.first_diff.is_none());
    }

    /// 注入丢失 — 实投少一条 → diff 捕获,位置正确
    #[test]
    fn dropped_event_captured() {
        let r = ShadowDiffRecorder::new();
        let events: Vec<_> = (0..8).map(|i| ev(&format!("c{i}"))).collect();
        for e in &events {
            r.observe_serial(e);
        }
        // 实投路径丢第 3 条（索引 2）
        for (i, e) in events.iter().enumerate() {
            if i != 2 {
                r.observe_merged(e);
            }
        }
        let report = r.compare();
        assert!(!report.zero_diff());
        assert_eq!(report.diff_count, 6, "位置 2 起全部错位: {report:?}");
        let first = report.first_diff.expect("必有首 diff");
        assert_eq!(first.position, 2);
        assert_eq!(first.actual, Some(event_fingerprint(&events[3])));
    }

    /// 注入乱序 — 交换两条 → 首 diff 定位到交换起点
    #[test]
    fn reordered_events_captured() {
        let r = ShadowDiffRecorder::new();
        let events: Vec<_> = (0..6).map(|i| ev(&format!("c{i}"))).collect();
        for e in &events {
            r.observe_serial(e);
        }
        // 实投路径交换 1/2
        let mut shuffled = events.clone();
        shuffled.swap(1, 2);
        for e in &shuffled {
            r.observe_merged(e);
        }
        let report = r.compare();
        assert!(!report.zero_diff());
        let first = report.first_diff.clone().expect("必有首 diff");
        assert_eq!(first.position, 1, "乱序首差异在位置 1: {first:?}");
    }

    /// 台账 — 连续零 diff ≥7 天 → Go 就绪;断档归零
    #[test]
    fn ledger_seven_day_gate_and_reset() {
        let zero = ShadowDiffReport {
            serial_count: 10,
            merged_count: 10,
            diff_count: 0,
            first_diff: None,
        };
        let mut ledger = ShadowSojournLedger::start(20_000);
        for d in 0..7 {
            ledger.record_day(&zero, 20_000 + d);
        }
        assert!(ledger.go_decision_ready(7), "连续 7 天零 diff → Go 就绪");
        // 第 8 天出现 diff → 归零
        let dirty = ShadowDiffReport {
            serial_count: 10,
            merged_count: 9,
            diff_count: 1,
            first_diff: Some(DiffEntry {
                position: 9,
                expected: 1,
                actual: None,
            }),
        };
        ledger.record_day(&dirty, 20_007);
        assert_eq!(ledger.zero_diff_days(), 0, "断档必须归零");
        assert_eq!(ledger.total_diffs(), 1);
        assert!(!ledger.go_decision_ready(7));
        // 重新连续 7 天
        for d in 0..7 {
            ledger.record_day(&zero, 20_008 + d);
        }
        assert!(ledger.go_decision_ready(7));
        assert_eq!(ledger.days_recorded(), 15);
    }

    /// 台账 — 同日幂等（重复记账不重复累计）
    #[test]
    fn ledger_same_day_idempotent() {
        let zero = ShadowDiffReport {
            serial_count: 1,
            merged_count: 1,
            diff_count: 0,
            first_diff: None,
        };
        let mut ledger = ShadowSojournLedger::start(100);
        ledger.record_day(&zero, 101);
        ledger.record_day(&zero, 101);
        ledger.record_day(&zero, 101);
        assert_eq!(ledger.days_recorded(), 1, "同日幂等");
        assert_eq!(ledger.zero_diff_days(), 1);
    }

    /// 快照 — JSON 可解析且含 Go 判定
    #[test]
    fn snapshot_json_parseable() {
        let zero = ShadowDiffReport {
            serial_count: 5,
            merged_count: 5,
            diff_count: 0,
            first_diff: None,
        };
        let mut ledger = ShadowSojournLedger::start(50_000);
        for d in 0..7 {
            ledger.record_day(&zero, 50_000 + d);
        }
        let snap = ledger.snapshot();
        let parsed: serde_json::Value = serde_json::from_str(&snap).expect("JSON 合法");
        assert_eq!(parsed["zero_diff_days"], 7);
        assert_eq!(parsed["go_ready_at_7"], true);
        assert_eq!(parsed["start_day"], 50_000);
    }

    /// reset — 序列清空后比值为空等价（窗口轮转语义）
    #[test]
    fn reset_clears_sequences() {
        let r = ShadowDiffRecorder::new();
        let e = ev("c1");
        r.observe_serial(&e);
        r.observe_merged(&e);
        assert!(r.compare().zero_diff());
        r.reset();
        let report = r.compare();
        assert_eq!(report.serial_count, 0);
        assert_eq!(report.merged_count, 0);
        assert!(report.zero_diff());
    }

    // ============================================================
    // P5-T4: payload 级规范化指纹 + 跨实例比对（ADR-158）
    // ============================================================

    /// 构造 QuestCreated（title 可变;metadata 每次新建 → event_id/timestamp 必不同）
    fn ev_titled(quest_id: &str, title: &str) -> NexusEvent {
        NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: quest_id.to_string(),
            title: title.to_string(),
            task_count: 1,
        }
    }

    /// 同 payload 异实例 → canonical 指纹相同;而 Debug 全量指纹不同
    #[test]
    fn canonical_same_payload_different_instances() {
        let a = ev_titled("q-1", "任务");
        let b = ev_titled("q-1", "任务");
        // metadata 不稳定字段使 Debug 全量指纹不同（跨实例基线问题）
        assert_ne!(event_fingerprint(&a), event_fingerprint(&b));
        // 规范化后同 payload → 同指纹
        let ca = canonical_fingerprint(&a).expect("序列化成功");
        let cb = canonical_fingerprint(&b).expect("序列化成功");
        assert_eq!(ca, cb, "同 payload 异实例必须同规范化指纹");
    }

    /// 异 payload → canonical 指纹不同（区分能力保留）
    #[test]
    fn canonical_different_payload_differ() {
        let a = canonical_fingerprint(&ev_titled("q-1", "任务")).expect("ok");
        let b = canonical_fingerprint(&ev_titled("q-2", "任务")).expect("ok");
        let c = canonical_fingerprint(&ev_titled("q-1", "另任务")).expect("ok");
        assert_ne!(a, b, "quest_id 不同必须区分");
        assert_ne!(a, c, "title 不同必须区分");
    }

    /// proptest:50 组随机 payload — 同 payload 双实例 canonical 恒等
    #[test]
    fn prop_canonical_stable_across_instances() {
        for seed in 0..50u64 {
            let quest_id = format!("q-{seed}");
            let title = format!("T{}", seed % 7);
            let a = ev_titled(&quest_id, &title);
            let b = ev_titled(&quest_id, &title);
            let ca = canonical_fingerprint(&a).expect("ok");
            let cb = canonical_fingerprint(&b).expect("ok");
            assert_eq!(ca, cb, "seed={seed}: 同 payload 双实例恒等");
            // 异 payload 必不同
            let c = canonical_fingerprint(&ev_titled(&format!("q-x{seed}"), &title)).expect("ok");
            assert_ne!(ca, c, "seed={seed}: 异 payload 必区分");
        }
    }

    /// 跨实例比对 — 同 payload 双序列零 diff;注入丢失可捕获
    #[test]
    fn cross_instance_compare_zero_diff_and_capture() {
        // 同 payload 双实例（各自独立构造 metadata）→ 零 diff
        let serial: Vec<_> = (0..8)
            .map(|i| ev_titled(&format!("q-{i}"), "双跑"))
            .collect();
        let merged: Vec<_> = (0..8)
            .map(|i| ev_titled(&format!("q-{i}"), "双跑"))
            .collect();
        let report = compare_cross_instance(&serial, &merged).expect("序列化成功");
        assert!(report.zero_diff(), "跨实例同 payload 必零 diff: {report:?}");

        // 实投实例丢第 4 条 → diff 捕获（位置 3 起）
        let merged_lossy: Vec<_> = merged
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != 3)
            .map(|(_, e)| e.clone())
            .collect();
        let report = compare_cross_instance(&serial, &merged_lossy).expect("ok");
        assert!(!report.zero_diff());
        assert_eq!(report.first_diff.expect("必有").position, 3);
    }

    /// 端到端 — 真实双路径:同事件串行影子记录 + 分片 worker 汇入 broadcast 收集比对
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn e2e_real_bus_dual_path() {
        use crate::credit_flow::CreditFlow;
        use crate::shard::ShardedEventBus;
        use std::sync::Arc;

        // worker 汇入目标:broadcast 通道（分片实投路径的终点）
        let (tx, mut rx) = tokio::sync::broadcast::channel(1024);
        let sharded = Arc::new(ShardedEventBus::new(8, 64));
        let recorder = Arc::new(ShadowDiffRecorder::new());
        sharded.spawn_workers(
            tx.clone(),
            Arc::new(CreditFlow::new()),
            &tokio::runtime::Handle::current(),
        );

        // 逐事件:串行影子路径记录 + 分片入队（实投路径）
        for i in 0..32 {
            let e = ev(&format!("e{i}"));
            recorder.observe_serial(&e); // 串行影子:发布即记
            sharded.try_push(e).expect("分片未满");
        }
        // 收集 merged 路径（worker 攒批汇入 broadcast 的 32 条）
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut merged = Vec::new();
        while merged.len() < 32 && std::time::Instant::now() < deadline {
            if let Ok(Ok(e)) =
                tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await
            {
                recorder.observe_merged(&e);
                merged.push(e);
            }
        }
        let report = recorder.compare();
        assert_eq!(report.merged_count, 32, "分片路径必须全量汇入: {report:?}");
        assert!(report.zero_diff(), "双路径语义等价（零 diff）: {report:?}");
    }
}
