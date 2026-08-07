//! Prompt 结构归一化 — 4 层稳定前缀架构（ADR-069 Token 效率优化）
//!
//! 对应架构层: L10 Interface（mca-gateway）
//!
//! # 四层前缀架构
//!
//! 厂商缓存命中是"最长公共前缀"语义：前缀越稳定、越长，缓存命中率越高。
//! 四层按稳定性递减排列，确保稳定内容永远在动态内容之前：
//!
//! | 层 | 名称 | 稳定性 | 变更频率 |
//! |---|------|--------|---------|
//! | L1 | System Identity | 最稳定 | 会话级不变 |
//! | L2 | Tool Declarations | Quest 级稳定 | 只追加不修改 |
//! | L3 | Repo-Wiki Context | Turn 级稳定 | 每轮可能变 |
//! | L4 | Conversation History | 动态 | 每轮追加 |
//!
//! # 稳定前缀工程原则
//! - 无时间戳/随机 ID 污染 L1-L3（否则每轮前缀哈希不同，缓存永不命中）
//! - 工具 schema 冻结只追加（新增工具追加到末尾，不修改/删除/重排已有工具）
//! - 历史 append-only（L4 只追加，不修改已有消息）

use std::sync::Arc;

use nexus_contracts::affinity::{
    AffinityMessage, AffinityRequest, ContentBlock, MessageRole, ModelAffinitySpec,
    ProtocolDialect, TokenCacheKey, ToolDecl,
};
use sha2::{Digest, Sha256};

/// mca-gateway 内置默认系统身份模板 — 稳定前缀 L1
///
/// 会话级恒定:AffinityRequest 无独立 system 字段(M0 无上层注入通道),
/// 本常量是 L1 层的唯一事实源,确保 system_prompt_hash 在会话内稳定。
/// 若未来上层引入系统提示注入,必须保持"同一会话内不变"的契约,
/// 否则厂商缓存命中率归零(前缀哈希每轮漂移)。
pub const DEFAULT_SYSTEM_IDENTITY: &str = "You are Chimera, a terminal-first AI coding agent \
    built on the OMEGA architecture. Follow the user's language for prose and answer precisely.";

/// 归一化后的 4 层 prompt 结构
///
/// 各层用 `Arc<str>` 共享（构造后不可变，多处引用仅 refcount++）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedPrompt {
    /// Layer 1: 系统身份提示（会话级最稳定前缀）
    pub system_identity: Arc<str>,
    /// Layer 2: 工具声明 JSON（Quest 级稳定，确定性序列化）
    pub tool_declarations: Arc<str>,
    /// Layer 3: Repo-Wiki 检索上下文（Turn 级稳定）
    pub repo_context: Arc<str>,
    /// Layer 4: 会话历史（每轮变化，append-only）
    pub conversation: Arc<str>,
}

impl NormalizedPrompt {
    /// 构造归一化 prompt（各层内容直接传入）
    pub fn new(
        system_identity: impl Into<Arc<str>>,
        tool_declarations: impl Into<Arc<str>>,
        repo_context: impl Into<Arc<str>>,
        conversation: impl Into<Arc<str>>,
    ) -> Self {
        Self {
            system_identity: system_identity.into(),
            tool_declarations: tool_declarations.into(),
            repo_context: repo_context.into(),
            conversation: conversation.into(),
        }
    }

    /// 稳定前缀（Layer 1 + Layer 2）— 厂商缓存断点覆盖的最稳定区域
    ///
    /// 显式缓存族（Anthropic 路径）的 cache_control 断点打在此区域之后，
    /// 确保系统提示 + 工具声明被缓存覆盖。
    pub fn stable_prefix(&self) -> String {
        format!("{}{}", self.system_identity, self.tool_declarations)
    }
}

