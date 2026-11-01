//! `LlmToolLoop` —— 模型 ↔ 工具的多轮闭环（Phase 7，ADR-232）
//!
//! 对应架构层: L10 Interface（chimera-cli）
//!
//! # 一条执行路径
//! 工具调用一律经 L7 `ToolRuntime::invoke_batch`，本模块不自建执行/超时/并发逻辑：
//! 并行水位、审批位点、payload 截断、取消传播都由 L7 负责。本层只做四件
//! 只有 L10 才该做的事：
//!
//! 1. 组装 `AffinityRequest`（含逐轮增长的 messages）；
//! 2. 把 `ContentBlock::ToolUse` 物化为 `ToolInvocation` 并做**审批前置**
//!    （未获批的调用根本不进 runtime —— 见 `approval.rs`）；
//! 3. 把 `ToolOutcome` 回填成 `ContentBlock::ToolResult`（未知工具/失败/被拒一律以
//!    `is_error` 喂回模型，而不是打断循环 —— 模型需要知道发生了什么才能收敛）；
//! 4. 轮数 / 调用数 / 重复次数 / token 预算四类硬上限与取消。
//!
//! # 守恒
//! `Thinking` 块原样进历史（`VerbatimThinking` 策略），不 strip、不重排；
//! 结果顺序与模型给出的调用顺序严格一致（`invoke_batch` 保序 + 按下标回填）。

use std::collections::HashMap;
use std::sync::Arc;

use event_bus::{EventBus, EventMetadata, NexusEvent};
use gqep_executor::tool_plane::{
    ToolDescriptor, ToolInvocation, ToolOrigin, ToolOutcome, ToolPriority, ToolRuntime, ToolStatus,
};
use nexus_contracts::affinity::{
    AffinityMessage, AffinityOverrides, AffinityRequest, AffinityResponse, ContentBlock,
    MessageRole, OutputFormat, SamplingParams, ThinkingPreference, UsageReport,
};
use nexus_contracts::tool_plan::SideEffectDecl;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::approval::ToolApproval;
use super::client::{LlmClient, LlmLoopError, LlmLoopOutcome, LlmLoopStop};
use super::tools::tool_decls;

/// 被拒调用回填给模型的固定标记（模型据此知道"这步不被允许"而非"工具坏了"）
pub const ASK_REQUIRED_MARKER: &str = "ask_required";

/// 循环配置
#[derive(Debug, Clone)]
pub struct LlmToolLoopConfig {
    /// 最大补全轮数
    pub max_turns: usize,
    /// 整个循环最多派发多少个工具调用
    pub max_tool_calls: usize,
    /// 单轮内的分批宽度（批间是屏障，批内由 L7 调度器并发）
    pub max_parallel_tools: usize,
    /// 同一 `(tool, args)` 允许的重复次数上限（超出即判模型打转）
    pub max_repeat_tool_calls: usize,
    /// 累计 token 上限（input + output）
    pub max_total_tokens: u64,
    /// 系统提示
    pub system_prompt: String,
    /// 思考档位
    pub thinking_pref: ThinkingPreference,
    /// 预算提示（微元，透传给 provider）
    pub budget_hint_micro: Option<u64>,
}

impl Default for LlmToolLoopConfig {
    fn default() -> Self {
        Self {
            max_turns: 8,
            max_tool_calls: 32,
            max_parallel_tools: 8,
            max_repeat_tool_calls: 3,
            max_total_tokens: 128_000,
            system_prompt: "你是 Chimera 工具编排助手。需要外部事实时必须调用工具，\
并在拿到结果后给出最终中文回答。"
                .to_string(),
            thinking_pref: ThinkingPreference::Standard,
            budget_hint_micro: None,
        }
    }
}

/// 一轮补全后的判定
enum TurnStep {
    /// 需要执行这些 `(调用 id, 工具名, args_json)` 并回填后继续
    ContinueTools(Vec<(String, String, String)>),
    /// 模型给出无 ToolUse 的最终文本
    Finished(String),
    /// 触到硬上限而停止（携带本轮已产出的文本）
    Stopped(LlmLoopStop, String),
}

/// 模型 ↔ 工具循环
pub struct LlmToolLoop {
    client: Arc<dyn LlmClient>,
    runtime: Arc<ToolRuntime>,
    bus: EventBus,
    cfg: LlmToolLoopConfig,
}

impl LlmToolLoop {
    /// 装配循环 —— 客户端 / 工具运行时 / 事件总线 / 配置全部外部注入
    ///
    /// WHY 不收 `&Arc<ToolRuntime>`：`run` 内部把 runtime 交给 `invoke_batch`
    /// 并跨 await 持有，`Arc` 是本层的生命周期前提而非调用方偏好。
    #[must_use]
    pub fn new(
        client: Arc<dyn LlmClient>,
        runtime: Arc<ToolRuntime>,
        bus: EventBus,
        cfg: LlmToolLoopConfig,
    ) -> Self {
        Self {
            client,
            runtime,
            bus,
            cfg,
        }
    }

