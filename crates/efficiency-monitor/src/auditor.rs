//! RuntimeAuditor — 运行时自我评估审计器(polish-v2.7 P1-1)
//!
//! 对应架构层:L9 Quest(efficiency-monitor 子模块)
//! 对应 ADR:ADR-049 决策 1(runtime-auditor 落点 efficiency-monitor)
//! 对应设计源:`chimera_ultimate_polish_v2.7.md` §14.2(Qoder Better Harness)
//!
//! # 核心职责
//!
//! 1. **证据纪律**(Qoder 核心洞察"静态发现 ≠ 已执行验证"):
//!    静态登记的能力必须有运行时事件证据才计为"已验证",否则产出
//!    `UnusedCapability` Finding 提示配置与实际行为脱节
//! 2. **五维度评估**:任务理解 / 可控执行 / 变更验证 / 可靠交付 / 经验沉淀,
//!    全部基于 NexusEvent 计数器的可实测启发式(无主观打分)
//! 3. **事件发布**:审计发现 → `AuditFindingRaised`,五维报告 → `HarnessReportGenerated`
//!
//! # 设计约束
//!
//! - **只读观察者**:仅消费事件计数,零执行路径侵入(ADR-049"零回归风险"档)
//! - **同步 API + publish_blocking**:遵循 §4.4 红线 8(sync 方法用 publish_blocking,
//!   避免 `#[test]` 无 runtime 时 tokio::spawn panic)
//! - **DashMap 计数**:热路径(record_event)无锁分片,不跨 await 持锁(§4.4 红线 1)
//!
//! # 数据流
//!
//! ```text
//! NexusEvent 流 ──record_event──▶ 计数器(type_name → count)
//! 能力调用点  ──record_capability_use──▶ 能力证据(capability → use count)
//! 静态配置    ──register_capability──▶ 待验证清单
//!                    │
//!                    ▼
//!        audit_capability / generate_report
//!                    │
//!                    ▼
//!   AuditFindingRaised / HarnessReportGenerated(EventBus)
//! ```

use dashmap::DashMap;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::warn;

// ============================================================
// 审计发现类型
// ============================================================

/// 审计发现严重度
///
/// WHY 独立枚举而非复用 `AlertSeverity`:审计发现是观察性分级
/// (info 也是有效发现——"能力已验证"),与告警的响应等级语义不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingSeverity {
    /// 信息级:正向发现(如能力已验证)
    Info,
    /// 低:轻微偏差,无需立即处理
    Low,
    /// 中:配置与运行时行为脱节(如未使用的能力)
    Medium,
    /// 高:证据缺口可能掩盖故障
    High,
}

impl FindingSeverity {
    /// 事件标签(与 `AuditFindingRaised.finding_severity` 字段约定一致)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// 审计发现类别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingCategory {
    /// 已配置但从未在运行时使用的能力(证据纪律违反)
    UnusedCapability,
    /// 已验证的能力(有运行时事件证据)
    VerifiedCapability,
    /// 证据缺口:审计对象未登记,无法评估
    EvidenceGap,
}

impl FindingCategory {
    /// 事件标签(与 `AuditFindingRaised.category` 字段约定一致)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UnusedCapability => "unused_capability",
            Self::VerifiedCapability => "verified_capability",
            Self::EvidenceGap => "evidence_gap",
        }
    }
}

/// 证据种类 — Qoder 证据纪律的核心区分
///
/// 只有 `RuntimeEvents` 才计为"已验证"正证据;`StaticOnly` 表示
/// 仅有静态配置声明,运行时从未观察到实际使用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceKind {
    /// 仅静态配置,无运行时证据
    StaticOnly,
    /// 有运行时事件证据(携带观察到的使用次数)
    RuntimeEvents(u64),
}

impl EvidenceKind {
    /// 事件标签(与 `AuditFindingRaised.evidence_kind` 字段约定一致)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::StaticOnly => "static_only",
            Self::RuntimeEvents(_) => "runtime_events",
        }
    }
}

/// 单条审计发现
#[derive(Debug, Clone)]
pub struct Finding {
    /// 严重度
    pub severity: FindingSeverity,
    /// 类别
    pub category: FindingCategory,
    /// 人类可读描述
    pub message: String,
    /// 证据种类
    pub evidence: EvidenceKind,
    /// 修复建议(无需动作时为描述性文本)
    pub fix_hint: String,
}

