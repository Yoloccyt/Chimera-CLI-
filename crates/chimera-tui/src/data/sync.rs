//! 事件同步器 — 从 NexusEvent 流维护各面板的本地状态
//!
//! 每个同步器只处理特定的 NexusEvent 变体,职责单一,
//! 便于单元测试直接喂事件验证状态变化。`DataPipeline` 组合
//! 所有同步器生成统一 `DataSnapshot`。
//!
//! 对应架构层:L10 Interface

use std::collections::HashSet;

use event_bus::{ChatStatus, NexusEvent};
use nexus_core::Quest;

use super::snapshot::{
    AsaInterventionSummary, BudgetMetrics, HealthMetrics, MemoryMetrics, RedTeamAuditSummary,
    SecurityState, SkepticVetoSummary,
};
use crate::types::{ChatMessage, ChatRole};

/// Critical 旁路通道丢弃事件数指标名(P1-W2.2)
///
/// 该字符串与 `efficiency-monitor::CRITICAL_DROPPED_METRIC_NAME` 保持一致,
/// 用于识别 `EfficiencyAlertTriggered` 事件中代表 Critical 旁路通道丢弃计数的事件。
///
/// WHY 在 L10 重新定义而非依赖 L9:§2.2 依赖铁律禁止 L10 → L9 向上依赖,
/// efficiency-monitor 位于 L9,chimera-tui 不能直接 import 其常量。
/// efficiency-monitor 侧的 `CRITICAL_DROPPED_METRIC_NAME` 注释已明确指出
/// "TUI(L10)在 CriticalDroppedSync 中硬编码同一字符串识别事件"。
pub(crate) const CRITICAL_DROPPED_METRIC_NAME: &str = "nexus_critical_event_dropped_total";

/// Quest 同步器 — 从 NexusEvent 维护本地 Quest 列表与暂停状态
///
/// WHY 独立结构体:将事件→状态的转换逻辑隔离,`DataPipeline`(P1.3)
/// 可组合多个同步器生成统一快照,同时方便单元测试直接喂事件。
///
/// # 暂停状态跟踪
/// `Quest` 本身无 paused 字段(nexus-core 领域类型稳定性约束),因此
/// `QuestSync` 订阅已有的 `QuestPaused`/`QuestResumed` 事件维护
/// `paused_quest_ids` 集合。只跟踪 quest_list 中存在的 Quest ID,
/// 避免计数不在活动列表中的暂停 Quest。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct QuestSync {
    quests: Vec<Quest>,
    /// 暂停 Quest ID 集合(从 QuestPaused/QuestResumed 事件派生)
    paused_quest_ids: HashSet<String>,
}

impl QuestSync {
    /// 创建空的 Quest 同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件影响 Quest 列表则返回更新后的列表副本
    ///
    /// - `QuestListUpdated`:替换整个列表(冷启动/lag 后对齐)。暂停集合保留,
    ///   因为新列表中仍存在的暂停 Quest 应继续被计数。
    /// - `QuestCompleted`:按 quest_id 从活动列表移除,并从暂停集合清理。
    /// - `QuestCancelled`:按 quest_id 从活动列表移除,并从暂停集合清理。
    ///   与 `QuestCompleted` 对称,确保取消的 Quest 不残留暂停状态(内存泄漏防护)。
    /// - `QuestPriorityAdjusted`:按 quest_id 原地更新 priority 字段。
    ///   不影响其他状态(暂停集合、任务列表等),仅刷新优先级。
    /// - `QuestPaused`:若 quest_id 在活动列表中,加入暂停集合。
    /// - `QuestResumed`:从暂停集合移除。
    /// - 其他事件:返回 `None`,状态不变。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<Vec<Quest>> {
        match event {
            NexusEvent::QuestListUpdated { quests, .. } => {
                self.quests = quests.clone();
                Some(self.quests.clone())
            }
            NexusEvent::QuestCompleted { quest_id, .. } => {
                self.quests.retain(|q| q.quest_id != *quest_id);
                self.paused_quest_ids.remove(quest_id);
                Some(self.quests.clone())
            }
            NexusEvent::QuestCancelled { quest_id, .. } => {
                self.quests.retain(|q| q.quest_id != *quest_id);
                self.paused_quest_ids.remove(quest_id);
                Some(self.quests.clone())
            }
            NexusEvent::QuestPriorityAdjusted {
                quest_id,
                new_priority,
                ..
            } => {
                if let Some(quest) = self.quests.iter_mut().find(|q| q.quest_id == *quest_id) {
                    quest.priority = *new_priority;
                    Some(self.quests.clone())
                } else {
                    None
                }
            }
            NexusEvent::QuestPaused { quest_id, .. } => {
                if self.quests.iter().any(|q| q.quest_id == *quest_id) {
                    self.paused_quest_ids.insert(quest_id.clone());
                }
                None
            }
            NexusEvent::QuestResumed { quest_id, .. } => {
                self.paused_quest_ids.remove(quest_id);
                None
            }
            _ => None,
        }
    }

    /// 获取当前活动 Quest 列表副本
    pub fn quests(&self) -> Vec<Quest> {
        self.quests.clone()
    }

    /// 获取当前暂停 Quest 数(quest_list 中同时处于暂停状态的 Quest 数量)
    ///
    /// WHY 交叉过滤:只统计 quest_list 中存在的暂停 Quest,确保暂停 Quest 数
    /// 不会因 quest_list 更新(如 QuestCompleted 移除)而虚高。
    pub fn paused_quest_count(&self) -> usize {
        self.quests
            .iter()
            .filter(|q| self.paused_quest_ids.contains(&q.quest_id))
            .count()
    }
}

