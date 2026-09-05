//! MetricsHistory/ResourceHistory 接线集成测试 — Concord 重构 T1.6(P4②)
//!
//! 对应架构层:L10 Interface(`chimera-tui`)
//!
//! 覆盖三条路径:
//! 1. 主路径:`new_with_history` 接线后,首个慢 tick 即完成回填(快照可见)
//!    且采样写入 SQLite(独立连接直查验证);
//! 2. 边界路径:空库回填返回成功但历史为空(首次启动场景);
//! 3. 零回归路径:经典 `new` 管道回填字段恒空(未接线零行为变化)。
//!
//! 红线对照:SQLite 读写全部经 MetricsHistory 内部 spawn_blocking(§4.4 #2);
//! 采样写入为幂等 fire-and-forget(§4.4 #7)。

#![forbid(unsafe_code)]

// Concord W4 T4.3:异步轮询超时统一经 build_scaled_timeout! 护栏(debug×4/release×1.5)
#[macro_use]
mod common;

use std::sync::Arc;
use std::time::Duration;

use chimera_tui::{DataPipeline, DataSourceConfig, EventSubscriber, MetricsHistory};
use event_bus::EventBus;
use tempfile::TempDir;

/// 当前 Unix 毫秒时间戳(与 MetricsHistory 内部口径一致)
fn current_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 快速 tick 配置(100ms,DataSourceConfig 合法下界,缩短测试等待)
fn fast_config() -> DataSourceConfig {
    DataSourceConfig {
        tick_interval_ms: 100,
        ..DataSourceConfig::default()
    }
}

/// 主路径:接线后回填进快照 + 采样落库可直查
#[tokio::test]
async fn pipeline_with_history_backfills_and_persists() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("hist.sqlite");
    let history = MetricsHistory::new(&db_path).await.expect("open db");

    // 预置历史点(重启前数据),供首个慢 tick 回填读取
    let now = current_ms();
    history
        .insert(now - 60_000, "cpu_usage", 42.0)
        .await
        .expect("seed cpu");
    history
        .insert(now - 60_000, "mem_usage", 55.0)
        .await
        .expect("seed mem");

    let bus = EventBus::new();
    let subscriber = EventSubscriber::new(bus.clone());
    let pipeline = DataPipeline::new_with_history(subscriber, fast_config(), Arc::new(history));

    // 轮询等待回填进快照(首 tick 含 SysMetricsCollector::new 的系统全量
    // 采集,Windows 上可达数秒;固定 sleep 会造成时序脆弱,与 data_test.rs
    // 的 wait_for_events 同一模式)。build_scaled_timeout!(2.0):debug 8s/release 3s。
    let deadline = std::time::Instant::now() + build_scaled_timeout!(2.0);
    let mut snap = pipeline.snapshot();
    while snap.resource_cpu_backfill.is_empty() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
        snap = pipeline.snapshot();
    }
    assert!(
        !snap.resource_cpu_backfill.is_empty(),
        "CPU 回填应在首慢 tick 后进快照(revision={},预置点位于 5 分钟窗口内)",
        snap.revision
    );
    assert!(
        !snap.resource_mem_backfill.is_empty(),
        "内存回填应在首慢 tick 后进快照"
    );

    // 采样写入路径:轮询等待 fire-and-forget insert 落库后独立连接直查
    let verify = MetricsHistory::new(&db_path).await.expect("reopen db");
    let deadline = std::time::Instant::now() + build_scaled_timeout!(2.0);
    let mut cpu = Vec::new();
    while cpu.len() < 2 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cpu = verify
            .query_range(
                "cpu_usage",
                now.saturating_sub(120_000),
                current_ms() + 60_000,
            )
            .await
            .expect("query cpu");
    }
    assert!(
        cpu.len() >= 2,
        "应含预置点 + 管道新写入采样点,实际 {} 条",
        cpu.len()
    );

    pipeline.shutdown().await;
}

/// 边界路径:空库回填 = 成功 + 空历史(首次启动无历史数据)
#[tokio::test]
async fn backfill_on_empty_db_returns_empty_success() {
    let tmp = TempDir::new().expect("tempdir");
    let history = MetricsHistory::new(&tmp.path().join("empty.sqlite"))
        .await
        .expect("open db");
    let result =
        chimera_tui::data::pipeline::backfill_resource_history(&history, current_ms()).await;
    let (cpu, mem) = result.expect("空库回填应成功(None 仅用于超时/失败)");
    assert!(cpu.is_empty(), "空库 CPU 历史应为空");
    assert!(mem.is_empty(), "空库内存历史应为空");
}

/// 零回归路径:经典 new() 管道回填字段恒空(未接线零行为变化)
#[tokio::test]
async fn plain_pipeline_keeps_backfill_fields_empty() {
    let bus = EventBus::new();
    let subscriber = EventSubscriber::new(bus.clone());
    let pipeline = DataPipeline::new(subscriber, fast_config());

    // 轮询等待首个 tick(revision>0)后断言回填字段恒空;同样规避
    // SysMetricsCollector 初始化耗时的时序脆弱。
    let deadline = std::time::Instant::now() + build_scaled_timeout!(2.0);
    let mut snap = pipeline.snapshot();
    while snap.revision == 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
        snap = pipeline.snapshot();
    }
    assert!(snap.revision > 0, "管道应至少完成一个 tick");
    assert!(
        snap.resource_cpu_backfill.is_empty() && snap.resource_mem_backfill.is_empty(),
        "未接历史层的管道不应产生回填数据"
    );
    pipeline.shutdown().await;
}

/// 性能预算(实施计划 T1.6/波次门槛):3000 样本规模回填必须在 250ms 内
///
/// WHY 3000:约一小时 1Hz 等效历史(慢同步 1s 节奏),覆盖典型运行时长;
/// 250ms 为 tick 预算上限(R7:超此值生产链路自动降级慢同步)。
#[tokio::test]
async fn backfill_within_budget_for_hour_scale_history() {
    let tmp = TempDir::new().expect("tempdir");
    let history = MetricsHistory::new(&tmp.path().join("budget.sqlite"))
        .await
        .expect("open db");
    let now = current_ms();
    // 预置 3000 个 cpu_usage 样本(间隔 1s,覆盖窗口内约 300 条)
    for i in 0..3000u64 {
        history
            .insert(now - i * 1000, "cpu_usage", 50.0)
            .await
            .expect("seed");
    }

    let start = std::time::Instant::now();
    let result = chimera_tui::data::pipeline::backfill_resource_history(&history, now).await;
    let elapsed = start.elapsed();

    let (cpu, _mem) = result.expect("预算内回填应成功");
    assert!(!cpu.is_empty(), "窗口内应有回填样本");
    assert!(
        elapsed < Duration::from_millis(250),
        "3000 样本规模回填耗时 {:?} 超 250ms 预算",
        elapsed
    );
}
