//! TUI 数据源抽象 — 为 L10 Interface 提供统一数据访问契约
//!
//! 设计约束(WHY):
//! - `chimera-tui` 位于 L10,按 §2.2 依赖铁律禁止直接依赖 L9 的
//!   `quest-engine`/`efficiency-monitor`。因此本模块只依赖 L1 的
//!   `event-bus` 与 `nexus-core`(共享领域类型),所有数据通过
//!   `NexusEvent` 事件流推导。
//! - `TuiDataSource` trait 将事件总线细节与面板渲染解耦:面板只读
//!   `DataSnapshot`,不关心数据是实时事件、本地缓存还是测试桩。
//! - `DataSnapshot` 使用本地 `BudgetMetrics` 而非直接暴露 L9 指标类型,
//!   避免跨层泄漏。
//!
//! # 当前 `NexusEvent` 覆盖度分析与建议新增变体(P0.2)
//!
//! TUI 需要三类数据:Quest 列表、最近事件流、Budget 指标。对
//! `event-bus/src/types.rs` 现有变体盘点如下:
//!
//! - **Quest 列表**:现有 `QuestCreated` / `QuestProgressUpdated` /
//!   `ThinkingModeSwitched` / `CheckpointSaved` / `ExecutionCompleted`
//!   可用于增量维护列表,但缺少:
//!   1. `QuestListUpdated { quests: Vec<Quest>, source: String }`:由 `quest-engine` 周期性发布完整列表,便于 TUI 冷启动或 lag 后快速对齐,避免依赖多次增量事件才能拼出完整状态。
//!   2. `QuestCompleted { quest_id: String, status: QuestStatus }`:标记 Quest 结束(成功/失败/取消),用于从活动列表移除。
//! - **Budget 指标**:现有 `BudgetStatsReported` / `BudgetAdjusted` /
//!   `BudgetExceeded` / `EfficiencyAlertTriggered` 已足够 TUI 聚合出
//!   `BudgetMetrics`;但为了减少面板解析文本的负担,建议新增:
//!   3. `BudgetMetricsUpdated { metrics: BudgetMetricsPayload }`:由 `efficiency-monitor` 发布结构化的预算指标,`BudgetMetricsPayload` 可复用本模块 `BudgetMetrics` 字段,使 TUI 直接消费而不必从多个事件拼合。
//! - **事件流**:直接订阅 `EventBus` 即可获得 `VecDeque<NexusEvent>`,
//!   无需新增事件变体。
//!
//! 上述新增变体将在 Task P1.2 中提交到 `event-bus/src/types.rs`,
//! 本模块当前仅作契约描述,不修改 event-bus。

use crate::error::TuiError;
use crate::subscriber::EventSubscriber;
use event_bus::{EventMetadata, NexusEvent};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;
use tokio::time::{self, Duration};

/// 数据快照 — TUI 各面板渲染所需数据的统一视图
///
/// WHY 独立结构体:面板渲染只依赖此快照,不依赖具体数据源实现,
/// 方便单元测试用内存桩替换 event-bus 订阅。
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataSnapshot {
    /// 当前活动 Quest 列表
    ///
    /// 来源:聚合 `QuestCreated` / `QuestListUpdated` 等事件。
    /// 使用 `nexus_core::Quest` 保证与 L1 领域模型一致。
    pub quest_list: Vec<Quest>,

    /// 最近接收到的 NexusEvent,按时间顺序,旧在前
    ///
    /// WHY VecDeque:面板需要"最新 N 条"语义,从队尾追加、队首丢弃
    /// 为 O(1),避免频繁 `Vec::remove(0)`。
    pub latest_events: VecDeque<NexusEvent>,

    /// 当前预算指标
    pub budget_metrics: BudgetMetrics,
}

/// 预算指标 — TUI Budget 面板的轻量级本地视图
///
/// WHY 不直接复用 `efficiency-monitor` 类型:该 crate 位于 L9,
/// L10 不能直接依赖。本结构体只保留面板展示必需字段,
/// 由 `BudgetStatsReported` / `BudgetAdjusted` / `BudgetExceeded`
/// 等事件聚合而来。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetMetrics {
    /// 总消耗量(单位由预算类型决定)
    pub total_consumption: f64,
    /// 剩余预算
    pub remaining_budget: f64,
    /// 利用率 [0.0, 1.0]
    pub utilization_rate: f32,
    /// 当前预算档位(如 "High"/"Medium"/"Low")
    pub current_tier: String,
    /// 档位系数,1.0 为基准
    pub coefficient: f32,
    /// 是否已触发预算超限
    pub is_exceeded: bool,
    /// 最新告警信息(无告警为 None)
    pub alert: Option<String>,
}

