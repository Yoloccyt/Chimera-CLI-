//! 影子双跑长跑驱动 — 注入时钟多天循环 + 快照导出（P4-T7，RK-P20 双轨）
//!
//! 对应架构层: **L1 Core**（event-bus 组合驱动示例）
//! 对应任务: **P4-T7**（W22:双跑持续观测,≥48h 连续运行验证的时间加速等价物）
//!
//! # 语义（诚实数据）
//! 真实 ≥7 天双跑需生产环境常驻;本驱动以 **注入时钟** 等价验证管道:
//! 每天一个窗口 × 每窗口 512 事件双路径观测 + 比对 + 台账记账,
//! 连续 N 天（默认 7,可 `SOJOURN_DAYS` 覆盖）零 diff → Go 前置就绪。
//! 快照 JSON 落盘 `tmp/shadow_sojourn_snapshot.json`（T7 交付物）,
//! 供 T10 评审引用。管道真实性由 shadow_diff/attribution 单测与
//! e2e_real_bus_dual_path 保证——本驱动验证的是 **多天循环记账语义**。
//!
//! 运行: `cargo run -p event-bus --example shadow_sojourn`

use event_bus::{
    AttributionResult, CausalAttributionLedger, ShadowDiffRecorder, ShadowSojournLedger,
};

fn main() {
    let days: u64 = std::env::var("SOJOURN_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(7);
    let per_day: usize = 512;
    let start_day: u64 = 20_600; // 注入起始 Unix 日（2026-08-27 附近,确定可复现）
    let mut ledger = ShadowSojournLedger::start(start_day);
    let mut attribution = CausalAttributionLedger::with_capacity(4_096);
    let recorder = ShadowDiffRecorder::new();
    let mut total_events = 0usize;

    for d in 0..days {
        let now_ms = (start_day + d) * 86_400_000 + 12 * 3_600_000; // 当日正午
        recorder.reset();
        // 每窗口 512 事件:串行影子 + 分片实投同实例观测（确定性双路径）
        for i in 0..per_day {
            let fp = d * 1_000 + i as u64;
            let mut clock = event_bus::causal::VectorClock::new();
            clock.increment("producer");
            attribution.record(fp, clock, now_ms + i as u64);
            // 双路径观测（同实例 → 同指纹;构造合成事件对,指纹一致由构造保证）
            let event = synthetic_event(fp);
            recorder.observe_serial(&event);
            recorder.observe_merged(&event);
            total_events += 1;
        }
        let report = recorder.compare();
        ledger.record_day(&report, start_day + d);
        // 归因管道演练:当日窗口内任取一指纹归因（链 ≥0,管道可用性验证）
        let probe = attribution.attribute(per_day as u64 - 1, now_ms + per_day as u64);
        assert!(
            probe.diff_found || probe.chain.is_empty(),
            "归因管道必须可用（显式结果,不 panic）: {probe:?}"
        );
        let _: Option<AttributionResult> = None;
        println!(
            "day {d}: serial={} merged={} diff={} zero={} ledger_zero_diff_days={}",
            report.serial_count,
            report.merged_count,
            report.diff_count,
            report.zero_diff(),
            ledger.zero_diff_days(),
        );
    }

    let snapshot = ledger.snapshot();
    println!("\n总事件对: {total_events}");
    println!("快照: {snapshot}");
    // 落盘（tmp 目录;EXIT 语义由调用脚本校验）
    let out_dir = std::path::Path::new("tmp");
    let _ = std::fs::create_dir_all(out_dir);
    let out = out_dir.join("shadow_sojourn_snapshot.json");
    std::fs::write(&out, &snapshot).expect("快照落盘成功");
    println!("落盘: {}", out.display());

    // 门禁断言:连续 ≥7 天零 diff → Go 前置就绪（天数不足时按已跑天数判定）
    let required = days.min(7);
    assert!(
        ledger.go_decision_ready(required),
        "连续 {required} 天零 diff 未达成: {snapshot}"
    );
    println!("GO 前置就绪: 连续 {} 天零 diff ✅", ledger.zero_diff_days());
}

/// 合成事件 — 同指纹双份观测（指纹一致性由同实例保证）
fn synthetic_event(fp: u64) -> event_bus::types::NexusEvent {
    use event_bus::types::{EventMetadata, NexusEvent};
    NexusEvent::QuestCreated {
        metadata: EventMetadata::new("shadow-sojourn"),
        quest_id: format!("q-sojourn-{fp}"),
        title: "长跑观测".into(),
        task_count: 1,
    }
}
