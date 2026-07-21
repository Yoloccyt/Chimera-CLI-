//! TUI Chtc 面板 — CHTC 跨平台适配器兼容性评分(P2.5)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 消费 `TuiState::chtc_state` 渲染 5 IDE 适配器(VSCode/JetBrains/Vim/Emacs/LSP)
//!   的兼容性评分、在线状态与最近请求类型分布。
//! - 5 IDE 适配器列表与 chtc-bridge 的 enum dispatch 设计一致(§4.3),
//!   覆盖主流编辑器生态。适配器类型字符串由事件 payload 提供,不在此处硬编码,
//!   以兼容 chtc-bridge 后续扩展(如 Zed 替代 LSP 时无需修改面板)。
//! - 兼容性评分 < 60 黄色高亮:60 分为常规百分制及格线,低于此值意味着适配器
//!   在该 IDE 上存在显著兼容性问题(API 缺失/格式转换失败率高),需运维关注。
//!   >= 80 绿色表示运行良好。60-79 用 Color::Reset(默认前景色),避免视觉噪音。
//! - 最近请求类型分布采用文本列表 `type(count) / type(count)` 而非 sparkline:
//!   请求类型是离散字符串(如 "tool_call"/"hover"),sparkline 适合时序数值,
//!   离散类别用文本更直观。P6 可选阶段可升级为柱状图。
//! - 评分进度条 `[====------]` 复用 `render::utilization_bar` 的视觉风格但内联构建,
//!   以便与 `score/100` 文本同行显示,避免重复百分比信息。
//! - 数据来源:L10 chtc-bridge 通过 `NexusEvent::ChtcAdapterStatus` 发布,
//!   经 `ChtcSync` 同步到 `DataSnapshot.chtc_state`,再由 `TuiApp::update`
//!   写入 `TuiState.chtc_state`。
//! - 复用 `list_state` 辅助函数(clamp_selected/adjust_scroll/handle_key_navigation/
//!   handle_mouse_scroll),与 Quest/Log/Parliament/Security 面板保持导航行为一致。

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
use crate::types::{ChtcAdapterInfo, PanelId, TuiCommand, TuiState};

/// 兼容性评分"需关注"阈值:低于此值显示黄色高亮警告
///
/// WHY 60:常规百分制及格线。低于 60 意味着适配器在该 IDE 上有 > 40% 的请求
/// 出现兼容性问题(API 缺失/格式转换失败/响应超时),需运维介入。
const COMPATIBILITY_SCORE_WARNING_THRESHOLD: u8 = 60;

/// 兼容性评分"优秀"阈值:高于或等于此值显示绿色
///
/// WHY 80:80+ 表示适配器在该 IDE 上运行良好,兼容性问题率 < 20%。
const COMPATIBILITY_SCORE_GOOD_THRESHOLD: u8 = 80;

/// 评分进度条宽度(字符数)
const PROGRESS_BAR_WIDTH: usize = 30;

/// CHTC 跨平台适配器面板
///
/// 列表面板,展示 5 IDE 适配器的兼容性评分、在线状态与最近请求分布。
/// 支持上下键导航、滚轮滚动、Enter 打开详情弹窗。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ChtcPanel {
    /// 当前选中适配器索引
    selected: usize,
    /// 列表垂直滚动偏移
    scroll_offset: usize,
}

impl ChtcPanel {
    /// 创建新的 Chtc 面板
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回当前选中索引(供测试断言)
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// 根据兼容性评分返回对应的前景色
    ///
    /// WHY 公开关联函数:TestBackend 渲染会丢失 Span 样式信息,无法从字符串
    /// 内容反查颜色。将颜色映射逻辑提取为 pub fn,既能独立验证阈值逻辑,
    /// 又便于 content() 方法复用,避免逻辑分散。
    ///
    /// # 阈值映射
    /// - `>= 80`:绿色(优秀,兼容性问题率 < 20%)
    /// - `60..=79`:`Color::Reset`(正常,默认前景色,避免视觉噪音)
    /// - `< 60`:黄色(需关注,兼容性不及格)
    pub fn score_color(score: u8) -> Color {
        if score >= COMPATIBILITY_SCORE_GOOD_THRESHOLD {
            Color::Green
        } else if score >= COMPATIBILITY_SCORE_WARNING_THRESHOLD {
            Color::Reset
        } else {
            Color::Yellow
        }
    }