impl Default for BudgetMetrics {
    fn default() -> Self {
        Self {
            total_consumption: 0.0,
            remaining_budget: 0.0,
            utilization_rate: 0.0,
            current_tier: "High".into(),
            coefficient: 1.0,
            is_exceeded: false,
            alert: None,
        }
    }
}

/// TUI 数据源配置 — 控制缓存大小与行为
///
/// WHY 提前定义配置:后续 `DataPipeline` 需要容量上限,
/// 避免事件流无限增长导致内存膨胀。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataSourceConfig {
    /// 事件流保留的最大条数
    pub max_event_history: usize,
    /// Quest 列表保留的最大条数
    pub max_quest_list_size: usize,
    /// 预算指标无更新时的过期时间(毫秒),当前占位
    pub budget_metrics_ttl_ms: u64,
    /// tick 间隔(毫秒),控制快照生成频率
    pub tick_interval_ms: u64,
}

impl Default for DataSourceConfig {
    fn default() -> Self {
        Self {
            // WHY 256:平衡调试可见性与内存占用;按每条 NexusEvent 约 500 字节估算,
            // 约 128KB,远低于 HCW 128K 窗口约束。
            max_event_history: 256,
            max_quest_list_size: 64,
            budget_metrics_ttl_ms: 5000,
            tick_interval_ms: 250,
        }
    }
}

/// TUI 数据源 trait — 抽象事件总线订阅、测试桩或缓存
///
/// 设计目标:
/// - 面板渲染只读 `DataSnapshot`,与事件订阅解耦。
/// - 返回 `TuiError` 统一错误处理(§4.1:库层用 thiserror)。
pub trait TuiDataSource {
    /// 获取当前数据快照
    ///
    /// 实现者应返回最近一次聚合结果;若尚未收到任何事件,
    /// 返回默认空快照而非错误,保证面板始终可渲染。
    fn snapshot(&self) -> Result<DataSnapshot, TuiError>;

    /// 返回数据源配置
    fn config(&self) -> &DataSourceConfig;
}

/// Quest 同步器 — 从 NexusEvent 维护本地 Quest 列表
///
/// WHY 独立结构体:将事件→状态的转换逻辑隔离,`DataPipeline`(P1.3)
/// 可组合多个同步器生成统一快照,同时方便单元测试直接喂事件。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct QuestSync {
    quests: Vec<Quest>,
}

