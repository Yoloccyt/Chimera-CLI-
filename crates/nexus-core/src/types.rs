//! 核心领域类型汇聚点 — NEXUS-OMEGA 全局领域模型(经 L0 nexus-contracts re-export)
//!
//! 对应架构层:L1 Core(被 L2-L10 所有上层 crate 依赖)
//! 对应创新点:CLV(Context Latent Vector)、MLC(多级记忆)、TTG(思考切换)
//!
//! # 类型职责
//! 本文件为**纯 re-export 汇聚点**,所有共享领域类型均下沉至 L0 nexus-contracts:
//! - `UserIntent` / `Quest` / `Task` / `ThinkingMode` / `MultimodalInput`:ADR-054 决策 6(P9-T7)
//! - `Checkpoint` / `TaskStatus`:Task 3.10(ADR-033 扩展)
//!
//! WHY re-export(而非本地定义): 保持向后兼容——L2-L10 上层 crate 现有
//! `use nexus_core::types::Quest` / `use nexus_core::Quest` 路径零破坏(30 依赖方)。

// Task 3.10 + P9-T7 Task 3: 共享领域类型已下沉至 L0 nexus-contracts(ADR-033 扩展 / ADR-054 决策 6)
// WHY re-export: 65+ 文件现有 `use nexus_core::types::TaskStatus` 路径、30 依赖方现有
// `use nexus_core::{Quest, Task, ThinkingMode}` 路径均不破坏。类型 + impl(Checkpoint::new /
// Quest::default / default_priority)均来自 nexus-contracts,re-export 完整保留构造方法与
// serde 行为(含 #[serde(default = "default_priority")] 旧数据兼容语义)。
pub use nexus_contracts::domain::{MultimodalInput, Quest, Task, ThinkingMode, UserIntent};
pub use nexus_contracts::{Checkpoint, TaskStatus};

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
        use chrono::Utc;
        let before = Utc::now();
        let cp = Checkpoint::new("q1", "c1", "hash123", vec![1, 2, 3]);
        let after = Utc::now();
        assert!(cp.created_at >= before);
        assert!(cp.created_at <= after);
    }

    #[test]
    fn quest_default_priority_is_128() {
        let quest = Quest::default();
        assert_eq!(quest.priority, 128, "默认优先级应为 128");
    }

    #[test]
    fn quest_with_priority_serde_roundtrip() {
        let quest = Quest {
            quest_id: "q1".into(),
            title: "Test".into(),
            tasks: vec![],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
            priority: 200,
        };
        let json = serde_json::to_string(&quest).unwrap();
        let decoded: Quest = serde_json::from_str(&json).unwrap();
        assert_eq!(quest, decoded);
    }

    #[test]
    fn quest_old_data_without_priority_deserializes_to_default() {
        // 模拟旧数据(无 priority 字段),验证 #[serde(default = "default_priority")] 兼容
        let old_json = r#"{"quest_id":"q1","title":"Old","tasks":[],"thinking_mode":"Standard","checkpoint_id":null}"#;
        let decoded: Quest = serde_json::from_str(old_json).unwrap();
        assert_eq!(decoded.priority, 128, "旧数据应取默认优先级 128");
    }
}