/// 五维度 Harness 报告(Qoder Better Harness 评估体系)
///
/// 所有维度取值 [0.0, 1.0]。分母为零(无观测数据)时该维度取中性值 0.5:
/// 无证据既不给满分也不给零分,呼应证据纪律。
#[derive(Debug, Clone)]
pub struct HarnessReport {
    /// 任务理解:意图 → 任务的转化率(QuestCreated / UserIntentEncoded)
    pub task_comprehension: f32,
    /// 可控执行:1 − 失控事件率(超时/孤儿调用/沙箱违规 vs 完成数)
    pub controllable_execution: f32,
    /// 变更验证:验证通过率(PredictionVerified / OperationProduced)
    pub change_verification: f32,
    /// 可靠交付:任务完成率(QuestCompleted / QuestCreated)
    pub reliable_delivery: f32,
    /// 经验沉淀:知识/检查点沉淀率((WikiUpdated + CheckpointSaved) / QuestCompleted)
    pub experience_accumulation: f32,
    /// 本次报告的审计发现集合
    pub findings: Vec<Finding>,
}

// ============================================================
// RuntimeAuditor
// ============================================================

/// 分母为零时的中性评分 — 无观测数据既不给满分也不给零分
const NEUTRAL_SCORE: f32 = 0.5;

/// 运行时自我评估审计器
///
/// # 线程安全
/// 全部字段为 DashMap(无锁分片)或 clone 廉价的 EventBus,
/// `RuntimeAuditor` 满足 `Send + Sync`,可 `Arc` 共享跨任务使用。
pub struct RuntimeAuditor {
    /// 事件类型计数(type_name → 观察次数)— record_event 热路径
    event_counts: DashMap<&'static str, u64>,
    /// 能力运行时使用证据(capability → 使用次数)
    capability_uses: DashMap<String, u64>,
    /// 静态登记的待验证能力清单
    registered_capabilities: DashMap<String, ()>,
    /// 可选事件总线(绑定后 Finding/Report 自动发布)
    event_bus: Option<EventBus>,
}

impl RuntimeAuditor {
    /// 创建独立审计器(不绑定 EventBus,发现仅返回不发布)
    pub fn new() -> Self {
        Self {
            event_counts: DashMap::new(),
            capability_uses: DashMap::new(),
            registered_capabilities: DashMap::new(),
            event_bus: None,
        }
    }

    /// 创建绑定 EventBus 的审计器(Finding/Report 自动发布)
    pub fn with_event_bus(bus: EventBus) -> Self {
        Self {
            event_bus: Some(bus),
            ..Self::new()
        }
    }

    /// 同步记录一个事件 — 按 type_name 累积计数
    ///
    /// # 性能
    /// DashMap 分片计数 ~20ns,适合在事件订阅循环中逐条调用。
    pub fn record_event(&self, event: &NexusEvent) {
        *self.event_counts.entry(event.type_name()).or_insert(0) += 1;
    }

    /// 登记静态配置的能力(待运行时验证)
    ///
    /// 通常在启动阶段从配置文件遍历登记,之后由 `audit_capability`
    /// 检查是否有运行时使用证据。
    pub fn register_capability(&self, name: impl Into<String>) {
        self.registered_capabilities.insert(name.into(), ());
    }

    /// 记录一次能力的运行时使用(证据埋点)
    ///
    /// 调用方在能力实际执行点埋点。WHY 显式埋点而非从事件字段推断:
    /// NexusEvent 各变体的能力字段命名不统一,推断易产生误报,
    /// 显式埋点保证证据的确定性(证据纪律要求零猜测)。
    pub fn record_capability_use(&self, name: impl Into<String>) {
        *self.capability_uses.entry(name.into()).or_insert(0) += 1;
    }

