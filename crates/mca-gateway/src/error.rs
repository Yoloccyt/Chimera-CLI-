//! 亲和错误类型 — mca-gateway 库层错误(thiserror,§4.1 库层约定)
//!
//! 五大类错误对应通道调用的五个失败面:传输 / 协议 / 能力 / 配额 / 未知。
//! 与三态降级协议(ADR-066 预告)的关系:`Capability` 类错误不一定终止
//! 请求——能力协商优先走 DegradedNotified 降级,仅核心能力缺失
//! (不支持流式/工具调用)才升级为 `ChannelRejected` 语义的硬错误。

use nexus_contracts::affinity::{ProtocolDialect, ProviderId};
use thiserror::Error;

/// mca-gateway 统一错误类型
///
/// # 设计决策(WHY)
/// - **五变体闭集**: 对齐设计文档 §4.2 error.rs 规格
///   (Transport/Protocol/Capability/Quota/Unknown),调用方可穷尽 match
///   做差异化处理(重试 / 降级 / 切通道 / 留痕)
/// - **携带结构化上下文**: provider/dialect/route_key 入错误体,
///   降级链(csn-substitutor)与健康探针无需解析错误字符串
#[derive(Debug, Error)]
pub enum AffinityError {
    /// 传输层错误 — 网络失败/超时/连接拒绝(可重试面)
    ///
    /// WHY retryable 字段: 429/5xx 可退避重试,DNS 失败/证书错误不可;
    /// transport 层重试策略据此分流,避免对不可恢复错误做无效退避。
    #[error("transport error on route '{route_key}': {reason} (retryable: {retryable})")]
    Transport {
        /// 路由键(provider/model)
        route_key: String,
        /// 失败原因描述
        reason: String,
        /// 是否可重试(429/5xx/超时 = true;DNS/TLS = false)
        retryable: bool,
    },

    /// 协议层错误 — 请求构造失败或响应解析失败(Codec 面)
    #[error("protocol error ({dialect:?}): {reason}")]
    Protocol {
        /// 出错的协议方言
        dialect: ProtocolDialect,
        /// 失败原因(如缺失必需字段/JSON 结构不符)
        reason: String,
    },

    /// 能力协商错误 — 核心能力缺失,通道不可用(ChannelRejected 语义)
    ///
    /// WHY 仅核心能力入错误: 非核心能力(思考模式/缓存)缺失走降级留痕
    /// (P3 容错),不报错中断;只有流式/工具调用等核心能力缺失才拒绝通道。
    #[error(
        "capability rejected for provider {provider:?}: missing core capability '{capability}'"
    )]
    Capability {
        /// 被拒绝的厂商
        provider: ProviderId,
        /// 缺失的核心能力名(如 "streaming" / "tool_calling")
        capability: String,
    },

    /// 配额错误 — 厂商额度耗尽/限流持续(触发 AffinityQuotaExhausted Critical 事件)
    #[error("quota exhausted on route '{route_key}': {reason}")]
    Quota {
        /// 路由键(provider/model)
        route_key: String,
        /// 配额失败细节(厂商错误码原文留存)
        reason: String,
    },

    /// 未知错误 — 无法归类的厂商侧异常(P3 容错兜底,原文留痕)
    ///
    /// WHY 保留原文: 未知错误是 spec 更新的驱动信号
    /// (AffinityUnknownField 事件同源),原文入日志辅助排查。
    #[error("unknown error: {raw}")]
    Unknown {
        /// 厂商返回的原始错误文本
        raw: String,
    },
}

impl AffinityError {
    /// 该错误是否值得传输层重试
    ///
    /// WHY: 重试决策集中一处,避免调用方各自解读错误语义产生分歧。
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Transport {
                retryable: true,
                ..
            }
        )
    }

    /// 该错误是否应触发通道降级链(csn-substitutor 切换通道)
    ///
    /// WHY: Quota(额度耗尽)与 Capability(核心能力缺失)都意味着
    /// "此通道当前不可用",应切换而非重试;Transport 不可重试错误
    /// 由熔断器累计后间接触发降级,不在此直接判定。
    pub fn should_degrade(&self) -> bool {
        matches!(self, Self::Quota { .. } | Self::Capability { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_retryable_flag() {
        let retryable = AffinityError::Transport {
            route_key: "zhipu/glm-5.2".into(),
            reason: "429 Too Many Requests".into(),
            retryable: true,
        };
        assert!(retryable.is_retryable());
        assert!(!retryable.should_degrade());

        let fatal = AffinityError::Transport {
            route_key: "zhipu/glm-5.2".into(),
            reason: "TLS handshake failed".into(),
            retryable: false,
        };
        assert!(!fatal.is_retryable());
    }

    #[test]
    fn quota_and_capability_trigger_degradation() {
        let quota = AffinityError::Quota {
            route_key: "deep_seek/deepseek-v4-flash".into(),
            reason: "monthly quota exceeded".into(),
        };
        assert!(quota.should_degrade());

        let capability = AffinityError::Capability {
            provider: ProviderId::StepFun,
            capability: "tool_calling".into(),
        };
        assert!(capability.should_degrade());
        assert!(!capability.is_retryable());
    }

    #[test]
    fn error_display_contains_context() {
        // WHY: 错误消息是排查一线信息,route_key/方言必须出现在 Display 中
        let e = AffinityError::Protocol {
            dialect: ProtocolDialect::AnthropicMessages,
            reason: "missing 'content' field".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("AnthropicMessages"), "msg = {msg}");
        assert!(msg.contains("missing 'content' field"));
    }
}