/// Budget 同步器 — 从 NexusEvent 维护本地 BudgetMetrics
///
/// WHY 独立结构体:与 `QuestSync` 对称,将事件→指标的转换隔离,
/// 由 `BudgetMetricsUpdated` 直接填充面板视图,无需拼合多个事件。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct BudgetSync {
    metrics: BudgetMetrics,
}

impl BudgetSync {
    /// 创建空的 Budget 同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件影响预算指标则返回更新后的指标副本
    ///
    /// - `BudgetMetricsUpdated`:直接替换本地指标。
    /// - 其他事件:返回 `None`,状态不变。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<BudgetMetrics> {
        match event {
            NexusEvent::BudgetMetricsUpdated { metrics, .. } => {
                self.metrics = BudgetMetrics {
                    total_consumption: metrics.total_consumption,
                    remaining_budget: metrics.remaining_budget,
                    utilization_rate: metrics.utilization_rate,
                    current_tier: metrics.current_tier.clone(),
                    coefficient: metrics.coefficient,
                    is_exceeded: metrics.is_exceeded,
                    alert: metrics.alert.clone(),
                };
                Some(self.metrics.clone())
            }
            _ => None,
        }
    }

    /// 获取当前预算指标副本
    pub fn metrics(&self) -> BudgetMetrics {
        self.metrics.clone()
    }
}

/// 记忆同步器 — 从 NexusEvent 维护本地 MemoryMetrics
///
/// WHY 独立结构体:与 `BudgetSync` 对称,将 L2/L3 事件→面板指标的转换隔离。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MemorySync {
    metrics: MemoryMetrics,
}

impl MemorySync {
    /// 创建空的 Memory 同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件影响记忆指标则返回更新后的指标副本
    ///
    /// 处理的事件:
    /// - `MemoryMetricsReported`:命中率、驱逐数。
    /// - `ContextWindowSwitched`:当前层级(`to_tier`)。
    /// - `ContextCompressed`:上下文窗口大小与压缩率。
    /// - `CacheStatsReported`:命中率与驱逐数(备选来源)。
    /// - `CacheHit` / `CacheMiss`:累计命中/未命中计数。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<MemoryMetrics> {
        let changed = match event {
            NexusEvent::MemoryMetricsReported {
                hit_rate,
                evictions,
                ..
            } => {
                self.metrics.hit_rate_percent = hit_rate * 100.0;
                self.metrics.evictions = *evictions;
                true
            }
            NexusEvent::ContextWindowSwitched { to_tier, .. } => {
                self.metrics.tier = to_tier.clone();
                true
            }
            NexusEvent::ContextCompressed {
                original_size,
                ratio,
                ..
            } => {
                self.metrics.context_window_size = *original_size;
                self.metrics.compressed_ratio = *ratio;
                true
            }
            NexusEvent::CacheStatsReported {
                hit_rate,
                eviction_count,
                ..
            } => {
                self.metrics.hit_rate_percent = hit_rate * 100.0;
                self.metrics.evictions = *eviction_count;
                true
            }
            NexusEvent::CacheHit { .. } => {
                self.metrics.cache_hits += 1;
                true
            }
            NexusEvent::CacheMiss { .. } => {
                self.metrics.cache_misses += 1;
                true
            }
            _ => false,
        };

