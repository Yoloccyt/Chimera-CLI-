//! 事件类型定义 — NEXUS-OMEGA 全维事件枚举
//!
//! 对应架构:十层架构 L1-L10 的跨层通信契约
//! 设计依据:Part A 依赖方向分析,通过预定义事件类型修正 4 处违规
//!
//! # 关键违规修正映射
//! - V1(OSA→HCW 向上依赖):`OmniSparseMasksComputed` 事件
//! - V2(MLC→efficiency-monitor 跨层):`MemoryMetricsReported` 事件
//! - V3/V4(Parliament→GSOE/AutoDPO 向上依赖):`ConsensusReached` 事件
//!
//! # 变体分组(ADR-322 迁出 event-bus 时按域切分)
//!
//! 155 个变体按语义域拆到 7 个子模块,本文件只做三件事:导入、装配、声明枚举。
//!
//! | 子模块 | 段首 → 段尾 |
//! |---|---|
//! | [`contract`] | `UserIntentEncoded` → `TelemetryReporterDegraded` |
//! | [`lifecycle`] | `NexusStateChanged` → `SandboxViolation` |
//! | [`execution`] | `OperationProduced` → `ExpertUnregistered` |
//! | [`governance`] | `DebateStarted` → `QuestPriorityAdjusted` |
//! | [`interface`] | `DecayMetricsReported` → `TuiHelloAck` |
//! | [`evolution`] | `R1ShadowRegressionDetected` → `ParliamentStrategyCapChanged` |
//! | [`runtime`] | `ModelAffinitySelected` → `SecurityInterceptionReported` |
//!
//! **声明顺序即变体序数**:子模块按上表顺序串成一条宏链,展开后仍是
//! 原来那个单一 `NexusEvent` 枚举,序数、serde 线格式、`severity()` 穷尽
//! match 全部逐字不变。组装方式是令牌累加链(WHY 见 `contract.rs` 模块文档):
//! Rust 不允许跨文件拆分 enum 定义,也不允许宏在枚举变体位展开。

// 辅助载荷类型从 `payloads` 模块导入,保持向后兼容
pub use crate::payloads::*;

use chrono::{DateTime, Utc};
// ADR-054 决策 6(P9-T7 Task 4):Quest 引用改从 L0 nexus-contracts 导入,
// 消除 event-bus 对 nexus-core 的 Quest 依赖(边解除的一部分)
use nexus_contracts::domain::Quest;
use serde::{Deserialize, Serialize};

mod contract;
mod evolution;
mod execution;
mod governance;
mod interface;
mod lifecycle;
mod runtime;

/// 变体分组 → 枚举的落成宏(装配链的最后一环)。
///
/// 前六段宏按声明序把变体令牌累加进 `$($acc)*`,最后由本宏一次性落成
/// `NexusEvent`。derive 与 serde 属性写在这里而不是散在各段,保证线格式
/// (`tag = "type", content = "data"`)只有一个声明点。
macro_rules! define_nexus_event {
    ($($variants:tt)*) => {
        /// NEXUS-OMEGA 核心事件枚举 — 跨层通信的唯一契约
        ///
        /// 设计原则:
        /// 1. 每个变体对应一条架构层间的数据流(见 §5.2 数据流参考)
        /// 2. 变体命名采用"动作完成时态"(PastTense),表达"已发生"事实
        /// 3. payload 仅携带消费者必需字段,大对象用 hash 引用
        /// 4. 关键事件在文档中标注 `[Critical]`,背压策略据此保护
        #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
        #[serde(tag = "type", content = "data")]
        pub enum NexusEvent {
            $($variants)*
        }
    };
}
pub(crate) use define_nexus_event;

crate::types::contract::variants_contract!();

/// §(b.1) 裁决权外置·消费辅助:供 UI/下游**只读**提取 `CertifyIssued` 的
/// `claim_ref`,无需在允许白名单外的 crate 里拼写
/// `NexusEvent::CertifyIssued { field }`(那会被 Certify 白名单门误判为
/// 构造/签发)。ADR-322 随事件目录自 `event-bus/src/bus/mod.rs` 迁入本文件
/// —— 它是纯只读访问器,不依赖任何运行时状态。非 Certify 事件返回 `None`。
impl NexusEvent {
    /// 若本事件为 `CertifyIssued`,返回其被认证的 `claim_ref`。
    #[must_use]
    pub fn certify_claim_ref(&self) -> Option<&str> {
        match self {
            NexusEvent::CertifyIssued { claim_ref, .. } => Some(claim_ref),
            _ => None,
        }
    }
}
