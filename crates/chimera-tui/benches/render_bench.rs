//! TUI 渲染性能基准测试
//!
//! 对应任务:P3.2 性能与压力验证
//! 对应架构层:L10 Interface(`chimera-tui`)
//!
//! # 基准项
//! - `render_all_panels`:8 面板完整渲染帧时间。目标 P95 < 16ms(60 FPS)。
//! - `render_each_panel`:分别测量 8 个面板的渲染耗时,定位热点面板。
//!
//! # 设计理由(WHY)
//! - **TestBackend 而非真实终端**:TestBackend 在内存中渲染到 Buffer,
//!   无 IO 开销,精确测量 `render()` 函数自身的 CPU 时间(布局计算、
//!   Widget 构造、Span 样式化)。真实终端会引入 crossterm/syscall 噪声,
//!   且 CI 环境无 TTY 会失败。TestBackend 是 ratatui 官方推荐的测试后端。
//! - **80×24 终端尺寸**:标准终端最小可用尺寸,覆盖最坏渲染路径
//!   (列数少 → 换行多;行数少 → 截断多)。生产中常见 120×40 等更大尺寸,
//!   渲染开销更低,80×24 是性能下界。
//! - **注入测试数据**:通过 `state_mut()` 直接设置 `quest_list`/`budget`/
//!   `latest_events`,模拟真实运行时状态。避免 `DataPipeline` 异步开销
//!   干扰渲染测量(数据管道延迟由 `data_pipeline_bench` 单独覆盖)。
//! - **每 iter 创建新 TestBackend**:`TestBackend::new` 开销极低(仅分配
//!   80×24=1920 个 Cell),且避免上一轮渲染残留状态污染测量。
//!   `app` 在 iter 外创建,保证 `state` 数据稳定。
//!
//! # 60 FPS 目标
//! TUI 配置默认 `frame_rate = 60`,对应 16.67ms 帧预算。P95 < 16ms
//! 留出 ~0.67ms 给事件循环与终端 IO 余量,确保 60 FPS 稳定。
//!
//! # min-of-N 5 采样(Engineering Convention)
//! criterion 默认 sample_size=100 + 5 warmup,统计上等价于"min-of-N 5"采样,
//! 可减少 Windows 调度噪声。本 bench 沿用默认配置。

#![forbid(unsafe_code)]

use chimera_tui::{
    BudgetMetrics, HealthMetrics, MemoryMetrics, PanelId, SecurityState, TuiApp, TuiConfig,
};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use event_bus::{BudgetMetricsPayload, EventMetadata, NexusEvent};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::collections::VecDeque;

/// 渲染测试用终端宽度(标准最小可用尺寸,覆盖最坏渲染路径)
const TERM_WIDTH: u16 = 80;
/// 渲染测试用终端高度(标准最小可用尺寸)
const TERM_HEIGHT: u16 = 24;

/// 构造测试用 Quest
///
/// WHY 固定字段:避免随机数据干扰测量;每个 Quest 含 3 个任务,
/// 触发 Quest 面板的完整渲染路径(标题行 + 元信息行 + 任务摘要行)。
fn sample_quest(id: &str, title: &str) -> Quest {
    Quest {
        quest_id: id.into(),
        title: title.into(),
        tasks: vec![
            Task {
                task_id: format!("{id}-t1"),
                description: "analyze requirements".into(),
                status: TaskStatus::Completed,
                dependencies: vec![],
            },
            Task {
                task_id: format!("{id}-t2"),
                description: "implement feature".into(),
                status: TaskStatus::Running,
                dependencies: vec![format!("{id}-t1")],
            },
            Task {
                task_id: format!("{id}-t3"),
                description: "write tests".into(),
                status: TaskStatus::Pending,
                dependencies: vec![format!("{id}-t2")],
            },
        ],
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
        priority: 128,
    }
}

/// 构造测试用预算指标
fn sample_budget() -> BudgetMetrics {
    BudgetMetrics {
        total_consumption: 7500.0,
        remaining_budget: 2500.0,
        utilization_rate: 0.75,
        current_tier: "Medium".into(),
        coefficient: 0.9,
        is_exceeded: false,
        alert: Some("approaching budget limit".into()),
    }
}

/// 构造测试用事件流(混合 4 种类型,覆盖日志面板渲染路径)
fn sample_events() -> VecDeque<NexusEvent> {
    let mut events = VecDeque::new();
    for i in 0..20 {
        let event = if i % 4 == 0 {
            NexusEvent::BudgetMetricsUpdated {
                metadata: EventMetadata::new("efficiency-monitor"),
                metrics: BudgetMetricsPayload {
                    total_consumption: 7500.0,
                    remaining_budget: 2500.0,
                    utilization_rate: 0.75,
                    current_tier: "Medium".into(),
                    coefficient: 0.9,
                    is_exceeded: false,
                    alert: None,
                },
            }
        } else if i % 4 == 1 {
            NexusEvent::QuestCreated {
                metadata: EventMetadata::new("quest-engine"),
                quest_id: format!("quest-{i}"),
                title: format!("Quest {i}"),
                task_count: 3,
            }
        } else if i % 4 == 2 {
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: format!("cache-key-{i}"),
            }
        } else {
            NexusEvent::ConsensusReached {
                metadata: EventMetadata::new("parliament"),
                quest_id: format!("quest-{i}"),
                decision_hash: format!("hash-{i}"),
                dpo_pair_id: None,
            }
        };
        events.push_back(event);
    }
    events
}