        if changed {
            Some(self.metrics.clone())
        } else {
            None
        }
    }

    /// 获取当前记忆指标副本
    pub fn metrics(&self) -> MemoryMetrics {
        self.metrics.clone()
    }
}

/// 安全同步器 — 从 NexusEvent 维护本地 SecurityState
///
/// WHY 独立结构体:将 L4/L8 安全事件→面板状态的转换隔离,
/// 面板侧无需理解 NexusEvent 的完整结构。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SecuritySync {
    state: SecurityState,
}

impl SecuritySync {
    /// 创建空的 Security 同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件影响安全状态则返回更新后的状态副本
    ///
    /// 处理的事件:
    /// - `SkepticVeto`:追加到 `active_vetoes` 并合并冻结能力。
    /// - `RedTeamAudit`:追加到 `recent_audits`。
    /// - `AsaIntervention`:追加到 `recent_interventions`。
    /// - `CapabilityFrozen`:追加到 `frozen_capabilities`。
    /// - `SandboxViolation`:不直接修改状态,仍进入日志流供 Log 面板展示。
    pub fn apply_event(
        &mut self,
        event: &NexusEvent,
        max_summaries: usize,
        max_frozen: usize,
    ) -> Option<SecurityState> {
        let changed = match event {
            NexusEvent::SkepticVeto {
                quest_id,
                veto_reason,
                frozen_capabilities,
                metadata,
            } => {
                self.state.active_vetoes.push(SkepticVetoSummary {
                    quest_id: quest_id.clone(),
                    veto_reason: veto_reason.clone(),
                    frozen_capabilities: frozen_capabilities.clone(),
                    timestamp: metadata.timestamp,
                });
                for cap in frozen_capabilities {
                    if !self.state.frozen_capabilities.contains(cap) {
                        self.state.frozen_capabilities.push(cap.clone());
                    }
                }
                true
            }
            NexusEvent::RedTeamAudit {
                vulnerability_type,
                failed_probes,
                total_probes,
                detection_rate,
                remediation_suggestion,
                metadata,
            } => {
                self.state.recent_audits.push(RedTeamAuditSummary {
                    vulnerability_type: vulnerability_type.clone(),
                    failed_probes: *failed_probes,
                    total_probes: *total_probes,
                    detection_rate: *detection_rate,
                    remediation_suggestion: remediation_suggestion.clone(),
                    timestamp: metadata.timestamp,
                });
                true
            }
            NexusEvent::AsaIntervention {
                operation_id,
                action,
                safety_score,
                block_reason,
                metadata,
                ..
            } => {
                self.state
                    .recent_interventions
                    .push(AsaInterventionSummary {
                        operation_id: operation_id.clone(),
                        action: action.clone(),
                        safety_score: *safety_score,
                        block_reason: block_reason.clone(),
                        timestamp: metadata.timestamp,
                    });
                true
            }
            NexusEvent::CapabilityFrozen {
                capability_id,
                reason,
                ..
            } => {
                if !self.state.frozen_capabilities.contains(capability_id) {
                    self.state.frozen_capabilities.push(capability_id.clone());
                }
                let _ = reason;
                true
            }
            NexusEvent::SandboxViolation { .. } => false,
            _ => false,
        };

        // 限制列表长度,避免内存无限增长。
        while self.state.active_vetoes.len() > max_summaries {
            self.state.active_vetoes.remove(0);
        }
        while self.state.recent_audits.len() > max_summaries {
            self.state.recent_audits.remove(0);
        }
        while self.state.recent_interventions.len() > max_summaries {
            self.state.recent_interventions.remove(0);
        }
        while self.state.frozen_capabilities.len() > max_frozen {
            self.state.frozen_capabilities.remove(0);
        }

        if changed {
            Some(self.state.clone())
        } else {
            None
        }
    }

    /// 获取当前安全状态副本
    pub fn state(&self) -> SecurityState {
        self.state.clone()
    }
}

/// 健康同步器 — 从 NexusEvent 维护本地 HealthMetrics
///
/// WHY 独立结构体:将系统健康事件→面板指标的转换隔离。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct HealthSync {
    metrics: HealthMetrics,
    /// 最近 MCP Mesh 事务延迟样本,用于计算平均延迟
    latency_samples: Vec<u64>,
    /// 最大延迟样本数
    max_latency_samples: usize,
}

