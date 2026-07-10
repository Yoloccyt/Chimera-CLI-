//! 事件主题分类 — 9 类 EventTopic 用于 FilteredSubscriber 选择性订阅
//!
//! 对应架构层：L1 Core
//! 设计决策（2026-07-09）：采用 9 类分类方案，架构纯净度优先
//!
//! # 9 类分类理据
//! 按十层架构的功能域划分，每个 topic 对应一组职责相关的 NexusEvent 变体。
//! FilteredSubscriber 订阅指定 topic 集合后，仅接收匹配事件，避免无关事件
//! 占用消费者缓冲区。既有 `subscribe()` 保持全量广播，向后兼容。
//!
//! # 与 recv_matching 的区别
//! - `recv_matching(FnMut)`：基于谓词的临时过滤，每次调用都要传闭包
//! - `FilteredSubscriber`：基于 topic 集合的订阅级过滤，构造时确定，
//!   后续 recv 自动跳过不匹配事件，更适合长期订阅场景

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::types::NexusEvent;

/// 事件主题 — 9 类分类覆盖全部 67 个 NexusEvent 变体
///
/// WHY 9 类分类：按架构层职责划分，每个 topic 对应一个功能域。
/// FilteredSubscriber 订阅指定 topic 集合，仅接收匹配事件，
/// 避免无关事件占用消费者缓冲区。
///
/// # 设计权衡（2026-07-09）
/// - 方案 A（细粒度 67 类）：每变体一个 topic，过细，FilterSubscriber 失去意义
/// - 方案 B（9 类，采用）：架构纯净度优先，每个 topic 对应一个功能域
/// - 方案 C（按 severity 分 2 类）：粒度过粗，无法支撑 N9 PrerequisiteChecker
///   等只需 Routing 事件的场景
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventTopic {
    /// 路由层事件 (L6 Router)：OSA/KVBSR/FaaE/SESA/GEA 路由与激活
    Routing,
    /// 记忆层事件 (L2 Memory)：NMC/MLC/HCW/CMT 记忆编码与分层
    Memory,
    /// 安全事件 (L4 Security)：SecCore/Decay/ASA/AHIRT 安全审计与干预
    Security,
    /// 执行层事件 (L7 Execution)：PVL/MTPE/SSRA 生产验证与融合
    Execution,
    /// 议会事件 (L8 Parliament)：投票/共识/预算治理
    Parliament,
    /// Quest 生命周期 (L9 Quest)：意图编码/任务分解/检查点
    Quest,
    /// 系统级事件 (L10 Interface + 跨层)：MCP Mesh/CSN/CHTC/监控告警
    System,
    /// 知识层事件 (L5 Knowledge)：Wiki/GSOE/AutoDPO 知识沉淀与进化
    Knowledge,
    /// 存储层事件 (L3 Storage)：SCC/LSCT 缓存与分层
    Storage,
}

impl EventTopic {
    /// 返回全部 9 个 topic 的 HashSet，用于"订阅全部"场景
    ///
    /// WHY 用 HashSet 而非 Vec：FilteredSubscriber 的 topics 字段需要 O(1) 查找，
    /// HashSet 满足此需求；Vec 虽然构造简单但每次 contains 是 O(n)。
    pub fn all() -> HashSet<EventTopic> {
        [
            EventTopic::Routing,
            EventTopic::Memory,
            EventTopic::Security,
            EventTopic::Execution,
            EventTopic::Parliament,
            EventTopic::Quest,
            EventTopic::System,
            EventTopic::Knowledge,
            EventTopic::Storage,
        ]
        .into_iter()
        .collect()
    }
}

