//! capability — 能力协商引擎(A2 能力亲和,ADR-066 决策 1/2 落地)
//!
//! 能力协商的唯一职责:把全局 TTG 语义(Fast/Standard/Deep)与请求的工具
//! 需求,映射为**具体通道**可执行的指令,并产出三态保真度(P1 能力协商
//! 取代名字嗅探)。
//!
//! # 协商算法(§5.1,3 函数拆分)
//! 1. `negotiate_thinking`:TTG 三档 ↔ 厂商思考参数三态映射
//!    - `None` → 强制 Off + 降级留痕(thinking_unsupported)
//!    - `OnOff` → Fast=Off,Standard/Deep=On
//!    - `EffortLevels(levels)` → 按档位表就近取档(Fast=最弱,Deep=最强)
//! 2. `negotiate_core`:核心能力(流式/工具调用)校验 → ChannelRejected 判定
//! 3. `negotiate`:顶层组合,产出 `NegotiationOutcome`
//!
//! # 幂等性(proptest 验证)
//! 任意 CapabilitySet × 任意 TTG 档 → 输出确定(纯函数,无随机/时钟依赖),
//! 是路由决策可复现的前提。

use nexus_contracts::affinity::{
    AffinityRequest, CapabilitySet, NegotiationFidelity, ThinkingPreference, ThinkingSupport,
};

/// 思考指令 — 协商产出的抽象思考directive,由 Codec 翻译为方言原生参数
///
/// WHY 抽象指令而非直接方言参数: 协商引擎与 Codec 解耦——引擎只决定
/// "关/开/哪一档",Codec 负责翻译为 `reasoning_effort=xhigh` /
/// `enable_thinking=true` / `thinking.type=adaptive` 等方言原生形态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinkingDirective {
    /// 关闭思考(Fast 档,或不支持思考的通道)
    Off,
    /// 开启思考(OnOff 通道的 Standard/Deep 档)
    On,
    /// 指定档位(EffortLevels 通道,载荷为厂商档位名,如 "xhigh")
    Effort(Box<str>),
}

/// 能力协商结果 — 三态保真度 + 可执行指令 + 降级留痕
#[derive(Debug, Clone, PartialEq)]
pub struct NegotiationOutcome {
    /// 三态保真度(FullFidelity/DegradedNotified/ChannelRejected)
    pub fidelity: NegotiationFidelity,
    /// 思考指令(Codec 据此构造方言参数)
    pub thinking: ThinkingDirective,
    /// 工具调用是否启用(请求需工具但通道不支持 → 通道被拒)
    pub tool_calling_enabled: bool,
    /// 被降级的能力名清单(空 = 全保真;驱动 E4 明确告知)
    pub degraded_capabilities: Vec<String>,
}

/// 协商思考模式:TTG 档 → 厂商思考指令(附是否降级)
///
/// 返回 `(指令, 是否因不支持而降级)`。EffortLevels 就近取档:
/// levels 保序从弱到强,Fast→首档,Standard→中档,Deep→末档。
pub fn negotiate_thinking(
    support: &ThinkingSupport,
    pref: ThinkingPreference,
) -> (ThinkingDirective, bool) {
    match support {
        // 不支持思考:强制 Fast(Off)并标记降级(P3 降级不报错)
        ThinkingSupport::None => (ThinkingDirective::Off, true),
        // 开关两态:Fast=关,Standard/Deep=开
        ThinkingSupport::OnOff => match pref {
            ThinkingPreference::Fast => (ThinkingDirective::Off, false),
            ThinkingPreference::Standard | ThinkingPreference::Deep => {
                (ThinkingDirective::On, false)
            }
        },
        // 多档位:就近取档(空档位域回落 On,不 panic)
        ThinkingSupport::EffortLevels(levels) => {
            if levels.is_empty() {
                return (ThinkingDirective::On, false);
            }
            let idx = match pref {
                ThinkingPreference::Fast => 0,
                // 中档:len/2(奇数取中,偶数偏上,倾向更强推理)
                ThinkingPreference::Standard => levels.len() / 2,
                ThinkingPreference::Deep => levels.len() - 1,
            };
            (ThinkingDirective::Effort(levels[idx].clone()), false)
        }
    }
}

/// 协商核心能力:流式为硬核心;工具调用在请求需要时为硬核心
///
/// 返回 `(通道是否被拒, 工具调用是否启用)`。
fn negotiate_core(capabilities: &CapabilitySet, request_needs_tools: bool) -> (bool, bool) {
    // 流式缺失:通道不可用(ChannelRejected;spec_loader 已在注册期拦截,
    // 此处双保险,防未来动态构造的 spec 绕过)
    if !capabilities.streaming {
        return (true, false);
    }
    // 请求需要工具但通道不支持:该请求无法服务(通道对此请求被拒)
    if request_needs_tools && !capabilities.tool_calling {
        return (true, false);
    }
    (false, capabilities.tool_calling)
}