impl HealthSync {
    /// 创建空的 Health 同步器
    pub fn new(max_latency_samples: usize) -> Self {
        Self {
            max_latency_samples,
            ..Default::default()
        }
    }

    /// 应用单个 NexusEvent,若事件影响健康指标则返回更新后的指标副本
    ///
    /// 处理的事件:
    /// - `SlowConsumerDropped`:增加慢消费者计数。
    /// - `McpMeshTransactionCompleted`:记录延迟样本并更新平均延迟。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<HealthMetrics> {
        let changed = match event {
            NexusEvent::SlowConsumerDropped { .. } => {
                self.metrics.slow_consumer_count += 1;
                true
            }
            NexusEvent::McpMeshTransactionCompleted { latency_ms, .. } => {
                self.latency_samples.push(*latency_ms);
                while self.latency_samples.len() > self.max_latency_samples {
                    self.latency_samples.remove(0);
                }
                self.metrics.average_latency_ms = if self.latency_samples.is_empty() {
                    0.0
                } else {
                    self.latency_samples.iter().sum::<u64>() as f64
                        / self.latency_samples.len() as f64
                };
                true
            }
            _ => false,
        };

        if changed {
            self.metrics.health_score =
                HealthMetrics::compute_health_score(self.metrics.slow_consumer_count);
            Some(self.metrics.clone())
        } else {
            None
        }
    }

    /// 获取当前健康指标副本
    pub fn metrics(&self) -> HealthMetrics {
        self.metrics.clone()
    }

    /// 根据本 tick 新增事件数计算每秒事件数
    ///
    /// `tick_interval_ms` 为 DataPipeline 的 tick 间隔。
    pub fn compute_events_per_second(&self, events_this_tick: usize, tick_interval_ms: u64) -> f64 {
        if tick_interval_ms == 0 {
            return 0.0;
        }
        events_this_tick as f64 / (tick_interval_ms as f64 / 1000.0)
    }
}

// ============================================================
// P2 TUI v1.7-omega 新增同步器 — 4 个监控面板的数据接入
// ============================================================
//
// WHY 独立结构体:与 QuestSync/BudgetSync 等保持对称,将事件→状态
// 转换逻辑隔离。每个同步器只处理一个 NexusEvent 变体,职责单一,
// 便于单元测试直接喂事件验证状态变化。

/// 衰减同步器 — 从 `DecayMetricsReported` 事件维护本地 DecayMetrics
///
/// 发布者:L4 decay-engine。消费:L10 TUI Decay 面板。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DecaySync {
    metrics: crate::types::DecayMetrics,
}

impl DecaySync {
    /// 创建空的衰减同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件影响衰减指标则返回更新后的指标副本
    ///
    /// - `DecayMetricsReported`:替换本地衰减指标,并返回新系数用于历史追加。
    ///   P2-11:同时提取 `fallback_count_delta` 字段,供 Decay 面板展示
    ///   learner_holder 异常回退次数,监控 learner 健康度。
    /// - 其他事件:返回 `None`,状态不变。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<crate::types::DecayMetrics> {
        match event {
            NexusEvent::DecayMetricsReported {
                coefficient,
                recent_events,
                cycle_start,
                fallback_count_delta,
                ..
            } => {
                self.metrics.coefficient = *coefficient;
                self.metrics.recent_events = recent_events.clone();
                self.metrics.cycle_start = Some(*cycle_start);
                // P2-11: 同步本周期 fallback 触发次数(异常回退层 + 熔断入口层)
                self.metrics.fallback_count_delta = *fallback_count_delta;
                Some(self.metrics.clone())
            }
            _ => None,
        }
    }

    /// 获取当前衰减指标副本
    pub fn metrics(&self) -> crate::types::DecayMetrics {
        self.metrics.clone()
    }
}

/// 路由器统计同步器 — 从 `RouterStatsReported` 事件维护本地 RouterMetrics
///
/// 发布者:L9 efficiency-monitor(聚合 L6 三路由器)。消费:L10 TUI Router 面板。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct RouterSync {
    metrics: crate::types::RouterMetrics,
}

