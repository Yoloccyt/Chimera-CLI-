//! 双层经验库 — 案例级 → 全局蒸馏（MemoHarness + OpenMLE 融合，设计文档 §7.2）
//!
//! 对应架构层: **L2 Memory**（mlc-engine 子模块）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §7.2
//! 对应论文: MemoHarness（双层经验库）+ OpenMLE（案例级经验）
//! 对应 ADR: ADR-049 决策 1（dual-experience-bank 落点 mlc-engine，内嵌模块）
//!
//! # 核心职责
//!
//! 双层经验存储与自动蒸馏：
//! - **案例级库** (`case_bank`): 原始经验卡片 + 任务类型索引
//! - **全局库** (`global_bank`): 蒸馏后的成功/失败模式与有效策略
//! - **自动蒸馏**: 未蒸馏案例数达阈值时触发 `distill_global`（按 task_type 分组）
//! - **检索**: `retrieve(TaskQuery)` 返回全局经验 + 相似案例
//!
//! # 设计约束
//!
//! - **铁律3**: 案例卡片只读消费（蒸馏提取模式，不修改原卡片）
//! - **f32 红线**: GlobalExperience/SuccessPattern/StrategyRecord 含 f32，仅 PartialEq
//! - **蒸馏幂等**: 已蒸馏案例标记 `distilled=true`，不重复蒸馏

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use nexus_contracts::experience_card::ExecutionStatus;
use nexus_contracts::ExperienceCard;

// ============================================================
// 案例经验
// ============================================================

/// 案例经验 — 案例级库条目（卡片 + 任务类型 + 蒸馏状态）
#[derive(Clone, Debug)]
pub struct CaseExperience {
    /// 经验卡片（铁律3 只读）
    pub card: ExperienceCard,
    /// 任务类型（蒸馏分组键）
    pub task_type: String,
    /// 是否已蒸馏到全局库
    pub distilled: bool,
    /// 插入时间
    pub inserted_at: DateTime<Utc>,
}

// ============================================================
// 全局经验（蒸馏产物）
// ============================================================

/// 成功模式 — 高分方法的统计特征
#[derive(Clone, Debug, PartialEq)]
pub struct SuccessPattern {
    /// 方法家族
    pub method_family: String,
    /// 分数范围 (min, max)
    pub score_range: (f32, f32),
    /// 关键因素摘要
    pub key_factors: Vec<String>,
    /// 平均 token 消耗
    pub avg_token_usage: u32,
}

/// 失败模式 — 错误签名的聚类统计
#[derive(Clone, Debug, PartialEq)]
pub struct FailurePattern {
    /// 错误签名哈希
    pub error_signature: String,
    /// 错误类型
    pub error_type: String,
    /// 修复策略建议
    pub fix_strategy: String,
    /// 出现频率
    pub frequency: u32,
    /// 平均修复耗时（毫秒）
    pub avg_fix_time_ms: u64,
}

/// 策略记录 — 算子维度的有效性统计
#[derive(Clone, Debug, PartialEq)]
pub struct StrategyRecord {
    /// 策略维度（如 "operator"）
    pub dimension: String,
    /// 策略值（如 "Draft"）
    pub strategy_value: String,
    /// 平均改进
    pub avg_improvement: f32,
    /// 样本数
    pub sample_count: u32,
}

/// 全局经验 — 蒸馏后的跨案例知识
#[derive(Clone, Debug, PartialEq)]
pub struct GlobalExperience {
    /// 适用任务类型
    pub applicable_task_types: Vec<String>,
    /// 成功模式列表
    pub success_patterns: Vec<SuccessPattern>,
    /// 失败模式列表
    pub failure_patterns: Vec<FailurePattern>,
    /// 有效策略列表
    pub effective_strategies: Vec<StrategyRecord>,
    /// 蒸馏时间
    pub distilled_at: DateTime<Utc>,
    /// 置信度（案例数 / 阈值，clamp [0,1]）
    pub confidence: f32,
    /// 来源案例数
    pub source_case_count: usize,
}