/// 计算 system_prompt_hash — SHA-256(Layer 1 + Layer 2 拼接)
///
/// 用于 `TokenCacheKey.system_prompt_hash` 字段。
/// 系统提示或工具声明任何变更 → 哈希变化 → 缓存自动失效。
pub fn compute_system_prompt_hash(prompt: &NormalizedPrompt) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(prompt.system_identity.as_bytes());
    // WHY 分隔符 0x00: 防止 "ab" + "cd" 与 "abc" + "d" 产生相同哈希
    hasher.update([0x00]);
    hasher.update(prompt.tool_declarations.as_bytes());
    hasher.finalize().into()
}

/// 计算 tool_schema_hash — SHA-256(工具 JSON 确定性序列化)
///
/// 用于 `TokenCacheKey.tool_schema_hash` 字段。
/// 工具集变更（新增/删除/修改）→ 哈希变化 → 不命中旧响应。
///
/// # 确定性保证
/// 调用方负责确保 `tools_json` 是确定性序列化产物（字段排序固定、
/// 无随机空白）。mca-gateway codec 层的 `build_request` 产出满足此约束。
pub fn compute_tool_schema_hash(tools_json: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(tools_json.as_bytes());
    hasher.finalize().into()
}

/// 工具声明的确定性序列化 — 按 name 排序(稳定排序)后 serde_json 序列化
///
/// WHY 排序: 调用方声明的工具顺序可能因路由/拼装流程变化,同一工具集
/// 必须产出同一哈希 —— 厂商缓存命中的前提是稳定前缀字节不变。
/// serde derive 序列化按字段声明序(name/description/parameters_schema),
/// 无随机空白,天然确定性。
pub(crate) fn deterministic_tools_json(tools: &[ToolDecl]) -> String {
    let mut sorted: Vec<&ToolDecl> = tools.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    // ToolDecl 序列化不会失败(无自定义序列化器),空数组兜底保持确定性
    serde_json::to_string(&sorted).unwrap_or_else(|_| "[]".into())
}

/// 隐式族稳定前缀重排(ADR-072 决策 ④)— System 消息重定位到消息末尾
///
/// 隐式缓存族(DeepSeek/Qwen/豆包)的自动前缀缓存是"最长公共前缀"语义:
/// System 消息(L3 repo-wiki 动态内容)每轮变化时,若位于消息首位则
/// 整条前缀失效,缓存命中率归零。重排为 [稳定历史..., System 动态内容]
/// 后,跨轮次公共前缀 = 除最新轮次外的全部历史,命中率实质性提升。
///
/// # 约束(fail-safe)
/// - 仅 `CacheSupport::Implicit` 族生效(显式族靠 cache_control 断点,
///   None 族无缓存语义)
/// - 仅 OpenAI Chat 方言生效(消息数组承载 system 角色;Anthropic 路径
///   system 是顶层参数,天然在断点外,无需重排)
/// - 无 System 消息或消息数 < 2 时返回 false(无重排发生)
///
/// 返回是否实际重排(调用方据此留痕)。
pub fn layout_messages(spec: &ModelAffinitySpec, request: &mut AffinityRequest) -> bool {
    // 仅隐式缓存族
    if spec.capabilities.prompt_caching != nexus_contracts::affinity::CacheSupport::Implicit {
        return false;
    }
    // 仅 OpenAI Chat 方言(消息数组承载 system 角色)
    if spec.preferred_dialect() != Some(ProtocolDialect::OpenAiChat) {
        return false;
    }
    let has_system = request
        .messages
        .iter()
        .any(|m| m.role == MessageRole::System);
    if !has_system || request.messages.len() < 2 {
        return false;
    }
    // 稳定部分(非 System)按原序保留,System 消息移入队尾
    let mut stable: Vec<AffinityMessage> = request
        .messages
        .iter()
        .filter(|m| m.role != MessageRole::System)
        .cloned()
        .collect();
    stable.extend(
        request
            .messages
            .iter()
            .filter(|m| m.role == MessageRole::System)
            .cloned(),
    );
    request.messages = stable;
    true
}

