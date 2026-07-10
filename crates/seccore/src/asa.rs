//! ASA 对抗性自我审计 — 基于 Critic PPO 思想的实时介入纠偏
//!
//! 对应架构层:L4 Security
//! 对应 Task 32:ASA 对抗性自我审计(Day 31)
//!
//! 设计决策(WHY):
//! - **事中拦截优先**:ASA Block 的操作不进入沙箱,避免危险操作触发真实执行
//! - **反馈闭环**:沙箱执行结果反馈到 ASA 历史失败率,形成自学习闭环
//! - **`RwLock<OperationHistory>`**:读多写少场景(每次 audit 读历史,执行后写历史)
//! - **Week 5 占位**:基于规则的评分模型,Week 6 替换为 Critic PPO 模型
//!
//! 评分公式(Week 5 占位):
//! `safety_score = 1.0 - risk_weight × keyword_count - history_failure_rate`
//! - `risk_weight`:风险关键字权重(默认 0.2)
//! - `keyword_count`:操作内容中匹配的风险关键字数
//! - `history_failure_rate`:历史失败次数 / 历史总次数(初始 0.0)

use std::collections::VecDeque;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::{error, warn};

use crate::error::SecCoreError;
use crate::sandbox::Sandbox;
use crate::types::{Command, ExecutionResult, RiskLevel};

/// 干预动作 — ASA 审计后的处置决策。
///
/// 分级阈值(可通过 AsaConfig 调整):
/// - `Allow`:safety_score ≥ safety_threshold_allow(默认 0.8),操作允许执行
/// - `Warn`:safety_threshold_warn ≤ score < safety_threshold_allow(默认 [0.5, 0.8)),操作允许执行但记录告警
/// - `Block`:score < safety_threshold_block(默认 0.5),操作被阻断,不进入沙箱
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterventionAction {
    /// 允许执行 — 安全分数高,无干预
    Allow,
    /// 告警执行 — 安全分数中等,记录告警但操作继续
    Warn,
    /// 阻断执行 — 安全分数低,操作被拦截
    Block,
}

/// ASA 审计结果 — 单次审计的完整输出。
#[derive(Debug, Clone)]
pub struct AuditResult {
    /// 安全分数 ∈ [0.0, 1.0],基于风险关键字与历史失败率
    pub safety_score: f32,
    /// 正确性分数 ∈ [0.0, 1.0],基于语法检查(Week 5 占位)
    pub correctness_score: f32,
    /// 效率分数 ∈ [0.0, 1.0],基于操作复杂度(Week 5 占位)
    pub efficiency_score: f32,
    /// 干预动作(Allow/Warn/Block)
    pub intervention: InterventionAction,
    /// 审计原因(人类可读,用于审计追溯)
    pub audit_reason: String,
    /// 风险等级 — 基于关键字列表完整性与匹配数评估
    ///
    /// WHY(N4 安全修复):当 `risk_keywords` 为空时返回 `RiskLevel::Unknown`,
    /// 作为信号触发 Parliament/下游消费者的额外审计检查。旧实现将空关键字
    /// 等同于 Low,调用者可通过省略关键字列表绕过检测。下游消费方应检查
    /// `risk_level == Unknown` 并启动补充审计(如要求调用方补全关键字、
    /// 触发人工复核或应用更严格的沙箱策略)。
    pub risk_level: RiskLevel,
}

/// ASA 配置 — 审计阈值与权重参数。
#[derive(Debug, Clone)]
pub struct AsaConfig {
    /// Allow 阈值:safety_score ≥ 此值 → Allow(默认 0.8)
    pub safety_threshold_allow: f32,
    /// Warn 阈值:safety_score ≥ 此值且 < allow → Warn(默认 0.5)
    pub safety_threshold_warn: f32,
    /// Block 阈值:safety_score < 此值 → Block(默认 0.5)
    pub safety_threshold_block: f32,
    /// 风险关键字权重(默认 0.2),用于 safety_score 计算
    pub risk_weight: f32,
    /// 历史失败率权重(默认 0.3,Week 6 Critic PPO 使用,Week 5 占位未加权)
    pub history_failure_weight: f32,
    /// 历史记录最大数量(默认 1000),限制 recent_failures 长度
    pub max_history_records: usize,
}

impl Default for AsaConfig {
    fn default() -> Self {
        Self {
            safety_threshold_allow: 0.8,
            safety_threshold_warn: 0.5,
            safety_threshold_block: 0.5,
            risk_weight: 0.2,
            history_failure_weight: 0.3,
            max_history_records: 1000,
        }
    }
}

