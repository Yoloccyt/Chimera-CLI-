//! 统一协议层 — 5 种 IDE 原生格式与 UnifiedToolCall 的双向转换
//!
//! 对应架构:L10 Interface 内部协议归一化
//!
//! # 转换规则
//! | IDE | 原生格式 | tool_id 来源 | parameters 来源 |
//! |-----|---------|------------|----------------|
//! | VSCode | `{ command, args }` | command | args(对象) |
//! | IntelliJ | `{ action, params }` | action | params(对象) |
//! | Vim | `{ cmd, args }` | cmd | args(数组) |
//! | Emacs | `{ sexp, buffer }` | sexp | `{ buffer }` |
//! | Zed | `{ action, data }` | action | data(对象) |

use crate::error::ChtcError;
use crate::types::{IdeSource, UnifiedToolCall};
use serde_json::{Map, Value};

/// 默认调用截止时间(毫秒),与 ChtcConfig::default 保持一致
pub const DEFAULT_DEADLINE_MS: u64 = 5000;

/// 协议转换器 — 无状态,所有方法可独立调用
#[derive(Debug, Clone, Default)]
pub struct ProtocolConverter;

impl ProtocolConverter {
    /// 创建转换器实例
    pub fn new() -> Self {
        Self
    }

    /// 从 VSCode 原生格式转换
    ///
    /// 原生格式:`{ "command": "...", "args": {...} }`
    pub fn from_vscode_format(raw: Value) -> Result<UnifiedToolCall, ChtcError> {
        let obj = Self::as_object(&raw, "vscode")?;
        let command = Self::take_str_field(obj, "command", "vscode")?;
        let args = obj
            .get("args")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        Ok(Self::build_call(command, args, IdeSource::vscode()))
    }

    /// 从 IntelliJ 原生格式转换
    ///
    /// 原生格式:`{ "action": "...", "params": {...} }`
    pub fn from_intellij_format(raw: Value) -> Result<UnifiedToolCall, ChtcError> {
        let obj = Self::as_object(&raw, "intellij")?;
        let action = Self::take_str_field(obj, "action", "intellij")?;
        let params = obj
            .get("params")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        Ok(Self::build_call(action, params, IdeSource::intellij()))
    }

    /// 从 Vim 原生格式转换
    ///
    /// 原生格式:`{ "cmd": "...", "args": [...] }`
    pub fn from_vim_format(raw: Value) -> Result<UnifiedToolCall, ChtcError> {
        let obj = Self::as_object(&raw, "vim")?;
        let cmd = Self::take_str_field(obj, "cmd", "vim")?;
        let args = obj
            .get("args")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        Ok(Self::build_call(cmd, args, IdeSource::vim()))
    }

    /// 从 Emacs 原生格式转换
    ///
    /// 原生格式:`{ "sexp": "(...)", "buffer": "..." }`
    /// WHY:sexp 作为 tool_id(它是 Emacs 调用的"命令"内容),
    /// buffer 归入 parameters 以便 to_native_format 还原。
    pub fn from_emacs_format(raw: Value) -> Result<UnifiedToolCall, ChtcError> {
        let obj = Self::as_object(&raw, "emacs")?;
        let sexp = Self::take_str_field(obj, "sexp", "emacs")?;
        let buffer = obj.get("buffer").cloned().unwrap_or(Value::Null);
        let params = serde_json::json!({ "buffer": buffer });
        Ok(Self::build_call(sexp, params, IdeSource::emacs()))
    }

    /// 从 Zed 原生格式转换
    ///
    /// 原生格式:`{ "action": "...", "data": {...} }`
    pub fn from_zed_format(raw: Value) -> Result<UnifiedToolCall, ChtcError> {
        let obj = Self::as_object(&raw, "zed")?;
        let action = Self::take_str_field(obj, "action", "zed")?;
        let data = obj
            .get("data")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        Ok(Self::build_call(action, data, IdeSource::zed()))
    }

