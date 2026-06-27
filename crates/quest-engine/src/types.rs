//! Quest Engine 内部类型 — 任务执行结果与检查点元数据
//!
//! 对应架构层:L9 Quest
//!
//! # 设计决策(WHY)
//! - `TaskResult`:Week 2 阶段保持最小化,仅记录执行产出哈希,
//!   后续阶段扩展为携带工具调用链、Wiki 增量等供 L5 Knowledge 层消费
//! - `CheckpointMeta`:检查点轻量元数据,完整状态存储在 Checkpoint 结构中

use serde::{Deserialize, Serialize};

/// 任务执行结果 — 单个 Task 完成后的产出摘要
///
/// WHY:Week 2 阶段仅记录产出哈希,后续阶段扩展为携带:
/// - 工具调用链(供 FaaE 路由学习)
/// - Wiki 增量条目(供 ISCM 索引)
/// - DPO 训练对 ID(供 AutoDPO 消费)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskResult {
    /// 所属 Task ID
    pub task_id: String,
    /// 产出内容哈希(SHA-256 hex),空字符串表示无产出
    pub result_hash: String,
    /// 是否成功完成
    pub success: bool,
}

impl TaskResult {
    /// 创建新任务结果
    pub fn new(task_id: impl Into<String>, result_hash: impl Into<String>, success: bool) -> Self {
        Self {
            task_id: task_id.into(),
            result_hash: result_hash.into(),
            success,
        }
    }
}

/// 检查点元数据 — 用于内存索引,完整状态见 `nexus_core::Checkpoint`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckpointMeta {
    /// 所属 Quest ID
    pub quest_id: String,
    /// 检查点 ID
    pub checkpoint_id: String,
    /// 创建时间戳(UTC ISO 8601)
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_result_new() {
        let r = TaskResult::new("t-1", "abc123", true);
        assert_eq!(r.task_id, "t-1");
        assert_eq!(r.result_hash, "abc123");
        assert!(r.success);
    }

    #[test]
    fn test_task_result_serde() {
        let r = TaskResult::new("t-1", "hash", false);
        let json = serde_json::to_string(&r).unwrap();
        let de: TaskResult = serde_json::from_str(&json).unwrap();
        assert_eq!(de, r);
    }

    #[test]
    fn test_checkpoint_meta_serde() {
        let m = CheckpointMeta {
            quest_id: "q-1".into(),
            checkpoint_id: "c-1".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&m).unwrap();
        let de: CheckpointMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(de, m);
    }
}
