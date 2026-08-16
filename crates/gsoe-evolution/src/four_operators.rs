//! 四套原子算子 — OpenMLE 算子化抽象（设计文档 §10.1）
//!
//! 对应架构层: **L5 Knowledge**（gsoe-evolution 子模块）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §10.1
//! 对应论文: 清华 OpenMLE（Draft/Improve/Debug/Crossover 贯穿 SFT/RL/推理全生命周期）
//! 对应 ADR: ADR-049 决策 1（four-operators 落点 gsoe-evolution，内嵌模块）
//!
//! # 核心职责
//!
//! 四套原子算子贯穿代码生成/修改/修复/融合全生命周期：
//! - **Draft**: 从零起草全新代码
//! - **Improve**: 在父卡片代码上迭代改进
//! - **Debug**: 基于错误签名查询历史修复方案
//! - **Crossover**: 融合多个高新颖度候选
//!
//! # 设计约束（铁律）
//!
//! - **D-3 依赖倒置**: 算子通过 [`CardQuery`] trait 访问历史卡片，不直接依赖
//!   L3 `ExperienceCardStorage`（避免 gsoe 引入 cmt-tiering 依赖）；调用方注入实现
//! - **铁律3**: 经验卡片只读消费（算子读取 parent_card，不修改）
//! - **铁律7**: 错误签名结构化（Debug 算子消费 ErrorSignature.error_hash 查询）

use std::sync::Arc;

use async_trait::async_trait;
use nexus_contracts::experience_card::{ErrorSignature, ExecutionStatus};
use nexus_contracts::{AtomicOperator, CardMetadata, ExperienceCard};

// ============================================================
// CardQuery trait（D-3 依赖倒置）
// ============================================================

/// 卡片查询抽象 — 算子访问历史经验卡片的依赖倒置接口
///
/// 签名对齐 L3 `ExperienceCardStorage.query_by_three_factor / query_by_error_signature`，
/// 由调用方注入实现（适配 L3 存储或内存缓存）。返回 `Vec<ExperienceCard>`，
/// 适配层内部处理错误（失败返回空 vec，算子逻辑保持简洁）。
#[async_trait]
pub trait CardQuery: Send + Sync {
    /// 按错误签名哈希查询历史修复卡片
    async fn query_by_error_signature(&self, error_hash: &str, limit: usize)
        -> Vec<ExperienceCard>;

    /// 按三因子查询高质量候选卡片
    async fn query_by_three_factor(
        &self,
        task_id: &str,
        min_quality: f32,
        k: usize,
    ) -> Vec<ExperienceCard>;
}

// ============================================================
// 算子上下文 / 结果 / 成本 / 错误
// ============================================================

/// 算子执行上下文 — 算子执行所需的输入
pub struct OperatorContext {
    /// 任务 ID
    pub task_id: String,
    /// 任务类型
    pub task_type: String,
    /// 父卡片（Improve/Debug 需要）
    pub parent_card: Option<ExperienceCard>,
    /// 错误签名（Debug 需要）
    pub error_signature: Option<ErrorSignature>,
    /// 需求描述（Draft 用）
    pub requirements: String,
    /// 现有代码（Improve/Debug 的基底代码）
    pub code: Option<String>,
    /// 卡片查询注入（D-3 依赖倒置，None = 无历史可查）
    pub card_query: Option<Arc<dyn CardQuery>>,
}

/// 算子执行结果
pub struct OperatorResult {
    /// 生成的代码
    pub code: String,
    /// 预估评分（0.0-1.0）
    pub score: f32,
    /// 执行的算子类型
    pub operator: AtomicOperator,
    /// 执行状态
    pub execution_status: ExecutionStatus,
    /// 错误签名（执行失败时 Some）
    pub error_signature: Option<ErrorSignature>,
    /// 执行元数据
    pub metadata: CardMetadata,
}

/// 算子资源成本估算
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceCost {
    /// 预估 token 消耗
    pub estimated_tokens: usize,
    /// 预估耗时（毫秒）
    pub estimated_time_ms: u64,
}

/// 算子执行错误
#[derive(Debug, thiserror::Error)]
pub enum OperatorError {
    /// 缺少父卡片（Improve/Debug 需要）
    #[error("No parent card")]
    NoParent,
    /// 缺少错误签名（Debug 需要）
    #[error("No error signature")]
    NoErrorSignature,
    /// 候选不足（Crossover 需要 ≥2）
    #[error("Insufficient candidates")]
    InsufficientCandidates,
    /// 执行失败
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
}

// ============================================================
// 原子算子 trait
// ============================================================

