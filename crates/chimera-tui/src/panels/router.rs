//! TUI Router 面板 — 三路由器(KVBSR/SESA/FaaE)命中率与延迟(P2.3)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 数据来源:L9 efficiency-monitor 通过 `NexusEvent::RouterStatsReported`
//!   聚合发布,经 `RouterSync` 同步到 `TuiState.router_metrics`。
//! - 布局:三路由器纵向排列,每个路由器显示命中率进度条 + P50/P95/P99 延迟行,
//!   末尾聚合热点 capability Top-10 列表(合并三路由器的 hot_capabilities)。
//! - Top-K 选择使用 `select_nth_unstable_by`(O(n)),遵守 §4.1 工程约定,
//!   禁止 `sort_by` 做 Top-K 选择(O(n log n))。最终对前 K 个结果做排序是
//!   O(k log k),k=10 时约 33 次比较,可接受。
//! - 命中率低水位:`< 60%` 时追加 `[LOW]` 标记并黄色高亮,提醒运维关注。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::render::{self, FOOTER_TEXT};
use crate::types::{PanelId, RouterStatsInfo, TuiCommand, TuiState};

/// 命中率低水位阈值:低于此值(60%)的命中率以黄色高亮提醒运维关注。
///
/// WHY 0.6:经验阈值,低于 60% 表示路由器命中率不及格。阈值边界(=0.6)不算低,
/// 与测试 `test_router_panel_threshold_boundary_60_percent` 行为一致(0.6 不高亮,
/// 0.59 高亮)。
const LOW_HIT_RATE_THRESHOLD: f32 = 0.6;

/// Top-K 默认截断数:热点 capability 列表展示前 K 个,避免终端溢出。
///
/// WHY 10:与 §4.1 `select_nth_unstable` Top-K 约定的示例值一致,也是终端
/// 一屏可容纳的合理行数。超过 10 行会挤压延迟统计区的可见性。
const TOP_K_CAPABILITIES: usize = 10;

/// Router 路由统计面板 — 可视化三路由器命中率与延迟分位数
///
/// 消费 `TuiState.router_metrics`(三路由器统计 + 各自热点能力列表)。
/// 渲染输出:
/// ```text
/// Router Stats
/// ─────────────
/// KVBSR  [=========] 87.0%
/// KVBSR  Latency  P50: 120μs  P95: 480μs  P99: 950μs
///
/// SESA   [=======-] 72.0%
/// SESA   Latency  P50: 200μs  P95: 800μs  P99: 1500μs
///
/// FaaE   [=========-] 91.0%
/// FaaE   Latency  P50: 60μs  P95: 280μs  P99: 650μs
///
/// Hot Capabilities (Top-10):
///   1. tool_call (88 hits)
///   2. search (42 hits)
///   ...
/// ```
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct RouterPanel;

impl RouterPanel {
    /// 创建新的 Router 面板
    pub fn new() -> Self {
        Self
    }

    /// 构建面板文本内容(三路由器命中率进度条 + 延迟 + 热点 Top-10)
    ///
    /// WHY 独立 pub 方法:与 BudgetPanel/DecayPanel 模式一致,
    /// 便于单元测试无需 TestBackend 即可验证文本输出。
    pub fn content(state: &TuiState) -> Text<'static> {
        let m = &state.router_metrics;
        let mut lines: Vec<Line<'static>> = Vec::new();

        lines.push(Line::from("Router Stats"));
        lines.push(Line::from("─────────────"));

        // 三路由器命中率进度条 + 延迟行,空行分隔
        Self::push_router_section(&mut lines, "KVBSR", &m.kvbsr_stats);
        lines.push(Line::from(""));
        Self::push_router_section(&mut lines, "SESA", &m.sesa_stats);
        lines.push(Line::from(""));
        Self::push_router_section(&mut lines, "FaaE", &m.faae_stats);