impl RouterSync {
    /// 创建空的路由器统计同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件影响路由器指标则返回更新后的指标副本
    ///
    /// - `RouterStatsReported`:替换三路由器统计。
    /// - 其他事件:返回 `None`,状态不变。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<crate::types::RouterMetrics> {
        match event {
            NexusEvent::RouterStatsReported {
                kvbsr_stats,
                sesa_stats,
                faae_stats,
                ..
            } => {
                self.metrics.kvbsr_stats = convert_router_payload(kvbsr_stats);
                self.metrics.sesa_stats = convert_router_payload(sesa_stats);
                self.metrics.faae_stats = convert_router_payload(faae_stats);
                Some(self.metrics.clone())
            }
            _ => None,
        }
    }

    /// 获取当前路由器指标副本
    pub fn metrics(&self) -> crate::types::RouterMetrics {
        self.metrics.clone()
    }
}

/// 将 event-bus 的 RouterStatsPayload 转换为 TUI 内部的 RouterStatsInfo
///
/// WHY 单独函数:DecaySync/RouterSync/McpNodesSync/ChtcSync 均需做类似
/// 载荷→本地类型的转换,提取为函数避免重复代码。同时隔离类型映射,
/// 未来若 TUI 内部类型字段变化,只需修改此函数。
fn convert_router_payload(
    payload: &event_bus::RouterStatsPayload,
) -> crate::types::RouterStatsInfo {
    crate::types::RouterStatsInfo {
        hit_rate: payload.hit_rate,
        p50_latency_us: payload.p50_latency_us,
        p95_latency_us: payload.p95_latency_us,
        p99_latency_us: payload.p99_latency_us,
        hot_capabilities: payload.hot_capabilities.clone(),
    }
}

/// MCP 节点同步器 — 从 `McpNodeHeartbeat` 事件维护本地节点列表
///
/// 发布者:L10 mcp-mesh。消费:L10 TUI McpNodes 面板。
/// 采用 upsert 语义:相同 node_id 更新,新 node_id 追加。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct McpNodesSync {
    nodes: Vec<crate::types::McpNodeStatus>,
}

impl McpNodesSync {
    /// 创建空的 MCP 节点同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件为节点心跳则 upsert 节点状态
    ///
    /// - `McpNodeHeartbeat`:按 node_id upsert。状态字符串映射到 NodeStatus 枚举:
    ///   - "online" → Online
    ///   - "degraded" → Degraded
    ///   - 其他(含 "offline")→ Offline
    /// - 其他事件:返回 `None`,状态不变。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<Vec<crate::types::McpNodeStatus>> {
        match event {
            NexusEvent::McpNodeHeartbeat {
                node_id,
                status,
                throughput,
                last_seen,
                ..
            } => {
                let node_status = match status.as_str() {
                    "online" => crate::types::NodeStatus::Online,
                    "degraded" => crate::types::NodeStatus::Degraded,
                    _ => crate::types::NodeStatus::Offline,
                };
                let new_status = crate::types::McpNodeStatus {
                    node_id: node_id.clone(),
                    status: node_status,
                    throughput: *throughput,
                    last_seen: Some(*last_seen),
                };
                if let Some(existing) = self.nodes.iter_mut().find(|n| n.node_id == *node_id) {
                    *existing = new_status;
                } else {
                    self.nodes.push(new_status);
                }
                Some(self.nodes.clone())
            }
            _ => None,
        }
    }

    /// 获取当前节点列表副本
    pub fn nodes(&self) -> Vec<crate::types::McpNodeStatus> {
        self.nodes.clone()
    }
}

/// CHTC 适配器同步器 — 从 `ChtcAdapterStatus` 事件维护本地适配器列表
///
/// 发布者:L10 chtc-bridge。消费:L10 TUI Chtc 面板。
/// 采用 upsert 语义:相同 adapter_id 更新,新 adapter_id 追加。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ChtcSync {
    state: crate::types::ChtcState,
}

impl ChtcSync {
    /// 创建空的 CHTC 适配器同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件为适配器状态则 upsert 适配器信息
    ///
    /// - `ChtcAdapterStatus`:按 adapter_id upsert。
    /// - 其他事件:返回 `None`,状态不变。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<crate::types::ChtcState> {
        match event {
            NexusEvent::ChtcAdapterStatus {
                adapter_id,
                adapter_type,
                compatibility_score,
                recent_requests,
                is_online,
                ..
            } => {
                let new_info = crate::types::ChtcAdapterInfo {
                    adapter_id: adapter_id.clone(),
                    adapter_type: adapter_type.clone(),
                    compatibility_score: *compatibility_score,
                    recent_requests: recent_requests.clone(),
                    is_online: *is_online,
                };
                if let Some(existing) = self
                    .state
                    .adapters
                    .iter_mut()
                    .find(|a| a.adapter_id == *adapter_id)
                {
                    *existing = new_info;
                } else {
                    self.state.adapters.push(new_info);
                }
                Some(self.state.clone())
            }
            _ => None,
        }
    }

    /// 获取当前 CHTC 状态副本
    pub fn state(&self) -> crate::types::ChtcState {
        self.state.clone()
    }
}

