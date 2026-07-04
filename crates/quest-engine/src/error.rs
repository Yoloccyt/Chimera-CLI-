//! Quest Engine 错误类型 — 库层 thiserror enum
//!
//! 遵循 §4.1:库层用自定义 thiserror enum,应用层才用 anyhow
//! 对应架构层:L9 Quest

use thiserror::Error;

/// Quest Engine 错误枚举 — 覆盖 DAG 校验、任务生命周期、检查点与事件总线
#[derive(Debug, Error)]
pub enum QuestError {
    /// 任务图存在循环依赖 — Kahn 算法检测到剩余节点无法消解
    #[error("cyclic dependency detected in task graph")]
    CyclicDependency,

    /// 任务不存在 — 按 task_id 查找时未找到
    #[error("task not found: {0}")]
    TaskNotFound(String),

    /// Quest 不存在 — 按 quest_id 查找时未找到
    #[error("quest not found: {0}")]
    QuestNotFound(String),

    /// 非法状态转换 — 违反 Pending→Running→Completed/Failed 单向流转
    #[error("invalid task status transition: {0}")]
    InvalidStatus(String),

    /// 任务分解失败 — 输入意图无法被规则分解器处理
    #[error("task decomposition failed: {0}")]
    DecompositionFailed(String),

    /// 事件总线错误 — 发布或订阅失败时包装底层 EventBusError
    #[error("event bus error: {0}")]
    EventBusError(#[from] event_bus::EventBusError),

    /// 序列化/反序列化失败 — JSON 或 MessagePack 编解码错误
    #[error("serialization error: {0}")]
    SerializationError(String),

    /// 检查点损坏 — 恢复时哈希校验失败,状态可能已漂移
    #[error("checkpoint corrupted: hash mismatch")]
    CheckpointCorrupted,

    /// 检查点保存失败 — IO 或序列化错误
    #[error("checkpoint save failed: {0}")]
    CheckpointSaveFailed(String),

    /// 检查点不存在 — 按 checkpoint_id 查找时未找到
    #[error("checkpoint not found: {0}")]
    CheckpointNotFound(String),

    /// TTG 手动覆盖被拒绝 — 当前预算档位不允许切换到目标思考模式
    ///
    /// WHY:Degraded 档位下预算接近耗尽,Deep 模式会消耗更多 token,
    /// 强制覆盖将导致预算溢出。此约束为架构红线(§6 架构红线:预算优先)。
    #[error("ttg override rejected: quest {quest_id} requested {requested_mode} under tier {current_tier}: {reason}")]
    TtgOverrideRejected {
        /// 所属 Quest ID
        quest_id: String,
        /// 请求的目标思考模式(Fast/Standard/Deep)
        requested_mode: String,
        /// 当前预算档位(high_tier/low_tier/degraded)
        current_tier: String,
        /// 拒绝原因(人类可读)
        reason: String,
    },
}

/// 从 serde_json 错误转换 — 检查点序列化等场景
impl From<serde_json::Error> for QuestError {
    fn from(err: serde_json::Error) -> Self {
        Self::SerializationError(format!("json: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cyclic_dependency_display() {
        let e = QuestError::CyclicDependency;
        assert!(e.to_string().contains("cyclic"));
    }

    #[test]
    fn test_task_not_found_display() {
        let e = QuestError::TaskNotFound("t-1".into());
        assert!(e.to_string().contains("t-1"));
    }

    #[test]
    fn test_invalid_status_display() {
        let e = QuestError::InvalidStatus("Running->Pending".into());
        assert!(e.to_string().contains("Running->Pending"));
    }

    #[test]
    fn test_from_serde_json_error() {
        let json_err = serde_json::from_str::<serde_json::Value>("{invalid}").unwrap_err();
        let quest_err: QuestError = json_err.into();
        assert!(matches!(quest_err, QuestError::SerializationError(_)));
    }

    #[test]
    fn test_ttg_override_rejected_display() {
        let e = QuestError::TtgOverrideRejected {
            quest_id: "quest-1".into(),
            requested_mode: "Deep".into(),
            current_tier: "degraded".into(),
            reason: "budget degraded".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("quest-1"));
        assert!(msg.contains("Deep"));
        assert!(msg.contains("degraded"));
        assert!(msg.contains("budget degraded"));
    }
}