    /// 当前配置（诊断/测试断言用）
    #[must_use]
    pub fn config(&self) -> &LlmToolLoopConfig {
        &self.cfg
    }

    /// 跑完一条闭环
    pub async fn run(
        &self,
        prompt: &str,
        tools: &[ToolDescriptor],
        cancel: &CancellationToken,
        approval: &dyn ToolApproval,
    ) -> Result<LlmLoopOutcome, LlmLoopError> {
        let decls = tool_decls(tools);
        let side_effects: HashMap<&str, SideEffectDecl> = tools
            .iter()
            .map(|descriptor| (descriptor.name.as_str(), descriptor.side_effect))
            .collect();

        // 请求体常驻：`intent_id` / `tools` / 静态档位一次填好，每轮只往
        // `request.messages` 原地追加历史。
        //
        // WHY 不每轮新建 request：`LlmClient::complete` 收的是 `&AffinityRequest`
        // （借用，不是成本源），所以"每轮造一个新 request"唯一的产出就是把整段历史
        // 和整张声明表各深拷一遍。实测这一段在 26 条历史上要 4.4998 µs
        // （`llm_diag_request_rebuild/history_26`），而一条只发 Final 的整轮读数是
        // 4.4205 µs（`llm_loop_16_tools_notools`）——同一量级，说明每轮固定成本几乎
        // 全在这一处。改的只是"每轮交出一份新历史"变成"同一份历史留在原地追加"，
        // 请求的形状、字段与所有权语义都不变。
        let mut request = AffinityRequest {
            // WHY 常量:本层不拥有 intent 生命周期（上层 run/chat 才有),
            // 需要跨层追踪时由调用方传入,而不是在此伪造 UUID
            intent_id: "llm-tool-loop".to_string().into_boxed_str(),
            messages: vec![
                AffinityMessage {
                    role: MessageRole::System,
                    blocks: vec![text_block(&self.cfg.system_prompt)],
                },
                AffinityMessage {
                    role: MessageRole::User,
                    blocks: vec![text_block(prompt)],
                },
            ],
            tools: decls,
            thinking_pref: self.cfg.thinking_pref,
            budget_hint_micro: self.cfg.budget_hint_micro,
            overrides: AffinityOverrides::default(),
            sampling: SamplingParams::default(),
            output_format: OutputFormat::default(),
        };

        // WHY 初值给 MaxTurns 而非派生 Default:每条 return 路径都显式落 stop,
        // 万一有路径漏落,MaxTurns 是"耗尽轮数没收敛"的真实语义,
        // 而 `Final` 会把漏落伪装成正常结束（不给 LlmLoopOutcome 派生 Default 同理）。
        let mut outcome = LlmLoopOutcome {
            text: String::new(),
            turns: 0,
            tool_calls: 0,
            usage: UsageReport::default(),
            stop: LlmLoopStop::MaxTurns,
            denied_tool_calls: 0,
        };
        let mut repeats: HashMap<(String, String), usize> = HashMap::new();
        let mut last_text = String::new();

        for turn in 1..=self.cfg.max_turns {
            if cancel.is_cancelled() {
                return Err(cancelled(&outcome));
            }
            outcome.turns = turn;

            let response = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(cancelled(&outcome)),
                result = self.client.complete(&request) => result.map_err(LlmLoopError::Client)?,
            };

            accumulate_usage(&mut outcome.usage, response.usage);
            if total_tokens(&outcome.usage) > self.cfg.max_total_tokens {
                self.publish_budget_exceeded(&outcome);
                outcome.text = last_text;
                outcome.stop = LlmLoopStop::Budget;
                return Ok(outcome);
            }

            // 守恒:assistant 消息整体入历史(Thinking / ToolUse 块逐字保留)
            // 这里 `response.blocks.clone()` 是响应侧的一次拷贝（response 之后还要
            // 交给 `step_of` 判定），不是"整段历史重拷"——本批消掉的是后者。
            request.messages.push(AffinityMessage {
                role: MessageRole::Assistant,
                blocks: response.blocks.clone(),
            });

            match step_of(&response, &self.cfg, &outcome, &mut repeats) {
                TurnStep::Finished(text) => {
                    outcome.text = text;
                    outcome.stop = LlmLoopStop::Final;
                    return Ok(outcome);
                }
                TurnStep::Stopped(stop, text) => {
                    outcome.text = text;
                    outcome.stop = stop;
                    return Ok(outcome);
                }
                TurnStep::ContinueTools(calls) => {
                    last_text = super::tools::text_of(&response.blocks);
                    let results = self
                        .dispatch(&calls, &side_effects, cancel, approval, &mut outcome)
                        .await?;
                    request.messages.push(AffinityMessage {
                        role: MessageRole::Tool,
                        blocks: results,
                    });
                }
            }
        }

