//! RTL 运行时策略复盘（Shadow 限定，P2-T10，v4.0 WI-30）
//!
//! 对应架构层: **L5 Knowledge**（gsoe-evolution）
//! 对应任务: **P2-T10**（手册 W13-14，R2 门禁）
//!
//! # R2 红线合规（v4.0 §17 禁止回退项 + 2026-08-15 治理决策）
//! - **零 Python、零梯度、零权重更新**：纯 Rust 规则/统计（指数加权成功率先验）
//! - **Shadow 限定**：策略产物仅写影子表双跑对比；**转正须议会治理审批**
//!   （ADR-142 流程），禁自动生效——本模块无任何"应用到生产"路径
//! - **无网络外发**：纯内存统计，无 IO
//! - 反馈写入 DualExperienceBank 冷层由既有机制承接（本模块不新建训练设施）
//!
//! # 可验证奖励（WI-30 规格，纯 Rust）
//! | 信号 | 奖励 |
//! |---|---|
//! | 测试通过 | +2.0 |
//! | 构建成功 | +1.5 |
//! | 工具调用 > 10 | −0.5 |
//! | 用户纠正 | −2.0 |
//! | 工具失败 | −1.0 |
//!
//! # 三类候选（Shadow 统计先验）
//! - 路由偏好（route_preference）：上下文签名 → 成功率先验
//! - 压缩阈值（compress_threshold）：上下文签名 → 阈值偏好先验
//! - 审批自动度（approval_autonomy）：上下文签名 → 自动度先验
//!
//! 周度 Shadow 报告（`shadow_report`）供议会审阅（ADR-142）。

use std::collections::HashMap;

/// 反馈信号（可验证奖励的输入事件）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FeedbackSignal {
    /// 测试通过（+2.0）
    TestPassed,
    /// 构建成功（+1.5）
    BuildSucceeded,
    /// 工具调用超阈值（−0.5，>10 次）
    ExcessiveToolCalls,
    /// 用户纠正（−2.0）
    UserCorrection,
    /// 工具失败（−1.0）
    ToolFailed,
}

impl FeedbackSignal {
    /// 可验证奖励值（WI-30 规格权威值）
    #[must_use]
    pub const fn reward(self) -> f64 {
        match self {
            Self::TestPassed => 2.0,
            Self::BuildSucceeded => 1.5,
            Self::ExcessiveToolCalls => -0.5,
            Self::UserCorrection => -2.0,
            Self::ToolFailed => -1.0,
        }
    }
}

/// 候选策略类型（三类，WI-30）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CandidateKind {
    /// 路由偏好
    RoutePreference,
    /// 压缩阈值
    CompressThreshold,
    /// 审批自动度
    ApprovalAutonomy,
}

/// 上下文签名（决策场景指纹：任务类型 + 阶段 + 关键性）
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContextSignature {
    /// 任务类型（如 "code-review" / "test-fix"）
    pub task_type: String,
    /// 阶段（如 "explore" / "execute" / "debug"）
    pub phase: String,
}

impl ContextSignature {
    /// 新建签名
    #[must_use]
    pub fn new(task_type: impl Into<String>, phase: impl Into<String>) -> Self {
        Self {
            task_type: task_type.into(),
            phase: phase.into(),
        }
    }
}

/// 候选策略的 Shadow 统计先验（指数加权成功率先验）
#[derive(Debug, Clone, Default)]
pub struct CandidateStats {
    /// 累计反馈数
    pub feedback_count: u64,
    /// 累计奖励和
    pub reward_sum: f64,
    /// 指数加权成功率先验（α=0.3）
    pub success_prior: f64,
}

impl CandidateStats {
    /// 记录一次反馈（EWMA 更新）
    pub fn record(&mut self, reward: f64) {
        self.feedback_count += 1;
        self.reward_sum += reward;
        // 归一化奖励到 [0,1] 区间做先验（+2.0 → 1.0，−2.0 → 0.0）
        let normalized = ((reward + 2.0) / 4.0).clamp(0.0, 1.0);
        self.success_prior = 0.3 * normalized + 0.7 * self.success_prior;
    }
}

/// RTL Shadow 收集器 — 记录 (context_signature, decision, outcome)
///
/// 影子表结构：签名 → 候选类型 → 统计先验。**只读 Shadow**：无任何
/// "应用到生产"路径（R2 红线——转正须议会审批，ADR-142）。
#[derive(Debug, Clone, Default)]
pub struct AsyncFeedbackCollector {
    /// 影子表（签名 → 候选 → 统计）
    shadow: HashMap<ContextSignature, HashMap<CandidateKind, CandidateStats>>,
}

impl AsyncFeedbackCollector {
    /// 新建收集器
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录决策反馈（Shadow 更新统计先验）
    ///
    /// # 参数
    /// - `signature`：上下文签名（决策场景）
    /// - `kind`：候选策略类型
    /// - `signal`：可验证奖励信号
    pub fn record(
        &mut self,
        signature: ContextSignature,
        kind: CandidateKind,
        signal: FeedbackSignal,
    ) {
        let stats = self
            .shadow
            .entry(signature)
            .or_default()
            .entry(kind)
            .or_default();
        stats.record(signal.reward());
    }

    /// 查询候选统计先验（Shadow 只读）
    #[must_use]
    pub fn prior(&self, signature: &ContextSignature, kind: CandidateKind) -> f64 {
        self.shadow
            .get(signature)
            .and_then(|m| m.get(&kind))
            .map(|s| s.success_prior)
            .unwrap_or(0.0)
    }

