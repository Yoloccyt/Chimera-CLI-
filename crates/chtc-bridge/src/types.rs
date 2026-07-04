//! CHTC 核心类型定义 — IDE 来源、统一工具调用、调用结果
//!
//! 对应创新点:CHTC(Cross-Harness Tool Compatibility)
//!
//! 设计原则:本模块仅定义纯数据类型,不依赖 adapters 模块,
//! 保持 `types → (无)` 的单向依赖,避免与 adapters 形成循环。

use serde::{Deserialize, Serialize};

/// IDE 来源标识 — 5 大受支持 IDE,每个变体携带可选版本信息
///
/// 版本信息用于审计日志与兼容性降级决策;`None` 表示版本未知。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdeSource {
    /// Visual Studio Code,附带可选版本号
    Vscode(Option<String>),
    /// IntelliJ IDEA,附带可选版本号
    IntelliJ(Option<String>),
    /// Vim/Neovim,附带可选版本号
    Vim(Option<String>),
    /// GNU Emacs,附带可选版本号
    Emacs(Option<String>),
    /// Zed Editor,附带可选版本号
    Zed(Option<String>),
}

impl IdeSource {
    /// 返回 IDE 种类的字符串标识(忽略版本),用于事件 payload 与日志
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Vscode(_) => "vscode",
            Self::IntelliJ(_) => "intellij",
            Self::Vim(_) => "vim",
            Self::Emacs(_) => "emacs",
            Self::Zed(_) => "zed",
        }
    }

    /// 便捷构造:VSCode(版本未知)
    pub fn vscode() -> Self {
        Self::Vscode(None)
    }

    /// 便捷构造:IntelliJ(版本未知)
    pub fn intellij() -> Self {
        Self::IntelliJ(None)
    }

    /// 便捷构造:Vim(版本未知)
    pub fn vim() -> Self {
        Self::Vim(None)
    }

    /// 便捷构造:Emacs(版本未知)
    pub fn emacs() -> Self {
        Self::Emacs(None)
    }

    /// 便捷构造:Zed(版本未知)
    pub fn zed() -> Self {
        Self::Zed(None)
    }
}

/// 统一工具调用 — 各 IDE 原生格式归一化后的中间表示
///
/// WHY:5 种 IDE 原生格式差异极大(VSCode 用 command/args、Emacs 用 sexp),
/// 统一为单一中间表示后,下层路由组件无需感知 IDE 差异。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedToolCall {
    /// 工具标识(VSCode 的 command / IntelliJ 的 action / Vim 的 cmd / Emacs 的 sexp / Zed 的 action)
    pub tool_id: String,
    /// 工具参数,保留原生格式中的参数部分(对象/数组/标量)
    pub parameters: serde_json::Value,
    /// 调用来源 IDE
    pub ide_source: IdeSource,
    /// 调用截止时间(毫秒),超时将由调用方处理
    pub deadline_ms: u64,
    /// 调用唯一标识(UUIDv7,时间有序,便于因果追踪)
    pub call_id: String,
}

/// 工具调用结果 — 适配器执行后的统一返回
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallResult {
    /// 对应的调用 ID(与 UnifiedToolCall.call_id 一致)
    pub call_id: String,
    /// 是否执行成功
    pub success: bool,
    /// 执行结果数据(成功时填充)
    pub result: serde_json::Value,
    /// 错误信息(失败时填充)
    pub error: Option<String>,
    /// 执行耗时(毫秒)
    pub latency_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ide_source_as_str() {
        assert_eq!(IdeSource::vscode().as_str(), "vscode");
        assert_eq!(IdeSource::intellij().as_str(), "intellij");
        assert_eq!(IdeSource::vim().as_str(), "vim");
        assert_eq!(IdeSource::emacs().as_str(), "emacs");
        assert_eq!(IdeSource::zed().as_str(), "zed");
    }

    #[test]
    fn test_ide_source_with_version() {
        let src = IdeSource::Vscode(Some("1.85.0".into()));
        assert_eq!(src.as_str(), "vscode");
        assert_eq!(src, IdeSource::Vscode(Some("1.85.0".into())));
    }

    #[test]
    fn test_ide_source_serialization() {
        let src = IdeSource::Zed(Some("0.120.0".into()));
        let json = serde_json::to_string(&src).expect("序列化失败");
        let restored: IdeSource = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(src, restored);
    }
}
