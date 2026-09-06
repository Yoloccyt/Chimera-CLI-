//! EPTS — 快照沙箱评测流水线（P3-T13b，v4.0 WI-31）
//!
//! 对应架构层: **L5 Knowledge**（gsoe-evolution，ADR-151 裁决：挂既有 crate 增强）
//! 对应任务: **P3-T13b**（手册 W19，WI-31：Extractor→Generator→Judge 三段式）
//!
//! # 设计（v4.0 WI-31 规格）
//! - **Extractor**:从生产轨迹（TokenLedger/经验卡）提取任务模式（模板化）;
//! - **Generator**:基于模板合成可验证回归任务（参数化,周产 ≥20）;
//! - **Judge**:验证任务可解性（确定性规则:目标明确 + 断言可验证;
//!   快照沙箱试跑由宿主注入——本模块为规则判定）;
//! - **门禁**:Judge 通过率 <40% 暂停合成 + 人工抽检（防噪声任务）。
//!
//! # 红线
//! 纯 Rust 规则/统计,零模型组件（WI-31 红线:合成噪声可暂停）。

use std::collections::HashMap;

/// Judge 通过率暂停阈值 — <40% 暂停合成（WI-31 门禁）
pub const JUDGE_PASS_GATE: f64 = 0.4;
/// 周产任务目标 — ≥20（WI-31 验收口径）
pub const WEEKLY_TARGET: usize = 20;

/// 任务模板 — Extractor 产物（Generator 输入）
#[derive(Debug, Clone, PartialEq)]
pub struct TaskTemplate {
    /// 模板 ID
    pub template_id: String,
    /// 任务类型（代码/检索/配置）
    pub task_type: String,
    /// 目标模板（含 {param} 占位符）
    pub goal_template: String,
    /// 断言模板（含 {param} 占位符;可验证性依据）
    pub assert_template: String,
}

/// 合成任务 — Generator 产物（Judge 输入）
#[derive(Debug, Clone, PartialEq)]
pub struct SynthesizedTask {
    /// 任务 ID
    pub task_id: String,
    /// 来源模板
    pub template_id: String,
    /// 目标（占位符已填充）
    pub goal: String,
    /// 断言（占位符已填充）
    pub assertion: String,
}

/// Judge 判定 — 可解性验证
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JudgeVerdict {
    /// 可解（目标明确 + 断言可验证）
    Solvable,
    /// 不可解（目标含糊/断言不可验证）
    Unsolvable,
}

/// 提取器 — 从生产轨迹提取任务模板
///
/// 简化:输入为轨迹文本行（每条 = 一个已完成任务的目标+断言）,
/// 提取 {param} 参数化的通用模板（同类型聚合）。
pub struct TaskExtractor;

impl TaskExtractor {
    /// 从轨迹提取模板 — 目标/断言按任务类型聚合
    ///
    /// # 参数
    /// - `trajectories`:轨迹条目（task_type, goal, assertion）
    ///
    /// # 返回
    /// 提取的模板（去重:同 goal 形状聚合;至少 1 条才出模板）
    #[must_use]
    pub fn extract(&self, trajectories: &[(&str, &str, &str)]) -> Vec<TaskTemplate> {
        let mut by_type: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
        for (ty, goal, assertion) in trajectories {
            by_type.entry(ty).or_default().push((goal, assertion));
        }
        by_type
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(ty, v)| TaskTemplate {
                template_id: format!("tmpl-{}", ty),
                task_type: ty.to_string(),
                // 简化参数化:取首个轨迹的目标/断言为模板,数值/路径子串替换为 {param}
                goal_template: parameterize(v[0].0),
                assert_template: parameterize(v[0].1),
            })
            .collect()
    }
}

/// 参数化 — 将数字/引号路径替换为 {param} 占位符（简化规则）
fn parameterize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars = s.chars().peekable();
    let mut in_num = false;
    for c in chars {
        if c.is_ascii_digit() {
            if !in_num {
                out.push_str("{param}");
                in_num = true;
            }
        } else {
            in_num = false;
            out.push(c);
        }
    }
    out
}

/// 生成器 — 基于模板合成任务（参数化填充）
pub struct TaskGenerator;

