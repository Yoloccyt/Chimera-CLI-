//! TUI McpNodes 面板 — MCP 节点状态与心跳(P2.4)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 数据来源:L10 mcp-mesh 通过 `NexusEvent::McpNodeHeartbeat` 发布,
//!   经 `McpNodesSync` 同步到 `TuiState.mcp_nodes`。
//! - 布局:垂直线性 — 离线告警横幅(若有)→ 节点列表 → FOOTER。
//!   线性布局而非分栏:节点列表行数不固定,垂直排列最大化信息密度。
//! - 节点状态三态着色与符号(spec P2.4.3 要求):
//!   - Online   绿色 `●`(实心圆:正常运转)
//!   - Degraded 黄色 `▲`(三角:警告,提示性能受限)
//!   - Offline  红色 `✗`(叉号:完全不可达)
//!     WHY 三种符号而非全用 `●`:色盲用户无法依赖颜色区分 Online/Degraded,
//!     符号差异提供第二层视觉线索。`▲` 与 `✗` 的形状对比比颜色对比更显著。
//! - 离线告警横幅:任何节点 `status == Offline` 或 `last_seen > 5s` 时,
//!   顶部显示红色 `[ALERT]` 横幅。WHY 顶部位置:故障信息必须不被列表滚动
//!   淹没,操作员无论滚动到何处都能看到告警(告警在内容最上方,渲染时
//!   随 scroll 一起滚动,但通常告警时节点数少,不会触发滚动)。
//!
//! # 心跳超时阈值(5s)— SubTask P2.4.6 WHY 注释
//! 5s 阈值的选择依据:平衡误报率与故障感知速度(分布式系统心跳惯例)。
//! - mcp-mesh 默认心跳间隔 2s,5s 允许 2 次心跳丢失的容错窗口
//! - 低于 3s:跨区域 MCP Mesh 网络抖动易误报(单次 ACK 超时即告警)
//! - 高于 10s:故障感知过慢,影响 SLA(用户等待故障转移时间过长)
//! - 5s 在多数广域网环境下能容忍短暂抖动,同时保证 5-10s 内感知故障,
//!   与 Redis/Gossip 等成熟分布式系统的默认心跳间隔(1-5s)对齐
//! - 参考:MCP Mesh 设计文档 "心跳间隔 = 2s,超时 = 2.5 × 间隔 = 5s"
use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::list_state;
use crate::panels::Panel;
use crate::popup::PopupKind;
use crate::render::FOOTER_TEXT;
use crate::types::{McpNodeStatus, NodeStatus, PanelId, TuiCommand, TuiState};

/// 心跳超时阈值(秒)— 超过此时间未收到心跳的节点视为离线。
///
/// WHY 5s:mcp-mesh 默认心跳间隔 2s,5s 允许 2 次丢失的容错窗口。
/// 低于 3s 易因网络抖动误报,高于 10s 故障感知过慢影响 SLA。
/// 详见模块级文档的"心跳超时阈值"章节。
///
/// WHY pub:供集成测试验证阈值已定义且符合分布式系统惯例(5s)。
pub const HEARTBEAT_TIMEOUT_SECS: i64 = 5;

/// McpNodes MCP 节点状态面板 — 数据驱动渲染节点列表与离线告警
///
/// 消费 `TuiState.mcp_nodes`(由 `McpNodesSync` 从 `McpNodeHeartbeat` 事件维护)。
/// 支持上下方向键导航、滚轮滚动、Enter 打开节点详情弹窗。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct McpNodesPanel {
    /// 当前选中节点的索引
    selected: usize,
    /// 节点列表的滚动偏移
    scroll_offset: usize,
}

impl McpNodesPanel {
    /// 创建新的 McpNodes 面板
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回当前选中项索引(测试与外部观察用)
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// 判断节点是否已心跳超时(last_seen 距 now 超过 HEARTBEAT_TIMEOUT_SECS)。
    ///
    /// WHY 提取为独立函数:超时判定在告警横幅、节点行渲染、详情弹窗中均需使用,
    /// 统一入口避免散落的魔法数字与不一致判定。
    /// `last_seen == None`(从未收到心跳)也视为超时。
    fn is_timed_out(node: &McpNodeStatus, now: DateTime<Utc>) -> bool {
        match node.last_seen {
            Some(last_seen) => (now - last_seen).num_seconds() > HEARTBEAT_TIMEOUT_SECS,
            // 从未收到心跳,视为超时
            None => true,
        }
    }

