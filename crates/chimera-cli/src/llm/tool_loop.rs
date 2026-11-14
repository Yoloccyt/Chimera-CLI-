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
use futures::stream::{FuturesUnordered, StreamExt};
use gqep_executor::tool_plane::{
    ToolDescriptor, ToolInvocation, ToolOrigin, ToolOutcome, ToolPriority, ToolRuntime, ToolStatus,
};
use nexus_contracts::affinity::{
    AffinityMessage, AffinityOverrides, AffinityRequest, AffinityResponse, ContentBlock,
    MessageRole, OutputFormat, SamplingParams, ThinkingPreference, UsageReport,
};
use nexus_contracts::tool_plan::SideEffectDecl;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::approval::ToolApproval;
use super::client::{LlmClient, LlmLoopError, LlmLoopOutcome, LlmLoopStop};
use super::tools::tool_decls;

/// 被拒调用回填给模型的固定标记（模型据此知道"这步不被允许"而非"工具坏了"）
pub const ASK_REQUIRED_MARKER: &str = "ask_required";

/// 结果无法按 `call_id` 归位时的标记（L7 少回/错回 call_id 的情形）。
///
/// WHY 单独一个标记而不是复用 `ask_required`：语义不同——那不是审批决策而是执行面
/// 契约破口，混在一起会让"被拒了几次"与"归位坏了几次"读成同一件事。
/// WHY 不 panic：`debug_assert` 在 release 档被编译掉，而 release 正是发布与
/// `bench_check`/`slo` 的档位；把可见性押在 debug 上等于在最需要它的构建里失明。
const ATTRIBUTION_MISMATCH_MARKER: &str = "attribution_mismatch";

/// 循环配置
#[derive(Debug, Clone)]
pub struct LlmToolLoopConfig {
    /// 最大补全轮数
    pub max_turns: usize,
    /// 整个循环最多派发多少个工具调用
    pub max_tool_calls: usize,
    /// 单轮的两个并发窗口宽度：同时在问的弹窗数 ≤ 此值，同时在飞的执行数 ≤ 此值。
    /// 两窗口各自独立计数（不共享额度），且**不是批次屏障** —— 一条调用在等人回答
    /// 不会按住同批里不需要等人的那些。
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
    /// 未获审批的调用**不进 runtime**：合成 `AskRequired` 结果，与执行结果一起交给同一个
    /// `attribute_results` 按 `call_id` 归位（那条函数与它的两条锁都不动）。
    ///
    /// # 两个独立窗口（第四波 S2，`ADR-233` 追加节九 D1）
    /// 旧写法是"审批 for 循环全部走完 → 再 `chunks(window)` 分批执行"，把问人与执行
    /// 写成前后两段：一批里一条待答的写调用会按住同批其余调用，而 `TuiApproval` 的回执
    /// await 没有超时（等人答才是正确语义），于是人的思考时长被计入整批启动延迟。
    /// 现在每条调用是一个独立 future：问人前取 **ask 窗口**、答完立刻归还；执行前取
    /// **exec 窗口**。两把闸互不相干 ⇒ "谁在等人"不再阻塞"不需要等人的那些"。
    ///
    /// WHY exec 窗口必须留着、不能整批交给 L7 自己限流：L7 `acquire` 在等待者超过
    /// `max_waiters` 时返回 `Overloaded`（`tool_plane/scheduler.rs`），一次交出整批会把
    /// **延后变成拒绝** —— 那是削减而非优化。窗口额度与今天的 chunk 尺寸同值
    /// （`max_parallel_tools`），故同时在飞的调用数逐位不变。
    async fn dispatch(
        &self,
        calls: &[(String, String, String)],
        side_effects: &HashMap<&str, SideEffectDecl>,
        cancel: &CancellationToken,
        approval: &dyn ToolApproval,
        outcome: &mut LlmLoopOutcome,
    ) -> Result<Vec<ContentBlock>, LlmLoopError> {
        let window = self.cfg.max_parallel_tools.max(1);
        let ask_sem = Arc::new(Semaphore::new(window));
        let exec_sem = Arc::new(Semaphore::new(window));
        // 计数发生在派发之前、逐条累加 ⇒ "派发了几条"与旧结构逐位相同（不是"执行了几条"）。
        outcome.tool_calls = outcome.tool_calls.saturating_add(calls.len());

        let mut futures = FuturesUnordered::new();
        for (index, (id, name, input_json)) in calls.iter().enumerate() {
            let (ask_sem, exec_sem) = (Arc::clone(&ask_sem), Arc::clone(&exec_sem));
            let (id, name, input_json) = (id.clone(), name.clone(), input_json.clone());
            // 未在工具表里(＝未知工具)按最严口径处理：写路径要审批
            let side_effect = side_effects
                .get(name.as_str())
                .copied()
                .unwrap_or(SideEffectDecl::Write);
            futures.push(async move {
                let mut invocation = ToolInvocation {
                    call_id: id.clone(),
                    tool: name,
                    args_json: input_json,
                    timeout_ms: None,
                    priority: ToolPriority::Normal,
                    origin: ToolOrigin::Llm,
                    quest_id: None,
                    task_id: None,
                    approved: false,
                };
                // WHY permit 跨 await 不触犯 §6.2「禁止持锁跨 await」：那条红线针对
                // Mutex/DashMap/RwLock 的**写锁**（持锁期间不得阻塞），而 Semaphore 的
                // permit 是设计上可持有的额度，只在本窗口内共享，不与其他路径互踩。
                let _ask = match ask_sem.acquire_owned().await {
                    Ok(permit) => permit,
                    // 闭包内两把闸随 dispatch 存活，理论上不可达；仍按拒收口而不 unwrap
                    Err(_) => return (index, denied_result(&invocation), true),
                };
                let allowed = approval.approve(&invocation, side_effect).await;
                drop(_ask); // 立刻归还问人额度 ⇒ 下一条可以同时在问
                if !allowed {
                    return (index, denied_result(&invocation), true);
                }
                invocation.approved = true;
                let _exec = match exec_sem.acquire_owned().await {
                    Ok(permit) => permit,
                    Err(_) => return (index, denied_result(&invocation), true),
                };
                let executed = self.runtime.invoke(invocation, cancel).await;
                drop(_exec);
                (index, executed, false)
            });
        }

        // 逐条收齐而不是 `collect`：与旧写法一样保留"取消优先"（biased）的语义。
        let mut executed: Vec<ToolOutcome> = Vec::with_capacity(calls.len());
        let mut denied = 0usize;
        for _ in 0..calls.len() {
            let next = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(cancelled(outcome)),
                item = futures.next() => item,
            };
            match next {
                Some((_index, result, was_denied)) => {
                    executed.push(result);
                    denied += usize::from(was_denied);
                }
                // 每条 future 必产出一个结果。拿到 None 说明有调用凭空消失 ——
                // 这是「结果不许丢」的破口，走下面 attribute_results 的显式标记分支
                // （运行时可见），不用 debug_assert（release 档会被编译掉 ⇒ 假绿）。
                None => break,
            }
        }
        outcome.denied_tool_calls = outcome.denied_tool_calls.saturating_add(denied);

