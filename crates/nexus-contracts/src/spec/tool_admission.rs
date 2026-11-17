//! 工具批派发准入契约 —— P-A（ADR-新-3 的 L0 载体）
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts umbrella，`spec/` 组）
//!
//! # WHY 这个模块
//! 执行面的批/窗口/许可语义此前散在 L7 三个模块 + L10 一个模块
//! （gqep tool_plane 双车道、toolplan_runner 在飞窗口、tool_loop 双信号量），
//! L0 只有单发语义的 `tool_dispatch`。本模块补上**批级**契约类型——
//! "一次 LLM turn 的结构化派发"的载体（`BatchPlan`）与回执（`BatchReport`），
//! 使 L8/L9 可按依赖铁律 `L(N) → L(0)` 消费调度语义而不必上探 L7。
//!
//! # 设计约束
//! - **受控纯逻辑**（ADR-033，已登记例外模块）：除 `is_conserved` /
//!   `segment_counts` / `is_read_safe` 纯判定函数族外无逻辑；调度行为本体
//!   留在 L7（G-99 辖区）。
//! - **不 serde**：`PlannedCall` 内嵌无 serde 的 [`DispatchRequest`]
//!   （执行面类型不经线，与 `tool_dispatch` 家族同口径）。
//! - **四段守恒**对齐 gqep gatherer 先例：`succeeded + failed + denied +
//!   not_started == total`，`not_started` 显式可见（副作用型调用方的
//!   重试幂等判据）。
//! - **串行边界由类型表达**：`EffectClass::Unknown` 一律"可能写"，
//!   经 [`PlannedCall::is_read_safe`] 判否后消费方必须串行（fail-closed，
//!   与 toolplan_runner 的 `effective_side_effect` 同一保守口径）。

use super::tool_dispatch::DispatchRequest;
use super::tool_plan::guards;

/// 批级派发准入决策 —— 统一调度器"该不该现在派发"的语义
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// 立即派发
    Granted,
    /// 窗口已满，延后（许可只延后不拒绝）
    Deferred,
    /// 拒绝（可重试信号由 [`DenyReason`] 表达）
    Deny(DenyReason),
}

/// 工具副作用分类的统一真值源 —— 三态；`Unknown` 按"可能写"保守处理
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectClass {
    /// 只读（查询/检索类）
    ReadOnly,
    /// 写/副作用
    Write,
    /// 未知 —— 消费方必须按串行处理（fail-closed）
    Unknown,
}

/// 拒绝归因 —— 随调度语义演进，下游必须保留 catch-all 臂
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DenyReason {
    /// 等待者超限（背压拒绝，可降级串行重放）
    Overloaded,
    /// 外部取消
    Cancelled,
    /// 策略层拒绝（消息为策略文本）
    Policy(String),
    /// 路由层未命中（消息为工具名）
    NotFound(String),
}

/// 批截断归因 —— 与 `PlanRunError` 的 TimedOut/Cancelled 及
/// `TruncationReason` 的 BudgetBytes 语义对齐
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncationCause {
    /// 批级绝对 deadline 到期
    Deadline,
    /// 外部取消
    Cancelled,
    /// 回填/结果预算超限
    Budget,
}

/// 批内单条调用结果 —— 四段守恒的段值
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallOutcome {
    /// 成功
    Succeeded {
        /// 批内序号（归因键）
        seq: u32,
        /// 输出文本
        output: String,
    },
    /// 执行失败
    Failed {
        /// 批内序号（归因键）
        seq: u32,
        /// 错误文本
        error: String,
    },
    /// 被拒绝
    Denied {
        /// 批内序号（归因键）
        seq: u32,
        /// 拒绝归因
        reason: DenyReason,
    },
    /// 从未派发（显式可见，重试幂等判据）
    NotStarted,
}

/// 批内单条计划 —— 结构化派发载体（执行面，构造必经 [`PlannedCall::new`]）
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PlannedCall {
    /// 批内序号（声明序；结果归因与局部序的键）
    pub seq: u32,
    /// 派发请求（拥有型边界类型）
    pub req: DispatchRequest,
    /// 副作用分类（Unknown 必串行）
    pub effect: EffectClass,
    /// 是否需要人工审批（true 的调用走 ask 窗口，不阻塞无审批调用）
    pub approval_required: bool,
}

impl PlannedCall {
    /// 构造一条批内计划
    #[must_use]
    pub fn new(
        seq: u32,
        req: DispatchRequest,
        effect: EffectClass,
        approval_required: bool,
    ) -> Self {
        Self {
            seq,
            req,
            effect,
            approval_required,
        }
    }

    /// 是否读安全 —— 唯一判据：`effect == ReadOnly`。
    /// `Unknown`/`Write` 返回 false，消费方必须串行（fail-closed 口径）。
    #[must_use]
    pub fn is_read_safe(&self) -> bool {
        self.effect == EffectClass::ReadOnly
    }
}

