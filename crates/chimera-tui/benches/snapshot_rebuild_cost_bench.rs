//! `DataSnapshot` 重建成本量化基准 — H-b 前提验证(2026-09-12)
//!
//! # WHY 此 bench 存在
//!
//! 架构重构报告 §4 方向 H-b 的主张是:「`DataSnapshot` 每 250 ms **无条件全量重建**
//! (≥8 个 Vec)」构成性能债,应引入 dirty 标记跳过无事件 tick。
//!
//! 但该主张**从未被量化**——而本项目已有先例:上一版报告曾把「2,125 处 `.clone()`」
//! 当作痛点,实测后发现绝大多数不在热路径,方向被否决(报告 §2 对账表)。
//! 为避免重复同类错误,本 bench 用**公开类型**复现该重建的各个分量,给出数量级证据,
//! 据以决定 H-b 是"实施"还是"否决"。
//!
//! # 分量拆解(对应 `data/pipeline.rs:699-748` 与 `data/sync.rs:67-93`)
//!
//! | 组 | 复现对象 |
//! |---|---|
//! | `tick_rebuild/*` | 每 tick 的"全量重建"分量:5 × `VecDeque<u64>`(容量 64) 的 `collect`、`Vec<TimelineSnapshot>`(100) 的 `clone`、2 × `Vec<MetricSample>`(300) 的 `clone` |
//! | `event_side/*` | 每**事件**的分量(`sync.rs` 模式):`Vec<Quest>` 整集合 `clone` 以改 1 个字段 vs 定位后按键更新 |
//! | `consumer_update/*` | **消费侧**分量(感知延迟方向的先决量化,2026-09-12 追加):`TuiApp::update()` 在 revision 变化时的全字段拷贝 vs 短路路径 vs 快照 clone 基准 |
//!
//! # 判读口径
//!
//! - tick 侧:重建成本 × 4 Hz 占 250 ms tick 预算的比例。若 < 0.1%,则"无条件重建"
//!   不是可观测瓶颈,引入 dirty 标记的复杂度与"跳过时间驱动字段"的正确性风险
//!   不成比例。
//! - 事件侧:单次成本 × 事件率。500–1000 事件/秒的压力档下若达毫秒量级,才值得改
//!   `apply_event` 的返回契约(会波及面板读取路径)。
//! - 消费侧:**这是"要不要提高 revision 频率(事件驱动即时重建)以压掉 350 ms
//!   固定等待"的先决证据**。代价模型:提高 revision 频率 = 重建成本(540 ns,已测)
//!   **+ 消费侧全字段拷贝成本 × 频率**。若消费侧单次拷贝远大于重建,则"即时重建"
//!   在成本上不划算,应先解决消费侧拷贝。
//!
//! 容量口径取自生产默认:`DataSourceConfig::default().max_history_len = 64`、
//! `max_snapshots = 100`、资源回填窗口约 300 样本(5 分钟 @1 s)。

#![forbid(unsafe_code)]

use chimera_tui::data::resource_history::MetricSample;
use chimera_tui::data::{DataSnapshot, DataSourceConfig, TuiDataSource};
use chimera_tui::types::TimelineSnapshot;
use chimera_tui::{TuiApp, TuiConfig, TuiError};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// 生产默认历史窗口(VecDeque 容量)
const MAX_HISTORY_LEN: usize = 64;
/// 生产默认 timeline 快照上限
const MAX_SNAPSHOTS: usize = 100;
/// 资源回填窗口样本数(5 分钟 @ 1 s)
const BACKFILL_SAMPLES: usize = 300;

/// 构造容量已满的 `VecDeque<u64>`(模拟稳态下的历史曲线)
fn full_u64_history() -> VecDeque<u64> {
    let mut d = VecDeque::with_capacity(MAX_HISTORY_LEN);
    for i in 0..MAX_HISTORY_LEN {
        d.push_back(i as u64);
    }
    d
}

