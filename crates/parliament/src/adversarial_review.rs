//! AdversarialReview 协议状态机 — 裁决权外置的异构对抗审查环节（v2.3 §(b.4)(1)）
//!
//! 对应架构层:L8 Parliament
//! 对应设计源:`Chimera架构重构与优化方案_v2.3` §(b.4)(1) / §(c) AdversarialReview
//!
//! # WHY
//! v2.3 主线②「异构对抗」:同模型 swarm 的错误是**相关的**,自评为必然失效。产出
//! `Claim` 后必须路由给**未参与该任务**的异构审查者,审查意见以 Critical 事件回写
//! 黑板,**通过后**才允许进入 Certify 候选队列。本模块承载这套状态机与约束逻辑,
//! 与 §(b.1) 的 Claim/Certify 事件语义闭环(agent 只 Claim,验证器经本审查后才 Certify)。
//!
//! # 一期进程内异构
//! 异构 = 不同角色先验/温度/系统提示词(复用 chimera-mas per-agent `ModelConfig`);
//! 跨厂商审查留 `mca-gateway` 接口。生产接线(激活 `cross_vendor` 至
//! `Parliament::new`)是本模块**之后**的独立步骤,此处只提供可测试的纯逻辑核心。

use std::collections::HashSet;

/// 审查阶段状态机的合法取值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewStage {
    /// 已登记,尚未开始审查
    Pending,
    /// 审查进行中
    UnderReview,
    /// 通过,进入 Certify 候选队列
    ApprovedForCertify,
    /// 被否决
    Rejected,
    /// 悬置(证据不足,待补充)
    Deferred,
}

/// 审查者提交的一次结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewOutcome {
    /// 批准认证
    Approve,
    /// 否决
    Reject,
    /// 悬置
    Defer,
}

/// 状态机推进错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewError {
    /// 审查者同时是任务参与者 —— 违反「非参与者约束」(§(b.4) 验收)
    ReviewerIsParticipant {
        /// 冲突的审查者标识
        reviewer: String,
    },
    /// 当前阶段不允许该操作(如未开始就提交结论)
    InvalidStage {
        /// 出错时所处阶段
        at: ReviewStage,
    },
}

/// 一次对抗审查记录(纯状态,不含 IO / 事件发送)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdversarialReview {
    claim_id: String,
    claimant: String,
    /// 任务参与者集合(生成者 / 委托方 / 协作 agent);审查者必须**不属于**此集合
    participants: HashSet<String>,
    reviewer: String,
    stage: ReviewStage,
    rounds: u32,
}

impl AdversarialReview {
    /// 由一条已登记的 `Claim` 构造审查记录,初始阶段 [`ReviewStage::Pending`]。
    #[must_use]
    pub fn new(
        claim_id: impl Into<String>,
        claimant: impl Into<String>,
        participants: impl IntoIterator<Item = String>,
        reviewer: impl Into<String>,
    ) -> Self {
        Self {
            claim_id: claim_id.into(),
            claimant: claimant.into(),
            participants: participants.into_iter().collect(),
            reviewer: reviewer.into(),
            stage: ReviewStage::Pending,
            rounds: 0,
        }
    }

    /// 非参与者约束:`reviewer` 不在参与者集合内即满足异构要求。
    #[must_use]
    pub fn reviewer_is_non_participant(&self) -> bool {
        !self.participants.contains(&self.reviewer)
    }

    /// 当前阶段。
    #[must_use]
    pub fn stage(&self) -> ReviewStage {
        self.stage
    }

    /// 是否通过并具备 Certify 资格(§(b.1):UI 成功渲染只消费 Certify)。
    #[must_use]
    pub fn is_certify_eligible(&self) -> bool {
        self.stage == ReviewStage::ApprovedForCertify
    }

    /// 开始审查:`Pending → UnderReview`。强制校验非参与者约束,违反即拒绝。
    pub fn begin_review(&mut self) -> Result<(), ReviewError> {
        if self.stage != ReviewStage::Pending {
            return Err(ReviewError::InvalidStage { at: self.stage });
        }
        if !self.reviewer_is_non_participant() {
            return Err(ReviewError::ReviewerIsParticipant {
                reviewer: self.reviewer.clone(),
            });
        }
        self.stage = ReviewStage::UnderReview;
        self.rounds += 1;
        Ok(())
    }

    /// 提交结论:`UnderReview → ApprovedForCertify / Rejected / Deferred`。
    pub fn submit_verdict(&mut self, outcome: ReviewOutcome) -> Result<(), ReviewError> {
        if self.stage != ReviewStage::UnderReview {
            return Err(ReviewError::InvalidStage { at: self.stage });
        }
        self.stage = match outcome {
            ReviewOutcome::Approve => ReviewStage::ApprovedForCertify,
            ReviewOutcome::Reject => ReviewStage::Rejected,
            ReviewOutcome::Defer => ReviewStage::Deferred,
        };
        Ok(())
    }

