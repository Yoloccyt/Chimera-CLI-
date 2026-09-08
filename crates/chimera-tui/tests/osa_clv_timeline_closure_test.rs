//! OSA / CLV / Timeline 数据链闭环集成测试(评估报告 FC-1 修复验收)
//!
//! # 背景(WHY)
//! 2026-09-06 四维评估发现:DataPipeline 已将 `osa_sparsity` / `clv_summary` /
//! `timeline_snapshots` 等字段写入 `DataSnapshot`(pipeline.rs 快照构造),
//! 但 `TuiApp::update` 的字段同步清单遗漏这些字段,导致 OsaSparse / ClvVector /
//! Timeline 三面板在生产环境恒为空态;此前相关面板测试全部手工注入 TuiState
//! 字段,绕过了真实管道,掩盖了该断链。
//!
//! # 测试策略
//! 黑盒事件流:经真实 `EventBus → DataPipeline → TuiApp::update → 面板渲染`
//! 全链路驱动,断言(1)字段同步;(2)面板渲染非空;(3)变化时 dirty 标记。
//! 严禁手工注入 TuiState 字段(那正是掩盖本 bug 的反模式)。

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use chimera_tui::{DataPipeline, DataSourceConfig, EventSubscriber, PanelId, TuiApp, TuiConfig};
use event_bus::{ClvSummary, EventBus, EventMetadata, NexusEvent};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 构造测试用 TuiConfig(Dashboard 视图,不持久化)
fn test_config() -> TuiConfig {
    TuiConfig {
        default_view_mode: chimera_tui::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    }
}

/// 构造 25ms tick + 1s Timeline 间隔的实时管道
fn make_pipeline(bus: &EventBus) -> Arc<DataPipeline> {
    Arc::new(DataPipeline::new(
        EventSubscriber::new(bus.clone()),
        DataSourceConfig {
            tick_interval_ms: 25,
            snapshot_interval_s: 1,
            ..Default::default()
        },
    ))
}

/// 发布一条 OSA 稀疏度事件
fn osa_event(sparsity: f32) -> NexusEvent {
    NexusEvent::OmniSparseMasksComputed {
        metadata: EventMetadata::new("osa-coordinator"),
        mask_hash: "test-mask".into(),
        sparsity,
        context_mask: vec!["file1.rs".into(), "file2.rs".into()],
    }
}

/// 发布一条 CLV 快照事件
fn clv_event(l2_norm: f32) -> NexusEvent {
    NexusEvent::ClvSnapshotReported {
        metadata: EventMetadata::new("nmc-encoder"),
        modality: "Text".into(),
        content_hash: "hash-1".into(),
        clv_summary: ClvSummary {
            block_means: vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8],
            l2_norm,
            top_dims: vec![(7, 0.8)],
        },
    }
}

/// 发布一条 HCW 召回评测事件
fn recall_event() -> NexusEvent {
    NexusEvent::HcwRecallReported {
        metadata: EventMetadata::new("hcw-window"),
        tier: "L2".into(),
        needle_recall_at_8: 0.8,
        position_bias: 0.1,
        chain_success_rate: 0.9,
        selected_count: 3,
    }
}

/// 将 TestBackend 渲染内容转为字符串(与 live_data_test 同范式)
fn render_to_string(app: &mut TuiApp, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    let buffer = terminal.backend().buffer();
    buffer
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect()
}

