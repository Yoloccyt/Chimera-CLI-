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

    /// re-export 类型等价性 — **编译期**证明（强于序列化 roundtrip）
    ///
    /// # WHY 不用 JSON roundtrip 验证
    /// 序列化往返只能证明"两个类型格式兼容"：若 L1 另起炉灶独立定义了一个字段
    /// 完全相同的 `TaskStatus`，roundtrip 照样通过，但两者已不是同一个类型。
    /// 本用例改用**赋值处的隐式类型转换**——只有两侧确为同一类型（纯 re-export）
    /// 才能编译，任何独立定义都会直接编译失败。这是该向后兼容契约的编译期锚点。
    ///
    /// # 断言搬迁说明
    /// 原位于 `nexus-contracts/tests/type_consistency_test.rs`
    /// （`test_backward_compat_nexus_core_reexport`，JSON roundtrip 弱证明）。
    /// 归位到 re-export 发生地并升级为编译期证明，同时消除 L0 的 dev 依赖环
    /// （契约层不再为验证 L1 反向依赖实现层）。
    #[test]
    fn reexported_types_are_identical_to_contracts() {
        // 编译期证明：左值走 L0 路径，右值走 L1 re-export 路径
        let _: nexus_contracts::TaskStatus = TaskStatus::Running;
        let _: nexus_contracts::Checkpoint = Checkpoint::new(
            "q-reexport",
            "c-reexport",
            "hash-reexport",
            vec![0xDE, 0xAD, 0xBE, 0xEF],
        );

        // 运行时抽查：构造方法与字段访问同样随 re-export 完整保留
        let cp: Checkpoint = Checkpoint::new("q1", "c1", "h1", vec![1, 2, 3]);
        assert_eq!(cp.quest_id, "q1");
        assert_eq!(cp.serialized_state, vec![1, 2, 3]);

        // 其余同源下沉类型一并锚定（Quest 来自 nexus_contracts::domain）
        let quests: Vec<nexus_contracts::Quest> = vec![Quest::default()];
        assert_eq!(quests.len(), 1, "Quest 应经 nexus-core 可直接构造");
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
