//! 经验卡片生成器 — PVL 验证结果 → 经验卡片转换（设计文档 §12.1）
//!
//! 对应架构层: **L7 Execution**（pvl-layer 子模块，ADR-049 决策 1 内嵌）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §12.1
//! 对应论文: 清华 OpenMLE（经验卡片体系，执行节点结构化经验）
//!
//! # 核心职责
//!
//! 将 PVL 生产验证闭环的执行元数据与验证结果转换为 L0 [`ExperienceCard`] 契约：
//! - **六类状态单一来源**（铁律8）：`execution_status` 委托
//!   `seccore::ExecutionFeedbackIntegrator::classify`，不手写分类
//! - **错误签名哈希单一来源**（铁律7）：`error_hash` 经
//!   `seccore::compute_error_hash`（SHA-256 前 16 位，与 L4/L3 索引对齐）
//! - **三因子纯函数**（铁律4）：quality + progress + novelty 计算无副作用
//! - **卡片不可变**（铁律3）：生成后无 setter，版本更新仅走
//!   `ExperienceCard::updated_assessment`
//! - **可选投递**（D-5）：注入 [`ExperienceCardBus`] 后生成即投递
//!   （双通道分级由 bus 内部处理），默认 None 保持既有行为
//!
//! # 规范偏差适配（照抄必败清单）
//!
//! 1. 规范原型 `ValidationResult` 与既有 `types::VerificationResult` 命名冲突
//!    → 本模块命名 [`CardValidationInput`]
//! 2. 规范原型 `ErrorSignature::from_output` 不存在 → 直接字段构造 +
//!    compute_error_hash
//! 3. L0 字符串字段全部 `Box<str>` → `.into()` 适配

use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
use event_bus::ExperienceCardBus;
use nexus_contracts::experience_card::{
    AtomicOperator, CardMetadata, EnvironmentInfo, ErrorSignature, ThreeFactorScore, TokenUsage,
};
use nexus_contracts::ExperienceCard;
use seccore::{compute_error_hash, ExecutionFeedbackIntegrator};

/// 卡片生成执行元数据 — 任务/算子/技能上下文（规范 §12.1 ExecutionMetadata）
#[derive(Clone, Debug)]
pub struct ExecutionMetadata {
    /// 所属任务 ID
    pub task_id: String,
    /// 父节点卡片 ID（None = 根节点）
    pub parent_id: Option<String>,
    /// 执行的原子算子
    pub operator: AtomicOperator,
    /// 使用的技能 ID 列表
    pub skills_used: Vec<String>,
}

/// 卡片生成验证输入 — PVL 验证结果投影（规范 §12.1 ValidationResult）
///
/// WHY 命名 CardValidationInput: 既有 `types::VerificationResult` 已被
/// Producer/Verifier 流水线占用，命名冲突协调（模块文档落偏差记录）。
#[derive(Clone, Debug)]
pub struct CardValidationInput {
    /// 验证是否通过
    pub success: bool,
    /// 绝对评分（0.0-1.0）
    pub score: f32,
    /// 错误类型（失败时，如 "compile_error"）
    pub error_type: Option<String>,
    /// 错误位置（失败时，如 "src/foo.rs:42"）
    pub error_location: Option<String>,
    /// 错误信息（失败时，人类可读概述）
    pub error_message: Option<String>,
    /// 是否超时（六类状态分类输入）
    pub timed_out: bool,
    /// 执行耗时（毫秒）
    pub execution_time_ms: u64,
    /// 输入 Token 数
    pub prompt_tokens: u64,
    /// 输出 Token 数
    pub completion_tokens: u64,
    /// 变更行数（负值表示净删除）
    pub lines_changed: i32,
}

/// 经验卡片生成器 — PVL 验证结果 → ExperienceCard 转换
///
/// `card_counter` 保证 card_id/node_id 单调唯一；`card_bus` 可选注入
/// （D-5，默认 None 保持既有行为，注入后生成即投递）。
pub struct ExperienceCardGenerator {
    /// 卡片计数器（card_id/node_id 序号来源）
    card_counter: AtomicU64,
    /// Chimera 版本（环境信息快照）
    chimera_version: Box<str>,
    /// 经验卡片总线（可选注入，D-5）
    card_bus: Option<ExperienceCardBus>,
}