/// 顶层协商:组合思考 + 核心能力 → 三态保真度
pub fn negotiate(capabilities: &CapabilitySet, request: &AffinityRequest) -> NegotiationOutcome {
    let request_needs_tools = !request.tools.is_empty();
    let (rejected, tool_calling_enabled) = negotiate_core(capabilities, request_needs_tools);

    if rejected {
        let missing = if !capabilities.streaming {
            "streaming"
        } else {
            "tool_calling"
        };
        return NegotiationOutcome {
            fidelity: NegotiationFidelity::ChannelRejected,
            thinking: ThinkingDirective::Off,
            tool_calling_enabled: false,
            degraded_capabilities: vec![missing.to_string()],
        };
    }

    let (thinking, thinking_degraded) =
        negotiate_thinking(&capabilities.thinking, request.thinking_pref);

    // 非核心能力降级留痕:思考不支持(请求偏好非 Fast 却被迫 Off)
    let mut degraded = Vec::new();
    if thinking_degraded && request.thinking_pref != ThinkingPreference::Fast {
        degraded.push("thinking".to_string());
    }

    let fidelity = if degraded.is_empty() {
        NegotiationFidelity::FullFidelity
    } else {
        NegotiationFidelity::DegradedNotified
    };

    NegotiationOutcome {
        fidelity,
        thinking,
        tool_calling_enabled,
        degraded_capabilities: degraded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::affinity::{
        AffinityMessage, AffinityOverrides, ContentBlock, MessageRole, ToolDecl,
    };
    use proptest::prelude::*;

    fn request(pref: ThinkingPreference, with_tools: bool) -> AffinityRequest {
        AffinityRequest {
            intent_id: "i".into(),
            messages: vec![AffinityMessage {
                role: MessageRole::User,
                blocks: vec![ContentBlock::Text { text: "q".into() }],
            }],
            tools: if with_tools {
                vec![ToolDecl {
                    name: "t".into(),
                    description: "d".into(),
                    parameters_schema: "{}".into(),
                }]
            } else {
                Vec::new()
            },
            thinking_pref: pref,
            budget_hint_micro: None,
            overrides: AffinityOverrides::default(),
        }
    }

    fn caps(streaming: bool, tool_calling: bool, thinking: ThinkingSupport) -> CapabilitySet {
        let mut c = CapabilitySet::minimal_text(100_000, 8_192);
        c.streaming = streaming;
        c.tool_calling = tool_calling;
        c.thinking = thinking;
        c
    }

    #[test]
    fn thinking_none_forces_off_and_degrades() {
        let (dir, degraded) = negotiate_thinking(&ThinkingSupport::None, ThinkingPreference::Deep);
        assert_eq!(dir, ThinkingDirective::Off);
        assert!(degraded);
    }

    #[test]
    fn thinking_onoff_maps_fast_off_deep_on() {
        assert_eq!(
            negotiate_thinking(&ThinkingSupport::OnOff, ThinkingPreference::Fast).0,
            ThinkingDirective::Off
        );
        assert_eq!(
            negotiate_thinking(&ThinkingSupport::OnOff, ThinkingPreference::Deep).0,
            ThinkingDirective::On
        );
    }

    #[test]
    fn thinking_effort_levels_nearest_bucket() {
        // GLM 七档:none/minimal/low/medium/high/xhigh/max
        let levels = ThinkingSupport::EffortLevels(vec![
            "none".into(),
            "minimal".into(),
            "low".into(),
            "medium".into(),
            "high".into(),
            "xhigh".into(),
            "max".into(),
        ]);
        // Fast → 首档 none;Deep → 末档 max;Standard → 中档(index 3 = medium)
        assert_eq!(
            negotiate_thinking(&levels, ThinkingPreference::Fast).0,
            ThinkingDirective::Effort("none".into())
        );
        assert_eq!(
            negotiate_thinking(&levels, ThinkingPreference::Deep).0,
            ThinkingDirective::Effort("max".into())
        );
        assert_eq!(
            negotiate_thinking(&levels, ThinkingPreference::Standard).0,
            ThinkingDirective::Effort("medium".into())
        );
    }

    #[test]
    fn negotiate_full_fidelity_when_all_supported() {
        let c = caps(true, true, ThinkingSupport::OnOff);
        let outcome = negotiate(&c, &request(ThinkingPreference::Deep, true));
        assert_eq!(outcome.fidelity, NegotiationFidelity::FullFidelity);
        assert!(outcome.tool_calling_enabled);
        assert!(outcome.degraded_capabilities.is_empty());
    }

    #[test]
    fn negotiate_degraded_when_thinking_unsupported() {
        // 请求 Deep 但通道不支持思考 → DegradedNotified + thinking 留痕
        let c = caps(true, true, ThinkingSupport::None);
        let outcome = negotiate(&c, &request(ThinkingPreference::Deep, false));
        assert_eq!(outcome.fidelity, NegotiationFidelity::DegradedNotified);
        assert_eq!(outcome.degraded_capabilities, vec!["thinking".to_string()]);
        assert_eq!(outcome.thinking, ThinkingDirective::Off);
    }

    #[test]
    fn negotiate_fast_with_no_thinking_is_full_fidelity() {
        // 请求 Fast + 通道不支持思考 → 无降级(Fast 本就不需思考)
        let c = caps(true, true, ThinkingSupport::None);
        let outcome = negotiate(&c, &request(ThinkingPreference::Fast, false));
        assert_eq!(outcome.fidelity, NegotiationFidelity::FullFidelity);
        assert!(outcome.degraded_capabilities.is_empty());
    }

    #[test]
    fn negotiate_rejects_when_streaming_missing() {
        let c = caps(false, true, ThinkingSupport::OnOff);
        let outcome = negotiate(&c, &request(ThinkingPreference::Fast, false));
        assert_eq!(outcome.fidelity, NegotiationFidelity::ChannelRejected);
        assert_eq!(outcome.degraded_capabilities, vec!["streaming".to_string()]);
    }

    #[test]
    fn negotiate_rejects_tool_request_on_non_tool_channel() {
        // 请求需工具但通道不支持工具 → 该请求被拒(N1:工具任务路由掩零)
        let c = caps(true, false, ThinkingSupport::OnOff);
        let outcome = negotiate(&c, &request(ThinkingPreference::Fast, true));
        assert_eq!(outcome.fidelity, NegotiationFidelity::ChannelRejected);
        assert_eq!(
            outcome.degraded_capabilities,
            vec!["tool_calling".to_string()]
        );
    }

    #[test]
    fn negotiate_allows_non_tool_request_on_non_tool_channel() {
        // 纯对话请求 + 无工具通道 → 不拒(工具能力未被需要)
        let c = caps(true, false, ThinkingSupport::OnOff);
        let outcome = negotiate(&c, &request(ThinkingPreference::Standard, false));
        assert_eq!(outcome.fidelity, NegotiationFidelity::FullFidelity);
        assert!(!outcome.tool_calling_enabled);
    }

    // 协商幂等性:任意能力集 × 任意 TTG 档 → 输出确定(纯函数)
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(300))]
        #[test]
        fn negotiate_is_idempotent(
            streaming in any::<bool>(),
            tool_calling in any::<bool>(),
            support_kind in 0u8..3,
            n_levels in 1usize..8,
            pref_kind in 0u8..3,
            with_tools in any::<bool>(),
        ) {
            let thinking = match support_kind {
                0 => ThinkingSupport::None,
                1 => ThinkingSupport::OnOff,
                _ => ThinkingSupport::EffortLevels(
                    (0..n_levels).map(|i| format!("lvl{i}").into()).collect()
                ),
            };
            let pref = match pref_kind {
                0 => ThinkingPreference::Fast,
                1 => ThinkingPreference::Standard,
                _ => ThinkingPreference::Deep,
            };
            let c = caps(streaming, tool_calling, thinking);
            let req = request(pref, with_tools);
            let a = negotiate(&c, &req);
            let b = negotiate(&c, &req);
            prop_assert_eq!(a, b, "协商必须幂等(相同输入相同输出)");
        }
    }

    // EffortLevels 协商产出的档位必须在声明域内(不臆造档位)
    proptest! {
        #[test]
        fn effort_directive_always_in_declared_domain(
            n_levels in 1usize..10,
            pref_kind in 0u8..3,
        ) {
            let levels: Vec<Box<str>> = (0..n_levels).map(|i| format!("lvl{i}").into()).collect();
            let support = ThinkingSupport::EffortLevels(levels.clone());
            let pref = match pref_kind {
                0 => ThinkingPreference::Fast,
                1 => ThinkingPreference::Standard,
                _ => ThinkingPreference::Deep,
            };
            let (dir, _) = negotiate_thinking(&support, pref);
            if let ThinkingDirective::Effort(chosen) = dir {
                prop_assert!(levels.contains(&chosen), "选中档位必在声明域内");
            } else {
                prop_assert!(false, "EffortLevels 应产出 Effort 指令");
            }
        }
    }
}