    /// 判断节点是否应被视为离线(status == Offline 或心跳超时)。
    ///
    /// WHY 双重判定:即使心跳事件中 status 字段为 "online",
    /// 若 last_seen 超过 5s 也应视为离线(节点可能已静默挂掉)。
    fn is_offline(node: &McpNodeStatus, now: DateTime<Utc>) -> bool {
        node.status == NodeStatus::Offline || Self::is_timed_out(node, now)
    }

    /// 返回节点状态对应的显示颜色。
    ///
    /// WHY pub 函数:颜色映射在节点列表与告警横幅两处使用,提取为函数避免散落
    /// 魔法值;同时便于集成测试直接验证语义色定义(Online=绿/Degraded=黄/Offline=红)。
    ///
    /// WHY 交通灯语义:绿(正常)/黄(警告)/红(故障)是工业标准,
    /// 操作员无需培训即可理解。色盲用户可通过 `●`/`▲`/`✗` 符号区分。
    pub fn status_color(status: NodeStatus) -> Color {
        match status {
            NodeStatus::Online => Color::Green,
            NodeStatus::Degraded => Color::Yellow,
            NodeStatus::Offline => Color::Red,
        }
    }

    /// 返回节点状态对应的符号(spec P2.4.3 要求)。
    ///
    /// - Online=●(实心圆:正常运转)
    /// - Degraded=▲(三角:警告,提示性能受限)
    /// - Offline=✗(叉号:完全不可达)
    ///
    /// WHY 三种符号而非全用 `●`:色盲用户无法依赖颜色区分 Online/Degraded,
    /// 符号差异提供第二层视觉线索。`▲` 与 `✗` 的形状对比比颜色对比更显著。
    fn status_symbol(status: NodeStatus) -> &'static str {
        match status {
            NodeStatus::Online => "●",
            NodeStatus::Degraded => "▲",
            NodeStatus::Offline => "✗",
        }
    }

    /// 格式化 last_seen 为人类可读的相对时间格式(如 "2s ago", "5m ago")。
    ///
    /// WHY 独立函数:相对时间在节点行与详情弹窗两处使用,统一格式化逻辑。
    /// `None` 时返回 "never"(尚未收到心跳)。
    fn format_last_seen(last_seen: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
        match last_seen {
            Some(ts) => {
                let delta = now.signed_duration_since(ts);
                let secs = delta.num_seconds().max(0);
                if secs < 60 {
                    format!("{}s ago", secs)
                } else if secs < 3600 {
                    format!("{}m ago", secs / 60)
                } else if secs < 86400 {
                    format!("{}h ago", secs / 3600)
                } else {
                    format!("{}d ago", secs / 86400)
                }
            }
            None => "never".to_string(),
        }
    }

    /// 构建面板文本内容(离线告警 + 节点列表)。
    ///
    /// WHY 独立 pub 方法:与 LogPanel/QuestPanel/DecayPanel 模式一致,
    /// 便于单元测试与集成测试无需 TestBackend 即可验证文本输出与样式。
    ///
    /// # 布局
    /// 1. 离线告警横幅(若有 Offline 节点)— 红色加粗,每节点一行
    /// 2. 节点列表:每节点一行,格式 `[icon] node_id  throughput msg/s  last_seen: Xs ago`
    /// 3. FOOTER_TEXT
    ///
    /// `selected` 参数用于高亮当前选中行(黄色背景 + 黑色前景 + 加粗),
    /// 选中状态比状态颜色更高优先级(参考 LogPanel/ParliamentPanel)。
    pub fn content(state: &TuiState, selected: usize) -> Text<'static> {
        let now = Utc::now();
        let nodes = &state.mcp_nodes;
        let mut lines: Vec<Line<'static>> = Vec::new();

        // 1. 离线告警横幅:任何节点 Offline 或心跳超时时显示
        // WHY 顶部位置:故障信息必须最先被操作员看到,不被列表滚动淹没
        let offline_nodes: Vec<&McpNodeStatus> =
            nodes.iter().filter(|n| Self::is_offline(n, now)).collect();
        if !offline_nodes.is_empty() {
            for node in &offline_nodes {
                lines.push(Line::from(Span::styled(
                    format!("[ALERT] Node {} offline", node.node_id),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )));
            }
            lines.push(Line::from(""));
        }

        // 2. 节点列表(spec P2.4.3 格式:`[icon] node_id  1250 msg/s  last_seen: 2s ago`)
        if nodes.is_empty() {
            lines.push(Line::from(Span::styled(
                "No MCP nodes connected",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            for (idx, node) in nodes.iter().enumerate().take(50) {
                let is_selected = idx == selected;
                let color = McpNodesPanel::status_color(node.status);
                let icon = Self::status_symbol(node.status);
                let last_seen_str = Self::format_last_seen(node.last_seen, now);

                // WHY 选中行样式:黄色背景 + 黑色前景 + 加粗,
                // 与 LogPanel/QuestPanel 保持一致,选中状态比状态颜色更高优先级。
                // 非选中行保留状态颜色(绿/黄/红)。
                let style = if is_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(color)
                };

                let prefix = if is_selected { "> " } else { "  " };

                // 节点行格式:`[icon] node_id  throughput msg/s  last_seen: Xs ago`
                let line_text = format!(
                    "{prefix}[{icon}] {}  {} msg/s  last_seen: {}",
                    node.node_id, node.throughput, last_seen_str
                );
                lines.push(Line::from(Span::styled(line_text, style)));
            }
        }

        // 3. FOOTER
        lines.push(Line::from(""));
        lines.push(Line::from(FOOTER_TEXT));

        Text::from(lines)
    }

    /// 构建选中节点的详情弹窗内容。
    ///
    /// 显示节点的完整状态信息,包括心跳年龄与超时警告。
    fn detail_content(node: &McpNodeStatus) -> String {
        let now = Utc::now();
        let mut lines = vec![
            format!("Node ID: {}", node.node_id),
            format!("Status: {:?}", node.status),
            format!("Throughput: {} msg/s", node.throughput),
        ];

        match node.last_seen {
            Some(last_seen) => {
                let age = (now - last_seen).num_seconds().max(0);
                lines.push(format!(
                    "Last Seen: {}",
                    last_seen.format("%Y-%m-%d %H:%M:%S UTC")
                ));
                lines.push(format!("Heartbeat Age: {}s", age));
                // WHY 超时警告:操作员需知道节点是否已超时,以便排查网络/进程问题
                if age > HEARTBEAT_TIMEOUT_SECS {
                    lines.push(format!(
                        "Warning: Heartbeat timed out (>{}s threshold)",
                        HEARTBEAT_TIMEOUT_SECS
                    ));
                }
            }
            None => {
                lines.push("Last Seen: (never)".into());
                lines.push("Warning: No heartbeat received".into());
            }
        }

        lines.join("\n")
    }
}