impl QuestSync {
    /// 创建空的 Quest 同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件影响 Quest 列表则返回更新后的列表副本
    ///
    /// - `QuestListUpdated`:替换整个列表(冷启动/lag 后对齐)。
    /// - `QuestCompleted`:按 quest_id 从活动列表移除。
    /// - 其他事件:返回 `None`,状态不变。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<Vec<Quest>> {
        match event {
            NexusEvent::QuestListUpdated { quests, .. } => {
                self.quests = quests.clone();
                Some(self.quests.clone())
            }
            NexusEvent::QuestCompleted { quest_id, .. } => {
                self.quests.retain(|q| q.quest_id != *quest_id);
                Some(self.quests.clone())
            }
            _ => None,
        }
    }

    /// 获取当前活动 Quest 列表副本
    pub fn quests(&self) -> Vec<Quest> {
        self.quests.clone()
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

/// 内存桩数据源 — 返回包含示例 Quest 与 Budget 数据的快照
///
/// WHY: TUI 默认启动时不强制要求真实 event-bus 连接；提供一个无依赖的
/// 桩实现，使 `TuiApp::new` 保持向后兼容，同时让 demo/stub 模式也能展示
/// 有意义的数据，而不是空面板。
#[derive(Debug, Default, Clone)]
pub struct StubDataSource {
    config: DataSourceConfig,
}

impl StubDataSource {
    /// 创建新的示例桩数据源
    pub fn new() -> Self {
        Self::default()
    }
}

impl TuiDataSource for StubDataSource {
    fn snapshot(&self) -> Result<DataSnapshot, TuiError> {
        let mut snapshot = DataSnapshot::default();

        // 提供一条示例 Quest，让默认启动的 Quest 面板不空载。
        snapshot.quest_list.push(Quest {
            quest_id: "stub-q1".into(),
            title: "Demo Quest".into(),
            tasks: vec![
                Task {
                    task_id: "stub-t1".into(),
                    description: "completed demo task".into(),
                    status: TaskStatus::Completed,
                    dependencies: vec![],
                },
                Task {
                    task_id: "stub-t2".into(),
                    description: "pending demo task".into(),
                    status: TaskStatus::Pending,
                    dependencies: vec![],
                },
            ],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
        });

        // 提供非零预算指标，让 Budget 面板展示进度条与状态。
        snapshot.budget_metrics = BudgetMetrics {
            total_consumption: 350.0,
            remaining_budget: 650.0,
            utilization_rate: 0.35,
            current_tier: "High".into(),
            coefficient: 1.0,
            is_exceeded: false,
            alert: None,
        };

        // 提供一条示例事件，让 Log / Parliament 面板不空载。
        snapshot.latest_events.push_back(NexusEvent::CacheHit {
            metadata: EventMetadata::new("stub"),
            cache_key: "demo".into(),
        });

        Ok(snapshot)
    }

    fn config(&self) -> &DataSourceConfig {
        &self.config
    }
}

/// 数据管道 — 后台聚合事件并生成统一快照
///
/// WHY:将事件订阅、去重、状态同步与快照生成封装为独立生命周期，
/// 让 TUI 主循环只读 `DataSnapshot`，不直接处理 event-bus 细节。
#[derive(Debug)]
pub struct DataPipeline {
    config: DataSourceConfig,
    // WHY Mutex<Option<JoinHandle>>: 支持 `shutdown(&self)` 被外部 Arc 持有方调用,
    // 无需 TuiApp 归还所有权即可清理后台任务。
    task: Mutex<Option<JoinHandle<()>>>,
    snapshot: Arc<Mutex<DataSnapshot>>,
}

impl DataPipeline {
    /// 创建数据管道并启动后台聚合任务
    ///
    /// # 参数
    /// - `subscriber`: 已订阅 event-bus 的事件订阅者
    /// - `config`: 数据源配置，包含 tick 间隔与容量限制
    pub fn new(mut subscriber: EventSubscriber, config: DataSourceConfig) -> Self {
        let snapshot = Arc::new(Mutex::new(DataSnapshot::default()));
        let snapshot_clone = Arc::clone(&snapshot);
        let tick_ms = config.tick_interval_ms;
        let max_event_history = config.max_event_history;
        let max_quest_list_size = config.max_quest_list_size;

        let task = tokio::spawn(async move {
            // WHY interval 而非 sleep:interval 会自动追钟，避免任务处理耗时导致 tick 漂移。
            let mut interval = time::interval(Duration::from_millis(tick_ms));
            let mut quest_sync = QuestSync::new();
            let mut budget_sync = BudgetSync::new();
            let mut latest_events: VecDeque<NexusEvent> = VecDeque::new();

            loop {
                // EventSubscriber 提供同步 try_recv，因此 select! 中仅 timer 分支驱动
                // 批量消费；没有 timer 时任务阻塞等待，避免 busy loop。
                tokio::select! {
                    _ = interval.tick() => {
                        // 批量取出当前缓冲区中的所有事件，一次性消费避免多次加锁。
                        let mut events = Vec::new();
                        while let Some(event) = subscriber.try_recv() {
                            events.push(event);
                        }

                        // 先定位同一 tick 内最后一个 QuestListUpdated 与 BudgetMetricsUpdated
                        // 的索引。仅这两个高频状态事件需要在状态更新层去重，日志流仍保留全部。
                        let mut last_quest_idx = None::<usize>;
                        let mut last_budget_idx = None::<usize>;
                        for (idx, event) in events.iter().enumerate() {
                            match event {
                                NexusEvent::QuestListUpdated { .. } => last_quest_idx = Some(idx),
                                NexusEvent::BudgetMetricsUpdated { .. } => last_budget_idx = Some(idx),
                                _ => {}
                            }
                        }

                        for (idx, event) in events.iter().enumerate() {
                            let is_deduped_quest = matches!(event, NexusEvent::QuestListUpdated { .. })
                                && Some(idx) != last_quest_idx;
                            let is_deduped_budget =
                                matches!(event, NexusEvent::BudgetMetricsUpdated { .. })
                                    && Some(idx) != last_budget_idx;

                            // 非去重状态事件才应用同步器；被去重的事件仍进入日志流。
                            if !is_deduped_quest && !is_deduped_budget {
                                quest_sync.apply_event(event);
                                budget_sync.apply_event(event);
                            }
                            latest_events.push_back(event.clone());
                        }

                        // 限制事件日志长度，防止内存无限增长。
                        while latest_events.len() > max_event_history {
                            latest_events.pop_front();
                        }

                        let snap = DataSnapshot {
                            quest_list: truncate_quests(quest_sync.quests(), max_quest_list_size),
                            latest_events: latest_events.clone(),
                            budget_metrics: budget_sync.metrics(),
                        };
                        let mut guard = snapshot_clone.lock().unwrap_or_else(|e| e.into_inner());
                        *guard = snap;
                    }
                }
            }
        });

        Self {
            config,
            task: Mutex::new(Some(task)),
            snapshot,
        }
    }

    /// 非阻塞读取当前快照
    pub fn snapshot(&self) -> DataSnapshot {
        let guard = self.snapshot.lock().unwrap_or_else(|e| e.into_inner());
        guard.clone()
    }

    /// 返回数据源配置
    pub fn config(&self) -> &DataSourceConfig {
        &self.config
    }

    /// 关闭数据管道，中止并等待后台任务结束
    ///
    /// 取 `&self` 使外部 Arc 持有方可在不回收所有权的情况下清理后台任务。
    pub async fn shutdown(&self) {
        // 取出 JoinHandle 所有权后再 abort + await，避免 `&self` 无法消费 handle。
        let Some(handle) = self.task.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return;
        };
        // abort 唤醒可能正在 interval.tick() 上等待的任务，再 await 确保资源释放，
        // 避免 orphan task(§4.4 反模式 #7)。
        handle.abort();
        let _ = handle.await;
    }
}

