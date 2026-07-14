//! P4.1 增量渲染集成测试
//!
//! 验证 `dirty_panels` 标记机制:
//! - 数据未变化时不产生脏标记;
//! - 仅 Budget 字段变化时只标记 Budget,不影响其他面板;
//! - 事件流变化同时驱动 Parliament / Log / EventStream;
//! - 渲染结束后 `dirty_panels` 被清空,为下一帧提供干净的起点。
//!
//! WHY 独立测试文件:P4.1 引入的标记机制跨 `TuiState` 与 `TuiApp`,
//! 需要真实 `update` → `render` 流程,无法仅靠 `TuiState` 单元测试
//! 覆盖;同时与现有 `data_test.rs` / `live_data_test.rs` 区分,避免
//! 单一测试文件持续膨胀。
//!
//! # 测试设计说明
//! 每个用例通过 `MockDataSource` 注入一份 `DataSnapshot` 后调用
//! `TuiApp::update`,利用 `TuiState::new()` 默认值与 `DataSnapshot::default()`
//! 在各数据字段上语义一致这一事实,使得"仅改动某字段"时,`update`
//! 只会把对应面板标记为 dirty,无需先做 warm-up 同步。

use std::collections::VecDeque;

use chimera_tui::config::TuiConfig;
use chimera_tui::data::{BudgetMetrics, DataSnapshot, DataSourceConfig, TuiDataSource};
use chimera_tui::error::TuiError;
use chimera_tui::types::{PanelId, TuiState};
use chimera_tui::TuiApp;
use event_bus::{EventMetadata, NexusEvent};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 可编程 mock 数据源 — 每次 `snapshot()` 返回当前内部快照的克隆。
#[derive(Debug)]
struct MockDataSource {
    snapshot: DataSnapshot,
    config: DataSourceConfig,
}

impl MockDataSource {
    fn new(snapshot: DataSnapshot) -> Self {
        Self {
            snapshot,
            config: DataSourceConfig::default(),
        }
    }
}

impl TuiDataSource for MockDataSource {
    fn snapshot(&self) -> Result<DataSnapshot, TuiError> {
        Ok(self.snapshot.clone())
    }

    fn config(&self) -> &DataSourceConfig {
        &self.config
    }
}

/// 构造一个带指定利用率的 BudgetMetrics,其余字段使用默认值。
fn budget(utilization: f32) -> BudgetMetrics {
    BudgetMetrics {
        utilization_rate: utilization,
        ..Default::default()
    }
}

/// 构造一个简单 CacheHit 事件。
fn cache_hit(key: &str) -> NexusEvent {
    NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: key.into(),
    }
}

/// 用指定快照创建一个 TuiApp 并立即调用一次 update,返回 app。
///
/// WHY 抽取:`TuiState::new()` 与 `DataSnapshot::default()` 在各数据字段
/// 上语义一致,所以快照中改动的字段会驱动 `mark_dirty`,其余字段保持 clean。
fn make_app_and_update(snapshot: DataSnapshot) -> TuiApp {
    let data_source = MockDataSource::new(snapshot);
    let mut app = TuiApp::with_data_source(TuiConfig::default(), Box::new(data_source)).unwrap();
    app.update();
    app
}

// ============================================================
// 基础 mark_dirty / take_dirty / clear_dirty API 行为
// ============================================================

#[test]
fn test_state_mark_and_take_dirty() {
    let mut state = TuiState::new();

    assert!(state.dirty_panels.is_empty());
    assert!(!state.is_dirty(PanelId::Budget));

    state.mark_dirty(PanelId::Budget);
    state.mark_dirty(PanelId::Quest);

    assert!(state.is_dirty(PanelId::Budget));
    assert!(state.is_dirty(PanelId::Quest));
    assert!(!state.is_dirty(PanelId::Memory));

    // take_dirty 返回集合且自身清空
    let taken = state.take_dirty();
    assert_eq!(taken.len(), 2);
    assert!(taken.contains(&PanelId::Budget));
    assert!(taken.contains(&PanelId::Quest));
    assert!(state.dirty_panels.is_empty());
}

#[test]
fn test_state_clear_dirty_resets() {
    let mut state = TuiState::new();
    state.mark_dirty(PanelId::Budget);
    state.mark_dirty(PanelId::Memory);
    assert_eq!(state.dirty_panels.len(), 2);

    state.clear_dirty();
    assert!(state.dirty_panels.is_empty());
}

// ============================================================
// update() 时数据未变化 → 不产生 dirty
// ============================================================