/// 轮询等待谓词成立,超时 panic(替代固定 sleep,避免 CI 抖动 flaky)
async fn wait_for(pred: impl Fn() -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !pred() {
        if Instant::now() >= deadline {
            panic!("timed out waiting for {what}");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ============================================================
// FC-1 核心验收:OSA / CLV / 召回读数经真实管道同步到 TuiState
// 并在面板上渲染(此前恒空)
// ============================================================

#[tokio::test]
async fn osa_clv_recall_sync_through_real_pipeline() {
    let bus = EventBus::new();
    let pipeline = make_pipeline(&bus);

    bus.publish(osa_event(0.45)).await.unwrap();
    bus.publish(clv_event(12.5)).await.unwrap();
    bus.publish(recall_event()).await.unwrap();

    // 轮询等待管道把三个事件聚合进快照
    wait_for(
        || {
            let snap = pipeline.snapshot();
            snap.osa_sparsity.is_some()
                && snap.clv_summary.is_some()
                && snap.recall_needle_at_8.is_some()
        },
        "pipeline snapshot to carry osa/clv/recall fields",
    )
    .await;

    let mut app = TuiApp::with_data_source(test_config(), Box::new(Arc::clone(&pipeline))).unwrap();
    app.update();

    // --- 字段同步断言(DataSnapshot → TuiState) ---
    {
        let st = app.state();
        assert_eq!(
            st.osa_sparsity,
            Some(0.45),
            "osa_sparsity 应同步进 TuiState"
        );
        assert_eq!(
            st.osa_context_mask,
            vec!["file1.rs".to_string(), "file2.rs".to_string()],
            "osa_context_mask 应同步进 TuiState"
        );
        assert!(
            !st.osa_sparsity_history.is_empty(),
            "osa_sparsity_history 应同步进 TuiState"
        );
        assert_eq!(st.recall_needle_at_8, Some(0.8), "召回读数应同步");
        assert_eq!(st.recall_position_bias, Some(0.1), "位置偏置应同步");
        assert_eq!(st.recall_chain_success, Some(0.9), "链路成功率应同步");
        let clv = st
            .clv_summary
            .as_ref()
            .expect("clv_summary 应同步进 TuiState");
        assert_eq!(clv.l2_norm, 12.5);
    }

    // --- 面板渲染断言(FC-1 验收:三面板不再恒空) ---
    // WHY 120x40:OsaSparse 四段布局需 inner≥18 行(3+4+Min(5)+6),
    // 100x30 下掩码区被压缩到 3 行,召回行会被静默裁掉(布局问题另案记录)。
    app.switch_panel_to(PanelId::OsaSparse);
    let content = render_to_string(&mut app, 120, 40);
    assert!(
        content.contains("45.0%"),
        "OsaSparse 面板应渲染实时稀疏度 45.0%,got: {}",
        &content[..content.len().min(400)]
    );
    // 召回读数行(此前恒为 N/A):Recall: needle@8=0.800 bias=0.100 chain=0.900
    assert!(
        content.contains("Recall: needle@8=0.800") && content.contains("bias=0.100"),
        "OsaSparse 面板应渲染 HCW 召回读数,got: {}",
        &content[..content.len().min(400)]
    );

    app.switch_panel_to(PanelId::ClvVector);
    let content = render_to_string(&mut app, 120, 40);
    assert!(
        content.contains("L2 Norm: 12.5000"),
        "ClvVector 面板应渲染实时 L2 范数,got: {}",
        &content[..content.len().min(400)]
    );

    pipeline.shutdown().await;
}

// ============================================================
// FC-1 验收:Timeline 周期快照经真实管道同步并在面板渲染
// ============================================================

#[tokio::test]
async fn timeline_snapshots_sync_through_real_pipeline() {
    let bus = EventBus::new();
    let pipeline = make_pipeline(&bus);

    // 任一事件即可让 Timeline 快照携带非零 event_count
    bus.publish(osa_event(0.5)).await.unwrap();

    // snapshot_interval_s = 1:管道启动约 1s 后首个 TimelineSnapshot 生成
    wait_for(
        || !pipeline.snapshot().timeline_snapshots.is_empty(),
        "first TimelineSnapshot to be produced",
    )
    .await;

    let mut app = TuiApp::with_data_source(test_config(), Box::new(Arc::clone(&pipeline))).unwrap();
    app.update();

    assert!(
        !app.state().timeline_snapshots.is_empty(),
        "timeline_snapshots 应同步进 TuiState"
    );

    app.switch_panel_to(PanelId::Timeline);
    let content = render_to_string(&mut app, 120, 40);
    assert!(
        content.contains("of ") && content.contains("/100"),
        "Timeline 面板应渲染快照条目(计数与健康分),got: {}",
        &content[..content.len().min(400)]
    );

    pipeline.shutdown().await;
}

// ============================================================
// FC-1 验收:OSA 数据变化时 OsaSparse 面板被标记 dirty(P4.1)
// ============================================================

#[tokio::test]
async fn osa_panel_marked_dirty_on_sparsity_change() {
    let bus = EventBus::new();
    let pipeline = make_pipeline(&bus);

    bus.publish(osa_event(0.45)).await.unwrap();
    wait_for(
        || pipeline.snapshot().osa_sparsity == Some(0.45),
        "first sparsity sample",
    )
    .await;

    let mut app = TuiApp::with_data_source(test_config(), Box::new(Arc::clone(&pipeline))).unwrap();
    app.update();
    // 渲染一次以清空 dirty 集合(render 末尾 clear_dirty)
    let _ = render_to_string(&mut app, 100, 30);
    assert!(
        !app.state().is_dirty(PanelId::OsaSparse),
        "渲染后 dirty 应已清空"
    );

    // 新稀疏度事件 → 下一次 update 应把 OsaSparse 标 dirty
    bus.publish(osa_event(0.60)).await.unwrap();
    wait_for(
        || pipeline.snapshot().osa_sparsity == Some(0.60),
        "second sparsity sample",
    )
    .await;
    app.update();

    assert!(
        app.state().is_dirty(PanelId::OsaSparse),
        "稀疏度变化后 OsaSparse 应被标记 dirty"
    );
    // 反例选 Quest(budget_history 每 tick 追加属合理变化,Budget 恒标脏):
    // quest_list 未被本测试事件触碰,不应被误标
    assert!(
        !app.state().is_dirty(PanelId::Quest),
        "未变化的 Quest 不应被误标 dirty"
    );

    pipeline.shutdown().await;
}