        // 热点 capability Top-10(合并三路由器)
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("Hot Capabilities (Top-{}):", TOP_K_CAPABILITIES),
            Style::default().add_modifier(Modifier::BOLD),
        )));

        let merged = merge_hot_capabilities(&m.kvbsr_stats, &m.sesa_stats, &m.faae_stats);
        let top = top_k_capabilities(&merged, TOP_K_CAPABILITIES);
        if top.is_empty() {
            lines.push(Line::from("  (no hot capabilities)"));
        } else {
            for (idx, (cap, hits)) in top.iter().enumerate() {
                lines.push(Line::from(format!(
                    "  {}. {} ({} hits)",
                    idx + 1,
                    cap,
                    hits
                )));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(FOOTER_TEXT));

        Text::from(lines)
    }

    /// 追加单路由器段落(命中率进度条 + 延迟行)
    ///
    /// 命中率行格式:`{label:<6} [====---] {pct}%  [LOW]`(低命中率时追加标记)
    /// 延迟行格式:复用 `render::latency_line` 统一三列对比
    fn push_router_section(lines: &mut Vec<Line<'static>>, label: &str, stats: &RouterStatsInfo) {
        // 命中率行:label + utilization_bar + 可选 [LOW] 标记
        // WHY 利用 render::utilization_bar 统一进度条样式,与 Budget 面板一致
        let bar = render::utilization_bar(stats.hit_rate as f64, 1.0, 10);
        let low_hit = stats.hit_rate < LOW_HIT_RATE_THRESHOLD;

        let mut spans: Vec<Span<'static>> = vec![Span::styled(
            format!("{:<6}", label),
            Style::default().add_modifier(Modifier::BOLD),
        )];
        // utilization_bar 已包含 [====---] N.N% 全部 spans,直接追加
        spans.extend(bar.spans);

        // 低命中率时追加黄色 [LOW] 标记,提醒运维关注
        if low_hit {
            spans.push(Span::styled(
                "  [LOW]",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        lines.push(Line::from(spans));

        // 延迟行:复用 render::latency_line 统一 P50/P95/P99 三列对比格式
        lines.push(render::latency_line(
            label,
            stats.p50_latency_us,
            stats.p95_latency_us,
            stats.p99_latency_us,
        ));
    }
}

impl Panel for RouterPanel {
    fn id(&self) -> PanelId {
        PanelId::Router
    }

    fn title(&self) -> Line<'static> {
        Line::from(" Router Stats ")
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let paragraph = Paragraph::new(Self::content(state)).block(block);
        paragraph.render(area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        match key.code {
            KeyCode::Char('?') => Some(TuiCommand::ShowHelp),
            _ => None,
        }
    }
}

/// 合并三路由器的 hot_capabilities 列表,相同 capability_id 的 hits 累加
///
/// WHY 合并:同一 capability 可能被多个路由器调用(如 "search" 同时被
/// KVBSR 与 FaaE 路由),合并后才能反映全局热度。累加相同 ID 的 hits
/// 避免同一 capability 在 Top-10 中重复出现。
fn merge_hot_capabilities(
    kvbsr: &RouterStatsInfo,
    sesa: &RouterStatsInfo,
    faae: &RouterStatsInfo,
) -> Vec<(String, u64)> {
    let mut merged: Vec<(String, u64)> = Vec::new();
    for (cap, hits) in kvbsr
        .hot_capabilities
        .iter()
        .chain(sesa.hot_capabilities.iter())
        .chain(faae.hot_capabilities.iter())
    {
        if let Some(existing) = merged.iter_mut().find(|(c, _)| c == cap) {
            existing.1 += hits;
        } else {
            merged.push((cap.clone(), *hits));
        }
    }
    merged
}

/// 从 (capability_id, hits) 列表中选取 Top-K,按 hits 降序返回
///
/// # 算法选择(WHY)
/// 使用 `select_nth_unstable_by` 做 partial sort,O(n) 时间复杂度定位第 K 大元素,
/// 随后仅对前 K 个元素做最终排序(O(k log k))。相比 `sort_by` 全排序(O(n log n)),
/// 在 n 较大时(如数千个 capability)显著节省 CPU。遵守 §4.1 工程约定:
/// "Top-K 选择必须用 `select_nth_unstable` (O(n)),禁止 `sort_by` (O(n log n)) 做 Top-K"。
///
/// # 边界处理
/// - `k == 0`:返回空 Vec
/// - 输入为空:返回空 Vec
/// - 输入长度 ≤ k:跳过 partial sort,直接对全部元素排序返回(此时全排序是必要的,
///   因为需要降序输出)
pub fn top_k_capabilities(caps: &[(String, u64)], k: usize) -> Vec<(String, u64)> {
    if k == 0 || caps.is_empty() {
        return Vec::new();
    }
    let mut data: Vec<(String, u64)> = caps.to_vec();
    let k = k.min(data.len());
    if data.len() > k {
        // partial sort:把第 k 大的元素放到位置 k-1,前 k 个即为 Top-k(无序)
        // compare 用降序(b.1.cmp(a.1)),使前 k 个是 hits 最高的
        // 返回值(pivot + 左右切片)不需要使用,仅利用 side effect
        let _ = data.select_nth_unstable_by(k - 1, |a, b| b.1.cmp(&a.1));
    }
    // 对前 k 个做最终降序排序,O(k log k)
    // WHY sort_by_key + Reverse:clippy unnecessary_sort_by 建议的等价写法,
    // 比闭包 sort_by 更高效(避免每次比较调用闭包),且语义更清晰(Reverse = 降序)
    let mut top = data[..k].to_vec();
    top.sort_by_key(|item| std::cmp::Reverse(item.1));
    top
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_router_panel_id() {
        let panel = RouterPanel::new();
        assert_eq!(panel.id(), PanelId::Router);
    }

    #[test]
    fn test_router_panel_title() {
        let panel = RouterPanel::new();
        let title = panel.title();
        assert_eq!(title.to_string(), " Router Stats ");
    }

    #[test]
    fn test_router_panel_handle_key_returns_none() {
        let mut panel = RouterPanel::new();
        let mut state = TuiState::new();
        let key = KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        );
        assert!(panel.handle_key(key, &mut state).is_none());
    }

    #[test]
    fn test_router_panel_handle_key_question_mark() {
        let mut panel = RouterPanel::new();
        let mut state = TuiState::new();
        let key = KeyEvent::new(
            crossterm::event::KeyCode::Char('?'),
            crossterm::event::KeyModifiers::NONE,
        );
        assert_eq!(
            panel.handle_key(key, &mut state),
            Some(TuiCommand::ShowHelp)
        );
    }

    #[test]
    fn test_top_k_capabilities_empty() {
        let empty: Vec<(String, u64)> = vec![];
        let result = top_k_capabilities(&empty, 10);
        assert!(result.is_empty());
    }

    #[test]
    fn test_top_k_capabilities_fewer_than_k() {
        // 输入 < k 个,应返回全部并按频次降序排序
        let input: Vec<(String, u64)> = vec![("a".into(), 10), ("b".into(), 30), ("c".into(), 20)];
        let result = top_k_capabilities(&input, 10);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].1, 30, "highest frequency should be first");
        assert_eq!(result[1].1, 20, "second highest should be second");
        assert_eq!(result[2].1, 10, "lowest should be last");
    }

    #[test]
    fn test_top_k_capabilities_more_than_k() {
        // 输入 > k 个,应截断到 k 个并按频次降序排序
        let input: Vec<(String, u64)> = (0..15)
            .map(|i| (format!("cap-{:02}", i), (15 - i) as u64 * 10))
            .collect();
        let result = top_k_capabilities(&input, 10);
        assert_eq!(result.len(), 10);
        assert_eq!(result[0].0, "cap-00", "highest frequency should be first");
        assert_eq!(result[0].1, 150);
        assert_eq!(result[9].0, "cap-09", "10th should be cap-09");
        assert_eq!(result[9].1, 60);
    }

    #[test]
    fn test_top_k_capabilities_k_zero() {
        let input: Vec<(String, u64)> = vec![("a".into(), 10)];
        let result = top_k_capabilities(&input, 0);
        assert!(result.is_empty());
    }

    #[test]
    fn test_router_panel_content_default_state() {
        let state = TuiState::new();
        let content = RouterPanel::content(&state).to_string();
        // 默认状态也应包含三路由器标识与 0.0%
        assert!(content.contains("KVBSR"));
        assert!(content.contains("SESA"));
        assert!(content.contains("FaaE"));
        assert!(content.contains("0.0%"));
        // 默认 hit_rate = 0.0 < 0.6,应显示 [LOW] 标记
        assert!(content.contains("[LOW]"));
    }

    #[test]
    fn test_router_panel_content_with_data() {
        use crate::types::{RouterMetrics, RouterStatsInfo};

        let mut state = TuiState::new();
        state.router_metrics = RouterMetrics {
            kvbsr_stats: RouterStatsInfo {
                hit_rate: 0.87,
                p50_latency_us: 120,
                p95_latency_us: 480,
                p99_latency_us: 950,
                hot_capabilities: vec![("search".into(), 42), ("read_file".into(), 28)],
            },
            sesa_stats: RouterStatsInfo {
                hit_rate: 0.72,
                p50_latency_us: 200,
                p95_latency_us: 800,
                p99_latency_us: 1500,
                hot_capabilities: vec![("activate".into(), 15)],
            },
            faae_stats: RouterStatsInfo {
                hit_rate: 0.91,
                p50_latency_us: 60,
                p95_latency_us: 280,
                p99_latency_us: 650,
                hot_capabilities: vec![("tool_call".into(), 88)],
            },
        };
        let content = RouterPanel::content(&state).to_string();
        // 三路由器命中率百分比
        assert!(content.contains("87.0%"));
        assert!(content.contains("72.0%"));
        assert!(content.contains("91.0%"));
        // KVBSR 延迟值
        assert!(content.contains("120"));
        assert!(content.contains("480"));
        assert!(content.contains("950"));
        // 热点 capability(合并后按 hits 降序:tool_call(88) > search(42) > read_file(28) > activate(15))
        assert!(content.contains("tool_call"));
        assert!(content.contains("search"));
        assert!(content.contains("read_file"));
        assert!(content.contains("activate"));
        // 高命中率(>= 0.6)不应显示 [LOW]
        assert!(!content.contains("[LOW]"));
    }

    #[test]
    fn test_router_panel_content_low_hit_rate_marker() {
        use crate::types::{RouterMetrics, RouterStatsInfo};

        let mut state = TuiState::new();
        state.router_metrics = RouterMetrics {
            kvbsr_stats: RouterStatsInfo {
                hit_rate: 0.45,
                p50_latency_us: 500,
                p95_latency_us: 2000,
                p99_latency_us: 5000,
                hot_capabilities: vec![],
            },
            sesa_stats: RouterStatsInfo {
                hit_rate: 0.50,
                p50_latency_us: 300,
                p95_latency_us: 1200,
                p99_latency_us: 3500,
                hot_capabilities: vec![],
            },
            faae_stats: RouterStatsInfo {
                hit_rate: 0.30,
                p50_latency_us: 800,
                p95_latency_us: 3500,
                p99_latency_us: 9000,
                hot_capabilities: vec![],
            },
        };
        let content = RouterPanel::content(&state).to_string();
        assert!(content.contains("45.0%"));
        assert!(content.contains("50.0%"));
        assert!(content.contains("30.0%"));
        // 三路由器均 < 0.6,应显示 [LOW] 标记(至少 3 次)
        let low_count = content.matches("[LOW]").count();
        assert_eq!(low_count, 3, "all three routers should have [LOW] marker");
    }

    #[test]
    fn test_merge_hot_capabilities_dedup() {
        use crate::types::{RouterMetrics, RouterStatsInfo};

        let m = RouterMetrics {
            kvbsr_stats: RouterStatsInfo {
                hit_rate: 0.8,
                p50_latency_us: 100,
                p95_latency_us: 400,
                p99_latency_us: 800,
                hot_capabilities: vec![("search".into(), 42), ("common".into(), 10)],
            },
            sesa_stats: RouterStatsInfo {
                hit_rate: 0.7,
                p50_latency_us: 200,
                p95_latency_us: 800,
                p99_latency_us: 1500,
                hot_capabilities: vec![("common".into(), 5)],
            },
            faae_stats: RouterStatsInfo {
                hit_rate: 0.9,
                p50_latency_us: 60,
                p95_latency_us: 280,
                p99_latency_us: 650,
                hot_capabilities: vec![("tool_call".into(), 88)],
            },
        };
        let merged = merge_hot_capabilities(&m.kvbsr_stats, &m.sesa_stats, &m.faae_stats);
        // "common" 应合并为 15 (10 + 5)
        let common = merged.iter().find(|(c, _)| c == "common");
        assert!(common.is_some(), "common should be in merged list");
        assert_eq!(common.unwrap().1, 15, "common hits should be summed");
        assert_eq!(merged.len(), 3, "should have 3 unique capabilities");
    }

    #[test]
    fn test_router_panel_content_empty_hot_capabilities() {
        use crate::types::{RouterMetrics, RouterStatsInfo};

        let mut state = TuiState::new();
        state.router_metrics = RouterMetrics {
            kvbsr_stats: RouterStatsInfo {
                hit_rate: 0.8,
                p50_latency_us: 100,
                p95_latency_us: 400,
                p99_latency_us: 800,
                hot_capabilities: vec![],
            },
            sesa_stats: RouterStatsInfo::default(),
            faae_stats: RouterStatsInfo::default(),
        };
        let content = RouterPanel::content(&state).to_string();
        assert!(
            content.contains("no hot capabilities"),
            "empty hot_capabilities should show placeholder"
        );
    }
}