impl NexusEvent {
    /// 获取事件所属主题
    ///
    /// 67 个变体映射到 9 类 EventTopic。
    /// WHY 用 match 而非 HashMap：编译期穷尽性检查，新增变体时编译器强制更新映射，
    /// 避免遗漏导致 topic() panic。
    pub fn topic(&self) -> EventTopic {
        match self {
            // === Routing (11) === L6 Router 路由与激活
            Self::OmniSparseMasksComputed { .. }
            | Self::ToolsRouted { .. }
            | Self::BlocksRebalanced { .. }
            | Self::ExpertActivated { .. }
            | Self::ActivationThresholdAdjusted { .. }
            | Self::ActivationCacheStats { .. }
            | Self::ExpertRouted { .. }
            | Self::ExpertRegistered { .. }
            | Self::ExpertUnregistered { .. }
            | Self::EntropyBalanced { .. }
            | Self::SesaActivationCompleted { .. } => EventTopic::Routing,

            // === Memory (7) === L2 Memory 记忆编码与分层
            Self::NexusStateChanged { .. }
            | Self::MemoryMetricsReported { .. }
            | Self::MemoryTiered { .. }
            | Self::ContextWindowSwitched { .. }
            | Self::ContextCompressed { .. }
            | Self::CapabilityTiered { .. }
            | Self::NmcEncoded { .. } => EventTopic::Memory,

            // === Security (8) === L4 Security 安全审计与干预
            Self::CapabilityFrozen { .. }
            | Self::SandboxViolation { .. }
            | Self::AuditLogged { .. }
            | Self::SkepticVeto { .. }
            | Self::VetoOverridden { .. }
            | Self::RedTeamAudit { .. }
            | Self::AsaIntervention { .. }
            | Self::AhirtProbeCompleted { .. } => EventTopic::Security,

            // === Execution (12) === L7 Execution 生产验证与融合
            Self::OperationProduced { .. }
            | Self::PredictionVerified { .. }
            | Self::ExecutionCompleted { .. }
            | Self::GatherCompleted { .. }
            | Self::OperationTimedOut { .. }
            | Self::GatherTimedOut { .. }
            | Self::OrphanCallDetected { .. }
            | Self::ProducerStrategyAdjusted { .. }
            | Self::PredictionMade { .. }
            | Self::PredictionStatsReported { .. }
            | Self::PredictionRolledBack { .. }
            | Self::SsraFusionCompleted { .. } => EventTopic::Execution,

            // === Parliament (7) === L8 Parliament 投票/共识/预算
            Self::ConsensusReached { .. }
            | Self::VoteCast { .. }
            | Self::DebateStarted { .. }
            | Self::RoleRegistered { .. }
            | Self::BudgetAdjusted { .. }
            | Self::BudgetStatsReported { .. }
            | Self::BudgetExceeded { .. } => EventTopic::Parliament,

            // === Quest (7) === L9 Quest 意图/任务/检查点
            Self::UserIntentEncoded { .. }
            | Self::QuestCreated { .. }
            | Self::QuestProgressUpdated { .. }
            | Self::ThinkingModeSwitched { .. }
            | Self::CheckpointSaved { .. }
            | Self::CheckpointLoaded { .. }
            | Self::ModelRouteSelected { .. } => EventTopic::Quest,

            // === System (6) === L10 Interface + 跨层系统告警
            Self::McpMessageReceived { .. }
            | Self::SlowConsumerDropped { .. }
            | Self::ChtcToolCallReceived { .. }
            | Self::McpMeshTransactionCompleted { .. }
            | Self::CsnSubstitutionTriggered { .. }
            | Self::EfficiencyAlertTriggered { .. }
            // Task 11 (N16): ChainExhausted 属于 System 主题
            // WHY:CSN 降级链耗尽是 L10 Interface 层的系统级告警事件,
            // 与 CsnSubstitutionTriggered 同属 CSN 能力替代域,归为 System
            | Self::ChainExhausted { .. } => EventTopic::System,

            // === Knowledge (4) === L5 Knowledge 知识沉淀与进化
            Self::WikiUpdated { .. }
            | Self::EvolutionTriggered { .. }
            | Self::DpoPairGenerated { .. }
            | Self::GsoePolicyUpdated { .. } => EventTopic::Knowledge,

            // === Storage (5) === L3 Storage 缓存与分层
            Self::CacheHit { .. }
            | Self::CacheMiss { .. }
            | Self::CachePrefetched { .. }
            | Self::CacheStatsReported { .. }
            | Self::LsctTierSwitched { .. } => EventTopic::Storage,
        }
    }
}