// ============================================================
// 检索查询
// ============================================================

/// 任务查询 — 检索条件
#[derive(Clone, Debug)]
pub struct TaskQuery {
    /// 任务类型
    pub task_type: String,
    /// 最低分数过滤
    pub min_score: f32,
    /// 最大返回数
    pub max_results: usize,
}

/// 检索结果 — 全局经验 + 相似案例
#[derive(Clone, Debug)]
pub struct RetrievedExperiences<'a> {
    /// 匹配的全局经验
    pub global: Vec<&'a GlobalExperience>,
    /// 匹配的相似案例
    pub cases: Vec<&'a CaseExperience>,
}

// ============================================================
// 双层经验库
// ============================================================

/// 双层经验库 — 案例级存储 + 全局蒸馏 + 检索
#[derive(Debug)]
pub struct DualExperienceBank {
    /// 案例级库
    case_bank: Vec<CaseExperience>,
    /// 全局库（蒸馏产物）
    global_bank: Vec<GlobalExperience>,
    /// 蒸馏触发阈值（未蒸馏案例数）
    distill_threshold: usize,
    /// 最近蒸馏时间
    last_distillation: DateTime<Utc>,
    /// 任务类型索引（task_type → case_bank 下标）
    task_type_index: HashMap<String, Vec<usize>>,
}

impl DualExperienceBank {
    /// 创建双层经验库
    ///
    /// - `distill_threshold`: 未蒸馏案例数达此值触发蒸馏
    pub fn new(distill_threshold: usize) -> Self {
        Self {
            case_bank: Vec::new(),
            global_bank: Vec::new(),
            distill_threshold: distill_threshold.max(1),
            last_distillation: Utc::now(),
            task_type_index: HashMap::new(),
        }
    }

    /// 案例数
    pub fn case_count(&self) -> usize {
        self.case_bank.len()
    }

    /// 全局经验数
    pub fn global_count(&self) -> usize {
        self.global_bank.len()
    }

    /// 未蒸馏案例数
    pub fn undistilled_count(&self) -> usize {
        self.case_bank.iter().filter(|c| !c.distilled).count()
    }

    /// 添加案例 — 索引更新 + 阈值触发蒸馏
    pub fn add_case(&mut self, case: CaseExperience) {
        let idx = self.case_bank.len();
        self.task_type_index
            .entry(case.task_type.clone())
            .or_default()
            .push(idx);
        self.case_bank.push(case);
        // 阈值触发蒸馏（变更驱动，非周期轮询）
        if self.undistilled_count() >= self.distill_threshold {
            self.distill_global();
        }
    }

