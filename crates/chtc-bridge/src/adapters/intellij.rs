//! IntelliJ 适配器 — 骨架实现(execute 返回 NotImplemented)

use crate::adapters::IdeAdapter;
use crate::error::ChtcError;
use crate::protocol::ProtocolConverter;
use crate::types::{IdeSource, ToolCallResult, UnifiedToolCall};
use serde_json::Value;

/// IntelliJ 适配器实例
#[derive(Debug, Clone, Default)]
pub struct IntelliJAdapter;

impl IntelliJAdapter {
    /// 创建 IntelliJ 适配器
    pub fn new() -> Self {
        Self
    }
}

impl IdeAdapter for IntelliJAdapter {
    fn ide_source(&self) -> IdeSource {
        IdeSource::intellij()
    }

    fn convert_to_unified(&self, raw: Value) -> Result<UnifiedToolCall, ChtcError> {
        ProtocolConverter::from_intellij_format(raw)
    }

    fn convert_from_unified(&self, call: &UnifiedToolCall) -> Value {
        ProtocolConverter::to_native_format(call)
    }

    fn execute(&self, _call: &UnifiedToolCall) -> Result<ToolCallResult, ChtcError> {
        Err(ChtcError::NotImplemented {
            ide: "intellij".into(),
            feature: "execute".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intellij_adapter_convert() {
        let a = IntelliJAdapter::new();
        let raw = serde_json::json!({ "action": "a", "params": {} });
        let call = a.convert_to_unified(raw).unwrap();
        assert_eq!(call.tool_id, "a");
    }

    #[test]
    fn test_intellij_adapter_execute_not_implemented() {
        let a = IntelliJAdapter::new();
        let call = UnifiedToolCall {
            tool_id: "x".into(),
            parameters: serde_json::json!({}),
            ide_source: IdeSource::intellij(),
            deadline_ms: 5000,
            call_id: "c".into(),
        };
        assert!(a.execute(&call).is_err());
    }
}
