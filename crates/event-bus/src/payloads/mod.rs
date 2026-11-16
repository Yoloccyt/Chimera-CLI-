//! 事件载荷与辅助类型定义 — NexusEvent 枚举依赖的结构化数据类型
//!
//! 对应架构:L1 Core 事件总线载荷层
//!
//! # 域下沉(ADR-314 W6 / BC-03)
//!
//! 13 个载荷类型按 topic 域下沉到 5 个子模块,本文件只作 facade:
//!
//! | 子模块 | 域 | 类型 |
//! |---|---|---|
//! | `quest` | 议会 / Quest | `QuestStatus`、`VoteValue` |
//! | `chat` | TUI / Agent 交互 | `TuiChatMessagePayload`、`ConsultUrgency`、`ActionSource`、`ChatStatus`、`CompatLevel` |
//! | `metrics` | 面板结构化指标 | `BudgetMetricsPayload`、`RouterStatsPayload`、`ClvSummary` |
//! | `evolution` | 进化 / 回滚诊断 | `RollbackTriggerType`、`RollbackDiagnosticContext` |
//! | `critical` | Critical 通道指标 | `CriticalEventDropped` |
//!
//! # 契约零变更
//!
//! - 子模块为**私有** `mod`（真正的下沉:域内路径不外泄);
//! - 全部类型经本文件的 `pub use` 逐一转出 ⇒ `event_bus::payloads::X`
//!   路径**逐字不变**;
//! - `types.rs` 的 `pub use crate::payloads::*;`（glob）与 crate 根
//!   `pub use types::{...}` 均继续覆盖全部类型。
//!
//! # 类型分类
//! - **元数据**: [`EventMetadata`]、[`EventSeverity`] — 每个事件携带的通用追踪信息
//! - **载荷结构体**: [`BudgetMetricsPayload`]、[`RouterStatsPayload`]、[`ClvSummary`]
//!   — 事件携带的结构化业务数据
//! - **枚举标签**: [`QuestStatus`]、[`VoteValue`]、[`TaskPriority`]、[`ConsultUrgency`]、
//!   [`AgentStatus`]、[`ActionSource`]、[`ChatStatus`] — 事件变体中的轻量级分类枚举
//! - **回滚诊断**: [`RollbackTriggerType`]、[`RollbackDiagnosticContext`] — ADR-043 回滚决策
//! - **指标**: [`CriticalEventDropped`] — Critical 通道丢弃计数

mod chat;
mod critical;
mod evolution;
mod metrics;
mod quest;

// Task 3.10: EventMetadata 已下沉至 L0 nexus-contracts(ADR-033 扩展)
// WHY re-export: 保持向后兼容,100+ 文件现有 `use event_bus::EventMetadata` 路径不破坏
// 类型 + impl(new())均来自 nexus-contracts,re-export 完整保留构造方法
pub use nexus_contracts::EventMetadata;

// ADR-054 决策 6(P9-T7 Task 4):EventSeverity/TaskPriority/AgentStatus 已下沉 L0
// WHY re-export(原位替换本地定义):外部 `use event_bus::{EventSeverity, TaskPriority,
// AgentStatus}` 路径保持兼容,无需改 100+ 消费方;变体/derive 与 L0
// `nexus_contracts::event_payload` 定义逐字一致(线格式冻结,跨进程契约不破坏)。
// severity() 判定逻辑仍留在本 crate(架构红线:Critical 事件 mpsc 保障,见 types.rs)。
pub use nexus_contracts::event_payload::{AgentStatus, EventSeverity, TaskPriority};

pub use chat::{ActionSource, ChatStatus, CompatLevel, ConsultUrgency, TuiChatMessagePayload};
pub use critical::CriticalEventDropped;
pub use evolution::{RollbackDiagnosticContext, RollbackTriggerType};
pub use metrics::{BudgetMetricsPayload, ClvSummary, RouterStatsPayload};
pub use quest::{QuestStatus, VoteValue};