/// 构造 `Vec<TimelineSnapshot>`(稳态满额)
fn full_timeline() -> Vec<TimelineSnapshot> {
    (0..MAX_SNAPSHOTS)
        .map(|i| TimelineSnapshot {
            timestamp: chrono::Utc::now(),
            event_count: i as u64,
            event_rate: 42,
            budget_utilization: 0.75,
            health_score: 88,
            decay_coefficient: 0.9,
        })
        .collect()
}

/// 构造资源回填样本序列(稳态满额)
fn full_backfill() -> Vec<MetricSample> {
    (0..BACKFILL_SAMPLES)
        .map(|i| MetricSample::new(1_700_000_000_000 + i as u64 * 1000, 50.0))
        .collect()
}

/// 构造 `Vec<Quest>`(每 Quest 含 `tasks_per_quest` 个任务;任务含 2 个 String)
fn quests(n: usize, tasks_per_quest: usize) -> Vec<Quest> {
    (0..n)
        .map(|q| Quest {
            quest_id: format!("q-{q}"),
            title: format!("性能基准需求 {q}"),
            tasks: (0..tasks_per_quest)
                .map(|i| Task {
                    task_id: format!("t{i}"),
                    description: format!("执行第 {i} 步子任务"),
                    status: TaskStatus::Pending,
                    dependencies: if i == 0 {
                        vec![]
                    } else {
                        vec![format!("t{}", i - 1)]
                    },
                })
                .collect(),
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
            priority: 128,
        })
        .collect()
}

/// 组 1:tick 侧各分量(隔离测量,便于定位"贵在哪")
fn tick_rebuild_components(c: &mut Criterion) {
    let mut group = c.benchmark_group("tick_rebuild");

    let hist = full_u64_history();
    group.bench_function("history_collect_5x64", |b| {
        b.iter(|| {
            // pipeline.rs:710-712/717/728 模式:VecDeque<u64> → Vec<u64> 全量 collect
            let a: Vec<u64> = hist.iter().copied().collect();
            let b1: Vec<u64> = hist.iter().copied().collect();
            let c1: Vec<u64> = hist.iter().copied().collect();
            let d: Vec<u64> = hist.iter().copied().collect();
            let e: Vec<u64> = hist.iter().copied().collect();
            black_box((a, b1, c1, d, e));
        });
    });

    let timeline = full_timeline();
    group.bench_function("timeline_clone_100", |b| {
        b.iter(|| {
            // pipeline.rs:718 模式:iter().cloned().collect()
            let v: Vec<TimelineSnapshot> = timeline.to_vec();
            black_box(v);
        });
    });

    let backfill = full_backfill();
    group.bench_function("backfill_clone_2x300", |b| {
        b.iter(|| {
            // pipeline.rs:729-730 模式:两个 Vec<MetricSample> 直接 clone
            let cpu = backfill.clone();
            let mem = backfill.clone();
            black_box((cpu, mem));
        });
    });

    group.finish();
}

/// 组 2:tick 侧"全量重建"合计(以上分量之和,模拟一次无事件 tick 的可省部分)
fn tick_rebuild_total(c: &mut Criterion) {
    let mut group = c.benchmark_group("tick_rebuild_total");
    let hist = full_u64_history();
    let timeline = full_timeline();
    let backfill = full_backfill();

    group.bench_function("all_components_once", |b| {
        b.iter(|| {
            let h1: Vec<u64> = hist.iter().copied().collect();
            let h2: Vec<u64> = hist.iter().copied().collect();
            let h3: Vec<u64> = hist.iter().copied().collect();
            let h4: Vec<u64> = hist.iter().copied().collect();
            let h5: Vec<u64> = hist.iter().copied().collect();
            let t: Vec<TimelineSnapshot> = timeline.to_vec();
            let cpu = backfill.clone();
            let mem = backfill.clone();
            black_box((h1, h2, h3, h4, h5, t, cpu, mem));
        });
    });

    // 频率口径:4 Hz(250 ms tick)。用 BenchmarkId 标注,便于报告直接换算占比。
    group.bench_with_input(BenchmarkId::new("at_4hz_per_second", 4), &4, |b, &hz| {
        b.iter(|| {
            for _ in 0..hz {
                let h1: Vec<u64> = hist.iter().copied().collect();
                let h2: Vec<u64> = hist.iter().copied().collect();
                let h3: Vec<u64> = hist.iter().copied().collect();
                let h4: Vec<u64> = hist.iter().copied().collect();
                let h5: Vec<u64> = hist.iter().copied().collect();
                let t: Vec<TimelineSnapshot> = timeline.to_vec();
                let cpu = backfill.clone();
                let mem = backfill.clone();
                black_box((h1, h2, h3, h4, h5, t, cpu, mem));
            }
        });
    });

    group.finish();
}

