//! conversation 预算裁剪 — R4 上下文预算与动态裁剪(ADR-069 Token 效率优化)
//!
//! 对应架构层: L10 Interface(mca-gateway)
//!
//! # 与 hcw-window::trim_to_budget 的关系
//! 同源语义:预算驱动 + 重要性排序。hcw 版操作 `ContextEntry`(file_id/
//! access_count/时间戳),其语义与 `AffinityMessage`(角色/内容块)不匹配,
//! 故在 mca-gateway 内实现针对性 conversation 裁剪纯函数,避免
//! `ContextEntry` 类型泄漏到 L10。
//!
//! # 重要性评分
//! - 工具返回(Tool 消息):最高权重,模型下一步决策的依据,恒保留
//! - 用户输入(User):次高,承载最新意图
//! - 历史(Assistant/System):最低,裁剪优先
//! - 时近性:最后一条消息恒保留(保尾段);同权重内新消息优先保留
//!
//! # Top-K 实现(红线)
//! 预算约束的 Top-K 用 `select_nth_unstable_by`(O(n) 部分排序),
//! 禁止 `sort_by` 全排序。
//!
//! # token 估算口径
//! 字符/4 启发式,与 `adapters::estimate_cost` 同口径;但本模块**逐条
//! floor 后求和**,使裁剪累加与预算断言口径一致(sum(floor) ≤ floor(sum),
//! 若只对总量 floor,裁剪后总量可能反超预算,见 trim 测试)。

use std::sync::OnceLock;

use event_bus::EventBus;
use nexus_contracts::affinity::{AffinityMessage, AffinityRequest, MessageRole, ModelAffinitySpec};
use osa_coordinator::{ComplexityBand, OmniSparseCoordinator};

/// 预算利用率 — 上下文窗口 × 复杂度利用率 × 本比率(默认 0.6)
///
/// WHY 0.6: 预留输出 token 与峰值波动余量,避免输入占满窗口挤掉输出。
const DEFAULT_BUDGET_RATIO: f32 = 0.6;

/// 小窗口模型预算比率 — context_window ≤ 32K 时用 0.7
///
/// WHY 0.7: 小窗口模型(16K/32K)输出空间本就紧张,比大窗口多预留 10%
/// 窗口空间给输出,避免输入裁剪过激导致模型"挤牙膏"式输出。
const SMALL_WINDOW_BUDGET_RATIO: f32 = 0.7;

/// 小窗口判定阈值 — context_window ≤ 此值视为小窗口模型
const SMALL_WINDOW_THRESHOLD: u32 = 32768;

/// 压缩阈值 — 单条历史消息 token 估算超过此值才触发 sidecar 压缩(≈4K)
pub const COMPRESS_THRESHOLD_TOKENS: u32 = 4096;

/// 压缩目标比率 — LLMLingua-2 target_ratio(0.5 = 压缩一半)
pub const COMPRESS_TARGET_RATIO: f32 = 0.5;

/// 复杂度粗判的消息数阈值 — 有工具且消息 ≥ 此值判为 Complex 档
const COMPLEX_MESSAGE_THRESHOLD: usize = 30;

/// 预算计算器单例 — `compute_token_budget` 不依赖 OSA 运行时状态
/// (纯函数),进程内懒构造一次即可;独立 EventBus 仅作构造参数,
/// 不参与任何事件发布(预算面是纯计算)。
static BUDGET_COORDINATOR: OnceLock<OmniSparseCoordinator> = OnceLock::new();

/// 估算会话总 token — 委托 token_estimate 显式化估算(ADR-070)
///
/// 字节宽分类权重(1B:0.25/2B:0.5/3B:0.75/4B:1.0)与既有字节/4 等价,
/// 逐条 floor 后求和——裁剪累加与预算断言口径一致(sum(floor) ≤ floor(sum))。
pub fn estimate_tokens(messages: &[AffinityMessage]) -> u32 {
    crate::token_estimate::estimate_messages(messages)
}

