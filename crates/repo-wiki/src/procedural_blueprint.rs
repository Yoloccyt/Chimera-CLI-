//! Procedural Blueprint 提取器 — 从成功轨迹沉淀流程蓝图(polish-v2.7 P4-7)
//!
//! 对应架构层:L5 Knowledge(repo-wiki 子模块)
//! 对应 ADR:ADR-049 决策 1(procedural-blueprint 落点 repo-wiki)
//! 对应设计源:`chimera_ultimate_polish_v2.7.md` §9.3(北大 DataFlow 流程经验注入)
//!
//! # 职责分工
//!
//! - **类型与校验**:`nexus_contracts::ProceduralBlueprint`(L0 纯类型 +
//!   纯函数 `validate_plan`)
//! - **本模块**:轨迹 → 蓝图的提取与库管理(按任务类型索引 + 复用计数)

use std::collections::HashMap;

use nexus_contracts::{BlueprintSource, BlueprintStep, ProceduralBlueprint};
use uuid::Uuid;

/// 成功轨迹的步骤观测 — 提取器输入
#[derive(Debug, Clone)]
pub struct TrajectoryStep {
    /// 本步骤使用的能力/工具
    pub capability: String,
    /// 本步骤满足的前置条件
    pub preconditions: Vec<String>,
}

/// 蓝图库 — 按任务类型索引的蓝图集合
#[derive(Debug, Default)]
pub struct BlueprintLibrary {
    /// task_type → 蓝图列表
    by_task_type: HashMap<String, Vec<ProceduralBlueprint>>,
}

impl BlueprintLibrary {
    /// 创建空蓝图库
    pub fn new() -> Self {
        Self::default()
    }

    /// 库内蓝图总数
    pub fn len(&self) -> usize {
        self.by_task_type.values().map(Vec::len).sum()
    }

    /// 库是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 从成功轨迹提取蓝图并入库(方案 §9.3 from_trajectory)
    ///
    /// 空轨迹不入库(无步骤的蓝图无验证价值)。
    pub fn extract_from_trajectory(
        &mut self,
        task_type: impl Into<String>,
        steps: &[TrajectoryStep],
    ) -> Option<&ProceduralBlueprint> {
        if steps.is_empty() {
            return None;
        }
        let task_type = task_type.into();
        let blueprint = ProceduralBlueprint {
            blueprint_id: format!("bp-{}", Uuid::now_v7()),
            task_type: task_type.clone(),
            steps: steps
                .iter()
                .enumerate()
                .map(|(i, s)| BlueprintStep {
                    step_order: i as u32,
                    required_capability: s.capability.clone(),
                    preconditions: s.preconditions.clone(),
                })
                .collect(),
            // 提取自成功轨迹,初始成功率 1.0(后续按复用反馈衰减,本期不实现在线更新)
            success_rate: 1.0,
            source: BlueprintSource::Extracted,
            usage_count: 0,
        };
        let bucket = self.by_task_type.entry(task_type).or_default();
        bucket.push(blueprint);
        bucket.last()
    }

    /// 按任务类型查询最佳蓝图(成功率最高;复用时计数 +1)
    pub fn best_for(&mut self, task_type: &str) -> Option<&ProceduralBlueprint> {
        let bucket = self.by_task_type.get_mut(task_type)?;
        let best_idx = bucket
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                a.success_rate
                    .partial_cmp(&b.success_rate)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)?;
        bucket[best_idx].usage_count += 1;
        Some(&bucket[best_idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps() -> Vec<TrajectoryStep> {
        vec![
            TrajectoryStep {
                capability: "read_file".into(),
                preconditions: vec!["file_exists".into()],
            },
            TrajectoryStep {
                capability: "edit_file".into(),
                preconditions: vec![],
            },
        ]
    }

    #[test]
    fn test_extract_and_recall_blueprint() {
        let mut lib = BlueprintLibrary::new();
        let bp = lib
            .extract_from_trajectory("code_fix", &steps())
            .expect("非空轨迹应入库");
        assert_eq!(bp.steps.len(), 2);
        assert_eq!(bp.source, BlueprintSource::Extracted);

        let best = lib.best_for("code_fix").expect("应命中蓝图");
        assert_eq!(best.usage_count, 1, "复用应计数");
        assert!(lib.best_for("unknown_type").is_none());
    }

    #[test]
    fn test_empty_trajectory_not_extracted() {
        let mut lib = BlueprintLibrary::new();
        assert!(lib.extract_from_trajectory("code_fix", &[]).is_none());
        assert!(lib.is_empty());
    }

    #[test]
    fn test_extracted_blueprint_validates_conforming_plan() {
        let mut lib = BlueprintLibrary::new();
        lib.extract_from_trajectory("code_fix", &steps());
        let bp = lib.best_for("code_fix").unwrap();
        // 提取的蓝图立即可用于计划校验(L0 纯函数)
        let plan = vec![
            ("read_file".to_string(), vec!["file_exists".to_string()]),
            ("edit_file".to_string(), vec![]),
        ];
        assert!(bp.validate_plan(&plan).is_empty());
    }
}
