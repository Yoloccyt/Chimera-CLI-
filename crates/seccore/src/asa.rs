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

use crate::asa_ppo::PpoCritic;
use crate::asa_score_fusion::ScoreFusion;
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
/// P3-3 增强:集成 PPO Critic 模型和 ScoreFusion 评分融合器。
///
/// # 评分融合策略
/// - PPO 未初始化(冷启动):仅使用规则评分
/// - PPO 高置信度:使用 PPO 评分(规则评分 Block 时优先)
/// - PPO 低置信度:规则评分与 PPO 评分加权平均
pub struct AsaAuditor {
    /// ASA 配置(阈值与权重)
    config: AsaConfig,
    /// 操作历史(RwLock 保护,读多写少)
    history: RwLock<OperationHistory>,
    /// 事件总线(发布 AsaIntervention 事件,通知 Parliament 干预决策)
    event_bus: EventBus,
    /// PPO Critic 模型(可选,RwLock 支持内部可变性用于在线学习)
    pub(crate) ppocritic: Option<RwLock<PpoCritic>>,
    /// 评分融合器(协调规则评分与 PPO 评分)
    score_fusion: ScoreFusion,
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
            ppocritic: None,
            score_fusion: ScoreFusion::new(),
        }
    }

    /// 创建带 PPO Critic 模型和共享 EventBus 的 ASA 审计器
    ///
    /// PPO 模型初始化后即可使用(随机权重),冷启动时通过 `record_success`/
    /// `record_failure` 反馈闭环在线学习。
    pub fn with_ppo(config: AsaConfig, bus: EventBus, ppocritic: PpoCritic) -> Self {
        Self {
            config,
            history: RwLock::new(OperationHistory::new()),
            event_bus: bus,
            ppocritic: Some(RwLock::new(ppocritic)),
            score_fusion: ScoreFusion::new(),
        }
    }

    /// EventBus 访问器
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// 审计操作 — 基于规则评分 + 可选 PPO 强化学习评分
    ///
    /// 评分公式(规则):`safety_score = 1.0 - risk_weight × keyword_count - history_failure_rate`
    ///
    /// P3-3 增强:当 PPO Critic 模型可用时,通过 ScoreFusion 将规则评分与 PPO 评分融合,
    /// 提供更准确的安全评估。PPO 未初始化时退化为纯规则评分(冷启动保底)。
    ///
    /// # 评分融合流程
    /// 1. 计算规则评分(现有公式)
    /// 2. 如果 PPO 已初始化,构建状态向量,执行前向推理
    /// 3. ScoreFusion::fuse(规则评分, PPO 评分, PPO 置信度)
    /// 4. 使用融合评分确定干预动作
    ///
    /// 此方法是同步的(基于规则评分 + 纯 Rust 前向推理,无 I/O),满足 < 50μs 延迟要求。
    ///
    /// # P1-W4.1 tracing 贯穿观测
    /// span 携带 `operation_id` / `safety_score` / `intervention` / `risk_level` 字段,
    /// `safety_score` 与 `intervention` 在函数内部计算后通过 `Span::current().record()`
    /// 填充(instrument fields 不能引用局部变量)。`input.content` 可能含敏感信息,
    /// 故 `skip(self, input)` 仅记录显式声明的字段。
    #[tracing::instrument(
        skip(self, input),
        fields(
            operation_id = %input.operation_id,
            safety_score,
            intervention,
            risk_level
        )
    )]
    pub fn audit(&self, input: &OperationAuditInput) -> AuditResult {
        // 读取历史失败率(RwLock 读锁)
        // WHY: unwrap_or_else 处理 PoisonError,避免 expect/unwrap,poisoned 时仍可访问数据
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

        // safety_score = 1.0 - risk_weight × keyword_count - history_failure_rate
        // Week 5 占位:history_failure_rate 直接使用(不加权)
        let rule_score = 1.0 - self.config.risk_weight * keyword_count as f32 - history_rate;
        let rule_score = rule_score.clamp(0.0, 1.0);

        // P3-3: PPO 评分融合
        let (fused_score, ppo_used) = if let Some(ref critic_lock) = self.ppocritic {
            if let Ok(critic) = critic_lock.read() {
                // 构建状态向量: (keyword_count, history_failure_rate, complexity_score, op_type)
                let state = build_ppo_state(keyword_count, history_rate, input.complexity_score);
                let ppo_output = critic.forward(&state);
                let ppo_confidence = critic.confidence(&ppo_output);
                let ppo_score = PpoCritic::q_values_to_score(&ppo_output);
                let fused = self
                    .score_fusion
                    .fuse(rule_score, Some(ppo_score), ppo_confidence);
                (fused, true)
            } else {
                // RwLock poisoned,退化为规则评分
                (rule_score, false)
            }
        } else {
            (rule_score, false)
        };

        // correctness_score 占位:基于括号匹配的简单语法检查
        let correctness_score = compute_correctness_score(&input.content);

        // efficiency_score 占位:1.0 - complexity × 0.5
        let efficiency_score = 1.0 - input.complexity_score.clamp(0.0, 1.0) * 0.5;

        // 使用融合评分确定干预动作
        let intervention = self.classify_intervention(fused_score);

        // P1-W4.1: 填充 instrument span 的延迟字段(safety_score / intervention / risk_level
        // 在函数内计算后才能确定)。这些字段供 efficiency-monitor 关联同一审计的
        // 评分与最终干预决策,支持事后回放分析。
        // WHY tracing::field::debug:intervention / risk_level 是自定义 enum,未实现
        // tracing::Value,用 debug() 包装为 Debug Value(?value 是宏语法,record 不接受)
        tracing::Span::current()
            .record("safety_score", fused_score)
            .record("intervention", tracing::field::debug(&intervention))
            .record("risk_level", tracing::field::debug(&risk_level));

        // P1-W4.1: 所有路径(含 Allow)发出 debug 事件,确保 span 字段被 tracing-test 捕获。
        // WHY debug 而非 info:Allow 路径是高频常态(大多数操作安全),info 级会产生日志噪声;
        // debug 级在 生产环境默认被 env-filter 过滤,仅测试与调试时可见,兼顾可观测性与低噪声。
        // 此事件使 operation_id / safety_score / intervention / risk_level 字段进入日志,
        // 供 efficiency-monitor 跨日志关联同一审计的完整决策链。
        tracing::debug!(
            safety_score = fused_score,
            intervention = ?intervention,
            risk_level = ?risk_level,
            keyword_count = keyword_count,
            ppo_used = ppo_used,
            "ASA 审计完成"
        );

        // 生成审计原因
        let audit_reason =
            format_audit_reason(intervention, keyword_count, history_rate, fused_score);

        // 仅在 intervention != Allow 时发布(避免事件风暴)
        // WHY publish_blocking:audit() 是同步方法,不能 await。
        // EventBus::publish_blocking 是 event-bus 官方同步 API(内部 broadcast::send 非阻塞),
        // 专为不便 await 的同步场景设计,零运行时依赖,事件立即投递不丢失
        if intervention != InterventionAction::Allow {
            let event = NexusEvent::AsaIntervention {
                metadata: EventMetadata::new("seccore"),
                operation_id: input.operation_id.clone(),
                action: format!("{:?}", intervention),
                safety_score: fused_score,
                block_reason: (intervention == InterventionAction::Block)
                    .then(|| audit_reason.clone()),
                alternative_suggestion: None,
            };
            if let Err(e) = self.event_bus.publish_blocking(event) {
                warn!(error = %e, "发布 AsaIntervention 事件失败");
            }
        }

        AuditResult {
            safety_score: fused_score,
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
    ///
    /// # P1-W4.1 tracing 贯穿观测
    /// 顶层 span 携带 `operation_id` 与 `intervention` 字段。`intervention` 在
    /// `audit()` 返回后才能确定,通过 `Span::current().record()` 填充。
    /// 此 span 是 `audit()` span 的父级,形成 audit → audit_and_intervene
    /// 的 tracing 父子链,便于 efficiency-monitor 追溯完整 ASA 决策路径。
    #[tracing::instrument(
        skip(self, input),
        fields(
            operation_id = %input.operation_id,
            intervention
        )
    )]
    pub fn audit_and_intervene(
        &self,
        input: &OperationAuditInput,
    ) -> Result<AuditResult, SecCoreError> {
        let result = self.audit(input);

        // P1-W4.1: 填充 instrument span 的延迟字段(intervention 在 audit() 后才能确定)
        // WHY tracing::field::debug:result.intervention 是自定义 enum,用 debug() 包装
        tracing::Span::current()
            .record("intervention", tracing::field::debug(&result.intervention));

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
                    intervention = ?result.intervention,
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
                    intervention = ?result.intervention,
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

    /// 记录操作成功 — 更新历史(反馈闭环),同时训练 PPO 模型。
    ///
    /// P3-3:成功时训练 PPO 的目标 Q 值为 `[0.9, 0.1, 0.0]`(高 Allow,低 Block),
    /// 使用默认状态(低风险特征)。
    pub fn record_success(&self) {
        let (history_rate, keyword_count, complexity) = {
            let mut history = self
                .history
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            history.total_count += 1;
            let rate = history.failure_rate();
            // 记录成功时使用默认状态:低风险(0 关键字,中复杂度)
            (rate, 0usize, 0.3f32)
        };

        // P3-3: 训练 PPO 模型(如果可用)
        if let Some(ref critic_lock) = self.ppocritic {
            if let Ok(mut critic) = critic_lock.write() {
                let state = build_ppo_state(keyword_count, history_rate, complexity);
                let target = [0.9, 0.1, 0.0]; // 高 Allow,低 Warn,低 Block
                critic.train(&state, &target);
            }
        }
    }

    /// 记录操作失败 — 更新历史(反馈闭环),同时训练 PPO 模型。
    ///
    /// P3-3:失败时训练 PPO 的目标 Q 值为 `[0.0, 0.3, 0.9]`(低 Allow,高 Block),
    /// 使用默认状态(根据历史失败率估计风险特征)。
    pub fn record_failure(&self, operation_id: &str) {
        let (history_rate, keyword_count, complexity) = {
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
            let rate = history.failure_rate();
            // 记录失败时使用中等风险状态
            (rate, 2usize, 0.6f32)
        };

        // P3-3: 训练 PPO 模型(如果可用)
        if let Some(ref critic_lock) = self.ppocritic {
            if let Ok(mut critic) = critic_lock.write() {
                let state = build_ppo_state(keyword_count, history_rate, complexity);
                let target = [0.0, 0.3, 0.9]; // 低 Allow,中 Warn,高 Block
                critic.train(&state, &target);
            }
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
}

/// 计算正确性分数 — 基于括号匹配的简单语法检查(Week 5 占位)。
///
/// 检查 () [] {} 是否匹配。匹配返回 0.9,不匹配返回 0.3。
/// DEFERRED(T8-3 Audit): 替换为基于 PVL Verifier 的语法检查需要 PVL 语法定义文件,
/// 当前括号匹配已覆盖常见语法错误场景,满足 Week 5 安全审计需求。
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
    safety_score: f32,
) -> String {
    let action_str = match intervention {
        InterventionAction::Allow => "Allow",
        InterventionAction::Warn => "Warn",
        InterventionAction::Block => "Block",
    };
    format!(
        "{}: 安全分数 {:.3}(关键字 {} 个, 历史失败率 {:.3})",
        action_str, safety_score, keyword_count, history_rate
    )
}

/// 构建 PPO 状态向量 — 将审计输入编码为 4 维状态
///
/// 状态向量: `(keyword_count_norm, history_failure_rate, complexity_score, operation_type_embedding)`
///
/// - `keyword_count_norm`: 关键字数归一化到 [0, 1](除以 10,上限 1.0)
/// - `history_failure_rate`: 历史失败率,直接使用
/// - `complexity_score`: 操作复杂度,直接使用
/// - `operation_type_embedding`: 操作类型嵌入,默认 0.5(通用)
fn build_ppo_state(keyword_count: usize, history_rate: f32, complexity: f32) -> [f32; 4] {
    [
        (keyword_count as f32 / 10.0).min(1.0),
        history_rate.clamp(0.0, 1.0),
        complexity.clamp(0.0, 1.0),
        0.5, // 操作类型嵌入:默认通用
    ]
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
