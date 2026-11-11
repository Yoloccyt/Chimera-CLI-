//! 子命令业务骨架 — 各命令的入口分发与实现
//!
//! 分发逻辑:
//! - 有显式子命令时路由到对应 handler
//! - 无子命令时默认启动 TUI 交互界面(品牌统一后 `chimera` 命令即进入可视化面板)
//!
//! # v2.9.0-omega Task 1.7 / 1.11 / 1.12 / 2.2 全局 flag 传递
//!
//! `dispatch` 从 `Cli` 提取以下全局 flag 并传递到各命令:
//! - `json: bool`(Task 1.7):控制输出格式(JSON / 人类可读)
//! - `PermissionCtx`(Task 1.11):封装 `--yes` / `--no-permission` 跳过逻辑
//! - `dry_run: bool`(Task 2.2):仅破坏性命令消费,`true` 时只输出预览不执行
//!
//! `--no-color`(Task 1.12)由 `output::init_color_mode` 在 `main` 入口设置全局,
//! 各命令通过 `output::print_*` helper 自动感知,无需逐命令传递。

use anyhow::Result;

use crate::cli::{Cli, Commands};
use crate::config::ChimeraConfig;
use crate::error::ChimeraCliError;
use crate::permission::{PermissionCtx, PermissionGate};

/// P2-T9: ACP 桥（Agent Client Protocol,JSON-RPC 2.0 over stdio,转译层隔离）
pub mod acp;
/// Agent 生命周期管理子命令(Task 1.10)
pub mod agent;
/// 红队安全审计子命令(Task 1.9)
pub mod audit;
/// chat REPL 流式对话子命令(Task 1.5 / 1.6)
pub mod chat;
/// shell 补全脚本生成子命令(Task 1.14)
pub mod completions;
/// 配置管理子命令
pub mod config;
/// 价格向量一期可见性子命令（只读展示统一价格向量）
pub mod cost;
/// 系统健康检查子命令(Task 1.13)
pub mod doctor;
/// WI-02: 非交互执行契约（exec stdout 纪律 + 退出码四类语义）
pub mod exec;
/// Milestone B-5: Agent Grep 双通道检索子命令
pub mod grep;
/// EXAMPLES 一级入口(Task 5 of spec)
pub mod help;
/// LLM Provider 管理子命令(Task 2 of spec)
pub mod llm;
/// MCP 量子网格管理子命令(Task 1.8)
pub mod mcp;
/// 议会审议子命令
pub mod parliament;
/// Quest 管理子命令
pub mod quest;
/// 单次任务运行子命令
pub mod run;
/// WI-01: 协议宿主服务（serve 形态,JSON-RPC v1 over stdio）
pub mod serve;
/// TUI 交互界面子命令
pub mod tui;
/// Wiki 查询子命令
pub mod wiki;