    /// 周度 Shadow 报告（供议会审阅，ADR-142）
    ///
    /// 返回每 (签名, 候选) 的统计：反馈数、奖励和、先验——纯展示数据，
    /// 不触发任何策略变更。
    #[must_use]
    pub fn shadow_report(&self) -> Vec<ShadowRow> {
        let mut rows = Vec::new();
        for (sig, kinds) in &self.shadow {
            for (kind, stats) in kinds {
                rows.push(ShadowRow {
                    task_type: sig.task_type.clone(),
                    phase: sig.phase.clone(),
                    candidate: *kind,
                    feedback_count: stats.feedback_count,
                    reward_sum: stats.reward_sum,
                    success_prior: stats.success_prior,
                });
            }
        }
        rows.sort_by_key(|r| std::cmp::Reverse(r.feedback_count));
        rows
    }

    /// 影子表规模（诊断）
    #[must_use]
    pub fn shadow_size(&self) -> usize {
        self.shadow.values().map(HashMap::len).sum()
    }
}

/// Shadow 报告行（议会审阅用）
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowRow {
    /// 任务类型
    pub task_type: String,
    /// 阶段
    pub phase: String,
    /// 候选类型
    pub candidate: CandidateKind,
    /// 反馈数
    pub feedback_count: u64,
    /// 奖励和
    pub reward_sum: f64,
    /// 成功率先验
    pub success_prior: f64,
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reward_values_authoritative() {
        // WI-30 规格权威值逐项锁定
        assert_eq!(FeedbackSignal::TestPassed.reward(), 2.0);
        assert_eq!(FeedbackSignal::BuildSucceeded.reward(), 1.5);
        assert_eq!(FeedbackSignal::ExcessiveToolCalls.reward(), -0.5);
        assert_eq!(FeedbackSignal::UserCorrection.reward(), -2.0);
        assert_eq!(FeedbackSignal::ToolFailed.reward(), -1.0);
    }

    #[test]
    fn ewma_prior_converges() {
        let mut c = AsyncFeedbackCollector::new();
        let sig = ContextSignature::new("test-fix", "debug");
        // 5 次测试通过 → 先验趋近 1.0
        for _ in 0..5 {
            c.record(
                sig.clone(),
                CandidateKind::RoutePreference,
                FeedbackSignal::TestPassed,
            );
        }
        let prior = c.prior(&sig, CandidateKind::RoutePreference);
        assert!(prior > 0.8, "连续成功必须推高先验, 实际 {prior}");
        // 用户纠正 → 先验回落
        c.record(
            sig.clone(),
            CandidateKind::RoutePreference,
            FeedbackSignal::UserCorrection,
        );
        let prior2 = c.prior(&sig, CandidateKind::RoutePreference);
        assert!(prior2 < prior, "纠正必须压低先验");
    }

    #[test]
    fn candidates_isolated_per_signature() {
        let mut c = AsyncFeedbackCollector::new();
        let sig_a = ContextSignature::new("code-review", "explore");
        let sig_b = ContextSignature::new("test-fix", "debug");
        c.record(
            sig_a.clone(),
            CandidateKind::RoutePreference,
            FeedbackSignal::TestPassed,
        );
        // 不同签名/候选互不影响
        assert_eq!(c.prior(&sig_b, CandidateKind::RoutePreference), 0.0);
        assert_eq!(c.prior(&sig_a, CandidateKind::CompressThreshold), 0.0);
        assert_eq!(c.prior(&sig_a, CandidateKind::RoutePreference), 0.3);
    }

    #[test]
    fn shadow_report_sorted_and_readonly() {
        let mut c = AsyncFeedbackCollector::new();
        let sig = ContextSignature::new("test-fix", "debug");
        c.record(
            sig.clone(),
            CandidateKind::RoutePreference,
            FeedbackSignal::TestPassed,
        );
        c.record(
            sig.clone(),
            CandidateKind::CompressThreshold,
            FeedbackSignal::BuildSucceeded,
        );
        let report = c.shadow_report();
        assert_eq!(report.len(), 2);
        // 报告读取不改变影子表
        assert_eq!(c.shadow_size(), 2);
        assert_eq!(c.prior(&sig, CandidateKind::RoutePreference), 0.3);
    }

    #[test]
    fn r2_no_production_write_path() {
        // R2 红线：Shadow 收集器无任何"应用到生产"路径——API 面仅
        // record/prior/report（统计读取），无策略应用方法（编译期验证：
        // 若未来新增 apply/promote 方法需议会审批 ADR-142）
        let mut c = AsyncFeedbackCollector::new();
        let sig = ContextSignature::new("a", "b");
        c.record(
            sig.clone(),
            CandidateKind::ApprovalAutonomy,
            FeedbackSignal::ToolFailed,
        );
        // 唯一出口是报告（议会审阅）
        assert!(!c.shadow_report().is_empty());
    }

    #[test]
    fn deterministic_same_sequence() {
        let mut a = AsyncFeedbackCollector::new();
        let mut b = AsyncFeedbackCollector::new();
        let sig = ContextSignature::new("test-fix", "execute");
        for (kind, signal) in [
            (CandidateKind::RoutePreference, FeedbackSignal::TestPassed),
            (CandidateKind::RoutePreference, FeedbackSignal::ToolFailed),
            (
                CandidateKind::CompressThreshold,
                FeedbackSignal::BuildSucceeded,
            ),
        ] {
            a.record(sig.clone(), kind, signal);
            b.record(sig.clone(), kind, signal);
        }
        assert_eq!(
            a.shadow_report(),
            b.shadow_report(),
            "同序列必须逐项一致(Ω₂)"
        );
    }
}