impl ExperienceCardGenerator {
    /// 创建生成器
    pub fn new(chimera_version: &str) -> Self {
        Self {
            card_counter: AtomicU64::new(0),
            chimera_version: Box::from(chimera_version),
            card_bus: None,
        }
    }

    /// 注入经验卡片总线（D-5：注入后 generate_and_publish 自动投递）
    pub fn with_card_bus(mut self, bus: ExperienceCardBus) -> Self {
        self.card_bus = Some(bus);
        self
    }

    /// 生成经验卡片 — 纯组装（铁律4，同输入同输出除 ID/时间戳外）
    ///
    /// - 六类状态经 L4 classify（铁律8 单一来源）
    /// - 错误签名经 L4 compute_error_hash（铁律7 哈希聚类）
    /// - 卡片生成后不可变（铁律3）
    pub fn generate(
        &self,
        metadata: &ExecutionMetadata,
        validation: &CardValidationInput,
    ) -> ExperienceCard {
        let seq = self.card_counter.fetch_add(1, Ordering::SeqCst);
        let card_id = format!("card_{seq:010}");
        let node_id = format!("node_{seq:010}");
        let three_factor = compute_three_factor(validation.score, metadata.operator, validation);
        let error_signature = build_error_signature(validation);
        // 铁律8: 六类状态分类单一来源（L4 ExecutionFeedbackIntegrator）
        let execution_status = ExecutionFeedbackIntegrator::classify(
            validation.success,
            true, // has_output: PVL 验证恒产出验证报告
            true, // has_submission: PVL 验证对象恒为已提交操作
            Some(validation.score),
            validation.timed_out,
            validation.error_message.as_deref(),
        );
        ExperienceCard {
            card_id: card_id.into(),
            task_id: metadata.task_id.as_str().into(),
            node_id: node_id.into(),
            parent_id: metadata.parent_id.as_deref().map(Box::from),
            created_at: Utc::now(),
            operator: metadata.operator,
            score: validation.score,
            delta_vs_parent: three_factor.progress,
            method_family: Box::from(infer_method_family(metadata.operator)),
            error_signature,
            three_factor,
            execution_status,
            token_evidence_ids: Vec::new(),
            segment_id: None,
            metadata: CardMetadata {
                execution_time_ms: validation.execution_time_ms,
                token_usage: TokenUsage::new(
                    validation.prompt_tokens,
                    validation.completion_tokens,
                ),
                lines_changed: validation.lines_changed,
                skills_used: metadata
                    .skills_used
                    .iter()
                    .map(|s| Box::from(s.as_str()))
                    .collect(),
                environment: EnvironmentInfo {
                    rust_version: Box::from(env!("CARGO_PKG_VERSION")),
                    os: Box::from(std::env::consts::OS),
                    cpu_arch: Box::from(std::env::consts::ARCH),
                    chimera_version: self.chimera_version.clone(),
                },
            },
        }
    }

    /// 生成并投递 — 注入 card_bus 时经双通道分级投递（D-5）
    ///
    /// 未注入 bus 时等价于 `generate`（返回值不丢失）。
    pub fn generate_and_publish(
        &self,
        metadata: &ExecutionMetadata,
        validation: &CardValidationInput,
    ) -> ExperienceCard {
        let card = self.generate(metadata, validation);
        if let Some(bus) = &self.card_bus {
            bus.publish(card.clone());
        }
        card
    }

    /// 已生成卡片数只读访问（可观测性）
    pub fn generated_count(&self) -> u64 {
        self.card_counter.load(Ordering::SeqCst)
    }
}