/// 原子算子 trait — 四套算子的统一抽象
#[async_trait]
pub trait AtomicOperatorTrait: Send + Sync {
    /// 算子类型
    fn operator_type(&self) -> AtomicOperator;

    /// 执行算子，生成代码与评分
    async fn execute(&self, context: &OperatorContext) -> Result<OperatorResult, OperatorError>;

    /// 估算资源成本
    fn estimate_cost(&self, context: &OperatorContext) -> ResourceCost;

    /// 判定算子是否适用于当前上下文
    fn is_applicable(&self, context: &OperatorContext) -> bool;
}

/// 构造默认执行元数据（算子规则实现的估算值）
fn default_metadata(
    execution_time_ms: u64,
    prompt_tokens: u64,
    completion_tokens: u64,
    lines: i32,
) -> CardMetadata {
    CardMetadata {
        execution_time_ms,
        token_usage: nexus_contracts::experience_card::TokenUsage::new(
            prompt_tokens,
            completion_tokens,
        ),
        lines_changed: lines,
        ..Default::default()
    }
}

// ============================================================
// DraftOperator — 从零起草
// ============================================================

/// Draft 算子 — 从零起草全新代码
pub struct DraftOperator;

#[async_trait]
impl AtomicOperatorTrait for DraftOperator {
    fn operator_type(&self) -> AtomicOperator {
        AtomicOperator::Draft
    }

    async fn execute(&self, context: &OperatorContext) -> Result<OperatorResult, OperatorError> {
        // 规则实现：基于需求生成代码骨架（无 LLM，R2 冻结 ADR-042）
        let code = format!("// Draft for: {}\nfn main() {{}}", context.requirements);
        Ok(OperatorResult {
            code,
            score: 0.5,
            operator: AtomicOperator::Draft,
            execution_status: ExecutionStatus::Success,
            error_signature: None,
            metadata: default_metadata(30_000, 2000, 3000, 10),
        })
    }

    fn estimate_cost(&self, _context: &OperatorContext) -> ResourceCost {
        ResourceCost {
            estimated_tokens: 5000,
            estimated_time_ms: 30_000,
        }
    }

    fn is_applicable(&self, _context: &OperatorContext) -> bool {
        true
    }
}

// ============================================================
// ImproveOperator — 迭代改进
// ============================================================

/// Improve 算子 — 在父卡片代码上迭代改进
pub struct ImproveOperator;

#[async_trait]
impl AtomicOperatorTrait for ImproveOperator {
    fn operator_type(&self) -> AtomicOperator {
        AtomicOperator::Improve
    }

    async fn execute(&self, context: &OperatorContext) -> Result<OperatorResult, OperatorError> {
        let parent = context
            .parent_card
            .as_ref()
            .ok_or(OperatorError::NoParent)?;
        let base_code = context
            .code
            .clone()
            .unwrap_or_else(|| format!("// parent card: {}", parent.card_id));
        let improved_code = format!("{base_code}\n// Improved");
        Ok(OperatorResult {
            code: improved_code,
            score: (parent.score + 0.1).min(1.0),
            operator: AtomicOperator::Improve,
            execution_status: ExecutionStatus::Success,
            error_signature: None,
            metadata: default_metadata(20_000, 1500, 1500, 5),
        })
    }

    fn estimate_cost(&self, _context: &OperatorContext) -> ResourceCost {
        ResourceCost {
            estimated_tokens: 3000,
            estimated_time_ms: 20_000,
        }
    }

    fn is_applicable(&self, context: &OperatorContext) -> bool {
        context.parent_card.is_some()
    }
}

// ============================================================
// DebugOperator — 错误修复
// ============================================================

/// Debug 算子 — 基于错误签名查询历史修复方案
pub struct DebugOperator;

#[async_trait]
impl AtomicOperatorTrait for DebugOperator {
    fn operator_type(&self) -> AtomicOperator {
        AtomicOperator::Debug
    }