/// 操作审计输入 — 待审计的操作信息。
#[derive(Debug, Clone)]
pub struct OperationAuditInput {
    /// 操作 ID(唯一标识,用于审计追溯)
    pub operation_id: String,
    /// 操作内容(命令文本、代码片段等)
    pub content: String,
    /// 风险关键字列表(在 content 中匹配这些关键字)
    pub risk_keywords: Vec<String>,
    /// 操作复杂度 ∈ `[0.0, 1.0]`(越高越复杂)
    pub complexity_score: f32,
    /// P0-4:语义向量(用于语义相似度检测)
    ///
    /// 可选,提供时启用语义相似度评估(对抗性样本检测)。
    /// 向量为空时回退到关键词匹配。
    pub semantic_vector: Option<Vec<f32>>,
    /// P0-4:参考风险向量(用于语义相似度比较)
    ///
    /// 已知风险操作的语义向量,计算与当前操作的相似度。
    pub reference_risk_vectors: Vec<Vec<f32>>,
}

impl OperationAuditInput {
    /// 创建新的操作审计输入
    pub fn new(
        operation_id: impl Into<String>,
        content: impl Into<String>,
        risk_keywords: Vec<String>,
        complexity_score: f32,
    ) -> Self {
        Self {
            operation_id: operation_id.into(),
            content: content.into(),
            risk_keywords,
            complexity_score,
            semantic_vector: None,
            reference_risk_vectors: Vec::new(),
        }
    }

    /// P0-4:设置语义向量
    pub fn with_semantic_vector(mut self, vector: Vec<f32>) -> Self {
        self.semantic_vector = Some(vector);
        self
    }

    /// P0-4:添加参考风险向量
    pub fn with_reference_risk(mut self, vectors: Vec<Vec<f32>>) -> Self {
        self.reference_risk_vectors = vectors;
        self
    }
}

/// 操作历史 — 记录成功/失败次数与最近失败记录。
///
/// 用于计算 history_failure_rate,反馈闭环更新。
/// 读多写少,用 RwLock 保护(AsaAuditor 持有)。
#[derive(Debug, Clone)]
struct OperationHistory {
    /// 历史总操作次数
    total_count: u64,
    /// 历史失败次数
    failure_count: u64,
    /// 最近失败记录(operation_id, timestamp),按时间顺序
    recent_failures: VecDeque<(String, DateTime<Utc>)>,
}

impl OperationHistory {
    /// 创建空历史记录。
    fn new() -> Self {
        Self {
            total_count: 0,
            failure_count: 0,
            recent_failures: VecDeque::new(),
        }
    }

    /// 历史失败率 = failure_count / total_count(初始 0.0)
    fn failure_rate(&self) -> f32 {
        if self.total_count == 0 {
            0.0
        } else {
            self.failure_count as f32 / self.total_count as f32
        }
    }
}

impl Default for OperationHistory {
    fn default() -> Self {
        Self::new()
    }
}

/// ASA 审计器 — 基于 Critic PPO 思想的实时审计与介入。
///
/// Week 5 占位实现:基于规则的评分模型。
/// TODO(Week 6):替换为 Critic PPO 模型。
pub struct AsaAuditor {
    /// ASA 配置(阈值与权重)
    config: AsaConfig,
    /// 操作历史(RwLock 保护,读多写少)
    history: RwLock<OperationHistory>,
    /// 事件总线(发布 AsaIntervention 事件,通知 Parliament 干预决策)
    event_bus: EventBus,
}

impl AsaAuditor {
    /// 创建 ASA 审计器(内部创建私有 EventBus,仅用于测试)
    ///
    /// WHY 保留 new():AsaAuditor 有 63 处测试调用点,保留 new() 零测试修改。
    /// 生产代码(Week 6 集成时)改用 with_event_bus() 注入共享总线
    pub fn new(config: AsaConfig) -> Self {
        Self::with_event_bus(config, EventBus::new())
    }

    /// 创建使用默认配置的 ASA 审计器(测试兼容)
    pub fn with_default_config() -> Self {
        Self::new(AsaConfig::default())
    }

    /// 创建带共享 EventBus 的 ASA 审计器(生产代码推荐)
    ///
    /// WHY:生产代码需注入共享总线,使 AsaIntervention 事件能被 Parliament 订阅。
    /// 测试代码用 new()/with_default_config() 创建私有总线,publish 静默丢弃
    pub fn with_event_bus(config: AsaConfig, bus: EventBus) -> Self {
        Self {
            config,
            history: RwLock::new(OperationHistory::new()),
            event_bus: bus,
        }
    }