    /// 审计单个能力:静态配置是否真正生效
    ///
    /// # 判定规则(证据纪律)
    /// - 未登记 → `EvidenceGap`(High):审计对象不在配置清单,无法评估
    /// - 已登记且使用数 = 0 → `UnusedCapability`(Medium):配置了但从未使用
    /// - 已登记且使用数 > 0 → `VerifiedCapability`(Info):运行时证据确认生效
    pub fn audit_capability(&self, capability: &str) -> Finding {
        let finding = if !self.registered_capabilities.contains_key(capability) {
            Finding {
                severity: FindingSeverity::High,
                category: FindingCategory::EvidenceGap,
                message: format!("能力 '{capability}' 未登记,无法评估其配置有效性"),
                evidence: EvidenceKind::StaticOnly,
                fix_hint: format!("先调用 register_capability(\"{capability}\") 登记"),
            }
        } else {
            let uses = self
                .capability_uses
                .get(capability)
                .map(|c| *c.value())
                .unwrap_or(0);
            if uses == 0 {
                Finding {
                    severity: FindingSeverity::Medium,
                    category: FindingCategory::UnusedCapability,
                    message: format!("能力 '{capability}' 已配置但运行时从未使用"),
                    evidence: EvidenceKind::StaticOnly,
                    fix_hint: format!("移除 '{capability}' 配置,或排查其调用链路为何未触达"),
                }
            } else {
                Finding {
                    severity: FindingSeverity::Info,
                    category: FindingCategory::VerifiedCapability,
                    message: format!("能力 '{capability}' 已验证:运行时使用 {uses} 次"),
                    evidence: EvidenceKind::RuntimeEvents(uses),
                    fix_hint: "无需动作".to_string(),
                }
            }
        };
        self.publish_finding(&finding);
        finding
    }

    /// 审计全部已登记能力
    pub fn audit_all_capabilities(&self) -> Vec<Finding> {
        // 先取快照再逐个审计,避免遍历时持有 DashMap 分片引用跨发布调用
        let names: Vec<String> = self
            .registered_capabilities
            .iter()
            .map(|e| e.key().clone())
            .collect();
        names.iter().map(|n| self.audit_capability(n)).collect()
    }

    /// 生成五维度 Harness 报告并发布 `HarnessReportGenerated`
    ///
    /// 五维度全部为事件计数启发式(可实测、可复现),findings 为全量能力审计结果。
    pub fn generate_report(&self) -> HarnessReport {
        let count = |name: &str| -> f32 {
            self.event_counts
                .get(name)
                .map(|c| *c.value() as f32)
                .unwrap_or(0.0)
        };

        // 任务理解:意图 → 任务的转化率
        let intents = count("UserIntentEncoded");
        let quests = count("QuestCreated");
        let task_comprehension = ratio_or_neutral(quests, intents);

        // 可控执行:1 − 失控事件率(超时 + 孤儿调用 + 沙箱违规)
        let completed = count("ExecutionCompleted");
        let uncontrolled =
            count("OperationTimedOut") + count("OrphanCallDetected") + count("SandboxViolation");
        let controllable_execution = if completed == 0.0 {
            NEUTRAL_SCORE
        } else {
            (1.0 - uncontrolled / completed).clamp(0.0, 1.0)
        };

        // 变更验证:验证通过率
        let change_verification =
            ratio_or_neutral(count("PredictionVerified"), count("OperationProduced"));

        // 可靠交付:任务完成率
        let reliable_delivery = ratio_or_neutral(count("QuestCompleted"), quests);

        // 经验沉淀:知识/检查点沉淀率
        let experience_accumulation = ratio_or_neutral(
            count("WikiUpdated") + count("CheckpointSaved"),
            count("QuestCompleted"),
        );

        let findings = self.audit_all_capabilities();

        let report = HarnessReport {
            task_comprehension,
            controllable_execution,
            change_verification,
            reliable_delivery,
            experience_accumulation,
            findings,
        };
        self.publish_report(&report);
        report
    }

    /// 发布单条 Finding(未绑定 EventBus 时静默跳过)
    ///
    /// WHY publish_blocking:本方法为 sync,遵循 §4.4 红线 8
    /// (sync 方法用 publish_blocking,async 方法才用 publish().await)。
    fn publish_finding(&self, finding: &Finding) {
        if let Some(bus) = &self.event_bus {
            let event = NexusEvent::AuditFindingRaised {
                metadata: EventMetadata::new("efficiency-monitor"),
                finding_severity: finding.severity.as_str().to_string(),
                category: finding.category.as_str().to_string(),
                message: finding.message.clone(),
                evidence_kind: finding.evidence.as_str().to_string(),
                fix_hint: finding.fix_hint.clone(),
            };
            // 发布失败仅告警不上抛:审计是观察者,不应因总线故障中断审计流程
            if let Err(e) = bus.publish_blocking(event) {
                warn!(error = %e, "AuditFindingRaised 发布失败");
            }
        }
    }