#[test]
fn test_update_with_identical_snapshot_marks_nothing() {
    let mut app = make_app_and_update(DataSnapshot::default());
    // 第一次 update 后,state 与快照一致。
    app.state_mut().clear_dirty();

    // 第二次 update:数据源未变,与 state 一致,必无 dirty。
    app.update();
    assert!(
        app.state().dirty_panels.is_empty(),
        "identical snapshot should not mark any panel dirty"
    );
}

// ============================================================
// 仅 Budget 字段变化 → 只标记 Budget
// ============================================================

#[test]
fn test_budget_only_change_marks_budget_only() {
    let snapshot = DataSnapshot {
        budget_metrics: budget(0.75),
        ..Default::default()
    };
    let app = make_app_and_update(snapshot);

    assert!(
        app.state().is_dirty(PanelId::Budget),
        "budget change should mark Budget panel"
    );
    assert!(
        !app.state().is_dirty(PanelId::Memory),
        "unrelated Memory panel must stay clean"
    );
    assert!(
        !app.state().is_dirty(PanelId::Quest),
        "unrelated Quest panel must stay clean"
    );
    assert!(
        !app.state().is_dirty(PanelId::Log),
        "unrelated Log panel must stay clean"
    );
}

// ============================================================
// latest_events 变化 → 同时驱动 Parliament / Log / EventStream
// ============================================================

#[test]
fn test_event_stream_change_marks_three_panels() {
    let snapshot = DataSnapshot {
        latest_events: VecDeque::from([cache_hit("k1")]),
        ..Default::default()
    };
    let app = make_app_and_update(snapshot);

    assert!(app.state().is_dirty(PanelId::Parliament));
    assert!(app.state().is_dirty(PanelId::Log));
    assert!(app.state().is_dirty(PanelId::EventStream));
    assert!(!app.state().is_dirty(PanelId::Budget));
}

// ============================================================
// 渲染结束后 dirty_panels 被清空
// ============================================================

#[test]
fn test_render_clears_dirty_panels() {
    let snapshot = DataSnapshot {
        budget_metrics: budget(0.9),
        ..Default::default()
    };
    let mut app = make_app_and_update(snapshot);

    assert!(
        !app.state().dirty_panels.is_empty(),
        "update should mark Budget dirty before render"
    );

    // 执行一次 render,应在末尾清空 dirty
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    assert!(
        app.state().dirty_panels.is_empty(),
        "render() must clear dirty_panels at end of frame"
    );
}

// ============================================================
// 帧结束后再 update 相同快照 → 仍无 dirty
// ============================================================

#[test]
fn test_subsequent_identical_update_stays_clean() {
    let snapshot = DataSnapshot {
        budget_metrics: budget(0.4),
        ..Default::default()
    };

    let data_source = MockDataSource::new(snapshot);
    let mut app = TuiApp::with_data_source(TuiConfig::default(), Box::new(data_source)).unwrap();

    // 首次 update 会让 state 与 snapshot 对齐,budget 不同 → Budget dirty
    app.update();
    assert!(app.state().is_dirty(PanelId::Budget));

    // render 清空 dirty
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    assert!(app.state().dirty_panels.is_empty());

    // 再次 update 相同快照 → 无字段变化 → 无 dirty
    app.update();
    assert!(
        app.state().dirty_panels.is_empty(),
        "identical snapshot after first sync must not mark any panel dirty"
    );
}

// ============================================================
// 多面板同时变化 → 多面板被标记
// ============================================================

#[test]
fn test_multiple_panels_marked_when_multiple_fields_change() {
    let snapshot = DataSnapshot {
        budget_metrics: budget(0.5),
        budget_history: vec![10, 20, 30],
        memory_metrics: chimera_tui::data::MemoryMetrics {
            hit_rate_percent: 80.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let app = make_app_and_update(snapshot);

    assert!(app.state().is_dirty(PanelId::Budget));
    assert!(app.state().is_dirty(PanelId::Memory));
    assert!(!app.state().is_dirty(PanelId::Quest));
    assert!(!app.state().is_dirty(PanelId::Security));
}

// ============================================================
// Quest 列表变化 → Quest 面板被标记
// ============================================================

#[test]
fn test_quest_change_marks_quest_only() {
    use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};

    let quest = Quest {
        quest_id: "q-test".into(),
        title: "test quest".into(),
        tasks: vec![Task {
            task_id: "t1".into(),
            description: "task".into(),
            status: TaskStatus::Pending,
            dependencies: vec![],
        }],
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
    };
    let snapshot = DataSnapshot {
        quest_list: vec![quest],
        ..Default::default()
    };
    let app = make_app_and_update(snapshot);

    assert!(app.state().is_dirty(PanelId::Quest));
    assert!(!app.state().is_dirty(PanelId::Budget));
    assert!(!app.state().is_dirty(PanelId::Log));
}