    async fn execute(&self, context: &OperatorContext) -> Result<OperatorResult, OperatorError> {
        let parent = context
            .parent_card
            .as_ref()
            .ok_or(OperatorError::NoParent)?;
        let error = context
            .error_signature
            .as_ref()
            .ok_or(OperatorError::NoErrorSignature)?;
        // D-3: 通过 CardQuery 查询历史修复方案（依赖倒置）
        let similar_fixes = if let Some(ref query) = context.card_query {
            query.query_by_error_signature(&error.error_hash, 5).await
        } else {
            Vec::new()
        };
        // 选择评分最高的成功修复方案
        let best_fix = similar_fixes
            .iter()
            .filter(|c| c.execution_status == ExecutionStatus::Success)
            .max_by(|a, b| {
                a.score
                    .partial_cmp(&b.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        let base_code = context
            .code
            .clone()
            .unwrap_or_else(|| format!("// parent card: {}", parent.card_id));
        let fixed_code = if let Some(fix) = best_fix {
            format!(
                "{base_code}\n// Fixed using {} (score: {:.2})",
                fix.card_id, fix.score
            )
        } else {
            format!("{base_code}\n// Generic fix for: {}", error.error_type)
        };
        Ok(OperatorResult {
            code: fixed_code,
            score: parent.score,
            operator: AtomicOperator::Debug,
            execution_status: ExecutionStatus::Success,
            error_signature: None,
            metadata: default_metadata(15_000, 1000, 1000, 2),
        })
    }

    fn estimate_cost(&self, _context: &OperatorContext) -> ResourceCost {
        ResourceCost {
            estimated_tokens: 2000,
            estimated_time_ms: 15_000,
        }
    }

    fn is_applicable(&self, context: &OperatorContext) -> bool {
        context.parent_card.is_some() && context.error_signature.is_some()
    }
}

// ============================================================
// CrossoverOperator — 代码融合
// ============================================================

/// Crossover 算子 — 融合多个高新颖度候选
pub struct CrossoverOperator;

#[async_trait]
impl AtomicOperatorTrait for CrossoverOperator {
    fn operator_type(&self) -> AtomicOperator {
        AtomicOperator::Crossover
    }

    async fn execute(&self, context: &OperatorContext) -> Result<OperatorResult, OperatorError> {
        // D-3: 通过 CardQuery 查询三因子高质量候选（依赖倒置）
        let candidates = if let Some(ref query) = context.card_query {
            query.query_by_three_factor(&context.task_id, 0.7, 10).await
        } else {
            Vec::new()
        };
        if candidates.len() < 2 {
            return Err(OperatorError::InsufficientCandidates);
        }
        // 按新颖度降序（红线 R8: 候选排序用 partial_cmp，规模小可接受）
        let mut sorted = candidates;
        sorted.sort_by(|a, b| {
            b.three_factor
                .novelty
                .partial_cmp(&a.three_factor.novelty)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let merged_code = format!(
            "// Crossover of {} and {}\n// merged",
            sorted[0].card_id, sorted[1].card_id
        );
        let score = (sorted[0].score + sorted[1].score) / 2.0;
        Ok(OperatorResult {
            code: merged_code,
            score,
            operator: AtomicOperator::Crossover,
            execution_status: ExecutionStatus::Success,
            error_signature: None,
            metadata: default_metadata(25_000, 2000, 2000, 8),
        })
    }

    fn estimate_cost(&self, _context: &OperatorContext) -> ResourceCost {
        ResourceCost {
            estimated_tokens: 4000,
            estimated_time_ms: 25_000,
        }
    }

    fn is_applicable(&self, _context: &OperatorContext) -> bool {
        true
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use nexus_contracts::experience_card::ThreeFactorScore;

    fn parent_card(score: f32) -> ExperienceCard {
        ExperienceCard {
            card_id: "parent-1".into(),
            task_id: "task-1".into(),
            node_id: "node-1".into(),
            parent_id: None,
            created_at: Utc::now(),
            operator: AtomicOperator::Draft,
            score,
            delta_vs_parent: 0.0,
            method_family: "test".into(),
            error_signature: None,
            three_factor: ThreeFactorScore {
                quality: score,
                progress: 0.1,
                novelty: 0.5,
            },
            execution_status: ExecutionStatus::Success,
            token_evidence_ids: Vec::new(),
            segment_id: None,
            metadata: CardMetadata::default(),
        }
    }

    fn base_context() -> OperatorContext {
        OperatorContext {
            task_id: "task-1".to_string(),
            task_type: "code_gen".to_string(),
            parent_card: None,
            error_signature: None,
            requirements: "build a parser".to_string(),
            code: None,
            card_query: None,
        }
    }

    #[tokio::test]
    async fn draft_operator_executes_without_parent() {
        let op = DraftOperator;
        let ctx = base_context();
        assert!(op.is_applicable(&ctx));
        let result = op.execute(&ctx).await.expect("Draft 执行成功");
        assert_eq!(result.operator, AtomicOperator::Draft);
        assert_eq!(result.execution_status, ExecutionStatus::Success);
        assert!(result.code.contains("build a parser"));
        assert_eq!(op.operator_type(), AtomicOperator::Draft);
    }

    #[tokio::test]
    async fn improve_operator_requires_parent() {
        let op = ImproveOperator;
        let mut ctx = base_context();
        // 无父卡片 → 不适用
        assert!(!op.is_applicable(&ctx));
        assert!(matches!(
            op.execute(&ctx).await,
            Err(OperatorError::NoParent)
        ));
        // 有父卡片 → 可执行
        ctx.parent_card = Some(parent_card(0.6));
        assert!(op.is_applicable(&ctx));
        let result = op.execute(&ctx).await.expect("Improve 执行成功");
        assert_eq!(result.operator, AtomicOperator::Improve);
        assert!((result.score - 0.7).abs() < 1e-6, "0.6+0.1=0.7");
    }

    #[tokio::test]
    async fn debug_operator_requires_error_signature() {
        let op = DebugOperator;
        let mut ctx = base_context();
        ctx.parent_card = Some(parent_card(0.5));
        // 有父卡片但无错误签名 → 不适用
        assert!(!op.is_applicable(&ctx));
        assert!(matches!(
            op.execute(&ctx).await,
            Err(OperatorError::NoErrorSignature)
        ));
        // 补充错误签名 → 可执行
        ctx.error_signature = Some(ErrorSignature {
            error_type: "compile_error".into(),
            error_location: "src/x.rs".into(),
            error_summary: "E0308".into(),
            error_hash: "hash-x".into(),
        });
        assert!(op.is_applicable(&ctx));
        let result = op.execute(&ctx).await.expect("Debug 执行成功");
        assert_eq!(result.operator, AtomicOperator::Debug);
    }

    #[tokio::test]
    async fn crossover_operator_insufficient_candidates() {
        let op = CrossoverOperator;
        let ctx = base_context();
        // 无 card_query → 候选为空 → InsufficientCandidates
        assert!(matches!(
            op.execute(&ctx).await,
            Err(OperatorError::InsufficientCandidates)
        ));
    }

    /// Mock CardQuery — 返回固定候选（测试依赖倒置注入）
    struct MockCardQuery {
        cards: Vec<ExperienceCard>,
    }

    #[async_trait]
    impl CardQuery for MockCardQuery {
        async fn query_by_error_signature(
            &self,
            _error_hash: &str,
            limit: usize,
        ) -> Vec<ExperienceCard> {
            self.cards.iter().take(limit).cloned().collect()
        }

        async fn query_by_three_factor(
            &self,
            _task_id: &str,
            _min_quality: f32,
            k: usize,
        ) -> Vec<ExperienceCard> {
            self.cards.iter().take(k).cloned().collect()
        }
    }

    #[tokio::test]
    async fn crossover_with_injected_candidates() {
        let c1 = parent_card(0.8);
        let mut c2 = parent_card(0.6);
        c2.card_id = "parent-2".into();
        let mut ctx = base_context();
        ctx.card_query = Some(Arc::new(MockCardQuery {
            cards: vec![c1, c2],
        }));
        let op = CrossoverOperator;
        let result = op.execute(&ctx).await.expect("Crossover 执行成功");
        assert_eq!(result.operator, AtomicOperator::Crossover);
        // 平均分 (0.8+0.6)/2 = 0.7
        assert!((result.score - 0.7).abs() < 1e-6);
    }

    #[tokio::test]
    async fn debug_with_injected_fix_history() {
        let mut fix = parent_card(0.9);
        fix.execution_status = ExecutionStatus::Success;
        let mut ctx = base_context();
        ctx.parent_card = Some(parent_card(0.4));
        ctx.error_signature = Some(ErrorSignature {
            error_type: "compile_error".into(),
            error_location: "src/x.rs".into(),
            error_summary: "E0308".into(),
            error_hash: "hash-x".into(),
        });
        ctx.card_query = Some(Arc::new(MockCardQuery { cards: vec![fix] }));
        let op = DebugOperator;
        let result = op.execute(&ctx).await.expect("Debug 执行成功");
        // 使用注入的修复方案（parent-1 score 0.9）
        assert!(result.code.contains("Fixed using parent-1"));
    }

    #[test]
    fn estimate_cost_all_operators() {
        let ctx = base_context();
        assert_eq!(DraftOperator.estimate_cost(&ctx).estimated_tokens, 5000);
        assert_eq!(ImproveOperator.estimate_cost(&ctx).estimated_tokens, 3000);
        assert_eq!(DebugOperator.estimate_cost(&ctx).estimated_tokens, 2000);
        assert_eq!(CrossoverOperator.estimate_cost(&ctx).estimated_tokens, 4000);
    }
}