        let mut slots: Vec<Option<ContentBlock>> = vec![None; calls.len()];
        let all: Vec<usize> = (0..calls.len()).collect();
        attribute_results(&executed, &all, calls, &mut slots);
        // attribute_results 已保证每槽有内容；这里是防御性收尾，缺位显式标记而非静默丢。
        Ok(slots
            .into_iter()
            .enumerate()
            .map(|(i, slot)| {
                slot.unwrap_or_else(|| error_result(&calls[i].0, ATTRIBUTION_MISMATCH_MARKER))
            })
            .collect())
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
/// 合成"未获审批"的结果 —— 该调用**从未进 runtime**，故 provider 记成本层、attempts=0。
///
/// WHY 造 `ToolOutcome` 而不是直接造 `ContentBlock`：新结构下被拒结果与执行结果走同一条
/// `attribute_results` 归位路径，两条锁（无 debug_assert、缺位有显式标记）因此继续有效；
/// 模型侧看到的仍是 `is_error` + `ask_required` 标记，与旧结构逐位相同。
fn denied_result(inv: &ToolInvocation) -> ToolOutcome {
    ToolOutcome {
        call_id: inv.call_id.clone(),
        tool: inv.tool.clone(),
        provider: "approval".to_string(),
        status: ToolStatus::AskRequired,
        payload: None,
        error: Some(ASK_REQUIRED_MARKER.to_string()),
        duration_ms: 0,
        attempts: 0,
        truncated: false,
    }
}

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

/// 把一批执行结果按 **`call_id`** 写回各自的槽位。
///
/// WHY 键化而不是逐位 `zip`：逐位对应的前提是"两侧同序"，而它原先唯一的防线是
/// `debug_assert_eq!(result.call_id, invocation.call_id)`。`debug_assertions` 在
/// release 档是关闭的（`bench_check.yml` / `slo.yml` 与发布构建都在那一档），于是
/// 一次 L7 保序破口在 release 下静默变成"把 A 的结果写进 B 的槽位"，块里的
/// `tool_use_id` 也跟着错——模型读到的是一个从未发起过的配对。少回一条时更糟：
/// `zip` 截断 ⇒ 那个槽位干脆没有结果，回填给模型的历史里 tool_use 与 tool_result
/// 数量不齐（本仓红线：结果不许丢）。
///
/// 键化后错配既**可见**（`warn!` + 显式标记块）又**不炸批**：不 panic、不 unwrap，
/// 每个槽位都拿到一个属于自己 `tool_use_id` 的结果。同 id 的多条结果逐条取用
/// （`pop` 从尾开始，而同 id 的结果彼此不可区分，故取用次序无关语义），一条都不丢。
fn attribute_results(
    executed: &[ToolOutcome],
    group: &[usize],
    calls: &[(String, String, String)],
    slots: &mut [Option<ContentBlock>],
) {
    let mut by_call_id: HashMap<&str, Vec<&ToolOutcome>> = HashMap::with_capacity(group.len());
    for result in executed {
        by_call_id
            .entry(result.call_id.as_str())
            .or_default()
            .push(result);
    }

    let mut consumed = 0usize;
    for &slot in group {
        let id = calls[slot].0.as_str();
        match by_call_id.get_mut(id).and_then(Vec::pop) {
            Some(result) => {
                consumed += 1;
                slots[slot] = Some(result_block(result));
            }
            None => {
                warn!(
                    call_id = id,
                    "工具结果无法按 call_id 归位（批内缺条或 L7 回错了 id）"
                );
                slots[slot] = Some(error_result(id, ATTRIBUTION_MISMATCH_MARKER));
            }
        }
    }

    if consumed != executed.len() {
        // 有结果不属于本批任何派发 ⇒ 无处可归。不假装塞得下，只把事实说清楚。
        warn!(
            consumed,
            returned = executed.len(),
            "invoke_batch 返回了本批之外的 call_id，多余结果无槽位可归"
        );
    }
}

#[cfg(test)]
mod tests {
    //! 归位逻辑的单元面。
    //!
    //! WHY 单元面而不是集成面：`LlmToolLoop` 持有的 `runtime` 是具体的
    //! `Arc<ToolRuntime>`（不是 trait），外部测试无法让 `invoke_batch` 返回乱序或
    //! 缺条 —— 而"乱序/缺条时不静默错配"正是这里要钉的契约。被测对象就是被替换的
    //! 那段逻辑本身，不是它的替身。
    //!
    //! WHY 这些断言不依赖 debug 档：归位是**运行时分支**，不是 `debug_assert`。
    //! 旧实现里唯一的防线是 `debug_assert_eq!(result.call_id, invocation.call_id)`，
    //! 而 `debug_assertions` 在 release 档（CI 的 `bench_check.yml` / `slo.yml`、
    //! 以及发布构建）是关闭的 —— 于是一次 L7 保序破口在 release 下静默变成
    //! "把 A 调用的结果写进 B 的槽位"，模型收到的 tool_result 还带着另一个调用的 id。