    /// 构建 Chtc 面板文本内容
    ///
    /// WHY 独立静态方法:与 `render` 解耦,便于单元测试直接验证文本输出,
    /// 无需启动 TestBackend。模式与 `QuestPanel::content` 一致。
    ///
    /// # 布局
    /// - 标题行:"CHTC Adapters"
    /// - 空状态:"No CHTC adapters connected" + 等待事件提示
    /// - 非空:每个适配器 3 行(ID/状态 + 进度条/评分 + 请求分布)
    /// - 页脚:FOOTER_TEXT
    pub fn content(state: &TuiState) -> Text<'static> {
        let mut lines: Vec<Line<'static>> = vec![
            Line::from("CHTC Adapters"),
            Line::from("─────────────────────────────"),
        ];

        let adapters = &state.chtc_state.adapters;

        if adapters.is_empty() {
            // 空状态:显示占位文本,避免面板内容空白
            lines.push(Line::from("No CHTC adapters connected"));
            lines.push(Line::from(""));
            lines.push(Line::from(
                "Waiting for ChtcAdapterStatus events from chtc-bridge...",
            ));
        } else {
            let total = adapters.len();
            for (idx, adapter) in adapters.iter().enumerate() {
                // 第 1 行:适配器 ID + [类型] + 在线状态
                let status_text = if adapter.is_online {
                    "online"
                } else {
                    "offline"
                };
                let status_color = if adapter.is_online {
                    Color::Green
                } else {
                    Color::Red
                };

                lines.push(Line::from(vec![
                    Span::styled(
                        adapter.adapter_id.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" [{}]  ", adapter.adapter_type),
                        Style::default().fg(Color::Gray),
                    ),
                    Span::styled(status_text, Style::default().fg(status_color)),
                ]));

                // 第 2 行:评分进度条 + score/100
                let score_line = build_score_bar_line(adapter.compatibility_score);
                lines.push(score_line);

                // 第 3 行:最近请求类型分布
                let recent_line = build_recent_requests_line(&adapter.recent_requests);
                lines.push(recent_line);

                // 除最后一个适配器外,每个适配器后空一行,提升可读性
                if idx + 1 < total {
                    lines.push(Line::from(""));
                }
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(FOOTER_TEXT));
        Text::from(lines)
    }

    /// 构建选中适配器的详情弹窗内容
    ///
    /// WHY 独立方法:与 QuestPanel::detail_content 模式一致,
    /// 将详情字符串构造与渲染解耦,便于测试断言内容字段。
    fn detail_content(adapter: &ChtcAdapterInfo) -> String {
        let status_text = if adapter.is_online {
            "online"
        } else {
            "offline"
        };
        let mut lines = vec![
            format!("Adapter ID: {}", adapter.adapter_id),
            format!("Type: {}", adapter.adapter_type),
            format!("Compatibility Score: {}/100", adapter.compatibility_score),
            format!("Status: {status_text}"),
        ];

        if adapter.compatibility_score < COMPATIBILITY_SCORE_WARNING_THRESHOLD {
            lines.push(format!(
                "Warning: score below {} (compatibility risk)",
                COMPATIBILITY_SCORE_WARNING_THRESHOLD
            ));
        }

        if adapter.recent_requests.is_empty() {
            lines.push(String::new());
            lines.push("Recent Requests: (none)".into());
        } else {
            lines.push(String::new());
            lines.push("Recent Requests:".into());
            for (req_type, count) in &adapter.recent_requests {
                lines.push(format!("  - {req_type}: {count}"));
            }
        }

        lines.join("\n")
    }
}

