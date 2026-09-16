//! 事件同步器 — 从 NexusEvent 流维护各面板的本地状态
//!
//! 每个同步器只处理特定的 NexusEvent 变体,职责单一,
//! 便于单元测试直接喂事件验证状态变化。`DataPipeline` 组合
//! 所有同步器生成统一 `DataSnapshot`。
//!
//! 对应架构层:L10 Interface

use std::collections::HashSet;

use event_bus::{ChatStatus, NexusEvent};
use nexus_contracts::app::AppEvent;
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

    /// 应用协议面事件（WI-01 TUI dogfooding）— 从 AppEvent 流更新 Quest 列表
    ///
    /// # 协议面数据保真机制
    /// AppEvent 的 `Item.payload`（JSON 形态）承载完整 Quest 数据：
    /// 核心侧将 Quest 序列化写入 payload，TUI 侧反序列化还原——
    /// 协议面不损失信息（Codex Item payload 同源设计）。
    ///
    /// # 映射
    /// - `ThreadStarted`: 新建空 Quest（quest_id = thread.goal_id，title 同 goal_id）
    /// - `ItemChanged` kind="quest": 反序列化 payload 为 Quest，按 quest_id upsert
    /// - `ItemChanged` kind="quest_completed" / "quest_cancelled": 从列表移除
    /// - 其他事件: 返回 `None`，状态不变
    pub fn apply_app_event(&mut self, ev: &AppEvent) -> Option<Vec<Quest>> {
        match ev {
            AppEvent::ThreadStarted { thread } => {
                // 新建会话级 Quest（完整数据随后续 Item payload 到达）
                let quest = Quest {
                    quest_id: thread.goal_id.as_ref().to_string(),
                    title: thread.goal_id.as_ref().to_string(),
                    ..Quest::default()
                };
                self.quests.retain(|q| q.quest_id != quest.quest_id);
                self.quests.push(quest);
                Some(self.quests.clone())
            }
            AppEvent::ItemChanged { item } => match item.kind.as_ref() {
                // 协议面数据保真: payload 承载序列化 Quest
                "quest" => {
                    let Ok(quest) = serde_json::from_str::<Quest>(&item.payload) else {
                        return None;
                    };
                    if let Some(existing) = self
                        .quests
                        .iter_mut()
                        .find(|q| q.quest_id == quest.quest_id)
                    {
                        *existing = quest;
                    } else {
                        self.quests.push(quest);
                    }
                    Some(self.quests.clone())
                }
                // 完成/取消 → 从活动列表移除（对标 NexusEvent 语义）
                "quest_completed" | "quest_cancelled" => {
                    let Ok(meta) = serde_json::from_str::<serde_json::Value>(&item.payload) else {
                        return None;
                    };
                    if let Some(qid) = meta.get("quest_id").and_then(|v| v.as_str()) {
                        self.quests.retain(|q| q.quest_id != qid);
                        self.paused_quest_ids.remove(qid);
                        Some(self.quests.clone())
                    } else {
                        None
                    }
                }
                _ => None,
            },
            _ => None,
        }
    }
}

/// Budget 同步器 — 从 NexusEvent 维护本地 BudgetMetrics
///
/// WHY 独立结构体:与 `QuestSync` 对称,将事件→指标的转换隔离,
/// 由 `BudgetMetricsUpdated` 直接填充面板视图,无需拼合多个事件。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct BudgetSync {
    metrics: BudgetMetrics,
    /// 最近一次 BudgetMetricsUpdated 到达的 Unix 毫秒(Concord T1.7)
    ///
    /// None = 从未收到更新;配合 `budget_metrics_ttl_ms` 判定指标陈旧,
    /// 驱动 Budget 面板置灰展示(M0 TODO 闭环)。
    last_update_ms: Option<u64>,
}

