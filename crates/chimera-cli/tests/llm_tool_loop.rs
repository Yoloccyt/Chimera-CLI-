//! LLM 工具循环契约（Phase 7 / T7.1–T7.5，ADR-232）
//!
//! 全程用 `MockLlmClient` + 进程内探针 provider：无网络、无 API key、不依赖真实
//! 厂商。断言只押在两处客观事实：**模型看到的请求体**（`seen_requests`）与
//! **工具真的被调用了几次/是否并发**（provider 侧计数）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom
#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use event_bus::{EventBus, NexusEvent};
use gqep_executor::tool_plane::{
    BoundedToolScheduler, ToolDescriptor, ToolError, ToolInvocation, ToolProvider, ToolRegistry,
    ToolRuntime, ToolRuntimeConfig,
};
use nexus_contracts::affinity::{AffinityResponse, ContentBlock, MessageRole};
use nexus_contracts::tool_plan::SideEffectDecl;
use tokio_util::sync::CancellationToken;

use chimera_cli::llm::client::LlmClient;
use chimera_cli::llm::tool_loop::{LlmToolLoop, LlmToolLoopConfig, ASK_REQUIRED_MARKER};
use chimera_cli::llm::{
    tool_decls, DenyWrites, LlmError, MockLlmClient, ToolApproval, TuiApproval,
};
use chimera_tui::approval_gate::{ApprovalGate, ApprovalTicket};

// ============================================================
// 探针 provider
// ============================================================

/// 三个工具：`probe.echo`(只读) / `probe.write`(写) / `probe.slow`(只读、长耗时)
#[derive(Clone)]
struct ProbeProvider {
    live: Arc<std::sync::atomic::AtomicUsize>,
    peak: Arc<std::sync::atomic::AtomicUsize>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl ProbeProvider {
    fn new() -> Self {
        Self {
            live: Arc::default(),
            peak: Arc::default(),
            calls: Arc::default(),
        }
    }

    fn peak(&self) -> usize {
        self.peak.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl ToolProvider for ProbeProvider {
    fn provider_id(&self) -> &str {
        "probe"
    }

    fn descriptors(&self) -> Vec<ToolDescriptor> {
        let descriptor = |name: &str, side_effect: SideEffectDecl| ToolDescriptor {
            name: format!("probe.{name}"),
            provider: "probe".to_string(),
            description: format!("probe tool {name}"),
            side_effect,
            timeout_ms: 30_000,
            max_concurrency: 16,
            input_schema: Some(r#"{"type":"object"}"#.to_string()),
        };
        vec![
            descriptor("echo", SideEffectDecl::ReadOnly),
            descriptor("write", SideEffectDecl::Write),
            descriptor("slow", SideEffectDecl::ReadOnly),
        ]
    }

    async fn invoke(
        &self,
        inv: &ToolInvocation,
        cancel: CancellationToken,
    ) -> Result<String, ToolError> {
        use std::sync::atomic::Ordering;
        let now = self.live.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        self.calls.lock().unwrap().push(inv.tool.clone());
        if inv.tool == "probe.slow" {
            // 长耗时且必须响应取消（真实进程面靠 kill_on_drop，这里等价用 select）
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                _ = cancel.cancelled() => {}
            }
        } else {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.live.fetch_sub(1, Ordering::SeqCst);
        Ok(format!("{}:{}", inv.tool, inv.args_json))
    }
}

fn runtime() -> (Arc<ToolRuntime>, ProbeProvider) {
    let provider = ProbeProvider::new();
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(provider.clone()))
        .expect("register probe provider");
    let scheduler = Arc::new(BoundedToolScheduler::new(16, 16, 256, 16));
    let runtime = Arc::new(ToolRuntime::new(
        Arc::new(registry),
        None,
        scheduler,
        ToolRuntimeConfig {
            interactive_inflight: 16,
            background_inflight: 16,
            max_waiters: 256,
            max_provider_inflight: 16,
            default_timeout_ms: 30_000,
            max_payload_bytes: 4_096,
            max_batch: 64,
        },
    ));
    (runtime, provider)
}

fn loop_config() -> LlmToolLoopConfig {
    LlmToolLoopConfig {
        max_turns: 8,
        max_tool_calls: 32,
        max_parallel_tools: 8,
        max_repeat_tool_calls: 3,
        max_total_tokens: u64::MAX,
        system_prompt: "测试系统提示".to_string(),
        ..LlmToolLoopConfig::default()
    }
}

fn tool_calls_in(blocks: &[ContentBlock]) -> Vec<(String, String, String)> {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse {
                id,
                name,
                input_json,
            } => Some((id.to_string(), name.to_string(), input_json.to_string())),
            _ => None,
        })
        .collect()
}

/// 取出某轮请求里被回填的全部 `ToolResult`（"结果有没有喂回模型"的直接证据）
fn fed_results(
    request: &nexus_contracts::affinity::AffinityRequest,
) -> Vec<(String, String, bool)> {
    request
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::Tool)
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some((tool_use_id.to_string(), content.to_string(), *is_error)),
            _ => None,
        })
        .collect()
}

