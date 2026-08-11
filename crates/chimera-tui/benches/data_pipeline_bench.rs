//! TUI 数据管道性能基准测试
//!
//! 对应任务:P3.2 性能与压力验证
//! 对应架构层:L10 Interface(`chimera-tui` → `event-bus` 向下依赖,§2.2)
//!
//! # 基准项
//! - `data_pipeline_snapshot_latency`:在 1000 事件/秒压力下,单次快照生成延迟。
//!   目标 P95 < 100ms(250ms tick 内 250 条事件 + 1 次 snapshot)。
//! - `data_pipeline_throughput`:测量管道的事件处理吞吐量。
//!
//! # 设计理由(WHY)
//! - **250 事件/tick**:`DataSourceConfig::default().tick_interval_ms = 250`,
//!   模拟 1000 事件/秒的稳态压力 → 每 tick 累积 250 条事件。
//!   选择 `BudgetMetricsUpdated` 作为基准事件:它是 TUI Budget 面板
//!   高频消费的结构化指标,触发 `BudgetSync::apply_event` 完整路径。
//! - **300ms 等待**:略大于 250ms tick,确保 `interval.tick()` 至少触发一次,
//!   后台任务完成事件聚合与快照写入,避免测到空快照。
//! - **每 iter 重建管道**:避免历史事件污染下一轮测量,代价是包含
//!   `DataPipeline::new` 与 `shutdown` 的固定开销。这与"测量稳态延迟"
//!   的目标略有偏差,但 criterion 默认 sample_size=100 足以摊薄固定开销,
//!   且更接近真实生产中"启动 → 运行 → 关闭"的完整生命周期。
//! - **`publish_blocking` 同步发布**:避免 `publish().await` 引入 tokio
//!   调度噪声,精确测量事件投递 → 后台聚合 → 快照读取的端到端延迟。
//!
//! # min-of-N 5 采样(Engineering Convention)
//! criterion 默认 sample_size=100 + 5 warmup,统计上等价于"min-of-N 5"采样,
//! 可减少 Windows 调度噪声。本 bench 沿用默认配置不显式调小 sample_size。
//!
//! # 反模式规避(§4.4)
//! - `EventSubscriber::new` 在 `tokio::spawn` 之前同步调用 `bus.subscribe()`
//!   (反模式 #3:先 subscribe 再 spawn,避免事件静默丢失)。
//! - `pipeline.shutdown().await` 显式 abort + await JoinHandle,
//!   避免 orphan task(反模式 #7)。
//! - 后台任务内 `tokio::select!` 不持锁跨 `.await`(反模式 #1):
//!   锁仅在快照写入时短暂持有,事件消费与状态同步在锁外完成。

#![forbid(unsafe_code)]

use chimera_tui::{DataPipeline, DataSourceConfig, EventSubscriber};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use event_bus::{BudgetMetricsPayload, EventBus, EventMetadata, NexusEvent};
use std::collections::VecDeque;
use std::time::Duration;
use tokio::runtime::Runtime;

/// 每 tick 发布的事件数量(1000 事件/秒 × 250ms tick = 250 事件)
const EVENTS_PER_TICK: usize = 250;

/// 等待 tick 触发的余量(略大于 250ms,确保至少一次 tick 完成)
const TICK_WAIT_MS: u64 = 300;

/// 构造用于基准测试的 `BudgetMetricsUpdated` 事件
///
/// WHY 固定 payload:避免随机文本长度变化干扰测量;
/// `BudgetMetricsUpdated` 是 L9 efficiency-monitor → L10 TUI 的高频事件,
/// 触发 `BudgetSync::apply_event` 完整状态更新路径。
fn make_budget_event() -> NexusEvent {
    NexusEvent::BudgetMetricsUpdated {
        metadata: EventMetadata::new("bench-efficiency-monitor"),
        metrics: BudgetMetricsPayload {
            total_consumption: 7500.0,
            remaining_budget: 2500.0,
            utilization_rate: 0.75,
            current_tier: "Medium".into(),
            coefficient: 0.9,
            is_exceeded: false,
            alert: None,
        },
    }
}