/// 当前 Unix 毫秒时间戳(sync 内部时钟口径,与 MetricsHistory 一致)
fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 预算指标陈旧判定 — 纯函数(Concord T1.7,消费 `budget_metrics_ttl_ms`)
///
/// # 语义
/// - 从未收到更新(`None`)→ 陈旧(面板展示的是默认占位值,必须诚实标注);
/// - 距上次更新的间隔 **严格大于** ttl → 陈旧(恰等于 ttl 视为新鲜,
///   边界语义与 proptest 单调性不变量一致);
/// - `now_ms < last_update`(时钟回拨)→ saturating_sub 归零,判为新鲜。
pub fn budget_is_stale(last_update_ms: Option<u64>, now_ms: u64, ttl_ms: u64) -> bool {
    match last_update_ms {
        None => true,
        Some(t) => now_ms.saturating_sub(t) > ttl_ms,
    }
}

impl BudgetSync {
    /// 创建空的 Budget 同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件影响预算指标则返回更新后的指标副本
    ///
    /// - `BudgetMetricsUpdated`:直接替换本地指标并记录到达时刻。
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
                self.last_update_ms = Some(now_unix_ms());
                Some(self.metrics.clone())
            }
            _ => None,
        }
    }

    /// 获取当前预算指标副本
    pub fn metrics(&self) -> BudgetMetrics {
        self.metrics.clone()
    }

    /// 最近一次预算更新的 Unix 毫秒(None = 从未收到;Concord T1.7)
    pub fn last_update_ms(&self) -> Option<u64> {
        self.last_update_ms
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
    // === PROBE P0.4:HCW 召回读数(由 HcwRecallReported 事件同步) ===
    /// 多针召回率 needle_recall@8 ∈ [0,1](None = 未收到报告)
    recall_needle_at_8: Option<f32>,
    /// 位置偏置比 ∈ [0,1](None = 未收到报告)
    recall_position_bias: Option<f32>,
    /// 链路成功率 ∈ [0,1](None = 未收到报告)
    recall_chain_success: Option<f32>,
}