/// 命令分发入口
///
/// 根据 `Cli.command` 路由到对应子命令处理函数。
/// 无子命令时默认启动 TUI 交互界面,用户可直接输入 `chimera` 进入可视化面板。
///
/// # 组合根共享装配（M4-P1 + M9）
/// dispatch 先经 [`crate::composition::build`] 装配 AppContext（bus+engine
/// 一揽子，C12 唯一装配点），chat/run/exec/quest/parliament/agent/tui 七命令
/// 共享同一 AppContext（事件经共享 bus 对进程内订阅者可见，C3 Critical 旁路
/// 由 build() 标准装配保证）。serve/acp 走自身 `build_app_server` 路径
/// （本波次保留不动）；tui 臂带检查点引擎基于共享 bus 臂内构造
/// （B1 行为零变化，见 commands/tui.rs `assemble_quest_stack`）。
///
/// 注:参数命名为 `cfg` 而非 `config`,避免遮蔽 `pub mod config;` 声明的模块名,
/// 否则 `config::execute(...)` 会被解析为对 `&ChimeraConfig` 参数的方法调用。
pub async fn dispatch(cli: &Cli, cfg: &ChimeraConfig) -> Result<()> {
    // 从 Cli 构造 PermissionCtx(各命令按需消费,不破坏不需要 prompt 的命令签名)
    let perm = PermissionCtx::from_cli(cli);
    // C12 单一装配点保留；只把调用时机下沉到 FullContext 命令。
    // NoContext 命令（help/completions/config/cost/wiki/grep/audit/llm/mcp）
    // 不再为 EventBus/64 分片/8 experts/SccCache 付费。
    // D1：FullContext 按 spec::plan 选定的分片策略装配 —— one-shot（run/exec/…）
    // 走 Off（不付 64 worker 启动税），长驻宿主（chat/tui/serve/acp/None）走 Auto。
    let assemble_started = std::time::Instant::now();
    let mut ctx = match crate::spec::plan(cli.command.as_ref()) {
        crate::spec::CommandPlan::FullContext { sharding } => {
            Some(crate::composition::build_with_sharding(cfg, sharding)?)
        }
        crate::spec::CommandPlan::NoContext => None,
    };
    tracing::info!(
        assemble_ms = assemble_started.elapsed().as_secs_f64() * 1000.0,
        "startup phase: assemble"
    );
    match &cli.command {
        // Task 5 of spec: EXAMPLES 一级入口 — 顶级命令位置(Run 之前)
        // 不消费 json/perm(纯字符串输出)
        Some(Commands::Help { command }) => help::execute(command.as_deref(), cli).await,
        Some(Commands::Run { prompt, execute }) => {
            // ADR-247:权限门按进程 stdin 推断交互能力;测试经注入式构造覆盖三态
            run::execute_with_ctx(
                ctx_ref(&ctx)?,
                prompt,
                cli.json,
                &PermissionGate::from_stdin(),
                &perm,
                *execute,
            )
            .await
        }
        // WI-02: exec 非交互契约 — 独立退出码语义（0/2/3/4）经 exec::exec_exit_code 映射；
        // std::process::exit 立即终止（exec 为 CI 管道场景，析构无业务依赖）
        Some(Commands::Exec {
            prompt,
            plan,
            plan_concurrency,
            op_timeout_ms,
            plan_timeout_ms,
            plan_strict,
            plan_readonly,
            plan_max_steps,
            plan_budget_bytes,
        }) => {
            // E3′：`--plan` 走声明式计划执行内核；否则既有 prompt 路径逐字不变。
            let result = match plan {
                Some(plan_path) => {
                    exec::execute_plan_with_timeouts(
                        ctx_ref(&ctx)?,
                        plan_path,
                        *plan_concurrency,
                        cli.json,
                        *op_timeout_ms,
                        *plan_timeout_ms,
                        *plan_strict,
                        crate::exec_plan::PlanGuardOverrides {
                            readonly: *plan_readonly,
                            // u64→usize 用 try_from 饱和转换：32 位宿主上超大值退化为 usize::MAX，
                            // 随后 apply() 会把它夹回默认上界（只能收紧），不会放宽。
                            max_steps: (*plan_max_steps)
                                .map(|v| usize::try_from(v).unwrap_or(usize::MAX)),
                            budget_bytes: (*plan_budget_bytes)
                                .map(|v| usize::try_from(v).unwrap_or(usize::MAX)),
                        },
                    )
                    .await
                }
                None => match prompt {
                    Some(prompt) => {
                        exec::execute_with_ctx(ctx_ref(&ctx)?, prompt, cli.json, &perm).await
                    }
                    // clap `required_unless_present` 已拦截；保留防御性分支避免空 prompt 静默执行
                    None => Err::<(), anyhow::Error>(
                        ChimeraCliError::EngineError("exec 缺少 prompt（clap 应已拦截）".into())
                            .into(),
                    ),
                },
            };
            match result {
                Ok(()) => Ok(()),
                Err(e) => {
                    if let Some(ce) = e.downcast_ref::<ChimeraCliError>() {
                        eprintln!("exec error: {}", ce.message());
                        std::process::exit(exec::exec_exit_code(ce) as i32);
                    }
                    eprintln!("exec error: {e:?}");
                    std::process::exit(4); // 工具失败默认码
                }
            }
        }
        // WI-01: serve 协议宿主（不消费 json/perm——协议面自身是 JSON）
        Some(Commands::Serve) => serve::execute_with_ctx(ctx_owned(&mut ctx)?, cfg).await,
        // P2-T9: ACP 桥（转译层隔离,协议面自身是 JSON）
        Some(Commands::Acp) => acp::execute_with_ctx(ctx_owned(&mut ctx)?, cfg).await,
        // Task 1.5: chat REPL 不消费 json flag(REPL 内部统一人类可读),
        // 但消费 perm(--no-permission 自动允许 tool 调用,CI 友好)
        Some(Commands::Chat) => chat::execute_with_ctx(ctx_ref(&ctx)?, cli, cfg).await,
        // v3-engine M2(ADR-061):传递 `--no-v3-engine` flag 到 tui::execute_with_ctx,
        // 由其设置 CHIMERA_NO_V3_ENGINE 环境变量控制渲染路径回退。
        // WI-01: `--protocol` flag 传递协议模式开关（Quest 生命周期经协议面）
        // TUI 不消费 json/perm(TUI 有自己的渲染管线,不走 stdout 输出 helper)
        // M9: tui 臂已收敛至 dispatch 共享 AppContext(与 M4 六臂同先例)
        Some(Commands::Tui {
            no_v3_engine,
            protocol,
        }) => tui::execute_with_ctx(ctx_ref(&ctx)?, cfg, *no_v3_engine, *protocol).await,
        // Quest:全局 --json 优先,子命令级 --json 兼容保留（B4a 门禁红：
        // 嵌套子命令 `list` 之后不接受全局 --json，故局部 flag 不能删）
        Some(Commands::Quest { action, json }) => {
            quest::execute_with_ctx(
                ctx_ref(&ctx)?,
                action,
                cli.json || *json,
                &perm,
                cli.dry_run,
            )
            .await
        }
        Some(Commands::Config { action }) => {
            config::execute(action, cfg, cli.json, cli.config.as_deref()).await
        }
        // Wiki:全局 --json 优先,子命令级 --json 兼容保留（B4a 门禁同 Quest）;
        // --limit 由子命令参数传递(SubTask 1.3.3,默认 10)
        Some(Commands::Wiki { query, json, limit }) => {
            wiki::execute(query, cfg, cli.json || *json, *limit).await
        }
        // Milestone B-5: Agent Grep 双通道检索 — 全局 --json 优先,子命令级兼容保留
        Some(Commands::Grep { pattern, json }) => {
            grep::execute(pattern, cfg, cli.json || *json).await
        }
        // Parliament:全局 --json 优先,子命令级 --json 兼容保留;perm 预留供未来权限检查
        Some(Commands::Parliament { proposal, json }) => {
            parliament::execute_with_ctx(ctx_ref(&ctx)?, proposal, cli.json || *json, &perm).await
        }
        // Task 1.8: MCP 量子网格管理 — 全局 --json 传递;perm 供 mcp call 使用;dry_run 供 mcp call 预览
        // ADR-273：仅 `mcp call` 取 AppContext（消费唯一 ToolRuntime 做 policy+sandbox
        // 前置）；list/inspect/serve 仍 NoContext，零装配税。
        Some(Commands::Mcp { action }) => {
            let runtime = match action {
                crate::cli::McpAction::Call { .. } => Some(ctx_ref(&ctx)?.tool_runtime.as_ref()),
                _ => None,
            };
            mcp::execute(action, cfg, cli.json, &perm, cli.dry_run, runtime).await
        }
        // Task 1.9: 红队安全审计 — 全局 --json 优先,子命令级 --json 兼容保留
        Some(Commands::Audit { json, severity }) => {
            audit::execute(cfg, cli.json || *json, severity.as_deref()).await
        }
        // Task 1.10: Agent 生命周期管理 — 全局 --json 传递;perm 供 agent cancel 使用;dry_run 供 agent cancel 预览
        Some(Commands::Agent { action, parallel }) => {
            agent::execute_with_ctx(
                ctx_ref(&ctx)?,
                action,
                cli.json,
                *parallel,
                &perm,
                cli.dry_run,
            )
            .await
        }
        // Task 1.13: 系统健康检查 — 全局 --json 优先,子命令级 --json 兼容保留
        // M12 / ADR-185 D3:doctor 已迁共享 AppContext,11 探针(M13 +scc_cache、W5-1 +model_assets)经 gqep 并行 gather
        // W3-2:线程化 --config 显式路径,探针 0/2 复用已加载 cfg(不再重复 config::load)
        Some(Commands::Doctor { json, fix }) => {
            doctor::execute_with_ctx(
                ctx_ref(&ctx)?,
                cfg,
                cli.json || *json,
                *fix,
                cli.config.clone(),
            )
            .await
        }
        // Task 1.14: 生成 shell 补全脚本 — 不消费 json/perm,直接输出到 stdout
        Some(Commands::Completions { shell }) => completions::execute(*shell).await,
        // Task 2 of spec: LLM Provider 管理 — 全局 --json 传递;perm 供 set-default/strategy 使用
        Some(Commands::Llm { action }) => llm::execute(action, cfg, cli.json, &perm).await,
        // 价格向量一期可见性：只读展示，无副作用；不消费 ctx/perm/dry_run。
        // CHIMERA_LAZY_CONFIG=1 时走 LazyConfig pilot（只解析 model_router section）。
        Some(Commands::Cost) => {
            if std::env::var("CHIMERA_LAZY_CONFIG").ok().as_deref() == Some("1") {
                cost::execute_lazy(cli.config.clone(), cli.json).await
            } else {
                cost::execute(cfg, cli.json).await
            }
        }
        None => {
            // 无子命令:默认启动 TUI 交互界面(默认启用 v3-engine,非协议模式)
            // --help/--version 由 Clap 在 Cli::parse() 阶段内置处理,不会进入此分支
            tui::execute_with_ctx(ctx_ref(&ctx)?, cfg, false, false).await
        }
    }
}

