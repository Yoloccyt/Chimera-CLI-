//! LLM 工具循环延迟基线（Phase 7 / T7.8，ADR-232，dev-only）。
//!
//! WHY dev-only 而非四表阈值：本文件建立的是**趋势基线**——首跑无对照值即定阈值
//! 等于把噪声钉成规格。组名与 `scripts/bench_inventory_freeze.txt` 的 dev-only 行
//! 同批登记；将来升格受门时删该行。
//!
//! 三件事分开量，否则读数无法归因：
//! 1. `llm_tool_decls/<n>` —— 纯声明映射，O(tools) 的确定性成本，不含调度；
//! 2. `llm_loop/turn2/<n>` —— 同一循环形状、只改声明规模，差值即"注册表变大"每轮的边际成本；
//! 3. `llm_loop_16_tools_window/window8` —— 一轮打满并行窗口，量窗口与派发路径本身。
//!
//! 零网络：客户端脚本化（偶数轮发 ToolUse、奇数轮回文本），工具面是进程内探针
//! provider。tokio runtime 用 current-thread 且**跨 iter 复用**，避免宿主核数把并行
//! 段量成墙钟噪声——并行**正确性**（峰值在飞 ≤ 窗口）由 `tests/llm_tool_loop.rs`
//! 的结构断言负责，bench 只管延迟。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

use async_trait::async_trait;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use event_bus::EventBus;
use gqep_executor::tool_plane::{
    BoundedToolScheduler, ToolDescriptor, ToolError, ToolInvocation, ToolOrigin, ToolPriority,
    ToolProvider, ToolRegistry, ToolRuntime, ToolRuntimeConfig, ToolScheduler,
};
use nexus_contracts::affinity::{
    AffinityMessage, AffinityOverrides, AffinityRequest, AffinityResponse, ContentBlock,
    MessageRole, OutputFormat, SamplingParams,
};
use nexus_contracts::tool_plan::SideEffectDecl;
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

use chimera_cli::llm::client::{LlmClient, LlmError};
use chimera_cli::llm::tool_loop::{LlmToolLoop, LlmToolLoopConfig};
use chimera_cli::llm::{tool_decls, DenyWrites, LlmLoopOutcome, LlmLoopStop, MockLlmClient};

