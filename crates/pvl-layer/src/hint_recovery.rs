//! Hint-Boosted Recovery — 过程级提示引导恢复(polish-v2.7 P3-6)
//!
//! 对应架构层:L7 Execution(pvl-layer 子模块)
//! 对应 ADR:ADR-049 决策 1(hint-recovery 落点 pvl-layer)
//! 对应设计源:`chimera_ultimate_polish_v2.7.md` §11(快手 KAT Hint-Boosted Recovery)
//!
//! # 核心思想(快手 KAT)
//!
//! 验证失败后不盲目重试:根据拒绝原因生成**过程级提示**注入下一次生产,
//! 并随失败次数分级升压(轻提示 → 具体指引 → 建议升级人工),
//! 避免 Producer 在同一错误上无提示地反复撞墙。
//!
//! # 设计决策(WHY)
//!
//! - **规则映射而非 LLM 生成**:提示模板按拒绝原因类别静态映射,
//!   确定性输出可测可审计(与 AEGIS-lite Planner 同款降级哲学,ADR-050)
//! - **三级升压**:attempt 1 = 类别提示;attempt 2 = 具体行动指引;
//!   attempt ≥3 = 建议 EscalateToHuman(对齐 seccore escalation 语义)

use serde::{Deserialize, Serialize};

use crate::types::VerificationResult;

/// 建议升级人工的失败次数阈值
///
/// WHY 3:两次带提示的重试仍失败,说明问题超出提示可修复范围,
/// 继续重试只烧预算(对应 §6.1 红线"所有异步操作必须有超时处理"的精神)。
const ESCALATE_ATTEMPT_THRESHOLD: u32 = 3;

/// 恢复提示类别 — 拒绝原因的规则分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HintCategory {
    /// 语法/格式类失败
    Syntax,
    /// 超时类失败
    Timeout,
    /// 安全/权限类失败
    Security,
    /// 低置信度被风险门控拒绝
    LowConfidence,
    /// 未分类失败(通用提示)
    Generic,
    /// 已达升级阈值,建议人工介入
    EscalateToHuman,
}

/// 恢复提示 — 注入下一次生产的过程级引导
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveryHint {
    /// 提示类别
    pub category: HintCategory,
    /// 提示文本(注入 Producer 的生产上下文)
    pub hint: String,
    /// 是否建议停止重试并升级人工
    pub should_escalate: bool,
}

/// 提示引导恢复器 — 无状态规则映射
///
/// 调用方(FeedbackChannel/上层编排)维护 per-operation 的失败计数,
/// 每次验证失败后调用 [`hint_for`](Self::hint_for) 获取分级提示。
#[derive(Debug, Default, Clone, Copy)]
pub struct HintRecovery;

impl HintRecovery {
    /// 创建恢复器
    pub fn new() -> Self {
        Self
    }

    /// 依据验证结果与已失败次数生成分级恢复提示
    ///
    /// # 参数
    /// - `result`:验证结果(passed=true 时返回 None,无需恢复)
    /// - `attempt`:本操作已失败次数(1 = 首次失败)
    pub fn hint_for(&self, result: &VerificationResult, attempt: u32) -> Option<RecoveryHint> {
        if result.passed {
            return None;
        }

        // 三级升压:达到阈值直接建议升级,不再生成修复提示
        if attempt >= ESCALATE_ATTEMPT_THRESHOLD {
            return Some(RecoveryHint {
                category: HintCategory::EscalateToHuman,
                hint: format!(
                    "操作已连续失败 {attempt} 次(最近原因: {}),建议停止自动重试并升级人工介入",
                    result.reason
                ),
                should_escalate: true,
            });
        }

        let reason = result.reason.to_lowercase();
        let (category, base_hint) = if reason.contains("syntax") || reason.contains("format") {
            (
                HintCategory::Syntax,
                "产出存在语法/格式问题,生成前先校验输出结构",
            )
        } else if reason.contains("timeout") {
            (
                HintCategory::Timeout,
                "执行超时,将操作拆分为更小步骤或减少单步范围",
            )
        } else if reason.contains("security")
            || reason.contains("forbidden")
            || reason.contains("sandbox")
        {
            (
                HintCategory::Security,
                "触发安全拦截,检查命令是否在白名单内并移除高危模式",
            )
        } else if reason.contains("confidence") {
            (
                HintCategory::LowConfidence,
                "置信度不足被风险门控拒绝,补充上下文证据后重新生成",
            )
        } else {
            (
                HintCategory::Generic,
                "验证未通过,对照拒绝原因逐项修正后重试",
            )
        };

        // attempt 2:在类别提示基础上追加具体原因(更强指引)
        let hint = if attempt >= 2 {
            format!("{base_hint};具体拒绝原因: {}", result.reason)
        } else {
            base_hint.to_string()
        };

        Some(RecoveryHint {
            category,
            hint,
            should_escalate: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::OperationId;

    fn rejected(reason: &str) -> VerificationResult {
        VerificationResult::rejected(OperationId::new("op-1"), reason)
    }

    #[test]
    fn test_passed_result_needs_no_hint() {
        let result = VerificationResult::passed(OperationId::new("op-1"));
        assert!(HintRecovery::new().hint_for(&result, 1).is_none());
    }

    #[test]
    fn test_category_mapping() {
        let recovery = HintRecovery::new();
        let cases = [
            ("syntax error at line 3", HintCategory::Syntax),
            ("operation timeout after 30s", HintCategory::Timeout),
            ("forbidden command pattern", HintCategory::Security),
            ("confidence below threshold", HintCategory::LowConfidence),
            ("something unexpected", HintCategory::Generic),
        ];
        for (reason, expected) in cases {
            let hint = recovery
                .hint_for(&rejected(reason), 1)
                .expect("失败结果应有提示");
            assert_eq!(hint.category, expected, "原因 '{reason}' 分类错误");
            assert!(!hint.should_escalate);
        }
    }

    #[test]
    fn test_second_attempt_appends_specific_reason() {
        let recovery = HintRecovery::new();
        let hint1 = recovery.hint_for(&rejected("timeout"), 1).unwrap();
        let hint2 = recovery.hint_for(&rejected("timeout"), 2).unwrap();
        // 第 2 次失败:提示升压,携带具体拒绝原因
        assert!(!hint1.hint.contains("具体拒绝原因"));
        assert!(hint2.hint.contains("具体拒绝原因"));
    }

    #[test]
    fn test_third_attempt_escalates_to_human() {
        let hint = HintRecovery::new()
            .hint_for(&rejected("timeout"), 3)
            .unwrap();
        assert_eq!(hint.category, HintCategory::EscalateToHuman);
        assert!(hint.should_escalate);
        assert!(hint.hint.contains("升级人工"));
    }
}
