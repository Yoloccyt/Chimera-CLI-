//! 长期任务引擎 — Quest 分解、检查点持久化与思考模式治理
//!
//! 对应架构层:L9 Quest
//! 对应创新点:TTG(Thinking Toggle Governance)+ LHQP(Long-Horizon Quest Persistence)
//!
//! # 核心职责
//! - 从 `UserIntent` 分解任务图(DAG),校验无环后创建 Quest
//! - 维护 Task 状态机(Pending→Running→Completed/Failed),广播进度事件
//! - TTG 自动思考模式治理:基于 Quest 复杂度与 DECB 预算档位自动选择 Fast/Standard/Deep
//! - 模式切换通过 EventBus 发布 ThinkingModeSwitched 事件,供 Parliament 等下游订阅
//! - 完成 Quest 时广播 ExecutionCompleted 事件
//! - LHQP 检查点持久化:Quest 状态序列化为 MessagePack 落盘,崩溃后可恢复
//!
//! # 快速示例
//! ```
//! use event_bus::EventBus;
//! use nexus_core::{UserIntent, MultimodalInput};
//! use quest_engine::QuestEngine;
//!
//! # async fn run() {
//! let bus = EventBus::new();
//! let engine = QuestEngine::new(bus);
//! let intent = UserIntent {
//!     intent_id: "i-1".into(),
//!     raw_text: "分析需求。设计方案。".into(),
//!     multimodal_inputs: vec![MultimodalInput::Text("...".into())],
//!     risk_level: 10,
//! };
//! let quest = engine.create_quest(intent).await.unwrap();
//! assert_eq!(quest.tasks.len(), 2);
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod ambient_mode;
pub mod arbitration;
pub mod checkpoint;
pub mod config;
pub mod control;
pub mod coordination_metrics;
pub mod dag;
pub mod engine;
pub mod error;
/// Phase 9 §14.2: 长任务地图（TencentDB 机制：短摘要入上下文 + 详情外置 + 地图注入）
pub mod long_task_map;
/// Phase 9 二次审查增强 §14.4: 长时程信用分配器（SHARP 统计版时间维度）
pub mod long_term_credit;
/// Phase 9 二次审查增强: L2 记忆层协同接口（MemorySyncHook 依赖倒置）
pub mod memory_sync_hook;
pub mod metrics_sync;
/// Phase 9 §14.1: OpenMLE 搜索树管理器（经验卡片进化树，与 dag.rs 任务 DAG 语义分离）
pub mod search_tree;
/// P1-1: NexusStateChanged EventBus 适配器(L1 Core 深度优化,依赖倒置接线)
pub mod state_listener;
pub mod trajectory_exporter;
pub mod ttg;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use arbitration::ArbitrationLayer;
// Milestone B-2: Ambient Mode（后台常驻守护循环:资源看门狗/记忆整理/检查点调度）
pub use ambient_mode::{
    spawn_ambient_subscriber, spawn_ambient_subscriber_with_receivers, AmbientModeConfig,
    MemoryTidyHook, NoopTidyHook,
};
pub use checkpoint::CheckpointManager;
pub use config::QuestConfig;
// P2-1: 协调成本/推理增益比值度量(三重悖论推理悖论红线)
pub use control::{
    handle_control_event, spawn_control_subscriber, spawn_control_subscriber_with_receiver,
};
pub use coordination_metrics::{
    CoordinationCostSample, CoordinationMetricsCollector, CoordinationMetricsConfig,
    CoordinationToGainRatio, InferenceGainSample,
};
pub use engine::QuestEngine;
pub use error::QuestError;
// Phase 9 §14.2: 长任务地图公开 API 重导出
pub use long_task_map::{
    ExternalStorage, InMemoryExternalStorage, LongTaskMap, NodeStatus, StepResult, TaskEdge,
    TaskMapRef, TaskNode,
};
// Phase 9 §14.1: OpenMLE 搜索树公开 API 重导出
pub use search_tree::{SearchTreeManager, TreeError, TreeStats};
// Phase 9 二次审查增强 §14.4: 长时程信用分配器公开 API 重导出
pub use long_term_credit::{CreditAssignment, CreditStep, LongTermCreditAssigner};
// Phase 9 二次审查增强: L2 记忆层协同接口公开 API 重导出
pub use memory_sync_hook::{MemorySyncHook, NoopMemorySyncHook};
// 协调度量接线闭环:待合并样本与观测事件订阅器
// (消费 DebateCompleted / DelegationCompleted,填充协调成本 Option 字段)
pub use metrics_sync::{
    spawn_metrics_subscriber, spawn_metrics_subscriber_with_receiver, PendingCoordSample,
};
// P1-1: NexusState 状态变更 → NexusStateChanged 事件适配器
pub use state_listener::{BusStateChangeListener, STATE_LISTENER_SOURCE};
// P4-W16.1.3: Quest 轨迹导出器类型重导出(供 L5 omega-learner / L9 efficiency-monitor 使用)
pub use trajectory_exporter::{
    export_trajectory, export_trajectory_from_quest, ContextSummary, QuestTrajectory, TaskProgress,
    TrajectoryAction, TrajectoryReward, TrajectoryState,
};
pub use ttg::{ComplexityScore, ModeSwitchReason, TtgConfig, TtgGovernor};
pub use types::{CheckpointMeta, TaskResult};

/// 默认 Quest 配置
///
/// 生产推荐值:
/// - max_tasks_per_quest: 16(与 GQEP 批处理窗口对齐)
/// - checkpoint_interval: 3(每 3 个 Task 完成触发检查点)
pub fn default_config() -> QuestConfig {
    QuestConfig::default()
}
