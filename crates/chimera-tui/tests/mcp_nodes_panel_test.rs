//! McpNodes 面板集成测试 — P2.4 TUI v1.7-omega
//!
//! 验证 McpNodesPanel 正确渲染节点列表、状态颜色映射、吞吐量数值,
//! 离线告警横幅,以及空状态占位文本。
//!
//! # 设计决策(WHY)
//! - 使用 TestBackend 内存渲染,无需真实终端,CI 可运行。
//! - 颜色映射通过 `McpNodesPanel::content` 返回的 `Text` 中 `Span` 的样式
//!   直接验证,因为 TestBackend 字符串提取会丢失颜色信息。
//! - 5s 心跳超时阈值在 mcp_nodes.rs 的 `HEARTBEAT_TIMEOUT_SECS` 常量中定义,
//!   作为 future client-side 超时检测的参考阈值(当前面板基于事件载荷的
//!   status 字段判定,不依赖客户端时间差)。

#![forbid(unsafe_code)]

use chimera_tui::{
    DataSnapshot, DataSourceConfig, McpNodeStatus, McpNodesPanel, NodeStatus, Panel, PanelId,
    TuiApp, TuiCommand, TuiConfig, TuiDataSource, TuiError, TuiState,
};
use chrono::{Duration, Utc};
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

// ============================================================
// 测试数据源 — 返回预设 MCP 节点快照
// ============================================================

#[derive(Debug)]
struct McpNodesTestSource {
    snapshot: DataSnapshot,
    config: DataSourceConfig,
}

impl McpNodesTestSource {
    fn new(snapshot: DataSnapshot) -> Self {
        Self {
            snapshot,
            config: DataSourceConfig::default(),
        }
    }
}

impl TuiDataSource for McpNodesTestSource {
    fn snapshot(&self) -> Result<DataSnapshot, TuiError> {
        Ok(self.snapshot.clone())
    }

    fn config(&self) -> &DataSourceConfig {
        &self.config
    }
}

/// 渲染应用并返回字符串内容
fn render_to_string(app: &mut TuiApp, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect()
}

/// 构造正常节点快照:1 Online + 1 Degraded,无 Offline
fn normal_nodes_snapshot() -> DataSnapshot {
    let now = Utc::now();
    DataSnapshot {
        mcp_nodes: vec![
            McpNodeStatus {
                node_id: "mcp-node-1".into(),
                status: NodeStatus::Online,
                throughput: 1250,
                last_seen: Some(now),
            },
            McpNodeStatus {
                node_id: "mcp-node-2".into(),
                status: NodeStatus::Degraded,
                throughput: 450,
                last_seen: Some(now - Duration::seconds(3)),
            },
        ],
        ..Default::default()
    }
}

/// 构造含离线节点的快照:1 Online + 1 Offline(应触发告警横幅)
fn offline_node_snapshot() -> DataSnapshot {
    let now = Utc::now();
    DataSnapshot {
        mcp_nodes: vec![
            McpNodeStatus {
                node_id: "mcp-node-online".into(),
                status: NodeStatus::Online,
                throughput: 800,
                last_seen: Some(now),
            },
            McpNodeStatus {
                node_id: "mcp-node-down".into(),
                status: NodeStatus::Offline,
                throughput: 0,
                last_seen: Some(now - Duration::seconds(30)),
            },
        ],
        ..Default::default()
    }
}

/// 构造多离线节点快照:验证告警横幅只显示第一个 Offline 节点
fn multiple_offline_snapshot() -> DataSnapshot {
    let now = Utc::now();
    DataSnapshot {
        mcp_nodes: vec![
            McpNodeStatus {
                node_id: "down-1".into(),
                status: NodeStatus::Offline,
                throughput: 0,
                last_seen: Some(now - Duration::seconds(60)),
            },
            McpNodeStatus {
                node_id: "down-2".into(),
                status: NodeStatus::Offline,
                throughput: 0,
                last_seen: Some(now - Duration::seconds(90)),
            },
        ],
        ..Default::default()
    }
}

// ============================================================
// A. 基础渲染测试 — 面板 ID 与标题
// ============================================================

#[test]
fn test_mcp_nodes_panel_id() {
    let mut app = TuiApp::new(TuiConfig::default()).unwrap();
    app.switch_panel_to(PanelId::McpNodes);
    assert_eq!(app.current_panel(), PanelId::McpNodes);
}

