//! model-visible 白名单投影 — 会话事件 → 模型可见视图（WI-18）
//!
//! 对应架构层: **L3 Storage**（session-store,Phase 2 新增,ADR-141）
//! 对应任务: **P2-T3**（v4.0 WI-18:「model-visible means logged」不变量）
//!
//! # 「model-visible means logged」不变量
//!
//! 模型可见的事件 **必然已落盘**（append-only 段,`flush` 后 fsync 确认）——
//! 投影不产生新事件,只对已记录事件做白名单过滤 + 敏感字段剔除:
//!
//! - **白名单**:仅 [`MODEL_VISIBLE_TYPES`] 中的事件类型可投影（模型对话
//!   相关的 Prompt/ToolCall/Result 语义;其余类型如内部遥测不出现在模型视图中）
//! - **敏感剔除**:载荷（payload）含敏感标记（secret/credential/api_key/
//!   token/password 等,大小写不敏感子串）时**载荷整体过滤**——只记录
//!   「发生了该事件」,不记录载荷内容;事件本体与元数据保留
//!
//! # WHY 启发式子串而非结构化解析
//! `SessionEvent.payload` 是调用方自编码的不透明字节（存储层不透传解析,
//! types.rs 文档声明）——投影层不做协议感知,用保守的字节扫描标记敏感
//! 内容（宁可多滤不可漏滤:过度过滤只损失信息,漏滤则泄漏敏感数据）。
//!
//! # 红线
//!
//! 纯函数投影（无 IO 无状态）——可安全用于日志/审计/模型上下文组装。

use serde::{Deserialize, Serialize};

use crate::types::SessionEvent;

/// 白名单事件类型 — PromptBuilt / ToolCall / Result 语义的代表（5 个）:
///
/// 对齐 L0 `OmniMessage`（omni_message.rs）的六变体语义:
/// ModelRequest(≈PromptBuilt) / ModelResponse / ToolRequest(≈ToolCall) /
/// ToolResult(≈Result) / StateUpdate。内部遥测事件（如 "session.flushed"）
/// 不在白名单——模型视图只包含对话/工具执行语义。
pub const MODEL_VISIBLE_TYPES: [&str; 5] = [
    "model.request",  // PromptBuilt 语义:提示词构建完成
    "model.response", // 模型响应
    "tool.request",   // ToolCall 语义:工具调用请求
    "tool.result",    // Result 语义:工具执行结果
    "state.update",   // 状态同步
];

/// 敏感载荷标记 — 载荷字节含任一标记（大小写不敏感）即整包过滤
///
/// WHY 保守集合:密钥/凭据类字段命名在业界高度趋同,覆盖常见英文命名;
/// 中文/混淆键名不在覆盖范围（启发式边界,文档注明——结构化转译层可扩展）
pub const SENSITIVE_MARKERS: [&str; 9] = [
    "secret",
    "credential",
    "api_key",
    "apikey",
    "password",
    "authorization",
    "bearer ",
    "access_token",
    "private_key",
];

/// 模型可见事件 — 白名单投影结果（敏感字段已剔除）
///
/// # 不变量
/// `offset` 即落盘 Offset.seq（model-visible means logged:投影只来自
/// 已持久化事件,`offset` 是其在段文件中的全局序列号镜像）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelVisibleEvent {
    /// 全局序列号（= 落盘 Offset.seq）
    pub offset: u64,
    /// 事件类型（白名单内）
    pub event_type: String,
    /// 事件时间戳（Unix 毫秒;审计排序）
    pub timestamp_ms: i64,
    /// 载荷（None = 敏感已剔除 / 原事件无载荷）
    pub payload: Option<Vec<u8>>,
}

/// 白名单投影 — `SessionEvent` → 模型可见视图
///
/// # 返回
/// - `Some(view)`:事件类型在白名单且载荷（若含敏感字段）已剔除
/// - `None`:事件类型不在白名单（模型不可见;事件仍已记录,仅不投影）
///
/// # 「model-visible means logged」注释
/// 投影只读已落盘事件,不新增事件;被过滤的敏感载荷**仍存储**（审计
/// 需要原始数据）,只是不出现在模型可见视图——logged ≠ model-visible,
/// model-visible ⊂ logged。
#[must_use]
pub fn to_model_view(event: &SessionEvent, offset: u64) -> Option<ModelVisibleEvent> {
    if !MODEL_VISIBLE_TYPES.contains(&event.event_type.as_str()) {
        return None;
    }
    let payload = event.payload.as_ref().and_then(|p| {
        if contains_sensitive(p) {
            None
        } else {
            Some(p.clone())
        }
    });
    Some(ModelVisibleEvent {
        offset,
        event_type: event.event_type.clone(),
        timestamp_ms: event.metadata.timestamp.timestamp_millis(),
        payload,
    })
}

