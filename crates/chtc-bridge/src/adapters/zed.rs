//! Zed 适配器 — 完整实现(execute 返回模拟执行结果)
//!
//! P2-4: 补全 execute 实现,对齐 VSCode 适配器的模拟执行模式。
//! Zed 通过 action-based 协议与 IDE 交互,execute 返回携带
//! tool_id 与 ide 标识的成功结果,供上层 CHTC 桥接消费。

use crate::adapters::IdeAdapter;
use crate::error::ChtcError;
use crate::protocol::ProtocolConverter;
use crate::types::{IdeSource, ToolCallResult, UnifiedToolCall};
use serde_json::Value;
use std::time::Instant;

/// Zed 适配器实例
#[derive(Debug, Clone, Default)]
pub struct ZedAdapter;

impl ZedAdapter {
    /// 创建 Zed 适配器
    pub fn new() -> Self {
        Self
    }
}

impl IdeAdapter for ZedAdapter {
    fn ide_source(&self) -> IdeSource {
        IdeSource::zed()
    }

    fn convert_to_unified(&self, raw: Value) -> Result<UnifiedToolCall, ChtcError> {
        ProtocolConverter::from_zed_format(raw)
    }

    fn convert_from_unified(&self, call: &UnifiedToolCall) -> Value {
        ProtocolConverter::to_native_format(call)
    }

    fn execute(&self, call: &UnifiedToolCall) -> Result<ToolCallResult, ChtcError> {
        let start = Instant::now();
        // DEFERRED: 当前为模拟执行,真实 IDE 集成需通过 MCP Mesh 跨进程通信。
        // 预计 v3.x MCP Mesh 实装后替换为真实 Zed action-based 协议通信。
        let result = serde_json::json!({
            "executed": true,
            "tool": call.tool_id,
            "ide": "zed",
        });
        Ok(ToolCallResult {
            call_id: call.call_id.clone(),
            success: true,
            result,
            error: None,
            latency_ms: start.elapsed().as_millis() as u64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zed_adapter_convert() {
        let a = ZedAdapter::new();
        let raw = serde_json::json!({ "action": "z", "data": {} });
        let call = a.convert_to_unified(raw).unwrap();
        assert_eq!(call.tool_id, "z");
    }

    #[test]
    fn test_zed_adapter_execute_returns_success() {
        let a = ZedAdapter::new();
        let call = UnifiedToolCall {
            tool_id: "editor.open".into(),
            parameters: serde_json::json!({}),
            ide_source: IdeSource::zed(),
            deadline_ms: 5000,
            call_id: "cid".into(),
        };
        let r = a.execute(&call).unwrap();
        assert!(r.success);
        assert_eq!(r.call_id, "cid");
        assert_eq!(r.result["ide"], "zed");
        assert_eq!(r.result["tool"], "editor.open");
        assert!(r.error.is_none());
    }
}
