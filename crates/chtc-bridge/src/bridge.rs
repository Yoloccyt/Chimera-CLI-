//! CHTC 桥接器 — 整合协议转换、适配器分发与 EventBus 集成
//!
//! 对应架构:L10 Interface → L1 EventBus(跨层解耦)
//!
//! # 架构铁律 §2.2
//! CHTC 位于 L10,不直接调用下层路由/执行组件。工具调用到达后,
//! 通过 EventBus 发布 `ChtcToolCallReceived` 事件,下层(L6/L7)订阅消费。
//! 这是 L10→下层通信的唯一合法路径。

use crate::adapters::IdeAdapterKind;
use crate::config::ChtcConfig;
use crate::error::ChtcError;
use crate::protocol::ProtocolConverter;
use crate::types::{IdeSource, ToolCallResult, UnifiedToolCall};
use event_bus::{EventBus, EventMetadata, NexusEvent};
use sha2::{Digest, Sha256};

/// CHTC 桥接器 — 跨 IDE 工具调用的统一入口
pub struct ChtcBridge {
    /// 桥接配置(支持的 IDE、超时、并发上限)
    config: ChtcConfig,
    /// 协议转换器(无状态)
    converter: ProtocolConverter,
    /// 可选事件总线,用于向下层广播工具调用事件
    event_bus: Option<EventBus>,
}

impl ChtcBridge {
    /// 创建桥接器(不接入 EventBus,仅做协议转换与本地执行)
    pub fn new(config: ChtcConfig) -> Self {
        Self {
            config,
            converter: ProtocolConverter::new(),
            event_bus: None,
        }
    }

    /// 创建桥接器并接入 EventBus,工具调用将广播给下层订阅者
    pub fn with_event_bus(config: ChtcConfig, bus: EventBus) -> Self {
        Self {
            config,
            converter: ProtocolConverter::new(),
            event_bus: Some(bus),
        }
    }

    /// 获取配置引用
    pub fn config(&self) -> &ChtcConfig {
        &self.config
    }

    /// 接收原生工具调用,归一化为 UnifiedToolCall 并广播事件
    ///
    /// 步骤:
    /// 1. 校验 ide_source 是否受支持
    /// 2. 协议转换为 UnifiedToolCall
    /// 3. 通过 EventBus 发布 `ChtcToolCallReceived`(若已接入)
    ///
    /// WHY 用 `publish_blocking`:`receive` 是同步方法(适配 IDE 同步回调),
    /// 无法 await;`publish_blocking` 内部为 broadcast::send,不阻塞。
    pub fn receive(
        &self,
        raw_call: serde_json::Value,
        ide_source: IdeSource,
    ) -> Result<UnifiedToolCall, ChtcError> {
        if !self.config.is_supported(&ide_source) {
            return Err(ChtcError::UnsupportedIde {
                ide: ide_source.as_str().into(),
            });
        }
        let call = self.converter.receive(raw_call, ide_source)?;
        if let Some(bus) = &self.event_bus {
            let parameters_hash = sha256_hex(&call.parameters);
            let event = NexusEvent::ChtcToolCallReceived {
                metadata: EventMetadata::new("chtc-bridge"),
                call_id: call.call_id.clone(),
                tool_id: call.tool_id.clone(),
                ide_source: call.ide_source.as_str().to_string(),
                parameters_hash,
            };
            bus.publish_blocking(event)
                .map_err(|e| ChtcError::ProtocolError {
                    reason: format!("event publish: {e}"),
                })?;
        }
        Ok(call)
    }

    /// 执行工具调用 — 根据 ide_source 选择适配器
    ///
    /// 本周仅 VSCode 完整实现,其余返回 `NotImplemented`。
    pub fn execute(&self, call: &UnifiedToolCall) -> Result<ToolCallResult, ChtcError> {
        let adapter = IdeAdapterKind::for_source(&call.ide_source);
        adapter.execute(call)
    }
}

/// 计算 JSON Value 的 SHA256 十六进制摘要
///
/// WHY:事件 payload 仅携带参数哈希(而非完整参数),避免大对象
/// 经 EventBus 传播;消费者据哈希去重或拉取具体参数。
fn sha256_hex(value: &serde_json::Value) -> String {
    // serde_json::Value 序列化几乎不会失败;失败时哈希空字节,仍是稳定摘要
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_vscode_raw() -> serde_json::Value {
        serde_json::json!({ "command": "editor.open", "args": { "file": "/x" } })
    }

    #[test]
    fn test_bridge_receive_without_event_bus() {
        let bridge = ChtcBridge::new(ChtcConfig::default());
        let call = bridge
            .receive(sample_vscode_raw(), IdeSource::vscode())
            .expect("转换失败");
        assert_eq!(call.tool_id, "editor.open");
        assert!(!call.call_id.is_empty());
    }

    #[test]
    fn test_bridge_receive_publishes_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let bridge = ChtcBridge::with_event_bus(ChtcConfig::default(), bus);

        let call = bridge
            .receive(sample_vscode_raw(), IdeSource::vscode())
            .expect("转换失败");

        // 验证事件已发布
        let event = rx.try_recv().expect("接收失败");
        let event = event.expect("应有事件");
        match event {
            NexusEvent::ChtcToolCallReceived {
                call_id,
                tool_id,
                ide_source,
                parameters_hash,
                ..
            } => {
                assert_eq!(call_id, call.call_id);
                assert_eq!(tool_id, "editor.open");
                assert_eq!(ide_source, "vscode");
                assert!(!parameters_hash.is_empty());
            }
            other => panic!("期望 ChtcToolCallReceived, 实际: {other:?}"),
        }
    }

    #[test]
    fn test_bridge_receive_unsupported_ide() {
        // 构造仅支持 VSCode 的配置
        let cfg = ChtcConfig {
            supported_ides: vec![IdeSource::vscode()],
            ..Default::default()
        };
        let bridge = ChtcBridge::new(cfg);
        let err = bridge
            .receive(
                serde_json::json!({ "action": "x", "data": {} }),
                IdeSource::zed(),
            )
            .unwrap_err();
        assert!(matches!(err, ChtcError::UnsupportedIde { .. }));
    }

    #[test]
    fn test_bridge_execute_vscode_success() {
        let bridge = ChtcBridge::new(ChtcConfig::default());
        let call = bridge
            .receive(sample_vscode_raw(), IdeSource::vscode())
            .unwrap();
        let result = bridge.execute(&call).expect("执行失败");
        assert!(result.success);
        assert_eq!(result.result["ide"], "vscode");
    }

    #[test]
    fn test_bridge_execute_intellij_not_implemented() {
        let bridge = ChtcBridge::new(ChtcConfig::default());
        let call = bridge
            .receive(
                serde_json::json!({ "action": "a", "params": {} }),
                IdeSource::intellij(),
            )
            .unwrap();
        let err = bridge.execute(&call).unwrap_err();
        assert!(matches!(err, ChtcError::NotImplemented { .. }));
    }

    #[test]
    fn test_bridge_event_metadata_source() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let bridge = ChtcBridge::with_event_bus(ChtcConfig::default(), bus);
        let _ = bridge
            .receive(sample_vscode_raw(), IdeSource::vscode())
            .unwrap();
        let event = rx.try_recv().unwrap().unwrap();
        assert_eq!(event.metadata().source, "chtc-bridge");
    }
}
