//! ID newtype 宏 — 为标识类型统一生成 newtype 模式实现
//!
//! 对应架构层:L1 Core(供 L2-L10 所有上层 crate 共享)
//!
//! # 设计决策(WHY)
//! - **集中定义宏**:mlc-engine / osa-coordinator / kvbsr-router 三处重复实现 newtype ID
//!   类型(ToolId / FileId / MemoryId 等),每个约 30-50 行。提取到 L1 Core 消除重复,
//!   确保所有 ID 类型行为一致(Deref / AsRef / Borrow / From / Display)
//! - **`#[macro_export]`**:宏导出到 crate 根,调用方用 `nexus_core::id_newtype!(...)` 调用
//! - **完全限定路径**:宏内部用 `::serde::Serialize` / `::std::ops::Deref` 等绝对路径,
//!   使宏在调用方展开时无需额外 `use` 语句,降低使用门槛
//! - **`#[serde(transparent)]`**:保证 newtype 序列化为裸字符串,与原 `String` 别名
//!   向后兼容(已序列化的 SQLite 数据、EventBus 事件 payload 无需迁移)
//!
//! # 类型安全
//! newtype 模式使编译器能拦截 `ToolId` 误传为 `FileId`(不同类型不可互赋值),
//! 同时通过 `Deref<Target=str>` 保持与 `&str` 接口兼容(零运行时开销)。
//!
//! # 示例
//! ```
//! use nexus_core::id_newtype;
//!
//! nexus_core::id_newtype!(ToolId, "工具唯一标识");
//!
//! let id = ToolId::new("tool-1");
//! assert_eq!(id.as_str(), "tool-1");
//! // Deref<Target=str> 允许 &ToolId 当作 &str 使用
//! let ref_str: &str = &id;
//! assert_eq!(ref_str, "tool-1");
//! ```

/// 为 ID 类型生成 newtype 模式的完整实现
///
/// 生成的实现包含:
/// - `Debug / Clone / PartialEq / Eq / Hash / Serialize / Deserialize` 派生
/// - `#[serde(transparent)]` 保证 JSON 向后兼容
/// - `new(id: impl Into<String>) -> Self` 构造函数
/// - `as_str(&self) -> &str` 零拷贝访问
/// - `Deref<Target=str>` / `AsRef<str>` / `Borrow<str>` 与 `&str` 接口兼容
/// - `From<String>` / `From<&str>` 方便构造
/// - `Display` 格式化输出
///
/// # 参数
/// - `$name`:类型名(如 `ToolId`)
/// - `$doc`:文档注释字符串(如 `"工具唯一标识"`)
///
/// # 调用方依赖要求
/// 调用方 crate 必须在 `Cargo.toml` 中依赖 `serde`(含 `derive` feature),
/// 因为宏生成的派生需要 `serde::Serialize` / `serde::Deserialize`。
///
/// # 示例
/// ```
/// use nexus_core::id_newtype;
///
/// nexus_core::id_newtype!(MemoryId, "记忆条目唯一标识");
///
/// let id = MemoryId::from("m-1");
/// assert_eq!(id.to_string(), "m-1");
/// ```
#[macro_export]
macro_rules! id_newtype {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, ::serde::Serialize, ::serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub ::std::string::String);

        impl $name {
            /// 从任意可转换为 String 的值构造 ID
            pub fn new(id: impl ::std::convert::Into<::std::string::String>) -> Self {
                Self(id.into())
            }

            /// 返回内部字符串引用(零拷贝)
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl ::std::ops::Deref for $name {
            type Target = str;
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl ::std::convert::AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl ::std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl ::std::convert::From<::std::string::String> for $name {
            fn from(s: ::std::string::String) -> Self {
                Self(s)
            }
        }

        impl ::std::convert::From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                ::std::write!(f, "{}", self.0)
            }
        }
    };
}

#[cfg(test)]
mod tests {
    id_newtype!(TestId, "测试用 ID 类型");

    #[test]
    fn test_new_constructor() {
        let id = TestId::new("id-1");
        assert_eq!(id.as_str(), "id-1");
        assert_eq!(id.0, "id-1");
    }

    #[test]
    fn test_from_string() {
        let id = TestId::from(String::from("from-string"));
        assert_eq!(id.as_str(), "from-string");
    }

    #[test]
    fn test_from_str() {
        let id = TestId::from("from-str");
        assert_eq!(id.as_str(), "from-str");
    }

    #[test]
    fn test_deref_as_str() {
        let id = TestId::new("deref-test");
        // Deref<Target=str> 允许 &id 当作 &str
        let s: &str = &id;
        assert_eq!(s, "deref-test");
        // AsRef<str>
        assert_eq!(id.as_ref(), "deref-test");
    }

    #[test]
    fn test_borrow_str() {
        use std::collections::HashMap;
        let id = TestId::new("borrow-test");
        // Borrow<str> 允许用 &str 查询 HashMap<TestId, _>
        let mut map: HashMap<TestId, i32> = HashMap::new();
        map.insert(id.clone(), 42);
        assert_eq!(map.get("borrow-test" as &str), Some(&42));
    }

    #[test]
    fn test_display() {
        let id = TestId::new("display-test");
        assert_eq!(id.to_string(), "display-test");
    }

    #[test]
    fn test_eq_hash() {
        let id1 = TestId::new("same");
        let id2 = TestId::new("same");
        let id3 = TestId::new("different");
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);

        use std::collections::HashMap;
        let mut map: HashMap<TestId, i32> = HashMap::new();
        map.insert(id1, 1);
        assert_eq!(map.get(&id2), Some(&1)); // 相同 ID 哈希一致
    }

    #[test]
    fn test_serde_transparent() {
        let id = TestId::new("serde-test");
        let json = serde_json::to_string(&id).unwrap();
        // #[serde(transparent)] 使序列化为裸字符串
        assert_eq!(json, "\"serde-test\"");
        let restored: TestId = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, id);
    }
}
