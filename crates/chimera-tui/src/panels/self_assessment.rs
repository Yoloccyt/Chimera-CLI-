//! TUI SelfAssessment 面板 — 五维度 Harness 自我评估仪表盘(polish-v2.7 P1-5)
//!
//! 对应架构层:L10 Interface
//! 对应 ADR:ADR-049 决策 1(五维评估面板落点 chimera-tui)
//! 对应设计源:`chimera_ultimate_polish_v2.7.md` §14.3(Qoder Better Harness 五维度)
//!
//! # 设计决策(WHY)
//! - **数据从 `latest_events` 派生**:面板反向扫描事件流取最新
//!   `HarnessReportGenerated`(五维评分)与最近 `AuditFindingRaised`(发现列表),
//!   零 DataPipeline/TuiState 字段侵入,符合 ADR-049"零回归风险"档的落地方式。
//! - **文本条形图而非 Gauge widget**:与 Budget 面板的利用率进度条同款实现,
//!   复用既有视觉语言,避免为单面板引入新 viz 依赖。

use crossterm::event::KeyEvent;
use event_bus::NexusEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::types::{PanelId, TuiCommand, TuiState};

/// 审计发现列表最多展示条数
///
/// WHY 5:面板高度有限,五维条形图已占 ~10 行,保留最近 5 条发现
/// 兼顾信息量与可读性;完整发现流可在 EventStream 面板过滤查看。
const MAX_FINDINGS_SHOWN: usize = 5;

/// SelfAssessment 面板
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct SelfAssessmentPanel;

impl SelfAssessmentPanel {
    /// 创建新的 SelfAssessment 面板
    pub fn new() -> Self {
        Self
    }

    /// 渲染单个维度的评分条形行(宽度 20,≥0.7 绿 / ≥0.4 黄 / <0.4 红)
    fn dimension_line(label: &'static str, score: f32) -> Line<'static> {
        const BAR_WIDTH: usize = 20;
        let clamped = score.clamp(0.0, 1.0);
        let filled = ((clamped * BAR_WIDTH as f32).round() as usize).min(BAR_WIDTH);
        let color = if clamped >= 0.7 {
            Color::Green
        } else if clamped >= 0.4 {
            Color::Yellow
        } else {
            Color::Red
        };
        Line::from(vec![
            Span::from(format!("{label:<14}[")),
            Span::styled("█".repeat(filled), Style::default().fg(color)),
            Span::styled(
                "░".repeat(BAR_WIDTH - filled),
                Style::default().fg(Color::Gray),
            ),
            Span::from(format!("] {:.0}%", clamped * 100.0)),
        ])
    }

    /// 构建面板文本内容
    ///
    /// 反向扫描 `latest_events`:
    /// 1. 最新一条 `HarnessReportGenerated` → 五维评分(无则显示等待提示)
    /// 2. 最近 `MAX_FINDINGS_SHOWN` 条 `AuditFindingRaised` → 发现列表
    pub fn content(state: &TuiState) -> Text<'static> {
        let mut lines: Vec<Line<'static>> = vec![
            Line::from("Harness Self Assessment (Qoder 5-Dim)"),
            Line::from("──────────────────────────────────────"),
        ];

        // 最新五维报告(反向扫描取第一条命中)
        let latest_report = state.latest_events.iter().rev().find_map(|e| match e {
            NexusEvent::HarnessReportGenerated {
                task_comprehension,
                controllable_execution,
                change_verification,
                reliable_delivery,
                experience_accumulation,
                findings_count,
                ..
            } => Some((
                *task_comprehension,
                *controllable_execution,
                *change_verification,
                *reliable_delivery,
                *experience_accumulation,
                *findings_count,
            )),
            _ => None,
        });

        match latest_report {
            Some((tc, ce, cv, rd, ea, fc)) => {
                lines.push(Self::dimension_line("Comprehension", tc));
                lines.push(Self::dimension_line("Execution", ce));
                lines.push(Self::dimension_line("Verification", cv));
                lines.push(Self::dimension_line("Delivery", rd));
                lines.push(Self::dimension_line("Experience", ea));
                lines.push(Line::from(format!("Findings in report: {fc}")));
            }
            None => {
                lines.push(Line::from(Span::styled(
                    "Awaiting first HarnessReportGenerated...",
                    Style::default().fg(Color::Gray),
                )));
            }
        }

