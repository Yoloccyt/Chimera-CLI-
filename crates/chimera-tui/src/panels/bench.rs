//! TUI Bench 面板 — `/bench` 基准数据先行(v2.3 §(b.6))
//!
//! 对应架构层:L10 Interface
//! 对应设计源:`Chimera架构重构与优化方案_v2.3` §(b.6)「/bench 面板(复用 criterion
//! 117/46 + 门触发率 + 检索延迟 + 一致率)」；反模式红线「凭感觉调参 → criterion 文化」
//!
//! # 核心职责
//! - 汇总展示评测/度量层关键指标：criterion 基准总数、SLO 违约数、
//!   检索 P99 延迟、裁判一致率、自动化结果可认证比例。
//! - 未接线时**诚实展示**等待提示(不渲染虚假数据)。
//!
//! # 设计决策(WHY)
//! - **数据经 `BenchStatsProvider` trait 注入**(D-1 依赖倒置):chimera-tui 不直接
//!   依赖 L6/L8 crate(bench 采集在 criterion/度量层),提供者负责聚合为本面板的
//!   自足快照类型。经验卡片/注入策略面板(ExperienceCardViz/InjectionStrategy)先例。
//! - **无提供者时诚实展示**:对齐「凭感觉调参」红线——数据未先行即如实标注未接线,
//!   杜绝虚假基准数字固化(SelfAssessment/PvlScore「未接线」先例)。
//! - **全部用户可见文案走 `t!()`**:与 i18n 硬编码防退化门(Concord W4 T4.4 /
//!   US-02)对齐,生产段零硬编码 CJK/英文短语(仅 ASCII i18n key)。

use std::sync::Arc;

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::types::{PanelId, TuiCommand, TuiState};

/// 基准统计快照 — 面板展示的自足类型(D-1)
///
/// WHY 自足类型:chimera-tui 不依赖 L6/L8 度量/评测 crate;提供者负责从
/// criterion 结果、perf-slo 报告、`latest_events` 聚合为本类型。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BenchStats {
    /// criterion 基准总数(§(e) 现测 117 个 `[[bench]]`)
    pub total_benches: u32,
    /// SLO 违约数(perf_thresholds 46 条中被突破的条数)
    pub slo_breaches: u32,
    /// 检索 P99 延迟(毫秒;§(b.3) 分层检索 SLO 关联)
    pub retrieval_p99_ms: f64,
    /// 裁判一致率(0.0~1.0;§(b.6) 阈值 ≥90% 方可自动化认证)
    pub agreement_rate: f64,
    /// 自动化结果可认证比例(0.0~1.0;低于阈值相关门降级为 Claim)
    pub certifiable_ratio: f64,
}

/// 基准统计提供者 — 依赖倒置注入点(D-1)
///
/// 由度量/评测层或调用方包装实现;未注入时面板诚实展示等待提示(不渲染虚假数据)。
pub trait BenchStatsProvider: Send + Sync + std::fmt::Debug {
    /// 采集当前基准统计快照
    fn bench_stats(&self) -> BenchStats;
}

/// Bench 面板(v2.3 §(b.6) `/bench` 数据先行)
#[derive(Debug, Default)]
pub struct BenchPanel {
    /// 统计提供者(None = 未接线,诚实展示等待提示)
    provider: Option<Arc<dyn BenchStatsProvider>>,
}

impl BenchPanel {
    /// 创建未接线面板(诚实展示等待提示)
    pub fn new() -> Self {
        Self { provider: None }
    }

    /// 注入统计提供者(接线后渲染真实基准摘要)
    pub fn with_provider(provider: Arc<dyn BenchStatsProvider>) -> Self {
        Self {
            provider: Some(provider),
        }
    }