/// 估算单条消息 token — 内容块文本字节宽加权(与 estimate_cost 同口径)
pub fn estimate_message_tokens(message: &AffinityMessage) -> u32 {
    crate::token_estimate::estimate_message(message)
}

/// 会话 token 预算 — osa-coordinator `compute_token_budget` 接线
///
/// complexity 粗判:无工具 = Simple(纯对话);有工具按消息量分档
/// (≥ COMPLEX_MESSAGE_THRESHOLD 判 Complex,否则 Regular)。
/// base_context_window = spec.capabilities.context_window;budget_ratio 按窗口大小
/// 分档:≤32K 用 SMALL_WINDOW_BUDGET_RATIO(0.7),>32K 用 DEFAULT_BUDGET_RATIO(0.6)。
pub fn conversation_budget(spec: &ModelAffinitySpec, request: &AffinityRequest) -> u32 {
    let complexity = if request.tools.is_empty() {
        ComplexityBand::Simple
    } else if request.messages.len() >= COMPLEX_MESSAGE_THRESHOLD {
        ComplexityBand::Complex
    } else {
        ComplexityBand::Regular
    };
    let budget_ratio = if spec.capabilities.context_window <= SMALL_WINDOW_THRESHOLD {
        SMALL_WINDOW_BUDGET_RATIO
    } else {
        DEFAULT_BUDGET_RATIO
    };
    let coordinator =
        BUDGET_COORDINATOR.get_or_init(|| OmniSparseCoordinator::new(EventBus::new()));
    coordinator.compute_token_budget(complexity, spec.capabilities.context_window, budget_ratio)
}

/// 预算驱动的 conversation 裁剪 — 超预算才裁剪(纯函数,不修改入参)
///
/// 规则:
/// 1. 空会话或 token ≤ 预算 → 原样返回
/// 2. 恒保留:工具返回消息 + 最后一条消息(时近性保尾段)
/// 3. 候选按 (角色权重, 序号) 评分,权重主键(工具 3 > 用户 2 > 历史 1)
/// 4. 超预算时按权重桶贪心保留,桶内用 `select_nth_unstable_by` 选
///    最近的 K 条(Top-K O(n) 红线);低权重桶全部丢弃
/// 5. 保留集按原始顺序重组
pub fn trim_to_budget(messages: Vec<AffinityMessage>, budget_tokens: u32) -> Vec<AffinityMessage> {
    // 空会话或预算充足 → 原样返回(不裁剪)
    if messages.is_empty() || estimate_tokens(&messages) <= budget_tokens {
        return messages;
    }

    // 恒保留:工具返回消息 + 最后一条消息(时近性保尾段)
    let last_idx = messages.len() - 1;
    let mut keep: Vec<bool> = messages
        .iter()
        .map(|m| m.role == MessageRole::Tool)
        .collect();
    keep[last_idx] = true;

    // 极端兜底:保留集自身已 ≥ 预算(工具+最新消息超预算)——语义优先,
    // 宁可超预算也不丢工具返回与最新输入(此时继续裁剪会破坏角色语义)。
    let keep_tokens: u32 = messages
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(m, _)| estimate_message_tokens(m))
        .sum();
    if keep_tokens >= budget_tokens {
        return messages
            .into_iter()
            .zip(keep)
            .filter(|(_, k)| *k)
            .map(|(m, _)| m)
            .collect();
    }
    let mut remaining = budget_tokens - keep_tokens;

    // 候选按权重桶贪心保留:高权重桶整桶保留,首个放不下的桶内
    // 用 select_nth_unstable_by 选最近的 K 条,更低权重桶全部丢弃。
    let mut selected: Vec<usize> = Vec::new();
    for weight in (1..=2).rev() {
        // 候选仅 User(2)/Assistant+System(1),Tool 已在 keep 恒保留
        let bucket: Vec<(usize, u32)> = messages
            .iter()
            .enumerate()
            .filter(|(i, m)| !keep[*i] && role_weight(m.role) == weight)
            .map(|(i, m)| (i, estimate_message_tokens(m)))
            .collect();
        if bucket.is_empty() {
            continue;
        }
        let bucket_tokens: u32 = bucket.iter().map(|(_, t)| t).sum();
        if remaining >= bucket_tokens {
            selected.extend(bucket.iter().map(|(i, _)| *i));
            remaining -= bucket_tokens;
        } else {
            // 桶内新消息优先:从最新往前贪心确定 K(最近的 K 条放得下预算)
            let mut acc = 0u32;
            let mut k = 0usize;
            for (_, t) in bucket.iter().rev() {
                if acc + t > remaining {
                    break;
                }
                acc += t;
                k += 1;
            }
            if k > 0 {
                // Top-K 红线:select_nth_unstable_by 部分排序 O(n),禁止 sort_by
                let mut top = bucket;
                top.select_nth_unstable_by(k - 1, |a, c| c.0.cmp(&a.0));
                for (i, _) in top[..k].iter() {
                    selected.push(*i);
                }
            }
            break;
        }
    }

    for i in selected {
        keep[i] = true;
    }
    messages
        .into_iter()
        .enumerate()
        .filter(|(i, _)| keep[*i])
        .map(|(_, m)| m)
        .collect()
}