        // 轮数耗尽仍未收敛
        outcome.text = last_text;
        outcome.stop = LlmLoopStop::MaxTurns;
        Ok(outcome)
    }

    /// 派发一批工具调用，返回与调用同序的结果块
    ///
    /// 未获审批的调用**不进 runtime**：直接合成 `ask_required` 错误结果回填。
    async fn dispatch(
        &self,
        calls: &[(String, String, String)],
        side_effects: &HashMap<&str, SideEffectDecl>,
        cancel: &CancellationToken,
        approval: &dyn ToolApproval,
        outcome: &mut LlmLoopOutcome,
    ) -> Result<Vec<ContentBlock>, LlmLoopError> {
        let mut slots: Vec<Option<ContentBlock>> = vec![None; calls.len()];
        // 审批阶段就把 invocation 留在它自己的槽位上，`approved` 只记"哪些槽过了审批"。
        // WHY:旧写法把 `(usize, ToolInvocation)` 收进一个紧凑 vec，再对每个 chunk
        // `group.iter().map(|(_, inv)| inv.clone())`——于是每个窗口都把整批调用又拷
        // 一遍（每条 4 个 String）。留槽之后派发时是**交出所有权**，零拷贝，且槽位
        // 天然带着结果该回填的下标。
        let mut invocations: Vec<Option<ToolInvocation>> = (0..calls.len()).map(|_| None).collect();
        let mut approved: Vec<usize> = Vec::with_capacity(calls.len());

        for (index, (id, name, input_json)) in calls.iter().enumerate() {
            outcome.tool_calls = outcome.tool_calls.saturating_add(1);
            let mut invocation = ToolInvocation {
                call_id: id.clone(),
                tool: name.clone(),
                args_json: input_json.clone(),
                timeout_ms: None,
                priority: ToolPriority::Normal,
                origin: ToolOrigin::Llm,
                quest_id: None,
                task_id: None,
                approved: false,
            };
            // 未在工具表里(＝未知工具)按最严口径处理：写路径要审批
            let side_effect = side_effects
                .get(name.as_str())
                .copied()
                .unwrap_or(SideEffectDecl::Write);
            if !approval.approve(&invocation, side_effect).await {
                outcome.denied_tool_calls = outcome.denied_tool_calls.saturating_add(1);
                slots[index] = Some(error_result(id, ASK_REQUIRED_MARKER));
                continue;
            }
            invocation.approved = true;
            invocations[index] = Some(invocation);
            approved.push(index);
        }

        let window = self.cfg.max_parallel_tools.max(1);
        for group in approved.chunks(window) {
            let batch: Vec<ToolInvocation> = group
                .iter()
                .filter_map(|&slot| invocations[slot].take())
                .collect();
            let executed = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(cancelled(outcome)),
                results = self.runtime.invoke_batch(batch, cancel) => results,
            };
            debug_assert_eq!(
                executed.len(),
                group.len(),
                "invoke_batch 返回的结果数与派发数不等 ⇒ 有调用没有结果（本仓红线：结果不许丢）"
            );
            for (slot, result) in group.iter().zip(executed) {
                // invoke_batch 保序 ⇒ 逐位对应；不一致即为 L7 契约破口。
                // 比对基准是**原始 tool_use id**（`calls[slot].0`），不是 invocation
                // 里的副本——invocation 已被 move 进 batch，正好省掉一次拷贝。
                debug_assert_eq!(
                    result.call_id, calls[*slot].0,
                    "invoke_batch 必须按入参顺序回填 call_id"
                );
                slots[*slot] = Some(result_block(&result));
            }
        }

        Ok(slots.into_iter().flatten().collect())
    }

    /// 发布既有 `BudgetExceeded`（Critical ⇒ `publish_critical` 走 mpsc 旁路）
    fn publish_budget_exceeded(&self, outcome: &LlmLoopOutcome) {
        let event = NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("chimera-cli:LlmToolLoop"),
            budget_type: "llm_total_tokens".to_string(),
            current: total_tokens(&outcome.usage),
            limit: self.cfg.max_total_tokens,
        };
        let bus = self.bus.clone();
        // WHY spawn:事件发布不得阻塞循环返回路径;Critical 走 mpsc 保证送达
        tokio::spawn(async move {
            if let Err(error) = bus.publish_critical(event).await {
                warn!(error = %error, "llm 预算事件发布失败");
            }
        });
    }
}

