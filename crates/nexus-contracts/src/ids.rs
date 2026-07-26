//! ID newtype 类型 — 五维度稀疏化的统一标识
//!
//! 对应架构层: L0 Contracts
//! 对应 ADR: ADR-033
//!
//! # 设计决策(WHY)
//!
//! - **从 osa-coordinator 上提**: 原 `ToolId` / `FileId` / `MemoryId` / `OperationId` / `TaskId`
//!   定义在 `osa-coordinator/src/types.rs`，通过 `nexus_core::id_newtype!` 宏生成。
//!   上提至 L0 后消除 L6 Router × 3 对 OSA 的星型依赖（Insight 2 消解）
//!
//! - **手动实现(不依赖 nexus-core)**: L0 禁止依赖任何 workspace crate（含 nexus-core），
//!   因此无法使用 `nexus_core::id_newtype!` 宏。改用 crate 私有宏 `id_newtype!`
//!   生成完全相同的 trait 实现（Debug/Clone/PartialEq/Eq/Hash/Serialize/Deserialize +
//!   Deref/AsRef/Borrow/From/Display），保证行为一致与序列化向后兼容
//!
//! - **`#[serde(transparent)]`**: 保证 newtype 序列化为裸字符串，与原 `String` 别名
//!   向后兼容（已序列化的 SQLite 数据、EventBus 事件 payload 无需迁移）
//!
//! # 类型安全
//!
//! newtype 模式使编译器能拦截 `ToolId` 误传为 `FileId`（不同类型不可互赋值），
//! 同时通过 `Deref<Target=str>` 保持与 `&str` 接口兼容（零运行时开销）。

use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;

/// 为 ID 类型生成 newtype 模式的完整实现（crate 私有宏）
///
/// 生成的实现与 `nexus_core::id_newtype!` 完全一致，保证序列化兼容性：
/// - `Debug / Clone / PartialEq / Eq / Hash / Serialize / Deserialize` 派生
/// - `#[serde(transparent)]` 保证 JSON 向后兼容
/// - `new(id: impl Into<String>) -> Self` 构造函数
/// - `as_str(&self) -> &str` 零拷贝访问
/// - `Deref<Target=str>` / `AsRef<str>` / `Borrow<str>` 与 `&str` 接口兼容
/// - `From<String>` / `From<&str>` 方便构造
/// - `Display` 格式化输出
macro_rules! id_newtype {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, ::serde::Serialize, ::serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// 从任意可转换为 String 的值构造 ID
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            /// 返回内部字符串引用(零拷贝)
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Deref for $name {
            type Target = str;
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                std::write!(f, "{}", self.0)
            }
        }
    };
}

// ============================================================
// 五维度稀疏化 ID 类型
// ============================================================
// 从 osa-coordinator/src/types.rs 上提，消除 L6 Router × 3 的星型耦合

id_newtype!(ToolId, "工具唯一标识 — routing 维度的稀疏化对象");
id_newtype!(FileId, "文件唯一标识 — context 维度的稀疏化对象");
id_newtype!(MemoryId, "记忆条目唯一标识 — memory 维度的稀疏化对象");
id_newtype!(OperationId, "操作唯一标识 — audit 维度的稀疏化对象");
id_newtype!(TaskId, "任务唯一标识 — budget 维度的稀疏化对象");

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_id_new() {
        let id = ToolId::new("tool-1");
        assert_eq!(id.as_str(), "tool-1");
        assert_eq!(id.0, "tool-1");
    }

    #[test]
    fn test_file_id_from_string() {
        let id = FileId::from(String::from("file-1"));
        assert_eq!(id.as_str(), "file-1");
    }

    #[test]
    fn test_memory_id_from_str() {
        let id = MemoryId::from("mem-1");
        assert_eq!(id.as_str(), "mem-1");
    }

    #[test]
    fn test_operation_id_deref() {
        let id = OperationId::new("op-1");
        let s: &str = &id; // Deref<Target=str>
        assert_eq!(s, "op-1");
        assert_eq!(id.as_ref(), "op-1"); // AsRef<str>
    }

    #[test]
    fn test_task_id_borrow() {
        use std::collections::HashMap;
        let id = TaskId::new("task-1");
        let mut map: HashMap<TaskId, i32> = HashMap::new();
        map.insert(id.clone(), 42);
        // Borrow<str> 允许用 &str 查询
        assert_eq!(map.get("task-1" as &str), Some(&42));
    }

    #[test]
    fn test_id_display() {
        let id = ToolId::new("display-test");
        assert_eq!(id.to_string(), "display-test");
    }

    #[test]
    fn test_id_eq_hash() {
        let id1 = ToolId::new("same");
        let id2 = ToolId::new("same");
        let id3 = ToolId::new("different");
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);

        use std::collections::HashMap;
        let mut map: HashMap<ToolId, i32> = HashMap::new();
        map.insert(id1, 1);
        assert_eq!(map.get(&id2), Some(&1)); // 相同 ID 哈希一致
    }

    #[test]
    fn test_id_serde_transparent() {
        let id = ToolId::new("serde-test");
        let json = serde_json::to_string(&id).unwrap();
        // #[serde(transparent)] 使序列化为裸字符串
        assert_eq!(json, "\"serde-test\"");
        let restored: ToolId = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, id);
    }

    #[test]
    fn test_different_id_types_not_interchangeable() {
        // newtype 模式保证类型安全: ToolId 不能当 FileId 用
        let tool = ToolId::new("same-id");
        let file = FileId::new("same-id");
        // 以下代码若取消注释会编译失败(类型不匹配):
        // assert_eq!(tool, file);
        // 但它们内部字符串相同
        assert_eq!(tool.as_str(), file.as_str());
    }
}
