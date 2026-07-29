//! 流程蓝图契约 — 隐性流程经验的结构化载体(polish-v2.7 P4-7)
//!
//! 对应架构层: L0 Contracts(新建)
//! 对应 ADR: ADR-049 决策 1(blueprint 类型归 L0)
//! 对应设计源: `chimera_ultimate_polish_v2.7.md` §4.2 / §9.3(北大 DataFlow
//! NL2Pipeline gap:说明书上没写的隐性流程约束)
//!
//! # 设计决策(WHY)
//!
//! - **纯类型 + 纯函数校验**: 遵循 ADR-033;`validate_plan` 是无副作用的
//!   契约校验(与 `HarnessSpec::validate` 同款模式),蓝图的提取逻辑在
//!   L5 repo-wiki(procedural_blueprint 模块)
//! - **消费层**: L5 repo-wiki(轨迹提取与沉淀)/ L9 quest-engine
//!   (计划生成后按蓝图预检,Phase 5+ 接入)

use serde::{Deserialize, Serialize};

/// 蓝图来源
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlueprintSource {
    /// 人工编写
    Manual,
    /// 从成功轨迹自动提取
    Extracted,
}

/// 蓝图步骤
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlueprintStep {
    /// 步骤序号(0 起)
    pub step_order: u32,
    /// 本步骤所需能力/工具标识
    pub required_capability: String,
    /// 前置条件断言集合
    pub preconditions: Vec<String>,
}

/// 计划校验错误(单条)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlanViolation {
    /// 步骤数不匹配
    StepCountMismatch {
        /// 蓝图期望的步骤数
        expected: usize,
        /// 计划的实际步骤数
        actual: usize,
    },
    /// 某步骤使用了错误的能力
    WrongCapability {
        /// 违规步骤序号
        step: usize,
        /// 蓝图期望的能力
        expected: String,
        /// 计划实际使用的能力
        actual: String,
    },
    /// 某步骤缺失蓝图要求的前置条件
    MissingPrecondition {
        /// 违规步骤序号
        step: usize,
        /// 缺失的前置条件
        missing: String,
    },
}

/// 流程蓝图 — 任务类型的已验证执行流程模板
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProceduralBlueprint {
    /// 蓝图唯一标识
    pub blueprint_id: String,
    /// 适用任务类型
    pub task_type: String,
    /// 有序步骤集合
    pub steps: Vec<BlueprintStep>,
    /// 历史成功率 [0.0, 1.0]
    pub success_rate: f32,
    /// 来源
    pub source: BlueprintSource,
    /// 被复用次数
    pub usage_count: u32,
}

impl ProceduralBlueprint {
    /// 校验计划是否符合蓝图 — 纯函数契约校验(方案 §9.3)
    ///
    /// `plan`:计划的步骤序列,每项为 `(capability, preconditions)`。
    /// 返回全部违规项(空 Vec = 通过)。
    pub fn validate_plan(&self, plan: &[(String, Vec<String>)]) -> Vec<PlanViolation> {
        let mut violations = Vec::new();

        if plan.len() != self.steps.len() {
            violations.push(PlanViolation::StepCountMismatch {
                expected: self.steps.len(),
                actual: plan.len(),
            });
        }

        for (i, (blueprint_step, (capability, preconditions))) in
            self.steps.iter().zip(plan.iter()).enumerate()
        {
            if &blueprint_step.required_capability != capability {
                violations.push(PlanViolation::WrongCapability {
                    step: i,
                    expected: blueprint_step.required_capability.clone(),
                    actual: capability.clone(),
                });
            }
            for required in &blueprint_step.preconditions {
                if !preconditions.contains(required) {
                    violations.push(PlanViolation::MissingPrecondition {
                        step: i,
                        missing: required.clone(),
                    });
                }
            }
        }
        violations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blueprint() -> ProceduralBlueprint {
        ProceduralBlueprint {
            blueprint_id: "bp-1".into(),
            task_type: "code_fix".into(),
            steps: vec![
                BlueprintStep {
                    step_order: 0,
                    required_capability: "read_file".into(),
                    preconditions: vec!["file_exists".into()],
                },
                BlueprintStep {
                    step_order: 1,
                    required_capability: "edit_file".into(),
                    preconditions: vec![],
                },
            ],
            success_rate: 1.0,
            source: BlueprintSource::Extracted,
            usage_count: 0,
        }
    }

    #[test]
    fn test_conforming_plan_passes() {
        let plan = vec![
            ("read_file".to_string(), vec!["file_exists".to_string()]),
            ("edit_file".to_string(), vec![]),
        ];
        assert!(blueprint().validate_plan(&plan).is_empty());
    }

    #[test]
    fn test_wrong_capability_and_missing_precondition_detected() {
        let plan = vec![
            ("delete_file".to_string(), vec![]), // 错误能力 + 缺前置
            ("edit_file".to_string(), vec![]),
        ];
        let violations = blueprint().validate_plan(&plan);
        assert_eq!(violations.len(), 2);
        assert!(matches!(
            violations[0],
            PlanViolation::WrongCapability { step: 0, .. }
        ));
        assert!(matches!(
            violations[1],
            PlanViolation::MissingPrecondition { step: 0, .. }
        ));
    }

    #[test]
    fn test_step_count_mismatch_detected() {
        let plan = vec![("read_file".to_string(), vec!["file_exists".to_string()])];
        let violations = blueprint().validate_plan(&plan);
        assert!(violations.iter().any(|v| matches!(
            v,
            PlanViolation::StepCountMismatch {
                expected: 2,
                actual: 1
            }
        )));
    }
}