        // Task 3.2: L2 Memory 协同 — 显示当前记忆策略阶段(全局快照)
        // 调用 mlc_engine::current_memory_stage() 获取 MlcEngine 策略变化时
        // 同步的全局快照,实现 L10 Panel ↔ L2 Memory 真实数据闭环。
        let stage = mlc_engine::current_memory_stage();
        lines.push(Line::from(format!("Memory Strategy Stage: {stage}")));

        // 最近审计发现(反向扫描,最新在前)
        lines.push(Line::from(""));
        lines.push(Line::from("Recent Findings"));
        lines.push(Line::from("──────────────────────────────────────"));
        let findings: Vec<Line<'static>> = state
            .latest_events
            .iter()
            .rev()
            .filter_map(|e| match e {
                NexusEvent::AuditFindingRaised {
                    finding_severity,
                    message,
                    evidence_kind,
                    ..
                } => {
                    // 证据纪律视觉化:仅静态证据的发现用黄色提示"未验证"
                    let style = match finding_severity.as_str() {
                        "high" => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                        "medium" => Style::default().fg(Color::Yellow),
                        _ if evidence_kind == "runtime_events" => Style::default().fg(Color::Green),
                        _ => Style::default(),
                    };
                    Some(Line::from(Span::styled(
                        format!("[{finding_severity}] {message}"),
                        style,
                    )))
                }
                _ => None,
            })
            .take(MAX_FINDINGS_SHOWN)
            .collect();

        if findings.is_empty() {
            lines.push(Line::from(Span::styled(
                "No findings yet.",
                Style::default().fg(Color::Gray),
            )));
        } else {
            lines.extend(findings);
        }

        Text::from(lines)
    }
}

impl Panel for SelfAssessmentPanel {
    fn id(&self) -> PanelId {
        PanelId::SelfAssessment
    }

    fn title(&self) -> Line<'static> {
        Line::from(crate::t!("panel.border.self_assessment"))
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let paragraph = Paragraph::new(Self::content(state)).block(block);
        paragraph.render(area, buf);
    }

    fn handle_key(&mut self, _key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // 展示型面板不处理专属按键(同 Budget 面板,`?` 由 TuiApp 全局拦截)
        None
    }

    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![("Tab", crate::t!("shortcut.switch_panel"))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::EventMetadata;

    #[test]
    fn test_self_assessment_panel_id() {
        let panel = SelfAssessmentPanel::new();
        assert_eq!(panel.id(), PanelId::SelfAssessment);
    }

    #[test]
    fn test_content_awaiting_when_no_report() {
        let state = TuiState::new();
        let content = SelfAssessmentPanel::content(&state).to_string();
        assert!(content.contains("Awaiting first HarnessReportGenerated"));
        assert!(content.contains("No findings yet."));
    }

    #[test]
    fn test_content_renders_latest_report_and_findings() {
        let mut state = TuiState::new();
        // 旧报告(应被新报告覆盖)
        state
            .latest_events
            .push_back(NexusEvent::HarnessReportGenerated {
                metadata: EventMetadata::new("test"),
                task_comprehension: 0.1,
                controllable_execution: 0.1,
                change_verification: 0.1,
                reliable_delivery: 0.1,
                experience_accumulation: 0.1,
                findings_count: 0,
            });
        // 新报告(反向扫描应命中此条)
        state
            .latest_events
            .push_back(NexusEvent::HarnessReportGenerated {
                metadata: EventMetadata::new("test"),
                task_comprehension: 0.8,
                controllable_execution: 0.6,
                change_verification: 0.5,
                reliable_delivery: 1.0,
                experience_accumulation: 0.3,
                findings_count: 2,
            });
        state
            .latest_events
            .push_back(NexusEvent::AuditFindingRaised {
                metadata: EventMetadata::new("test"),
                finding_severity: "medium".into(),
                category: "unused_capability".into(),
                message: "能力 'x' 已配置但运行时从未使用".into(),
                evidence_kind: "static_only".into(),
                fix_hint: "移除或排查".into(),
            });

        let content = SelfAssessmentPanel::content(&state).to_string();
        // 取最新报告:Comprehension 80% 而非旧报告 10%
        assert!(content.contains("80%"));
        assert!(content.contains("Findings in report: 2"));
        assert!(content.contains("[medium]"));
        assert!(content.contains("已配置但运行时从未使用"));
    }
}