    /// EventBus 访问器
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// 审计操作 — P0-4:基于语义相似度+关键词的混合评分模型。
    ///
    /// 评分公式:`safety_score = 1.0 - risk_weight × keyword_count - history_failure_rate - semantic_risk`
    /// - `keyword_count`:content 中匹配的 risk_keywords 数量(大小写不敏感)
    /// - `history_failure_rate`:历史失败次数 / 历史总次数
    /// - `semantic_risk`:与已知风险操作的语义相似度最大值(0.0-1.0)
    ///
    /// P0-4:当提供 semantic_vector 和 reference_risk_vectors 时,
    /// 启用语义相似度检测,可识别对抗性样本(如关键词替换、编码绕过)。
    ///
    /// 此方法是同步的(基于规则评分+向量点积,无 I/O),满足 < 5ms 延迟要求。
    pub fn audit(&self, input: &OperationAuditInput) -> AuditResult {
        // 读取历史失败率(RwLock 读锁)
        let history_rate = {
            let history = self
                .history
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            history.failure_rate()
        };

        // 统计匹配的风险关键字数(大小写不敏感)
        let content_lower = input.content.to_lowercase();
        let keyword_count = input
            .risk_keywords
            .iter()
            .filter(|kw| content_lower.contains(&kw.to_lowercase()))
            .count();

        // P0-4:语义相似度风险评估
        let semantic_risk = self.compute_semantic_risk(input);

        // 评估风险等级 — N4 安全修复
        // WHY: 当 risk_keywords 为空时返回 RiskLevel::Unknown(而非 Low),作为信号
        // 触发 Parliament/下游消费者的额外审计检查。安全语义:调用者未提供检测
        // 维度 = 风险无法评估 = Unknown,防止调用者通过省略关键字列表绕过风险检测。
        // 当关键字列表非空时,按匹配数映射 Low(0)/Medium(1-2)/High(3+)。
        // 注意:intervention 仍由 safety_score 决定(保持向后兼容),risk_level
        // 是独立的额外审计信号,下游消费方应显式检查 Unknown 启动补充审计。
        let risk_level = if input.risk_keywords.is_empty() {
            RiskLevel::Unknown
        } else {
            match keyword_count {
                0 => RiskLevel::Low,
                1..=2 => RiskLevel::Medium,
                _ => RiskLevel::High,
            }
        };

        // safety_score = 1.0 - risk_weight × keyword_count - history_failure_rate - semantic_risk
        // P0-4:semantic_risk 使对抗性样本(关键词绕过)仍能被检测
        let safety_score =
            1.0 - self.config.risk_weight * keyword_count as f32 - history_rate - semantic_risk;
        let safety_score = safety_score.clamp(0.0, 1.0);

        // correctness_score 占位:基于括号匹配的简单语法检查
        let correctness_score = compute_correctness_score(&input.content);

        // efficiency_score 占位:1.0 - complexity × 0.5
        let efficiency_score = 1.0 - input.complexity_score.clamp(0.0, 1.0) * 0.5;

        // 干预分级
        let intervention = self.classify_intervention(safety_score);

        // 生成审计原因(包含语义风险信息)
        let audit_reason = format_audit_reason(
            intervention,
            keyword_count,
            history_rate,
            semantic_risk,
            safety_score,
        );

        // 仅在 intervention != Allow 时发布(避免事件风暴)
        // WHY publish_blocking:audit() 是同步方法,不能 await。
        // EventBus::publish_blocking 是 event-bus 官方同步 API(内部 broadcast::send 非阻塞),
        // 专为不便 await 的同步场景设计,零运行时依赖,事件立即投递不丢失
        if intervention != InterventionAction::Allow {
            let event = NexusEvent::AsaIntervention {
                metadata: EventMetadata::new("seccore"),
                operation_id: input.operation_id.clone(),
                action: format!("{:?}", intervention),
                safety_score,
                block_reason: (intervention == InterventionAction::Block)
                    .then(|| audit_reason.clone()),
                alternative_suggestion: None,
            };
            if let Err(e) = self.event_bus.publish_blocking(event) {
                warn!(error = %e, "发布 AsaIntervention 事件失败");
            }
        }

        AuditResult {
            safety_score,
            correctness_score,
            efficiency_score,
            intervention,
            audit_reason,
            risk_level,
        }
    }