    use super::*;

    fn outcome(call_id: &str, payload: &str) -> ToolOutcome {
        ToolOutcome {
            call_id: call_id.to_string(),
            tool: "bench.t0".to_string(),
            provider: "bench".to_string(),
            status: ToolStatus::Ok,
            payload: Some(payload.to_string()),
            error: None,
            duration_ms: 0,
            attempts: 1,
            truncated: false,
        }
    }

    fn call(id: &str, name: &str) -> (String, String, String) {
        (id.to_string(), name.to_string(), "{}".to_string())
    }

    /// 槽位里的结果块 → (`tool_use_id`, 文本)。空槽位直接 panic：那正是"这个槽位
    /// 没被归位"的失败形状，不需要再额外写一层 `expect`（crate 面 `expect_used`
    /// 是 deny，测试面也不该靠 unwrap/expect 才能表达断言）。
    fn parts(slot: Option<&ContentBlock>) -> (String, String) {
        match slot {
            Some(ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            }) => (tool_use_id.to_string(), content.to_string()),
            other => panic!("expected a tool result block in the slot, got {other:?}"),
        }
    }

    #[test]
    fn results_are_keyed_by_call_id_not_by_position() {
        let calls = vec![
            call("a", "bench.t0"),
            call("b", "bench.t1"),
            call("c", "bench.t2"),
        ];
        let group = vec![0usize, 1, 2];
        let mut slots: Vec<Option<ContentBlock>> = vec![None; 3];
        // 测试侧把返回序整个倒过来 —— 这正是 L7 保序一旦破裂时的形状。
        let executed = vec![
            outcome("c", "third"),
            outcome("b", "second"),
            outcome("a", "first"),
        ];

        attribute_results(&executed, &group, &calls, &mut slots);

        let got: Vec<(String, String)> = slots.iter().map(|slot| parts(slot.as_ref())).collect();
        assert_eq!(
            got,
            vec![
                ("a".to_string(), "first".to_string()),
                ("b".to_string(), "second".to_string()),
                ("c".to_string(), "third".to_string()),
            ],
            "归位必须按 call_id：每个槽位拿到的结果内容与其 tool_use_id 同源"
        );
    }