#[test]
fn test_mcp_nodes_panel_renders_title() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(McpNodesTestSource::new(normal_nodes_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::McpNodes);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("MCP Nodes"),
        "MCP Nodes panel title should be rendered, got: {}",
        &content[..content.len().min(200)]
    );
}

// ============================================================
// B. 节点列表渲染测试 — SubTask P2.4.1 (1)(3)
// ============================================================

#[test]
fn test_mcp_nodes_panel_renders_node_list_when_non_empty() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(McpNodesTestSource::new(normal_nodes_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::McpNodes);

    let content = render_to_string(&mut app, 100, 30);
    assert!(
        content.contains("mcp-node-1"),
        "node 'mcp-node-1' should be rendered, got: {}",
        &content[..content.len().min(300)]
    );
    assert!(
        content.contains("mcp-node-2"),
        "node 'mcp-node-2' should be rendered"
    );
}

#[test]
fn test_mcp_nodes_panel_renders_throughput_value() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(McpNodesTestSource::new(normal_nodes_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::McpNodes);

    let content = render_to_string(&mut app, 100, 30);
    assert!(
        content.contains("1250"),
        "throughput value 1250 should be rendered, got: {}",
        &content[..content.len().min(300)]
    );
    assert!(
        content.contains("450"),
        "throughput value 450 should be rendered"
    );
}

#[test]
fn test_mcp_nodes_panel_renders_msg_per_second_unit() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(McpNodesTestSource::new(normal_nodes_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::McpNodes);

    let content = render_to_string(&mut app, 100, 30);
    assert!(
        content.contains("msg/s"),
        "throughput unit 'msg/s' should be rendered, got: {}",
        &content[..content.len().min(300)]
    );
}

#[test]
fn test_mcp_nodes_panel_renders_last_seen_relative_time() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(McpNodesTestSource::new(normal_nodes_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::McpNodes);

    let content = render_to_string(&mut app, 100, 30);
    assert!(
        content.contains("ago"),
        "last_seen should be rendered as relative time with 'ago' suffix, got: {}",
        &content[..content.len().min(300)]
    );
}

// ============================================================
// C. 离线告警横幅测试 — SubTask P2.4.1 (4)
// ============================================================

#[test]
fn test_mcp_nodes_panel_offline_alert_banner() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(McpNodesTestSource::new(offline_node_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::McpNodes);

    let content = render_to_string(&mut app, 100, 30);
    assert!(
        content.contains("[ALERT]"),
        "offline alert banner should be rendered, got: {}",
        &content[..content.len().min(300)]
    );
    assert!(
        content.contains("mcp-node-down"),
        "offline node 'mcp-node-down' should appear in alert banner"
    );
    assert!(
        content.contains("offline"),
        "alert should mention 'offline' keyword"
    );
}

#[test]
fn test_mcp_nodes_panel_no_alert_when_all_online() {
    let now = Utc::now();
    let snapshot = DataSnapshot {
        mcp_nodes: vec![
            McpNodeStatus {
                node_id: "n1".into(),
                status: NodeStatus::Online,
                throughput: 100,
                last_seen: Some(now),
            },
            McpNodeStatus {
                node_id: "n2".into(),
                status: NodeStatus::Degraded,
                throughput: 50,
                last_seen: Some(now),
            },
        ],
        ..Default::default()
    };
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(McpNodesTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::McpNodes);

    let content = render_to_string(&mut app, 100, 30);
    assert!(
        !content.contains("[ALERT]"),
        "no alert banner when no Offline nodes, got: {}",
        &content[..content.len().min(300)]
    );
}

#[test]
fn test_mcp_nodes_panel_alert_shows_first_offline_node() {
    // 多个 Offline 节点时,告警横幅应显示第一个 Offline 节点
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(McpNodesTestSource::new(multiple_offline_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::McpNodes);

    let content = render_to_string(&mut app, 100, 30);
    assert!(
        content.contains("[ALERT]"),
        "alert should be rendered for multiple Offline nodes"
    );
    assert!(
        content.contains("down-1"),
        "first Offline node 'down-1' should appear in alert, got: {}",
        &content[..content.len().min(300)]
    );
}

// ============================================================
// D. 空状态测试 — SubTask P2.4.1 (5)
// ============================================================

#[test]
fn test_mcp_nodes_panel_empty_state_shows_placeholder() {
    let snapshot = DataSnapshot::default();
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(McpNodesTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::McpNodes);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("No MCP nodes connected"),
        "empty state should show placeholder, got: {}",
        &content[..content.len().min(300)]
    );
    assert!(
        !content.contains("[ALERT]"),
        "empty state should not show alert banner"
    );
}