    /// 构建面板文本内容
    ///
    /// 有提供者 → 基准数/SLO 违约/检索 P99/一致率/可认证比例;
    /// 无提供者 → 诚实展示等待提示(不渲染虚假数据)。
    pub fn content(&self) -> Text<'static> {
        let mut lines: Vec<Line<'static>> = vec![
            Line::from(crate::t!("panel.bench.subtitle")),
            Line::from("──────────────────────────────────────"),
        ];

        let Some(provider) = &self.provider else {
            lines.push(Line::from(Span::styled(
                crate::t!("panel.bench.awaiting"),
                Style::default().fg(Color::Gray),
            )));
            return Text::from(lines);
        };

        let s = provider.bench_stats();
        lines.push(Line::from(format!(
            "{}: {}",
            crate::t!("panel.bench.total_benches"),
            s.total_benches
        )));
        lines.push(Line::from(format!(
            "{}: {}",
            crate::t!("panel.bench.slo_breaches"),
            s.slo_breaches
        )));
        lines.push(Line::from(format!(
            "{}: {:.2}",
            crate::t!("panel.bench.retrieval_p99"),
            s.retrieval_p99_ms
        )));
        // 一致率/可认证比例为 0-1 比值,直传统一助手(基数歧义由函数名消除,
        // 与 osa_sparse.rs 同口径;percent_format_invariant_test 门禁此写法)
        lines.push(Line::from(format!(
            "{}: {}",
            crate::t!("panel.bench.agreement"),
            crate::render::percent_detail(s.agreement_rate)
        )));
        lines.push(Line::from(format!(
            "{}: {}",
            crate::t!("panel.bench.certifiable"),
            crate::render::percent_detail(s.certifiable_ratio)
        )));

        Text::from(lines)
    }
}

impl Panel for BenchPanel {
    fn id(&self) -> PanelId {
        PanelId::Bench
    }

    fn title(&self) -> Line<'static> {
        Line::from(crate::t!("panel.border.bench"))
    }

    fn render(&mut self, _state: &TuiState, area: Rect, buf: &mut Buffer) {
        // PS-2 U-4:退化尺寸统一早退(共享最低线,见 crate::panels::MIN_PANEL_W/H)
        if crate::panels::degenerate(area) {
            crate::panels::render_too_small(area, buf);
            return;
        }

        let block = Block::default().borders(Borders::ALL).title(self.title());
        let paragraph = Paragraph::new(self.content()).block(block);
        paragraph.render(area, buf);
    }

    fn handle_key(&mut self, _key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // 展示型面板不处理专属按键(同 ExperienceCardViz/SelfAssessment 面板)
        None
    }

    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![("Tab", crate::t!("shortcut.switch_panel"))]
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Mock 提供者 — 返回固定基准快照
    #[derive(Debug, Default)]
    struct MockProvider {
        stats: BenchStats,
    }

    impl BenchStatsProvider for MockProvider {
        fn bench_stats(&self) -> BenchStats {
            self.stats
        }
    }

    #[test]
    fn panel_id_semantics() {
        let panel = BenchPanel::new();
        assert_eq!(panel.id(), PanelId::Bench);
    }

    #[test]
    fn content_awaiting_when_no_provider() {
        // 诚实展示:无提供者不渲染虚假基准数字
        let panel = BenchPanel::new();
        // 默认 locale 为 zh;直接检查是否含等待提示键的中文译文
        let content = panel.content().to_string();
        assert!(
            content.contains("未接线") || content.contains("Awaiting"),
            "无提供者应展示未接线提示, got: {content}"
        );
    }

    #[test]
    fn content_renders_stats_with_provider() {
        let panel = BenchPanel::with_provider(Arc::new(MockProvider {
            stats: BenchStats {
                total_benches: 117,
                slo_breaches: 2,
                retrieval_p99_ms: 2.11,
                agreement_rate: 0.93,
                certifiable_ratio: 0.88,
            },
        }));
        let content = panel.content().to_string();
        assert!(content.contains("117"), "应展示基准总数");
        // 行级匹配而非 contains('2'):同帧 retrieval_p99 的 "2.11" 亦含 '2',
        // 裸字符断言对 SLO 违约行是否渲染无判别力(批次一评审遗留)
        let slo_line = format!("{}: 2", crate::t!("panel.bench.slo_breaches"));
        assert!(
            content.lines().any(|l| l.contains(&slo_line)),
            "应按行渲染 SLO 违约数, got: {content}"
        );
        assert!(content.contains("2.11"), "应展示检索 P99");
        assert!(content.contains("93.0%"), "应展示一致率百分比");
        assert!(content.contains("88.0%"), "应展示可认证比例");
    }
}