/// 事件主题过滤订阅者 — 仅接收指定 topic 集合的事件
///
/// 包装 `EventReceiver`，内部跳过不匹配 topic 的事件。
/// 不匹配的事件从接收缓冲区移除（消费但不返回），与 `recv_matching` 语义一致。
///
/// # 使用场景
/// - TTG 仲裁层只需 Parliament + Budget 事件，无需接收全部 67 类
/// - N9 PrerequisiteChecker 只需 Routing 事件
/// - 减少无关事件对消费者缓冲区的占用
///
/// # 向后兼容
/// 既有 `EventBus::subscribe()` 返回 `EventReceiver`（全量广播）不受影响。
/// FilteredSubscriber 是独立的类型，通过 `EventBus::subscribe_filtered()` 创建。
pub struct FilteredSubscriber {
    /// 内部接收者（复用 EventReceiver 的日志与背压能力）
    inner: crate::bus::EventReceiver,
    /// 订阅的 topic 集合
    topics: HashSet<EventTopic>,
}

impl FilteredSubscriber {
    /// 内部构造函数（crate 内可见，由 EventBus::subscribe_filtered 调用）
    ///
    /// WHY pub(crate)：避免外部直接拼装 FilteredSubscriber 绕过 EventBus 的
    /// 订阅者计数与日志埋点；同时允许 bus.rs 在同 crate 内构造。
    pub(crate) fn new(inner: crate::bus::EventReceiver, topics: HashSet<EventTopic>) -> Self {
        Self { inner, topics }
    }

    /// 接收下一个匹配 topic 的事件
    ///
    /// 内部循环调用 `inner.recv()`，跳过 topic 不匹配的事件。
    /// 不匹配的事件被消费（从缓冲区移除），确保缓冲区不被无关事件占满。
    ///
    /// # 错误
    /// 透传 `EventReceiver::recv()` 的错误（ChannelClosed / SlowConsumerDropped）。
    pub async fn recv(&mut self) -> Result<NexusEvent, crate::error::EventBusError> {
        loop {
            let event = self.inner.recv().await?;
            if self.topics.contains(&event.topic()) {
                return Ok(event);
            }
            // 不匹配的事件被消费并丢弃
        }
    }

    /// 带超时的接收
    ///
    /// 透传 `EventReceiver::recv_timeout()` 的超时与错误处理。
    pub async fn recv_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<NexusEvent, crate::error::EventBusError> {
        loop {
            let event = self.inner.recv_timeout(timeout).await?;
            if self.topics.contains(&event.topic()) {
                return Ok(event);
            }
        }
    }

    /// 尝试非阻塞接收
    ///
    /// 扫描当前缓冲区，返回第一个匹配 topic 的事件。
    /// 不匹配的事件被消费（从缓冲区移除），与 `try_recv_matching` 语义一致。
    ///
    /// # 返回值
    /// - `Ok(Some(event))`：找到匹配事件
    /// - `Ok(None)`：缓冲区为空（可能还有后续事件，但当前无可用）
    /// - `Err`：通道关闭或 lag 超限
    pub fn try_recv(&mut self) -> Result<Option<NexusEvent>, crate::error::EventBusError> {
        loop {
            match self.inner.try_recv()? {
                Some(event) if self.topics.contains(&event.topic()) => return Ok(Some(event)),
                Some(_) => continue, // 不匹配，消费并继续
                None => return Ok(None),
            }
        }
    }

    /// 获取订阅的 topic 集合
    pub fn topics(&self) -> &HashSet<EventTopic> {
        &self.topics
    }