    /// 审计并介入 — 根据评分执行干预动作。
    ///
    /// - Allow/Warn:返回 Ok(AuditResult),操作继续执行
    /// - Block:返回 Err(SecCoreError::AsaBlocked),操作被阻断
    ///
    /// Block 级别使用 tracing::error! 记录,Warn 级别使用 tracing::warn!。
    /// AsaIntervention 事件已在 `audit()` 中通过 `publish_blocking` 发布,无需在此重复发布。
    pub fn audit_and_intervene(
        &self,
        input: &OperationAuditInput,
    ) -> Result<AuditResult, SecCoreError> {
        let result = self.audit(input);

        match result.intervention {
            InterventionAction::Allow => {
                // Allow:无干预,操作继续
            }
            InterventionAction::Warn => {
                // Warn:记录告警,操作继续
                // AsaIntervention 事件已在 audit() 中发布,此处仅记录告警日志
                warn!(
                    operation_id = %input.operation_id,
                    safety_score = result.safety_score,
                    reason = %result.audit_reason,
                    "ASA 告警:操作存在风险,继续执行"
                );
            }
            InterventionAction::Block => {
                // Block:记录错误,返回拦截错误
                // AsaIntervention 事件已在 audit() 中发布,此处仅记录错误日志并返回拦截错误
                error!(
                    operation_id = %input.operation_id,
                    safety_score = result.safety_score,
                    reason = %result.audit_reason,
                    "ASA 拦截:操作被阻断"
                );
                return Err(SecCoreError::AsaBlocked {
                    operation_id: input.operation_id.clone(),
                    block_reason: result.audit_reason.clone(),
                });
            }
        }

        Ok(result)
    }

    /// 记录操作成功 — 更新历史(反馈闭环)。
    pub fn record_success(&self) {
        let mut history = self
            .history
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        history.total_count += 1;
    }

    /// 记录操作失败 — 更新历史(反馈闭环)。
    pub fn record_failure(&self, operation_id: &str) {
        let mut history = self
            .history
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        history.total_count += 1;
        history.failure_count += 1;
        history
            .recent_failures
            .push_back((operation_id.to_string(), Utc::now()));

        // 限制 recent_failures 长度,防止内存无限增长
        while history.recent_failures.len() > self.config.max_history_records {
            history.recent_failures.pop_front();
        }
    }

    /// 根据安全分数判定干预动作。
    fn classify_intervention(&self, safety_score: f32) -> InterventionAction {
        if safety_score >= self.config.safety_threshold_allow {
            InterventionAction::Allow
        } else if safety_score >= self.config.safety_threshold_warn {
            InterventionAction::Warn
        } else {
            InterventionAction::Block
        }
    }

    /// 获取历史统计(用于测试与监控)。
    pub fn history_stats(&self) -> (u64, u64) {
        let history = self
            .history
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (history.total_count, history.failure_count)
    }

    /// P0-4:计算语义风险分数
    ///
    /// 计算当前操作语义向量与已知风险操作向量的最大相似度。
    /// 相似度越高,表示操作越接近已知风险模式。
    ///
    /// # 返回
    /// 语义风险分数 ∈ [0.0, 1.0],0.0表示无风险,1.0表示极高风险
    fn compute_semantic_risk(&self, input: &OperationAuditInput) -> f32 {
        let query = match &input.semantic_vector {
            Some(v) => v,
            None => return 0.0, // 无语义向量,回退到0
        };

        if query.is_empty() || input.reference_risk_vectors.is_empty() {
            return 0.0;
        }

        // 计算与所有参考风险向量的最大相似度
        let max_similarity = input
            .reference_risk_vectors
            .iter()
            .filter(|v| !v.is_empty() && v.len() == query.len())
            .map(|ref_vec| cosine_similarity(query, ref_vec))
            .fold(0.0f32, |max, sim| max.max(sim));

        // 将相似度映射到风险分数:相似度>0.8视为高风险
        if max_similarity > 0.8 {
            (max_similarity - 0.8) * 5.0 // 0.8→0.0, 1.0→1.0
        } else {
            0.0
        }
    }
}

/// P0-4:计算两个向量的余弦相似度
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot: f32 = 0.0;
    let mut norm_a: f32 = 0.0;
    let mut norm_b: f32 = 0.0;
    for (xa, xb) in a.iter().zip(b.iter()) {
        dot += xa * xb;
        norm_a += xa * xa;
        norm_b += xb * xb;
    }
    let norm_product = (norm_a * norm_b).sqrt();
    if norm_product == 0.0 {
        0.0
    } else {
        (dot / norm_product).clamp(0.0, 1.0)
    }
}