/// 选最长的可压缩历史消息 — 排除 System/Tool 与最后一条
///
/// 返回消息 index;无候选(全为 System/Tool/最后一条)返回 None。
pub fn longest_compressible_message(messages: &[AffinityMessage]) -> Option<usize> {
    let last = messages.len().saturating_sub(1);
    messages
        .iter()
        .enumerate()
        .filter(|(i, m)| *i != last && m.role != MessageRole::System && m.role != MessageRole::Tool)
        .max_by_key(|(_, m)| estimate_message_tokens(m))
        .map(|(i, _)| i)
}

/// 角色 → 裁剪权重(数值越大越优先保留)
fn role_weight(role: MessageRole) -> u32 {
    match role {
        MessageRole::Tool => 3,
        MessageRole::User => 2,
        // Assistant 与 System 同属历史消息,裁剪优先
        MessageRole::Assistant | MessageRole::System => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::affinity::ContentBlock;
    use nexus_contracts::affinity::{
        AffinityOverrides, OutputFormat, ProtocolDialect, ProviderId, SamplingParams,
        ThinkingPreference, ToolDecl,
    };

    fn msg(role: MessageRole, text: &str) -> AffinityMessage {
        AffinityMessage {
            role,
            blocks: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    fn request(tools: Vec<ToolDecl>, msg_count: usize) -> AffinityRequest {
        AffinityRequest {
            intent_id: "intent-trim".into(),
            messages: (0..msg_count)
                .map(|i| msg(MessageRole::User, &format!("turn {i}")))
                .collect(),
            tools,
            thinking_pref: ThinkingPreference::Fast,
            budget_hint_micro: None,
            overrides: AffinityOverrides::default(),
            sampling: SamplingParams::default(),
            output_format: OutputFormat::default(),
        }
    }

    // ============================================================
    // 3.1 失败测试:裁剪语义
    // ============================================================

    #[test]
    fn trim_reduces_under_budget_keeping_tool_results() {
        // 超预算 conversation:8 条长 User 历史 + 短工具返回 + 最后一条
        let mut messages = Vec::new();
        for i in 0..8 {
            messages.push(msg(
                MessageRole::User,
                &format!("history message number {i} {}", "x".repeat(200)),
            ));
        }
        messages.push(msg(MessageRole::Tool, "tool result ok"));
        messages.push(msg(MessageRole::Assistant, "last response"));
        let budget = 200;
        let trimmed = trim_to_budget(messages.clone(), budget);
        assert!(
            estimate_tokens(&trimmed) <= budget,
            "裁剪后 token 必须 ≤ 预算, got {}",
            estimate_tokens(&trimmed)
        );
        assert!(
            trimmed.iter().any(|m| m.role == MessageRole::Tool),
            "工具返回消息必须恒保留"
        );
        assert_eq!(
            trimmed.last().map(|m| m.blocks.clone()),
            messages.last().map(|m| m.blocks.clone()),
            "最后一条消息必须保留(时近性)"
        );
    }

    #[test]
    fn trim_keeps_original_when_within_budget() {
        let messages = vec![
            msg(MessageRole::User, "hi"),
            msg(MessageRole::Assistant, "hello"),
        ];
        let trimmed = trim_to_budget(messages.clone(), 1000);
        assert_eq!(trimmed, messages, "预算充足时不得裁剪");
    }

    #[test]
    fn trim_empty_conversation_is_noop() {
        let trimmed = trim_to_budget(Vec::new(), 100);
        assert!(trimmed.is_empty(), "空会话不 panic 且返回空");
    }

    #[test]
    fn trim_zero_budget_keeps_recent_message() {
        let messages = vec![
            msg(MessageRole::Assistant, "old assistant turn"),
            msg(MessageRole::User, "new user turn"),
        ];
        let trimmed = trim_to_budget(messages.clone(), 0);
        assert!(!trimmed.is_empty(), "预算零也必须兜底保留最近消息");
        assert!(
            trimmed.iter().any(|m| m.blocks == messages[1].blocks),
            "最新用户输入必须兜底保留"
        );
    }

    #[test]
    fn trim_preserves_original_relative_order() {
        // 工具返回在历史中段;裁剪后保留集按原序重组,工具恒在最后一条之前
        let messages = vec![
            msg(
                MessageRole::User,
                &format!("first user {}", "x".repeat(100)),
            ),
            msg(
                MessageRole::Assistant,
                &format!("assistant turn {}", "x".repeat(100)),
            ),
            msg(MessageRole::Tool, "tool result"),
            msg(
                MessageRole::User,
                &format!("second user {}", "x".repeat(100)),
            ),
            msg(MessageRole::Assistant, "latest assistant"),
        ];
        let trimmed = trim_to_budget(messages.clone(), 50);
        let roles: Vec<MessageRole> = trimmed.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            vec![MessageRole::Tool, MessageRole::User, MessageRole::Assistant,],
            "保留集按原序重组,工具恒在最新消息之前, roles = {roles:?}"
        );
    }

    // ============================================================
    // 3.2 预算接线:conversation_budget 复杂度分档
    // ============================================================

    #[test]
    fn conversation_budget_scales_with_complexity() {
        let mut spec = ModelAffinitySpec::minimal(
            ProviderId::DeepSeek,
            "deepseek-v4-flash",
            ProtocolDialect::OpenAiChat,
        );
        spec.capabilities.context_window = 128_000;
        let tool = ToolDecl {
            name: "search".into(),
            description: "web search".into(),
            parameters_schema: "{}".into(),
        };
        let simple = conversation_budget(&spec, &request(Vec::new(), 3));
        let regular = conversation_budget(&spec, &request(vec![tool.clone()], 3));
        let complex = conversation_budget(&spec, &request(vec![tool], 30));
        // 128K × 利用率 × 0.6:Simple=0.25 / Regular=0.5 / Complex=0.75
        assert!(
            (19_000..25_000).contains(&simple),
            "Simple ≈ 128K×0.25×0.6 = 19200, got {simple}"
        );
        assert!(
            simple < regular && regular < complex,
            "复杂度越高预算越高, {simple} < {regular} < {complex}"
        );
    }

    // ============================================================
    // 3.3 窗口大小分档:小窗口(≤32K)用 0.7,大窗口(>32K)用 0.6
    // ============================================================

    fn spec_with_window(window: u32) -> ModelAffinitySpec {
        let mut spec = ModelAffinitySpec::minimal(
            ProviderId::DeepSeek,
            "test-model",
            ProtocolDialect::OpenAiChat,
        );
        spec.capabilities.context_window = window;
        spec
    }

    #[test]
    fn small_window_models_use_higher_budget_ratio() {
        // 16K 窗口:Simple 档 = 16384 × 0.25 × 0.7 = 2867.2 → 2867
        let spec = spec_with_window(16384);
        let simple = conversation_budget(&spec, &request(Vec::new(), 3));
        // 允许 ±1 浮点误差
        assert!(
            (2866..=2868).contains(&simple),
            "16K Simple 预期 ≈ 16384×0.25×0.7=2867, got {simple}"
        );

        // 32K 窗口:Regular 档 = 32768 × 0.5 × 0.7 = 11468.8 → 11468
        let spec = spec_with_window(32768);
        let tool = ToolDecl {
            name: "search".into(),
            description: "web search".into(),
            parameters_schema: "{}".into(),
        };
        let regular = conversation_budget(&spec, &request(vec![tool], 5));
        assert!(
            (11467..=11470).contains(&regular),
            "32K Regular 预期 ≈ 32768×0.5×0.7=11468, got {regular}"
        );
    }

    #[test]
    fn large_window_models_use_default_budget_ratio() {
        // 64K 窗口:Simple 档 = 65536 × 0.25 × 0.6 = 9830.4 → 9830
        let spec = spec_with_window(65536);
        let simple = conversation_budget(&spec, &request(Vec::new(), 3));
        assert!(
            (9829..=9831).contains(&simple),
            "64K Simple 预期 ≈ 65536×0.25×0.6=9830, got {simple}"
        );

        // 128K 窗口:Simple 档 = 131072 × 0.25 × 0.6 = 19660.8 → 19660
        let spec = spec_with_window(131072);
        let simple = conversation_budget(&spec, &request(Vec::new(), 3));
        assert!(
            (19659..=19662).contains(&simple),
            "128K Simple 预期 ≈ 131072×0.25×0.6=19660, got {simple}"
        );

        // 1M 窗口:Simple 档 = 1048576 × 0.25 × 0.6 = 157286.4 → 157286
        let spec = spec_with_window(1_048_576);
        let simple = conversation_budget(&spec, &request(Vec::new(), 3));
        assert!(
            (157285..=157287).contains(&simple),
            "1M Simple 预期 ≈ 1048576×0.25×0.6=157286, got {simple}"
        );
    }

    // ============================================================
    // token 估算与压缩辅助
    // ============================================================

    #[test]
    fn estimate_tokens_uses_char4_heuristic() {
        assert_eq!(estimate_tokens(&[msg(MessageRole::User, "abcd")]), 1);
        assert_eq!(estimate_tokens(&[msg(MessageRole::User, "abc")]), 0);
        // 逐条 floor 求和:3 条 5 字符 = 3×1 = 3,而非 floor(15/4)=3
        let three = vec![
            msg(MessageRole::User, "12345"),
            msg(MessageRole::User, "abcde"),
            msg(MessageRole::User, "ABCDE"),
        ];
        assert_eq!(estimate_tokens(&three), 3);
    }

    #[test]
    fn longest_compressible_excludes_system_tool_and_latest() {
        let messages = vec![
            msg(MessageRole::System, "sys prompt"),
            msg(MessageRole::User, "short"),
            msg(MessageRole::Tool, "tool result"),
            msg(MessageRole::Assistant, &"a".repeat(400)),
            msg(MessageRole::User, "latest user turn"),
        ];
        assert_eq!(
            longest_compressible_message(&messages),
            Some(3),
            "最长可压缩历史消息为 index 3(Assistant)"
        );
        // 仅 System + 最后一条 → 无候选
        let only_latest = vec![msg(MessageRole::System, "s"), msg(MessageRole::User, "u")];
        assert_eq!(longest_compressible_message(&only_latest), None);
    }

    #[test]
    fn role_weight_ordering() {
        assert!(role_weight(MessageRole::Tool) > role_weight(MessageRole::User));
        assert!(role_weight(MessageRole::User) > role_weight(MessageRole::Assistant));
        assert_eq!(
            role_weight(MessageRole::Assistant),
            role_weight(MessageRole::System)
        );
    }
}
