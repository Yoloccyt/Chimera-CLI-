//! 占用率判定与周期上报循环（ADR-197 / ADR-222 D3）
//!
//! # 为什么把循环体抽成 [`reporter_tick`]
//!
//! 循环本身只做两件事：等到点、调用一次 tick。把「一次 tick 该做什么」抽成
//! 同步函数后，上报路径可被**确定性地**测试（调用次数、参数透传、返回值），
//! 不需要「起任务 + sleep 真实时间 + 断言日志」这种在慢机上必飘的形态——与
//! [`report_needed`] 因同一理由被抽成纯函数是同一条纪律。

use std::sync::Arc;
use std::time::Duration;

use event_bus::EventBus;

use crate::source::SaturationSource;

/// 占用率千分比 —— `active / threads × 1000`,以 `f64` 作中间值（u64 大数百分比红线）
///
/// # 契约
/// - `threads` 取 `max(1)` 兜底:池构造期已有 `MIN_THREADS = 2` 下限,此处只为让
///   纯函数在测试中可对任意输入求值,不掩盖运行期异常。
/// - 返回 `u32`,可超过 1000（active 大于线程数即排队积压,正是我们关心的信号）。
#[must_use]
pub fn saturation_ratio_bp(active: usize, threads: usize) -> u32 {
    let threads = threads.max(1) as f64;
    let ratio_bp = (active as f64 / threads) * 1_000.0;
    // 饱和只关心"大于等于阈值"方向,溢出 u32 的极端值钳到上限即可,不引入回绕
    ratio_bp.clamp(0.0, u32::MAX as f64) as u32
}

/// 上报判定（纯函数）—— 饱和且降级计数有变化才发事件
///
/// WHY 抽成函数而不是内联在循环里:去抖逻辑是本模块唯一有"状态错乱风险"的
/// 地方（漏报 = 不可观测，风暴 = 刷屏），必须能被确定性地单测覆盖，
/// 而不是靠真池占用 + 定时等待（慢机上必飘）。
///
/// # 参数
/// - `ratio_bp` / `threshold_bp`:当前占用与生效阈值（千分比）；
/// - `degraded`:当前累计降级数（L1 真值源）；
/// - `previous`:上次已上报的降级数；`u64::MAX` 表示尚未报过（复位哨兵）。
#[must_use]
pub fn report_needed(ratio_bp: u32, threshold_bp: u32, degraded: u64, previous: u64) -> bool {
    if ratio_bp < threshold_bp {
        return false;
    }
    // 首次超阈（哨兵）或计数变化 → 上报；同一超阈期内计数未变 → 静默
    previous == u64::MAX || previous != degraded
}

/// 采样周期 —— `0` 按 `1` 处理（避免 `sleep(0)` 退化成忙转）
///
/// WHY 独立成函数：`max(1)` 是本 crate 唯一一条"输入被静默改写"的规则，
/// 藏在 `Duration::from_secs(period_secs.max(1))` 里就没有可断言的接口；
/// 抽出来后可直接断言 0→1s，而不必起任务观察它有没有占满 CPU。
#[must_use]
pub fn clamped_period(period_secs: u64) -> Duration {
    Duration::from_secs(period_secs.max(1))
}

/// 一次 tick 的结果（供调用方断言与日志，不改变行为）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TickReport {
    /// SPRT 校准本轮写入 HTS 阈值表的行数（非 apply 模式恒为 0）
    pub applied: usize,
    /// 本轮导出的逐 kind 校准证据行数
    pub evidence_rows: usize,
}

/// 执行一次上报 tick：采样发布 → 校准写表 → 证据输出
///
/// 分工：采样、去抖、事件发布与写表全部由 [`SaturationSource`] 的实现方
/// （`nexus-core::compute::telemetry::ComputeTelemetry`）完成；本函数只保证
/// **每 tick 各调用一次**并按序执行。同一判定两处实现是漏修的头号入口
/// （本仓曾因 publish/publish_blocking 双份实现踩过一次，见 `bus.rs` 的宏形式注释）。
pub fn reporter_tick(source: &dyn SaturationSource, bus: &EventBus) -> TickReport {
    source.sample_and_report(bus);
    // B1/ADR-198:写表也只在低频周期发生（非 apply 模式时为 no-op，
    // 不把“改变运行期阈值”这种治理动作放进卸载热路径）
    let applied = source.apply_calibration();
    if applied > 0 {
        tracing::info!(applied, "HTS 阈值表由 SPRT 校准写入（apply 模式）");
    }
    // 校准证据输出：每 kind 一行（阈值校准是否已发生必须可审计，而不是
    // 只能靠“事后抽库”发现——本仓 Phase A 的教训就是静默无输出等于无守护）
    let evidence = source.calibration_evidence();
    let evidence_rows = evidence.len();
    for stat in evidence {
        tracing::debug!(
            kind = %stat.kind,
            inline_samples = stat.inline_samples,
            pool_samples = stat.pool_samples,
            decided = %stat.decided,
            candidate_threshold = stat.candidate_threshold,
            applied = stat.applied,
            mode = %stat.mode,
            "compute calibration evidence"
        );
    }
    TickReport {
        applied,
        evidence_rows,
    }
}

/// 饱和观测上报器句柄（由装配层启动,与吞吐/拦截率上报器同族）
///
/// # 参数
/// - `bus`:事件总线（`ComputePoolSaturated` 的发布通道）；
/// - `period_secs`:采样周期秒数，`0` 按 `1` 处理（见 [`clamped_period`]）；
/// - `source`:有状态遥测源，装配层注入。
///
/// 本函数只驱动**周期**；每 tick 的工作见 [`reporter_tick`]。
pub fn spawn_saturation_reporter(
    bus: EventBus,
    period_secs: u64,
    source: Arc<dyn SaturationSource>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let period = clamped_period(period_secs);
        loop {
            tokio::time::sleep(period).await;
            reporter_tick(source.as_ref(), &bus);
        }
    })
}