// ============================================================
// P7 TUI v1.8-omega 新增同步器 — OsaSparse / ClvVector 面板数据接入
// ============================================================
//
// WHY 独立同步器:与 DecaySync/RouterSync 等保持对称,将事件→状态
// 转换逻辑隔离。每个同步器只处理一个 NexusEvent 变体,职责单一,
// 便于单元测试直接喂事件验证状态变化。

/// OSA 稀疏度同步器 — 从 `OmniSparseMasksComputed` 事件维护本地 OSA 状态
///
/// 发布者:L6 osa-coordinator。消费:L10 TUI OsaSparse 面板。
///
/// WHY 独立同步器: OSA 事件的消费逻辑与预算/健康同步器解耦,
/// 便于独立测试和未来扩展(如五维独立稀疏度展示)。
#[derive(Debug, Clone, PartialEq)]
pub struct OsaSync {
    /// 平均稀疏度 [0.0, 1.0](None = 未收到事件)
    sparsity: Option<f32>,
    /// context 维度活跃文件 ID 列表
    context_mask: Vec<String>,
    /// 稀疏度历史(容量 256,FIFO,存 sparsity * 1000 为 u64)
    sparsity_history: Vec<u64>,
    /// 稀疏度历史容量(FIFO)
    max_history: usize,
}

impl Default for OsaSync {
    fn default() -> Self {
        Self {
            sparsity: None,
            context_mask: Vec::new(),
            sparsity_history: Vec::new(),
            max_history: 256,
        }
    }
}

impl OsaSync {
    /// 创建 OSA 稀疏度同步器,默认历史容量 256
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件为 OSA 稀疏度计算则更新本地状态
    ///
    /// - `OmniSparseMasksComputed`:更新 sparsity / context_mask,并追加历史点。
    ///   历史存储为 `sparsity * 1000` 的 u64 值,避免 f32 序列化精度问题。
    /// - 其他事件:返回 `None`,状态不变。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<()> {
        match event {
            NexusEvent::OmniSparseMasksComputed {
                sparsity,
                context_mask,
                ..
            } => {
                self.sparsity = Some(*sparsity);
                self.context_mask = context_mask.clone();
                let history_value = (*sparsity * 1000.0) as u64;
                self.sparsity_history.push(history_value);
                while self.sparsity_history.len() > self.max_history {
                    self.sparsity_history.remove(0);
                }
                Some(())
            }
            _ => None,
        }
    }

    /// 获取当前平均稀疏度
    pub fn sparsity(&self) -> Option<f32> {
        self.sparsity
    }

    /// 获取当前 context 维度活跃文件 ID 列表副本
    pub fn context_mask(&self) -> Vec<String> {
        self.context_mask.clone()
    }

    /// 获取稀疏度历史副本
    pub fn sparsity_history(&self) -> Vec<u64> {
        self.sparsity_history.clone()
    }
}

/// CLV 摘要同步器 — 从 `ClvSnapshotReported` 事件维护本地 CLV 摘要
///
/// 发布者:L2 nmc-encoder。消费:L10 TUI ClvVector 面板。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ClvSync {
    summary: Option<event_bus::ClvSummary>,
}

impl ClvSync {
    /// 创建 CLV 摘要同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件为 CLV 快照报告则更新本地摘要
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<()> {
        match event {
            NexusEvent::ClvSnapshotReported { clv_summary, .. } => {
                self.summary = Some(clv_summary.clone());
                Some(())
            }
            _ => None,
        }
    }

    /// 获取当前 CLV 摘要副本
    pub fn summary(&self) -> Option<event_bus::ClvSummary> {
        self.summary.clone()
    }
}

// ============================================================
// M3b Chat 同步器 — ChatSync
// ============================================================