impl TaskGenerator {
    /// 从模板合成 N 个任务（参数 = 序号,保证可验证性）
    ///
    /// # 返回
    /// 合成的任务列表（数量 = 模板数 × n）
    #[must_use]
    pub fn generate(&self, templates: &[TaskTemplate], n: usize) -> Vec<SynthesizedTask> {
        let mut tasks = Vec::new();
        for tmpl in templates {
            for i in 0..n {
                tasks.push(SynthesizedTask {
                    task_id: format!("{}-{}", tmpl.template_id, i),
                    template_id: tmpl.template_id.clone(),
                    goal: tmpl.goal_template.replace("{param}", &i.to_string()),
                    assertion: tmpl.assert_template.replace("{param}", &i.to_string()),
                });
            }
        }
        tasks
    }
}

/// 评判器 — 可解性验证（确定性规则;沙箱试跑由宿主注入）
pub struct TaskJudge;

impl TaskJudge {
    /// 判定任务可解性 — 目标非空 + 断言含可验证标记（`verify`/`assert`/`==`）
    #[must_use]
    pub fn judge(&self, task: &SynthesizedTask) -> JudgeVerdict {
        let goal_ok = !task.goal.trim().is_empty();
        let assert_ok = task.assertion.contains("verify")
            || task.assertion.contains("assert")
            || task.assertion.contains("==");
        if goal_ok && assert_ok {
            JudgeVerdict::Solvable
        } else {
            JudgeVerdict::Unsolvable
        }
    }
}

/// EPTS 流水线 — Extractor→Generator→Judge 组装
///
/// # 周产门禁
/// Judge 通过率 <40% → [`EptsStatus::Paused`]（暂停合成 + 人工抽检）。
pub struct EptsPipeline {
    /// 提取器
    pub extractor: TaskExtractor,
    /// 生成器
    pub generator: TaskGenerator,
    /// 评判器
    pub judge: TaskJudge,
}

impl Default for EptsPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl EptsPipeline {
    /// 新建流水线
    #[must_use]
    pub fn new() -> Self {
        Self {
            extractor: TaskExtractor,
            generator: TaskGenerator,
            judge: TaskJudge,
        }
    }

    /// 执行一周合成 — 提取 → 生成 → 判定,输出状态
    ///
    /// # 参数
    /// - `trajectories`:本周生产轨迹
    /// - `per_template`:每模板合成数（默认 2,周产 ≥20 需 ≥10 模板）
    ///
    /// # 返回
    /// 合成任务 + 通过率状态（<40% → Paused）
    #[must_use]
    pub fn run_week(
        &self,
        trajectories: &[(&str, &str, &str)],
        per_template: usize,
    ) -> (Vec<SynthesizedTask>, EptsStatus) {
        let templates = self.extractor.extract(trajectories);
        let tasks = self.generator.generate(&templates, per_template);
        let pass = tasks
            .iter()
            .filter(|t| self.judge.judge(t) == JudgeVerdict::Solvable)
            .count();
        let rate = if tasks.is_empty() {
            0.0
        } else {
            pass as f64 / tasks.len() as f64
        };
        let status = if tasks.is_empty() || rate < JUDGE_PASS_GATE {
            EptsStatus::Paused { pass_rate: rate }
        } else {
            EptsStatus::Active {
                pass_rate: rate,
                produced: tasks.len(),
            }
        };
        (tasks, status)
    }
}

/// 流水线状态 — 周度门禁
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EptsStatus {
    /// 活跃（通过率达标）
    Active {
        /// 通过率
        pass_rate: f64,
        /// 本周产出
        produced: usize,
    },
    /// 暂停（通过率 <40%,人工抽检）
    Paused {
        /// 通过率
        pass_rate: f64,
    },
}