impl Panel for McpNodesPanel {
    fn id(&self) -> PanelId {
        PanelId::McpNodes
    }

    fn title(&self) -> Line<'static> {
        Line::from(" MCP Nodes ")
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let count = state.mcp_nodes.len();
        self.selected = list_state::clamp_selected(self.selected, count);

        let block = Block::default().borders(Borders::ALL).title(" MCP Nodes ");
        let inner = block.inner(area);
        block.render(area, buf);

        // WHY content_height = inner.height - 3:预留 footer(2行)+ 边距(1行)
        // 与 LogPanel/QuestPanel 的滚动计算保持一致
        let content_height = inner.height.saturating_sub(3) as usize;
        self.scroll_offset =
            list_state::adjust_scroll(self.selected, self.scroll_offset, content_height);

        let paragraph = Paragraph::new(McpNodesPanel::content(state, self.selected))
            .scroll((self.scroll_offset as u16, 0));
        paragraph.render(inner, buf);
    }

    fn handle_key(&mut self, key: KeyEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = state.mcp_nodes.len();
        if let Some(new_selected) =
            list_state::handle_key_navigation(key.code, self.selected, count)
        {
            self.selected = new_selected;
            return None;
        }

        match key.code {
            KeyCode::Enter => {
                if let Some(node) = state.mcp_nodes.get(self.selected) {
                    let content = McpNodesPanel::detail_content(node);
                    Some(TuiCommand::OpenPopup(PopupKind::Detail {
                        title: format!("Node {} Detail", node.node_id),
                        content,
                        scroll: 0,
                    }))
                } else {
                    None
                }
            }
            KeyCode::Char('g') => {
                self.scroll_to_top(state);
                None
            }
            KeyCode::Char('G') => {
                self.scroll_to_bottom(state);
                None
            }
            // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
            _ => None,
        }
    }

    fn scroll_to_top(&mut self, _state: &mut TuiState) {
        self.selected = 0;
        self.scroll_offset = 0;
    }

    fn scroll_to_bottom(&mut self, state: &mut TuiState) {
        let count = state.mcp_nodes.len();
        self.selected = if count == 0 { 0 } else { count - 1 };
        self.scroll_offset = self.selected;
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = state.mcp_nodes.len();
        if let Some(new_selected) =
            list_state::handle_mouse_scroll(mouse.kind, self.selected, count)
        {
            self.selected = new_selected;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    /// 构造测试用节点(last_seen 为指定的秒数前)
    fn make_node(id: &str, status: NodeStatus, throughput: u64, secs_ago: i64) -> McpNodeStatus {
        McpNodeStatus {
            node_id: id.into(),
            status,
            throughput,
            last_seen: Some(Utc::now() - chrono::Duration::seconds(secs_ago)),
        }
    }

    // WHY 使用 `McpNodesPanel::` 而非 `Self::`:测试模块不是 impl 块,
    // `Self` 在 mod tests 中不可用,必须显式指定类型名。

    #[test]
    fn test_mcp_nodes_panel_id() {
        let panel = McpNodesPanel::new();
        assert_eq!(panel.id(), PanelId::McpNodes);
    }

    #[test]
    fn test_mcp_nodes_panel_title() {
        let panel = McpNodesPanel::new();
        let title = panel.title();
        assert_eq!(title.to_string(), " MCP Nodes ");
    }

    #[test]
    fn test_mcp_nodes_is_timed_out_fresh_heartbeat() {
        // 0s 前的心跳,未超时
        let node = make_node("n1", NodeStatus::Online, 100, 0);
        assert!(!McpNodesPanel::is_timed_out(&node, Utc::now()));
    }

    #[test]
    fn test_mcp_nodes_is_timed_out_stale_heartbeat() {
        // 10s 前的心跳,已超时(> 5s)
        let node = make_node("n1", NodeStatus::Online, 100, 10);
        assert!(McpNodesPanel::is_timed_out(&node, Utc::now()));
    }

    #[test]
    fn test_mcp_nodes_is_timed_out_never_seen() {
        // 从未收到心跳,视为超时
        let node = McpNodeStatus {
            node_id: "n1".into(),
            status: NodeStatus::Online,
            throughput: 0,
            last_seen: None,
        };
        assert!(McpNodesPanel::is_timed_out(&node, Utc::now()));
    }

    #[test]
    fn test_mcp_nodes_is_offline_status_offline() {
        // status == Offline,无论 last_seen 如何都是离线
        let node = make_node("n1", NodeStatus::Offline, 0, 0);
        assert!(McpNodesPanel::is_offline(&node, Utc::now()));
    }

    #[test]
    fn test_mcp_nodes_is_offline_status_online_no_timeout() {
        // status == Online 且心跳新鲜,不是离线
        let node = make_node("n1", NodeStatus::Online, 100, 0);
        assert!(!McpNodesPanel::is_offline(&node, Utc::now()));
    }

    #[test]
    fn test_mcp_nodes_is_offline_status_online_with_timeout() {
        // status == Online 但心跳超时,视为离线
        let node = make_node("n1", NodeStatus::Online, 100, 10);
        assert!(McpNodesPanel::is_offline(&node, Utc::now()));
    }

    #[test]
    fn test_mcp_nodes_status_color_mapping() {
        assert_eq!(
            McpNodesPanel::status_color(NodeStatus::Online),
            Color::Green
        );
        assert_eq!(
            McpNodesPanel::status_color(NodeStatus::Degraded),
            Color::Yellow
        );
        assert_eq!(McpNodesPanel::status_color(NodeStatus::Offline), Color::Red);
    }

    #[test]
    fn test_mcp_nodes_status_symbol_mapping() {
        // spec P2.4.3:Online=●, Degraded=▲, Offline=✗
        assert_eq!(McpNodesPanel::status_symbol(NodeStatus::Online), "●");
        assert_eq!(McpNodesPanel::status_symbol(NodeStatus::Degraded), "▲");
        assert_eq!(McpNodesPanel::status_symbol(NodeStatus::Offline), "✗");
    }

    #[test]
    fn test_mcp_nodes_format_last_seen_seconds() {
        let now = Utc::now();
        let ts = now - chrono::Duration::seconds(5);
        assert_eq!(McpNodesPanel::format_last_seen(Some(ts), now), "5s ago");
    }

    #[test]
    fn test_mcp_nodes_format_last_seen_minutes() {
        let now = Utc::now();
        let ts = now - chrono::Duration::seconds(125);
        assert_eq!(McpNodesPanel::format_last_seen(Some(ts), now), "2m ago");
    }

    #[test]
    fn test_mcp_nodes_format_last_seen_never() {
        assert_eq!(McpNodesPanel::format_last_seen(None, Utc::now()), "never");
    }

    #[test]
    fn test_mcp_nodes_content_empty_state() {
        let state = TuiState::new();
        let content = McpNodesPanel::content(&state, 0).to_string();
        assert!(content.contains("No MCP nodes connected"));
        assert!(!content.contains("[ALERT]"));
    }

    #[test]
    fn test_mcp_nodes_content_with_nodes() {
        let mut state = TuiState::new();
        state.mcp_nodes = vec![
            make_node("node-1", NodeStatus::Online, 150, 0),
            make_node("node-2", NodeStatus::Degraded, 45, 0),
        ];
        let content = McpNodesPanel::content(&state, 0).to_string();
        assert!(content.contains("node-1"));
        assert!(content.contains("node-2"));
        assert!(content.contains("150"));
        assert!(content.contains("msg/s"));
        assert!(content.contains("last_seen:"));
        assert!(content.contains("ago"));
    }

    #[test]
    fn test_mcp_nodes_content_offline_alert() {
        let mut state = TuiState::new();
        state.mcp_nodes = vec![make_node("down", NodeStatus::Offline, 0, 10)];
        let content = McpNodesPanel::content(&state, 0).to_string();
        assert!(content.contains("[ALERT]"));
        assert!(content.contains("down"));
    }

    #[test]
    fn test_mcp_nodes_content_uses_status_symbols() {
        let mut state = TuiState::new();
        state.mcp_nodes = vec![
            make_node("online-node", NodeStatus::Online, 100, 0),
            make_node("degraded-node", NodeStatus::Degraded, 50, 0),
            make_node("offline-node", NodeStatus::Offline, 0, 0),
        ];
        let content = McpNodesPanel::content(&state, 0).to_string();
        // 三种符号都应出现(spec P2.4.3)
        assert!(content.contains('●'), "Online should use ● symbol");
        assert!(content.contains('▲'), "Degraded should use ▲ symbol");
        assert!(content.contains('✗'), "Offline should use ✗ symbol");
    }

    #[test]
    fn test_mcp_nodes_detail_content_online() {
        let node = make_node("node-1", NodeStatus::Online, 200, 0);
        let content = McpNodesPanel::detail_content(&node);
        assert!(content.contains("node-1"));
        assert!(content.contains("Online"));
        assert!(content.contains("200"));
        assert!(content.contains("Last Seen"));
        assert!(content.contains("Heartbeat Age"));
        // 心跳未超时,不应有 Warning
        assert!(!content.contains("Warning"));
    }

    #[test]
    fn test_mcp_nodes_detail_content_timeout_warning() {
        let node = make_node("node-1", NodeStatus::Online, 100, 10);
        let content = McpNodesPanel::detail_content(&node);
        assert!(
            content.contains("Warning"),
            "timeout node detail should contain warning"
        );
    }

    #[test]
    fn test_mcp_nodes_handle_key_enter_empty_returns_none() {
        let mut panel = McpNodesPanel::new();
        let mut state = TuiState::new();
        let key = KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE);
        assert!(panel.handle_key(key, &mut state).is_none());
    }

    #[test]
    fn test_mcp_nodes_handle_key_question_mark_returns_none() {
        let mut panel = McpNodesPanel::new();
        let mut state = TuiState::new();
        let key = KeyEvent::new(KeyCode::Char('?'), crossterm::event::KeyModifiers::NONE);
        // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
        assert_eq!(panel.handle_key(key, &mut state), None);
    }

    #[test]
    fn test_mcp_nodes_heartbeat_timeout_constant_value() {
        // 5s 心跳超时阈值:分布式系统惯例
        assert_eq!(HEARTBEAT_TIMEOUT_SECS, 5);
    }
}