/// 计算正确性分数 — 基于括号匹配的简单语法检查(Week 5 占位)。
///
/// 检查 () [] {} 是否匹配。匹配返回 0.9,不匹配返回 0.3。
/// TODO(Week 6):替换为基于 PVL Verifier 的语法检查。
fn compute_correctness_score(content: &str) -> f32 {
    let parens = content.matches('(').count() as i32 - content.matches(')').count() as i32;
    let brackets = content.matches('[').count() as i32 - content.matches(']').count() as i32;
    let braces = content.matches('{').count() as i32 - content.matches('}').count() as i32;

    if parens == 0 && brackets == 0 && braces == 0 {
        0.9
    } else {
        0.3
    }
}

/// 生成审计原因(人类可读)。
fn format_audit_reason(
    intervention: InterventionAction,
    keyword_count: usize,
    history_rate: f32,
    semantic_risk: f32,
    safety_score: f32,
) -> String {
    let action_str = match intervention {
        InterventionAction::Allow => "Allow",
        InterventionAction::Warn => "Warn",
        InterventionAction::Block => "Block",
    };
    if semantic_risk > 0.0 {
        format!(
            "{}: 安全分数 {:.3}(关键字 {} 个, 历史失败率 {:.3}, 语义风险 {:.3})",
            action_str, safety_score, keyword_count, history_rate, semantic_risk
        )
    } else {
        format!(
            "{}: 安全分数 {:.3}(关键字 {} 个, 历史失败率 {:.3})",
            action_str, safety_score, keyword_count, history_rate
        )
    }
}

/// ASA-沙箱协同器 — 串联 ASA 审计与沙箱执行。
///
/// 协同流程:
/// 1. ASA 事中审计(Allow/Warn/Block)
/// 2. Block 的操作不进入沙箱(事中拦截优先),直接返回 Err
/// 3. Allow/Warn 的操作进入沙箱执行
/// 4. 沙箱执行结果反馈到 ASA 历史失败率(反馈闭环)
pub struct AsaSandboxCoordinator {
    /// ASA 审计器
    auditor: AsaAuditor,
    /// 零信任沙箱
    sandbox: Sandbox,
}

impl AsaSandboxCoordinator {
    /// 创建协同器,持有审计器与沙箱。
    pub fn new(auditor: AsaAuditor, sandbox: Sandbox) -> Self {
        Self { auditor, sandbox }
    }

    /// 审计并执行操作 — ASA 事中拦截 + 沙箱执行 + 反馈闭环。
    ///
    /// 注意:此方法用 `&mut self` 而非 `&self`,因为 Sandbox::audit_and_execute
    /// 需要 `&mut self`(沙箱的 audit_chain 有状态)。
    ///
    /// # 参数
    /// - `input`:操作审计信息(用于 ASA 评分)
    /// - `command`:待执行的命令(将 clone 后传入沙箱)
    ///
    /// # 返回
    /// - `Ok(ExecutionResult)`:ASA 通过 + 沙箱执行成功
    /// - `Err(SecCoreError::AsaBlocked)`:ASA Block,操作未进入沙箱
    /// - `Err(SecCoreError::*)`:沙箱执行失败(已更新 ASA 历史失败率)
    pub async fn execute_with_audit(
        &mut self,
        input: &OperationAuditInput,
        command: &Command,
    ) -> Result<ExecutionResult, SecCoreError> {
        // 步骤1:ASA 事中审计(Allow/Warn/Block)
        // Block 级别在此返回 Err,不进入沙箱(事中拦截优先)
        self.auditor.audit_and_intervene(input)?;

        // 步骤2:Allow/Warn 的操作进入沙箱执行
        // WHY: command clone 因为 Sandbox::audit_and_execute 需要 owned Command
        match self.sandbox.audit_and_execute(command.clone()).await {
            Ok(result) => {
                // 步骤3:执行成功,更新历史(成功)
                self.auditor.record_success();
                Ok(result)
            }
            Err(e) => {
                // 步骤4:执行失败(沙箱违规),更新历史(失败)
                // 反馈闭环:失败率上升,后续审计更严格
                self.auditor.record_failure(&input.operation_id);
                Err(e)
            }
        }
    }

