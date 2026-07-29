//! 类型安全的 ID newtype wrapper
//!
//! 为各种 ID 提供编译期类型安全,防止 `quest_id` 与 `model_id` 等混用。
//! 第一阶段:定义并导出,后续阶段逐步替换核心类型字段。
//!
//! 所有 ID 类型通过 [`crate::id_newtype!`] 宏生成,共享完整的 newtype 实现:
//! - `Deref<Target=str>` / `AsRef<str>` / `Borrow<str>` — 与 `&str` 接口兼容
//! - `From<String>` / `From<&str>` — 方便构造
//! - `Serialize` / `Deserialize`(`#[serde(transparent)]`) — JSON 向后兼容
//! - `Display` / `Debug` / `Clone` / `PartialEq` / `Eq` / `Hash`

// ─── 核心领域 ID ───────────────────────────────────────────────
id_newtype!(QuestId, "Quest 唯一标识(UUIDv7,时间有序)");
id_newtype!(TaskId, "子任务唯一标识");
id_newtype!(IntentId, "用户意图唯一标识");

// ─── 模型与 Agent ID ──────────────────────────────────────────
id_newtype!(ModelId, "模型唯一标识");
id_newtype!(AgentId, "Agent 唯一标识");

// ─── 能力与操作 ID ────────────────────────────────────────────
id_newtype!(CapabilityId, "能力唯一标识");
id_newtype!(OperationId, "操作唯一标识");

#[cfg(test)]
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
        let id = IntentId::new("intent-x");
        let s: &str = &id;
        assert_eq!(s, "intent-x");
    }

    #[test]
    fn test_id_serde_roundtrip() {
        let id = ModelId::new("gpt-4");
        let json = serde_json::to_string(&id).unwrap();
        // #[serde(transparent)] 保证序列化为裸字符串
        assert_eq!(json, "\"gpt-4\"");
        let deser: ModelId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, deser);
    }

    #[test]
    fn test_different_id_types_not_interchangeable() {
        // 编译期类型安全:QuestId 和 TaskId 是不同类型
        let _qid = QuestId::new("same-string");
        let _tid = TaskId::new("same-string");
        // 以下代码不会编译(预期行为):
        // assert_eq!(qid, tid); // 类型不匹配
    }

    #[test]
    fn test_all_id_types_constructible() {
        let _ = QuestId::new("q-1");
        let _ = TaskId::new("t-1");
        let _ = IntentId::new("i-1");
        let _ = ModelId::new("m-1");
        let _ = AgentId::new("a-1");
        let _ = CapabilityId::new("c-1");
        let _ = OperationId::new("o-1");
    }

    #[test]
    fn test_id_from_string() {
        let id = AgentId::from(String::from("agent-xyz"));
        assert_eq!(id.as_str(), "agent-xyz");
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