    /// 发布五维报告(未绑定 EventBus 时静默跳过)
    fn publish_report(&self, report: &HarnessReport) {
        if let Some(bus) = &self.event_bus {
            let event = NexusEvent::HarnessReportGenerated {
                metadata: EventMetadata::new("efficiency-monitor"),
                task_comprehension: report.task_comprehension,
                controllable_execution: report.controllable_execution,
                change_verification: report.change_verification,
                reliable_delivery: report.reliable_delivery,
                experience_accumulation: report.experience_accumulation,
                findings_count: report.findings.len() as u32,
            };
            if let Err(e) = bus.publish_blocking(event) {
                warn!(error = %e, "HarnessReportGenerated 发布失败");
            }
        }
    }
}

impl Default for RuntimeAuditor {
    fn default() -> Self {
        Self::new()
    }
}

/// 比率评分:`numerator / denominator`,cap 到 1.0;分母为零返回中性 0.5
///
/// WHY cap 1.0:计数器语义下分子可能大于分母(如一个 Quest 多次 CheckpointSaved),
/// 评分维度语义上限为"完全达成"。
fn ratio_or_neutral(numerator: f32, denominator: f32) -> f32 {
    if denominator == 0.0 {
        NEUTRAL_SCORE
    } else {
        (numerator / denominator).min(1.0)
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::{EventMetadata, QuestStatus};

    fn quest_created(id: &str) -> NexusEvent {
        NexusEvent::QuestCreated {
            metadata: EventMetadata::new("test"),
            quest_id: id.into(),
            title: "test quest".into(),
            task_count: 1,
        }
    }

    fn quest_completed(id: &str) -> NexusEvent {
        NexusEvent::QuestCompleted {
            metadata: EventMetadata::new("test"),
            quest_id: id.into(),
            status: QuestStatus::Completed,
        }
    }

    // --- 证据纪律:audit_capability 三分支 ---

    #[test]
    fn test_audit_unregistered_capability_is_evidence_gap() {
        let auditor = RuntimeAuditor::new();
        let finding = auditor.audit_capability("ghost-cap");
        assert_eq!(finding.category, FindingCategory::EvidenceGap);
        assert_eq!(finding.severity, FindingSeverity::High);
        assert_eq!(finding.evidence, EvidenceKind::StaticOnly);
    }

    #[test]
    fn test_audit_registered_but_unused_capability() {
        let auditor = RuntimeAuditor::new();
        auditor.register_capability("configured-cap");
        let finding = auditor.audit_capability("configured-cap");
        assert_eq!(finding.category, FindingCategory::UnusedCapability);
        assert_eq!(finding.severity, FindingSeverity::Medium);
        // 证据纪律核心断言:静态配置 ≠ 已验证
        assert_eq!(finding.evidence, EvidenceKind::StaticOnly);
        assert!(finding.message.contains("configured-cap"));
    }

    #[test]
    fn test_audit_verified_capability_has_runtime_evidence() {
        let auditor = RuntimeAuditor::new();
        auditor.register_capability("used-cap");
        auditor.record_capability_use("used-cap");
        auditor.record_capability_use("used-cap");
        let finding = auditor.audit_capability("used-cap");
        assert_eq!(finding.category, FindingCategory::VerifiedCapability);
        assert_eq!(finding.severity, FindingSeverity::Info);
        assert_eq!(finding.evidence, EvidenceKind::RuntimeEvents(2));
    }

    #[test]
    fn test_audit_all_capabilities_covers_registered_set() {
        let auditor = RuntimeAuditor::new();
        auditor.register_capability("cap-a");
        auditor.register_capability("cap-b");
        auditor.record_capability_use("cap-a");
        let findings = auditor.audit_all_capabilities();
        assert_eq!(findings.len(), 2);
        let verified = findings
            .iter()
            .filter(|f| f.category == FindingCategory::VerifiedCapability)
            .count();
        let unused = findings
            .iter()
            .filter(|f| f.category == FindingCategory::UnusedCapability)
            .count();
        assert_eq!((verified, unused), (1, 1));
    }

    // --- 五维度评分 ---

    #[test]
    fn test_report_neutral_when_no_events() {
        let auditor = RuntimeAuditor::new();
        let report = auditor.generate_report();
        // 无观测数据 → 全维度中性 0.5(不给满分不给零分)
        assert_eq!(report.task_comprehension, NEUTRAL_SCORE);
        assert_eq!(report.controllable_execution, NEUTRAL_SCORE);
        assert_eq!(report.change_verification, NEUTRAL_SCORE);
        assert_eq!(report.reliable_delivery, NEUTRAL_SCORE);
        assert_eq!(report.experience_accumulation, NEUTRAL_SCORE);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn test_report_reliable_delivery_ratio() {
        let auditor = RuntimeAuditor::new();
        // 4 个 Quest 创建,1 个完成 → 交付率 0.25
        for i in 0..4 {
            auditor.record_event(&quest_created(&format!("q{i}")));
        }
        auditor.record_event(&quest_completed("q0"));
        let report = auditor.generate_report();
        assert!((report.reliable_delivery - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn test_report_dimension_capped_at_one() {
        let auditor = RuntimeAuditor::new();
        auditor.record_event(&quest_created("q0"));
        // 1 个 Quest 完成 + 3 次 CheckpointSaved → 沉淀率 cap 到 1.0
        auditor.record_event(&quest_completed("q0"));
        for _ in 0..3 {
            auditor.record_event(&NexusEvent::CheckpointSaved {
                metadata: EventMetadata::new("test"),
                quest_id: "q0".into(),
                checkpoint_id: "c".into(),
                memory_snapshot_hash: "h".into(),
            });
        }
        let report = auditor.generate_report();
        assert_eq!(report.experience_accumulation, 1.0);
    }

    // --- 事件发布 ---

    #[test]
    fn test_finding_published_to_event_bus() {
        let bus = EventBus::new();
        // §4.4 红线 3:先 subscribe 再触发发布,否则事件静默丢失
        let mut rx = bus.subscribe();
        let auditor = RuntimeAuditor::with_event_bus(bus);
        auditor.register_capability("cap-x");
        auditor.audit_capability("cap-x");

        let event = rx
            .try_recv()
            .expect("try_recv 不应报错")
            .expect("应收到 AuditFindingRaised");
        match event {
            NexusEvent::AuditFindingRaised {
                finding_severity,
                category,
                evidence_kind,
                ..
            } => {
                assert_eq!(finding_severity, "medium");
                assert_eq!(category, "unused_capability");
                assert_eq!(evidence_kind, "static_only");
            }
            other => panic!("期望 AuditFindingRaised,收到 {}", other.type_name()),
        }
    }

    #[test]
    fn test_report_published_to_event_bus() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let auditor = RuntimeAuditor::with_event_bus(bus);
        let _ = auditor.generate_report();

        let event = rx
            .try_recv()
            .expect("try_recv 不应报错")
            .expect("应收到 HarnessReportGenerated");
        match event {
            NexusEvent::HarnessReportGenerated {
                task_comprehension,
                findings_count,
                ..
            } => {
                assert_eq!(task_comprehension, NEUTRAL_SCORE);
                assert_eq!(findings_count, 0);
            }
            other => panic!("期望 HarnessReportGenerated,收到 {}", other.type_name()),
        }
    }

    // --- 标签约定(与 event-bus 字段文档一致) ---

    #[test]
    fn test_label_conventions() {
        assert_eq!(FindingSeverity::Info.as_str(), "info");
        assert_eq!(FindingSeverity::High.as_str(), "high");
        assert_eq!(
            FindingCategory::UnusedCapability.as_str(),
            "unused_capability"
        );
        assert_eq!(FindingCategory::EvidenceGap.as_str(), "evidence_gap");
        assert_eq!(EvidenceKind::StaticOnly.as_str(), "static_only");
        assert_eq!(EvidenceKind::RuntimeEvents(1).as_str(), "runtime_events");
    }
}