/// 三因子评分纯函数（铁律4，规范 §12.1 compute_three_factor）
///
/// - quality: 绝对评分钳制 [0,1]
/// - progress: 父节点差值由消费方回填（生成时 0.0）
/// - novelty: 算子基础新颖性 + Token 效率加成（上限 1.0）
fn compute_three_factor(
    score: f32,
    operator: AtomicOperator,
    validation: &CardValidationInput,
) -> ThreeFactorScore {
    let quality = score.clamp(0.0, 1.0);
    let progress = 0.0;
    // 算子基础新颖性表（规范 §12.1）
    let base_novelty = match operator {
        AtomicOperator::Draft => 0.3,
        AtomicOperator::Improve => 0.5,
        AtomicOperator::Debug => 0.2,
        AtomicOperator::Crossover => 0.8,
    };
    // Token 效率加成: 相对 5000 token 基线的节省比例（最多 +0.2）
    let total_tokens = validation.prompt_tokens + validation.completion_tokens;
    let token_efficiency = if total_tokens > 0 {
        let baseline = 5000.0f32;
        (baseline / total_tokens as f32).min(1.0) * 0.2
    } else {
        0.0
    };
    let novelty = (base_novelty + token_efficiency).min(1.0);
    ThreeFactorScore {
        quality,
        progress,
        novelty,
    }
}

/// 错误签名构造（铁律7，失败且错误字段齐备时 Some）
fn build_error_signature(validation: &CardValidationInput) -> Option<ErrorSignature> {
    if validation.success {
        return None;
    }
    let error_type = validation.error_type.as_deref()?;
    let summary = validation.error_message.as_deref()?;
    Some(ErrorSignature {
        error_type: Box::from(error_type),
        error_location: Box::from(validation.error_location.as_deref().unwrap_or("unknown")),
        error_summary: Box::from(summary),
        // 铁律7: 哈希单一来源（L4 compute_error_hash，SHA-256 前 16 位）
        error_hash: compute_error_hash(error_type, summary).into(),
    })
}

