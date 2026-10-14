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
    /// criterion 基准总数（build.rs 扫描清单得到的真实静态值）
    pub total_benches: u32,
    /// SLO 阈值/门禁段总数（build.rs 扫描 `perf_thresholds.toml` 得到的真实静态值）
    pub slo_thresholds_total: u32,
    /// SLO 违约数——CI 动态评定结果，**无运行时生产者时为 `None`**（诚实标 N/A，不回填 0）。
    pub slo_breaches: Option<u32>,
    /// 检索 P99 延迟（毫秒）——需 mlc 检索延迟直方图生产者；当前无源 → `None`。
    pub retrieval_p99_ms: Option<f64>,
    /// 裁判一致率（0.0~1.0）——需评测层运行时计数；当前无源 → `None`。
    pub agreement_rate: Option<f64>,
    /// 自动化结果可认证比例（0.0~1.0）——需认证闭环运行时计数；当前无源 → `None`。
    pub certifiable_ratio: Option<f64>,
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
        // 无运行时生产者的字段诚实标 N/A（绝不回填 0 造假——数据先行红线）。
        let unavailable = || crate::t!("panel.bench.unavailable").to_string();
        lines.push(Line::from(format!(
            "{}: {}",
            crate::t!("panel.bench.total_benches"),
            s.total_benches
        )));
        lines.push(Line::from(format!(
            "{}: {}",
            crate::t!("panel.bench.slo_thresholds_total"),
            s.slo_thresholds_total
        )));
        lines.push(Line::from(format!(
            "{}: {}",
            crate::t!("panel.bench.slo_breaches"),
            match s.slo_breaches {
                Some(v) => v.to_string(),
                None => unavailable(),
            }
        )));
        lines.push(Line::from(format!(
            "{}: {}",
            crate::t!("panel.bench.retrieval_p99"),
            match s.retrieval_p99_ms {
                Some(v) => format!("{v:.2}"),
                None => unavailable(),
            }
        )));
        // 一致率/可认证比例为 0-1 比值,直传统一助手(基数歧义由函数名消除,
        // 与 osa_sparse.rs 同口径;percent_format_invariant_test 门禁此写法)
        lines.push(Line::from(format!(
            "{}: {}",
            crate::t!("panel.bench.agreement"),
            match s.agreement_rate {
                Some(v) => crate::render::percent_detail(v),
                None => unavailable(),
            }
        )));
        lines.push(Line::from(format!(
            "{}: {}",
            crate::t!("panel.bench.certifiable"),
            match s.certifiable_ratio {
                Some(v) => crate::render::percent_detail(v),
                None => unavailable(),
            }
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
                slo_thresholds_total: 46,
                slo_breaches: Some(2),
                retrieval_p99_ms: Some(2.11),
                agreement_rate: Some(0.93),
                certifiable_ratio: Some(0.88),
            },
        }));
        let content = panel.content().to_string();
        assert!(content.contains("117"), "应展示基准总数");
        assert!(content.contains("46"), "应展示阈值总数");
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

    #[test]
    fn content_marks_unavailable_for_sourceless_fields() {
        // 有源字段（静态计数）实填，无运行时生产者的四项诚实标 N/A（不回填 0）。
        let panel = BenchPanel::with_provider(Arc::new(MockProvider {
            stats: BenchStats {
                total_benches: 117,
                slo_thresholds_total: 46,
                ..Default::default()
            },
        }));
        let content = panel.content().to_string();
        let na = crate::t!("panel.bench.unavailable").to_string();
        assert!(content.contains("117"), "静态基准数应实填");
        assert!(content.contains("46"), "静态阈值总数应实填");
        assert!(
            content.contains(&na),
            "无运行时生产者的字段应标 N/A, got: {content}"
        );
    }
}