// ============================================================
// T7.1 / T7.2 —— 单轮回填与声明映射
// ============================================================

/// 一轮工具调用 → 结果回填 → 第二轮收敛
#[tokio::test]
async fn loop_executes_tool_and_feeds_result_back() {
    let (runtime, provider) = runtime();
    let client = Arc::new(MockLlmClient::new([
        MockLlmClient::tool_use(&[("c1", "probe.echo", r#"{"x":1}"#)]),
        MockLlmClient::final_text("done"),
    ]));
    let tools = provider.descriptors();
    let chain = LlmToolLoop::new(client.clone(), runtime, EventBus::new(), loop_config());

    let outcome = chain
        .run("用一下工具", &tools, &CancellationToken::new(), &DenyWrites)
        .await
        .expect("loop ok");

    assert_eq!(outcome.text, "done");
    assert_eq!(outcome.stop, chimera_cli::llm::LlmLoopStop::Final);
    assert_eq!(outcome.turns, 2, "工具轮 + 收敛轮");
    assert_eq!(outcome.tool_calls, 1);
    assert_eq!(provider.calls(), vec!["probe.echo".to_string()]);

    // 第二轮请求必须带 Tool 角色消息,且结果块对得上 c1
    let seen = client.seen_requests();
    assert_eq!(seen.len(), 2);
    let fed_back = fed_results(&seen[1]);
    assert_eq!(fed_back.len(), 1, "必须回填一条工具结果");
    assert_eq!(fed_back[0].0, "c1", "结果必须按 tool_use_id 对应回调用");
    assert!(!fed_back[0].2, "成功结果 is_error=false");
    assert!(
        fed_back[0].1.contains("probe.echo"),
        "内容来自 provider 返回"
    );
}

/// `ToolDescriptor` → `ToolDecl`：只暴露三件事，schema 缺省有兜底
#[test]
fn tool_decls_maps_descriptor() {
    let mut without_schema = ToolDescriptor {
        name: "x.y".to_string(),
        provider: "x".to_string(),
        description: "desc".to_string(),
        side_effect: SideEffectDecl::ReadOnly,
        timeout_ms: 1,
        max_concurrency: 1,
        input_schema: None,
    };
    without_schema.timeout_ms = 1;
    let decls = tool_decls(&[without_schema]);
    assert_eq!(decls.len(), 1);
    assert_eq!(&*decls[0].name, "x.y");
    assert_eq!(&*decls[0].description, "desc");
    assert_eq!(
        &*decls[0].parameters_schema,
        chimera_cli::llm::FALLBACK_OBJECT_SCHEMA,
        "缺 schema 必须走兜底而不是空字符串"
    );
    // WHY 断言里没有 timeout/max_concurrency：执行属性不得进 prompt(占 token 且像可协商参数)
    let json = serde_json::to_string(&decls).expect("decl serializable");
    assert!(!json.contains("timeout"), "执行属性不得泄漏: {json}");
    assert!(
        !json.contains("max_concurrency"),
        "执行属性不得泄漏: {json}"
    );
}

/// mock 必须记录每一轮请求体（否则"回填是否发生"无法证明）
#[tokio::test]
async fn mock_client_records_requests() {
    let client = MockLlmClient::new([MockLlmClient::final_text("ok")]);
    let request = chimera_cli::llm::mock::request_with("hi", vec![]);
    let response: AffinityResponse = client.complete(&request).await.expect("mock complete ok");
    assert_eq!(tool_calls_in(&response.blocks).len(), 0);
    assert_eq!(client.seen_count(), 1);
    assert_eq!(client.seen_requests()[0].messages.len(), 1);

    // 脚本耗尽必须报错(防止"模型还在要工具"被静默当成结束)
    let exhausted = client.complete(&request).await;
    assert!(matches!(
        exhausted,
        Err(LlmError::Protocol(message)) if message.contains("脚本已耗尽")
    ));
}

// ============================================================
// T7.3 —— 并行、上限与重复
// ============================================================

/// 8 个 ToolUse：批宽 8 ⇒ 峰值 ≤ 8 且确实并发（≥2）
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loop_parallel_tool_calls_bounded() {
    let (runtime, provider) = runtime();
    let calls: Vec<(String, String, String)> = (0..8)
        .map(|i| {
            (
                format!("c{i}"),
                "probe.echo".to_string(),
                format!(r#"{{"i":{i}}}"#),
            )
        })
        .collect();
    let script = [
        MockLlmClient::tool_use(
            &calls
                .iter()
                .map(|(id, name, args)| (id.as_str(), name.as_str(), args.as_str()))
                .collect::<Vec<_>>(),
        ),
        MockLlmClient::final_text("8 done"),
    ];
    let client = Arc::new(MockLlmClient::new(script));
    let mut cfg = loop_config();
    cfg.max_parallel_tools = 8;
    let chain = LlmToolLoop::new(client, runtime, EventBus::new(), cfg);

    let outcome = chain
        .run(
            "并行",
            &provider.descriptors(),
            &CancellationToken::new(),
            &DenyWrites,
        )
        .await
        .expect("loop ok");

    assert_eq!(outcome.tool_calls, 8);
    assert_eq!(provider.calls().len(), 8, "8 个调用必须全部派发");
    let peak = provider.peak();
    assert!(peak <= 8, "峰值在飞 {peak} 必须 ≤ max_parallel_tools=8");
    assert!(peak >= 2, "峰值 {peak} 说明根本没并行,批派发退化成串行");
}

/// 轮数耗尽 ⇒ `MaxTurns`（不是 Final，调用方据此知道"没谈完"）
#[tokio::test]
async fn loop_stops_at_max_turns() {
    let (runtime, provider) = runtime();
    let script: Vec<AffinityResponse> = (0..4)
        .map(|i| {
            // 每轮参数不同 ⇒ 只有"轮数"会上限,不会被重复检测抢先
            let args = format!(r#"{{"turn":{i}}}"#);
            MockLlmClient::tool_use(&[("c", "probe.echo", args.as_str())])
        })
        .collect();
    let client = Arc::new(MockLlmClient::new(script));
    let mut cfg = loop_config();
    cfg.max_turns = 2;
    cfg.max_repeat_tool_calls = 99; // 让"重复"不抢先,专测轮数
    let chain = LlmToolLoop::new(client, runtime, EventBus::new(), cfg);

    let outcome = chain
        .run(
            "一直要工具",
            &provider.descriptors(),
            &CancellationToken::new(),
            &DenyWrites,
        )
        .await
        .expect("loop ok");
    assert_eq!(outcome.stop, chimera_cli::llm::LlmLoopStop::MaxTurns);
    assert_eq!(outcome.turns, 2);
}

/// 调用数上限 ⇒ `MaxToolCalls`，且**不执行**超出的那一批
#[tokio::test]
async fn loop_stops_at_max_tool_calls() {
    let (runtime, provider) = runtime();
    let client = Arc::new(MockLlmClient::new([
        MockLlmClient::tool_use(&[("a", "probe.echo", "{}"), ("b", "probe.echo", "{}")]),
        MockLlmClient::tool_use(&[("c", "probe.echo", "{}")]),
        MockLlmClient::final_text("never"),
    ]));
    let mut cfg = loop_config();
    cfg.max_tool_calls = 2;
    let chain = LlmToolLoop::new(client, runtime, EventBus::new(), cfg);

    let outcome = chain
        .run(
            "两个就够",
            &provider.descriptors(),
            &CancellationToken::new(),
            &DenyWrites,
        )
        .await
        .expect("loop ok");
    assert_eq!(outcome.stop, chimera_cli::llm::LlmLoopStop::MaxToolCalls);
    assert_eq!(outcome.tool_calls, 2, "第三次的批次必须整体不派发");
    assert_eq!(provider.calls().len(), 2);
}

/// 同 `(tool,args)` 重复超上限 ⇒ `MaxRepeatToolCalls`（模型打转的确定性信号）
#[tokio::test]
async fn loop_detects_repeated_tool_calls() {
    let (runtime, provider) = runtime();
    let same = || MockLlmClient::tool_use(&[("c", "probe.echo", r#"{"same":1}"#)]);
    let client = Arc::new(MockLlmClient::new([
        same(),
        same(),
        same(),
        MockLlmClient::final_text("never"),
    ]));
    let mut cfg = loop_config();
    cfg.max_repeat_tool_calls = 2;
    let chain = LlmToolLoop::new(client, runtime, EventBus::new(), cfg);

    let outcome = chain
        .run(
            "重复吧",
            &provider.descriptors(),
            &CancellationToken::new(),
            &DenyWrites,
        )
        .await
        .expect("loop ok");
    assert_eq!(
        outcome.stop,
        chimera_cli::llm::LlmLoopStop::MaxRepeatToolCalls,
        "第 3 次同参调用必须掐断"
    );
    assert_eq!(outcome.tool_calls, 2);
}

/// Thinking 块必须原样回到下一轮请求（状态守恒）
#[tokio::test]
async fn loop_preserves_thinking_blocks() {
    let (runtime, provider) = runtime();
    let thinking = MockLlmClient::response(vec![
        ContentBlock::Thinking {
            thinking: "先查文件再决定".to_string().into_boxed_str(),
            signature: Some("sig-1".to_string().into_boxed_str()),
        },
        ContentBlock::ToolUse {
            id: "c1".to_string().into_boxed_str(),
            name: "probe.echo".to_string().into_boxed_str(),
            input_json: "{}".to_string().into_boxed_str(),
        },
    ]);
    let client = Arc::new(MockLlmClient::new([
        thinking,
        MockLlmClient::final_text("ok"),
    ]));
    let chain = LlmToolLoop::new(client.clone(), runtime, EventBus::new(), loop_config());

    chain
        .run(
            "想想再答",
            &provider.descriptors(),
            &CancellationToken::new(),
            &DenyWrites,
        )
        .await
        .expect("loop ok");

    let seen = client.seen_requests();
    let preserved: Vec<&ContentBlock> = seen[1]
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::Assistant)
        .flat_map(|m| m.blocks.iter())
        .filter(|block| matches!(block, ContentBlock::Thinking { .. }))
        .collect();
    assert_eq!(preserved.len(), 1, "Thinking 块必须逐字回传");
    match preserved[0] {
        ContentBlock::Thinking {
            thinking,
            signature,
        } => {
            assert_eq!(&**thinking, "先查文件再决定");
            assert_eq!(signature.as_ref().map(|s| &**s), Some("sig-1"));
        }
        _ => unreachable!("filtered to Thinking"),
    }
}

/// 未知工具：不炸循环，把错误当结果喂回模型
#[tokio::test]
async fn loop_unknown_tool_feeds_error_to_model() {
    let (runtime, provider) = runtime();
    let client = Arc::new(MockLlmClient::new([
        MockLlmClient::tool_use(&[("c1", "no.such.tool", "{}")]),
        MockLlmClient::final_text("recovered"),
    ]));
    let chain = LlmToolLoop::new(client.clone(), runtime, EventBus::new(), loop_config());

    let outcome = chain
        .run(
            "调个不存在的",
            &provider.descriptors(),
            &CancellationToken::new(),
            &DenyWrites,
        )
        .await
        .expect("未知工具不得打断循环");
    assert_eq!(outcome.text, "recovered");
    assert_eq!(
        provider.calls(),
        Vec::<String>::new(),
        "未知工具不可能被执行"
    );

    let results = fed_results(&client.seen_requests()[1]);
    assert_eq!(results.len(), 1);
    assert!(results[0].2, "未知工具必须以 is_error 回填");
}

// ============================================================
// T7.4 —— 审批
// ============================================================

/// 只读工具在默认档下自动放行
#[tokio::test]
async fn loop_readonly_tools_auto_approved() {
    let (runtime, provider) = runtime();
    let client = Arc::new(MockLlmClient::new([
        MockLlmClient::tool_use(&[("c1", "probe.echo", "{}")]),
        MockLlmClient::final_text("done"),
    ]));
    let chain = LlmToolLoop::new(client, runtime, EventBus::new(), loop_config());

    let outcome = chain
        .run(
            "读一下",
            &provider.descriptors(),
            &CancellationToken::new(),
            &DenyWrites,
        )
        .await
        .expect("loop ok");
    assert_eq!(provider.calls(), vec!["probe.echo".to_string()]);
    assert_eq!(outcome.denied_tool_calls, 0);
}

/// 写工具未获批 ⇒ **不执行**并回填 ask_required
#[tokio::test]
async fn loop_denied_write_is_not_executed() {
    let (runtime, provider) = runtime();
    let client = Arc::new(MockLlmClient::new([
        MockLlmClient::tool_use(&[("w1", "probe.write", r#"{"path":"x"}"#)]),
        MockLlmClient::final_text("told me"),
    ]));
    let chain = LlmToolLoop::new(client.clone(), runtime, EventBus::new(), loop_config());

    let outcome = chain
        .run(
            "写个文件",
            &provider.descriptors(),
            &CancellationToken::new(),
            &DenyWrites,
        )
        .await
        .expect("被拒不等于失败");
    assert_eq!(
        provider.calls(),
        Vec::<String>::new(),
        "未获批的写调用绝不能进 provider"
    );
    assert_eq!(outcome.denied_tool_calls, 1);
    assert_eq!(outcome.tool_calls, 1, "派发计数仍要如实记录");

    let results = fed_results(&client.seen_requests()[1]);
    assert_eq!(
        results,
        vec![("w1".to_string(), ASK_REQUIRED_MARKER.to_string(), true)]
    );
}

/// 审批口径来自工具表(而非 provider 自觉):同一 provider 内读写并存时按声明分档
#[tokio::test]
async fn loop_requires_approval_for_write_tools() {
    let (runtime, provider) = runtime();
    let client = Arc::new(MockLlmClient::new([
        MockLlmClient::tool_use(&[("r1", "probe.echo", "{}"), ("w1", "probe.write", "{}")]),
        MockLlmClient::final_text("mixed"),
    ]));
    let chain = LlmToolLoop::new(client, runtime, EventBus::new(), loop_config());

    let outcome = chain
        .run(
            "一读一写",
            &provider.descriptors(),
            &CancellationToken::new(),
            &DenyWrites,
        )
        .await
        .expect("loop ok");
    assert_eq!(
        provider.calls(),
        vec!["probe.echo".to_string()],
        "同批里只读执行、写不执行"
    );
    assert_eq!(outcome.denied_tool_calls, 1);
}

// ============================================================
// 第三波 B3 —— 弹窗档与 DenyWrites 的等价性 / R3 per-invocation
// ============================================================

/// 一轮「一读两写」的四条可对照口径（全部现取自同一趟执行，不事后重算）
#[derive(Debug, PartialEq, Eq)]
struct RunObs {
    /// provider 侧真实调用序（顺序即派发序，能同时抓「多执行」和「换顺序」）
    calls: Vec<String>,
    tool_calls: usize,
    denied: usize,
    /// 第二轮请求里被回填的 (call_id, content, is_error)，有序
    fed: Vec<(String, String, bool)>,
}

/// 用给定审批档跑一次「一读两写」。
///
/// 每次调用都新建 runtime/provider/client —— 两侧若共用计数面，等价性断言就成了
/// 自我印证（第一侧的调用会混进第二侧的 `calls`）。
async fn run_one_read_two_write(ask: &dyn ToolApproval) -> RunObs {
    let (runtime, provider) = runtime();
    let client = Arc::new(MockLlmClient::new([
        MockLlmClient::tool_use(&[
            ("r1", "probe.echo", "{}"),
            ("w1", "probe.write", r#"{"path":"a"}"#),
            ("w2", "probe.write", r#"{"path":"b"}"#),
        ]),
        MockLlmClient::final_text("收敛"),
    ]));
    let chain = LlmToolLoop::new(client.clone(), runtime, EventBus::new(), loop_config());

    let outcome = chain
        .run(
            "一读两写",
            &provider.descriptors(),
            &CancellationToken::new(),
            ask,
        )
        .await
        .expect("被拒不等于失败");
    RunObs {
        calls: provider.calls(),
        tool_calls: outcome.tool_calls,
        denied: outcome.denied_tool_calls,
        fed: fed_results(&client.seen_requests()[1]),
    }
}

/// 起一个「按表应答」的弹窗接收端：`verdicts` 未列出的 call_id 一律答 No。
///
/// 返回闸门与任务句柄；任务在闸门的全部克隆 drop 后退出，并把**它实际被问到的
/// call_id 序列**作为返回值交出 —— 这条序列就是「审批粒度」的直接证据，
/// 不必再去读循环内部实现。
fn popup_verdicts(
    verdicts: &[(&'static str, bool)],
) -> (ApprovalGate, tokio::task::JoinHandle<Vec<String>>) {
    // 显式 `::<ApprovalTicket>`：`unbounded_channel` 的 `T` 只靠函数返回类型反推，
    // 在 `let` 点尚未定型（rustc 会就地报 E0282 而非回溯别名）。
    let (gate, mut rx) = tokio::sync::mpsc::unbounded_channel::<ApprovalTicket>();
    let verdicts = verdicts.to_vec();
    let task = tokio::spawn(async move {
        let mut asked = Vec::new();
        while let Some((request, reply)) = rx.recv().await {
            let allowed = verdicts
                .iter()
                .find(|(id, _)| *id == request.call_id.as_str())
                .is_some_and(|(_, verdict)| *verdict);
            asked.push(request.call_id);
            let _ = reply.send(allowed);
        }
        asked
    });
    (gate, task)
}

/// 计划 B3 硬判据：`gate=None`（`DenyWrites`）与「每条都弹窗、人人答 No」
/// （`TuiApproval`）必须逐位等价。
///
/// WHY 这是 `gate=None` 回落路径的反向证明：编排器里 `None ⇒ DenyWrites`、
/// `Some ⇒ TuiApproval`。若「全拒的弹窗」与「不弹的直拒」在这四条口径上不等，
/// 说明接了 TUI 的进程多做（或少做）了某件本不该做的事 —— 那正是本批唯一的
/// 行为改点破裂的位置，不是测试写松了。
#[tokio::test]
async fn popup_answered_no_is_identical_to_deny_writes() {
    let without = run_one_read_two_write(&DenyWrites).await;

    let (gate, answered) = popup_verdicts(&[]);
    let asker = TuiApproval::new(gate.clone());
    let with = run_one_read_two_write(&asker).await;
    // 先交出全部克隆，接收端才会退出；否则 await 会永久等待
    drop(gate);
    drop(asker);
    let asked = answered.await.expect("接收端不 panic");

    assert_eq!(
        asked,
        vec!["w1".to_string(), "w2".to_string()],
        "弹窗只该问写工具，且两条各问一次"
    );
    assert_eq!(
        with, without,
        "闸门档全拒时必须与不接闸门的现状逐位相同（不多放行、不改回填序）"
    );
}

/// 拒绝项 R3（ADR-232 D6）：一次 Yes 不得为后续写调用背书。
///
/// 这条同时是本仓第一例「获批写调用真的穿过循环进 provider」的测试 ——
/// 此前审批面只测过拒绝侧，执行侧的 `approved=true` 通路无人走过。
#[tokio::test]
async fn one_yes_does_not_endorse_later_writes() {
    let (gate, answered) = popup_verdicts(&[("w1", true)]);
    let asker = TuiApproval::new(gate.clone());
    let obs = run_one_read_two_write(&asker).await;
    drop(gate);
    drop(asker);
    let asked = answered.await.expect("接收端不 panic");

    assert_eq!(
        asked,
        vec!["w1".to_string(), "w2".to_string()],
        "同批两条写必须各问一次；只问一条即为批量背书（R3）"
    );
    assert_eq!(
        obs.calls,
        vec!["probe.echo".to_string(), "probe.write".to_string()],
        "获批的写执行、未获批的写绝不进 provider"
    );
    assert_eq!(obs.tool_calls, 3, "三条都算派发");
    assert_eq!(obs.denied, 1, "只有 w2 记拒");
    assert_eq!(obs.fed.len(), 3, "三条都要有回执，一条都不能静默丢");
    assert_eq!(obs.fed[1].0, "w1");
    assert!(!obs.fed[1].2, "获批执行的调用不得以 is_error 回填");
    assert_eq!(
        obs.fed[2],
        ("w2".to_string(), ASK_REQUIRED_MARKER.to_string(), true),
        "未获批的 w2 按 ask_required 回填，且位置仍在 w1 之后"
    );
}

// ============================================================
// T7.5 —— 预算与取消
// ============================================================

/// token 预算耗尽 ⇒ 发布既有 `BudgetExceeded` 并 `stop=Budget`
#[tokio::test]
async fn loop_budget_exceeded_publishes_event() {
    let (runtime, provider) = runtime();
    // 每轮 usage 固定 10+5=15；上限 20 ⇒ 第 2 轮判超
    let client = Arc::new(MockLlmClient::new([
        MockLlmClient::tool_use(&[("c1", "probe.echo", "{}")]),
        MockLlmClient::tool_use(&[("c2", "probe.echo", r#"{"i":2}"#)]),
    ]));
    let bus = EventBus::new();
    // §4.4 红线 3:先 subscribe 再驱动会产生事件的路径
    let mut rx = bus.subscribe();
    let mut cfg = loop_config();
    cfg.max_total_tokens = 20;
    cfg.max_repeat_tool_calls = 99;
    let chain = LlmToolLoop::new(client, runtime, bus.clone(), cfg);

    let outcome = chain
        .run(
            "烧预算",
            &provider.descriptors(),
            &CancellationToken::new(),
            &DenyWrites,
        )
        .await
        .expect("loop ok");
    assert_eq!(outcome.stop, chimera_cli::llm::LlmLoopStop::Budget);
    assert_eq!(outcome.turns, 2, "第二轮请求后判超");

    let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("预算事件必须送达")
        .expect("recv ok");
    match event {
        NexusEvent::BudgetExceeded {
            budget_type,
            current,
            limit,
            ..
        } => {
            assert_eq!(budget_type, "llm_total_tokens");
            assert_eq!(limit, 20);
            assert!(current > 20, "current={current} 必须已越限");
        }
        other => panic!("期望 BudgetExceeded,实际 {other:?}"),
    }
}

/// 在飞补全期间取消 ⇒ `Cancelled{turns, tool_calls}`
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loop_cancels_mid_flight() {
    struct Hanging;
    #[async_trait]
    impl LlmClient for Hanging {
        fn provider_id(&self) -> &str {
            "hanging"
        }
        async fn complete(
            &self,
            _request: &nexus_contracts::affinity::AffinityRequest,
        ) -> Result<AffinityResponse, LlmError> {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Err(LlmError::Protocol("不该到这里".to_string()))
        }
    }

    let (runtime, provider) = runtime();
    let chain = LlmToolLoop::new(Arc::new(Hanging), runtime, EventBus::new(), loop_config());
    let cancel = CancellationToken::new();
    let firer = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        firer.cancel();
    });

    let started = std::time::Instant::now();
    let result = chain
        .run("永远等下去", &provider.descriptors(), &cancel, &DenyWrites)
        .await;
    assert!(
        started.elapsed() < Duration::from_millis(2_000),
        "取消必须立刻打断,不得等 30s"
    );
    let err = result.expect_err("取消必须报错而非假装成功");
    assert!(err.is_cancelled(), "实际: {err}");
}

/// 在飞工具被取消丢弃 ⇒ 不返回 outcome,错误里带已派发计数
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loop_cancel_drops_inflight_tools() {
    let (runtime, provider) = runtime();
    let client = Arc::new(MockLlmClient::new([
        MockLlmClient::tool_use(&[("s1", "probe.slow", "{}")]),
        MockLlmClient::final_text("never"),
    ]));
    let chain = LlmToolLoop::new(client, runtime, EventBus::new(), loop_config());
    let cancel = CancellationToken::new();
    let firer = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(60)).await;
        firer.cancel();
    });

    let started = std::time::Instant::now();
    let result = chain
        .run("慢工具", &provider.descriptors(), &cancel, &DenyWrites)
        .await;
    let err = result.expect_err("在飞工具应被丢弃");
    assert!(err.is_cancelled(), "实际: {err}");
    assert!(
        started.elapsed() < Duration::from_millis(2_000),
        "5s 的在飞工具必须被丢弃,实测 {:?}",
        started.elapsed()
    );
    assert_eq!(provider.calls(), vec!["probe.slow".to_string()]);
}

// ============================================================
// T7.6 —— McaLlmClient 装配（仅 `--features mca` 下编译;只装配,不发网络）
// ============================================================

/// 默认配置里有 provider ⇒ 客户端可装配（不发起任何 HTTP）
#[cfg(feature = "mca")]
#[test]
fn mca_client_assembles_from_config() {
    use chimera_cli::config::ChimeraConfig;
    use chimera_cli::llm::mca_client::McaLlmClient;

    let config = ChimeraConfig::default();
    assert!(
        !config.model_router.providers.is_empty(),
        "前置:默认配置应含 provider(否则本用例证明不了装配路径)"
    );
    let client = McaLlmClient::from_config(&config).expect("默认配置应可装配");
    let label = client.provider_id();
    assert!(label.starts_with("mca:"), "标签应标明来源通道,实际 {label}");
}

/// 无 provider ⇒ `ProviderUnavailable`（CLI 映射为退出码 1 的配置错误）
#[cfg(feature = "mca")]
#[test]
fn mca_client_missing_provider_is_unavailable() {
    use chimera_cli::config::ChimeraConfig;
    use chimera_cli::llm::client::LlmError;
    use chimera_cli::llm::mca_client::McaLlmClient;

    let mut config = ChimeraConfig::default();
    config.model_router.providers.clear();
    let error = McaLlmClient::from_config(&config)
        .err()
        .expect("空 provider 必须报错");
    assert!(
        matches!(error, LlmError::ProviderUnavailable(_)),
        "实际: {error}"
    );
}

/// provider 名 → ProviderId 映射（含未知名落 Custom，不伪造厂商身份）
#[cfg(feature = "mca")]
#[test]
fn mca_provider_name_mapping_is_conservative() {
    use chimera_cli::llm::mca_client::provider_id_from_name;
    use nexus_contracts::affinity::ProviderId;

    assert_eq!(provider_id_from_name("DeepSeek"), ProviderId::DeepSeek);
    assert_eq!(provider_id_from_name("kimi"), ProviderId::Moonshot);
    assert_eq!(provider_id_from_name("qwen"), ProviderId::AlibabaCloud);
    // 未知厂商必须留在 Custom，不能冒充已知通道（路由留痕会失真）
    match provider_id_from_name("some-local-vllm") {
        ProviderId::Custom(name) => assert_eq!(&*name, "some-local-vllm"),
        other => panic!("未知名被误判为已知通道: {other:?}"),
    }
}