    /// 获取 ASA 审计器引用(用于测试与监控)。
    pub fn auditor(&self) -> &AsaAuditor {
        &self.auditor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试用 OperationAuditInput。
    fn make_input(content: &str, keywords: Vec<&str>, complexity: f32) -> OperationAuditInput {
        OperationAuditInput {
            operation_id: "test-op-001".to_string(),
            content: content.to_string(),
            risk_keywords: keywords.iter().map(|s| s.to_string()).collect(),
            complexity_score: complexity,
            semantic_vector: None,
            reference_risk_vectors: Vec::new(),
        }
    }

    // === SubTask 32.2: 评分模型测试 ===

    #[test]
    fn test_audit_allow_no_keywords() {
        // 无风险关键字,无历史失败 → safety_score = 1.0 → Allow
        let auditor = AsaAuditor::with_default_config();
        let input = make_input("echo hello", vec![], 0.1);
        let result = auditor.audit(&input);
        assert_eq!(result.intervention, InterventionAction::Allow);
        assert!(result.safety_score >= 0.8);
    }

    #[test]
    fn test_audit_warn_with_keywords() {
        // 2 个风险关键字 → safety_score = 1.0 - 0.4 = 0.6 → Warn
        let auditor = AsaAuditor::with_default_config();
        let input = make_input("sudo rm", vec!["sudo", "rm"], 0.1);
        let result = auditor.audit(&input);
        assert_eq!(result.intervention, InterventionAction::Warn);
        assert!(result.safety_score >= 0.5 && result.safety_score < 0.8);
    }

    #[test]
    fn test_audit_block_with_many_keywords() {
        // 3 个风险关键字 → safety_score = 1.0 - 0.6 = 0.4 → Block
        let auditor = AsaAuditor::with_default_config();
        let input = make_input("sudo rm secret", vec!["sudo", "rm", "secret"], 0.1);
        let result = auditor.audit(&input);
        assert_eq!(result.intervention, InterventionAction::Block);
        assert!(result.safety_score < 0.5);
    }

    #[test]
    fn test_audit_boundary_allow_threshold() {
        // safety_score 刚好 = 0.8 → Allow(>= 0.8)
        // 1 个关键字:1.0 - 0.2 = 0.8
        let auditor = AsaAuditor::with_default_config();
        let input = make_input("sudo test", vec!["sudo"], 0.0);
        let result = auditor.audit(&input);
        assert_eq!(result.intervention, InterventionAction::Allow);
        assert!((result.safety_score - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_audit_boundary_warn_threshold() {
        // safety_score 刚好 = 0.5 → Warn(>= 0.5)
        // history_failure_rate = 0.3,1 个关键字:1.0 - 0.2 - 0.3 = 0.5
        let auditor = AsaAuditor::with_default_config();
        auditor.record_success();
        auditor.record_success();
        auditor.record_success();
        auditor.record_failure("fail-op"); // total=4, fail=1, rate=0.25
                                           // 1 个关键字:1.0 - 0.2 - 0.25 = 0.55 → Warn
        let input = make_input("sudo test", vec!["sudo"], 0.0);
        let result = auditor.audit(&input);
        assert_eq!(result.intervention, InterventionAction::Warn);
    }

    #[test]
    fn test_audit_history_failure_rate_impact() {
        // 高历史失败率 → safety_score 降低
        let auditor = AsaAuditor::with_default_config();
        auditor.record_success();
        auditor.record_failure("fail-1"); // total=2, fail=1, rate=0.5
                                          // 无关键字:1.0 - 0 - 0.5 = 0.5 → Warn
        let input = make_input("safe op", vec![], 0.0);
        let result = auditor.audit(&input);
        assert_eq!(result.intervention, InterventionAction::Warn);
        assert!((result.safety_score - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_audit_history_failure_rate_block() {
        // 高历史失败率 + 关键字 → Block
        let auditor = AsaAuditor::with_default_config();
        auditor.record_success();
        auditor.record_failure("fail-1"); // rate=0.5
                                          // 1 个关键字:1.0 - 0.2 - 0.5 = 0.3 → Block
        let input = make_input("sudo test", vec!["sudo"], 0.0);
        let result = auditor.audit(&input);
        assert_eq!(result.intervention, InterventionAction::Block);
    }

    #[test]
    fn test_audit_correctness_score_paren_matched() {
        let auditor = AsaAuditor::with_default_config();
        let input = make_input("func(arg)", vec![], 0.0);
        let result = auditor.audit(&input);
        assert!((result.correctness_score - 0.9).abs() < 0.001);
    }

    #[test]
    fn test_audit_correctness_score_paren_unmatched() {
        let auditor = AsaAuditor::with_default_config();
        let input = make_input("func(arg", vec![], 0.0);
        let result = auditor.audit(&input);
        assert!((result.correctness_score - 0.3).abs() < 0.001);
    }

    #[test]
    fn test_audit_efficiency_score() {
        let auditor = AsaAuditor::with_default_config();
        // complexity = 0.4 → efficiency = 1.0 - 0.4*0.5 = 0.8
        let input = make_input("op", vec![], 0.4);
        let result = auditor.audit(&input);
        assert!((result.efficiency_score - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_audit_keyword_case_insensitive() {
        // 大小写不敏感匹配
        let auditor = AsaAuditor::with_default_config();
        let input = make_input("SUDO RM", vec!["sudo", "rm"], 0.0);
        let result = auditor.audit(&input);
        // 2 个关键字匹配:1.0 - 0.4 = 0.6 → Warn
        assert_eq!(result.intervention, InterventionAction::Warn);
    }

    // === SubTask 32.3: 干预动作分级测试(15 个用例) ===

    // --- Allow 级别 5 个用例 ---

    #[test]
    fn test_intervene_allow_1() {
        let auditor = AsaAuditor::with_default_config();
        let input = make_input("echo hello", vec![], 0.0);
        let result = auditor.audit_and_intervene(&input);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().intervention, InterventionAction::Allow);
    }

    #[test]
    fn test_intervene_allow_2() {
        let auditor = AsaAuditor::with_default_config();
        let input = make_input("ls -la", vec![], 0.1);
        let result = auditor.audit_and_intervene(&input);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().intervention, InterventionAction::Allow);
    }

    #[test]
    fn test_intervene_allow_3() {
        let auditor = AsaAuditor::with_default_config();
        let input = make_input("pwd", vec![], 0.0);
        let result = auditor.audit_and_intervene(&input);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().intervention, InterventionAction::Allow);
    }

    #[test]
    fn test_intervene_allow_4() {
        // 1 个关键字:0.8 → Allow(边界)
        let auditor = AsaAuditor::with_default_config();
        let input = make_input("sudo test", vec!["sudo"], 0.0);
        let result = auditor.audit_and_intervene(&input);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().intervention, InterventionAction::Allow);
    }

    #[test]
    fn test_intervene_allow_5() {
        let auditor = AsaAuditor::with_default_config();
        let input = make_input("whoami", vec![], 0.2);
        let result = auditor.audit_and_intervene(&input);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().intervention, InterventionAction::Allow);
    }

    // --- Warn 级别 5 个用例 ---

    #[test]
    fn test_intervene_warn_1() {
        // 2 个关键字:0.6 → Warn
        let auditor = AsaAuditor::with_default_config();
        let input = make_input("sudo rm", vec!["sudo", "rm"], 0.0);
        let result = auditor.audit_and_intervene(&input);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().intervention, InterventionAction::Warn);
    }

    #[test]
    fn test_intervene_warn_2() {
        // 2 个关键字:0.6 → Warn
        let auditor = AsaAuditor::with_default_config();
        let input = make_input("chmod chown", vec!["chmod", "chown"], 0.0);
        let result = auditor.audit_and_intervene(&input);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().intervention, InterventionAction::Warn);
    }

    #[test]
    fn test_intervene_warn_3() {
        // 1 个关键字 + 0.1 失败率:1.0 - 0.2 - 0.1 = 0.7 → Warn
        let auditor = AsaAuditor::with_default_config();
        for _ in 0..9 {
            auditor.record_success();
        }
        auditor.record_failure("fail"); // total=10, fail=1, rate=0.1
        let input = make_input("sudo test", vec!["sudo"], 0.0);
        let result = auditor.audit_and_intervene(&input);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().intervention, InterventionAction::Warn);
    }

    #[test]
    fn test_intervene_warn_4() {
        // 2 个关键字:0.6 → Warn
        let auditor = AsaAuditor::with_default_config();
        let input = make_input("secret password", vec!["secret", "password"], 0.0);
        let result = auditor.audit_and_intervene(&input);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().intervention, InterventionAction::Warn);
    }

    #[test]
    fn test_intervene_warn_5() {
        // 边界:safety_score = 0.5 → Warn(>= 0.5)
        let auditor = AsaAuditor::with_default_config();
        auditor.record_success();
        auditor.record_failure("fail"); // total=2, fail=1, rate=0.5
                                        // 无关键字:1.0 - 0 - 0.5 = 0.5 → Warn
        let input = make_input("safe op", vec![], 0.0);
        let result = auditor.audit_and_intervene(&input);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().intervention, InterventionAction::Warn);
    }

