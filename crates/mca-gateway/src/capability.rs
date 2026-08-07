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
    AffinityRequest, CapabilitySet, NegotiationFidelity, OutputBudget, ThinkingPreference,
    ThinkingSupport,
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

// ============================================================
// 输出预算协商（ADR-069 Token 效率优化）
// ============================================================

/// 预算档位配置 — TTG 档 × (thinking_budget, response_reserve) 的类型化绑定
///
/// WHY 结构体而非散列常量: 6 个独立常量的对应关系仅靠命名后缀维系,
/// 新增档位需同步修改 2 处; 类型化后编译器保证配对完整性。
struct BudgetTierConfig {
    /// 思考预算上限(token)
    thinking_budget: u32,
    /// 响应预留空间(token)— thinking 不能占满 max_output
    response_reserve: u32,
}

/// WHY 8K/32K/128K(二轮升级, ADR-069): 对齐 2026 年主力模型实际 max_output
/// (Kimi K3=65536, GLM-5.2=131072, DeepSeek V4=393216)。
///
/// | 档位 | thinking | 模型容量占用 | 设计意图 |
/// |------|----------|-------------|--------|
/// | Fast=8K | 8_192 | K3 的 12.5% | 简单问答/格式转换, 响应速度优先 |
/// | Standard=32K | 32_768 | K3 的 50% | 代码生成/多步推理, 覆盖大多数日常场景 |
/// | Deep=128K | 131_072 | GLM 满档/DSV4 占 1/3 | 架构设计/长链推理, 释放全部推理 |
///
/// 响应预留: thinking 不能占满 max_output, 必须为实际回复留空间。
/// 有效 thinking = min(档位预算, max_output - response_reserve)
const BUDGET_TIERS: [BudgetTierConfig; 3] = [
    BudgetTierConfig {
        thinking_budget: 8_192,
        response_reserve: 2_048,
    }, // Fast
    BudgetTierConfig {
        thinking_budget: 32_768,
        response_reserve: 8_192,
    }, // Standard
    BudgetTierConfig {
        thinking_budget: 131_072,
        response_reserve: 16_384,
    }, // Deep
];

/// WHY 独立成本地板: budget_hint 约束时的绝对下限，与 Fast 档解耦。
/// 旧实现用 THINKING_BUDGET_FAST 做地板，Fast 档提升后地板跟着涨，
/// 削弱成本护栏效力。独立常量保持成本约束的精细度。
const BUDGET_HINT_FLOOR: u32 = 2_048;

/// Deep 档成本护栏: budget_hint_micro(微元) → 可承受 thinking 上限
///
/// WHY /10: 保守按最贵厂商 Kimi K3 输出价 ~10M 微元/Mtok 估算,
/// hint ÷ 10M = hint/10 可购买 token 数。
/// 智能降级(非硬截断): 结果钳制在 [BUDGET_HINT_FLOOR, base]——
/// 低于地板保最低思考深度(hint 是提示, 硬预算由 acb-governor 治理),
/// 高于档位仍封顶在 base。
///
/// WHY 先按 base 钳制再截断: 旧实现 (hint/10) as u32 在 hint > 42.9B
/// 微元时静默回绕; affordable ≤ base ≤ 131072 < u32::MAX, 截断必然安全。
/// 用 max/min 组合而非 clamp: clamp 在 (FLOOR > base) 的未来档位会 panic,
/// 本式恒 total。
fn deep_budget_with_cost_hint(base: u32, hint_micro: Option<u64>) -> u32 {
    match hint_micro {
        None => base,
        Some(hint) => {
            let affordable = (hint / 10).min(u64::from(base)) as u32;
            affordable.max(BUDGET_HINT_FLOOR).min(base)
        }
    }
}