/// 取 FullContext 命令的共享上下文引用。
///
/// WHY 用 Result 而非 `expect`：NoContext 分支不会调用本函数；一旦分类错误，
/// 返回结构化 EngineError（退出码 3），而不是 panic。
fn ctx_ref(
    ctx: &Option<crate::composition::AppContext>,
) -> Result<&crate::composition::AppContext> {
    ctx.as_ref().ok_or_else(|| {
        ChimeraCliError::EngineError("内部装配错误：FullContext 命令缺少 AppContext".into()).into()
    })
}

/// 取走 FullContext 命令的共享上下文（serve/acp 需要 move 进 AppServer）。
fn ctx_owned(
    ctx: &mut Option<crate::composition::AppContext>,
) -> Result<crate::composition::AppContext> {
    ctx.take().ok_or_else(|| {
        ChimeraCliError::EngineError("内部装配错误：FullContext 命令缺少 AppContext".into()).into()
    })
}

/// 测试专用共享工具 —— 供 serve.rs / acp.rs 的事件循环测试复用（杜绝重复 mock）。
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod testutil {
    use async_trait::async_trait;
    use nexus_app_server::{AppTransport, JsonRpcError, TransportError};
    use nexus_contracts::app::{AppEvent, AppOp};
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// 脚本化传输 mock：`recv_op` 依次弹出预置 `AppOp`，队列空返回 `Eof`；
    /// `send_event` 按事件 kind 记录标签供断言。无需真 stdio 即可驱动事件循环。
    ///
    /// F-c 终章扩展（2026-09-13）：支持注入**帧解码错误**（`recv_op` 优先弹出，
    /// 模拟客户端发坏帧）并记录 `send_decode_error` 回帧的 code（供断言
    /// serve/acp 主循环的协议完备性回帧行为）。
    pub(crate) struct MockTransport {
        pending: Mutex<VecDeque<AppOp>>,
        sent_kinds: Mutex<Vec<String>>,
        /// 预置解码错误（recv_op 优先于 pending 弹出，模拟坏帧到达）
        decode_errors: Mutex<VecDeque<JsonRpcError>>,
        /// 已回送错误帧的 code 序列（供断言回帧行为）
        sent_error_codes: Mutex<Vec<i32>>,
    }

    impl MockTransport {
        pub(crate) fn new(ops: impl IntoIterator<Item = AppOp>) -> Self {
            Self {
                pending: Mutex::new(ops.into_iter().collect()),
                sent_kinds: Mutex::new(Vec::new()),
                decode_errors: Mutex::new(VecDeque::new()),
                sent_error_codes: Mutex::new(Vec::new()),
            }
        }

        /// 带解码错误注入的构造 — `recv_op` 先弹 `errors`（每次一个坏帧），
        /// 耗尽后再弹 `ops`，两者皆空返回 `Eof`。
        pub(crate) fn new_with_decode_errors(
            ops: impl IntoIterator<Item = AppOp>,
            errors: impl IntoIterator<Item = JsonRpcError>,
        ) -> Self {
            Self {
                pending: Mutex::new(ops.into_iter().collect()),
                sent_kinds: Mutex::new(Vec::new()),
                decode_errors: Mutex::new(errors.into_iter().collect()),
                sent_error_codes: Mutex::new(Vec::new()),
            }
        }

        /// 已推送事件的 kind 标签序列（供断言帧序 / 是否正常处理）。
        pub(crate) fn sent_kinds(&self) -> Vec<String> {
            self.sent_kinds.lock().unwrap().clone()
        }

        /// 已回送错误帧的 JSON-RPC code 序列（供断言协议完备性回帧）。
        pub(crate) fn sent_error_codes(&self) -> Vec<i32> {
            self.sent_error_codes.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl AppTransport for MockTransport {
        async fn recv_op(&self) -> Result<AppOp, TransportError> {
            // 优先弹出预置解码错误（模拟客户端发坏帧;主循环应对其回错误帧）
            if let Some(je) = self.decode_errors.lock().unwrap().pop_front() {
                return Err(TransportError::Decode(je));
            }
            self.pending
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(TransportError::Eof)
        }

        async fn send_event(&self, ev: &AppEvent) -> Result<(), TransportError> {
            let tag = match ev {
                AppEvent::ThreadStarted { .. } => "thread_started",
                AppEvent::ItemChanged { .. } => "item_changed",
                AppEvent::ApprovalRequested { .. } => "approval_requested",
                AppEvent::Error { .. } => "error",
                _ => "other",
            };
            self.sent_kinds.lock().unwrap().push(tag.to_string());
            Ok(())
        }

        /// 记录回帧的 code（供断言;真实回帧行为由 StdinTransport 覆写承载）
        async fn send_decode_error(&self, error: &JsonRpcError) -> Result<(), TransportError> {
            self.sent_error_codes.lock().unwrap().push(error.code);
            Ok(())
        }
    }
}