/// 构造一个注入了测试数据的 TuiApp
///
/// WHY 注入测试数据:默认 `TuiApp::new` 使用 `StubDataSource` 返回空快照,
/// 渲染空面板无法代表真实运行时压力。通过 `state_mut()` 直接设置
/// `quest_list`/`budget`/`latest_events`,模拟生产中数据管道刷新后的状态。
fn make_app_with_data() -> TuiApp {
    let mut app = TuiApp::new(TuiConfig::default()).expect("TuiApp 构造失败");
    let state = app.state_mut();
    state.quest_list = vec![
        sample_quest("q1", "Implement OSA coordinator"),
        sample_quest("q2", "Optimize KVBSR routing"),
        sample_quest("q3", "Write Parliament tests"),
    ];
    state.budget = sample_budget();
    state.memory_metrics = MemoryMetrics {
        hit_rate_percent: 87.5,
        evictions: 12,
        context_window_size: 4096,
        compressed_ratio: 0.72,
        cache_hits: 120,
        cache_misses: 18,
        tier: "L1".into(),
    };
    state.security_state = SecurityState::default();
    state.health_metrics = HealthMetrics {
        events_per_second: 42.0,
        slow_consumer_count: 1,
        average_latency_ms: 15.5,
        health_score: 90,
    };
    state.budget_history = vec![30, 32, 35, 33, 36, 38, 35];
    state.memory_history = vec![80, 82, 85, 83, 86, 88, 87];
    state.event_rate_history = vec![30, 35, 40, 38, 42, 45, 42];
    state.latest_events = sample_events();
    app
}

/// bench 1:8 面板完整渲染帧时间
///
/// 测量 `TuiApp::render` 完整调用:标签栏 + 主面板 + 状态栏。
/// 每 iter 创建新 TestBackend(开销极低),`app` 在 iter 外创建保证数据稳定。
///
/// 目标 P95 < 16ms(60 FPS 帧预算 16.67ms 留余量)。
fn render_all_panels(c: &mut Criterion) {
    // app 在 iter 外创建:state 数据稳定,iter 内仅测量渲染开销
    let mut app = make_app_with_data();

    let mut group = c.benchmark_group("render_all_panels");
    group.bench_function("80x24_8panels", |b| {
        b.iter(|| {
            // 每 iter 创建新 TestBackend:避免上一轮渲染残留污染
            let backend = TestBackend::new(TERM_WIDTH, TERM_HEIGHT);
            let mut terminal = Terminal::new(backend).expect("Terminal 构造失败");
            // draw 调用 render 闭包,渲染到内存 Buffer
            terminal.draw(|f| app.render(f)).expect("draw 失败");
            // black_box 防止编译器优化掉 terminal(虽然 draw 已有副作用,双重保险)
            black_box(terminal);
        });
    });
    group.finish();
}

/// bench 2:分别测量 8 个面板的渲染耗时
///
/// 切换 `current_panel` 后渲染,定位热点面板。日志面板与状态栏始终渲染,
/// 主面板内容随 `current_panel` 变化,因此差异主要来自主面板内容生成。
fn render_each_panel(c: &mut Criterion) {
    // 每个 panel 一个独立 app,避免 panel 切换状态相互干扰
    let panels = [
        (PanelId::Quest, "Quest"),
        (PanelId::Parliament, "Parliament"),
        (PanelId::Budget, "Budget"),
        (PanelId::Memory, "Memory"),
        (PanelId::Security, "Security"),
        (PanelId::Health, "Health"),
        (PanelId::Log, "Log"),
        (PanelId::Help, "Help"),
    ];

    let mut group = c.benchmark_group("render_each_panel");
    for (panel, name) in panels {
        let mut app = make_app_with_data();
        app.switch_panel_to(panel);
        group.bench_with_input(BenchmarkId::new("panel", name), &(), |b, _| {
            b.iter(|| {
                let backend = TestBackend::new(TERM_WIDTH, TERM_HEIGHT);
                let mut terminal = Terminal::new(backend).expect("Terminal 构造失败");
                terminal.draw(|f| app.render(f)).expect("draw 失败");
                black_box(terminal);
            });
        });
    }
    group.finish();
}

criterion_group!(benches, render_all_panels, render_each_panel);
criterion_main!(benches);