    /// 反向转换 — 根据 call.ide_source 还原为对应 IDE 的原生格式
    pub fn to_native_format(call: &UnifiedToolCall) -> Value {
        match call.ide_source {
            IdeSource::Vscode(_) => serde_json::json!({
                "command": call.tool_id,
                "args": call.parameters,
            }),
            IdeSource::IntelliJ(_) => serde_json::json!({
                "action": call.tool_id,
                "params": call.parameters,
            }),
            IdeSource::Vim(_) => serde_json::json!({
                "cmd": call.tool_id,
                "args": call.parameters,
            }),
            IdeSource::Emacs(_) => {
                let buffer = call
                    .parameters
                    .get("buffer")
                    .cloned()
                    .unwrap_or(Value::Null);
                serde_json::json!({
                    "sexp": call.tool_id,
                    "buffer": buffer,
                })
            }
            IdeSource::Zed(_) => serde_json::json!({
                "action": call.tool_id,
                "data": call.parameters,
            }),
        }
    }

    /// 统一入口 — 根据 ide_source 选择对应的 from_*_format 方法
    ///
    /// 保留传入 ide_source 的版本信息(覆盖 from_*_format 生成的默认值),
    /// 确保调用方传入的版本元数据不丢失。
    pub fn receive(&self, raw: Value, ide_source: IdeSource) -> Result<UnifiedToolCall, ChtcError> {
        let mut call = match &ide_source {
            IdeSource::Vscode(_) => Self::from_vscode_format(raw)?,
            IdeSource::IntelliJ(_) => Self::from_intellij_format(raw)?,
            IdeSource::Vim(_) => Self::from_vim_format(raw)?,
            IdeSource::Emacs(_) => Self::from_emacs_format(raw)?,
            IdeSource::Zed(_) => Self::from_zed_format(raw)?,
        };
        call.ide_source = ide_source;
        Ok(call)
    }

    // === 内部辅助方法 ===