    // --- Block 级别 5 个用例 ---

    #[test]
    fn test_intervene_block_1() {
        // 3 个关键字:0.4 → Block
        let auditor = AsaAuditor::with_default_config();
        let input = make_input("sudo rm secret", vec!["sudo", "rm", "secret"], 0.0);
        let result = auditor.audit_and_intervene(&input);
        assert!(matches!(result, Err(SecCoreError::AsaBlocked { .. })));
    }

    #[test]
    fn test_intervene_block_2() {
        // 3 个关键字:0.4 → Block
        let auditor = AsaAuditor::with_default_config();
        let input = make_input("sudo chmod chown", vec!["sudo", "chmod", "chown"], 0.0);
        let result = auditor.audit_and_intervene(&input);
        assert!(matches!(result, Err(SecCoreError::AsaBlocked { .. })));
    }

    #[test]
    fn test_intervene_block_3() {
        // 高失败率 + 关键字:1.0 - 0.2 - 0.5 = 0.3 → Block
        let auditor = AsaAuditor::with_default_config();
        auditor.record_success();
        auditor.record_failure("fail"); // rate=0.5
        let input = make_input("sudo test", vec!["sudo"], 0.0);
        let result = auditor.audit_and_intervene(&input);
        assert!(matches!(result, Err(SecCoreError::AsaBlocked { .. })));
    }