/// 构造 Token 效率缓存键(ADR-069)— 语义缓存与厂商缓存亲和的统一键空间
///
/// 五维覆盖:{model, model_version, tool_schema_hash, system_prompt_hash, thinking_tier}。
/// - `tool_schema_hash`: L2 工具声明按 name 排序后确定性序列化的 SHA-256
/// - `system_prompt_hash`: L1(内置常量) + L2 拼接的 SHA-256(`compute_system_prompt_hash`)
/// - repo_context(L3)/conversation(L4) 从请求消息提取并传入 NormalizedPrompt
///   (语义完整),但**不参与** system_prompt_hash(L1+L2 哈希,与 ADR-069
///   稳定前缀定义一致)——动态历史变化不导致前缀哈希漂移。
///
/// 会话内稳定是缓存命中的前提:同一 spec + 同一工具集 + 同一思考档位
/// 必须产出同一键(见 token_cache_key_* 测试)。
pub fn build_token_cache_key(spec: &ModelAffinitySpec, request: &AffinityRequest) -> TokenCacheKey {
    let tools_json = deterministic_tools_json(&request.tools);
    let prompt = NormalizedPrompt::new(
        DEFAULT_SYSTEM_IDENTITY,
        tools_json.as_str(),
        system_role_text(&request.messages),
        conversation_json(&request.messages),
    );
    TokenCacheKey {
        model: spec.model.clone(),
        model_version: spec.model_version.clone(),
        tool_schema_hash: compute_tool_schema_hash(&tools_json),
        system_prompt_hash: compute_system_prompt_hash(&prompt),
        thinking_tier: request.thinking_pref,
        // 采样桶(ADR-070):temperature/top_p 离散化,桶间互不命中——
        // 防"确定性任务命中随机采样响应"或反之(多样性/正确性双保)
        sampling_bucket: nexus_contracts::affinity::sampling_bucket(&request.sampling),
    }
}

/// 提取 System 角色消息的文本块作为 repo 上下文(L3,无 System 消息 = 空)
fn system_role_text(messages: &[AffinityMessage]) -> String {
    let mut buf = String::new();
    for m in messages {
        if m.role == MessageRole::System {
            for b in &m.blocks {
                if let ContentBlock::Text { text } = b {
                    buf.push_str(text);
                }
            }
        }
    }
    buf
}

/// 非 System 消息的确定性序列化(L4 会话历史,参与键的语义载体)
fn conversation_json(messages: &[AffinityMessage]) -> String {
    let history: Vec<&AffinityMessage> = messages
        .iter()
        .filter(|m| m.role != MessageRole::System)
        .collect();
    serde_json::to_string(&history).unwrap_or_else(|_| "[]".into())
}

/// 前缀不稳定性错误 — 稳定前缀中检测到动态内容
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PrefixInstability {
    /// 检测到时间戳模式（如 "2026-08-02T12:00:00Z"）
    #[error("layer '{layer}' contains timestamp pattern: {pattern}")]
    TimestampDetected {
        /// 所在层名
        layer: String,
        /// 匹配的模式
        pattern: String,
    },
    /// 检测到 UUID/随机 ID 模式
    #[error("layer '{layer}' contains random ID pattern: {pattern}")]
    RandomIdDetected {
        /// 所在层名
        layer: String,
        /// 匹配的模式
        pattern: String,
    },
}