    /// 全局蒸馏 — 按 task_type 分组提取成功/失败模式与有效策略
    ///
    /// 蒸馏后标记案例 `distilled=true`（幂等，不重复蒸馏）。
    pub fn distill_global(&mut self) {
        let undistilled: Vec<usize> = self
            .case_bank
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.distilled)
            .map(|(i, _)| i)
            .collect();
        if undistilled.is_empty() {
            return;
        }
        // 按 task_type 分组
        let mut by_task_type: HashMap<String, Vec<usize>> = HashMap::new();
        for &idx in &undistilled {
            let task_type = self.case_bank[idx].task_type.clone();
            by_task_type.entry(task_type).or_default().push(idx);
        }
        for (task_type, indices) in by_task_type {
            let cases: Vec<&CaseExperience> = indices.iter().map(|&i| &self.case_bank[i]).collect();
            let global = GlobalExperience {
                applicable_task_types: vec![task_type],
                success_patterns: self.extract_success_patterns(&cases),
                failure_patterns: self.extract_failure_patterns(&cases),
                effective_strategies: self.extract_strategies(&cases),
                distilled_at: Utc::now(),
                confidence: (cases.len() as f32 / self.distill_threshold as f32).min(1.0),
                source_case_count: cases.len(),
            };
            self.global_bank.push(global);
        }
        // 标记已蒸馏（幂等）
        for idx in undistilled {
            self.case_bank[idx].distilled = true;
        }
        self.last_distillation = Utc::now();
    }

    /// 提取成功模式 — score > 0.7 的 Success 案例按方法家族聚类
    fn extract_success_patterns(&self, cases: &[&CaseExperience]) -> Vec<SuccessPattern> {
        let success_cases: Vec<&CaseExperience> = cases
            .iter()
            .filter(|c| c.card.execution_status == ExecutionStatus::Success && c.card.score > 0.7)
            .copied()
            .collect();
        let mut by_method: HashMap<String, Vec<&CaseExperience>> = HashMap::new();
        for case in success_cases {
            by_method
                .entry(case.card.method_family.to_string())
                .or_default()
                .push(case);
        }
        by_method
            .into_iter()
            .map(|(method, group)| {
                let scores: Vec<f32> = group.iter().map(|c| c.card.score).collect();
                let avg_score = scores.iter().sum::<f32>() / scores.len() as f32;
                let min_score = scores.iter().copied().fold(f32::MAX, f32::min);
                let max_score = scores.iter().copied().fold(f32::MIN, f32::max);
                let avg_tokens = group
                    .iter()
                    .map(|c| c.card.metadata.token_usage.total_tokens)
                    .sum::<u64>() as u32
                    / group.len() as u32;
                SuccessPattern {
                    method_family: method,
                    score_range: (min_score, max_score),
                    key_factors: vec![format!("avg_score={avg_score:.2}")],
                    avg_token_usage: avg_tokens,
                }
            })
            .collect()
    }

    /// 提取失败模式 — 含错误签名的案例按 error_hash 聚类
    fn extract_failure_patterns(&self, cases: &[&CaseExperience]) -> Vec<FailurePattern> {
        let failure_cases: Vec<&CaseExperience> = cases
            .iter()
            .filter(|c| c.card.error_signature.is_some())
            .copied()
            .collect();
        let mut by_error: HashMap<String, Vec<&CaseExperience>> = HashMap::new();
        for case in failure_cases {
            if let Some(ref sig) = case.card.error_signature {
                by_error
                    .entry(sig.error_hash.to_string())
                    .or_default()
                    .push(case);
            }
        }
        by_error
            .into_iter()
            .map(|(hash, group)| {
                let first = group.first().expect("分组非空");
                let error_type = first
                    .card
                    .error_signature
                    .as_ref()
                    .map(|s| s.error_type.to_string())
                    .unwrap_or_default();
                let avg_time = group
                    .iter()
                    .map(|c| c.card.metadata.execution_time_ms)
                    .sum::<u64>()
                    / group.len() as u64;
                FailurePattern {
                    error_signature: hash,
                    error_type,
                    fix_strategy: "Apply known fix from similar cases".to_string(),
                    frequency: group.len() as u32,
                    avg_fix_time_ms: avg_time,
                }
            })
            .collect()
    }

    /// 提取有效策略 — 成功案例按算子维度统计平均改进
    fn extract_strategies(&self, cases: &[&CaseExperience]) -> Vec<StrategyRecord> {
        let mut by_operator: HashMap<String, Vec<f32>> = HashMap::new();
        for case in cases
            .iter()
            .filter(|c| c.card.execution_status == ExecutionStatus::Success)
        {
            by_operator
                .entry(format!("{:?}", case.card.operator))
                .or_default()
                .push(case.card.score);
        }
        by_operator
            .into_iter()
            .map(|(op, scores)| {
                let avg = scores.iter().sum::<f32>() / scores.len() as f32;
                StrategyRecord {
                    dimension: "operator".to_string(),
                    strategy_value: op,
                    avg_improvement: avg,
                    sample_count: scores.len() as u32,
                }
            })
            .collect()
    }

    /// 检索 — 全局经验（task_type 匹配）+ 相似案例（分数过滤）
    pub fn retrieve(&self, query: &TaskQuery) -> RetrievedExperiences<'_> {
        let global = self
            .global_bank
            .iter()
            .filter(|g| g.applicable_task_types.contains(&query.task_type))
            .collect();
        let cases = self.retrieve_similar_cases(query);
        RetrievedExperiences { global, cases }
    }

    /// 检索相似案例 — task_type 索引 + min_score 过滤 + max_results 截断
    fn retrieve_similar_cases(&self, query: &TaskQuery) -> Vec<&CaseExperience> {
        if let Some(indices) = self.task_type_index.get(&query.task_type) {
            indices
                .iter()
                .filter_map(|&i| self.case_bank.get(i))
                .filter(|c| c.card.score >= query.min_score)
                .take(query.max_results)
                .collect()
        } else {
            Vec::new()
        }
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::experience_card::{
        AtomicOperator, CardMetadata, ErrorSignature, ThreeFactorScore,
    };

    fn card(score: f32, status: ExecutionStatus) -> ExperienceCard {
        ExperienceCard {
            card_id: Box::from("card-1"),
            task_id: Box::from("t1"),
            node_id: Box::from("n1"),
            parent_id: None,
            created_at: Utc::now(),
            operator: AtomicOperator::Draft,
            score,
            delta_vs_parent: 0.0,
            method_family: Box::from("draft_pipeline"),
            error_signature: None,
            three_factor: ThreeFactorScore {
                quality: score,
                progress: 0.1,
                novelty: 0.5,
            },
            execution_status: status,
            token_evidence_ids: Vec::new(),
            segment_id: None,
            metadata: CardMetadata::default(),
        }
    }

    fn case(score: f32, status: ExecutionStatus, task_type: &str) -> CaseExperience {
        CaseExperience {
            card: card(score, status),
            task_type: task_type.to_string(),
            distilled: false,
            inserted_at: Utc::now(),
        }
    }

    #[test]
    fn add_case_triggers_distillation_at_threshold() {
        let mut bank = DualExperienceBank::new(3);
        bank.add_case(case(0.9, ExecutionStatus::Success, "code_gen"));
        bank.add_case(case(0.8, ExecutionStatus::Success, "code_gen"));
        assert_eq!(bank.global_count(), 0, "未达阈值不蒸馏");
        bank.add_case(case(0.85, ExecutionStatus::Success, "code_gen"));
        assert_eq!(bank.global_count(), 1, "达阈值触发蒸馏");
        assert_eq!(bank.undistilled_count(), 0, "蒸馏后无未蒸馏案例");
    }

    #[test]
    fn distillation_is_idempotent() {
        let mut bank = DualExperienceBank::new(2);
        bank.add_case(case(0.9, ExecutionStatus::Success, "code_gen"));
        bank.add_case(case(0.8, ExecutionStatus::Success, "code_gen"));
        let first = bank.global_count();
        bank.distill_global(); // 重复蒸馏（无未蒸馏案例）
        assert_eq!(bank.global_count(), first, "重复蒸馏不新增全局经验");
    }

    #[test]
    fn success_patterns_extract_high_score_methods() {
        let mut bank = DualExperienceBank::new(2);
        bank.add_case(case(0.9, ExecutionStatus::Success, "code_gen"));
        bank.add_case(case(0.8, ExecutionStatus::Success, "code_gen"));
        let retrieved = bank.retrieve(&TaskQuery {
            task_type: "code_gen".into(),
            min_score: 0.0,
            max_results: 10,
        });
        assert_eq!(retrieved.global.len(), 1);
        let global = retrieved.global[0];
        assert!(
            !global.success_patterns.is_empty(),
            "高分方法应提取成功模式"
        );
        let pattern = &global.success_patterns[0];
        assert_eq!(pattern.method_family, "draft_pipeline");
        assert!(pattern.score_range.0 >= 0.8);
    }

    #[test]
    fn failure_patterns_cluster_by_error_hash() {
        let mut bank = DualExperienceBank::new(2);
        let mut c1 = case(0.3, ExecutionStatus::Error, "code_gen");
        c1.card.error_signature = Some(ErrorSignature {
            error_type: Box::from("compile_error"),
            error_location: Box::from("src/a.rs"),
            error_summary: Box::from("E0308"),
            error_hash: Box::from("hash-1"),
        });
        let mut c2 = case(0.2, ExecutionStatus::Error, "code_gen");
        c2.card.error_signature = Some(ErrorSignature {
            error_type: Box::from("compile_error"),
            error_location: Box::from("src/b.rs"),
            error_summary: Box::from("E0308"),
            error_hash: Box::from("hash-1"), // 同哈希聚类
        });
        bank.add_case(c1);
        bank.add_case(c2);
        let retrieved = bank.retrieve(&TaskQuery {
            task_type: "code_gen".into(),
            min_score: 0.0,
            max_results: 10,
        });
        let global = retrieved.global[0];
        assert_eq!(global.failure_patterns.len(), 1, "同哈希应聚类为一个模式");
        assert_eq!(global.failure_patterns[0].frequency, 2);
    }

    #[test]
    fn strategy_records_by_operator() {
        let mut bank = DualExperienceBank::new(2);
        bank.add_case(case(0.9, ExecutionStatus::Success, "code_gen"));
        bank.add_case(case(0.8, ExecutionStatus::Success, "code_gen"));
        let retrieved = bank.retrieve(&TaskQuery {
            task_type: "code_gen".into(),
            min_score: 0.0,
            max_results: 10,
        });
        let global = retrieved.global[0];
        assert!(
            !global.effective_strategies.is_empty(),
            "成功案例应产生策略记录"
        );
        assert_eq!(global.effective_strategies[0].dimension, "operator");
    }

    #[test]
    fn retrieve_filters_by_task_type_and_score() {
        let mut bank = DualExperienceBank::new(100);
        bank.add_case(case(0.9, ExecutionStatus::Success, "code_gen"));
        bank.add_case(case(0.3, ExecutionStatus::Success, "code_gen"));
        bank.add_case(case(0.8, ExecutionStatus::Success, "refactor"));
        // task_type 过滤
        let result = bank.retrieve(&TaskQuery {
            task_type: "code_gen".into(),
            min_score: 0.5,
            max_results: 10,
        });
        assert_eq!(result.cases.len(), 1, "min_score=0.5 应只返回 0.9 案例");
        // 无匹配 task_type
        let empty = bank.retrieve(&TaskQuery {
            task_type: "nonexistent".into(),
            min_score: 0.0,
            max_results: 10,
        });
        assert!(empty.cases.is_empty());
        assert!(empty.global.is_empty());
    }

    #[test]
    fn retrieve_respects_max_results() {
        let mut bank = DualExperienceBank::new(100);
        for _ in 0..5 {
            bank.add_case(case(0.9, ExecutionStatus::Success, "code_gen"));
        }
        let result = bank.retrieve(&TaskQuery {
            task_type: "code_gen".into(),
            min_score: 0.0,
            max_results: 2,
        });
        assert_eq!(result.cases.len(), 2, "应受 max_results 截断");
    }

    #[test]
    fn confidence_bounded_by_threshold() {
        let mut bank = DualExperienceBank::new(2);
        bank.add_case(case(0.9, ExecutionStatus::Success, "code_gen"));
        bank.add_case(case(0.8, ExecutionStatus::Success, "code_gen"));
        let retrieved = bank.retrieve(&TaskQuery {
            task_type: "code_gen".into(),
            min_score: 0.0,
            max_results: 10,
        });
        // 2 案例 / 阈值 2 = 1.0
        assert!((retrieved.global[0].confidence - 1.0).abs() < 1e-6);
        assert_eq!(retrieved.global[0].source_case_count, 2);
    }
}
