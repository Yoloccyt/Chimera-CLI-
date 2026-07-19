//! CHIMERA-MAS 多 Agent 协同子系统性能基准测试
//!
//! 对应任务:Task 16(SubTask 16.1 - 16.5)
//! 架构层归属:L9 Quest(chimera-mas 性能验证)
//!
//! # 基准场景
//!
//! - **SubTask 16.1**:Agent 创建/销毁延迟(5 种 AgentType)
//! - **SubTask 16.2**:消息路由延迟(NexusEvent::AgentTaskDelegated 发布/订阅)
//! - **SubTask 16.3**:任务拆分延迟(RootOrchestrator::delegate × 4 种 TaskComplexity)
//! - **SubTask 16.4**:上下文构建延迟(AgentContext::build_prompt × 4 种 token 规模)
//!
//! # 性能可证伪(§3.4.1 第 6 条)
//!
//! 所有 benchmark 使用 `criterion::black_box` 防止编译器优化,
//! 确保测量真实性能。基线数据用于后续优化对比(P0-P4 优先级评估,§3.4.4)。
//!
//! # 运行
//!
//! ```bash
//! cargo bench -p chimera-mas                    # 全量运行(较慢)
//! cargo bench -p chimera-mas -- --quick         # 快速验证
//! cargo bench -p chimera-mas -- agent_creation  # 单个 benchmark
//! ```

#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use chrono::Utc;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use event_bus::{EventBus, EventMetadata, EventTopic, NexusEvent, TaskPriority};
use nexus_core::{Task, TaskStatus};

use chimera_mas::{
    AgentContext, AgentFactory, AgentTask, AgentType, ContextBlock, ContextPriority, QualityLevel,
    RootOrchestrator, TaskComplexity,
};

// ============================================================
// 全局唯一 ID 计数器
// ============================================================

/// 全局原子计数器 — 生成唯一 ID,避免 create_agent / delegate 因 ID 重复报错
///
/// WHY 全局 static:benchmark 函数间共享计数器,确保所有 agent_id / task_id 跨
/// benchmark 唯一。criterion 的 `iter_batched` 每次 setup 调用 `next_bench_id`
/// 递增计数器,保证同一 AgentFactory / RootOrchestrator 内 ID 不冲突。
static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 生成唯一 benchmark ID(线程安全)
fn next_bench_id(prefix: &str) -> String {
    let n = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{n}")
}

// ============================================================
// 辅助构造函数
// ============================================================

/// 构造 AgentTaskDelegated 事件(用于消息路由 benchmark)
///
/// 字段对齐 `AgentFactory::create_agent` 内部发布的事件结构,
/// 确保 FilteredSubscriber 按 `EventTopic::Agent` 能正确接收。
fn make_delegated_event(task_id: &str) -> NexusEvent {
    NexusEvent::AgentTaskDelegated {
        metadata: EventMetadata::new("chimera-mas:benchmark"),
        from: "bench-source".to_string(),
        to: "bench-target".to_string(),
        task_id: task_id.to_string(),
        // 用 seconds(3600) 而非 hours(1):chrono 0.4.35+ 中 Duration::hours 已 deprecated
        deadline: Utc::now() + chrono::Duration::seconds(3600),
        priority: TaskPriority::Medium,
    }
}

/// 构造 AgentTask(用于任务拆分 benchmark)
///
/// `delegation_depth=0`(RootOrchestrator 直接发起),`parent_agent_id=None`,
/// 与 spec 示例一致,确保 delegate 深度检查通过。
fn make_agent_task(complexity: TaskComplexity, task_id: String) -> AgentTask {
    let task = Task {
        task_id,
        description: "benchmark 任务".to_string(),
        status: TaskStatus::Pending,
        dependencies: vec![],
    };
    AgentTask::new(
        task,
        complexity,
        1000,
        Duration::from_secs(60),
        QualityLevel::Standard,
    )
}

/// 构造 5 种 AgentType 列表(用于 Agent 创建 benchmark)
///
/// 覆盖 ADR-026 决策 1 的全部层级类型:
/// RootOrchestrator(depth=0) / MainAgent(1) / SubAgent(2) / GrandAgent(3) / ExpertAgent(0,咨询)
fn make_agent_types() -> Vec<(&'static str, AgentType)> {
    vec![
        ("root_orchestrator", AgentType::RootOrchestrator),
        (
            "main_agent",
            AgentType::MainAgent {
                domain: "benchmark".into(),
            },
        ),
        (
            "sub_agent",
            AgentType::SubAgent {
                parent_id: "bench-parent".into(),
                task_scope: "bench-scope".into(),
            },
        ),
        (
            "grand_agent",
            AgentType::GrandAgent {
                parent_id: "bench-parent".into(),
                task_scope: "bench-scope".into(),
            },
        ),
        (
            "expert_agent",
            AgentType::ExpertAgent {
                specialty: vec!["benchmark".into()],
            },
        ),
    ]
}