/// 探针 provider —— `bench.t0..t{N-1}` 全部只读，耗时只让出一次调度
#[derive(Clone)]
struct BenchProvider {
    tool_count: usize,
    invokes: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolProvider for BenchProvider {
    fn provider_id(&self) -> &str {
        "bench"
    }

    fn descriptors(&self) -> Vec<ToolDescriptor> {
        (0..self.tool_count)
            .map(|i| ToolDescriptor {
                name: format!("bench.t{i}"),
                provider: "bench".to_string(),
                description: format!("bench tool {i}"),
                side_effect: SideEffectDecl::ReadOnly,
                timeout_ms: 30_000,
                max_concurrency: 32,
                input_schema: Some(r#"{"type":"object"}"#.to_string()),
            })
            .collect()
    }

    async fn invoke(
        &self,
        inv: &ToolInvocation,
        _cancel: CancellationToken,
    ) -> Result<String, ToolError> {
        self.invokes.fetch_add(1, Ordering::Relaxed);
        // WHY yield_now 而不是 sleep(ZERO)：Windows 上 tokio 的 park 唤醒按系统时钟粒度
        // 量化（≈15.6ms = 1/64s）。实测对照见本文件 `llm_diag_provider_invoke_only`：
        // 用 timer 让出时单次调用读数 15.5ms，换成 yield_now 后掉到 µs 级——
        // 那 15ms 是宿主时钟粒度，不是执行面成本。要量"真让出一次调度"用 yield_now。
        tokio::task::yield_now().await;
        Ok(format!("{}:{}", inv.tool, inv.args_json))
    }
}

/// 脚本化客户端 —— 不复用 `MockLlmClient` 的请求留存。
///
/// WHY 不直接上 MockLlmClient：它每轮 `clone()` 整份请求进 `seen`，那是**测试替身的
/// 记账成本**而非生产路径成本；量进基线会让未来的优化被一个不存在的开销掩盖。
/// 奇偶交替（一轮工具、一轮收敛）使同一实例可跨 iter 复用 ⇒ 构造成本不进计时区。
struct ScriptedClient {
    calls: AtomicUsize,
    per_turn: usize,
}

#[async_trait]
impl LlmClient for ScriptedClient {
    fn provider_id(&self) -> &str {
        "bench-scripted"
    }

    async fn complete(&self, _request: &AffinityRequest) -> Result<AffinityResponse, LlmError> {
        let turn = self.calls.fetch_add(1, Ordering::Relaxed);
        if turn % 2 == 1 {
            return Ok(MockLlmClient::final_text("done"));
        }
        // 相同 tool+args 会触发重复调用守卫 ⇒ args 带轮号，保证每轮唯一
        let seq = turn / 2;
        let calls: Vec<(String, String, String)> = (0..self.per_turn)
            .map(|i| {
                (
                    format!("c{seq}-{i}"),
                    format!("bench.t{}", i % 16),
                    format!(r#"{{"q":{seq},"i":{i}}}"#),
                )
            })
            .collect();
        let refs: Vec<(&str, &str, &str)> = calls
            .iter()
            .map(|(id, name, args)| (id.as_str(), name.as_str(), args.as_str()))
            .collect();
        Ok(MockLlmClient::tool_use(&refs))
    }
}

/// 一次基准的共享夹具（构造成本全部留在计时区外）
struct Harness {
    rt: Runtime,
    loop_runtime: Arc<ToolRuntime>,
    descriptors: Vec<ToolDescriptor>,
    invokes: Arc<AtomicUsize>,
    client: Arc<ScriptedClient>,
    /// 零工具调用的对照客户端（用来把"循环固定成本"与"派发成本"分开）
    client_min: Arc<ScriptedClient>,
    per_turn: usize,
    /// WHY 夹具级共享：生产里 bus 是组合根持有的那一条（`llm_answer_with` 收 `&EventBus`），
    /// 每轮新建既不符合真实路径，也会把固定成本混进循环读数（首轮实测 15ms/轮 的元凶之一）。
    bus: EventBus,
}

impl Harness {
    fn new(tool_count: usize, per_turn: usize) -> Self {
        Self::with_threads(tool_count, per_turn, None)
    }

    /// `threads = Some(n)` 建 multi-thread 运行时（生产 `chimera run/chat/tui` 用的就是它），
    /// `None` 建 current-thread。用于区分"真实成本"与"单线程夹具假象"。
    fn with_threads(tool_count: usize, per_turn: usize, threads: Option<usize>) -> Self {
        let invokes = Arc::new(AtomicUsize::new(0));
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(BenchProvider {
                tool_count,
                invokes: Arc::clone(&invokes),
            }))
            .expect("register bench provider");
        let loop_runtime = Arc::new(ToolRuntime::new(
            Arc::new(registry),
            None,
            Arc::new(BoundedToolScheduler::new(32, 32, 256, 32)),
            ToolRuntimeConfig {
                interactive_inflight: 32,
                background_inflight: 32,
                max_waiters: 256,
                max_provider_inflight: 32,
                default_timeout_ms: 30_000,
                max_payload_bytes: 4_096,
                max_batch: 64,
            },
        ));
        let descriptors = loop_runtime.registry().descriptors();
        assert_eq!(
            descriptors.len(),
            tool_count,
            "探针应产出 {tool_count} 个描述符"
        );
        let rt = match threads {
            Some(n) => tokio::runtime::Builder::new_multi_thread()
                .worker_threads(n)
                .enable_all()
                .build()
                .expect("multi-thread runtime"),
            None => tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("current-thread runtime"),
        };
        Self {
            rt,
            loop_runtime,
            descriptors,
            invokes,
            client: Arc::new(ScriptedClient {
                calls: AtomicUsize::new(0),
                per_turn,
            }),
            client_min: Arc::new(ScriptedClient {
                calls: AtomicUsize::new(0),
                per_turn: 0,
            }),
            per_turn,
            bus: EventBus::new(),
        }
    }

    /// 跑一轮完整循环（1 轮工具 + 1 轮收敛），并校验它**真的**执行了工具
    fn run_cycle(&self, max_parallel_tools: usize) -> usize {
        let before = self.invokes.load(Ordering::Relaxed);
        let outcome = self.run(&self.client, max_parallel_tools);
        // 形状断言留在计时区内：脚本错位（工具没被执行）会让基线静默变成"空转很快"
        assert_eq!(
            outcome.turns, 2,
            "应为 1 轮工具 + 1 轮收敛,实际 {}",
            outcome.turns
        );
        assert_eq!(outcome.tool_calls, self.per_turn, "调用数应与脚本一致");
        self.invokes.load(Ordering::Relaxed) - before
    }

    /// 零工具调用的一轮（固定成本下限：只走声明映射 + 一轮补全 + 收敛）
    fn run_minimal(&self, max_parallel_tools: usize) -> usize {
        let outcome = self.run(&self.client_min, max_parallel_tools);
        assert_eq!(outcome.turns, 1, "无 ToolUse 应单轮收敛");
        assert_eq!(outcome.tool_calls, 0);
        0
    }

    fn run(&self, client: &Arc<ScriptedClient>, max_parallel_tools: usize) -> LlmLoopOutcome {
        let chain = LlmToolLoop::new(
            Arc::clone(client) as Arc<dyn LlmClient>,
            Arc::clone(&self.loop_runtime),
            self.bus.clone(),
            loop_config(max_parallel_tools),
        );
        let outcome = self
            .rt
            .block_on(chain.run(
                "bench",
                &self.descriptors,
                &CancellationToken::new(),
                &DenyWrites,
            ))
            .expect("循环应成功收敛");
        assert_eq!(
            outcome.stop,
            LlmLoopStop::Final,
            "两种脚本都应由 Final 收敛"
        );
        outcome
    }
}

/// 构造一次基准用调用（tool 名循环取 `bench.t{0..15}`，args 唯一避开重复守卫）
fn inv(i: usize) -> ToolInvocation {
    ToolInvocation {
        call_id: format!("b{i}"),
        tool: format!("bench.t{}", i % 16),
        args_json: "{}".to_string(),
        timeout_ms: None,
        priority: ToolPriority::Normal,
        origin: ToolOrigin::Llm,
        quest_id: None,
        task_id: None,
        approved: true,
    }
}

fn loop_config(max_parallel_tools: usize) -> LlmToolLoopConfig {
    LlmToolLoopConfig {
        max_turns: 8,
        max_tool_calls: 32,
        max_parallel_tools,
        max_repeat_tool_calls: 3,
        max_total_tokens: u64::MAX,
        system_prompt: "bench".to_string(),
        ..LlmToolLoopConfig::default()
    }
}

/// ① 声明映射本身（无 runtime、无循环）
fn bench_tool_decls(c: &mut Criterion) {
    for tool_count in [8usize, 16, 32] {
        let harness = Harness::new(tool_count, 2);
        let descriptors = &harness.descriptors;
        // 名字全部平铺可 grep（llm_tool_decls_16 等），与报告/台账一一对应
        c.bench_function(&format!("llm_tool_decls_{tool_count}"), |b| {
            b.iter(|| black_box(tool_decls(black_box(descriptors))).len())
        });
    }
}

/// ② 完整循环：只改声明规模，差值即可归因到"每轮声明/请求体变大"
fn bench_llm_loop(c: &mut Criterion) {
    for tool_count in [8usize, 16] {
        let harness = Harness::new(tool_count, 2);
        // 自检：先跑一次确认工具真的被执行，否则计时区在量一条不存在的路径
        assert_eq!(harness.run_cycle(8), 2, "前置自检:探针应执行 2 次工具调用");
        c.bench_function(&format!("llm_loop_{tool_count}_tools"), |b| {
            b.iter(|| black_box(harness.run_cycle(8)))
        });
    }
}

/// ③ 打满并行窗口：同一批工具数，一轮发 8 个调用
fn bench_llm_loop_window(c: &mut Criterion) {
    let harness = Harness::new(16, 8);
    assert_eq!(
        harness.run_cycle(8),
        8,
        "前置自检:窗口打满应执行 8 次工具调用"
    );
    c.bench_function("llm_loop_16_tools_window8", |b| {
        b.iter(|| black_box(harness.run_cycle(8)))
    });
}

/// ④ 固定成本下限：同一夹具，但脚本不发工具调用（单轮 Final）
///
/// WHY 这一条：`llm_loop_*` 与它的差值才是"派发路径"的真实成本。首跑实测
/// 每轮 15ms 且与工具数无关 ⇒ 固定成本压过一切，必须先把它显式化，
/// 否则基线只是在量夹具自己。
fn bench_llm_loop_floor(c: &mut Criterion) {
    let harness = Harness::new(16, 2);
    assert_eq!(harness.run_minimal(8), 0, "前置自检:下限用例不应执行工具");
    c.bench_function("llm_loop_16_tools_notools", |b| {
        b.iter(|| black_box(harness.run_minimal(8)))
    });
}

/// ⑤ 诊断：`EventBus::new()` 自身成本（判断"每轮新建总线"是否是固定成本元凶）
fn bench_bus_new(c: &mut Criterion) {
    c.bench_function("llm_diag_event_bus_new", |b| {
        b.iter(|| black_box(EventBus::new()))
    });
}

/// ⑥ 诊断：直接量 `ToolRuntime::invoke_batch`，不经过 LLM 循环。
///
/// WHY：④ 显示"循环下限"仅 4µs，而 ② 每轮 15ms 且与调用数无关 ⇒ 成本在派发路径里，
/// 且是**每批一次**的固定量。本例把它钉到 `invoke_batch` 本身，下一步再往里剥。
fn bench_invoke_batch_direct(c: &mut Criterion) {
    let harness = Harness::new(16, 2);
    for n in [1usize, 2, 8] {
        let runtime = Arc::clone(&harness.loop_runtime);
        c.bench_function(&format!("llm_diag_invoke_batch_{n}"), |b| {
            b.iter(|| {
                black_box(harness.rt.block_on(
                    runtime.invoke_batch((0..n).map(inv).collect(), &CancellationToken::new()),
                ))
            })
        });
    }
}

/// ⑦ 诊断：同一 `invoke_batch(1)` 放到 multi-thread 运行时上（生产口径）
///
/// WHY：⑥ 的 15ms 若只在 current-thread 出现，则是夹具假象；若在两种 flavor 上都出现，
/// 就是真实缺陷，必须修而不是解释过去。
fn bench_invoke_batch_mt(c: &mut Criterion) {
    let harness = Harness::with_threads(16, 2, Some(2));
    let runtime = Arc::clone(&harness.loop_runtime);
    c.bench_function("llm_diag_invoke_batch_1_mt", |b| {
        b.iter(|| {
            black_box(
                harness
                    .rt
                    .block_on(runtime.invoke_batch(vec![inv(0)], &CancellationToken::new())),
            )
        })
    });
}

/// ⑧ 诊断：把 15ms 剥到最小单元 —— 单独量 scheduler.acquire 与 provider.invoke
fn bench_sched_and_provider(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime");
    let invokes = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(BenchProvider {
        tool_count: 16,
        invokes: Arc::clone(&invokes),
    });
    let descriptor = provider.descriptors().remove(0);
    let scheduler = BoundedToolScheduler::new(32, 32, 256, 32);

    c.bench_function("llm_diag_scheduler_acquire", |b| {
        b.iter(|| {
            black_box(rt.block_on(async {
                let permit = scheduler
                    .acquire(&inv(0), &descriptor, &CancellationToken::new())
                    .await;
                permit.is_ok()
            }))
        })
    });
    c.bench_function("llm_diag_provider_invoke_only", |b| {
        b.iter(|| {
            black_box(
                rt.block_on(provider.invoke(&inv(0), CancellationToken::new()))
                    .is_ok(),
            )
        })
    });
    // A/B 的另一半：同一次让出改用 timer ⇒ 量到的是宿主时钟粒度（Windows ≈15.6ms）
    c.bench_function("llm_diag_host_timer_granularity", |b| {
        b.iter(|| {
            black_box(rt.block_on(async {
                let start = tokio::time::Instant::now();
                tokio::time::sleep(Duration::ZERO).await;
                start.elapsed().as_nanos()
            }))
        })
    });
    // token 自身（cancel.cancelled() 在 select! 里每层都出现一次，先排除它）
    // WHY 先 cancel 再 await：未取消的 token 上 `cancelled()` 永不完成——上一轮这条
    // 诊断把 criterion 的 warm-up 直接挂死（15ms 的 host timer 那条能跑完，这条不能），
    // 是夹具缺陷而非执行面缺陷。
    c.bench_function("llm_diag_cancel_token_only", |b| {
        b.iter(|| {
            black_box(rt.block_on(async {
                let token = CancellationToken::new();
                let child = token.child_token();
                child.cancel();
                child.cancelled().await;
                (child.is_cancelled(), token.is_cancelled())
            }))
        })
    });
}

/// ⑨ 诊断：单次 `registry.resolve` —— 每调用堆分配的比较器。
///
/// WHY 单独量这一段：`ToolRuntime::invoke` 的热路径上，resolve 是唯一每次都要把
/// 整个描述符（`name`/`provider`/`description`/`input_schema` 四个 String）clone
/// 一遍的地方。
/// WHY 用 `.map(|(_, d)| d.name.len())` 消费：`Arc<ToolDescriptor>` 与按值的
/// `ToolDescriptor` 都能 `.name` 取到同一字段，所以这一组在"返回克隆"与"返回 Arc"
/// 两种实现下都编译得起来 —— 比较器必须比它所衡量的改动更稳定，否则改完就没法对照。
fn bench_resolve_only(c: &mut Criterion) {
    let provider = Arc::new(BenchProvider {
        tool_count: 16,
        invokes: Arc::new(AtomicUsize::new(0)),
    });
    let mut registry = ToolRegistry::new();
    registry
        .register(provider)
        .expect("bench provider registers");

    c.bench_function("llm_diag_resolve_hit", |b| {
        b.iter(|| {
            black_box(
                registry
                    .resolve("bench.t0")
                    .map(|(_, desc)| desc.name.len()),
            )
        })
    });
    c.bench_function("llm_diag_resolve_miss", |b| {
        b.iter(|| black_box(registry.resolve("bench.absent").is_some()))
    });
}

/// 历史夹具：`len` 条消息，system + user 起头，其余按 assistant/tool 成对填。
///
/// WHY 成对：真实循环每轮追加的正是这两条（`tool_loop.rs` 把 assistant 与 tool
/// 结果各 push 一次），历史长度的奇偶会影响 clone 的块构成。
fn synthetic_history(len: usize) -> Vec<AffinityMessage> {
    let text = |s: &str| ContentBlock::Text {
        text: s.to_string().into_boxed_str(),
    };
    let mut out = Vec::with_capacity(len.max(2));
    out.push(AffinityMessage {
        role: MessageRole::System,
        blocks: vec![text("system prompt for the benchmarked tool loop")],
    });
    out.push(AffinityMessage {
        role: MessageRole::User,
        blocks: vec![text("do the thing with the tools")],
    });
    while out.len() < len {
        out.push(AffinityMessage {
            role: MessageRole::Assistant,
            blocks: vec![
                text("reasoning out loud, then a call"),
                ContentBlock::ToolUse {
                    id: "call-1".to_string().into_boxed_str(),
                    name: "bench.t0".to_string().into_boxed_str(),
                    input_json: "{}".to_string().into_boxed_str(),
                },
            ],
        });
        if out.len() >= len {
            break;
        }
        out.push(AffinityMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "call-1".to_string().into_boxed_str(),
                content: "{\"ok\":true}".to_string().into_boxed_str(),
                is_error: false,
            }],
        });
    }
    out.truncate(len);
    out
}