impl Default for OsaSync {
    fn default() -> Self {
        Self {
            sparsity: None,
            context_mask: Vec::new(),
            sparsity_history: Vec::new(),
            max_history: 256,
            recall_needle_at_8: None,
            recall_position_bias: None,
            recall_chain_success: None,
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
            // PROBE P0.4:HCW 召回评测报告 → 更新三项召回读数
            // WHY 归入 OsaSync: 召回是 HCW 窗口装载质量的核心指标,
            // 与稀疏度同属 OSA 面板的上下文健康读数(设计文档 §4.1 接线)
            NexusEvent::HcwRecallReported {
                needle_recall_at_8,
                position_bias,
                chain_success_rate,
                ..
            } => {
                self.recall_needle_at_8 = Some(*needle_recall_at_8);
                self.recall_position_bias = Some(*position_bias);
                self.recall_chain_success = Some(*chain_success_rate);
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

    /// 获取多针召回率 needle_recall@8（None = 未收到报告）
    pub fn recall_needle_at_8(&self) -> Option<f32> {
        self.recall_needle_at_8
    }

    /// 获取位置偏置比（None = 未收到报告）
    pub fn recall_position_bias(&self) -> Option<f32> {
        self.recall_position_bias
    }

    /// 获取链路成功率（None = 未收到报告）
    pub fn recall_chain_success(&self) -> Option<f32> {
        self.recall_chain_success
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
    /// 行闸门(Concord W3 T3.1):流式增量按完整行提交,半行暂存
    ///
    /// WHY 置于同步层:闸门是"事件→消息内容"累积的一部分,与 streaming
    /// 生命周期同归 ChatSync 所有;渲染层(v3 引擎)零改动。
    gate: super::newline_gate::NewlineGate,
}

impl ChatSync {
    /// 创建 Chat 同步器
    pub fn new(max_messages: usize) -> Self {
        Self {
            messages: Vec::new(),
            status: ChatStatus::Idle,
            streaming: false,
            max_messages,
            gate: super::newline_gate::NewlineGate::new(),
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
                // 新一轮交互:重置闸门(上一轮若有未闭合残段不再续接)
                self.gate.flush();
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
                // Concord W3 T3.1:增量经行闸门,仅完整行追加进消息;
                // 半行/未闭合 fence 块留存闸门,避免半行闪烁(内容守恒)
                let committed = self.gate.feed(delta);
                if !committed.is_empty() {
                    if let Some(last) = self.messages.last_mut() {
                        for line in committed {
                            last.content.push_str(&line);
                        }
                    }
                }
            }
            NexusEvent::TuiChatCompleted { .. } => {
                // 流结束:冲刷闸门残段(含未闭合 fence 块),不丢内容
                if let Some(rest) = self.gate.flush() {
                    if let Some(last) = self.messages.last_mut() {
                        last.content.push_str(&rest);
                    }
                }
                self.streaming = false;
            }
            NexusEvent::TuiChatStatusChanged { status, .. } => {
                self.status = *status;
            }
            // FC-2(ADR-081):/compact 策展回写 —— 压缩后的历史整体替换。
            // WHY 事件而非直接写:ChatSync 是会话历史唯一所有者(M3b),
            // 本事件是发给所有者的控制指令,保持单一所有权设计不破;
            // 替换后重置流式状态与行闸门(旧轮次的未闭合残段不再续接)。
            NexusEvent::TuiChatHistoryReplaced { messages, .. } => {
                self.messages = messages
                    .iter()
                    .map(|m| ChatMessage {
                        // 角色以字符串编码(L1 不感知 ChatRole 枚举):
                        // 仅 "assistant" 映射 Assistant,其余(含未知值)
                        // 保守映射 User —— 策展 Pinned 段按 User 轮次保护
                        role: if m.role == "assistant" {
                            ChatRole::Assistant
                        } else {
                            ChatRole::User
                        },
                        content: m.content.clone(),
                    })
                    .collect();
                self.streaming = false;
                self.gate.flush();
                self.enforce_cap();
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
    /// 最近一次回执归属的请求标识(P1 回执精准配对)
    latest_request_id: Option<String>,
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
            NexusEvent::TuiActionCompleted {
                request_id, result, ..
            } => {
                self.latest = Some((result.clone(), false));
                self.latest_request_id = Some(request_id.clone());
                self.seq += 1;
            }
            NexusEvent::TuiActionFailed {
                request_id, error, ..
            } => {
                self.latest = Some((error.clone(), true));
                self.latest_request_id = Some(request_id.clone());
                self.seq += 1;
            }
            _ => {}
        }
    }

    /// 当前反馈副本(供 DataSnapshot 同步)
    pub fn latest(&self) -> Option<(String, bool)> {
        self.latest.clone()
    }

    /// 当前反馈归属的请求标识副本(P1:供 app 按 request_id 精准清除超时计时)
    pub fn latest_request_id(&self) -> Option<String> {
        self.latest_request_id.clone()
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

// ============================================================
// PS-2(F-1):协议握手回执同步器(ADR-082 SEC-4)
// ============================================================

/// 协议握手回执同步器(PS-2 F-1)
///
/// 消费编排器应答的 `TuiHelloAck`(编排器仅应答**首帧**握手,SEC-4),
/// 把 `compat` 投影为 TUI 本地 [`HandshakeState`](crate::types::HandshakeState)。
///
/// # 闭环意义(评估报告 F-1)
/// 此前 TUI 发布 `TuiHello` 后从不读取回执的 `compat` 字段 —— 版本不兼容
/// (Degraded/Refused)时 UI 无感知,握手是"发射后不管"的单向死链。
/// 本同步器 + `TuiApp::update` 的状态栏上屏使其成为可观测闭环。
///
/// # 多帧语义
/// SEC-4 保证正常运行只到达一帧;若异常重复到达,以最新一帧覆盖
/// (幂等收敛,与"最新握手为准"的直觉一致)。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct HandshakeSync {
    latest: Option<crate::types::HandshakeState>,
}

impl HandshakeSync {
    /// 创建握手回执同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent:消费 TuiHelloAck(其余事件无操作)
    pub fn apply_event(&mut self, event: &NexusEvent) {
        if let NexusEvent::TuiHelloAck {
            compat,
            server_version,
            ..
        } = event
        {
            use event_bus::CompatLevel;
            let (level, degraded_items) = match compat {
                CompatLevel::Full => (crate::types::HandshakeLevel::Full, Vec::new()),
                CompatLevel::Degraded(items) => {
                    (crate::types::HandshakeLevel::Degraded, items.clone())
                }
                CompatLevel::Refused => (crate::types::HandshakeLevel::Refused, Vec::new()),
            };
            self.latest = Some(crate::types::HandshakeState {
                level,
                degraded_items,
                server_version: server_version.clone(),
            });
        }
    }

    /// 当前握手状态副本(供 DataSnapshot 同步;None = 尚未收到 Ack)
    pub fn latest(&self) -> Option<crate::types::HandshakeState> {
        self.latest.clone()
    }
}

#[cfg(test)]
mod stale_tests {
    //! Concord T1.7:budget_is_stale 纯判定函数测试(边界 + proptest 单调性)
    use super::budget_is_stale;
    use proptest::prelude::*;

    #[test]
    fn never_updated_is_always_stale() {
        // 从未收到更新 → 无论 ttl 多大都判陈旧
        assert!(budget_is_stale(None, 1_000, u64::MAX));
        assert!(budget_is_stale(None, 0, 0));
    }

    #[test]
    fn boundary_equal_ttl_is_fresh() {
        // 恰等于 ttl → 新鲜(严格 > 语义);ttl+1 → 陈旧
        assert!(!budget_is_stale(Some(0), 5000, 5000));
        assert!(budget_is_stale(Some(0), 5001, 5000));
    }

    #[test]
    fn clock_rollback_is_fresh() {
        // now < last_update(时钟回拨)→ saturating_sub 归零 → 新鲜(不谎报)
        assert!(!budget_is_stale(Some(9000), 1000, 500));
    }

    proptest! {
        /// 属性:判定等价于"间隔严格大于 ttl",且对间隔单调不减
        #[test]
        fn stale_iff_elapsed_exceeds_ttl_and_monotone(
            base in 0u64..1_000_000,
            elapsed in 0u64..1_000_000,
            ttl in 0u64..1_000_000,
            extra in 0u64..1_000_000,
        ) {
            let now = base.saturating_add(elapsed);
            prop_assert_eq!(budget_is_stale(Some(base), now, ttl), elapsed > ttl);
            // 单调性:间隔再增大不可能从陈旧变回新鲜
            let later = now.saturating_add(extra);
            if budget_is_stale(Some(base), now, ttl) {
                prop_assert!(budget_is_stale(Some(base), later, ttl));
            }
        }
    }
}

// ============================================================
// PS-2(F-6):子代理任务失败聚合同步器(Critical 级可观测性)
// ============================================================

/// `AgentTaskFailed` 保留条数上限
///
/// WHY 5:安全面板右栏是窄栏(30%),展示最新失败即可;完整历史仍可由
/// EventStream 面板检索(事件流不截断)。上限同时防止重试风暴刷屏。
pub const MAX_AGENT_FAILURES: usize = 5;

/// 子代理任务失败聚合同步器(PS-2 F-6)
///
/// 消费 Critical 级 `AgentTaskFailed`(`chimera-mas` 任务失败/超时发布,
/// 见 `chunker.rs:497/520`),维护"最近 N 条 + 累计数 + 序号"。
///
/// # 闭环意义(评估报告 F-6,证据修正版)
/// 该事件此前**已到达 TUI**(`publish_critical` 双通道含 broadcast,
/// `bus.rs:844-846`;TUI 订阅无主题过滤,`subscriber.rs:57`)且严重度已判为
/// Critical(`registry.rs:234`),EventStream 会以 Critical 样式显示。
/// 真实缺口是**无聚合**:失败随滚动消失,安全面板无态势、状态栏无告警。
/// 本同步器补齐这一层,使"一闪而过"变为"持久可见"。
///
/// # 时序语义
/// `seq` 每收到一条失败 +1,供 app 判定"新失败"并一次性告警;
/// `total` 为会话内累计(不随保留窗口淘汰而减少)。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct AgentFailureSync {
    /// 最近失败(新在前,容量 [`MAX_AGENT_FAILURES`])
    recent: std::collections::VecDeque<crate::types::AgentFailureSummary>,
    /// 累计失败数(单调递增)
    total: u64,
    /// 失败序号(单调递增,供上屏去重)
    seq: u64,
}

impl AgentFailureSync {
    /// 创建子代理失败同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent:消费 `AgentTaskFailed`(其余事件无操作)
    pub fn apply_event(&mut self, event: &NexusEvent) {
        if let NexusEvent::AgentTaskFailed {
            from,
            to,
            task_id,
            error,
            retry_count,
            ..
        } = event
        {
            self.recent.push_front(crate::types::AgentFailureSummary {
                from: from.clone(),
                to: to.clone(),
                task_id: task_id.clone(),
                error: error.clone(),
                retry_count: *retry_count,
            });
            // 超出窗口淘汰最旧一条(累计数不受影响)
            while self.recent.len() > MAX_AGENT_FAILURES {
                self.recent.pop_back();
            }
            self.total += 1;
            self.seq += 1;
        }
    }

    /// 最近失败副本(新在前,供 DataSnapshot 同步)
    pub fn recent(&self) -> Vec<crate::types::AgentFailureSummary> {
        self.recent.iter().cloned().collect()
    }

    /// 累计失败数(供 DataSnapshot 同步)
    pub fn total(&self) -> u64 {
        self.total
    }

    /// 失败序号(app 据此判定新失败,避免每 tick 重复上屏)
    pub fn seq(&self) -> u64 {
        self.seq
    }
}

// ============================================================
// PS-2(F-1):HandshakeSync 单测 —— 三级 compat 投影 + 无关事件免疫
// ============================================================

#[cfg(test)]
mod handshake_sync_tests {
    use super::*;
    use event_bus::{CompatLevel, EventMetadata};

    fn ack(compat: CompatLevel) -> NexusEvent {
        NexusEvent::TuiHelloAck {
            metadata: EventMetadata::new("handshake-test"),
            proto: "1.0.0".into(),
            compat,
            server_version: "2.28.2-omega".into(),
        }
    }

    #[test]
    fn full_ack_projects_to_full_state() {
        let mut sync = HandshakeSync::new();
        assert!(sync.latest().is_none(), "初始:尚未收到 Ack");

        sync.apply_event(&ack(CompatLevel::Full));
        let hs = sync.latest().expect("Full Ack 应产生状态");
        assert_eq!(hs.level, crate::types::HandshakeLevel::Full);
        assert!(hs.degraded_items.is_empty(), "Full 无降级项");
        assert_eq!(hs.server_version, "2.28.2-omega");
    }

    #[test]
    fn degraded_ack_carries_degraded_items() {
        let mut sync = HandshakeSync::new();
        sync.apply_event(&ack(CompatLevel::Degraded(vec![
            "agent-tree".into(),
            "overwindow".into(),
        ])));
        let hs = sync.latest().expect("Degraded Ack 应产生状态");
        assert_eq!(hs.level, crate::types::HandshakeLevel::Degraded);
        assert_eq!(hs.degraded_items, vec!["agent-tree", "overwindow"]);
    }

    #[test]
    fn refused_ack_projects_to_refused_state() {
        let mut sync = HandshakeSync::new();
        sync.apply_event(&ack(CompatLevel::Refused));
        let hs = sync.latest().expect("Refused Ack 应产生状态");
        assert_eq!(hs.level, crate::types::HandshakeLevel::Refused);
    }

    #[test]
    fn unrelated_events_are_ignored() {
        let mut sync = HandshakeSync::new();
        sync.apply_event(&NexusEvent::RefreshStateRequested {
            metadata: EventMetadata::new("handshake-test"),
            requested_by: "drift-test".into(),
        });
        assert!(sync.latest().is_none(), "非握手事件不得产生状态");

        // SEC-4 幂等:重复 Ack 以最新一帧覆盖(此处连续同帧,状态稳定)
        sync.apply_event(&ack(CompatLevel::Refused));
        sync.apply_event(&ack(CompatLevel::Refused));
        assert_eq!(
            sync.latest().expect("Ack 应产生状态").level,
            crate::types::HandshakeLevel::Refused
        );
    }
}

// ============================================================
// PS-2 批次1:议会数据同步器(事件快照取代 L10→L8 越层直调)
// ============================================================

/// 议会数据同步器(PS-2 批次1)
///
/// 消费两个**已发布但 TUI 从未消费**的事件,取代原 `parliament::immune_system_status()`
/// 直调(`panels/parliament.rs:303`,该函数实为硬编码全零占位,见 `immune_system.rs:646`)。
///
/// - `CoordinationRatioReported`(`strategy_cap.rs:645`)→ [`CoordinationMetrics`](crate::types::CoordinationMetrics)
/// - `ParliamentStrategyCapChanged`(`strategy_cap.rs:675`)→ [`StrategyCapState`](crate::types::StrategyCapState)
///
/// # 设计要点
/// 两事件**独立填充**各自字段(不做字段级合并):到达次序不定,分字段保存可如实
/// 表达"只收到其中一个"的中间态,面板对缺项显示 N/A,不伪造默认值。
///
/// # 收益
/// 面板数据从"进程内全局零值"变为"事件流派生",**可回放、可归因**;
/// 同时使 `chimera-tui → parliament` 依赖边得以移除(§2.2 依赖铁律)。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ParliamentSync {
    state: crate::types::ParliamentState,
}

impl ParliamentSync {
    /// 创建议会数据同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent:消费上述两个议会事件(其余事件无操作)
    pub fn apply_event(&mut self, event: &NexusEvent) {
        match event {
            NexusEvent::CoordinationRatioReported {
                coordination_cost_ms,
                inference_gain,
                ratio,
                ..
            } => {
                self.state.coordination = Some(crate::types::CoordinationMetrics {
                    cost_ms: *coordination_cost_ms,
                    inference_gain: *inference_gain,
                    ratio: *ratio,
                });
            }
            NexusEvent::ParliamentStrategyCapChanged {
                new_cap,
                ratio,
                threshold,
                ..
            } => {
                self.state.strategy_cap = Some(crate::types::StrategyCapState {
                    cap: new_cap.clone(),
                    ratio: *ratio,
                    threshold: *threshold,
                });
            }
            _ => {}
        }
    }

    /// 当前议会数据副本(供 DataSnapshot 同步)
    pub fn state(&self) -> crate::types::ParliamentState {
        self.state.clone()
    }
}

// ============================================================
// PS-2(F-6):AgentFailureSync 单测 —— 聚合 / 窗口封顶 / 无关事件免疫
// ============================================================

#[cfg(test)]
mod agent_failure_sync_tests {
    use super::*;
    use event_bus::EventMetadata;

    fn fail(task_id: &str) -> NexusEvent {
        NexusEvent::AgentTaskFailed {
            metadata: EventMetadata::new("agent-test"),
            from: "agent-a".into(),
            to: "orchestrator".into(),
            task_id: task_id.into(),
            error: format!("boom-{task_id}"),
            retry_count: 1,
        }
    }

    #[test]
    fn aggregates_recent_total_and_seq_with_newest_first() {
        let mut sync = AgentFailureSync::new();
        assert_eq!(sync.total(), 0, "初始无失败");
        assert_eq!(sync.seq(), 0);
        assert!(sync.recent().is_empty());

        sync.apply_event(&fail("t1"));
        assert_eq!(sync.total(), 1);
        assert_eq!(sync.seq(), 1);
        assert_eq!(sync.recent()[0].task_id, "t1");
        assert_eq!(sync.recent()[0].error, "boom-t1");

        // 新失败排在最前
        sync.apply_event(&fail("t2"));
        assert_eq!(sync.total(), 2);
        assert_eq!(sync.seq(), 2);
        assert_eq!(sync.recent()[0].task_id, "t2");
        assert_eq!(sync.recent()[1].task_id, "t1");
    }

    #[test]
    fn recent_window_is_capped_while_total_keeps_growing() {
        let mut sync = AgentFailureSync::new();
        let n = MAX_AGENT_FAILURES + 3;
        for i in 0..n {
            sync.apply_event(&fail(&format!("t{i}")));
        }
        assert_eq!(
            sync.recent().len(),
            MAX_AGENT_FAILURES,
            "保留窗口必须封顶(防重试风暴刷屏)"
        );
        assert_eq!(sync.total(), n as u64, "累计数不受窗口淘汰影响");
        assert_eq!(
            sync.recent()[0].task_id,
            format!("t{}", n - 1),
            "窗口首条应为最新失败"
        );
    }

    #[test]
    fn unrelated_events_do_not_aggregate() {
        let mut sync = AgentFailureSync::new();
        // 同层但语义不同的事件不得计入失败聚合
        sync.apply_event(&NexusEvent::RefreshStateRequested {
            metadata: EventMetadata::new("test"),
            requested_by: "drift-test".into(),
        });
        assert_eq!(sync.total(), 0, "无关事件不应产生失败聚合");
        assert_eq!(sync.seq(), 0, "无关事件不应推进失败序号");
    }
}

// ============================================================
// PS-2 批次2:GQEP 超时统计同步器(事件计数取代 L10→L7 越层直调)
// ============================================================

/// GQEP 超时统计同步器(PS-2 批次2)
///
/// 以**事件计数**取代原 `gqep_executor::timeout_stats()` 直调
/// (`panels/metrics_dashboard.rs:276`,L10→L7 下行依赖):
/// - `OperationTimedOut` → [`per_op`](crate::types::GqepTimeoutStats::per_op)
/// - `GatherTimedOut` → [`global`](crate::types::GqepTimeoutStats::global)
/// - `OrphanCallDetected` → [`orphan`](crate::types::GqepTimeoutStats::orphan)
///
/// # WHY 计数从 0 起而非同步进程内计数器
/// 进程内原子计数器是"不可回放的瞬时值";事件流计数则是**可回放、可归因**的
/// 单一事实源(与已治理的 Decay/CMT/OSA 同轨)。启动期短暂为 0 是**真值**
/// (尚未发生超时),不同于旧实现的"恒零占位假数据"。
///
/// # 溢出
/// 计数器为 `u64`,实际不可能溢出;即便溢出也会 `wrapping_add` 而非 panic
/// (测试以 `saturating_add` 语义断言稳定,此处用 wrapping 仅在极端场景生效)。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct GqepTimeoutSync {
    stats: crate::types::GqepTimeoutStats,
}

impl GqepTimeoutSync {
    /// 创建 GQEP 超时统计同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent:累加三路超时/孤儿调用计数(其余事件无操作)
    pub fn apply_event(&mut self, event: &NexusEvent) {
        match event {
            NexusEvent::OperationTimedOut { .. } => {
                self.stats.per_op = self.stats.per_op.saturating_add(1)
            }
            NexusEvent::GatherTimedOut { .. } => {
                self.stats.global = self.stats.global.saturating_add(1)
            }
            NexusEvent::OrphanCallDetected { .. } => {
                self.stats.orphan = self.stats.orphan.saturating_add(1)
            }
            _ => {}
        }
    }

    /// 当前统计副本(供 DataSnapshot 同步)
    pub fn stats(&self) -> crate::types::GqepTimeoutStats {
        self.stats.clone()
    }
}

// ============================================================
// PS-2 批次1:ParliamentSync 单测 —— 双事件独立填充 + 无关事件免疫
// ============================================================

#[cfg(test)]
mod parliament_sync_tests {
    use super::*;
    use event_bus::EventMetadata;

    fn ratio_event(cost: f64, gain: f32, ratio: f64) -> NexusEvent {
        NexusEvent::CoordinationRatioReported {
            metadata: EventMetadata::new("parliament-test"),
            coordination_cost_ms: cost,
            inference_gain: gain,
            cost_index: 1.0,
            gain_index: 1.0,
            ratio,
            is_paradox_risk: false,
            threshold: 0.6,
            sample_count: 1,
        }
    }

    fn cap_event(new_cap: &str) -> NexusEvent {
        NexusEvent::ParliamentStrategyCapChanged {
            metadata: EventMetadata::new("parliament-test"),
            old_cap: "full".into(),
            new_cap: new_cap.into(),
            ratio: 0.5,
            threshold: 0.6,
        }
    }

    #[test]
    fn both_events_fill_their_own_fields() {
        let mut sync = ParliamentSync::new();
        assert!(sync.state().coordination.is_none(), "初始无协调比");
        assert!(sync.state().strategy_cap.is_none(), "初始无策略封顶");

        sync.apply_event(&ratio_event(12.5, 0.8, 0.42));
        let s = sync.state();
        let c = s.coordination.expect("协调比应已填充");
        assert_eq!(c.cost_ms, 12.5);
        assert!((c.inference_gain - 0.8).abs() < f32::EPSILON);
        assert_eq!(c.ratio, 0.42);
        assert!(s.strategy_cap.is_none(), "策略封顶不应被协调比事件填充");

        sync.apply_event(&cap_event("simplified"));
        let s = sync.state();
        assert_eq!(s.strategy_cap.expect("策略封顶应已填充").cap, "simplified");
        assert!(s.coordination.is_some(), "先到的协调比不应被覆盖或清空");
    }

    #[test]
    fn later_event_overwrites_same_field_only() {
        let mut sync = ParliamentSync::new();
        sync.apply_event(&ratio_event(1.0, 0.1, 0.1));
        sync.apply_event(&ratio_event(2.0, 0.2, 0.2));
        let c = sync.state().coordination.expect("应有协调比");
        assert_eq!(c.cost_ms, 2.0, "同字段以最新一帧为准");
        assert_eq!(c.inference_gain, 0.2);
    }

    #[test]
    fn unrelated_events_do_not_fill_anything() {
        let mut sync = ParliamentSync::new();
        sync.apply_event(&NexusEvent::RefreshStateRequested {
            metadata: EventMetadata::new("test"),
            requested_by: "drift-test".into(),
        });
        assert!(sync.state().coordination.is_none());
        assert!(sync.state().strategy_cap.is_none());
    }
}

// ============================================================
// PS-2 批次2:GqepTimeoutSync 单测 —— 三路计数 + 无关事件免疫
// ============================================================

#[cfg(test)]
mod gqep_timeout_sync_tests {
    use super::*;
    use event_bus::EventMetadata;

    fn op_timed_out() -> NexusEvent {
        NexusEvent::OperationTimedOut {
            metadata: EventMetadata::new("gqep-test"),
            operation_id: "op-1".into(),
            timeout_ms: 100,
        }
    }

    fn gather_timed_out() -> NexusEvent {
        NexusEvent::GatherTimedOut {
            metadata: EventMetadata::new("gqep-test"),
            deadline_ms: 1000,
            elapsed_ms: 1001,
            total: 4,
            abandoned: 2,
        }
    }

    fn orphan() -> NexusEvent {
        NexusEvent::OrphanCallDetected {
            metadata: EventMetadata::new("gqep-test"),
            operation_id: "op-2".into(),
            spawn_location: "gatherer".into(),
        }
    }

    #[test]
    fn counts_three_categories_independently() {
        let mut sync = GqepTimeoutSync::new();
        assert_eq!(sync.stats().per_op, 0);
        assert_eq!(sync.stats().global, 0);
        assert_eq!(sync.stats().orphan, 0);

        sync.apply_event(&op_timed_out());
        sync.apply_event(&op_timed_out());
        sync.apply_event(&gather_timed_out());
        sync.apply_event(&orphan());

        let s = sync.stats();
        assert_eq!(s.per_op, 2, "单操作超时应累加 2");
        assert_eq!(s.global, 1, "全局超时应累加 1");
        assert_eq!(s.orphan, 1, "孤儿调用应累加 1");
    }

    #[test]
    fn unrelated_events_do_not_increment_counters() {
        let mut sync = GqepTimeoutSync::new();
        sync.apply_event(&NexusEvent::RefreshStateRequested {
            metadata: EventMetadata::new("test"),
            requested_by: "drift-test".into(),
        });
        let s = sync.stats();
        assert_eq!((s.per_op, s.global, s.orphan), (0, 0, 0));
    }

    #[test]
    fn counters_saturate_instead_of_panicking() {
        let mut sync = GqepTimeoutSync::new();
        sync.stats.per_op = u64::MAX;
        sync.apply_event(&op_timed_out());
        assert_eq!(
            sync.stats().per_op,
            u64::MAX,
            "极端场景下应饱和而非回绕/panic"
        );
    }
}