// ============================================================
// E. 状态颜色映射测试 — SubTask P2.4.1 (2)
// ============================================================

#[test]
fn test_mcp_nodes_panel_status_color_online_green() {
    assert_eq!(
        McpNodesPanel::status_color(NodeStatus::Online),
        Color::Green
    );
}

#[test]
fn test_mcp_nodes_panel_status_color_degraded_yellow() {
    assert_eq!(
        McpNodesPanel::status_color(NodeStatus::Degraded),
        Color::Yellow
    );
}

#[test]
fn test_mcp_nodes_panel_status_color_offline_red() {
    assert_eq!(McpNodesPanel::status_color(NodeStatus::Offline), Color::Red);
}

#[test]
fn test_mcp_nodes_panel_content_uses_status_colors() {
    // 通过 content 方法返回的 Text 验证 Span 样式(因为 TestBackend 字符流丢失颜色)
    let mut state = TuiState::new();
    state.mcp_nodes = vec![
        McpNodeStatus {
            node_id: "online-node".into(),
            status: NodeStatus::Online,
            throughput: 100,
            last_seen: Some(Utc::now()),
        },
        McpNodeStatus {
            node_id: "degraded-node".into(),
            status: NodeStatus::Degraded,
            throughput: 50,
            last_seen: Some(Utc::now()),
        },
        McpNodeStatus {
            node_id: "offline-node".into(),
            status: NodeStatus::Offline,
            throughput: 0,
            last_seen: Some(Utc::now()),
        },
    ];
    // WHY selected=usize::MAX:本测试目标是验证状态色映射(Online=Green / Degraded=Yellow / Offline=Red),
    // 必须让所有节点都处于"非选中"状态。若 selected=0,则 idx=0 的 online-node 会进入选中样式
    // (Black on Yellow + Bold),覆盖状态色,导致 found_green 永远为 false。
    // 选中样式与状态色的优先级关系由 mcp_nodes.rs 中 content 方法的设计决定:
    // 选中状态 > 状态颜色(参考 LogPanel/QuestPanel 选中行优先的约定)。
    let text = McpNodesPanel::content(&state, usize::MAX);
    let content_str = text.to_string();

    // 文本应包含所有节点 ID
    assert!(
        content_str.contains("online-node"),
        "Online node should be listed"
    );
    assert!(
        content_str.contains("degraded-node"),
        "Degraded node should be listed"
    );
    assert!(
        content_str.contains("offline-node"),
        "Offline node should be listed"
    );

    // 验证颜色样式:遍历所有 Span,收集对应节点行的样式
    // 选中状态优先于状态颜色(与 LogPanel/QuestPanel 一致),因此测试时使用
    // selected=usize::MAX 让所有节点都非选中,以验证状态色映射本身正确。
    let mut found_green = false;
    let mut found_yellow = false;
    let mut found_red = false;
    let mut found_alert_red = false;
    for line in text.lines.iter() {
        for span in line.spans.iter() {
            let style = span.style;
            let span_text = span.content.as_ref();
            // 节点行(非选中)的图标 / 文本颜色应与状态对应
            if span_text.contains("online-node") && style.fg == Some(Color::Green) {
                found_green = true;
            }
            if span_text.contains("degraded-node") && style.fg == Some(Color::Yellow) {
                found_yellow = true;
            }
            if span_text.contains("offline-node") && style.fg == Some(Color::Red) {
                found_red = true;
            }
            // 离线告警横幅应使用 Red + Bold
            if span_text.contains("[ALERT]")
                && style.fg == Some(Color::Red)
                && style.add_modifier.contains(Modifier::BOLD)
            {
                found_alert_red = true;
            }
        }
    }
    assert!(found_green, "Online node should be styled Green");
    assert!(found_yellow, "Degraded node should be styled Yellow");
    assert!(found_red, "Offline node should be styled Red");
    assert!(found_alert_red, "Alert banner should be Red + Bold");
}

// ============================================================
// F. content 方法单元测试 — 直接验证文本输出
// ============================================================

#[test]
fn test_mcp_nodes_panel_content_with_nodes() {
    let mut state = TuiState::new();
    state.mcp_nodes = vec![McpNodeStatus {
        node_id: "node-x".into(),
        status: NodeStatus::Online,
        throughput: 999,
        last_seen: Some(Utc::now()),
    }];
    let content = McpNodesPanel::content(&state, 0).to_string();
    assert!(
        content.contains("node-x"),
        "content should contain node id, got: {}",
        &content[..content.len().min(200)]
    );
    assert!(content.contains("999"), "content should contain throughput");
    assert!(content.contains("msg/s"), "content should contain unit");
}

