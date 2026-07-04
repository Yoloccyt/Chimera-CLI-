//! IDE 适配器 — enum dispatch 实现 5 大 IDE 的工具调用兼容
//!
//! 对应架构:L10 Interface 内部适配层
//!
//! # 为何用 enum dispatch 而非 `Box<dyn IdeAdapter>`
//! - 静态分发:编译期单态化,无虚函数调用开销
//! - 无堆分配:IdeAdapterKind 是栈上 enum,缓存友好
//! - 类型穷尽:新增 IDE 时编译器强制更新所有 match 分支

use crate::error::ChtcError;
use crate::types::{IdeSource, ToolCallResult, UnifiedToolCall};
use serde_json::Value;

pub mod emacs;
pub mod intellij;
pub mod vim;
pub mod vscode;
pub mod zed;

pub use emacs::EmacsAdapter;
pub use intellij::IntelliJAdapter;
pub use vim::VimAdapter;
pub use vscode::VscodeAdapter;
pub use zed::ZedAdapter;

/// IDE 适配器 trait — 每个具体适配器实现的统一契约
pub trait IdeAdapter: Send + Sync {
    /// 返回适配器对应的 IDE 来源
    fn ide_source(&self) -> IdeSource;
    /// 将原生格式转换为统一工具调用
    fn convert_to_unified(&self, raw: Value) -> Result<UnifiedToolCall, ChtcError>;
    /// 将统一工具调用反向转换为原生格式
    fn convert_from_unified(&self, call: &UnifiedToolCall) -> Value;
    /// 执行工具调用(本周仅 VSCode 完整实现,其余返回 NotImplemented)
    fn execute(&self, call: &UnifiedToolCall) -> Result<ToolCallResult, ChtcError>;
}

/// IDE 适配器种类 — enum dispatch 容器
///
/// 持有具体适配器实例,通过 match 静态分发到对应实现。
#[derive(Debug, Clone)]
pub enum IdeAdapterKind {
    /// VSCode 适配器
    Vscode(VscodeAdapter),
    /// IntelliJ 适配器
    IntelliJ(IntelliJAdapter),
    /// Vim 适配器
    Vim(VimAdapter),
    /// Emacs 适配器
    Emacs(EmacsAdapter),
    /// Zed 适配器
    Zed(ZedAdapter),
}

impl IdeAdapterKind {
    /// 根据 IDE 来源构造对应适配器
    pub fn for_source(source: &IdeSource) -> Self {
        match source {
            IdeSource::Vscode(_) => Self::Vscode(VscodeAdapter::new()),
            IdeSource::IntelliJ(_) => Self::IntelliJ(IntelliJAdapter::new()),
            IdeSource::Vim(_) => Self::Vim(VimAdapter::new()),
            IdeSource::Emacs(_) => Self::Emacs(EmacsAdapter::new()),
            IdeSource::Zed(_) => Self::Zed(ZedAdapter::new()),
        }
    }

    /// 返回适配器对应的 IDE 来源
    pub fn ide_source(&self) -> IdeSource {
        match self {
            Self::Vscode(a) => a.ide_source(),
            Self::IntelliJ(a) => a.ide_source(),
            Self::Vim(a) => a.ide_source(),
            Self::Emacs(a) => a.ide_source(),
            Self::Zed(a) => a.ide_source(),
        }
    }

    /// 原生格式 → 统一工具调用(dispatch)
    pub fn convert_to_unified(&self, raw: Value) -> Result<UnifiedToolCall, ChtcError> {
        match self {
            Self::Vscode(a) => a.convert_to_unified(raw),
            Self::IntelliJ(a) => a.convert_to_unified(raw),
            Self::Vim(a) => a.convert_to_unified(raw),
            Self::Emacs(a) => a.convert_to_unified(raw),
            Self::Zed(a) => a.convert_to_unified(raw),
        }
    }

    /// 统一工具调用 → 原生格式(dispatch)
    pub fn convert_from_unified(&self, call: &UnifiedToolCall) -> Value {
        match self {
            Self::Vscode(a) => a.convert_from_unified(call),
            Self::IntelliJ(a) => a.convert_from_unified(call),
            Self::Vim(a) => a.convert_from_unified(call),
            Self::Emacs(a) => a.convert_from_unified(call),
            Self::Zed(a) => a.convert_from_unified(call),
        }
    }