impl EptsStatus {
    /// 是否暂停
    #[must_use]
    pub const fn is_paused(&self) -> bool {
        matches!(self, Self::Paused { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三段式 — 提取 → 生成 → 判定闭环（WI-31 主路径）
    #[test]
    fn pipeline_full_cycle() {
        let p = EptsPipeline::new();
        let trajectories = [
            (
                "code",
                "refactor function f to handle 10 items",
                "assert result == 10",
            ),
            (
                "code",
                "add retry for 3 attempts",
                "verify retry count == 3",
            ),
            ("search", "find docs for 5 apis", "assert count == 5"),
        ];
        let (tasks, status) = p.run_week(&trajectories, 2);
        assert!(!status.is_paused(), "全部可解必须活跃");
        // code×2 同类型合并为 1 模板 + search 1 模板 = 2 模板 × 2 = 4 任务
        assert_eq!(tasks.len(), 4, "2 模板 × 2 = 4 任务");
        match status {
            EptsStatus::Active {
                pass_rate,
                produced,
            } => {
                assert!((pass_rate - 1.0).abs() < 1e-9);
                assert_eq!(produced, 4, "2 模板 × 2 = 4");
            }
            _ => panic!("应活跃"),
        }
    }

    /// 暂停门禁 — 断言不可验证 → 通过率 <40% 暂停（WI-31）
    #[test]
    fn pause_on_low_pass_rate() {
        let p = EptsPipeline::new();
        // 目标含糊 + 断言不可验证（无 verify/assert/==）
        let trajectories = [
            ("code", "do something", "maybe ok"),
            ("code", "improve things", "hopefully fine"),
            ("code", "make it better", "we will see"),
        ];
        let (_tasks, status) = p.run_week(&trajectories, 1);
        assert!(status.is_paused(), "低通过率必须暂停（人工抽检）");
    }

    /// 空轨迹 — 暂停（无模板不产出）
    #[test]
    fn empty_trajectory_paused() {
        let p = EptsPipeline::new();
        let (_tasks, status) = p.run_week(&[], 1);
        assert!(status.is_paused());
    }

    /// 参数化 — 数字替换为 {param}
    #[test]
    fn parameterize_replaces_digits() {
        assert_eq!(parameterize("handle 10 items"), "handle {param} items");
        assert_eq!(parameterize("no numbers"), "no numbers");
        assert_eq!(parameterize("a 1 b 22 c"), "a {param} b {param} c");
    }

    /// Judge 判定 — 目标/断言双条件
    #[test]
    fn judge_solvability() {
        let j = TaskJudge;
        let ok = SynthesizedTask {
            task_id: "t1".into(),
            template_id: "x".into(),
            goal: "refactor f".into(),
            assertion: "assert result == 0".into(),
        };
        assert_eq!(j.judge(&ok), JudgeVerdict::Solvable);
        let bad_goal = SynthesizedTask {
            task_id: "t2".into(),
            template_id: "x".into(),
            goal: "  ".into(),
            assertion: "assert result == 0".into(),
        };
        assert_eq!(j.judge(&bad_goal), JudgeVerdict::Unsolvable, "空目标不可解");
        let bad_assert = SynthesizedTask {
            task_id: "t3".into(),
            template_id: "x".into(),
            goal: "refactor".into(),
            assertion: "maybe".into(),
        };
        assert_eq!(
            j.judge(&bad_assert),
            JudgeVerdict::Unsolvable,
            "断言不可验证"
        );
    }

    /// 周产目标 — ≥20 需 ≥10 模板（WI-31 口径可满足）
    #[test]
    fn weekly_target_reachable() {
        let p = EptsPipeline::new();
        let goals: Vec<String> = (0..10).map(|i| format!("task {i} items")).collect();
        let asserts: Vec<String> = (0..10).map(|i| format!("assert count == {i}")).collect();
        // 10 种任务类型（code0..code9）→ 10 模板（Extractor 按类型分组）
        let trajectories: Vec<(&str, &str, &str)> = (0..10)
            .map(|i| {
                // Box::leak 返回 &'static mut str;显式标注 &str 完成 coerce
                let ty: &'static str = Box::leak(format!("code{i}").into_boxed_str());
                (ty, goals[i].as_str(), asserts[i].as_str())
            })
            .collect();
        let (tasks, status) = p.run_week(&trajectories, 2);
        assert_eq!(tasks.len(), 20, "10 模板 × 2 = 20 ≥ 周产目标");
        assert!(!status.is_paused());
    }
}
