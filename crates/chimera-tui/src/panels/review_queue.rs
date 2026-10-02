//! TUI 审查队列面板 — AdversarialReview 待审/在审/已认证队列（v2.3 §(b.6)/§(b.4)）
//!
//! 对应架构层:L10 Interface
//! 对应设计源:`Chimera架构重构与优化方案_v2.3` §(b.6)「新增面板 审查队列」+「成功渲染只消费 CertifyEvent」
//!           与 §(b.1)「Claim 未获 Certify 即渲染违规=0」
//!
//! # 核心职责(§(b.1) 铁律的 UI 落地)
//! **零管道侵入**地从 `state.latest_events` 派生(与 SelfAssessment/DagViz/OverWindow 同范式):
//! 扫描 `ClaimIssued`/`CertifyIssued`/`ClaimRetracted` 三事件,按 claim id 聚合出审查阶段。
//! 关键不变量:某项渲染为「已认证」的**充要条件**是存在引用同一 `claim_id` 的
//! `CertifyIssued` 事件 —— `stage_for` 只认 Certify 事件,故「未获 Certify 即渲染为已认证」
//! 在本面板**结构上不可达**(§(b.1) 验收「违规=0」的机械保证)。
//!
//! # 消费点(供 event_coverage_drift 声明一致性校验)
//! 本文件匹配 `NexusEvent::ClaimIssued`/`NexusEvent::CertifyIssued`/`NexusEvent::ClaimRetracted`,
//! 是三事件在 TUI 侧的真实消费点(声明为 Consumed 的实现依据)。

use std::collections::{HashMap, HashSet};

use crossterm::event::KeyEvent;
use event_bus::NexusEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::types::{PanelId, TuiCommand, TuiState};

/// 审查阶段 — 与 §(b.4) AdversarialReview 状态机对齐的展示投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewStage {
    /// 已认领,未见 Certify(未认证)
    Claim,
    /// 已通过 Certify(唯一可渲染为「已认证」的阶段)
    Certified,
    /// 已撤回/否决(未认证)
    Retracted,
}

/// 队列项 — 一个被审查命题的展示快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewItem {
    /// Claim/Certify 引用 id
    pub id: String,
    /// 命题摘要
    pub subject: String,
    /// 审查阶段
    pub stage: ReviewStage,
}

/// 由事件流计算各 claim id 的阶段。**Certified 仅当存在同 id 的 CertifyIssued**。
#[must_use]
pub fn stage_for(
    id: &str,
    certified: &HashSet<String>,
    retracted: &HashSet<String>,
) -> ReviewStage {
    if certified.contains(id) {
        ReviewStage::Certified
    } else if retracted.contains(id) {
        ReviewStage::Retracted
    } else {
        ReviewStage::Claim
    }
}

/// 审查阶段 → 展示 i18n 标签。**唯一**产出 `panel.review.certified` 的路径是 Certified。
#[must_use]
pub fn stage_label(stage: ReviewStage) -> String {
    match stage {
        ReviewStage::Certified => crate::t!("panel.review.certified").to_string(),
        ReviewStage::Claim => crate::t!("panel.review.claim_pending").to_string(),
        ReviewStage::Retracted => crate::t!("panel.review.retracted").to_string(),
    }
}

