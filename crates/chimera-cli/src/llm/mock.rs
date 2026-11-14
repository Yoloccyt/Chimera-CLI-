//! `MockLlmClient` —— 脚本化模型，零网络（Phase 7，ADR-232）
//!
//! 对应架构层: L10 Interface（chimera-cli）
//!
//! # 为什么需要它
//! 工具循环的正确性（回填顺序、Thinking 保真、预算/上限/取消、审批拒绝路径）
//! 必须能在无 API key、无网络的环境里逐条断言；真实 provider 只做装配验证
//! （见 `mca_client.rs` 与 §9.2 的 RL/网络闸门纪律）。
//!
//! # 记录而非吞掉
//! 每次 `complete` 都先把请求存进 `seen`，测试因此能断言"第 2 轮请求里是否真的
//! 带了 `Tool` 角色消息与对应 `tool_use_id`" —— 这是闭环是否成立的唯一硬证据。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use nexus_contracts::affinity::{
    AffinityMessage, AffinityOverrides, AffinityRequest, AffinityResponse, ContentBlock,
    CostEstimate, FinishReason, MessageRole, OutputFormat, ProtocolDialect, ProviderId,
    ProviderReceipt, SamplingParams, ThinkingPreference, UsageReport,
};

use super::client::{LlmClient, LlmError};

/// 脚本化客户端 —— 按入队顺序逐条返回响应
#[derive(Debug, Clone)]
pub struct MockLlmClient {
    scripted: Arc<Mutex<VecDeque<AffinityResponse>>>,
    seen: Arc<Mutex<Vec<AffinityRequest>>>,
    provider_id: Arc<str>,
}

impl MockLlmClient {
    /// 以给定脚本构造（顺序即返回顺序）
    #[must_use]
    pub fn new(responses: impl IntoIterator<Item = AffinityResponse>) -> Self {
        Self {
            scripted: Arc::new(Mutex::new(responses.into_iter().collect())),
            seen: Arc::new(Mutex::new(Vec::new())),
            provider_id: "mock".into(),
        }
    }

    /// 追加一条脚本（测试中途扩展用）
    pub fn push_response(&self, response: AffinityResponse) {
        self.lock_scripted().push_back(response);
    }

    /// 已记录的请求快照（供断言）
    #[must_use]
    pub fn seen_requests(&self) -> Vec<AffinityRequest> {
        self.lock_seen().clone()
    }

    /// 已发生的补全轮数
    #[must_use]
    pub fn seen_count(&self) -> usize {
        self.lock_seen().len()
    }

    /// 构造最终文本响应（无 ToolUse ⇒ 循环应收敛为 `Final`）
    #[must_use]
    pub fn final_text(text: &str) -> AffinityResponse {
        Self::response(vec![ContentBlock::Text {
            text: text.to_string().into_boxed_str(),
        }])
    }

    /// 构造"带 token 计量的最终文本响应"（预算类测试用它把 token 打满）
    #[must_use]
    pub fn final_text_with_usage(
        text: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> AffinityResponse {
        let mut response = Self::final_text(text);
        response.usage = UsageReport {
            input_tokens,
            output_tokens,
            cache_hit_tokens: 0,
            thinking_tokens: None,
        };
        response
    }

    /// 构造工具调用响应：`(调用 id, 工具名, args_json)` 三元组列表
    #[must_use]
    pub fn tool_use(calls: &[(&str, &str, &str)]) -> AffinityResponse {
        let blocks = calls
            .iter()
            .map(|(id, name, args)| ContentBlock::ToolUse {
                id: (*id).to_string().into_boxed_str(),
                name: (*name).to_string().into_boxed_str(),
                input_json: (*args).to_string().into_boxed_str(),
            })
            .collect();
        let mut response = Self::response(blocks);
        response.finish_reason = FinishReason::ToolUse;
        response
    }

    /// 由任意内容块构造响应（Thinking 保真类测试直接给块）
    #[must_use]
    pub fn response(blocks: Vec<ContentBlock>) -> AffinityResponse {
        AffinityResponse {
            blocks,
            usage: UsageReport {
                input_tokens: 10,
                output_tokens: 5,
                cache_hit_tokens: 0,
                thinking_tokens: None,
            },
            cost: CostEstimate::default(),
            finish_reason: FinishReason::Stop,
            receipt: ProviderReceipt {
                provider: ProviderId::DeepSeek,
                model: "mock-model".into(),
                dialect: ProtocolDialect::OpenAiChat,
                request_id: None,
            },
        }
    }

    fn lock_scripted(&self) -> std::sync::MutexGuard<'_, VecDeque<AffinityResponse>> {
        // WHY unwrap_or_else(into_inner):毒化只可能来自另一个测试线程 panic,
        // mock 的临界区里没有可 panic 的用户代码 —— 取回内部数据比整轮失败更有用
        self.scripted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_seen(&self) -> std::sync::MutexGuard<'_, Vec<AffinityRequest>> {
        self.seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    fn provider_id(&self) -> &str {
        &self.provider_id
    }

    async fn complete(&self, request: &AffinityRequest) -> Result<AffinityResponse, LlmError> {
        self.lock_seen().push(request.clone());
        self.lock_scripted()
            .pop_front()
            .ok_or_else(|| LlmError::Protocol("mock 脚本已耗尽,模型仍被请求补全".to_string()))
    }
}

/// 离线回声客户端 —— CLI/TUI 的无网络 e2e 档（`CHIMERA_LLM_PROVIDER=mock`）
///
/// 与 [`MockLlmClient`] 的差别：后者按脚本逐轮回放（测试用），本类型**无限次**返回
/// "回声文本"，因此适合端到端跑通装配/审批/回填链路而不预置脚本。
///
/// WHY 存在于生产码：CI 与离线演示必须能走 `--llm` 全链路而不配 API key；
/// 它永不调用工具（无 ToolUse 块）⇒ 循环单轮收敛，行为可预测。
#[derive(Debug, Clone, Copy, Default)]
pub struct EchoLlmClient;

#[async_trait]
impl LlmClient for EchoLlmClient {
    fn provider_id(&self) -> &str {
        "mock-echo"
    }

    async fn complete(&self, request: &AffinityRequest) -> Result<AffinityResponse, LlmError> {
        let last_user = request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == MessageRole::User)
            .and_then(|message| {
                message.blocks.iter().find_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.to_string()),
                    _ => None,
                })
            })
            .unwrap_or_default();
        Ok(MockLlmClient::final_text(&format!(
            "mock-echo: {last_user}"
        )))
    }
}

/// 便捷构造：`User` 消息（测试断言请求体时比对用）
#[must_use]
pub fn user_message(text: &str) -> AffinityMessage {
    AffinityMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::Text {
            text: text.to_string().into_boxed_str(),
        }],
    }
}

/// 便捷构造：一个只含 prompt 的请求（比对 `tool_decls` 输出时用）
#[must_use]
pub fn request_with(
    prompt: &str,
    tools: Vec<nexus_contracts::affinity::ToolDecl>,
) -> AffinityRequest {
    AffinityRequest {
        intent_id: "mock-intent".to_string().into_boxed_str(),
        messages: vec![user_message(prompt)],
        tools,
        thinking_pref: ThinkingPreference::Standard,
        budget_hint_micro: None,
        overrides: AffinityOverrides::default(),
        sampling: SamplingParams::default(),
        output_format: OutputFormat::default(),
    }
}