/// 构建兼容性评分进度条行
///
/// 格式:`  [==========--------------------] 95/100`
///
/// WHY 内联构建而非复用 `render::utilization_bar`:`utilization_bar` 返回的 Line
/// 包含百分比 `40.0%`,而本面板需要显示 `score/100` 格式。内联构建可同时控制
/// 进度条宽度、填充字符与评分文本,避免信息重复。
fn build_score_bar_line(score: u8) -> Line<'static> {
    let color = ChtcPanel::score_color(score);
    let filled = (score as usize * PROGRESS_BAR_WIDTH / 100).min(PROGRESS_BAR_WIDTH);
    let remaining = PROGRESS_BAR_WIDTH - filled;

    Line::from(vec![
        Span::styled("  [", Style::default().fg(Color::Gray)),
        Span::styled("=".repeat(filled), Style::default().fg(color)),
        Span::styled("-".repeat(remaining), Style::default().fg(Color::DarkGray)),
        Span::styled("] ", Style::default().fg(Color::Gray)),
        Span::styled(
            format!("{}/100", score),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ])
}

/// 构建最近请求类型分布行
///
/// 有数据时显示为 `  Requests: type(count) / type(count) / ...`,
/// 无数据时显示为 `  Requests: (none)`。
fn build_recent_requests_line(recent: &[(String, u32)]) -> Line<'static> {
    if recent.is_empty() {
        return Line::from(vec![
            Span::styled("  Requests: ", Style::default().fg(Color::Gray)),
            Span::styled("(none)", Style::default().fg(Color::DarkGray)),
        ]);
    }

    let mut spans: Vec<Span<'static>> = vec![Span::styled(
        "  Requests: ",
        Style::default().fg(Color::Gray),
    )];

    let last_idx = recent.len() - 1;
    for (idx, (req_type, count)) in recent.iter().enumerate() {
        // 每个请求类型显示为 `type(count)`,类型名用默认色,次数用青色突出
        spans.push(Span::raw(req_type.clone()));
        spans.push(Span::styled(
            format!("({count})"),
            Style::default().fg(Color::Cyan),
        ));
        if idx < last_idx {
            spans.push(Span::styled(" / ", Style::default().fg(Color::DarkGray)));
        }
    }

    Line::from(spans)
}

impl Panel for ChtcPanel {
    fn id(&self) -> PanelId {
        PanelId::Chtc
    }