/// 从 `latest_events` 聚合审查队列(首次出现序,按 claim id 去重)。
#[must_use]
pub fn collect_review_queue(state: &TuiState) -> Vec<ReviewItem> {
    let mut order: Vec<String> = Vec::new();
    let mut subject: HashMap<String, String> = HashMap::new();
    let mut certified: HashSet<String> = HashSet::new();
    let mut retracted: HashSet<String> = HashSet::new();
    for event in state.latest_events.iter() {
        match event {
            NexusEvent::ClaimIssued {
                claim_id,
                subject: s,
                ..
            } => {
                if !subject.contains_key(claim_id) {
                    order.push(claim_id.clone());
                }
                subject.insert(claim_id.clone(), s.clone());
            }
            NexusEvent::CertifyIssued { .. } => {
                // §(b.1):仅经 `{ .. }` 识别 + event-bus 访问器取 ref,避免在 L10
                // 拼写构造式 `CertifyIssued { field }`(白名单门据此判签发)。
                if let Some(claim_ref) = event.certify_claim_ref() {
                    if !subject.contains_key(claim_ref) {
                        order.push(claim_ref.to_string());
                    }
                    certified.insert(claim_ref.to_string());
                }
            }
            NexusEvent::ClaimRetracted { claim_ref, .. } => {
                if !subject.contains_key(claim_ref) {
                    order.push(claim_ref.clone());
                }
                retracted.insert(claim_ref.clone());
            }
            _ => {}
        }
    }
    order
        .into_iter()
        .map(|id| ReviewItem {
            subject: subject.get(&id).cloned().unwrap_or_default(),
            stage: stage_for(&id, &certified, &retracted),
            id,
        })
        .collect()
}

/// 审查队列面板(v2.3 §(b.6)/§(b.4))
#[derive(Debug, Default)]
pub struct ReviewQueuePanel {}

impl ReviewQueuePanel {
    /// 创建面板(数据从 `state.latest_events` 派生,无需外部注入)
    pub fn new() -> Self {
        Self {}
    }

    /// 构建面板文本内容:按事件流派生的审查队列渲染;
    /// Certified 绿色、Retracted 红色(F-40 可辨识)、Claim 暗灰。
    #[must_use]
    pub fn content(state: &TuiState) -> Text<'static> {
        let mut lines: Vec<Line<'static>> = vec![
            Line::from(crate::t!("panel.review.subtitle")),
            Line::from("──────────────────────────────────────"),
        ];
        let items = collect_review_queue(state);
        if items.is_empty() {
            lines.push(Line::from(Span::styled(
                crate::t!("panel.review.none"),
                Style::default().fg(Color::Gray),
            )));
            return Text::from(lines);
        }
        for (idx, item) in items.iter().enumerate() {
            // §(b.1):仅 Certified 以绿色呈现,各态均不冒充已认证;
            // F-40:Retracted 红色与 Claim 暗灰区分 —— 撤回/否决状态必须可辨识
            let color = match item.stage {
                ReviewStage::Certified => Color::Green,
                ReviewStage::Retracted => Color::Red,
                ReviewStage::Claim => Color::DarkGray,
            };
            lines.push(Line::from(Span::styled(
                format!("{}. [{}] {}", idx + 1, stage_label(item.stage), item.id),
                Style::default().fg(color),
            )));
        }
        Text::from(lines)
    }
}

impl Panel for ReviewQueuePanel {
    fn id(&self) -> PanelId {
        PanelId::ReviewQueue
    }