/// 验证前缀稳定性 — 检测稳定层中的动态内容污染
///
/// 仅对 Layer 1-3 调用（Layer 4 本就动态，无需校验）。
/// 检测规则：
/// - ISO 8601 时间戳模式（`YYYY-MM-DDTHH:MM:SS`）
/// - UUID v4 模式（`xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx`）
///
/// WHY 启发式而非禁止列表：完全精确检测不可行（动态内容形态无穷），
/// 启发式覆盖最常见污染源（时间戳 + UUID），误报率低。
pub fn validate_prefix_stability(
    layer_content: &str,
    layer_name: &str,
) -> Result<(), PrefixInstability> {
    // 时间戳检测：ISO 8601 格式 YYYY-MM-DDTHH:MM
    // 简单启发式：连续 16+ 字符匹配 \d{4}-\d{2}-\d{2}T\d{2}:\d{2}
    if contains_timestamp_pattern(layer_content) {
        return Err(PrefixInstability::TimestampDetected {
            layer: layer_name.to_string(),
            pattern: "ISO-8601 timestamp".to_string(),
        });
    }
    // UUID 检测：8-4-4-4-12 hex 格式
    if contains_uuid_pattern(layer_content) {
        return Err(PrefixInstability::RandomIdDetected {
            layer: layer_name.to_string(),
            pattern: "UUID v4".to_string(),
        });
    }
    Ok(())
}

/// 时间戳模式检测（启发式：查找 YYYY-MM-DDTHH:MM 子串）
fn contains_timestamp_pattern(s: &str) -> bool {
    let bytes = s.as_bytes();
    // 最小长度: "2026-08-02T12:00" = 16 字符
    if bytes.len() < 16 {
        return false;
    }
    for window in bytes.windows(16) {
        // 模式: dddd-dd-ddTdd:dd
        if window[4] == b'-'
            && window[7] == b'-'
            && window[10] == b'T'
            && window[13] == b':'
            && window[..4].iter().all(|b| b.is_ascii_digit())
            && window[5..7].iter().all(|b| b.is_ascii_digit())
            && window[8..10].iter().all(|b| b.is_ascii_digit())
            && window[11..13].iter().all(|b| b.is_ascii_digit())
            && window[14..16].iter().all(|b| b.is_ascii_digit())
        {
            return true;
        }
    }
    false
}

/// UUID 模式检测（启发式：查找 8-4-4-4-12 hex 格式）
fn contains_uuid_pattern(s: &str) -> bool {
    let bytes = s.as_bytes();
    // UUID 长度: 36 字符 (8+1+4+1+4+1+4+1+12)
    if bytes.len() < 36 {
        return false;
    }
    for window in bytes.windows(36) {
        if window[8] == b'-'
            && window[13] == b'-'
            && window[18] == b'-'
            && window[23] == b'-'
            && is_hex_segment(&window[..8])
            && is_hex_segment(&window[9..13])
            && is_hex_segment(&window[14..18])
            && is_hex_segment(&window[19..23])
            && is_hex_segment(&window[24..36])
        {
            return true;
        }
    }
    false
}