/// 构造指定总 token 数的 AgentContext(用于上下文构建 benchmark)
///
/// 块布局:1 个 Critical(system_prompt) + N 个 Normal(task_context),
/// 每块 content 长度 ≈ tokens * 4 bytes(1 token ≈ 4 chars,模拟真实文本)。
/// 多块设计确保 OSA compute_all_masks 有多个 file_id 可稀疏化,
/// 避免单块场景稀疏化无意义。
///
/// 块数与单块 tokens 按总规模自适应:
/// - 小规模(≤8K):每块 1024 tokens(4K → 4 块)
/// - 中规模(≤64K):每块 4096 tokens(32K → 8 块)
/// - 大规模(>64K):每块 8192 tokens(128K → 16 块,1M → 128 块)
fn make_context(total_tokens: usize) -> AgentContext {
    let bus = EventBus::new();
    let agent_id = next_bench_id("ctx-agent");
    let mut ctx = AgentContext::new(agent_id, 1_048_576, bus).expect("AgentContext 创建成功");

    // Critical 块:system_prompt,永不压缩(ADR-026 决策 7 红线)
    const CRITICAL_TOKENS: usize = 512;
    let critical_block = ContextBlock::new(
        "system_prompt",
        "x".repeat(CRITICAL_TOKENS * 4),
        CRITICAL_TOKENS,
        ContextPriority::Critical,
    );
    ctx.add_block(critical_block).expect("添加 Critical 块成功");

    // Normal 块:task_context,按需压缩
    let remaining = total_tokens.saturating_sub(CRITICAL_TOKENS);
    if remaining == 0 {
        return ctx;
    }

    // 根据总规模选择每块 tokens,平衡块数与单块大小
    let per_block = if remaining <= 8_192 {
        1024
    } else if remaining <= 65_536 {
        4096
    } else {
        8192
    };
    // 向上取整计算块数(div_ceil 在 Rust 1.73 稳定,当前工具链 1.97.0 支持)
    let block_count = remaining.div_ceil(per_block);

    for i in 0..block_count {
        // 最后一块可能不足 per_block,用剩余 tokens 补齐
        let tokens = if i == block_count - 1 {
            remaining - per_block * (block_count - 1)
        } else {
            per_block
        };
        let block = ContextBlock::new(
            format!("task_ctx_{i}"),
            "y".repeat(tokens * 4),
            tokens,
            ContextPriority::Normal,
        );
        ctx.add_block(block).expect("添加 Normal 块成功");
    }

    ctx
}

// ============================================================
// SubTask 16.1: Agent 创建/销毁 benchmark
// ============================================================

