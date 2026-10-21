#![allow(clippy::unwrap_used, clippy::expect_used)] // test-unit idiom; E-5 targets production code
//! `nexus-telemetry` 公共契约测试（ADR-222 D3）
//!
//! 三条分工，各自不可被对方替代：
//! - **表驱动边界**：占用率千分比、阈值解析、去抖判定——走**外部 crate 视角**，
//!   证明这些入口对消费方（`nexus-core` / 装配层）确实可达且语义如文档所述；
//! - **[`reporter_tick`] 的调用契约**：一次 tick 必须恰好各调用一次
//!   `sample_and_report` / `apply_calibration` / `calibration_evidence`，且按此顺序
//!   （顺序有实质意义：写表发生在采样之后，才能让本轮观测影响下一轮阈值）；
//! - **周期任务真在跑**：`spawn_saturation_reporter` 不得是"spawn 了但什么都不做"
//!   的装饰（本仓对零生产者装饰的既有禁令）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use event_bus::EventBus;
use nexus_telemetry::{
    clamped_period, parse_threshold_bp, report_needed, reporter_tick, saturation_ratio_bp,
    spawn_saturation_reporter, CalibrationEvidence, SaturationSource, TickReport,
    DEFAULT_SATURATION_BP,
};

// ---------------------------------------------------------------------------
// 表驱动：占用率千分比
// ---------------------------------------------------------------------------

#[test]
fn ratio_uses_f64_and_never_wraps() {
    assert_eq!(saturation_ratio_bp(1, 2), 500);
    assert_eq!(saturation_ratio_bp(2, 2), 1000);
    // 积压方向可超 1000,正是饱和信号
    assert_eq!(saturation_ratio_bp(3, 2), 1500);
    // threads=0 不 panic(兜底 max(1))
    assert_eq!(saturation_ratio_bp(5, 0), 5000);
    // usize::MAX 钳制到 u32 上限,不回绕
    assert_eq!(saturation_ratio_bp(usize::MAX, 1), u32::MAX);
    // 零 active 恒为零（与 proptests 同一口径的定点样本）
    assert_eq!(saturation_ratio_bp(0, 0), 0);
}

// ---------------------------------------------------------------------------
// 表驱动：阈值解析的 fail-safe 边界
// ---------------------------------------------------------------------------

#[test]
fn threshold_boundaries_and_fail_safe() {
    // 缺失/空白/非数字
    assert_eq!(parse_threshold_bp(None), DEFAULT_SATURATION_BP);
    assert_eq!(parse_threshold_bp(Some("")), DEFAULT_SATURATION_BP);
    assert_eq!(parse_threshold_bp(Some("  ")), DEFAULT_SATURATION_BP);
    assert_eq!(parse_threshold_bp(Some("abc")), DEFAULT_SATURATION_BP);
    // 0 = "永不降级" —— 必须回落默认值（fail-safe 核心用例）
    assert_eq!(parse_threshold_bp(Some("0")), DEFAULT_SATURATION_BP);
    assert_eq!(parse_threshold_bp(Some("99")), DEFAULT_SATURATION_BP);
    // 区间上界之外一律回落：>1000 与"占用比不可能超过它"等价（T2.3 收窄）
    assert_eq!(parse_threshold_bp(Some("1001")), DEFAULT_SATURATION_BP);
    assert_eq!(parse_threshold_bp(Some("1500")), DEFAULT_SATURATION_BP);
    assert_eq!(parse_threshold_bp(Some("10000")), DEFAULT_SATURATION_BP);
    assert_eq!(parse_threshold_bp(Some("10001")), DEFAULT_SATURATION_BP);
    // 合法边界与典型值（含前后空白容错）
    assert_eq!(parse_threshold_bp(Some("100")), 100);
    assert_eq!(parse_threshold_bp(Some("900")), 900);
    assert_eq!(parse_threshold_bp(Some(" 1000 ")), 1_000);
}

// ---------------------------------------------------------------------------
// 表驱动：去抖判定
// ---------------------------------------------------------------------------

#[test]
fn report_needed_debounce_matrix() {
    // 首次超阈（哨兵）
    assert!(report_needed(1_000, 900, 3, u64::MAX));
    // 同一超阈期内计数未变 → 不重复发
    assert!(!report_needed(1_000, 900, 3, 3));
    // 计数变化 → 再发
    assert!(report_needed(1_000, 900, 4, 3));
    // 未达阈值 → 一律不发（即使计数在动）
    assert!(!report_needed(800, 900, 9, u64::MAX));
    // 等号边界：恰好等于阈值算饱和（与 is_saturated 的 >= 语义一致）
    assert!(report_needed(900, 900, 0, u64::MAX));
}

