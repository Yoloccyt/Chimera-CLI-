//! 消费侧 `TuiApp::update()` 对齐成本量化 — 感知延迟路径裁定前置(2026-09-12)
//!
//! # 背景(H-b 系列裁定链条)
//!
//! §7.6 已裁定:感知延迟(端到端 225–370 ms)的主导项是 **350 ms 固定等待**
//! (`poll_duration` 100 ms 轮询 + 250 ms 数据 tick 串联),而快照重建仅 540 ns
//! ——「事件驱动即时刷新」(事件到达即重建快照,不等 tick)在**重建侧**成本可行。
//!
//! 剩下的未知数是**消费侧**:`TuiApp::update()` 在 `revision` 变化时走
//! 「全量对齐」路径(约 25 个字段 `clone` + 40 字段 `PartialEq` 比对 + dirty 标记)。
//! 现状数据 tick 4 Hz × ~40% 命中 ⇒ 全量对齐约 1.6 次/秒;若改为事件驱动,
//! 每次 poll(10 Hz)都会命中新 revision ⇒ 全量对齐升到 10 次/秒。
//! **本 bench 量化该路径的单次成本**,据以裁定事件驱动刷新是否可行。
//!
//! # 场景
//!
//! - `update_short_circuit`:`revision` 不变 → P-1 短路(现状多数 poll 命中)
//! - `update_full_align`:`revision` 递增 → 全量对齐(事件驱动后每次 poll 命中)
//!
//! 两场景用同一 `TuiApp`(headless,`persist_state=false` 保证确定性)与同一
//! **真实规模**的快照(quest 8 / chat 60 条 / 历史 64 / timeline 100 / 回填 300)。
//!
//! # 判读口径
//!
//! 全量对齐成本 × 10 Hz(poll 频率)与「350 ms → ~100 ms」的收益对比:
//! - 若单次对齐为**几十 µs** → 每秒 CPU < 0.05%,事件驱动刷新可行;
//! - 若达**毫秒级** → 需先做字段级增量对齐(仅同步变化的字段)再实施。

#![forbid(unsafe_code)]

use chimera_tui::data::resource_history::MetricSample;
use chimera_tui::types::{ChatMessage, ChatRole, TimelineSnapshot};
use chimera_tui::{DataSnapshot, TuiApp, TuiConfig};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use event_bus::EventMetadata;
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// 每条聊天消息的正文长度(模拟真实流式回复的单条规模)
const CHAT_MSG_CHARS: usize = 180;
/// 聊天消息条数上限(与 ChatSync 内部保留上限同量级)
const CHAT_MSG_COUNT: usize = 60;

/// 构造生产规模的 quest 列表(8 quests × 8 tasks)
fn quest_list() -> Vec<Quest> {
    (0..8)
        .map(|q| Quest {
            quest_id: format!("q-{q}"),
            title: format!("性能基准需求 {q}"),
            tasks: (0..8)
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

/// 构造聊天消息流(n 条,每条 ~200 字符)
fn chat_msgs(n: usize) -> Vec<ChatMessage> {
    (0..n)
        .map(|i| ChatMessage {
            role: if i % 2 == 0 {
                ChatRole::User
            } else {
                ChatRole::Assistant
            },
            content: format!("消息 {i}:{}", "x".repeat(CHAT_MSG_CHARS)),
        })
        .collect()
}

/// 构造稳态满额的合成快照(revision 可指定)
fn synthetic_snapshot(revision: u64, chat_n: usize) -> DataSnapshot {
    let mut snap = DataSnapshot::default();
    snap.revision = revision;
    snap.quest_list = quest_list();
    snap.chat_messages = chat_msgs(chat_n);
    snap.budget_history = (0..64).collect();
    snap.memory_history = (0..64).collect();
    snap.event_rate_history = (0..64).collect();
    snap.decay_history = (0..64).collect();
    snap.sys_metrics_history = (0..64).collect();
    snap.timeline_snapshots = (0..100)
        .map(|i| TimelineSnapshot {
            timestamp: chrono::Utc::now(),
            event_count: i as u64,
            event_rate: 42,
            budget_utilization: 0.75,
            health_score: 88,
            decay_coefficient: 0.9,
        })
        .collect();
    snap.resource_cpu_backfill = (0..300)
        .map(|i| MetricSample::new(1_700_000_000_000 + i as u64 * 1000, 50.0))
        .collect();
    snap.resource_mem_backfill = (0..300)
        .map(|i| MetricSample::new(1_700_000_000_000 + i as u64 * 1000, 60.0))
        .collect();
    snap
}

/// 可切换语义的数据源桩:
/// - `fixed`:每次返回同一 revision(短路路径)
/// - `seq`:每次调用递增 revision(全量对齐路径;chat 流随 revision 增长,
///   模拟"流式回复期间每次 poll 都有新内容"的真实形态)
struct SeqSource {
    fixed: Option<Arc<DataSnapshot>>,
    seq: AtomicU64,
    cfg: chimera_tui::DataSourceConfig,
}

impl chimera_tui::data::TuiDataSource for SeqSource {
    fn snapshot(&self) -> Result<Arc<DataSnapshot>, chimera_tui::TuiError> {
        if let Some(s) = &self.fixed {
            return Ok(Arc::clone(s));
        }
        let rev = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(Arc::new(synthetic_snapshot(
            rev,
            ((rev as usize) % (CHAT_MSG_COUNT + 1)).max(1),
        )))
    }

    fn config(&self) -> &chimera_tui::DataSourceConfig {
        &self.cfg
    }
}

/// headless 构造 TuiApp(persist_state=false 保证 bench 确定性,
/// 不读用户机器上的状态文件)
fn make_app(source: SeqSource) -> TuiApp {
    let tui_config = TuiConfig {
        persist_state: false,
        ..TuiConfig::default()
    };
    TuiApp::with_data_source(tui_config, Box::new(source)).expect("TuiApp 构造失败")
}

/// 场景 A:revision 不变 → 短路路径(现状多数 poll 命中的成本)
fn update_short_circuit(c: &mut Criterion) {
    let mut group = c.benchmark_group("tui_update_cost");
    group.bench_function("short_circuit_revision_unchanged", |b| {
        let mut app = make_app(SeqSource {
            fixed: Some(Arc::new(synthetic_snapshot(1, 40))),
            seq: AtomicU64::new(0),
            cfg: chimera_tui::DataSourceConfig::default(),
        });
        app.update(); // 首次全量对齐,之后每 iter 命中短路
        b.iter(|| {
            black_box(app.update());
        });
    });
    group.finish();
}

/// 场景 B:revision 递增 → 全量对齐路径(事件驱动刷新后每次 poll 命中的成本)
fn update_full_align(c: &mut Criterion) {
    let mut group = c.benchmark_group("tui_update_cost");
    group.bench_function(BenchmarkId::new("full_align", "revision_changes"), |b| {
        let mut app = make_app(SeqSource {
            fixed: None,
            seq: AtomicU64::new(0),
            cfg: chimera_tui::DataSourceConfig::default(),
        });
        app.update(); // 首次对齐
        b.iter(|| {
            black_box(app.update());
        });
    });
    group.finish();
}

criterion_group!(benches, update_short_circuit, update_full_align);
criterion_main!(benches);
