//! 核心领域类型 — NEXUS-OMEGA 全局领域模型
//!
//! 对应架构层:L1 Core(被 L2-L10 所有上层 crate 依赖)
//! 对应创新点:CLV(Context Latent Vector)、MLC(多级记忆)、TTG(思考切换)
//!
//! # 类型职责
//! - `UserIntent`:用户意图编码,含多模态输入与风险等级
//! - `Quest`:长期任务,含任务列表与思考模式
//! - `Task`:任务节点,含状态与依赖
//! - `Checkpoint`:检查点,用于 Quest 断点恢复
//! - `ThinkingMode`:TTG 三级思考模式(Fast/Standard/Deep)

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 用户意图 — NMC 编码后的多模态用户输入
///
/// `risk_level` 范围 0-100,影响后续沙箱策略:
/// - 0-30:低风险,只读操作
/// - 31-70:中风险,有副作用但可控
/// - 71-100:高风险,需 Parliament 审议
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserIntent {
    /// 意图唯一标识(UUIDv7,时间有序)
    pub intent_id: String,
    /// 用户输入原始文本
    pub raw_text: String,
    /// 多模态输入列表(Week 2 仅 Text 变体,Week 6 扩展 Image/Video/Audio)
    pub multimodal_inputs: Vec<MultimodalInput>,
    /// 风险等级(0-100),影响沙箱策略与议会审议门槛
    pub risk_level: u8,
}

/// 多模态输入枚举 — 支持文本、图像、视频、音频
///
/// WHY:Week 2 阶段仅实现 Text 变体,但提前定义完整枚举
/// 以避免后续扩展时破坏序列化兼容性(serde tag 已固定)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MultimodalInput {
    /// 文本输入(Week 2 唯一实现的变体)
    Text(String),
    // Week 6 扩展:
    // Image(Vec<u8>),
    // Video(Vec<u8>),
    // Audio(Vec<u8>),
}

/// 任务状态 — Task 生命周期
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskStatus {
    /// 待执行:尚未开始
    Pending,
    /// 执行中:已启动但未完成
    Running,
    /// 已完成:成功结束
    Completed,
    /// 已失败:执行出错或被中止
    Failed,
}

/// 思考模式 — TTG(Thinking Toggle Governance)三级切换
///
/// Parliament 根据 Quest 复杂度与预算动态切换:
/// - `Fast`:简单任务,快速响应(如查询、格式化)
/// - `Standard`:常规任务,平衡速度与深度
/// - `Deep`:复杂任务,深度推理(如架构设计、调试)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ThinkingMode {
    /// 快速模式:低延迟,适合简单任务
    Fast,
    /// 标准模式:平衡,适合常规任务
    Standard,
    /// 深度模式:高延迟高深度,适合复杂任务
    Deep,
}

/// 任务节点 — Quest 中的单个执行单元
///
/// `dependencies` 存储前置 Task ID 列表,支持 DAG 依赖图。
/// GQEP 执行器据此拓扑排序,确保依赖先完成。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Task {
    /// 任务唯一标识
    pub task_id: String,
    /// 任务描述(自然语言)
    pub description: String,
    /// 当前状态
    pub status: TaskStatus,
    /// 前置 Task ID 列表(空表示无依赖,可立即执行)
    pub dependencies: Vec<String>,
}

/// 长期任务 — 用户意图分解后的多步骤执行计划
///
/// 由 Quest Engine 从 `UserIntent` 分解而来,经 Parliament 审议后执行。
/// `checkpoint_id` 指向最近一次检查点,支持断点恢复(LHQP)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Quest {
    /// Quest 唯一标识
    pub quest_id: String,
    /// Quest 标题(人类可读)
    pub title: String,
    /// 任务列表(DAG 结构,通过 Task.dependencies 表达)
    pub tasks: Vec<Task>,
    /// 思考模式(TTG),影响执行深度与延迟
    pub thinking_mode: ThinkingMode,
    /// 最近检查点 ID(无检查点时为 None)
    pub checkpoint_id: Option<String>,
}

/// 检查点 — Quest 执行状态的持久化快照
///
/// WHY:`serialized_state` 存储 MessagePack 序列化的 Quest 状态,
/// 而非直接存储 Quest 结构,以支持版本演进(字段增减不破坏旧检查点)。
/// `memory_snapshot_hash` 用于恢复时校验完整性,防止状态漂移。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Checkpoint {
    /// 所属 Quest ID
    pub quest_id: String,
    /// 检查点唯一标识
    pub checkpoint_id: String,
    /// 记忆快照哈希(SHA-256 hex),恢复时校验完整性
    pub memory_snapshot_hash: String,
    /// MessagePack 序列化的 Quest 状态(版本无关的持久化表示)
    pub serialized_state: Vec<u8>,
    /// 创建时间(UTC,自动生成)
    pub created_at: DateTime<Utc>,
}

impl Checkpoint {
    /// 创建新检查点,`created_at` 自动设为当前 UTC 时间
    pub fn new(
        quest_id: impl Into<String>,
        checkpoint_id: impl Into<String>,
        memory_snapshot_hash: impl Into<String>,
        serialized_state: Vec<u8>,
    ) -> Self {
        Self {
            quest_id: quest_id.into(),
            checkpoint_id: checkpoint_id.into(),
            memory_snapshot_hash: memory_snapshot_hash.into(),
            serialized_state,
            created_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_status_serde() {
        let status = TaskStatus::Running;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"Running\"");
        let de: TaskStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(de, status);
    }

    #[test]
    fn test_thinking_mode_serde() {
        let mode = ThinkingMode::Deep;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"Deep\"");
        let de: ThinkingMode = serde_json::from_str(&json).unwrap();
        assert_eq!(de, mode);
    }

    #[test]
    fn test_multimodal_input_text_variant() {
        let input = MultimodalInput::Text("hello".into());
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("Text"));
        assert!(json.contains("hello"));
    }

    #[test]
    fn test_checkpoint_new_auto_timestamp() {
        let before = Utc::now();
        let cp = Checkpoint::new("q1", "c1", "hash123", vec![1, 2, 3]);
        let after = Utc::now();
        assert!(cp.created_at >= before);
        assert!(cp.created_at <= after);
    }
}