#[test]
fn test_mcp_nodes_panel_content_empty_state() {
    let state = TuiState::new();
    let content = McpNodesPanel::content(&state, 0).to_string();
    assert!(
        content.contains("No MCP nodes connected"),
        "empty state should show placeholder, got: {}",
        &content[..content.len().min(200)]
    );
}

#[test]
fn test_mcp_nodes_panel_content_offline_alert() {
    let mut state = TuiState::new();
    state.mcp_nodes = vec![McpNodeStatus {
        node_id: "dead-node".into(),
        status: NodeStatus::Offline,
        throughput: 0,
        last_seen: Some(Utc::now() - Duration::seconds(60)),
    }];
    let content = McpNodesPanel::content(&state, 0).to_string();
    assert!(
        content.contains("[ALERT] Node dead-node offline"),
        "alert banner should mention node id, got: {}",
        &content[..content.len().min(300)]
    );
}

// ============================================================
// G. handle_key 测试 — 导航与 ShowHelp
// ============================================================

#[test]
fn test_mcp_nodes_panel_handle_key_question_mark() {
    let mut panel = McpNodesPanel::new();
    let mut state = TuiState::new();
    state.mcp_nodes = normal_nodes_snapshot().mcp_nodes;
    let key = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('?'),
        crossterm::event::KeyModifiers::NONE,
    );
    assert_eq!(
        panel.handle_key(key, &mut state),
        Some(TuiCommand::ShowHelp)
    );
}

#[test]
fn test_mcp_nodes_panel_handle_key_other_returns_none() {
    // WHY 使用 Char('x') 而非 Enter:Enter 在有选中节点时会打开详情弹窗(返回 Some),
    // 而此测试验证的是"未处理的按键"应返回 None。Char('x') 不在任何 handle_key 分支中。
    let mut panel = McpNodesPanel::new();
    let mut state = TuiState::new();
    state.mcp_nodes = normal_nodes_snapshot().mcp_nodes;
    let key = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('x'),
        crossterm::event::KeyModifiers::NONE,
    );
    assert!(panel.handle_key(key, &mut state).is_none());
}

#[test]
fn test_mcp_nodes_panel_navigation_down() {
    let mut panel = McpNodesPanel::new();
    let mut state = TuiState::new();
    state.mcp_nodes = normal_nodes_snapshot().mcp_nodes;
    assert_eq!(panel.selected(), 0);

    panel.handle_key(
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ),
        &mut state,
    );
    assert_eq!(panel.selected(), 1, "Down should move selection to index 1");
}

#[test]
fn test_mcp_nodes_panel_navigation_up_clamps_at_zero() {
    let mut panel = McpNodesPanel::new();
    let mut state = TuiState::new();
    state.mcp_nodes = normal_nodes_snapshot().mcp_nodes;
    assert_eq!(panel.selected(), 0);

    panel.handle_key(
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::NONE,
        ),
        &mut state,
    );
    // 在索引 0 处按 Up,应保持在 0(不 wrap)
    assert_eq!(panel.selected(), 0, "Up at index 0 should stay at 0");
}

#[test]
fn test_mcp_nodes_panel_navigation_clamps_when_empty() {
    let mut panel = McpNodesPanel::new();
    let mut state = TuiState::new();
    // 空节点列表
    panel.handle_key(
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ),
        &mut state,
    );
    assert_eq!(
        panel.selected(),
        0,
        "Down on empty list should keep selection at 0"
    );
}

// ============================================================
// H. 默认状态测试 — 无数据时不应 panic
// ============================================================

#[test]
fn test_mcp_nodes_panel_default_state_renders() {
    let snapshot = DataSnapshot::default();
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(McpNodesTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::McpNodes);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("MCP Nodes"),
        "panel should render title in default state"
    );
    assert!(
        content.contains("No MCP nodes connected"),
        "default state should show empty placeholder"
    );
}

// ============================================================
// I. 心跳超时常量测试 — 验证 5s 阈值已定义
// ============================================================

#[test]
fn test_mcp_nodes_panel_heartbeat_timeout_constant() {
    // 5s 心跳超时阈值:平衡误报率与故障感知速度
    assert_eq!(
        chimera_tui::panels::mcp_nodes::HEARTBEAT_TIMEOUT_SECS,
        5,
        "heartbeat timeout should be 5 seconds (distributed systems convention)"
    );
}