    /// 获取订阅者标识（委托给内部 EventReceiver）
    pub fn subscriber_id(&self) -> &str {
        self.inner.subscriber_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::EventMetadata;

    #[test]
    fn test_event_topic_all_returns_nine_topics() {
        let all = EventTopic::all();
        assert_eq!(all.len(), 9, "EventTopic::all() 应返回 9 个 topic");
        // 验证每个 topic 都在集合内
        for topic in [
            EventTopic::Routing,
            EventTopic::Memory,
            EventTopic::Security,
            EventTopic::Execution,
            EventTopic::Parliament,
            EventTopic::Quest,
            EventTopic::System,
            EventTopic::Knowledge,
            EventTopic::Storage,
        ] {
            assert!(all.contains(&topic), "all() 应包含 {topic:?}");
        }
    }

    #[test]
    fn test_topic_mapping_routing() {
        let e = NexusEvent::ExpertRegistered {
            metadata: EventMetadata::new("test"),
            tool_id: "t-1".into(),
        };
        assert_eq!(e.topic(), EventTopic::Routing);
    }

    #[test]
    fn test_topic_mapping_memory() {
        let e = NexusEvent::NmcEncoded {
            metadata: EventMetadata::new("test"),
            modality: "Text".into(),
            content_hash: "h".into(),
            clv_dimension: 512,
        };
        assert_eq!(e.topic(), EventTopic::Memory);
    }

    #[test]
    fn test_topic_mapping_security() {
        let e = NexusEvent::SkepticVeto {
            metadata: EventMetadata::new("test"),
            quest_id: "q-1".into(),
            veto_reason: "test".into(),
            frozen_capabilities: vec![],
        };
        assert_eq!(e.topic(), EventTopic::Security);
    }

    #[test]
    fn test_topic_mapping_execution() {
        let e = NexusEvent::PredictionMade {
            metadata: EventMetadata::new("test"),
            quest_id: "q-1".into(),
            n: 3,
            avg_confidence: 0.85,
        };
        assert_eq!(e.topic(), EventTopic::Execution);
    }

    #[test]
    fn test_topic_mapping_parliament() {
        let e = NexusEvent::VoteCast {
            metadata: EventMetadata::new("test"),
            proposal_id: "p-1".into(),
            voter: "v-1".into(),
            vote: true,
        };
        assert_eq!(e.topic(), EventTopic::Parliament);
    }

    #[test]
    fn test_topic_mapping_quest() {
        let e = NexusEvent::QuestCreated {
            metadata: EventMetadata::new("test"),
            quest_id: "q-1".into(),
            title: "t".into(),
            task_count: 1,
        };
        assert_eq!(e.topic(), EventTopic::Quest);
    }

    #[test]
    fn test_topic_mapping_system() {
        let e = NexusEvent::SlowConsumerDropped {
            metadata: EventMetadata::new("test"),
            subscriber_id: "s-1".into(),
            lag: 10,
            dropped_count: 5,
        };
        assert_eq!(e.topic(), EventTopic::System);
    }

    #[test]
    fn test_topic_mapping_knowledge() {
        let e = NexusEvent::WikiUpdated {
            metadata: EventMetadata::new("test"),
            wiki_hash: "h".into(),
            delta: 5,
        };
        assert_eq!(e.topic(), EventTopic::Knowledge);
    }

    #[test]
    fn test_topic_mapping_storage() {
        let e = NexusEvent::CacheHit {
            metadata: EventMetadata::new("test"),
            cache_key: "k-1".into(),
        };
        assert_eq!(e.topic(), EventTopic::Storage);
    }

    #[test]
    fn test_topic_mapping_chain_exhausted() {
        let e = NexusEvent::ChainExhausted {
            metadata: EventMetadata::new("csn-substitutor"),
            chain_id: "cap-1".into(),
            last_error: "降级链已耗尽".into(),
        };
        assert_eq!(
            e.topic(),
            EventTopic::System,
            "ChainExhausted 属于 System 主题(CSN 降级链事件)"
        );
    }
}