/// 批策略 —— 窗口/失败/截止三旋钮（默认值与 gqep `PlanGuards` 基线对齐）
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct BatchPolicy {
    /// 在飞上限（0 = 由消费方自决；消费方应钳到 ≥ 1）
    pub max_in_flight: usize,
    /// true = 任一调用失败立即终止整批
    pub fail_fast: bool,
    /// 批级绝对 deadline（ms；0 = 消费方自决）
    pub deadline_ms: u64,
}

impl BatchPolicy {
    /// 构造批策略
    #[must_use]
    pub fn new(max_in_flight: usize, fail_fast: bool, deadline_ms: u64) -> Self {
        Self {
            max_in_flight,
            fail_fast,
            deadline_ms,
        }
    }
}

impl Default for BatchPolicy {
    /// 默认基线：窗口 8 / fail-soft / 30s（与 `PlanGuards` 默认同值——
    /// 两处数字观感一致但语义独立，禁止互相写入）
    fn default() -> Self {
        Self {
            max_in_flight: 8,
            fail_fast: false,
            deadline_ms: guards::MAX_TIMEOUT_MS,
        }
    }
}

/// 批计划 —— 一次 LLM turn 的结构化派发载体（取代平批 `Vec`）
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct BatchPlan {
    /// 批内调用（seq 单调）
    pub calls: Vec<PlannedCall>,
    /// 批策略
    pub policy: BatchPolicy,
}

impl BatchPlan {
    /// 构造批计划
    #[must_use]
    pub fn new(calls: Vec<PlannedCall>, policy: BatchPolicy) -> Self {
        Self { calls, policy }
    }

    /// 读安全调用数（可并发段的规模上界）
    #[must_use]
    pub fn read_safe_count(&self) -> usize {
        self.calls.iter().filter(|c| c.is_read_safe()).count()
    }
}

/// 批回执 —— 四段守恒：`(succeeded, failed, denied, not_started)` 之和 == 批内总数
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct BatchReport {
    /// 逐条结果（顺序不限，seq 归因）
    pub outcomes: Vec<CallOutcome>,
    /// 截断归因（None = 全量收敛）
    pub truncated: Option<TruncationCause>,
}

impl BatchReport {
    /// 构造批回执
    #[must_use]
    pub fn new(outcomes: Vec<CallOutcome>, truncated: Option<TruncationCause>) -> Self {
        Self {
            outcomes,
            truncated,
        }
    }

    /// 四段计数（succeeded, failed, denied, not_started）
    #[must_use]
    pub fn segment_counts(&self) -> (usize, usize, usize, usize) {
        let mut counts = (0usize, 0usize, 0usize, 0usize);
        for o in &self.outcomes {
            match o {
                CallOutcome::Succeeded { .. } => counts.0 += 1,
                CallOutcome::Failed { .. } => counts.1 += 1,
                CallOutcome::Denied { .. } => counts.2 += 1,
                CallOutcome::NotStarted => counts.3 += 1,
            }
        }
        counts
    }

    /// 四段守恒判定：`succeeded + failed + denied + not_started == total`
    #[must_use]
    pub fn is_conserved(&self, total: usize) -> bool {
        let (s, f, d, n) = self.segment_counts();
        s + f + d + n == total
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;

    fn req(name: &str) -> DispatchRequest {
        DispatchRequest {
            tool_name: name.to_string(),
            args_json: "{}".to_string(),
        }
    }

    #[test]
    fn unknown_is_not_read_safe() {
        // 不变量：EffectClass::Unknown 的调用必须串行（fail-closed 口径）
        for effect in [EffectClass::Write, EffectClass::Unknown] {
            let call = PlannedCall::new(0, req("t"), effect, false);
            assert!(!call.is_read_safe(), "{effect:?} 不得读安全");
        }
        assert!(PlannedCall::new(0, req("t"), EffectClass::ReadOnly, false).is_read_safe());
    }

    #[test]
    fn default_policy_matches_plan_guards_baseline() {
        let p = BatchPolicy::default();
        assert_eq!(p.max_in_flight, 8);
        assert!(!p.fail_fast);
        assert_eq!(p.deadline_ms, guards::MAX_TIMEOUT_MS);
    }

    #[test]
    fn four_segment_conservation_holds() {
        let report = BatchReport::new(
            vec![
                CallOutcome::Succeeded {
                    seq: 0,
                    output: "ok".into(),
                },
                CallOutcome::Failed {
                    seq: 1,
                    error: "boom".into(),
                },
                CallOutcome::Denied {
                    seq: 2,
                    reason: DenyReason::Overloaded,
                },
                CallOutcome::NotStarted,
            ],
            None,
        );
        assert!(report.is_conserved(4));
        assert!(!report.is_conserved(3));
        assert_eq!(report.segment_counts(), (1, 1, 1, 1));
    }

    #[test]
    fn truncated_reports_still_conserve() {
        let report = BatchReport::new(
            vec![CallOutcome::NotStarted, CallOutcome::NotStarted],
            Some(TruncationCause::Deadline),
        );
        assert!(report.is_conserved(2));
    }
}
