//! 序列化工具 — 用于跨进程投递(MCP Mesh)与持久化
//!
//! 拆分自原 `bus.rs`（ADR-314 W6 / BC-01：按职责切分巨型文件）。
//!
//! WHY 独立成模块：这四个函数是**纯编解码**，不触碰总线状态机
//! （无 `self`、无锁、无通道操作），与发布/订阅/分片三类职责正交。
//! 留在 `bus.rs` 会让"总线本体"的边界模糊——读代码的人要在一屏内
//! 跳过编解码才能看到投递语义。
//!
//! ADR-004:序列化协议为 MessagePack；JSON 是降级通道（调试与兼容场景）。

use crate::error::EventBusError;
use crate::types::NexusEvent;

/// 将事件序列化为 MessagePack 字节(ADR-004)
///
/// 跨进程通信(MCP Mesh)与事件日志持久化时使用。
pub fn serialize_msgpack(event: &NexusEvent) -> Result<Vec<u8>, EventBusError> {
    rmp_serde::to_vec_named(event).map_err(EventBusError::from)
}

/// 从 MessagePack 字节反序列化事件
pub fn deserialize_msgpack(bytes: &[u8]) -> Result<NexusEvent, EventBusError> {
    rmp_serde::from_slice(bytes).map_err(EventBusError::from)
}

/// 将事件序列化为 JSON 字符串(降级通道,调试与兼容场景)
///
/// WHY:MessagePack 不可读,调试时 JSON 更直观;
/// 部分 MCP 客户端可能仅支持 JSON
pub fn serialize_json(event: &NexusEvent) -> Result<String, EventBusError> {
    serde_json::to_string(event).map_err(EventBusError::from)
}

/// 从 JSON 字符串反序列化事件
pub fn deserialize_json(s: &str) -> Result<NexusEvent, EventBusError> {
    serde_json::from_str(s).map_err(EventBusError::from)
}