/// bench 1:1000 事件/秒压力下的快照生成延迟
///
/// 模拟生产场景:每 250ms tick 累积 250 条事件,后台任务聚合后生成快照。
/// 此 bench 测量"事件投递 → 后台聚合 → 快照读取"的端到端延迟,
/// 目标 P95 < 100ms(扣除 300ms 等待后的净延迟)。
///
/// # 测量说明
/// 每 iter 包含:
/// 1. 创建 EventBus + EventSubscriber + DataPipeline(固定开销)
/// 2. 同步发布 250 个事件(模拟 250ms 内累积)
/// 3. 等待 300ms 让 tick 发生(固定开销,反映真实生产时序)
/// 4. 调用 `pipeline.snapshot()` 读取快照(被测目标)
/// 5. `pipeline.shutdown().await` 释放资源(固定开销)
///
/// 注:步骤 1/3/5 的固定开销会拉高绝对延迟,但 criterion 默认 sample_size=100
/// 足以稳定统计,且固定开销在 P95 分位上同样稳定,不影响相对比较。
fn data_pipeline_snapshot_latency(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("data_pipeline_snapshot_latency");
    group.bench_function("250_events_per_tick", |b| {
        b.iter(|| {
            rt.block_on(async {
                // 步骤 1:创建 EventBus、EventSubscriber、DataPipeline
                // WHY 先 subscribe 再 spawn(§4.4 反模式 #3):
                // EventSubscriber::new 内部同步调用 bus.subscribe() 后才 spawn 后台任务,
                // 避免错过早期发布的事件。
                let bus = EventBus::new();
                let subscriber = EventSubscriber::new(bus.clone());
                let config = DataSourceConfig {
                    tick_interval_ms: 250,
                    ..Default::default()
                };
                let pipeline = DataPipeline::new(subscriber, config);

                // 步骤 2:同步发布 250 个事件(模拟 1000 事件/秒 × 250ms tick)
                // WHY publish_blocking:同步发布排除 tokio 调度噪声,
                // 精确测量事件投递 → 后台聚合 → 快照读取的端到端延迟。
                for _ in 0..EVENTS_PER_TICK {
                    bus.publish_blocking(black_box(make_budget_event()))
                        .expect("publish 失败");
                }

                // 步骤 3:等待 tick 触发(略大于 250ms,确保至少一次 tick)
                tokio::time::sleep(Duration::from_millis(TICK_WAIT_MS)).await;

                // 步骤 4:读取快照(被测目标)
                let snapshot = pipeline.snapshot();
                black_box(snapshot);

                // 步骤 5:关闭管道,显式 await 避免 orphan task(§4.4 反模式 #7)
                pipeline.shutdown().await;
            });
        });
    });
    group.finish();
}