    #[test]
    fn missing_result_becomes_visible_error_not_a_silent_hole() {
        let calls = vec![call("a", "bench.t0"), call("b", "bench.t1")];
        let group = vec![0usize, 1];
        let mut slots: Vec<Option<ContentBlock>> = vec![None; 2];
        // 只回来 a ⇒ b 没有结果（缺条，不是错序）。
        let executed = vec![outcome("a", "first")];

        attribute_results(&executed, &group, &calls, &mut slots);

        let (id_a, text_a) = parts(slots[0].as_ref());
        assert_eq!((id_a.as_str(), text_a.as_str()), ("a", "first"));
        let (id_b, text_b) = parts(slots[1].as_ref());
        assert_eq!(id_b, "b", "缺失结果的槽位仍要带自己的 tool_use_id");
        assert_eq!(
            text_b, ATTRIBUTION_MISMATCH_MARKER,
            "b 必须显式可见为归位失败，而不是把 a 的结果写进来或被静默丢弃"
        );
    }

    #[test]
    fn result_with_unknown_call_id_is_not_placed_anywhere() {
        let calls = vec![call("a", "bench.t0"), call("b", "bench.t1")];
        let group = vec![0usize, 1];
        let mut slots: Vec<Option<ContentBlock>> = vec![None; 2];
        // 两条结果都带着批外的 id（L7 若把 call_id 写错到这里）。
        let executed = vec![outcome("x", "stray-1"), outcome("y", "stray-2")];

        attribute_results(&executed, &group, &calls, &mut slots);

        for (index, slot) in slots.iter().enumerate() {
            let (id, text) = parts(slot.as_ref());
            assert_eq!(id, calls[index].0, "槽位 {index} 的 id 必须是自己那条");
            assert_eq!(
                text, ATTRIBUTION_MISMATCH_MARKER,
                "批外 id 的结果不许被塞进任何槽位"
            );
        }
    }

    /// 归位不依赖 `debug_assertions`——这条是**机制层面**的锁。
    ///
    /// WHY 需要这一条：本文件里 `dispatch` 确实还留着 `debug_assert_eq!`（结果数与
    /// 派发数不等时中止测试构建），而 release 档（`bench_check.yml` / `slo.yml` /
    /// 发布构建）会把它编译掉。若归位行为也押在那类断言上，"测试全绿"就只在 debug
    /// 档成立 —— 那正是"假绿"的形状。
    /// WHY 用源码扫描而不是"在 release 档再跑一次"：release 跑一遍只能证明"没失败"，
    /// 不能说明**为什么**不依赖档位；而"helper 体内没有 debug_assert"是可直接机检的
    /// 结构性事实，且有人在归位路径上加回 debug_assert 时这条会立刻红。
    #[test]
    fn attribution_behavior_is_not_debug_only() {
        const SRC: &str = include_str!("tool_loop.rs");
        let Some(start) = SRC.find("fn attribute_results(") else {
            panic!("attribute_results 不在了——这条结构断言的锚点需要先更新");
        };
        let Some(end) = SRC[start..].find(
            "
}
",
        ) else {
            panic!("找不到 attribute_results 的函数体结尾");
        };
        let body = &SRC[start..start + end];
        assert!(
            !body.contains("debug_assert"),
            "归位函数体内出现了 debug_assert ⇒ 该行为在 release 档会被编译掉:
{body}"
        );
        assert!(
            body.contains("ATTRIBUTION_MISMATCH_MARKER"),
            "归位失败必须有运行时可见性，而不是只在中止时可见"
        );
    }

    #[test]
    fn duplicate_tool_use_ids_keep_every_result() {
        // 模型侧用同一个 tool_use_id 发两次调用是可能发生的（本层不替它去重）。
        // 逐位实现会把两条结果按位置分给两个槽；键化实现必须**一条都不丢**。
        let calls = vec![call("dup", "bench.t0"), call("dup", "bench.t0")];
        let group = vec![0usize, 1];
        let mut slots: Vec<Option<ContentBlock>> = vec![None; 2];
        let executed = vec![outcome("dup", "one"), outcome("dup", "two")];

        attribute_results(&executed, &group, &calls, &mut slots);

        let texts: Vec<String> = slots.iter().map(|slot| parts(slot.as_ref()).1).collect();
        assert!(
            texts.iter().all(|text| text != ATTRIBUTION_MISMATCH_MARKER),
            "同 id 的多条结果不该被判成归位失败: {texts:?}"
        );
        assert_eq!(texts.len(), 2);
        assert_ne!(
            texts[0], texts[1],
            "两条结果都必须进入历史，不能一条覆盖另一条: {texts:?}"
        );
    }
}
