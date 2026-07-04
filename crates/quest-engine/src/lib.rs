//! 长期任务引擎 — Quest 分解、检查点持久化与思考模式切换
//!
//! 对应架构层:L9 Quest
//! 对应创新点:TTG(Thinking Toggle Governance)+ LHQP(Long-Horizon Quest Persistence)
//!
//! # 核心职责
//! - 从 `UserIntent` 分解任务图(DAG),校验无环后创建 Quest
//! - 维护 Task 状态机(Pending→Running→Completed/Failed),广播进度事件
//! - 切换思考模式(TTG),广播模式切换事件供 Parliament 调整预算
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

pub mod checkpoint;
pub mod config;
pub mod dag;
pub mod engine;
pub mod error;
pub mod ttg;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use checkpoint::CheckpointManager;
pub use config::QuestConfig;
pub use engine::QuestEngine;
pub use error::QuestError;
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