/// Agent 创建/销毁基准 — 测量 5 种 AgentType 的 create_agent + drop 延迟
///
/// `create_agent` 内部:校验唯一性 → 构造 meta → 构造 context(1M Token 等效) →
/// 构造 lifecycle → 组装 Agent → 注册 ID → 发布 AgentTaskDelegated 事件。
/// `drop Agent`:释放三组件(meta/context/lifecycle)。
///
/// 使用 `iter_batched` 生成唯一 agent_id,避免 registry 重复检测报错。
/// EventBus 无订阅者,AgentTaskDelegated 事件被静默丢弃(不测量订阅端开销)。
fn bench_agent_creation(c: &mut Criterion) {
    let factory = AgentFactory::new(EventBus::new());
    let mut group = c.benchmark_group("agent_creation");

    for (name, agent_type) in make_agent_types() {
        group.bench_with_input(BenchmarkId::from_parameter(name), &agent_type, |b, at| {
            b.iter_batched(
                || next_bench_id("agent"),
                |agent_id| {
                    let agent = factory
                        .create_agent(at.clone(), &agent_id)
                        .expect("创建 Agent 成功");
                    // drop Agent(销毁) — black_box 确保不被编译器优化掉
                    criterion::black_box(agent);
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

// ============================================================
// SubTask 16.2: 消息路由 benchmark
// ============================================================

/// 消息路由基准 — AgentTaskDelegated 事件的 publish / subscribe 延迟
///
/// - `publish_only`:仅发布(无订阅者,事件静默丢弃),测量纯 publish_blocking 开销
/// - `publish_subscribe`:发布 + FilteredSubscriber 订阅接收(双向往返延迟)
///
/// WHY 单向用 publish_blocking(同步)而非 publish(async):
/// 单向场景无需 runtime,测量更纯的 publish 延迟(序列化 + broadcast::send)。
/// 双向场景需 async recv,用 tokio runtime block_on。
fn bench_message_routing(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建成功");
    let mut group = c.benchmark_group("message_routing");

    // 场景 1:publish only(单向,无订阅者)
    {
        let bus = EventBus::new();
        let event = make_delegated_event("bench-pub-only");
        group.bench_function("publish_only", |b| {
            b.iter(|| {
                bus.publish_blocking(event.clone()).expect("发布成功");
            });
        });
    }

    // 场景 2:publish + subscribe(双向)
    {
        let bus = EventBus::new();
        // §4.4 反模式 3:subscribe 必须在 publish 之前同步调用,否则事件静默丢失
        let mut rx = bus.subscribe_filtered(HashSet::from([EventTopic::Agent]));
        let event = make_delegated_event("bench-pub-sub");
        group.bench_function("publish_subscribe", |b| {
            b.iter(|| {
                rt.block_on(async {
                    bus.publish(event.clone()).await.expect("发布成功");
                    let received = rx.recv().await.expect("接收成功");
                    criterion::black_box(received);
                });
            });
        });
    }

    group.finish();
}

// ============================================================
// SubTask 16.3: 任务拆分 benchmark
// ============================================================

/// 任务拆分基准 — RootOrchestrator::delegate 根据 TaskComplexity 分发子 Agent
///
/// 4 种 complexity 对应子 Agent 数量:
/// - Simple=1 / Medium=2 / Complex=3 / VeryComplex=5
///
/// `delegate` 内部:深度检查 → 根据 complexity 决定数量 → 循环 create_agent →
/// 收集 AgentHandle。使用 `iter_batched` 生成唯一 task_id,
/// 确保 delegate 生成的 `{task_id}-sub-{index}` agent_id 不重复。
fn bench_task_delegation(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建成功");
    let orchestrator = RootOrchestrator::new(EventBus::new());
    let mut group = c.benchmark_group("task_delegation");

    let complexities = [
        ("simple", TaskComplexity::Simple),
        ("medium", TaskComplexity::Medium),
        ("complex", TaskComplexity::Complex),
        ("very_complex", TaskComplexity::VeryComplex),
    ];

    for (name, complexity) in complexities {
        group.bench_with_input(BenchmarkId::from_parameter(name), &complexity, |b, cx| {
            let cx = *cx; // TaskComplexity: Copy
            b.iter_batched(
                || make_agent_task(cx, next_bench_id("task")),
                |task| {
                    rt.block_on(async {
                        let handles = orchestrator.delegate(task).await.expect("委托成功");
                        criterion::black_box(handles);
                    });
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

// ============================================================
// SubTask 16.4: 上下文构建 benchmark
// ============================================================

/// 上下文构建基准 — AgentContext::build_prompt 经 HCW 稀疏化的延迟
///
/// 4 种 token 规模:4K / 32K / 128K / 1M
/// 目标:1M Token 经 HCW 稀疏化后延迟 < 100ms(§3.4.4 验收)
///
/// `build_prompt` 内部:创建临时 HcwWindow → insert blocks → select_window →
/// OSA compute_all_masks → apply_sparse_mask → 按优先级拼接。
/// `&self` 方法不改状态,可重复调用,故预构造 AgentContext 在 iter 外。
///
/// WHY 每个 size 独立 group:`sample_size` 是 group 级配置,1M 场景需降到 10
/// (criterion 0.5 最小值)避免超时,其余场景用 20 平衡精度与时间。
fn bench_context_build_prompt(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建成功");

    let sizes: [(&str, usize); 4] = [
        ("4k", 4_096),
        ("32k", 32_768),
        ("128k", 131_072),
        ("1m", 1_048_576),
    ];

    for (name, total_tokens) in sizes {
        // 预构造 AgentContext(build_prompt 是 &self,不改状态,可重复调用)
        let ctx = make_context(total_tokens);
        let mut group = c.benchmark_group(format!("context_build_prompt_{name}"));

        // 1M 场景较慢,降低 sample_size 避免 benchmark 超时(criterion 0.5 最小 10)
        group.sample_size(if total_tokens >= 1_048_576 { 10 } else { 20 });

        group.bench_function("build_prompt", |b| {
            b.iter(|| {
                rt.block_on(async {
                    let prompt = ctx.build_prompt().await.expect("构建提示词成功");
                    criterion::black_box(prompt);
                });
            });
        });

        group.finish();
    }
}

// ============================================================
// criterion 注册
// ============================================================

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(50)
        .warm_up_time(Duration::from_secs(3))
        .measurement_time(Duration::from_secs(10));
    targets =
        bench_agent_creation,
        bench_message_routing,
        bench_task_delegation,
        bench_context_build_prompt,
}

criterion_main!(benches);