    /// 复审:`Deferred → UnderReview`(同一记录追加一轮),仍校验非参与者约束。
    pub fn reopen_review(&mut self) -> Result<(), ReviewError> {
        if self.stage != ReviewStage::Deferred {
            return Err(ReviewError::InvalidStage { at: self.stage });
        }
        if !self.reviewer_is_non_participant() {
            return Err(ReviewError::ReviewerIsParticipant {
                reviewer: self.reviewer.clone(),
            });
        }
        self.stage = ReviewStage::UnderReview;
        self.rounds += 1;
        Ok(())
    }

    /// 已进行的审查轮次(供毕业判定与可观测)。
    #[must_use]
    pub fn rounds(&self) -> u32 {
        self.rounds
    }

    /// Claim ID(供 `CertifyIssued.claim_ref` / 黑板回写引用)。
    #[must_use]
    pub fn claim_id(&self) -> &str {
        &self.claim_id
    }
}

/// 郭子扬 Prompt 梯度测试法的毕业判据(§(b.4)(1)「毕业条件」)。
///
/// 输入为逐轮**增强审查 prompt 后新发现的缺陷数**序列;当发现数不再上升(末尾持平或
/// 归零)即视为审查强度收敛——继续加大 prompt 强度已无额外收益,审查者质量毕业。
/// 少于两轮时无从判定,返回 `false`(保守:未收敛)。
#[must_use]
pub fn prompt_gradient_converged(findings_per_round: &[usize]) -> bool {
    if findings_per_round.len() < 2 {
        return false;
    }
    let (last, prev) = (
        *findings_per_round.last().unwrap_or(&0),
        findings_per_round[findings_per_round.len() - 2],
    );
    // 末轮新发现 ≤ 前一轮 ⇒ 增益耗尽(含末轮为 0 的完全收敛)。
    last <= prev
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;

    fn review() -> AdversarialReview {
        AdversarialReview::new(
            "claim-1",
            "agent-A",
            ["agent-A".to_string(), "agent-B".to_string()],
            "skeptic-X",
        )
    }

    #[test]
    fn non_participant_reviewer_can_certify() {
        let mut r = review();
        assert!(r.reviewer_is_non_participant());
        r.begin_review().unwrap();
        assert_eq!(r.stage(), ReviewStage::UnderReview);
        r.submit_verdict(ReviewOutcome::Approve).unwrap();
        assert!(r.is_certify_eligible());
    }

    #[test]
    fn participant_reviewer_rejected() {
        // 审查者同时是参与者 => 违反非参与者约束,begin 即失败
        let mut r =
            AdversarialReview::new("claim-2", "agent-A", ["agent-A".to_string()], "agent-A");
        assert!(!r.reviewer_is_non_participant());
        assert_eq!(
            r.begin_review(),
            Err(ReviewError::ReviewerIsParticipant {
                reviewer: "agent-A".into()
            })
        );
    }

    #[test]
    fn verdict_before_begin_is_invalid() {
        let mut r = review();
        assert_eq!(
            r.submit_verdict(ReviewOutcome::Approve),
            Err(ReviewError::InvalidStage {
                at: ReviewStage::Pending
            })
        );
    }

    #[test]
    fn defer_then_reopen_rounds_increment() {
        let mut r = review();
        r.begin_review().unwrap();
        r.submit_verdict(ReviewOutcome::Defer).unwrap();
        assert_eq!(r.stage(), ReviewStage::Deferred);
        r.reopen_review().unwrap();
        assert_eq!(r.rounds(), 2);
        r.submit_verdict(ReviewOutcome::Reject).unwrap();
        assert_eq!(r.stage(), ReviewStage::Rejected);
        assert!(!r.is_certify_eligible());
    }

    #[test]
    fn prompt_gradient_convergence_rule() {
        assert!(!prompt_gradient_converged(&[]), "无样本不判收敛");
        assert!(!prompt_gradient_converged(&[5]), "单轮不判收敛");
        assert!(prompt_gradient_converged(&[5, 2, 0]), "递减归零=收敛");
        assert!(prompt_gradient_converged(&[3, 3]), "持平=增益耗尽");
        assert!(!prompt_gradient_converged(&[1, 4]), "上升=未收敛");
    }

    #[tokio::test]
    async fn parliament_certify_if_reviewed_end_to_end() {
        // v2.3 §(b.4)(1) 生产接线闭环:未过审不签发;非参与者审查通过→经 publish_certify
        // 发 CertifyIssued(Critical 旁路必达)。
        use crate::{Parliament, ParliamentConfig};
        use event_bus::{EventBus, NexusEvent};
        let bus = EventBus::new();
        let mut rx = bus.subscribe_critical_events();
        let p = Parliament::new(ParliamentConfig::default(), bus.clone());
        let mut review =
            AdversarialReview::new("c1", "agent-a", ["agent-a".to_string()], "skeptic-x");
        // Pending(未审查)→ 不签发
        assert!(!p.certify_if_reviewed(&review, "TestSuite").await);
        // 非参与者审查通过 → 签发
        review.begin_review().unwrap();
        review.submit_verdict(ReviewOutcome::Approve).unwrap();
        assert!(p.certify_if_reviewed(&review, "TestSuite").await);
        let ev = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("Certify 应经 Critical 旁路投递")
            .expect("mpsc 不应关闭");
        assert!(matches!(ev, NexusEvent::CertifyIssued { .. }));
    }
}