/// 方法家族推断（规范 §12.1 infer_method_family）
fn infer_method_family(operator: AtomicOperator) -> &'static str {
    match operator {
        AtomicOperator::Draft => "draft_pipeline",
        AtomicOperator::Improve => "iterative_improvement",
        AtomicOperator::Debug => "error_fix",
        AtomicOperator::Crossover => "code_merge",
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::experience_card::ExecutionStatus;

    fn metadata(operator: AtomicOperator) -> ExecutionMetadata {
        ExecutionMetadata {
            task_id: "task-1".to_string(),
            parent_id: None,
            operator,
            skills_used: vec!["skill-a".to_string()],
        }
    }

    fn success_validation(score: f32) -> CardValidationInput {
        CardValidationInput {
            success: true,
            score,
            error_type: None,
            error_location: None,
            error_message: None,
            timed_out: false,
            execution_time_ms: 100,
            prompt_tokens: 1000,
            completion_tokens: 2000,
            lines_changed: 10,
        }
    }

    fn failure_validation() -> CardValidationInput {
        CardValidationInput {
            success: false,
            score: 0.2,
            error_type: Some("compile_error".to_string()),
            error_location: Some("src/foo.rs:42".to_string()),
            error_message: Some("mismatched types".to_string()),
            timed_out: false,
            execution_time_ms: 50,
            prompt_tokens: 500,
            completion_tokens: 100,
            lines_changed: 0,
        }
    }

    #[test]
    fn generate_success_card() {
        let gen = ExperienceCardGenerator::new("2.26.0-omega");
        let card = gen.generate(&metadata(AtomicOperator::Draft), &success_validation(0.9));
        assert_eq!(card.card_id.as_ref(), "card_0000000000");
        assert_eq!(card.node_id.as_ref(), "node_0000000000");
        assert_eq!(card.task_id.as_ref(), "task-1");
        assert_eq!(card.execution_status, ExecutionStatus::Success);
        assert!(card.error_signature.is_none());
        assert_eq!(card.method_family.as_ref(), "draft_pipeline");
        assert_eq!(gen.generated_count(), 1);
    }

    #[test]
    fn generate_failure_card_with_error_signature() {
        let gen = ExperienceCardGenerator::new("2.26.0-omega");
        let card = gen.generate(&metadata(AtomicOperator::Debug), &failure_validation());
        // 铁律8: 失败 + 错误输出 → Error 状态（classify 单一来源）
        assert_eq!(card.execution_status, ExecutionStatus::Error);
        // 铁律7: 错误签名 + 哈希聚类
        let sig = card.error_signature.expect("失败卡片应有错误签名");
        assert_eq!(sig.error_type.as_ref(), "compile_error");
        assert_eq!(sig.error_hash.len(), 16, "SHA-256 前 16 位十六进制");
    }

    #[test]
    fn error_hash_dedup_clustering() {
        let gen = ExperienceCardGenerator::new("v1");
        // 同错误类型 + 摘要 → 同哈希（L3 idx_error_hash 聚类基础）
        let c1 = gen.generate(&metadata(AtomicOperator::Debug), &failure_validation());
        let c2 = gen.generate(&metadata(AtomicOperator::Debug), &failure_validation());
        assert_eq!(
            c1.error_signature.as_ref().unwrap().error_hash,
            c2.error_signature.as_ref().unwrap().error_hash
        );
    }

    #[test]
    fn timeout_status_classification() {
        let gen = ExperienceCardGenerator::new("v1");
        let mut v = failure_validation();
        v.timed_out = true;
        let card = gen.generate(&metadata(AtomicOperator::Improve), &v);
        assert_eq!(card.execution_status, ExecutionStatus::Timeout);
    }

    #[test]
    fn three_factor_pure_function() {
        // 铁律4: 同输入同输出
        let v = success_validation(0.8);
        let t1 = compute_three_factor(0.8, AtomicOperator::Draft, &v);
        let t2 = compute_three_factor(0.8, AtomicOperator::Draft, &v);
        assert_eq!(t1, t2);
        assert!((t1.quality - 0.8).abs() < 1e-6);
        // Draft 基础新颖性 0.3 + Token 效率（5000/3000>1 → +0.2）= 0.5
        assert!((t1.novelty - 0.5).abs() < 1e-6, "实际 {}", t1.novelty);
    }

    #[test]
    fn three_factor_novelty_operator_ordering() {
        let v = success_validation(0.5);
        let draft = compute_three_factor(0.5, AtomicOperator::Draft, &v);
        let crossover = compute_three_factor(0.5, AtomicOperator::Crossover, &v);
        assert!(
            crossover.novelty > draft.novelty,
            "Crossover 新颖性应高于 Draft"
        );
    }

    #[test]
    fn card_ids_monotonic_unique() {
        let gen = ExperienceCardGenerator::new("v1");
        let c1 = gen.generate(&metadata(AtomicOperator::Draft), &success_validation(0.5));
        let c2 = gen.generate(&metadata(AtomicOperator::Draft), &success_validation(0.5));
        assert_ne!(c1.card_id, c2.card_id);
        assert_eq!(c2.card_id.as_ref(), "card_0000000001");
    }

    #[test]
    fn generate_and_publish_without_bus_returns_card() {
        // 未注入 bus 时等价于 generate（D-5 默认行为）
        let gen = ExperienceCardGenerator::new("v1");
        let card =
            gen.generate_and_publish(&metadata(AtomicOperator::Draft), &success_validation(0.5));
        assert_eq!(card.card_id.as_ref(), "card_0000000000");
    }

    #[test]
    fn generate_and_publish_with_bus_delivers() {
        let bus = ExperienceCardBus::new();
        let mut rx = bus.subscribe_critical();
        let gen = ExperienceCardGenerator::new("v1").with_card_bus(bus);
        // 高分卡片（>0.8）走 Critical 通道
        gen.generate_and_publish(&metadata(AtomicOperator::Draft), &success_validation(0.95));
        let received = rx.try_recv();
        assert!(received.is_ok(), "高分卡片应经 Critical 通道送达");
    }

    #[test]
    fn meaningful_card_filter_semantics() {
        // generates_meaningful_card 过滤语义（消费方据此决定入库）
        assert!(ExecutionStatus::Success.generates_meaningful_card());
        assert!(ExecutionStatus::Error.generates_meaningful_card());
        assert!(ExecutionStatus::Timeout.generates_meaningful_card());
        assert!(!ExecutionStatus::ScoreFailed.generates_meaningful_card());
    }
}
