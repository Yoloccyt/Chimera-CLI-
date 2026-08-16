//! 六类状态反馈集成器 — OpenMLE 全链路追踪（设计文档 §9.3）
//!
//! 对应架构层: **L4 Security**（seccore 子模块）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §9.3
//! 对应论文: 清华 OpenMLE（六类状态反馈全链路追踪）
//! 对应 ADR: ADR-049 决策 1（内嵌 seccore）
//!
//! # 核心职责
//!
//! 将执行的原始信号（成功/输出/提交/评分/超时/错误输出）分类为 L0
//! [`ExecutionStatus`] 六类状态（铁律8 全链路追踪）：
//! Success / Error / MissingCode / NoSubmit / ScoreFailed / Timeout
//!
//! # 分类优先级（规范 §9.3）
//!
//! ```text
//! Timeout → (!success → Error/ScoreFailed) → NoSubmit → MissingCode → ScoreFailed → Success
//! ```
//!
//! # 设计约束
//!
//! - **铁律8**: 六类状态全链路追踪纯函数
//! - **D-5**: 纯函数零副作用，不发事件（分类纯函数，事件由调用方发布，Ω₄-Event）
//! - **消费 L0**: 返回 `nexus_contracts::ExecutionStatus`

use nexus_contracts::experience_card::ExecutionStatus;

/// 六类状态反馈集成器 — 执行信号分类纯函数
///
/// 无状态纯函数集合（关联函数），无内部字段。
#[derive(Debug, Default, Clone, Copy)]
pub struct ExecutionFeedbackIntegrator;

impl ExecutionFeedbackIntegrator {
    /// 分类执行状态 — 六类状态全链路追踪（铁律8）
    ///
    /// # 参数
    /// - `success`: 执行是否成功（退出码 0 / 构建通过）
    /// - `has_output`: 是否产生输出（代码/产物）
    /// - `has_submission`: 是否提交评分
    /// - `score`: 评分（None = 未评分）
    /// - `timed_out`: 是否超时
    /// - `error_output`: 错误输出（Some = 有错误信息）
    ///
    /// # 返回
    /// 分类后的 [`ExecutionStatus`]（六类之一）
    ///
    /// # 分类优先级
    /// 1. `timed_out` → Timeout
    /// 2. `!success` → 有错误输出 Error / 否则 ScoreFailed
    /// 3. `!has_submission` → NoSubmit
    /// 4. `!has_output` → MissingCode
    /// 5. `score.is_none()` → ScoreFailed
    /// 6. 否则 → Success
    pub fn classify(
        success: bool,
        has_output: bool,
        has_submission: bool,
        score: Option<f32>,
        timed_out: bool,
        error_output: Option<&str>,
    ) -> ExecutionStatus {
        if timed_out {
            return ExecutionStatus::Timeout;
        }
        if !success {
            if error_output.is_some() {
                return ExecutionStatus::Error;
            }
            return ExecutionStatus::ScoreFailed;
        }
        if !has_submission {
            return ExecutionStatus::NoSubmit;
        }
        if !has_output {
            return ExecutionStatus::MissingCode;
        }
        if score.is_none() {
            return ExecutionStatus::ScoreFailed;
        }
        ExecutionStatus::Success
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_timeout_highest_priority() {
        // 超时优先于一切（即使 success=true）
        let status = ExecutionFeedbackIntegrator::classify(true, true, true, Some(1.0), true, None);
        assert_eq!(status, ExecutionStatus::Timeout);
    }

    #[test]
    fn classify_error_with_error_output() {
        // !success + 有错误输出 → Error
        let status = ExecutionFeedbackIntegrator::classify(
            false,
            true,
            true,
            None,
            false,
            Some("compilation failed"),
        );
        assert_eq!(status, ExecutionStatus::Error);
    }

    #[test]
    fn classify_score_failed_without_error_output() {
        // !success + 无错误输出 → ScoreFailed
        let status = ExecutionFeedbackIntegrator::classify(false, true, true, None, false, None);
        assert_eq!(status, ExecutionStatus::ScoreFailed);
    }

    #[test]
    fn classify_no_submit() {
        // success + 未提交 → NoSubmit
        let status =
            ExecutionFeedbackIntegrator::classify(true, true, false, Some(0.8), false, None);
        assert_eq!(status, ExecutionStatus::NoSubmit);
    }

    #[test]
    fn classify_missing_code() {
        // success + 已提交 + 无输出 → MissingCode
        let status =
            ExecutionFeedbackIntegrator::classify(true, false, true, Some(0.8), false, None);
        assert_eq!(status, ExecutionStatus::MissingCode);
    }

    #[test]
    fn classify_score_failed_no_score() {
        // success + 已提交 + 有输出 + 未评分 → ScoreFailed
        let status = ExecutionFeedbackIntegrator::classify(true, true, true, None, false, None);
        assert_eq!(status, ExecutionStatus::ScoreFailed);
    }

    #[test]
    fn classify_success_full() {
        // 全满足 → Success
        let status =
            ExecutionFeedbackIntegrator::classify(true, true, true, Some(0.95), false, None);
        assert_eq!(status, ExecutionStatus::Success);
    }

    #[test]
    fn classify_all_six_statuses_reachable() {
        // 六类状态全部可达（铁律8 全覆盖）
        let timeout =
            ExecutionFeedbackIntegrator::classify(true, true, true, Some(1.0), true, None);
        let error =
            ExecutionFeedbackIntegrator::classify(false, true, true, None, false, Some("err"));
        let no_submit =
            ExecutionFeedbackIntegrator::classify(true, true, false, Some(0.5), false, None);
        let missing_code =
            ExecutionFeedbackIntegrator::classify(true, false, true, Some(0.5), false, None);
        let score_failed =
            ExecutionFeedbackIntegrator::classify(true, true, true, None, false, None);
        let success =
            ExecutionFeedbackIntegrator::classify(true, true, true, Some(0.9), false, None);
        assert_eq!(timeout, ExecutionStatus::Timeout);
        assert_eq!(error, ExecutionStatus::Error);
        assert_eq!(no_submit, ExecutionStatus::NoSubmit);
        assert_eq!(missing_code, ExecutionStatus::MissingCode);
        assert_eq!(score_failed, ExecutionStatus::ScoreFailed);
        assert_eq!(success, ExecutionStatus::Success);
    }

    #[test]
    fn classify_priority_order_timeout_over_error() {
        // timed_out + !success + error_output: Timeout 优先于 Error
        let status =
            ExecutionFeedbackIntegrator::classify(false, true, true, None, true, Some("err"));
        assert_eq!(status, ExecutionStatus::Timeout);
    }

    #[test]
    fn classify_priority_order_no_submit_over_missing_code() {
        // success + !has_submission + !has_output: NoSubmit 优先于 MissingCode
        let status =
            ExecutionFeedbackIntegrator::classify(true, false, false, Some(0.5), false, None);
        assert_eq!(status, ExecutionStatus::NoSubmit);
    }

    #[test]
    fn classify_is_pure_function() {
        // 铁律8: 纯函数，同输入同输出
        let s1 = ExecutionFeedbackIntegrator::classify(true, true, true, Some(0.9), false, None);
        let s2 = ExecutionFeedbackIntegrator::classify(true, true, true, Some(0.9), false, None);
        assert_eq!(s1, s2);
    }
}