/// ⑩ 诊断：每轮 `AffinityRequest` 重建 = 整段历史 clone + 整张声明表 clone。
///
/// WHY 单独量这一段：turn 循环每轮都新建一个 request，而 `client.complete` 收的是
/// `&AffinityRequest`（借用，不是成本源）——所以循环自身的每轮固定成本几乎全在这
/// 两次 clone 上。第一波实测它比整个 L7 批处理（`llm_diag_invoke_batch_8`）还贵，
/// 故 W1b-2 的判据必须能把"这一段"单独读出来，而不是混在 loop 总读数里归不了因。
fn bench_request_rebuild(c: &mut Criterion) {
    let provider = BenchProvider {
        tool_count: 16,
        invokes: Arc::new(AtomicUsize::new(0)),
    };
    let decls = tool_decls(&provider.descriptors());
    let cfg = loop_config(8);
    let mut group = c.benchmark_group("llm_diag_request_rebuild");
    // 2 = 首轮（system+user），10 ≈ 第 4 轮，26 ≈ 第 12 轮 —— 覆盖 `max_turns` 常见区间
    for history in [2usize, 10, 26] {
        let messages = synthetic_history(history);
        let decls = decls.clone();
        group.bench_function(format!("history_{history}"), move |b| {
            b.iter(|| {
                black_box(AffinityRequest {
                    intent_id: "llm-tool-loop".to_string().into_boxed_str(),
                    messages: messages.clone(),
                    tools: decls.clone(),
                    thinking_pref: cfg.thinking_pref,
                    budget_hint_micro: cfg.budget_hint_micro,
                    overrides: AffinityOverrides::default(),
                    sampling: SamplingParams::default(),
                    output_format: OutputFormat::default(),
                })
                .messages
                .len()
            })
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_tool_decls,
    bench_llm_loop,
    bench_llm_loop_window,
    bench_llm_loop_floor,
    bench_bus_new,
    bench_invoke_batch_direct,
    bench_invoke_batch_mt,
    bench_sched_and_provider,
    bench_resolve_only,
    bench_request_rebuild
);
criterion_main!(benches);