/// 协商输出预算：TTG 档 × 模型 max_output → 具体 token 数
///
/// # 算法(4 步流水线)
/// 1. 思考不支持 → 不分配 thinking budget(与 negotiate_thinking 的 None→Off 语义一致)
/// 2. 按 ThinkingPreference 确定 thinking_budget 档位 + 响应预留
/// 3. Deep 档受 budget_hint_micro 成本护栏约束(智能降级而非硬截断, 见 deep_budget_with_cost_hint)
/// 4. 模型容量钳制: thinking + response 必须装进 max_output, .max(1) 兜底
///
/// # max_output_tokens = 厂商上限(不随档位收紧)
/// 1. max_tokens 是硬上限非预留, 缩小只截断长回复不省钱(Fast 档收至 10K
///    会截断长格式转换输出), 成本治理交给 budget_hint_micro 护栏
/// 2. 档位联动已由 thinking 钳制到 ≤ max_output - reserve 实现——Deep 档的
///    max_output 天然包含 thinking + response 两部分空间(thinking 是子分配)
/// 3. K3 reasoning 按输出计费下 thinking ≤ 49152, 总输出恒 ≤ max_output=65536, 不超限
///
/// # K3 reasoning 按输出计费专项
/// Kimi K3 的 thinking token 按输出价计费(~10M 微元/Mtok), Deep 档无约束时
/// 在 DSV4 上可能产生 128K thinking token, 费用爆炸。budget_hint_micro 是上层成本护栏。
pub fn negotiate_budget(
    capabilities: &CapabilitySet,
    pref: ThinkingPreference,
    budget_hint_micro: Option<u64>,
) -> OutputBudget {
    let max_output = capabilities.max_output;

    // 思考不支持时，不分配 thinking budget
    if !capabilities.thinking.is_supported() {
        return OutputBudget {
            max_output_tokens: max_output,
            thinking_budget_tokens: None,
        };
    }

    // 按 TTG 档位确定 thinking budget 与响应预留(类型化查表, 编译期保证配对完整)
    let tier = match pref {
        ThinkingPreference::Fast => &BUDGET_TIERS[0],
        ThinkingPreference::Standard => &BUDGET_TIERS[1],
        ThinkingPreference::Deep => &BUDGET_TIERS[2],
    };
    let (base_thinking, response_reserve) = (tier.thinking_budget, tier.response_reserve);

    // Deep 档受 budget_hint_micro 成本护栏约束(K3 thinking 按输出价计费)
    let thinking_cap = if pref == ThinkingPreference::Deep {
        deep_budget_with_cost_hint(base_thinking, budget_hint_micro)
    } else {
        base_thinking
    };

    // WHY 模型容量钳制: thinking + response 必须装进 max_output
    // Kimi K3: max_output=65536, Deep 有效 thinking = min(131072, 65536-16384) = 49152
    // GLM-5.2: max_output=131072, Deep 有效 thinking = min(131072, 131072-16384) = 114688
    // .max(1) 兖底：proptest 极端值(max_output < reserve)时不产生 0 预算
    // .min(max_output.max(1)) 纵深防御: 保证 thinking ≤ max_output 对全 u32 域 total
    // (max_output=0 时 thinking=1 会违反不变量; spec_loader 已在边界拒绝, 此处双保险)
    let thinking_budget = thinking_cap
        .min(max_output.saturating_sub(response_reserve))
        .max(1)
        .min(max_output.max(1));

    // WHY max_output_tokens = max_output: 见函数文档"档位联动"论证——
    // thinking 是 max_output 内部子分配, Anthropic/OpenAI 语义:
    // max_tokens 包含 thinking + response。
    OutputBudget {
        max_output_tokens: max_output,
        thinking_budget_tokens: Some(thinking_budget),
    }
}