    /// 执行工具调用(dispatch)
    pub fn execute(&self, call: &UnifiedToolCall) -> Result<ToolCallResult, ChtcError> {
        match self {
            Self::Vscode(a) => a.execute(call),
            Self::IntelliJ(a) => a.execute(call),
            Self::Vim(a) => a.execute(call),
            Self::Emacs(a) => a.execute(call),
            Self::Zed(a) => a.execute(call),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === for_source 正确性 ===

    #[test]
    fn test_for_source_vscode() {
        let a = IdeAdapterKind::for_source(&IdeSource::vscode());
        assert!(matches!(a, IdeAdapterKind::Vscode(_)));
        assert_eq!(a.ide_source(), IdeSource::vscode());
    }

    #[test]
    fn test_for_source_intellij() {
        let a = IdeAdapterKind::for_source(&IdeSource::intellij());
        assert!(matches!(a, IdeAdapterKind::IntelliJ(_)));
    }

    #[test]
    fn test_for_source_vim() {
        let a = IdeAdapterKind::for_source(&IdeSource::vim());
        assert!(matches!(a, IdeAdapterKind::Vim(_)));
    }

    #[test]
    fn test_for_source_emacs() {
        let a = IdeAdapterKind::for_source(&IdeSource::emacs());
        assert!(matches!(a, IdeAdapterKind::Emacs(_)));
    }

    #[test]
    fn test_for_source_zed() {
        let a = IdeAdapterKind::for_source(&IdeSource::zed());
        assert!(matches!(a, IdeAdapterKind::Zed(_)));
    }

    // === dispatch 转换路径 ===

    #[test]
    fn test_dispatch_convert_to_unified_vscode() {
        let a = IdeAdapterKind::for_source(&IdeSource::vscode());
        let raw = serde_json::json!({ "command": "c", "args": {} });
        let call = a.convert_to_unified(raw).unwrap();
        assert_eq!(call.tool_id, "c");
    }

    #[test]
    fn test_dispatch_convert_from_unified_intellij() {
        let a = IdeAdapterKind::for_source(&IdeSource::intellij());
        let call = UnifiedToolCall {
            tool_id: "a1".into(),
            parameters: serde_json::json!({}),
            ide_source: IdeSource::intellij(),
            deadline_ms: 5000,
            call_id: "id1".into(),
        };
        let native = a.convert_from_unified(&call);
        assert_eq!(native["action"], "a1");
    }

    // === execute 行为 ===

    #[test]
    fn test_vscode_execute_success() {
        let a = IdeAdapterKind::for_source(&IdeSource::vscode());
        let call = UnifiedToolCall {
            tool_id: "editor.open".into(),
            parameters: serde_json::json!({}),
            ide_source: IdeSource::vscode(),
            deadline_ms: 5000,
            call_id: "call-1".into(),
        };
        let result = a.execute(&call).expect("VSCode execute 应成功");
        assert!(result.success);
        assert_eq!(result.call_id, "call-1");
        assert!(result.error.is_none());
    }

    #[test]
    fn test_intellij_execute_not_implemented() {
        let a = IdeAdapterKind::for_source(&IdeSource::intellij());
        let call = UnifiedToolCall {
            tool_id: "x".into(),
            parameters: serde_json::json!({}),
            ide_source: IdeSource::intellij(),
            deadline_ms: 5000,
            call_id: "c2".into(),
        };
        let err = a.execute(&call).unwrap_err();
        assert!(matches!(err, ChtcError::NotImplemented { .. }));
    }

    #[test]
    fn test_vim_execute_not_implemented() {
        let a = IdeAdapterKind::for_source(&IdeSource::vim());
        let call = UnifiedToolCall {
            tool_id: "x".into(),
            parameters: serde_json::json!({}),
            ide_source: IdeSource::vim(),
            deadline_ms: 5000,
            call_id: "c3".into(),
        };
        let err = a.execute(&call).unwrap_err();
        assert!(matches!(err, ChtcError::NotImplemented { .. }));
    }

    #[test]
    fn test_emacs_execute_not_implemented() {
        let a = IdeAdapterKind::for_source(&IdeSource::emacs());
        let call = UnifiedToolCall {
            tool_id: "x".into(),
            parameters: serde_json::json!({}),
            ide_source: IdeSource::emacs(),
            deadline_ms: 5000,
            call_id: "c4".into(),
        };
        let err = a.execute(&call).unwrap_err();
        assert!(matches!(err, ChtcError::NotImplemented { .. }));
    }

    #[test]
    fn test_zed_execute_not_implemented() {
        let a = IdeAdapterKind::for_source(&IdeSource::zed());
        let call = UnifiedToolCall {
            tool_id: "x".into(),
            parameters: serde_json::json!({}),
            ide_source: IdeSource::zed(),
            deadline_ms: 5000,
            call_id: "c5".into(),
        };
        let err = a.execute(&call).unwrap_err();
        assert!(matches!(err, ChtcError::NotImplemented { .. }));
    }

    #[test]
    fn test_for_source_preserves_version_in_dispatch() {
        // 带版本的 ide_source 仍能正确分发
        let src = IdeSource::Zed(Some("0.130.0".into()));
        let a = IdeAdapterKind::for_source(&src);
        let raw = serde_json::json!({ "action": "z", "data": {} });
        let call = a.convert_to_unified(raw).unwrap();
        assert_eq!(call.tool_id, "z");
    }
}