/// Chat 同步器 — 对话历史与状态的单一所有权(M3b)
///
/// WHY 单一所有权:app.rs 只发布 `TuiChatSubmitted`,该事件经 EventBus 回环
/// 到本同步器追加"用户消息";响应事件由编排器(M3c)产生。历史仅此一处拥有,
/// 经 DataSnapshot 同步到 TuiState,与其余面板"事件→Sync→Snapshot→State"一致,
/// 避免 app.rs 直写 TuiState 被 snapshot 覆盖的双所有权冲突。
#[derive(Debug, Clone, PartialEq)]
pub struct ChatSync {
    messages: Vec<ChatMessage>,
    status: ChatStatus,
    streaming: bool,
    max_messages: usize,
}

impl ChatSync {
    /// 创建 Chat 同步器
    pub fn new(max_messages: usize) -> Self {
        Self {
            messages: Vec::new(),
            status: ChatStatus::Idle,
            streaming: false,
            max_messages,
        }
    }

    /// 应用单个 NexusEvent,消费对话相关事件更新历史/状态
    pub fn apply_event(&mut self, event: &NexusEvent) {
        match event {
            NexusEvent::TuiChatSubmitted { query, .. } => {
                self.messages.push(ChatMessage {
                    role: ChatRole::User,
                    content: query.clone(),
                });
                self.streaming = false;
                self.enforce_cap();
            }
            NexusEvent::TuiChatResponseChunk { delta, .. } => {
                if !self.streaming {
                    self.messages.push(ChatMessage {
                        role: ChatRole::Assistant,
                        content: String::new(),
                    });
                    self.streaming = true;
                    self.enforce_cap();
                }
                if let Some(last) = self.messages.last_mut() {
                    last.content.push_str(delta);
                }
            }
            NexusEvent::TuiChatCompleted { .. } => {
                self.streaming = false;
            }
            NexusEvent::TuiChatStatusChanged { status, .. } => {
                self.status = *status;
            }
            _ => {}
        }
    }

    fn enforce_cap(&mut self) {
        while self.messages.len() > self.max_messages {
            self.messages.remove(0);
        }
    }

    /// 获取对话历史副本
    pub fn messages(&self) -> Vec<ChatMessage> {
        self.messages.clone()
    }

    /// 获取当前会话状态
    pub fn status(&self) -> ChatStatus {
        self.status
    }
}

/// Action 反馈同步器 — 消费编排层回发的 Action 终态,供 TUI 状态栏呈现(P0 交互链)
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActionFeedbackSync {
    latest: Option<(String, bool)>,
    seq: u64,
}

impl ActionFeedbackSync {
    /// 创建 Action 反馈同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent:消费 Action 终态反馈事件
    pub fn apply_event(&mut self, event: &NexusEvent) {
        match event {
            NexusEvent::TuiActionCompleted { result, .. } => {
                self.latest = Some((result.clone(), false));
                self.seq += 1;
            }
            NexusEvent::TuiActionFailed { error, .. } => {
                self.latest = Some((error.clone(), true));
                self.seq += 1;
            }
            _ => {}
        }
    }

    /// 当前反馈副本(供 DataSnapshot 同步)
    pub fn latest(&self) -> Option<(String, bool)> {
        self.latest.clone()
    }

    /// 当前反馈序号(app 据此判定是否为新反馈)
    pub fn seq(&self) -> u64 {
        self.seq
    }
}

// ============================================================
// P1-W2.2 Critical 旁路通道丢弃计数同步器
// ============================================================

/// Critical 旁路通道丢弃计数同步器(P1-W2.2 新增)
///
/// 从 `EfficiencyAlertTriggered` 事件(metric_name ==
/// [`CRITICAL_DROPPED_METRIC_NAME`])维护本地累计丢弃计数。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct CriticalDroppedSync {
    count: u64,
}

impl CriticalDroppedSync {
    /// 创建空的 Critical 丢弃同步器(count = 0)
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent:消费 Critical 旁路通道丢弃告警事件
    pub fn apply_event(&mut self, event: &NexusEvent) {
        if let NexusEvent::EfficiencyAlertTriggered {
            metric_name,
            triggered_value,
            ..
        } = event
        {
            if metric_name == CRITICAL_DROPPED_METRIC_NAME {
                self.count = *triggered_value as u64;
            }
        }
    }

    /// 当前累计丢弃事件数(供 DataSnapshot 同步)
    pub fn count(&self) -> u64 {
        self.count
    }
}