impl TuiDataSource for DataPipeline {
    fn snapshot(&self) -> Result<DataSnapshot, TuiError> {
        Ok(DataPipeline::snapshot(self))
    }

    fn config(&self) -> &DataSourceConfig {
        DataPipeline::config(self)
    }
}

// WHY Arc<DataPipeline>: CLI 需要保留 `pipeline` 变量以便在 TUI 退出后调用
// `pipeline.shutdown().await`，同时把数据源的共享引用交给 `TuiApp`。
impl TuiDataSource for Arc<DataPipeline> {
    fn snapshot(&self) -> Result<DataSnapshot, TuiError> {
        Ok(DataPipeline::snapshot(self))
    }

    fn config(&self) -> &DataSourceConfig {
        DataPipeline::config(self)
    }
}

/// 辅助函数：截断 quest 列表至配置上限
///
/// WHY 单独函数:DataSnapshot 只保留面板展示所需前 N 个 quest，
/// 同时让 QuestSync 保持完整语义，便于未来按优先级排序后截断。
fn truncate_quests(quests: Vec<Quest>, max: usize) -> Vec<Quest> {
    let mut quests = quests;
    quests.truncate(max);
    quests
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::{BudgetMetricsPayload, EventMetadata, QuestStatus};
    use nexus_core::{Task, TaskStatus, ThinkingMode};

    /// 构造测试用 Quest
    fn quest(id: &str, title: &str) -> Quest {
        Quest {
            quest_id: id.into(),
            title: title.into(),
            tasks: vec![Task {
                task_id: format!("{id}-t1"),
                description: "test task".into(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            }],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
        }
    }

    /// 构造 QuestListUpdated 事件
    fn quest_list_event(quests: Vec<Quest>) -> NexusEvent {
        NexusEvent::QuestListUpdated {
            metadata: EventMetadata::new("quest-engine"),
            quests,
            source: "quest-engine".into(),
        }
    }

    /// 构造 QuestCompleted 事件
    fn quest_completed_event(quest_id: &str, status: QuestStatus) -> NexusEvent {
        NexusEvent::QuestCompleted {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: quest_id.into(),
            status,
        }
    }

    /// 构造 BudgetMetricsUpdated 事件
    fn budget_metrics_event(metrics: BudgetMetrics) -> NexusEvent {
        NexusEvent::BudgetMetricsUpdated {
            metadata: EventMetadata::new("efficiency-monitor"),
            metrics: BudgetMetricsPayload {
                total_consumption: metrics.total_consumption,
                remaining_budget: metrics.remaining_budget,
                utilization_rate: metrics.utilization_rate,
                current_tier: metrics.current_tier,
                coefficient: metrics.coefficient,
                is_exceeded: metrics.is_exceeded,
                alert: metrics.alert,
            },
        }
    }

    #[test]
    fn test_data_snapshot_default_empty() {
        let snap = DataSnapshot::default();
        assert!(snap.quest_list.is_empty());
        assert!(snap.latest_events.is_empty());
        assert_eq!(snap.budget_metrics.utilization_rate, 0.0);
    }

    #[test]
    fn test_budget_metrics_default() {
        let bm = BudgetMetrics::default();
        assert!(!bm.is_exceeded);
        assert_eq!(bm.current_tier, "High");
        assert_eq!(bm.coefficient, 1.0);
    }

    #[test]
    fn test_data_source_config_default() {
        let cfg = DataSourceConfig::default();
        assert_eq!(cfg.max_event_history, 256);
        assert_eq!(cfg.max_quest_list_size, 64);
        assert_eq!(cfg.budget_metrics_ttl_ms, 5000);
        assert_eq!(cfg.tick_interval_ms, 250);
    }

    #[test]
    fn test_quest_sync_list_updated_replaces_list() {
        let mut sync = QuestSync::new();
        let q1 = quest("q1", "first");
        let q2 = quest("q2", "second");

        let updated = sync.apply_event(&quest_list_event(vec![q1.clone(), q2.clone()]));
        assert_eq!(updated, Some(vec![q1.clone(), q2.clone()]));
        assert_eq!(sync.quests(), vec![q1, q2]);
    }

    #[test]
    fn test_quest_sync_completed_removes_quest() {
        let mut sync = QuestSync::new();
        let q1 = quest("q1", "first");
        let q2 = quest("q2", "second");
        sync.apply_event(&quest_list_event(vec![q1.clone(), q2.clone()]));

        let updated = sync.apply_event(&quest_completed_event("q1", QuestStatus::Completed));
        assert_eq!(updated, Some(vec![q2.clone()]));
        assert_eq!(sync.quests(), vec![q2]);
    }

    #[test]
    fn test_quest_sync_unrelated_event_unchanged() {
        let mut sync = QuestSync::new();
        let q1 = quest("q1", "first");
        sync.apply_event(&quest_list_event(vec![q1.clone()]));

        let unrelated = NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        };
        let result = sync.apply_event(&unrelated);
        assert!(result.is_none());
        assert_eq!(sync.quests(), vec![q1]);
    }

    #[test]
    fn test_budget_sync_metrics_updated() {
        let mut sync = BudgetSync::new();
        let metrics = BudgetMetrics {
            total_consumption: 8000.0,
            remaining_budget: 2000.0,
            utilization_rate: 0.8,
            current_tier: "Medium".into(),
            coefficient: 0.8,
            is_exceeded: false,
            alert: Some("approaching limit".into()),
        };

        let updated = sync.apply_event(&budget_metrics_event(metrics.clone()));
        assert_eq!(updated, Some(metrics.clone()));
        assert_eq!(sync.metrics(), metrics);
    }

    #[test]
    fn test_budget_sync_unrelated_event_unchanged() {
        let mut sync = BudgetSync::new();
        let metrics = BudgetMetrics {
            total_consumption: 5000.0,
            remaining_budget: 5000.0,
            utilization_rate: 0.5,
            current_tier: "High".into(),
            coefficient: 1.0,
            is_exceeded: false,
            alert: None,
        };
        sync.apply_event(&budget_metrics_event(metrics.clone()));

        let unrelated = NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        };
        let result = sync.apply_event(&unrelated);
        assert!(result.is_none());
        assert_eq!(sync.metrics(), metrics);
    }
}
