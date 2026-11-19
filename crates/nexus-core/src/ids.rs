//! 类型安全的 ID newtype wrapper
//!
//! 为各种 ID 提供编译期类型安全,防止 quest_id 与 task_id 等混用。
//!
//! 所有 ID 类型通过 crate::id_newtype! 宏生成,共享完整的 newtype 实现:
//! - Deref<Target=str> / AsRef<str> / Borrow<str> — 与 &str 接口兼容
//! - From<String> / From<&str> — 方便构造
//! - Serialize / Deserialize(#[serde(transparent)]) — JSON 向后兼容
//! - Display / Debug / Clone / PartialEq / Eq / Hash
//!
//! [L1-C-2 / 2026-11-16] AgentId / IntentId / ModelId 已退役:生产代码零消费
//! (nexus-core 自身 tests/benches 外无引用),由本次破坏式重构物理删除。
//! 消费方如需 Agent 标识,直连 nexus_contracts 的领域 ID。

// ─── 核心领域 ID ───────────────────────────────────────────────
id_newtype!(QuestId, "Quest 唯一标识(UUIDv7,时间有序)");
id_newtype!(TaskId, "子任务唯一标识");

// ─── 能力与操作 ID ────────────────────────────────────────────
id_newtype!(CapabilityId, "能力唯一标识");
id_newtype!(OperationId, "操作唯一标识");

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_id_creation_and_equality() {
        let id1 = QuestId::new("quest-001");
        let id2 = QuestId::from("quest-001");
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_id_display() {
        let id = TaskId::new("task-42");
        assert_eq!(format!("{}", id), "task-42");
    }

    #[test]
    fn test_id_deref() {
        let id = TaskId::new("task-x");
        let s: &str = &id;
        assert_eq!(s, "task-x");
    }

    #[test]
    fn test_id_serde_roundtrip() {
        let id = QuestId::new("q-1");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"q-1\"");
        let deser: QuestId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, deser);
    }

    #[test]
    fn test_different_id_types_not_interchangeable() {
        let _qid = QuestId::new("same-string");
        let _tid = TaskId::new("same-string");
    }

    #[test]
    fn test_all_id_types_constructible() {
        let _ = QuestId::new("q-1");
        let _ = TaskId::new("t-1");
        let _ = CapabilityId::new("c-1");
        let _ = OperationId::new("o-1");
    }

    #[test]
    fn test_id_from_string() {
        let id = OperationId::from(String::from("op-xyz"));
        assert_eq!(id.as_str(), "op-xyz");
    }

    #[test]
    fn test_id_hash() {
        use std::collections::HashMap;
        let id = CapabilityId::new("cap-1");
        let mut map = HashMap::new();
        map.insert(id.clone(), 42);
        assert_eq!(map.get("cap-1" as &str), Some(&42));
    }
}