/// 敏感载荷检测 — 大小写不敏感子串匹配任一标记
fn contains_sensitive(bytes: &[u8]) -> bool {
    // WHY to_lowercase 后匹配:JSON 载荷的键名大小写不定（"API_KEY"/"apiKey"）
    // 统一小写后按标记匹配;lossy 转换容忍非 UTF-8 载荷（二进制事件不误伤,
    // 但标记匹配不到——启发式边界,文档注明）
    let lower = String::from_utf8_lossy(bytes).to_lowercase();
    SENSITIVE_MARKERS.iter().any(|m| lower.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SessionEvent;

    #[test]
    fn whitelist_types_projected() {
        // 白名单 5 类型全部可投影（model-visible 子集完整）
        for (i, et) in MODEL_VISIBLE_TYPES.iter().enumerate() {
            let ev = SessionEvent::with_payload(*et, vec![i as u8]);
            let view = to_model_view(&ev, i as u64).expect("白名单必须投影");
            assert_eq!(view.event_type, *et);
            assert_eq!(view.offset, i as u64);
            assert_eq!(view.payload, Some(vec![i as u8]), "无敏感载荷原样保留");
        }
    }

    #[test]
    fn non_whitelist_types_filtered() {
        // 非白名单类型 → None（已记录但不投影）
        let ev = SessionEvent::new("session.flushed");
        assert!(to_model_view(&ev, 0).is_none(), "内部遥测不投影");
        let ev2 = SessionEvent::new("tool.result"); // 白名单内
        assert!(to_model_view(&ev2, 0).is_some());
    }

    #[test]
    fn sensitive_payload_stripped() {
        // 含 secret 的载荷被整体剔除（model-visible means logged:事件仍记录,
        // 载荷不外泄）
        let ev = SessionEvent::with_payload(
            "tool.result",
            br#"{"tool":"read_file","secret_key":"sk-abc123"}"#.to_vec(),
        );
        let view = to_model_view(&ev, 3).expect("投影");
        assert_eq!(view.offset, 3);
        assert!(view.payload.is_none(), "敏感载荷必须剔除");
    }

    #[test]
    fn sensitive_markers_case_insensitive() {
        // 大小写不敏感:API_KEY / ApiKey / authorization 均命中
        for marker in [
            "API_KEY",
            "ApiKey",
            "Authorization",
            "PASSWORD",
            "Bearer eyJhbGci",
        ] {
            let json = format!(r#"{{"{}": "value"}}"#, marker);
            let ev = SessionEvent::with_payload("model.request", json.into_bytes());
            let view = to_model_view(&ev, 1).expect("投影");
            assert!(view.payload.is_none(), "标记 {marker} 必须命中");
        }
    }

    #[test]
    fn benign_payload_kept() {
        // 无敏感标记的正常载荷原样保留（不过度过滤）
        let ev = SessionEvent::with_payload(
            "model.response",
            "{\"content\":\"普通回复文本\",\"tokens\":42}"
                .as_bytes()
                .to_vec(),
        );
        let view = to_model_view(&ev, 5).expect("投影");
        assert_eq!(
            view.payload,
            Some(
                "{\"content\":\"普通回复文本\",\"tokens\":42}"
                    .as_bytes()
                    .to_vec()
            )
        );
    }

    #[test]
    fn non_utf8_payload_does_not_panic() {
        // 非 UTF-8 二进制载荷:lossy 扫描,不 panic,不误判敏感
        let ev = SessionEvent::with_payload("state.update", vec![0xFF, 0x00, 0xFE, 0x12]);
        let view = to_model_view(&ev, 2).expect("投影");
        assert!(view.payload.is_some(), "二进制载荷保留(未命中敏感标记)");
    }
}
