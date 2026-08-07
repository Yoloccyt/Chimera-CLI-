//! 面板大数据量渲染基准 — P2 性能(P-3/P-4)的可证伪证据
//!
//! 覆盖评估报告指出的基准盲区:
//! - Log 面板 1 万事件 `content()` 构建(优化前全量构建 50 行 + 二次滚动,
//!   优化后虚拟滚动窗口 O(visible + 2×BUFFER));
//! - EventStream 面板 1 万事件 `content()` 构建(既有虚拟滚动基线);
//! - EventStream 关键字过滤(每条事件 serde_json 全量序列化路径);
//! - Router 面板 1 万事件 `content()`(Top-K 能力列表 + 事件流驱动展示)。
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use event_bus::{EventMetadata, NexusEvent};
use std::collections::VecDeque;

use chimera_tui::{
    EventStreamPanel, LogPanel, Panel, ParliamentPanel, QuestPanel, RouterPanel, TuiState,
};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// 万级事件流(生产超窗/长会话场景)
const EVENT_COUNT: usize = 10_000;

/// 构造固定 payload 的 CacheHit 事件(避免随机长度干扰测量)
fn make_event(i: usize) -> NexusEvent {
    NexusEvent::CacheHit {
        metadata: EventMetadata::new("bench-scc-cache"),
        cache_key: format!("bench-key-{i}"),
    }
}

/// 构造含 N 条事件的 TuiState
fn state_with_events(count: usize) -> TuiState {
    let mut state = TuiState::new();
    state.latest_events = (0..count).map(make_event).collect::<VecDeque<_>>();
    state
}

fn log_content_10k(c: &mut Criterion) {
    let state = state_with_events(EVENT_COUNT);
    c.bench_function("log_content_10k", |b| {
        b.iter(|| {
            let text = LogPanel::content(&state, EVENT_COUNT - 1);
            black_box(text);
        });
    });
}

fn event_stream_content_10k(c: &mut Criterion) {
    let state = state_with_events(EVENT_COUNT);
    c.bench_function("event_stream_content_10k", |b| {
        b.iter(|| {
            let text = EventStreamPanel::content(&state, EVENT_COUNT - 1);
            black_box(text);
        });
    });
}

fn event_stream_keyword_filter_10k(c: &mut Criterion) {
    let mut state = state_with_events(EVENT_COUNT);
    state.filter_keyword = Some("bench-key-99".into());
    c.bench_function("event_stream_keyword_filter_10k", |b| {
        b.iter(|| {
            let filtered = EventStreamPanel::filtered_events(&state);
            black_box(filtered.len());
        });
    });
}

/// legacy 慢路径对比基准(P1-3 评估报告 v2):内联复刻优化前的
/// `type_name + source + serde_json 全量序列化` 过滤,量化快速路径收益。
fn event_stream_keyword_filter_legacy_10k(c: &mut Criterion) {
    let state = state_with_events(EVENT_COUNT);
    let keyword = "bench-key-99".to_lowercase();
    c.bench_function("event_stream_keyword_filter_legacy_10k", |b| {
        b.iter(|| {
            let n = state
                .latest_events
                .iter()
                .filter(|ev| {
                    let meta = ev.metadata();
                    let haystack = format!(
                        "{} {} {}",
                        ev.type_name(),
                        meta.source,
                        serde_json::to_string(ev).unwrap_or_default()
                    )
                    .to_lowercase();
                    haystack.contains(&keyword)
                })
                .count();
            black_box(n);
        });
    });
}

fn router_content_10k(c: &mut Criterion) {
    let state = state_with_events(EVENT_COUNT);
    c.bench_function("router_content_10k", |b| {
        b.iter(|| {
            let text = RouterPanel::content(&state);
            black_box(text);
        });
    });
}

/// Log 过滤缓存(M4 v1):冷路径(每帧重算索引)vs 命中路径(索引映射)
fn log_filter_cache_10k(c: &mut Criterion) {
    let mut state = state_with_events(EVENT_COUNT);
    state.last_snapshot_revision = 1;
    state.filter_keyword = Some("bench-key-99".into());
    let mut group = c.benchmark_group("log_filter_cache");

    // 冷路径:每次新建面板,模拟生产每帧重算(缓存未命中基线)
    group.bench_function("cold_10k", |b| {
        b.iter(|| {
            let mut panel = LogPanel::new();
            let filtered = panel.filtered_events_cached(&state);
            black_box(filtered.len());
        });
    });

    // 命中路径:复用面板(缓存索引,仅做引用映射,跨帧零过滤开销)
    group.bench_function("hit_10k", |b| {
        let mut panel = LogPanel::new();
        let _ = panel.filtered_events_cached(&state);
        b.iter(|| {
            let filtered = panel.filtered_events_cached(&state);
            black_box(filtered.len());
        });
    });
    group.finish();
}

/// Quest 面板渲染缓存(M4 二期):冷路径(每帧重建)vs 命中路径(复用缓存文本)
fn quest_render_cache_100(c: &mut Criterion) {
    let mut state = TuiState::new();
    state.last_snapshot_revision = 1;
    state.quest_list = (0..100)
        .map(|i| Quest {
            quest_id: format!("q{i}"),
            title: format!("Quest {i}"),
            tasks: vec![Task {
                task_id: format!("t{i}"),
                description: "bench task".into(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            }],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
            priority: 128,
        })
        .collect();
    let area = Rect::new(0, 0, 80, 24);
    let mut group = c.benchmark_group("quest_render_cache");

    // 冷路径:每帧新建面板(模拟缓存未命中基线)
    group.bench_function("cold_100", |b| {
        b.iter(|| {
            let mut panel = QuestPanel::new();
            let mut buf = Buffer::empty(area);
            panel.render(&state, area, &mut buf);
        });
    });

    // 命中路径:复用面板(缓存键未变,仅 clone 缓存文本供渲染)
    group.bench_function("hit_100", |b| {
        let mut panel = QuestPanel::new();
        let mut prime = Buffer::empty(area);
        panel.render(&state, area, &mut prime);
        b.iter(|| {
            let mut buf = Buffer::empty(area);
            panel.render(&state, area, &mut buf);
        });
    });
    group.finish();
}

/// Parliament 面板渲染缓存(M4 二期):冷路径 vs 命中路径
fn parliament_render_cache_100(c: &mut Criterion) {
    let mut state = TuiState::new();
    state.last_snapshot_revision = 1;
    state.latest_events = (0..100)
        .map(|i| NexusEvent::VoteCast {
            metadata: EventMetadata::new("bench"),
            proposal_id: format!("p{i}"),
            voter: "alice".into(),
            vote: i % 2 == 0,
        })
        .collect();
    let area = Rect::new(0, 0, 80, 24);
    let mut group = c.benchmark_group("parliament_render_cache");

    group.bench_function("cold_100", |b| {
        b.iter(|| {
            let mut panel = ParliamentPanel::new();
            let mut buf = Buffer::empty(area);
            panel.render(&state, area, &mut buf);
        });
    });

    group.bench_function("hit_100", |b| {
        let mut panel = ParliamentPanel::new();
        let mut prime = Buffer::empty(area);
        panel.render(&state, area, &mut prime);
        b.iter(|| {
            let mut buf = Buffer::empty(area);
            panel.render(&state, area, &mut buf);
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    log_content_10k,
    event_stream_content_10k,
    event_stream_keyword_filter_10k,
    event_stream_keyword_filter_legacy_10k,
    router_content_10k,
    log_filter_cache_10k,
    quest_render_cache_100,
    parliament_render_cache_100
);
criterion_main!(benches);