// ---------------------------------------------------------------------------
// 周期钳制：0 不得退化成忙转
// ---------------------------------------------------------------------------

#[test]
fn clamped_period_never_zero() {
    use std::time::Duration;
    assert_eq!(clamped_period(0), Duration::from_secs(1));
    assert_eq!(clamped_period(1), Duration::from_secs(1));
    assert_eq!(clamped_period(30), Duration::from_secs(30));
}

// ---------------------------------------------------------------------------
// reporter_tick 的调用契约
// ---------------------------------------------------------------------------

/// 记录调用次数与顺序的假源；`applied`/`evidence_rows` 由测试指定，
/// 以便断言 [`TickReport`] 是**透传**而非自造数字。
struct MockSource {
    samples: AtomicUsize,
    flushes: AtomicUsize,
    evidences: AtomicUsize,
    order: Mutex<Vec<&'static str>>,
    applied: usize,
    evidence_rows: usize,
}

impl MockSource {
    fn new(applied: usize, evidence_rows: usize) -> Self {
        Self {
            samples: AtomicUsize::new(0),
            flushes: AtomicUsize::new(0),
            evidences: AtomicUsize::new(0),
            order: Mutex::new(Vec::new()),
            applied,
            evidence_rows,
        }
    }

    fn push(&self, tag: &'static str) {
        self.order.lock().unwrap().push(tag);
    }
}

impl SaturationSource for MockSource {
    fn sample_and_report(&self, _bus: &EventBus) {
        self.samples.fetch_add(1, Ordering::SeqCst);
        self.push("sample");
    }

    fn apply_calibration(&self) -> usize {
        self.flushes.fetch_add(1, Ordering::SeqCst);
        self.push("flush");
        self.applied
    }

    fn calibration_evidence(&self) -> Vec<CalibrationEvidence> {
        self.evidences.fetch_add(1, Ordering::SeqCst);
        self.push("evidence");
        (0..self.evidence_rows)
            .map(|i| CalibrationEvidence {
                kind: format!("Kind{i}"),
                inline_samples: i as u64,
                pool_samples: i as u64 + 1,
                decided: "Pending".to_string(),
                candidate_threshold: 0,
                applied: false,
                mode: "Off".to_string(),
            })
            .collect()
    }
}

#[test]
fn reporter_tick_calls_each_stage_once_in_order() {
    let bus = EventBus::with_capacity(16);
    let source = MockSource::new(3, 2);

    let report = reporter_tick(&source, &bus);

    assert_eq!(source.samples.load(Ordering::SeqCst), 1);
    assert_eq!(source.flushes.load(Ordering::SeqCst), 1);
    assert_eq!(source.evidences.load(Ordering::SeqCst), 1);
    // 顺序有实质意义：写表在采样之后，本 tick 的观测才可能影响下一轮阈值
    assert_eq!(
        *source.order.lock().unwrap(),
        vec!["sample", "flush", "evidence"],
        "tick 内三阶段顺序不得被重排"
    );
    // TickReport 是透传，不是自造
    assert_eq!(
        report,
        TickReport {
            applied: 3,
            evidence_rows: 2
        }
    );
}

#[test]
fn reporter_tick_reports_zero_when_calibration_is_off() {
    // 生产默认（calibration off）下 applied 恒 0；本用例锁住"没有写表也照样采样"，
    // 防止将来有人把采样挂在 applied > 0 分支里（那会让默认配置下观测整体失效）。
    let bus = EventBus::with_capacity(16);
    let source = MockSource::new(0, 0);

    let report = reporter_tick(&source, &bus);

    assert_eq!(report.applied, 0);
    assert_eq!(report.evidence_rows, 0);
    assert_eq!(source.samples.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// 周期任务确实驱动 tick（不是装饰性 spawn）
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn spawned_reporter_drives_ticks() {
    use std::sync::Arc;
    use std::time::Duration;

    let bus = EventBus::with_capacity(16);
    let source = Arc::new(MockSource::new(0, 1));
    // 周期最小值 1s（见 clamped_period）；等到 1.3s 只断言"下界 ≥1 次"，
    // 不断言上界——上界会随时钟抖动飘，而下界恰好能抓住"spawn 了但从不 tick"这一类。
    let handle =
        spawn_saturation_reporter(bus, 0, Arc::clone(&source) as Arc<dyn SaturationSource>);
    tokio::time::sleep(Duration::from_millis(1_300)).await;
    let ticks = source.samples.load(Ordering::SeqCst);
    handle.abort();

    assert!(
        ticks >= 1,
        "上报任务未驱动任何 tick（ticks={ticks}）：要么周期钳制失效，要么循环体未被调用"
    );
}