    fn as_object<'a>(raw: &'a Value, ide: &str) -> Result<&'a Map<String, Value>, ChtcError> {
        raw.as_object().ok_or_else(|| ChtcError::ProtocolError {
            reason: format!("{ide} 格式必须是 JSON 对象"),
        })
    }

    fn take_str_field(
        obj: &Map<String, Value>,
        field: &str,
        ide: &str,
    ) -> Result<String, ChtcError> {
        obj.get(field)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| ChtcError::ProtocolError {
                reason: format!("{ide} 格式缺少 {field} 字段或类型错误(期望字符串)"),
            })
    }

    fn build_call(tool_id: String, parameters: Value, ide_source: IdeSource) -> UnifiedToolCall {
        UnifiedToolCall {
            tool_id,
            parameters,
            ide_source,
            deadline_ms: DEFAULT_DEADLINE_MS,
            call_id: uuid::Uuid::now_v7().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === 5 种 IDE 正向转换测试 ===

    #[test]
    fn test_from_vscode_format_basic() {
        let raw = serde_json::json!({
            "command": "editor.open",
            "args": { "file": "/tmp/a.rs" }
        });
        let call = ProtocolConverter::from_vscode_format(raw).expect("转换失败");
        assert_eq!(call.tool_id, "editor.open");
        assert_eq!(call.parameters["file"], "/tmp/a.rs");
        assert_eq!(call.ide_source, IdeSource::vscode());
        assert_eq!(call.deadline_ms, DEFAULT_DEADLINE_MS);
        assert!(!call.call_id.is_empty());
    }

    #[test]
    fn test_from_intellij_format_basic() {
        let raw = serde_json::json!({
            "action": "refactor.rename",
            "params": { "target": "foo" }
        });
        let call = ProtocolConverter::from_intellij_format(raw).expect("转换失败");
        assert_eq!(call.tool_id, "refactor.rename");
        assert_eq!(call.parameters["target"], "foo");
        assert_eq!(call.ide_source, IdeSource::intellij());
    }

    #[test]
    fn test_from_vim_format_basic() {
        let raw = serde_json::json!({
            "cmd": ":w",
            "args": ["/tmp/b.txt"]
        });
        let call = ProtocolConverter::from_vim_format(raw).expect("转换失败");
        assert_eq!(call.tool_id, ":w");
        assert!(call.parameters.is_array());
        assert_eq!(call.parameters[0], "/tmp/b.txt");
        assert_eq!(call.ide_source, IdeSource::vim());
    }

    #[test]
    fn test_from_emacs_format_basic() {
        let raw = serde_json::json!({
            "sexp": "(+ 1 2)",
            "buffer": "scratch"
        });
        let call = ProtocolConverter::from_emacs_format(raw).expect("转换失败");
        assert_eq!(call.tool_id, "(+ 1 2)");
        assert_eq!(call.parameters["buffer"], "scratch");
        assert_eq!(call.ide_source, IdeSource::emacs());
    }

    #[test]
    fn test_from_zed_format_basic() {
        let raw = serde_json::json!({
            "action": "git.commit",
            "data": { "msg": "init" }
        });
        let call = ProtocolConverter::from_zed_format(raw).expect("转换失败");
        assert_eq!(call.tool_id, "git.commit");
        assert_eq!(call.parameters["msg"], "init");
        assert_eq!(call.ide_source, IdeSource::zed());
    }

    // === 异常格式测试(缺字段/类型错误) ===

    #[test]
    fn test_from_vscode_missing_command() {
        let raw = serde_json::json!({ "args": {} });
        let err = ProtocolConverter::from_vscode_format(raw).unwrap_err();
        assert!(matches!(err, ChtcError::ProtocolError { .. }));
    }

    #[test]
    fn test_from_vscode_command_not_string() {
        let raw = serde_json::json!({ "command": 123 });
        let err = ProtocolConverter::from_vscode_format(raw).unwrap_err();
        assert!(matches!(err, ChtcError::ProtocolError { .. }));
    }

    #[test]
    fn test_from_vscode_not_object() {
        let raw = serde_json::json!(["not", "an", "object"]);
        let err = ProtocolConverter::from_vscode_format(raw).unwrap_err();
        assert!(matches!(err, ChtcError::ProtocolError { .. }));
    }

    #[test]
    fn test_from_intellij_missing_action() {
        let raw = serde_json::json!({ "params": {} });
        let err = ProtocolConverter::from_intellij_format(raw).unwrap_err();
        assert!(matches!(err, ChtcError::ProtocolError { .. }));
    }

    #[test]
    fn test_from_vim_missing_cmd() {
        let raw = serde_json::json!({ "args": [] });
        let err = ProtocolConverter::from_vim_format(raw).unwrap_err();
        assert!(matches!(err, ChtcError::ProtocolError { .. }));
    }

    #[test]
    fn test_from_emacs_missing_sexp() {
        let raw = serde_json::json!({ "buffer": "x" });
        let err = ProtocolConverter::from_emacs_format(raw).unwrap_err();
        assert!(matches!(err, ChtcError::ProtocolError { .. }));
    }

    #[test]
    fn test_from_zed_missing_action() {
        let raw = serde_json::json!({ "data": {} });
        let err = ProtocolConverter::from_zed_format(raw).unwrap_err();
        assert!(matches!(err, ChtcError::ProtocolError { .. }));
    }

    // === 反向转换测试 ===

    #[test]
    fn test_to_native_format_vscode() {
        let call = ProtocolConverter::from_vscode_format(serde_json::json!({
            "command": "c1", "args": { "k": "v" }
        }))
        .unwrap();
        let native = ProtocolConverter::to_native_format(&call);
        assert_eq!(native["command"], "c1");
        assert_eq!(native["args"]["k"], "v");
    }

    #[test]
    fn test_to_native_format_intellij() {
        let call = ProtocolConverter::from_intellij_format(serde_json::json!({
            "action": "a1", "params": { "p": 1 }
        }))
        .unwrap();
        let native = ProtocolConverter::to_native_format(&call);
        assert_eq!(native["action"], "a1");
        assert_eq!(native["params"]["p"], 1);
    }

    #[test]
    fn test_to_native_format_vim() {
        let call = ProtocolConverter::from_vim_format(serde_json::json!({
            "cmd": ":q", "args": ["a", "b"]
        }))
        .unwrap();
        let native = ProtocolConverter::to_native_format(&call);
        assert_eq!(native["cmd"], ":q");
        assert_eq!(native["args"][1], "b");
    }

    #[test]
    fn test_to_native_format_emacs() {
        let call = ProtocolConverter::from_emacs_format(serde_json::json!({
            "sexp": "(x)", "buffer": "buf"
        }))
        .unwrap();
        let native = ProtocolConverter::to_native_format(&call);
        assert_eq!(native["sexp"], "(x)");
        assert_eq!(native["buffer"], "buf");
    }

    #[test]
    fn test_to_native_format_zed() {
        let call = ProtocolConverter::from_zed_format(serde_json::json!({
            "action": "z1", "data": { "d": true }
        }))
        .unwrap();
        let native = ProtocolConverter::to_native_format(&call);
        assert_eq!(native["action"], "z1");
        assert_eq!(native["data"]["d"], true);
    }

    // === receive 分发与 round-trip 一致性 ===

    #[test]
    fn test_receive_dispatch_vscode() {
        let conv = ProtocolConverter::new();
        let raw = serde_json::json!({ "command": "x", "args": {} });
        let call = conv.receive(raw, IdeSource::vscode()).unwrap();
        assert_eq!(call.tool_id, "x");
        assert_eq!(call.ide_source, IdeSource::vscode());
    }

    #[test]
    fn test_receive_preserves_version() {
        let conv = ProtocolConverter::new();
        let raw = serde_json::json!({ "command": "x", "args": {} });
        let src = IdeSource::Vscode(Some("1.90.0".into()));
        let call = conv.receive(raw, src.clone()).unwrap();
        assert_eq!(call.ide_source, src);
    }

    #[test]
    fn test_roundtrip_tool_id_consistency() {
        // 不变量:to_native_format(from_*(raw)) 保持 tool_id 一致
        let cases = vec![
            (
                IdeSource::vscode(),
                serde_json::json!({ "command": "tc1", "args": {} }),
            ),
            (
                IdeSource::intellij(),
                serde_json::json!({ "action": "tc2", "params": {} }),
            ),
            (
                IdeSource::vim(),
                serde_json::json!({ "cmd": "tc3", "args": [] }),
            ),
            (
                IdeSource::emacs(),
                serde_json::json!({ "sexp": "tc4", "buffer": "b" }),
            ),
            (
                IdeSource::zed(),
                serde_json::json!({ "action": "tc5", "data": {} }),
            ),
        ];
        let conv = ProtocolConverter::new();
        for (src, raw) in cases {
            let call = conv.receive(raw, src).unwrap();
            let native = ProtocolConverter::to_native_format(&call);
            // tool_id 在原生格式中的字段名因 IDE 而异,但值必须一致
            let native_tool_id = match call.ide_source {
                IdeSource::Vscode(_) => native["command"].as_str(),
                IdeSource::IntelliJ(_) => native["action"].as_str(),
                IdeSource::Vim(_) => native["cmd"].as_str(),
                IdeSource::Emacs(_) => native["sexp"].as_str(),
                IdeSource::Zed(_) => native["action"].as_str(),
            };
            assert_eq!(native_tool_id, Some(call.tool_id.as_str()));
        }
    }
}
