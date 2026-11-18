//! TUI / Agent 交互辅助类型
//!
//! WHY 独立成模块：本组类型只服务于 `TUI / Agent 交互辅助类型` 一个领域，
//! 与其它域的载荷无耦合（同文件平铺时改任一领域都要翻找全部类型）。
//!
//! 对外路径不变：全部类型经 `payloads/mod.rs` 的 facade 转出，
//! `event_bus::payloads::X` 逐字不变（ADR-314 W6 / BC-03 域下沉）。
//!
//! 类型：`TuiChatMessagePayload` / `ConsultUrgency` / `ActionSource` /
//! `ChatStatus` / `CompatLevel`（TUI 交互协议与握手兼容级别）。

use serde::{Deserialize, Serialize};

/// TUI 聊天消息载荷 — 角色以字符串编码(L1 不感知 L10 的 ChatRole 枚举)
///
/// WHY 定义在 event-bus:`/compact` 策展(FC-2,ADR-081)的压缩结果需经
/// `TuiChatHistoryReplaced` 事件整体回写会话历史;ChatMessage 类型归属
/// chimera-tui(L10),event-bus(L1)不能反向依赖,故以字符串角色
/// ("user"/"assistant"/"system")承载,转换由消费端(ChatSync)负责。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TuiChatMessagePayload {
    /// 消息角色:"user" / "assistant" / "system"
    pub role: String,
    /// 消息内容
    pub content: String,
}

/// 咨询紧急度 — Agent 咨询请求(AgentConsultRequested)的紧急级别
///
/// WHY 独立定义在 event-bus:同 TaskPriority,避免 L1→L9 向上依赖。
/// 用于 Agent 间咨询请求的优先级标注,影响被咨询 Agent 的响应顺序。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ConsultUrgency {
    /// 低紧急度
    Low,
    /// 中等紧急度
    Medium,
    /// 高紧急度
    High,
    /// 最高紧急度(立即响应)
    Critical,
}

// AgentStatus 已下沉 L0(ADR-054 决策 6,P9-T7 Task 4),见文件顶部 re-export。

/// TUI 交互动作来源 — `TuiActionRequested` 事件的触发入口标识(ADR-029)
///
/// WHY 独立定义在 event-bus:TUI 交互协议(Action)是 L10 与编排层
/// (chimera-cli)之间经 L1 EventBus 通信的契约,来源标识用于审计与
/// UI 反馈定位(区分同一 Action 由哪个入口触发)。三入口共享同一
/// Action 协议,行为一致性由 chimera-tui 的 ActionRegistry 单源保证。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ActionSource {
    /// 来自 Chat 面板的斜杠命令
    Chat,
    /// 来自命令面板(模糊搜索)
    Palette,
    /// 来自面板上下文动作(焦点面板 Enter/Space 唤出)
    Panel,
}

/// TUI 交互式对话状态 — `TuiChatStatusChanged` 事件携带的 Agent 会话状态(ADR-029)
///
/// WHY 独立定义在 event-bus:与 `AgentStatus`(chimera-mas 多 Agent 生命周期)
/// 区分——`ChatStatus` 面向单条交互式会话的 UI 呈现(思考中/工具执行中/空闲),
/// 由 chimera-cli 编排器在流式回答过程中广播,驱动 Chat 面板状态指示器。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum ChatStatus {
    /// 思考中(已提交查询,等待/生成首 token)
    Thinking,
    /// 工具执行中(Agent 调用工具,暂停 token 流)
    ToolExecuting,
    /// 空闲(本轮完成,等待下一次输入)
    ///
    /// WHY `#[default]`:`DataSnapshot`/`TuiState` 派生 `Default`,对话初始态
    /// 天然为"空闲"(尚未提交任何查询)。
    #[default]
    Idle,
}

/// TUI ↔ 编排器协议握手兼容级别(Concord W10 T10.1,ADR-082)
///
/// WHY 独立定义在 event-bus:`TuiHelloAck` 携带本枚举跨进程/跨层传递,
/// TUI 与 chimera-cli 编排器双方均需消费。防 Codex #37536 式版本偏移
/// 静默故障:不兼容时显式降级/拒绝,而非静默缺失功能。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompatLevel {
    /// 完全兼容:M4 通道全开
    Full,
    /// 降级兼容:携带降级项列表(banner 展示,orchestrated 命令部分禁用)
    Degraded(Vec<String>),
    /// 拒绝:版本不可调和,banner 警告 + orchestrated 命令只读化
    Refused,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn roundtrip<T>(value: &T) -> T
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let json = serde_json::to_string(value).unwrap();
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn chat_message_payload_roundtrip_at_boundaries() {
        // 空串边界:两个字段都为空时线格式不得丢字段
        let empty = TuiChatMessagePayload {
            role: String::new(),
            content: String::new(),
        };
        assert_eq!(roundtrip(&empty), empty);

        let filled = TuiChatMessagePayload {
            role: "assistant".into(),
            content: "多行\n内容\u{1F600}".into(),
        };
        assert_eq!(roundtrip(&filled), filled);
    }

    #[test]
    fn consult_urgency_roundtrip_all_variants() {
        for value in [
            ConsultUrgency::Low,
            ConsultUrgency::Medium,
            ConsultUrgency::High,
            ConsultUrgency::Critical,
        ] {
            assert_eq!(roundtrip(&value), value);
        }
    }

    #[test]
    fn action_source_and_chat_status_roundtrip() {
        for value in [
            ActionSource::Chat,
            ActionSource::Palette,
            ActionSource::Panel,
        ] {
            assert_eq!(roundtrip(&value), value);
        }
        for value in [
            ChatStatus::Thinking,
            ChatStatus::ToolExecuting,
            ChatStatus::Idle,
        ] {
            assert_eq!(roundtrip(&value), value);
        }
        // 派生 Default 的语义锚点:会话初始态是"空闲"
        assert_eq!(ChatStatus::default(), ChatStatus::Idle);
    }

    #[test]
    fn compat_level_roundtrip_including_empty_degradation_list() {
        for value in [
            CompatLevel::Full,
            CompatLevel::Refused,
            CompatLevel::Degraded(Vec::new()),
            CompatLevel::Degraded(vec!["m4-channel".into(), String::new()]),
        ] {
            assert_eq!(roundtrip(&value), value);
        }
    }
}