    fn title(&self) -> Line<'static> {
        Line::from(" CHTC Adapters ")
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        // 渲染前先钳制选中索引,防止数据更新后选中越界
        let count = state.chtc_state.adapters.len();
        self.selected = list_state::clamp_selected(self.selected, count);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" CHTC Adapters ");
        let inner = block.inner(area);
        block.render(area, buf);

        // 计算可视区域行数,调整滚动偏移使选中项始终可见
        // WHY saturating_sub(3):为标题行、分隔线、底部分隔预留空间
        let content_height = inner.height.saturating_sub(3) as usize;
        self.scroll_offset =
            list_state::adjust_scroll(self.selected, self.scroll_offset, content_height);

        let paragraph = Paragraph::new(Self::content(state)).scroll((self.scroll_offset as u16, 0));
        paragraph.render(inner, buf);
    }

    fn handle_key(&mut self, key: KeyEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = state.chtc_state.adapters.len();

        // 上下键导航:复用 list_state 辅助函数,与其他列表面板行为一致
        if let Some(new_selected) =
            list_state::handle_key_navigation(key.code, self.selected, count)
        {
            self.selected = new_selected;
            return None;
        }

        match key.code {
            KeyCode::Enter => {
                // Enter 打开选中适配器的详情弹窗;空列表时返回 None
                let adapters = &state.chtc_state.adapters;
                if let Some(adapter) = adapters.get(self.selected) {
                    let content = Self::detail_content(adapter);
                    Some(TuiCommand::OpenPopup(PopupKind::Detail {
                        title: format!("CHTC Adapter: {}", adapter.adapter_id),
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
        let count = state.chtc_state.adapters.len();
        self.selected = if count == 0 { 0 } else { count - 1 };
        self.scroll_offset = self.selected;
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = state.chtc_state.adapters.len();
        // 滚轮:复用 list_state 辅助函数
        if let Some(new_selected) =
            list_state::handle_mouse_scroll(mouse.kind, self.selected, count)
        {
            self.selected = new_selected;
        }
        None
    }

    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![("↑/↓", "导航"), ("R", "刷新")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_adapter(id: &str, score: u8, online: bool) -> ChtcAdapterInfo {
        ChtcAdapterInfo {
            adapter_id: id.into(),
            adapter_type: "vscode".into(),
            compatibility_score: score,
            recent_requests: vec![("tool_call".into(), 5)],
            is_online: online,
        }
    }

    #[test]
    fn test_chtc_panel_id() {
        let panel = ChtcPanel::new();
        assert_eq!(panel.id(), PanelId::Chtc);
    }

    #[test]
    fn test_chtc_panel_title() {
        let panel = ChtcPanel::new();
        let title = panel.title();
        assert_eq!(title.to_string(), " CHTC Adapters ");
    }

    #[test]
    fn test_score_color_thresholds() {
        // 评分着色三档阈值验证
        assert_eq!(ChtcPanel::score_color(80), Color::Green);
        assert_eq!(ChtcPanel::score_color(100), Color::Green);
        assert_eq!(ChtcPanel::score_color(79), Color::Reset);
        assert_eq!(ChtcPanel::score_color(60), Color::Reset);
        assert_eq!(ChtcPanel::score_color(59), Color::Yellow);
        assert_eq!(ChtcPanel::score_color(0), Color::Yellow);
    }

    #[test]
    fn test_build_recent_requests_line_empty() {
        let line = build_recent_requests_line(&[]);
        let text = line.to_string();
        assert!(text.contains("(none)"), "empty recent should show (none)");
        assert!(
            text.contains("Requests"),
            "should have Requests label even when empty"
        );
    }

    #[test]
    fn test_build_recent_requests_line_with_data() {
        let line = build_recent_requests_line(&[("tool_call".into(), 42), ("hover".into(), 18)]);
        let text = line.to_string();
        assert!(text.contains("Requests"));
        assert!(text.contains("tool_call(42)"));
        assert!(text.contains("hover(18)"));
        assert!(text.contains("/"), "entries should be separated by /");
    }

    #[test]
    fn test_build_score_bar_line_format() {
        let line = build_score_bar_line(95);
        let text = line.to_string();
        assert!(text.contains('['), "bar should start with [");
        assert!(text.contains(']'), "bar should end with ]");
        assert!(text.contains('='), "filled part should use =");
        assert!(
            text.contains("95/100"),
            "score should be displayed as X/100"
        );
    }

    #[test]
    fn test_detail_content_contains_all_fields() {
        let adapter = sample_adapter("vscode-ext", 95, true);
        let content = ChtcPanel::detail_content(&adapter);
        assert!(content.contains("vscode-ext"));
        assert!(content.contains("vscode"));
        assert!(content.contains("95/100"));
        assert!(content.contains("online"));
        assert!(content.contains("tool_call"));
        assert!(!content.contains("Warning"), "score 95 should not warn");
    }

    #[test]
    fn test_detail_content_low_score_includes_warning() {
        let adapter = sample_adapter("vim-plugin", 45, false);
        let content = ChtcPanel::detail_content(&adapter);
        assert!(
            content.contains("Warning"),
            "low score should include warning"
        );
        assert!(content.contains("offline"));
    }
}