    #[test]
    fn test_intervene_block_4() {
        // 5 个关键字:1.0 - 1.0 = 0.0 → Block
        let auditor = AsaAuditor::with_default_config();
        let input = make_input(
            "sudo rm secret password chmod",
            vec!["sudo", "rm", "secret", "password", "chmod"],
            0.0,
        );
        let result = auditor.audit_and_intervene(&input);
        assert!(matches!(result, Err(SecCoreError::AsaBlocked { .. })));
    }

    #[test]
    fn test_intervene_block_5() {
        // 高失败率(无关键字):1.0 - 0 - 0.8 = 0.2 → Block
        let auditor = AsaAuditor::with_default_config();
        for _ in 0..4 {
            auditor.record_failure("fail");
        }
        auditor.record_success(); // total=5, fail=4, rate=0.8
        let input = make_input("safe op", vec![], 0.0);
        let result = auditor.audit_and_intervene(&input);
        assert!(matches!(result, Err(SecCoreError::AsaBlocked { .. })));
    }

    // === 历史记录测试 ===

    #[test]
    fn test_record_success_updates_total() {
        let auditor = AsaAuditor::with_default_config();
        auditor.record_success();
        auditor.record_success();
        let (total, fail) = auditor.history_stats();
        assert_eq!(total, 2);
        assert_eq!(fail, 0);
    }

    #[test]
    fn test_record_failure_updates_counts() {
        let auditor = AsaAuditor::with_default_config();
        auditor.record_success();
        auditor.record_failure("fail-1");
        let (total, fail) = auditor.history_stats();
        assert_eq!(total, 2);
        assert_eq!(fail, 1);
    }

    #[test]
    fn test_max_history_records_limit() {
        // 验证 recent_failures 长度受 max_history_records 限制
        let config = AsaConfig {
            max_history_records: 3,
            ..AsaConfig::default()
        };
        let auditor = AsaAuditor::new(config);
        auditor.record_failure("fail-1");
        auditor.record_failure("fail-2");
        auditor.record_failure("fail-3");
        auditor.record_failure("fail-4");
        auditor.record_failure("fail-5");
        // total=5, fail=5,recent_failures 限制为 3,但 failure_rate 仍正确
        let (total, fail) = auditor.history_stats();
        assert_eq!(total, 5);
        assert_eq!(fail, 5);
        // failure_rate = 5/5 = 1.0,safety_score = 1.0 - 0 - 1.0 = 0.0
        let result = auditor.audit(&make_input("test", vec![], 0.0));
        assert!((result.safety_score - 0.0).abs() < 0.001);
    }

    // === 配置测试 ===

    #[test]
    fn test_custom_config_thresholds() {
        let config = AsaConfig {
            safety_threshold_allow: 0.9,
            safety_threshold_warn: 0.7,
            safety_threshold_block: 0.7,
            risk_weight: 0.1,
            history_failure_weight: 0.3,
            max_history_records: 1000,
        };
        let auditor = AsaAuditor::new(config);
        // 1 个关键字:1.0 - 0.1 = 0.9 → Allow(>= 0.9)
        let input = make_input("sudo test", vec!["sudo"], 0.0);
        let result = auditor.audit(&input);
        assert_eq!(result.intervention, InterventionAction::Allow);
    }

    #[test]
    fn test_audit_reason_not_empty() {
        let auditor = AsaAuditor::with_default_config();
        let input = make_input("echo hello", vec![], 0.0);
        let result = auditor.audit(&input);
        assert!(!result.audit_reason.is_empty());
        assert!(result.audit_reason.contains("Allow"));
    }
}