/// 将 OutputBudget 应用到已构造的请求体 JSON（跨方言统一后处理）
///
/// WHY 后处理而非修改 Codec 签名：输出预算是跨方言统一关注点，
/// 不属于方言序列化职责。在 adapter 层 build_request 产出后统一覆盖，
/// 避免侵入 3 个 Codec 实现的签名（保持 P2 方言保真职责单一）。
pub fn apply_output_budget(body: &mut serde_json::Value, budget: &OutputBudget) {
    // OpenAI/Responses 方言：max_tokens 或 max_completion_tokens
    if body.get("max_completion_tokens").is_some() {
        body["max_completion_tokens"] = serde_json::json!(budget.max_output_tokens);
    } else if body.get("max_tokens").is_some() {
        body["max_tokens"] = serde_json::json!(budget.max_output_tokens);
    }

    // Anthropic 方言：thinking.budget_tokens
    if let Some(thinking_budget) = budget.thinking_budget_tokens {
        if body.get("thinking").is_some() {
            body["thinking"]["budget_tokens"] = serde_json::json!(thinking_budget);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::affinity::{
        AffinityMessage, AffinityOverrides, ContentBlock, MessageRole, OutputFormat,
        SamplingParams, ToolDecl,
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
            sampling: SamplingParams::default(),
            output_format: OutputFormat::default(),
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

    // ADR-069: negotiate_budget 幂等性(任意 CapabilitySet × ThinkingPreference → 输出确定)
    // 随机域 + 档位/模型边界插值: 覆盖新档位完整展开边界(8192/147456)
    // 与三主力模型实测上限(K3=65536/GLM-5.2=131072/DSV4=393216)
    // 注: workspace proptest=1.5 不支持 prop_oneof! 带权重语法, 边界等权插值
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]
        #[test]
        fn negotiate_budget_is_idempotent(
            max_output in prop_oneof![
                1024u32..200_000,           // 随机域(幂等随机覆盖)
                Just(0u32),                 // 不变量边界(max_output=0 纵深防御)
                Just(8_192u32),             // Fast 完整展开边界
                Just(16_384u32),
                Just(65_536u32),            // Kimi K3
                Just(131_072u32),           // GLM-5.2
                Just(147_456u32),           // Deep 完整展开边界(131072+16384)
                Just(393_216u32),           // DeepSeek V4
            ],
            pref_kind in 0u8..3,
            has_thinking in any::<bool>(),
            budget_hint in proptest::option::of(prop_oneof![
                1u64..5_000_000,            // 常规域(覆盖 hint>档位 路径)
                Just(0u64),                 // 地板抬升路径
                Just(u64::MAX),             // 防回绕极值
            ]),
        ) {
            let mut c = CapabilitySet::minimal_text(100_000, max_output);
            c.thinking = if has_thinking {
                ThinkingSupport::OnOff
            } else {
                ThinkingSupport::None
            };
            let pref = match pref_kind {
                0 => ThinkingPreference::Fast,
                1 => ThinkingPreference::Standard,
                _ => ThinkingPreference::Deep,
            };
            let a = negotiate_budget(&c, pref, budget_hint);
            let b = negotiate_budget(&c, pref, budget_hint);
            prop_assert_eq!(a, b, "输出预算协商必须幂等");
            // max_output_tokens 不超过厂商上限
            prop_assert!(a.max_output_tokens <= max_output);
            // thinking_budget 不超过 max_output(思考不能超出模型容量)
            if let Some(tb) = a.thinking_budget_tokens {
                prop_assert!(tb <= max_output.max(1), "thinking {} > max_output {}", tb, max_output);
                // .max(1) 兖底不变量: max_output ≥ 1 时 thinking ≥ 1
                if max_output >= 1 {
                    prop_assert!(tb >= 1, "thinking budget 不得为 0 (max_output ≥ 1)");
                }
                if pref == ThinkingPreference::Deep {
                    // hint 上限属性: hint 永不放大预算(除地板抬升外)
                    if let Some(hint) = budget_hint {
                        let cap = u64::from(BUDGET_HINT_FLOOR).max(hint / 10);
                        prop_assert!(
                            u64::from(tb) <= cap,
                            "hint 不应放大 thinking: tb={} cap={}",
                            tb, cap
                        );
                    }
                    // 地板属性: 容量允许时 hint 智能降级不硬截断到地板以下
                    if max_output > BUDGET_TIERS[2].response_reserve + BUDGET_HINT_FLOOR {
                        prop_assert!(
                            tb >= BUDGET_HINT_FLOOR,
                            "容量允许时 thinking 不得低于成本地板"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn negotiate_budget_no_thinking_returns_full_max() {
        let mut c = CapabilitySet::minimal_text(100_000, 8192);
        c.thinking = ThinkingSupport::None;
        let budget = negotiate_budget(&c, ThinkingPreference::Deep, None);
        assert_eq!(budget.max_output_tokens, 8192);
        assert_eq!(budget.thinking_budget_tokens, None);
    }

    #[test]
    fn negotiate_budget_fast_gets_8k_thinking() {
        let mut c = CapabilitySet::minimal_text(100_000, 100_000);
        c.thinking = ThinkingSupport::OnOff;
        let budget = negotiate_budget(&c, ThinkingPreference::Fast, None);
        // Fast=8192, reserve=2048, min(8192, 100000-2048=97952) = 8192(整档完整展开)
        assert_eq!(budget.thinking_budget_tokens, Some(8192));
    }

    #[test]
    fn negotiate_budget_standard_gets_32k_thinking() {
        // Kimi K3: max_output=65536, Standard 档 32K 完整可用(不触发容量钳制)
        let mut c = CapabilitySet::minimal_text(100_000, 65_536);
        c.thinking = ThinkingSupport::OnOff;
        let budget = negotiate_budget(&c, ThinkingPreference::Standard, None);
        // Standard=32768, reserve=8192, min(32768, 65536-8192=57344) = 32768
        assert_eq!(budget.thinking_budget_tokens, Some(32768));
    }

    #[test]
    fn negotiate_budget_deep_gets_128k_thinking() {
        let mut c = CapabilitySet::minimal_text(100_000, 200_000);
        c.thinking = ThinkingSupport::OnOff;
        let budget = negotiate_budget(&c, ThinkingPreference::Deep, None);
        // Deep=131072, reserve=16384, min(131072, 200000-16384=183616) = 131072(整档完整展开)
        assert_eq!(budget.thinking_budget_tokens, Some(131072));
    }

    #[test]
    fn negotiate_budget_deep_on_glm_52_fits_with_reserve() {
        // GLM-5.2: max_output=131072, Deep 档 128K 触发响应预留钳制(保 16K 回复空间)
        let mut c = CapabilitySet::minimal_text(100_000, 131_072);
        c.thinking = ThinkingSupport::OnOff;
        let budget = negotiate_budget(&c, ThinkingPreference::Deep, None);
        // Deep=131072, reserve=16384, min(131072, 131072-16384=114688) = 114688
        assert_eq!(budget.thinking_budget_tokens, Some(114688));
        assert_eq!(budget.max_output_tokens, 131_072);
    }

    #[test]
    fn negotiate_budget_deep_constrained_by_hint() {
        let mut c = CapabilitySet::minimal_text(100_000, 128_000);
        c.thinking = ThinkingSupport::OnOff;
        // hint=8000 微元 → affordable = 8000/10 = 800 token
        // max(BUDGET_HINT_FLOOR=2048, 800) = 2048 → min(Deep=131072, 2048) = 2048
        // 容量钳制: min(2048, 128000-16384) = 2048(地板生效路径)
        let budget = negotiate_budget(&c, ThinkingPreference::Deep, Some(8000));
        assert_eq!(budget.thinking_budget_tokens, Some(2048));
    }

    #[test]
    fn negotiate_budget_deep_constrained_by_mid_hint() {
        let mut c = CapabilitySet::minimal_text(100_000, 200_000);
        c.thinking = ThinkingSupport::OnOff;
        // hint=400000 微元 → affordable = 40000 token(介于地板与档位之间)
        // max(2048, 40000) = 40000 → min(Deep=131072, 40000) = 40000(hint 真实生效)
        let budget = negotiate_budget(&c, ThinkingPreference::Deep, Some(400_000));
        assert_eq!(budget.thinking_budget_tokens, Some(40000));
    }

    #[test]
    fn negotiate_budget_deep_hint_above_tier_keeps_tier() {
        let mut c = CapabilitySet::minimal_text(100_000, 200_000);
        c.thinking = ThinkingSupport::OnOff;
        // hint=2000000 微元 → affordable = 200000, 被 base 钳制到 131072
        // max(2048, 131072) = 131072 → min(131072, 131072) = 131072(hint 不超档位)
        let budget = negotiate_budget(&c, ThinkingPreference::Deep, Some(2_000_000));
        assert_eq!(budget.thinking_budget_tokens, Some(131072));
    }

    #[test]
    fn negotiate_budget_hint_overflow_safe() {
        let mut c = CapabilitySet::minimal_text(100_000, 200_000);
        c.thinking = ThinkingSupport::OnOff;
        // hint=u64::MAX → 旧实现 (hint/10) as u32 会静默回绕;
        // 新实现先按 base 钳制再截断: affordable = min(1.8e18, 131072) = 131072
        let budget = negotiate_budget(&c, ThinkingPreference::Deep, Some(u64::MAX));
        assert_eq!(budget.thinking_budget_tokens, Some(131072));
    }

    #[test]
    fn negotiate_budget_deep_on_kimi_k3_clamped_by_reserve() {
        // Kimi K3: max_output=65536, Deep 档 128K 触发响应预留钳制(与 64K 档结果一致, 成本零变化)
        let mut c = CapabilitySet::minimal_text(100_000, 65_536);
        c.thinking = ThinkingSupport::OnOff;
        let budget = negotiate_budget(&c, ThinkingPreference::Deep, None);
        // Deep=131072, reserve=16384, min(131072, 65536-16384=49152) = 49152
        assert_eq!(budget.thinking_budget_tokens, Some(49152));
        // max_output_tokens 仍为厂商上限（thinking 是子分配）
        assert_eq!(budget.max_output_tokens, 65_536);
    }

    #[test]
    fn tier_budgets_are_monotonic() {
        // DeepSeek V4: max_output=393216, 三档均完整展开——验证档位单调性
        // 与 negotiate_thinking 的 Fast→最弱/Deep→最强语义保持一致
        let mut c = CapabilitySet::minimal_text(100_000, 393_216);
        c.thinking = ThinkingSupport::OnOff;
        let fast = negotiate_budget(&c, ThinkingPreference::Fast, None);
        let std = negotiate_budget(&c, ThinkingPreference::Standard, None);
        let deep = negotiate_budget(&c, ThinkingPreference::Deep, None);
        assert!(fast.thinking_budget_tokens.unwrap() < std.thinking_budget_tokens.unwrap());
        assert!(std.thinking_budget_tokens.unwrap() < deep.thinking_budget_tokens.unwrap());
    }

    #[test]
    fn apply_output_budget_overwrites_max_tokens() {
        let mut body = serde_json::json!({"model": "test", "max_tokens": 4096});
        let budget = OutputBudget {
            max_output_tokens: 2048,
            thinking_budget_tokens: None,
        };
        apply_output_budget(&mut body, &budget);
        assert_eq!(body["max_tokens"], 2048);
    }

    #[test]
    fn apply_output_budget_sets_thinking_budget() {
        let mut body = serde_json::json!({
            "model": "test",
            "max_tokens": 8192,
            "thinking": {"type": "enabled", "budget_tokens": 4096}
        });
        let budget = OutputBudget {
            max_output_tokens: 8192,
            thinking_budget_tokens: Some(2048),
        };
        apply_output_budget(&mut body, &budget);
        assert_eq!(body["thinking"]["budget_tokens"], 2048);
    }

    // ============================================================
    // 钉住测试: 边界行为回归守护(Wave 1 Task 1.1)
    // ============================================================

    #[test]
    fn empty_effort_levels_falls_back_to_on() {
        // 空 EffortLevels 回落 On(L71-74 有意设计,不 panic):
        // 厂商声明 EffortLevels 但列表为空(配置错误)时,保守开启思考
        let empty = ThinkingSupport::EffortLevels(vec![]);
        for pref in [
            ThinkingPreference::Fast,
            ThinkingPreference::Standard,
            ThinkingPreference::Deep,
        ] {
            let (dir, degraded) = negotiate_thinking(&empty, pref);
            assert_eq!(
                dir,
                ThinkingDirective::On,
                "空档位域应回落 On, pref={pref:?}"
            );
            assert!(!degraded, "空档位域回落不视为降级, pref={pref:?}");
        }
    }

    #[test]
    fn apply_output_budget_noop_when_no_max_field() {
        // OpenAI/Responses 方言: build_request 不产出 max_tokens/max_completion_tokens
        // 时, apply_output_budget 不应凭空注入字段(设计使然: 数值 budget 在
        // 该方言不可表达, codec 直出 enable_thinking/reasoning_effort)
        let mut body = serde_json::json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "hi"}]
        });
        let original = body.clone();
        let budget = OutputBudget {
            max_output_tokens: 65536,
            thinking_budget_tokens: Some(32768),
        };
        apply_output_budget(&mut body, &budget);
        assert_eq!(body, original, "无 max_tokens 字段时 body 不应被修改");
    }

    #[test]
    fn both_capabilities_missing_reports_streaming_only() {
        // streaming + tool_calling 同时缺失时, 仅报告 streaming(L108-112):
        // streaming 是硬核心(通道不可用), tool_calling 是请求级核心(该请求被拒);
        // 当两者同时缺失, streaming 优先(通道级拒绝 > 请求级拒绝)
        let c = caps(false, false, ThinkingSupport::OnOff);
        let outcome = negotiate(&c, &request(ThinkingPreference::Fast, true));
        assert_eq!(outcome.fidelity, NegotiationFidelity::ChannelRejected);
        assert_eq!(
            outcome.degraded_capabilities,
            vec!["streaming".to_string()],
            "streaming 优先报告(通道级拒绝)"
        );
    }

    #[test]
    fn negotiate_budget_zero_max_output_keeps_invariant() {
        // 纵深防御: max_output=0 时 thinking 不得超出 max_output(不变量 total)
        // spec_loader 已在系统边界拒绝 max_output=0, 此处验证函数级双保险
        let mut c = CapabilitySet::minimal_text(100_000, 0);
        c.thinking = ThinkingSupport::OnOff;
        let budget = negotiate_budget(&c, ThinkingPreference::Deep, None);
        // max_output=0: thinking 应被钳制到 ≤ max(0, 1) = 1
        if let Some(tb) = budget.thinking_budget_tokens {
            assert!(tb <= 1, "max_output=0 时 thinking 应 ≤ 1, got {tb}");
        }
        assert_eq!(budget.max_output_tokens, 0);
    }
}