    fn title(&self) -> Line<'static> {
        Line::from(crate::t!("panel.border.review_queue"))
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        if crate::panels::degenerate(area) {
            crate::panels::render_too_small(area, buf);
            return;
        }
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let paragraph = Paragraph::new(Self::content(state)).block(block);
        paragraph.render(area, buf);
    }

    fn handle_key(&mut self, _key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // 展示型面板不处理专属按键(同 SelfAssessment)
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
    use event_bus::EventMetadata;

    fn push(state: &mut TuiState, ev: NexusEvent) {
        std::sync::Arc::make_mut(&mut state.latest_events).push_back(ev);
    }

    fn claim(id: &str) -> NexusEvent {
        NexusEvent::ClaimIssued {
            metadata: EventMetadata::new("t"),
            claim_id: id.into(),
            subject: format!("命题-{id}"),
            claimant: "agent-1".into(),
        }
    }
    fn certify(id: &str) -> NexusEvent {
        NexusEvent::CertifyIssued {
            metadata: EventMetadata::new("parliament"),
            claim_ref: id.into(),
            subject: format!("命题-{id}"),
            verifier: "Compiler".into(),
        }
    }
    fn retract(id: &str) -> NexusEvent {
        NexusEvent::ClaimRetracted {
            metadata: EventMetadata::new("t"),
            claim_ref: id.into(),
            reason: "作废".into(),
        }
    }

    #[test]
    fn panel_id_semantics() {
        assert_eq!(ReviewQueuePanel::new().id(), PanelId::ReviewQueue);
    }

    #[test]
    fn empty_when_no_events() {
        let state = TuiState::new();
        assert!(collect_review_queue(&state).is_empty());
    }

    #[test]
    fn claim_without_certify_is_not_certified() {
        // §(b.1) 核心:仅有 Claim 的事件流,任何项都不得进入 Certified 阶段
        let mut state = TuiState::new();
        push(&mut state, claim("c-1"));
        let q = collect_review_queue(&state);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].stage, ReviewStage::Claim);
        let certified_label = crate::t!("panel.review.certified").to_string();
        let content = ReviewQueuePanel::content(&state).to_string();
        assert!(
            !content.contains(&certified_label),
            "未获 Certify 的 Claim 渲染不得含「已认证」, got: {content}"
        );
    }

    #[test]
    fn claim_with_matching_certify_becomes_certified() {
        let mut state = TuiState::new();
        push(&mut state, claim("c-1"));
        push(&mut state, certify("c-1"));
        let q = collect_review_queue(&state);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].stage, ReviewStage::Certified, "同 id Certify → 已认证");
        let content = ReviewQueuePanel::content(&state).to_string();
        assert!(content.contains(&crate::t!("panel.review.certified").to_string()));
    }

    #[test]
    fn certify_for_other_id_does_not_certify_claim() {
        // c-1 有 Claim,certify 针对 c-2 → c-1 仍为 Claim(引用错位不误认证)
        let mut state = TuiState::new();
        push(&mut state, claim("c-1"));
        push(&mut state, certify("c-2"));
        let q = collect_review_queue(&state);
        let c1 = q.iter().find(|i| i.id == "c-1").unwrap();
        assert_eq!(c1.stage, ReviewStage::Claim);
    }

    #[test]
    fn retracted_shows_as_retracted_stage() {
        let mut state = TuiState::new();
        push(&mut state, claim("c-1"));
        push(&mut state, retract("c-1"));
        let q = collect_review_queue(&state);
        assert_eq!(q[0].stage, ReviewStage::Retracted);
        // Retracted 不得渲染为已认证
        assert_ne!(
            stage_label(ReviewStage::Retracted),
            crate::t!("panel.review.certified")
        );
    }

    #[test]
    fn stage_label_only_certified_maps_to_certified() {
        assert_eq!(
            stage_label(ReviewStage::Certified),
            crate::t!("panel.review.certified").to_string()
        );
        for s in [ReviewStage::Claim, ReviewStage::Retracted] {
            assert_ne!(
                stage_label(s),
                crate::t!("panel.review.certified").to_string()
            );
        }
    }

    #[test]
    fn retracted_is_distinguishable_from_claim() {
        // F-40:Retracted 不得与 Claim 同标签(撤回/否决状态必须可辨识)
        assert_ne!(
            stage_label(ReviewStage::Retracted),
            stage_label(ReviewStage::Claim),
            "Retracted 与 Claim 的展示标签必须可辨识"
        );

        // 颜色可辨识:渲染含 Claim 与 Retracted 各一项的队列,逐行核对前景色
        let mut state = TuiState::new();
        push(&mut state, claim("c-1"));
        push(&mut state, retract("c-2"));
        let text = ReviewQueuePanel::content(&state);
        let row_fg = |id: &str| {
            text.lines
                .iter()
                .find(|l| l.spans.iter().any(|s| s.content.contains(id)))
                .and_then(|l| l.spans.first())
                .map(|s| s.style.fg)
        };
        assert_eq!(
            row_fg("c-1"),
            Some(Some(Color::DarkGray)),
            "Claim 行应保持暗灰"
        );
        assert_eq!(
            row_fg("c-2"),
            Some(Some(Color::Red)),
            "Retracted 行应为红色可辨识"
        );
    }
}