/// 组 3:事件侧分量(`sync.rs` 模式 —— 改 1 个字段复制整个集合 vs 按键更新)
///
/// `sync.rs:83-93` 每次 `apply_event` 都以 `Some(self.quests.clone())` 返回整集合,
/// 即使只改了一个 Quest 的 priority。本组量化该模式与"定位后按键更新"的差距,
/// 并给出 8 / 16 quests 两档(生产规模通常个位数 quest)。
fn event_side_clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("event_side");

    for quest_n in [8usize, 16] {
        let data = quests(quest_n, 8);
        let target_id = data[quest_n / 2].quest_id.clone();

        group.bench_with_input(
            BenchmarkId::new("full_vec_clone", quest_n),
            &quest_n,
            |b, _| {
                b.iter_batched(
                    || data.clone(),
                    |mut v| {
                        // 现状模式:定位改 1 字段,然后**整集合 clone** 交出快照
                        if let Some(q) = v.iter_mut().find(|q| q.quest_id == target_id) {
                            q.priority = 200;
                        }
                        let out = v.clone();
                        black_box(out);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("key_update_view", quest_n),
            &quest_n,
            |b, _| {
                b.iter_batched(
                    || data.clone(),
                    |mut v| {
                        // 对照模式:定位改 1 字段,只返回变更视图(不复制整集合)
                        let mut changed = None;
                        if let Some(q) = v.iter_mut().find(|q| q.quest_id == target_id) {
                            q.priority = 200;
                            changed = Some((q.quest_id.clone(), q.priority));
                        }
                        black_box(changed);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

/// 构造生产典型规模的"满额"快照(revision 可变)。
///
/// 规模口径:8 quests × 8 tasks(活动 Quest 列表)、64 点历史曲线 ×5、
/// 100 条 timeline 快照、300 点资源回填 ×2;其余字段取 `Default`(空指标)。
/// 该规模对应管道稳态(`max_history_len=64` / `max_snapshots=100` 已填满),
/// 是消费侧 `update()` 全字段拷贝的**上界**场景。
fn full_snapshot(revision: u64) -> DataSnapshot {
    DataSnapshot {
        revision,
        quest_list: quests(8, 8),
        budget_history: (0..MAX_HISTORY_LEN as u64).collect(),
        memory_history: (0..MAX_HISTORY_LEN as u64).collect(),
        event_rate_history: (0..MAX_HISTORY_LEN as u64).collect(),
        decay_history: (0..MAX_HISTORY_LEN as u64).collect(),
        sys_metrics_history: (0..MAX_HISTORY_LEN as u64).collect(),
        timeline_snapshots: full_timeline(),
        resource_cpu_backfill: full_backfill(),
        resource_mem_backfill: full_backfill(),
        ..DataSnapshot::default()
    }
}

/// 数据源桩:每次返回 **revision 递增**的快照 —— 使 `TuiApp::update()` 走
/// "revision 变化"的完整路径(全字段拷贝 + dirty 面板标记)。
struct RisingRevisionSource {
    base: DataSnapshot,
    n: AtomicU64,
    cfg: DataSourceConfig,
}

impl TuiDataSource for RisingRevisionSource {
    fn snapshot(&self) -> Result<Arc<DataSnapshot>, TuiError> {
        let r = self.n.fetch_add(1, Ordering::Relaxed) + 1;
        // 结构体更新语法:仅覆盖 revision,其余字段 clone(成本与管道构建一次
        // 快照等价,量级已由 tick_rebuild_total 组给出,可在作差时扣除)
        Ok(Arc::new(DataSnapshot {
            revision: r,
            ..self.base.clone()
        }))
    }
    fn config(&self) -> &DataSourceConfig {
        &self.cfg
    }
}

/// 数据源桩:返回 **固定 revision** 的同一 `Arc` —— 命中 `update()` 的
/// revision 短路(`state.rs:89-92`),用于隔离"无变化帧"成本。
struct FrozenRevisionSource {
    snap: Arc<DataSnapshot>,
    cfg: DataSourceConfig,
}

impl TuiDataSource for FrozenRevisionSource {
    fn snapshot(&self) -> Result<Arc<DataSnapshot>, TuiError> {
        Ok(Arc::clone(&self.snap))
    }
    fn config(&self) -> &DataSourceConfig {
        &self.cfg
    }
}

/// 组 4:消费侧 `update()` 成本(感知延迟方向的先决量化)
///
/// # 为何必须测这里
/// 感知延迟的主导项是 350 ms 固定等待(`poll_duration` 100 ms + 250 ms tick 串联)。
/// 压掉它的候选手段是"事件驱动即时重建"(提高 revision 频率),但代价 =
/// **重建成本(540 ns,tick 侧已测)+ 消费侧全字段拷贝 × 新频率**。
/// 若消费侧单次拷贝远大于 540 ns,则"少重建"式的优化方向不成立,
/// 而应先解决消费侧拷贝本身。
///
/// # 三组对照
/// - `snapshot_clone_only`:单次满额 `DataSnapshot::clone()`(作差基准)
/// - `app_update_full_path`:真实 `TuiApp::update()`(revision 变化 → 全字段拷贝 + dirty 标记)
/// - `app_update_short_circuit`:真实 `TuiApp::update()`(revision 不变 → 短路返回)
///
/// 净消费成本 = `app_update_full_path` − `snapshot_clone_only`(criterion 分项读数作差)。
fn consumer_update_cost(c: &mut Criterion) {
    let mut group = c.benchmark_group("consumer_update");
    let snap = full_snapshot(1);

    // (a) 作差基准:单次满额快照 clone
    group.bench_function("snapshot_clone_only", |b| {
        b.iter(|| black_box(snap.clone()));
    });

    // (b) 真实 update():revision 每次递增 → 走全字段拷贝路径
    // WHY persist_state=false:避免构造器读/写状态文件引入 IO 噪声
    let cfg_full = TuiConfig {
        persist_state: false,
        ..TuiConfig::default()
    };
    let mut app_full = TuiApp::with_data_source(
        cfg_full,
        Box::new(RisingRevisionSource {
            base: snap.clone(),
            n: AtomicU64::new(1),
            cfg: DataSourceConfig::default(),
        }),
    )
    .expect("TuiApp 构造失败(revision 递增桩)");
    group.bench_function("app_update_full_path", |b| {
        b.iter(|| {
            app_full.update();
            black_box(&app_full);
        });
    });

    // (c) 真实 update():revision 固定 → 短路路径
    let cfg_frozen = TuiConfig {
        persist_state: false,
        ..TuiConfig::default()
    };
    let mut app_frozen = TuiApp::with_data_source(
        cfg_frozen,
        Box::new(FrozenRevisionSource {
            snap: Arc::new(full_snapshot(1)),
            cfg: DataSourceConfig::default(),
        }),
    )
    .expect("TuiApp 构造失败(revision 固定桩)");
    app_frozen.update(); // 建立 last_snapshot_revision,后续调用走短路
    group.bench_function("app_update_short_circuit", |b| {
        b.iter(|| {
            app_frozen.update();
            black_box(&app_frozen);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    tick_rebuild_components,
    tick_rebuild_total,
    event_side_clone,
    consumer_update_cost
);
criterion_main!(benches);