impl LlmToolLoopConfig {
    /// 从 `CHIMERA_LLM_*` env 读取上限（非法/0 值回退默认，不 panic）
    ///
    /// WHY env 而非配置文件：与 `CHIMERA_TOOL_*` / `CHIMERA_QUEST_*` 同族，
    /// 灰度与限额属运行期调参面；系统提示词刻意**不**走 env（多行文本经 shell
    /// 传参必坏，且它会改变模型行为，应经代码/配置显式给出）。
    #[must_use]
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            max_turns: env_usize("CHIMERA_LLM_MAX_TURNS", defaults.max_turns),
            max_tool_calls: env_usize("CHIMERA_LLM_MAX_TOOL_CALLS", defaults.max_tool_calls),
            max_parallel_tools: env_usize(
                "CHIMERA_LLM_MAX_PARALLEL_TOOLS",
                defaults.max_parallel_tools,
            ),
            max_repeat_tool_calls: env_usize(
                "CHIMERA_LLM_MAX_REPEAT",
                defaults.max_repeat_tool_calls,
            ),
            max_total_tokens: env_u64("CHIMERA_LLM_MAX_TOTAL_TOKENS", defaults.max_total_tokens),
            system_prompt: defaults.system_prompt,
            thinking_pref: match std::env::var("CHIMERA_LLM_THINKING").ok().as_deref() {
                Some("fast") => ThinkingPreference::Fast,
                Some("deep") => ThinkingPreference::Deep,
                _ => defaults.thinking_pref,
            },
            budget_hint_micro: std::env::var("CHIMERA_LLM_BUDGET_MICRO")
                .ok()
                .and_then(|value| value.parse().ok()),
        }
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

/// 判定本轮该继续、收敛还是因上限停止
fn step_of(
    response: &AffinityResponse,
    cfg: &LlmToolLoopConfig,
    outcome: &LlmLoopOutcome,
    repeats: &mut HashMap<(String, String), usize>,
) -> TurnStep {
    let text = super::tools::text_of(&response.blocks);
    let calls: Vec<(String, String, String)> = response
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse {
                id,
                name,
                input_json,
            } => Some((id.to_string(), name.to_string(), input_json.to_string())),
            _ => None,
        })
        .collect();

    if calls.is_empty() {
        return TurnStep::Finished(text);
    }
    // 先判调用数上限：它比"重复检测"更先，因为一次调用风暴就是打转本身
    if outcome.tool_calls.saturating_add(calls.len()) > cfg.max_tool_calls {
        return TurnStep::Stopped(LlmLoopStop::MaxToolCalls, text);
    }
    for (_, name, input_json) in &calls {
        let count = repeats
            .entry((name.clone(), input_json.clone()))
            .or_insert(0);
        *count = count.saturating_add(1);
        if *count > cfg.max_repeat_tool_calls {
            return TurnStep::Stopped(LlmLoopStop::MaxRepeatToolCalls, text);
        }
    }
    TurnStep::ContinueTools(calls)
}

fn total_tokens(usage: &UsageReport) -> u64 {
    usage.input_tokens.saturating_add(usage.output_tokens)
}

fn cancelled(outcome: &LlmLoopOutcome) -> LlmLoopError {
    LlmLoopError::Cancelled {
        turns: outcome.turns,
        tool_calls: outcome.tool_calls,
    }
}

/// 累计 usage（逐轮相加，不回退）
fn accumulate_usage(total: &mut UsageReport, added: UsageReport) {
    total.input_tokens = total.input_tokens.saturating_add(added.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(added.output_tokens);
    total.cache_hit_tokens = total
        .cache_hit_tokens
        .saturating_add(added.cache_hit_tokens);
    total.thinking_tokens = match (total.thinking_tokens, added.thinking_tokens) {
        (Some(a), Some(b)) => Some(a.saturating_add(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
}

fn text_block(text: &str) -> ContentBlock {
    ContentBlock::Text {
        text: text.to_string().into_boxed_str(),
    }
}

fn error_result(tool_use_id: &str, content: &str) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: tool_use_id.to_string().into_boxed_str(),
        content: content.to_string().into_boxed_str(),
        is_error: true,
    }
}

/// `ToolOutcome` → `ToolResult` 块（非 Ok 一律 `is_error`，内容取 provider 错误文本）
fn result_block(executed: &ToolOutcome) -> ContentBlock {
    let ok = executed.status == ToolStatus::Ok;
    let content = if ok {
        executed.payload.clone().unwrap_or_default()
    } else {
        executed
            .error
            .clone()
            .unwrap_or_else(|| format!("{:?}", executed.status))
    };
    ContentBlock::ToolResult {
        tool_use_id: executed.call_id.clone().into_boxed_str(),
        content: content.into_boxed_str(),
        is_error: !ok,
    }
}