/// 检查字节切片是否全为 hex 字符
fn is_hex_segment(bytes: &[u8]) -> bool {
    bytes.iter().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::affinity::{
        AffinityMessage, AffinityOverrides, AffinityRequest, ContentBlock, MessageRole,
        ModelAffinitySpec, OutputFormat, ProtocolDialect, ProviderId, SamplingParams,
        ThinkingPreference, ToolDecl,
    };

    fn sample_prompt() -> NormalizedPrompt {
        NormalizedPrompt::new(
            "You are Chimera, an AI coding assistant.",
            r#"[{"name":"read_file","description":"Read a file"}]"#,
            "## Project: chimera-cli\n35 crates workspace",
            "User: Hello\nAssistant: Hi!",
        )
    }

    #[test]
    fn system_prompt_hash_is_deterministic() {
        let prompt = sample_prompt();
        let h1 = compute_system_prompt_hash(&prompt);
        let h2 = compute_system_prompt_hash(&prompt);
        assert_eq!(h1, h2, "相同输入必须产出相同哈希");
    }

    #[test]
    fn system_prompt_hash_changes_on_system_identity_change() {
        let p1 = sample_prompt();
        let p2 = NormalizedPrompt::new(
            "You are a different assistant.",
            p1.tool_declarations.as_ref(),
            p1.repo_context.as_ref(),
            p1.conversation.as_ref(),
        );
        assert_ne!(
            compute_system_prompt_hash(&p1),
            compute_system_prompt_hash(&p2),
        );
    }

    #[test]
    fn system_prompt_hash_changes_on_tool_change() {
        let p1 = sample_prompt();
        let p2 = NormalizedPrompt::new(
            p1.system_identity.as_ref(),
            r#"[{"name":"write_file","description":"Write a file"}]"#,
            p1.repo_context.as_ref(),
            p1.conversation.as_ref(),
        );
        assert_ne!(
            compute_system_prompt_hash(&p1),
            compute_system_prompt_hash(&p2),
        );
    }

    #[test]
    fn tool_schema_hash_deterministic() {
        let json = r#"[{"name":"read_file"}]"#;
        assert_eq!(
            compute_tool_schema_hash(json),
            compute_tool_schema_hash(json)
        );
    }

    #[test]
    fn tool_schema_hash_different_for_different_tools() {
        let j1 = r#"[{"name":"read_file"}]"#;
        let j2 = r#"[{"name":"write_file"}]"#;
        assert_ne!(compute_tool_schema_hash(j1), compute_tool_schema_hash(j2));
    }

    #[test]
    fn stable_prefix_concatenates_l1_l2() {
        let prompt = sample_prompt();
        let prefix = prompt.stable_prefix();
        assert!(prefix.starts_with("You are Chimera"));
        assert!(prefix.contains("read_file"));
    }

    #[test]
    fn validate_stability_passes_clean_content() {
        assert!(validate_prefix_stability("You are an assistant.", "L1").is_ok());
        assert!(validate_prefix_stability("tool declarations here", "L2").is_ok());
    }

    #[test]
    fn validate_stability_detects_timestamp() {
        let content = "Generated at 2026-08-02T12:30 for user";
        let result = validate_prefix_stability(content, "L1");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            PrefixInstability::TimestampDetected { .. }
        ));
    }

    #[test]
    fn validate_stability_detects_uuid() {
        let content = "Session: 550e8400-e29b-41d4-a716-446655440000 active";
        let result = validate_prefix_stability(content, "L1");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            PrefixInstability::RandomIdDetected { .. }
        ));
    }

    #[test]
    fn hash_separator_prevents_collision() {
        // "ab" + "cd" vs "abc" + "d" 不应产生相同哈希
        let p1 = NormalizedPrompt::new("ab", "cd", "", "");
        let p2 = NormalizedPrompt::new("abc", "d", "", "");
        assert_ne!(
            compute_system_prompt_hash(&p1),
            compute_system_prompt_hash(&p2),
        );
    }

    // ============================================================
    // build_token_cache_key(ADR-069 Token 效率缓存键构造)
    // ============================================================

    fn cache_key_spec() -> ModelAffinitySpec {
        ModelAffinitySpec::minimal(
            ProviderId::DeepSeek,
            "deepseek-v4-flash",
            ProtocolDialect::OpenAiChat,
        )
    }

    fn cache_key_tool(name: &str) -> ToolDecl {
        ToolDecl {
            name: name.into(),
            description: "test tool".into(),
            parameters_schema: "{}".into(),
        }
    }

    fn cache_key_request(tools: Vec<ToolDecl>, thinking: ThinkingPreference) -> AffinityRequest {
        AffinityRequest {
            intent_id: "intent-key".into(),
            messages: vec![AffinityMessage {
                role: MessageRole::User,
                blocks: vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
            }],
            tools,
            thinking_pref: thinking,
            budget_hint_micro: None,
            overrides: AffinityOverrides::default(),
            sampling: SamplingParams::default(),
            output_format: OutputFormat::default(),
        }
    }

    #[test]
    fn token_cache_key_stable_across_identical_requests() {
        // 同一 spec + 同一 tools + 同一 thinking_pref → 两次构造哈希恒定
        let spec = cache_key_spec();
        let req = cache_key_request(
            vec![cache_key_tool("read_file"), cache_key_tool("write_file")],
            ThinkingPreference::Standard,
        );
        let k1 = build_token_cache_key(&spec, &req);
        let k2 = build_token_cache_key(&spec, &req);
        assert_eq!(k1, k2, "相同 spec/请求必须产出相同缓存键");
        assert_eq!(k1.model.as_ref(), "deepseek-v4-flash");
        assert_eq!(k1.thinking_tier, ThinkingPreference::Standard);
    }

    #[test]
    fn token_cache_key_tool_order_insensitive() {
        // 工具声明顺序变化不应改变 tool_schema_hash(按 name 排序后确定性序列化)
        let spec = cache_key_spec();
        let req_a = cache_key_request(
            vec![cache_key_tool("read_file"), cache_key_tool("write_file")],
            ThinkingPreference::Standard,
        );
        let req_b = cache_key_request(
            vec![cache_key_tool("write_file"), cache_key_tool("read_file")],
            ThinkingPreference::Standard,
        );
        assert_eq!(
            build_token_cache_key(&spec, &req_a).tool_schema_hash,
            build_token_cache_key(&spec, &req_b).tool_schema_hash,
            "工具顺序变化必须保持 tool_schema_hash 稳定"
        );
    }

    #[test]
    fn token_cache_key_changes_on_tool_change() {
        // 工具集变更(读→写)→ tool_schema_hash 变化 → 缓存不命中旧响应
        let spec = cache_key_spec();
        let req1 = cache_key_request(
            vec![cache_key_tool("read_file")],
            ThinkingPreference::Standard,
        );
        let req2 = cache_key_request(
            vec![cache_key_tool("write_file")],
            ThinkingPreference::Standard,
        );
        assert_ne!(
            build_token_cache_key(&spec, &req1).tool_schema_hash,
            build_token_cache_key(&spec, &req2).tool_schema_hash,
            "工具变更必须改变 tool_schema_hash"
        );
    }

    #[test]
    fn token_cache_key_changes_on_thinking_tier() {
        // 思考档位切换 → 键不同 → Fast/Deep 响应不混淆
        let spec = cache_key_spec();
        let fast = cache_key_request(Vec::new(), ThinkingPreference::Fast);
        let deep = cache_key_request(Vec::new(), ThinkingPreference::Deep);
        assert_ne!(
            build_token_cache_key(&spec, &fast),
            build_token_cache_key(&spec, &deep),
            "思考档位切换必须产生不同缓存键"
        );
    }

    #[test]
    fn token_cache_key_system_prompt_hash_stable_in_session() {
        // 会话历史(L4)追加不影响 system_prompt_hash(L1+L2 稳定前缀哈希)
        let spec = cache_key_spec();
        let req1 = cache_key_request(vec![cache_key_tool("read_file")], ThinkingPreference::Fast);
        let mut req2 =
            cache_key_request(vec![cache_key_tool("read_file")], ThinkingPreference::Fast);
        req2.messages.push(AffinityMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: "more".into(),
            }],
        });
        assert_eq!(
            build_token_cache_key(&spec, &req1).system_prompt_hash,
            build_token_cache_key(&spec, &req2).system_prompt_hash,
            "会话内稳定前缀哈希必须与动态历史无关"
        );
    }

    // ============================================================
    // 隐式族稳定前缀重排(ADR-072 决策 ④)
    // ============================================================

    fn implicit_spec() -> ModelAffinitySpec {
        let mut spec = ModelAffinitySpec::minimal(
            ProviderId::DeepSeek,
            "deepseek-v4-flash",
            ProtocolDialect::OpenAiChat,
        );
        spec.capabilities.prompt_caching = nexus_contracts::affinity::CacheSupport::Implicit;
        spec
    }

    #[test]
    fn layout_moves_system_to_tail_for_implicit_family() {
        // 隐式族 + OpenAI Chat:System 消息重定位到末尾,稳定历史成为前缀主体
        let spec = implicit_spec();
        let mut req = cache_key_request(Vec::new(), ThinkingPreference::Fast);
        req.messages.insert(
            0,
            AffinityMessage {
                role: MessageRole::System,
                blocks: vec![ContentBlock::Text {
                    text: "repo-wiki 检索结果(每轮变化)".into(),
                }],
            },
        );
        assert!(layout_messages(&spec, &mut req), "必须实际重排");
        assert_eq!(
            req.messages.first().map(|m| m.role),
            Some(MessageRole::User),
            "重排后首位必须是稳定历史而非动态 System"
        );
        assert_eq!(
            req.messages.last().map(|m| m.role),
            Some(MessageRole::System),
            "重排后 System 消息必须在末尾"
        );
    }

    #[test]
    fn layout_noop_without_system_message() {
        // 无 System 消息 → 零操作(无动态前缀可重排)
        let spec = implicit_spec();
        let mut req = cache_key_request(Vec::new(), ThinkingPreference::Fast);
        assert!(!layout_messages(&spec, &mut req));
    }

    #[test]
    fn layout_noop_for_explicit_family() {
        // 显式族靠 cache_control 断点,不重排(断点在 L2 后,重排反而破坏断点位置)
        let mut spec = implicit_spec();
        spec.capabilities.prompt_caching = nexus_contracts::affinity::CacheSupport::ExplicitControl;
        let mut req = cache_key_request(Vec::new(), ThinkingPreference::Fast);
        req.messages.insert(
            0,
            AffinityMessage {
                role: MessageRole::System,
                blocks: vec![ContentBlock::Text { text: "sys".into() }],
            },
        );
        assert!(!layout_messages(&spec, &mut req));
    }

    #[test]
    fn layout_noop_for_anthropic_dialect() {
        // Anthropic 路径 system 是顶层参数(不在 messages 数组),无需重排
        let mut spec = implicit_spec();
        spec.dialects = vec![ProtocolDialect::AnthropicMessages];
        let mut req = cache_key_request(Vec::new(), ThinkingPreference::Fast);
        req.messages.insert(
            0,
            AffinityMessage {
                role: MessageRole::System,
                blocks: vec![ContentBlock::Text { text: "sys".into() }],
            },
        );
        assert!(!layout_messages(&spec, &mut req));
    }

    #[test]
    fn layout_preserves_stable_history_prefix() {
        // 核心性质:重排后跨轮次公共前缀 = 稳定历史(System 每轮变化不再破坏前缀)
        let spec = implicit_spec();
        // 第 N 轮:system 内容 A + 稳定历史
        let mut turn_n = cache_key_request(Vec::new(), ThinkingPreference::Fast);
        turn_n.messages.insert(
            0,
            AffinityMessage {
                role: MessageRole::System,
                blocks: vec![ContentBlock::Text {
                    text: "检索结果 A".into(),
                }],
            },
        );
        // 第 N+1 轮:system 内容 B(变化)+ 稳定历史 + 新轮次
        let mut turn_n1 = turn_n.clone();
        turn_n1.messages[0] = AffinityMessage {
            role: MessageRole::System,
            blocks: vec![ContentBlock::Text {
                text: "检索结果 B".into(),
            }],
        };
        turn_n1.messages.push(AffinityMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: "follow-up".into(),
            }],
        });

        // 重排后:两轮共享的公共前缀 = 稳定历史(首条消息与重排前 System 无关)
        layout_messages(&spec, &mut turn_n);
        layout_messages(&spec, &mut turn_n1);
        assert_eq!(
            turn_n.messages.first().map(|m| m.role),
            turn_n1.messages.first().map(|m| m.role),
            "重排后两轮首条消息角色一致(稳定历史开头)"
        );
        assert_eq!(
            turn_n.messages[..turn_n.messages.len() - 1],
            turn_n1.messages[..turn_n.messages.len() - 1],
            "重排后第 N 轮序列必须是第 N+1 轮的前缀(隐式缓存命中前提)"
        );
    }
}