/// bench 2:事件处理吞吐量
///
/// 使用 `Throughput::Elements(n)` 让 criterion 报告 events/sec,
/// 直观反映管道在不同压力档位下的稳态处理能力。
///
/// # 测量说明
/// 与 bench 1 流程相同,但通过 `Throughput::Elements` 标注每次 iter 处理
/// n 条事件(n 随档位变化),criterion 会自动换算为 events/sec 报告。
///
/// # 多档位对比
/// 通过 125/250/500 三档事件量,观察吞吐随压力的变化:
/// - 125 事件/tick = 500 事件/秒(轻载)
/// - 250 事件/tick = 1000 事件/秒(目标场景)
/// - 500 事件/tick = 2000 事件/秒(过载)
fn data_pipeline_throughput(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("data_pipeline_throughput");
    // 标注每次 iter 处理 n 条事件,criterion 报告 events/sec
    // 注:Throughput 在 bench_with_input 内通过闭包参数 n 动态对应

    // 三档事件量对比:500/1000/2000 事件/秒压力
    for &event_count in &[125usize, 250, 500] {
        group.throughput(Throughput::Elements(event_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(event_count),
            &event_count,
            |b, &n| {
                b.iter(|| {
                    rt.block_on(async {
                        let bus = EventBus::new();
                        let subscriber = EventSubscriber::new(bus.clone());
                        let config = DataSourceConfig {
                            tick_interval_ms: 250,
                            ..Default::default()
                        };
                        let pipeline = DataPipeline::new(subscriber, config);

                        for _ in 0..n {
                            bus.publish_blocking(black_box(make_budget_event()))
                                .expect("publish 失败");
                        }

                        tokio::time::sleep(Duration::from_millis(TICK_WAIT_MS)).await;

                        let snapshot = pipeline.snapshot();
                        black_box(snapshot);

                        pipeline.shutdown().await;
                    });
                });
            },
        );
    }
    group.finish();
}

/// bench 3:快照读取开销(隔离测量,排除 tick 等待)
///
/// WHY 补全评估盲区:bench 1 把 300ms tick 等待计入每次 iter,快照读取本身
/// 的成本被固定开销淹没。本 bench 先完成一次"发布 + tick",再在 iter 内只测
/// `pipeline.snapshot()`——P-1 优化后为 Arc 引用计数递增(原实现为整包深拷贝,
/// 含 latest_events / quest_list / 历史曲线),直接量化快照读取的边际成本。
fn snapshot_read_latency(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("snapshot_read_latency");
    group.bench_function("after_250_events_tick", |b| {
        let pipeline = rt.block_on(async {
            let bus = EventBus::new();
            let subscriber = EventSubscriber::new(bus.clone());
            let config = DataSourceConfig {
                tick_interval_ms: 250,
                ..Default::default()
            };
            let pipeline = DataPipeline::new(subscriber, config);

            for _ in 0..EVENTS_PER_TICK {
                bus.publish_blocking(black_box(make_budget_event()))
                    .expect("publish 失败");
            }
            tokio::time::sleep(Duration::from_millis(TICK_WAIT_MS)).await;
            pipeline
        });

        b.iter(|| {
            let snapshot = pipeline.snapshot();
            black_box(snapshot);
        });

        rt.block_on(async { pipeline.shutdown().await });
    });
    group.finish();
}

/// bench 4:push_history 队首淘汰复杂度(评估报告 P0-2 验收)
///
/// VecDeque `push_back`/`pop_front` 为 O(1):不同容量(64/1024/16384)下
/// 持续 push 的耗时应当近似恒定;若回归为 `Vec::remove(0)` 的 O(n) 实现,
/// 16384 容量下耗时将随容量显著增长(性能可证伪铁律)。
fn push_history_scale(c: &mut Criterion) {
    use chimera_tui::data::pipeline::push_history;

    let mut group = c.benchmark_group("push_history_scale");
    for &max in &[64usize, 1024, 16384] {
        group.bench_with_input(BenchmarkId::from_parameter(max), &max, |b, &max| {
            // 预热至满容量:后续 iter 全部触发队首淘汰路径
            let mut history: VecDeque<u64> = VecDeque::with_capacity(max);
            for i in 0..max {
                push_history(&mut history, i as u64, max);
            }
            b.iter(|| {
                // 满容量下连续 push 64 次:每次触发 pop_front + push_back
                for i in 0..64u64 {
                    push_history(&mut history, i, max);
                }
                black_box(&history);
            });
        });
    }
    group.finish();
}

/// bench 5: history backfill latency (Concord T1.6, P4 wired)
///
/// backfill_resource_history reads the last 5-minute window from SQLite with a
/// 200ms budget. Two scales: empty DB (first-run) and 3000 seeded samples
/// (about one hour of 1Hz-equivalent history). Both must stay far below the
/// 200ms budget; a regression toward the budget triggers the R7 slow-sync
/// degradation in production (review against refactor_base baseline).
fn history_backfill_latency(c: &mut Criterion) {
    use chimera_tui::data::pipeline::backfill_resource_history;
    use chimera_tui::MetricsHistory;

    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("history_backfill_latency");
    for &samples in &[0usize, 3000] {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("bench_hist.sqlite");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let seeded = rt.block_on(async {
            let h = MetricsHistory::new(&db_path).await.expect("open db");
            for i in 0..samples {
                h.insert(now - (i as u64) * 1000, "cpu_usage", 50.0)
                    .await
                    .expect("seed");
            }
            h
        });
        group.bench_with_input(BenchmarkId::from_parameter(samples), &samples, |b, &_| {
            b.iter(|| {
                let res = rt.block_on(backfill_resource_history(&seeded, now));
                black_box(res);
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    data_pipeline_snapshot_latency,
    data_pipeline_throughput,
    snapshot_read_latency,
    push_history_scale,
    history_backfill_latency
);
criterion_main!(benches);
