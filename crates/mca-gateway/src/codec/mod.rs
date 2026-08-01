//! Codec — 三协议方言码器的 enum 分发入口(C4 红线:enum 优于 trait object)
//!
//! # 方言保真原则(P2)
//! 请求侧**不做统一中间表示**——各 Codec 直接从 `AffinityRequest` 构造方言
//! 原生请求体,厂商专有参数不经过"最小公分母"抹平(LiteLLM 教训:统一格式
//! 丢厂商特性,Qwen/GLM 官方均承认兼容层丢特性)。
//! 响应侧统一为 `DecodedResponse`(统一块模型),对齐 Anthropic 内容块语义。
//!
//! # M0 范围
//! M0 只声明 OpenAiChat / Anthropic 两变体;M1 追加 Responses 变体时,
//! 编译器将强制补全本文件所有 match(穷尽性检查),优于 `todo!()` 占位
//! (项目铁律:零 `todo!()`/`unimplemented!()`)。
//!
//! # M1 状态
//! 三方言均已落地(OpenAiChat / Anthropic / Responses),`for_dialect` 全覆盖。

use nexus_contracts::affinity::{
    AffinityRequest, ContentBlock, FinishReason, ModelAffinitySpec, ProtocolDialect, UsageReport,
};

use crate::error::AffinityError;

pub mod anthropic;
pub mod openai_chat;
pub mod responses;

pub use anthropic::AnthropicCodec;
pub use openai_chat::OpenAiChatCodec;
pub use responses::ResponsesCodec;

/// 解码后的线上响应 — Codec 的统一输出(块模型 + 计量 + 终止原因)
///
/// WHY 不直接输出 `AffinityResponse`: 成本(cost)与回执(receipt)需要
/// spec 定价与路由上下文才能装配,是适配器(VendorAdapter)的职责;
/// Codec 只负责"字节 → 语义块"的纯解码,职责单一便于录播测试。
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedResponse {
    /// 统一内容块序列(Text / Thinking / ToolUse)
    pub blocks: Vec<ContentBlock>,
    /// token 计量(缓存命中数厂商可选返回)
    pub usage: UsageReport,
    /// 归一后的终止原因
    pub finish_reason: FinishReason,
    /// 厂商侧请求标识(问题排查/限流申诉)
    pub request_id: Option<Box<str>>,
}

/// 协议码器 — 三方言 enum 分发
///
/// 每个变体是独立无状态码器;方言选择由 `ModelAffinitySpec::preferred_dialect()`
/// 决定(含 PreferDialect 怪癖,如 Kimi 钉住 Anthropic 路径)。
#[derive(Debug, Clone)]
pub enum Codec {
    /// OpenAI Chat Completions 方言(`/v1/chat/completions`)
    OpenAiChat(OpenAiChatCodec),
    /// Anthropic Messages 方言(`/v1/messages`,thinking/tool_use 块原生)
    Anthropic(AnthropicCodec),
    /// OpenAI Responses API 方言(`/responses`,DeepSeek V4-Flash 原生支持)
    Responses(ResponsesCodec),
}

impl Codec {
    /// 按方言构造对应码器
    ///
    /// M1 起三方言均已落地,不再返回 None。保留 Option 签名以兼容
    /// 未来可能新增的待实现方言(如 DashScope 原生第四方言,M2 可选)。
    pub fn for_dialect(dialect: ProtocolDialect) -> Option<Self> {
        match dialect {
            ProtocolDialect::OpenAiChat => Some(Self::OpenAiChat(OpenAiChatCodec)),
            ProtocolDialect::AnthropicMessages => Some(Self::Anthropic(AnthropicCodec)),
            ProtocolDialect::OpenAiResponses => Some(Self::Responses(ResponsesCodec)),
        }
    }

    /// 本码器对应的协议方言
    pub fn dialect(&self) -> ProtocolDialect {
        match self {
            Self::OpenAiChat(_) => ProtocolDialect::OpenAiChat,
            Self::Anthropic(_) => ProtocolDialect::AnthropicMessages,
            Self::Responses(_) => ProtocolDialect::OpenAiResponses,
        }
    }

    /// 构造非流式请求体(方言原生 JSON,P2 保真)
    pub fn build_request(
        &self,
        spec: &ModelAffinitySpec,
        request: &AffinityRequest,
    ) -> Result<serde_json::Value, AffinityError> {
        match self {
            Self::OpenAiChat(codec) => codec.build_request(spec, request),
            Self::Anthropic(codec) => codec.build_request(spec, request),
            Self::Responses(codec) => codec.build_request(spec, request),
        }
    }

    /// 解析非流式响应体(字节 → 统一块模型)
    pub fn parse_response(&self, body: &[u8]) -> Result<DecodedResponse, AffinityError> {
        match self {
            Self::OpenAiChat(codec) => codec.parse_response(body),
            Self::Anthropic(codec) => codec.parse_response(body),
            Self::Responses(codec) => codec.parse_response(body),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_dialect_covers_all_three() {
        assert!(matches!(
            Codec::for_dialect(ProtocolDialect::OpenAiChat),
            Some(Codec::OpenAiChat(_))
        ));
        assert!(matches!(
            Codec::for_dialect(ProtocolDialect::AnthropicMessages),
            Some(Codec::Anthropic(_))
        ));
        // M1: Responses 码器已落地,三方言全覆盖
        assert!(matches!(
            Codec::for_dialect(ProtocolDialect::OpenAiResponses),
            Some(Codec::Responses(_))
        ));
    }

    #[test]
    fn dialect_roundtrip() {
        for d in [
            ProtocolDialect::OpenAiChat,
            ProtocolDialect::AnthropicMessages,
            ProtocolDialect::OpenAiResponses,
        ] {
            let codec = Codec::for_dialect(d).unwrap();
            assert_eq!(codec.dialect(), d);
        }
    }
}
