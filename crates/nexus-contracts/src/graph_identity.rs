//! 图身份契约 — GIP（Graph Identity Propagation）跨层成本归因三元组（WI-04）
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts）
//! 对应工作项: **WI-04 GIP 图身份传播**（v4.0 统一执行总案 §13）
//! 对应设计源: Graph Engineering 企业实践（graph_id/run_id/node_id 全链传播，
//!             TrueFoundry / Andrew Ng playbook, 2026-07/08）+ Qwen3.8-Max
//!             preserve_thinking"痕迹一等公民"
//!
//! # 核心职责
//!
//! 承载事件流中每个 Goal/节点级的**图身份三元组**，使任意 Goal/节点成本可归因：
//!
//! | 字段 | 语义 | 示例 |
//! |------|------|------|
//! | `goal_id` | 长期目标（Quest）ID | "quest-7f3a..." |
//! | `run_id` | 单次执行（run/session）ID | "run-0c2e..." |
//! | `node_id` | 目标内节点（step/tool-call）ID | "node-91ab..." |
//!
//! # 设计约束（ADR-033 + WI-04）
//!
//! - **纯类型零逻辑**: 仅类型定义与构造辅助（无 IO 无状态变更）
//! - **可选传播**: `EventMetadata.graph_identity` 为 `Option<GraphIdentity>`，
//!   渐进铺开——既有事件无身份字段不破坏序列化（`skip_serializing_if`）
//! - **不可变承载**: 构造后字段不可变更（身份一旦落档不可篡改，
//!   与 `SegmentMetadata.parent_traj_id` 铁律9 同源精神）
//! - **144 事件枚举本体不动**: 本类型仅作为 `EventMetadata` 的扩展字段承载，
//!   不新增事件变体、不改 severity() 权威源（v4.0 §17 治理红线）

use serde::{Deserialize, Serialize};

/// 图身份三元组 — 事件流中的 Goal/run/node 归因标识（WI-04）
///
/// # WHY 三元组
/// - 成本归因从"总账"细化到"任意 Goal/节点瀑布"（超支定位从小时级到分钟级）
/// - 与 `TokenLedgerEntry` 按三元组聚合（账本聚合 API），支撑 WI-04 验收
///   "给定 run_id 拉出完整成本瀑布"
/// - `Box<str>` 定长只读语义（token_evidence.rs 先例）
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GraphIdentity {
    /// 长期目标（Quest）ID — 跨 run 共享
    pub goal_id: Box<str>,
    /// 单次执行（run/session）ID — 同 goal 下可多次执行
    pub run_id: Box<str>,
    /// 目标内节点（step/tool-call）ID — 同 run 下逐节点递增
    pub node_id: Box<str>,
}

impl GraphIdentity {
    /// 创建图身份三元组
    ///
    /// # Panics
    ///
    /// 任一字段为空时 panic —— 归因身份不变量：三元组必须完整，
    /// 缺失维度会导致成本瀑布断链（WI-04 验收依赖全链传播）。
    pub fn new(goal_id: &str, run_id: &str, node_id: &str) -> Self {
        assert!(
            !goal_id.is_empty() && !run_id.is_empty() && !node_id.is_empty(),
            "GraphIdentity 不变量: goal_id/run_id/node_id 均不得为空（WI-04 归因完整性）"
        );
        Self {
            goal_id: Box::from(goal_id),
            run_id: Box::from(run_id),
            node_id: Box::from(node_id),
        }
    }

    /// 提升身份粒度：goal + run 确定后，为节点创建带 node_id 的身份
    ///
    /// # Panics
    ///
    /// `node_id` 为空时 panic（同 [`Self::new`] 不变量）。
    pub fn with_node(&self, node_id: &str) -> Self {
        assert!(
            !node_id.is_empty(),
            "GraphIdentity 不变量: node_id 不得为空（WI-04 归因完整性）"
        );
        Self {
            goal_id: self.goal_id.clone(),
            run_id: self.run_id.clone(),
            node_id: Box::from(node_id),
        }
    }

    /// 生成三元组聚合键 — 供 `TokenLedger` 按图身份聚合成本瀑布
    ///
    /// 格式: `goal_id::run_id::node_id`（`::` 分隔，与事件 ID 惯例一致）
    pub fn aggregate_key(&self) -> String {
        format!("{}::{}::{}", self.goal_id, self.run_id, self.node_id)
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_identity_json_roundtrip() {
        let gi = GraphIdentity::new("goal-1", "run-1", "node-1");
        let json = serde_json::to_string(&gi).expect("JSON 序列化失败");
        let decoded: GraphIdentity = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, gi);
    }

    #[test]
    fn graph_identity_wire_format_frozen() {
        let gi = GraphIdentity::new("goal-1", "run-1", "node-1");
        let json = serde_json::to_string(&gi).expect("JSON 序列化失败");
        assert!(json.contains("\"goal_id\":\"goal-1\""));
        assert!(json.contains("\"run_id\":\"run-1\""));
        assert!(json.contains("\"node_id\":\"node-1\""));
    }

    #[test]
    fn graph_identity_with_node_keeps_prefix() {
        let gi = GraphIdentity::new("goal-1", "run-1", "node-1");
        let child = gi.with_node("node-2");
        assert_eq!(child.goal_id.as_ref(), "goal-1");
        assert_eq!(child.run_id.as_ref(), "run-1");
        assert_eq!(child.node_id.as_ref(), "node-2");
    }

    #[test]
    fn graph_identity_aggregate_key() {
        let gi = GraphIdentity::new("goal-1", "run-1", "node-1");
        assert_eq!(gi.aggregate_key(), "goal-1::run-1::node-1");
    }

    #[test]
    fn graph_identity_empty_field_asserted() {
        // 不变量: 任一字段为空必须 panic（归因完整性）
        let result = std::panic::catch_unwind(|| GraphIdentity::new("goal-1", "", "node-1"));
        assert!(result.is_err(), "空 run_id 必须触发断言 panic");

        let result = std::panic::catch_unwind(|| GraphIdentity::new("", "run-1", "node-1"));
        assert!(result.is_err(), "空 goal_id 必须触发断言 panic");

        let gi = GraphIdentity::new("goal-1", "run-1", "node-1");
        let result = std::panic::catch_unwind(|| gi.with_node(""));
        assert!(result.is_err(), "空 node_id 必须触发断言 panic");
    }
}
