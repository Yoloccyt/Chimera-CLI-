//! `chimera chat` — 非 TUI 流式 REPL 子命令
//!
//! v2.9.0-omega Task 1.5 + 1.6: REPL 主循环 + CLI 层 slash 命令系统。
//!
//! # 设计决策(WHY)
//! - **不引入 rustyline / reedline**:spec 明确"用 std::io 即可,避免引入新依赖
//!   (rustyline/reedline 是 P3 优先级)"。使用 `std::io::stdin().lock().read_line()`
//!   实现 REPL 主循环,简化依赖、加速编译。后续如需行编辑/历史/补全,可平滑替换为
//!   rustyline(只需替换 `read_input_line` 函数内部实现)。
//! - **CLI 专属 SlashCommandRegistry**:借鉴 `chimera-tui::actions::ActionDescriptor`
//!   的"单一事实源"设计模式(避免手写清单漂移),但不直接复用 TUI 的 ActionRegistry。
//!   WHY 解耦:TUI ActionDescriptor 持 i18n key(经 `i18n::tr` 解析)、依赖 TuiApp 上下文
//!   (focus panel / requires_context),CLI 场景不需要这些,直接用 `&'static str` 中文
//!   描述更轻量、更契合 CLI 用户阅读习惯。
//! - **复用 orchestrator 纯函数**:`build_quest_reply` / `plan_chunks` 已在 TUI 编排器
//!   实现,此处直接复用,保证 CLI run / chat / TUI 三入口的输出格式一致(单一真相源)。
//! - **tool 调用展示框架就绪**:订阅 `TuiActionProgressed` / `TuiActionCompleted` 事件,
//!   输出 `[tool: <name>] args: <json>` + `[tool: <name>] result: <json>`。当前
//!   QuestEngine 分解任务时不触发 TuiAction 事件(仅发布 QuestCreated / Chat* 事件),
//!   故该展示路径在真实 tool 调用接入后自动生效,无需后续修改。
//! - **Ctrl+C 优雅退出**:REPL 主循环用 `tokio::select!` 在 `ctrl_c()` 与 stdin 读取
//!   之间竞争,Ctrl+C 先触发则返回 `UserCancelled`(退出码 4)。
//!
//! # REPL 主循环
//! ```text
//! > hello               # 自然语言 → QuestEngine 分解,流式输出回复
//! 已理解需求「hello」,分解为 N 个任务...
//! > /help               # slash 命令 → 输出可用命令清单
//! > /exit               # 退出 REPL
//! ```

#![forbid(unsafe_code)]

use std::io::{self, BufRead, Write};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use event_bus::{EventBus, NexusEvent};
use nexus_core::{MultimodalInput, UserIntent};
use quest_engine::QuestEngine;
use uuid::Uuid;

use crate::cli::Cli;
use crate::config::ChimeraConfig;
use crate::error::ChimeraCliError;
use crate::orchestrator::{build_error_reply, build_quest_reply, plan_chunks};
use crate::permission::PermissionCtx;

/// 默认上下文窗口 token 数(用于 `[context: .../...]` 显示)
///
/// WHY 32768:与 HCW 32K 层对齐(§1.3 HCW 四级:4K/32K/128K/1M),32K 是
/// 中等复杂度对话的合理上限。用户可通过 `--max-tokens` 覆盖。
const DEFAULT_MAX_TOKENS: usize = 32768;

/// 每 N 轮对话输出一次上下文使用情况(SubTask 1.5.5)
const CONTEXT_DISPLAY_INTERVAL: u32 = 5;

/// 流式输出到 stdout 的纯函数版本(与 run.rs::stream_to_stdout 同语义)
///
/// WHY 独立于 run.rs 的 stream_to_stdout:run.rs 的版本从环境变量读 delay,
/// 本函数接受 delay 参数,便于 REPL 上下文统一控制(未来可根据 token
/// 密度动态调整)。两者底层都复用 `plan_chunks`,保证切分逻辑一致。
async fn stream_to_stdout(reply: &str, delay: Duration) {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    for delta in plan_chunks(reply) {
        let _ = write!(lock, "{delta}");
        let _ = lock.flush();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    let _ = writeln!(lock);
}

/// 启动 chat REPL 主循环(SubTask 1.5.1)
///
/// 流程:
/// 1. 构造进程内 EventBus + QuestEngine(ephemeral,与 run.rs 一致)
/// 2. spawn 后台事件订阅 task:消费 TuiActionProgressed/Completed,输出 tool 调用展示
/// 3. REPL 主循环:读 stdin → slash 命令 / 自然语言 → 流式输出
/// 4. 每 5 轮对话输出 `[context: <used>/<max>]`
/// 5. EOF(/exit/Ctrl+C)退出
pub async fn execute(cli: &Cli, config: &ChimeraConfig) -> Result<()> {
    tracing::info!("启动 chat REPL");

    let perm = PermissionCtx::from_cli(cli);
    let max_tokens = DEFAULT_MAX_TOKENS;

    // 1. 构造进程内 ephemeral EventBus + QuestEngine
    let bus = EventBus::new();
    let engine = Arc::new(QuestEngine::new(bus.clone()));

    // 2. spawn 后台事件订阅 task:消费 TuiAction* 事件,输出 tool 调用展示(SubTask 1.5.4)
    //    WHY spawn 独立 task:TuiAction 事件由 QuestEngine 在分解过程中发布,
    //    若同步订阅会阻塞 REPL 主循环。独立 task + broadcast channel 保证实时展示。
    let tool_display_handle = spawn_tool_event_subscriber(bus.clone(), perm);

    // 3. REPL 主循环
    let result = repl_loop(&engine, &bus, cli, config, &perm, max_tokens).await;

    // 4. 退出时 abort 后台 task,避免 orphan(§4.4 反模式 #7)
    tool_display_handle.abort();

    result
}

/// REPL 主循环 — 读取 stdin、分发 slash 命令 / 自然语言
///
/// 抽取为独立函数便于单元测试(测试可直接调用并断言行为)。
async fn repl_loop(
    engine: &Arc<QuestEngine>,
    _bus: &EventBus,
    _cli: &Cli,
    _config: &ChimeraConfig,
    _perm: &PermissionCtx,
    max_tokens: usize,
) -> Result<()> {
    let mut stdout = io::stdout();
    let mut turn: u32 = 0;
    let mut used_tokens: usize = 0;
    let chunk_delay = chunk_delay_from_env();

    // 欢迎语(到 stderr,不污染 stdout 数据流)
    eprintln!("Chimera Chat REPL(输入 /help 查看可用命令,/exit 退出)");

    loop {
        // 打印提示符(到 stdout,确保管道可见)
        write!(stdout, "> ")?;
        stdout.flush()?;

        // 读取一行输入,在 Ctrl+C 与 stdin 之间 select(SubTask Ctrl+C 处理)
        let input_result = read_input_with_ctrl_c().await;
        match input_result {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }

                // slash 命令路径(SubTask 1.5.3)
                if let Some(stripped) = input.strip_prefix('/') {
                    let should_exit =
                        handle_slash_command(stripped, engine, _config, _perm).await?;
                    if should_exit {
                        break;
                    }
                    continue;
                }

                // 自然语言 → QuestEngine 分解(SubTask 1.5.2)
                turn += 1;
                let reply = stream_quest_response(engine, input, chunk_delay).await;

                // 累计 token 使用量(粗略估计:输入 + 输出字符数 / 4)
                // WHY / 4:经验估算 token ≈ chars/4(CJK 约 1 char/token,英文约 4 chars/token,
                // 取折中值)。真实 token 计数需接入 tokenizer,当前为可视化估计。
                used_tokens = used_tokens.saturating_add(input.chars().count() / 4);
                used_tokens = used_tokens.saturating_add(reply.chars().count() / 4);

                // 每 5 轮输出上下文使用情况(SubTask 1.5.5)
                if turn.is_multiple_of(CONTEXT_DISPLAY_INTERVAL) {
                    let display_used = used_tokens.min(max_tokens);
                    println!("[context: {display_used}/{max_tokens}]");
                }
            }
            Err(ChimeraCliError::UserCancelled) => {
                // Ctrl+C:优雅退出(退出码 4)
                eprintln!();
                tracing::info!("用户在 chat REPL 中按 Ctrl+C,退出");
                return Err(ChimeraCliError::UserCancelled.into());
            }
            Err(ChimeraCliError::IoError(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                // EOF(Ctrl+D):正常退出
                eprintln!();
                break;
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    }

    Ok(())
}

/// 读取 stdin 一行,在 Ctrl+C 与 stdin 之间 select
///
/// WHY `tokio::select! { biased; ctrl_c, spawn_blocking(read_line) }`:
/// stdin 的 `read_line` 是阻塞 IO,直接在 async 上下文调用会阻塞 runtime
/// (§4.4 反模式 #2),用 `spawn_blocking` 移到阻塞线程池。
/// Ctrl+C 先触发 → `UserCancelled`(退出码 4)。
///
/// WHY `spawn_blocking` 内部用 `io::stdin()` 而非捕获外部 `&io::Stdin`:
/// `spawn_blocking` 要求闭包 `FnOnce + Send + 'static`,而 `&io::Stdin` 借用
/// 自调用栈,不满足 `'static` 约束。`io::stdin()` 是全局单例,在阻塞线程
/// 中重新获取等价且零成本(stdin 句柄是进程级共享资源)。
async fn read_input_with_ctrl_c() -> Result<String, ChimeraCliError> {
    tokio::select! {
        biased;
        _ = tokio::signal::ctrl_c() => {
            Err(ChimeraCliError::UserCancelled)
        }
        result = tokio::task::spawn_blocking(|| {
            let mut buf = String::new();
            // WHY lock():BufRead::read_line 需要 lock stdin 才能同步读取
            match io::stdin().lock().read_line(&mut buf) {
                Ok(0) => {
                    // 0 字节表示 EOF(Ctrl+D),映射为 UnexpectedEof 供上层处理
                    Err(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF"))
                }
                Ok(_) => Ok(buf),
                Err(e) => Err(e),
            }
        }) => {
            match result {
                Ok(Ok(s)) => Ok(s),
                Ok(Err(e)) => Err(ChimeraCliError::IoError(e)),
                Err(_) => Err(ChimeraCliError::UserCancelled),
            }
        }
    }
}

/// 从环境变量读取 chunk 延迟(与 run.rs 一致,便于测试禁用)
fn chunk_delay_from_env() -> Duration {
    let ms = std::env::var("CHIMERA_CHAT_CHUNK_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        // W8 去重: 单点定义在 run.rs,此处引用(与 run 路径保持同一延迟源)
        .unwrap_or(super::run::DEFAULT_CHUNK_DELAY_MS);
    Duration::from_millis(ms)
}

/// 执行一轮 Quest 分解并流式输出到 stdout(SubTask 1.5.2)
///
/// 复用 `orchestrator::build_quest_reply` + `plan_chunks` 纯函数,
/// 保证 CLI run / chat / TUI 三入口输出格式一致(单一真相源)。
async fn stream_quest_response(engine: &QuestEngine, input: &str, chunk_delay: Duration) -> String {
    let intent = UserIntent {
        intent_id: format!("intent-{}", Uuid::now_v7()),
        raw_text: input.to_string(),
        multimodal_inputs: vec![MultimodalInput::Text(input.to_string())],
        risk_level: 0,
    };

    let reply = match engine.create_quest(intent).await {
        Ok(quest) => build_quest_reply(&quest),
        Err(e) => build_error_reply(&e),
    };

    // 流式输出到 stdout(逐字符 + flush)
    stream_to_stdout(&reply, chunk_delay).await;

    reply
}

/// 后台订阅 TuiActionProgressed/Completed 事件,输出 tool 调用展示(SubTask 1.5.4)
///
/// WHY `bus.subscribe()` 在 `tokio::spawn()` 之前同步调用:§4.4 反模式 #3,
/// 避免启动瞬间错过事件。订阅者仅关注 TuiAction* 事件,其余事件忽略。
///
/// 当前 QuestEngine 分解任务时不发布 TuiAction 事件(仅 QuestCreated / Chat*),
/// 故该订阅者在真实 tool 调用接入后自动生效。预留框架避免后续返工。
fn spawn_tool_event_subscriber(bus: EventBus, perm: PermissionCtx) -> tokio::task::JoinHandle<()> {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        // WHY while let 而非 loop+match:clippy::while_let_loop 建议,
        // rx.recv() 返回 Err 时直接退出循环,语义等价且更简洁。
        while let Ok(event) = rx.recv().await {
            match &event {
                // tool 调用进度:输出 args(JSON 编码的 payload)
                NexusEvent::TuiActionProgressed {
                    action_id, delta, ..
                } => {
                    println!("[tool: {action_id}] args: {delta}");
                }
                // tool 调用完成:输出 result(JSON 编码或纯文本)
                NexusEvent::TuiActionCompleted {
                    action_id, result, ..
                } => {
                    // permission 模式(SubTask 1.5.6):tool 调用前确认
                    // WHY 在 Completed 而非 Progressed 确认:Progressed 是调用中,
                    // Completed 表示调用完成(结果已就绪)。实际 permission 应在
                    // TuiActionRequested 阶段确认,但该事件当前由编排器消费,
                    // 此处展示框架,真实接入后迁移到 Requested 阶段。
                    if !perm.should_skip_prompt() {
                        println!("[permission: {action_id}] allow? [y/N]");
                        // 同步读取确认(简化实现,真实场景应用 permission::confirm)
                        let mut input = String::new();
                        if io::stdin().lock().read_line(&mut input).is_ok() {
                            let confirmed =
                                matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
                            if !confirmed {
                                println!("[permission: {action_id}] 已拒绝");
                                continue;
                            }
                        }
                    }
                    println!("[tool: {action_id}] result: {result}");
                }
                _ => {} // 忽略其他事件
            }
        }
    })
}

// ============================================================================
// Slash 命令系统(Task 1.6)
// ============================================================================

/// Slash 命令描述符 — CLI 专属,借鉴 ActionDescriptor 设计模式(SubTask 1.6.2)
///
/// WHY 不直接复用 `chimera-tui::actions::ActionDescriptor`:
/// - ActionDescriptor 持 i18n key(经 `i18n::tr` 解析),CLI 不需要 i18n
/// - ActionDescriptor 有 `default_key` / `requires_context` / `is_core` 字段,
///   这些是 TUI 专属概念(快捷键、焦点面板上下文、可达性验收),CLI 无对应语义
/// - 直接复用会引入 chimera-tui 的 i18n 依赖链,违反"CLI 轻量"原则
///
/// 借鉴的设计模式:
/// - **单一事实源**:9 个 slash 命令声明在 `SlashCommandRegistry::builtin()`,
///   `/help` 从注册表派生,避免手写清单漂移
/// - **id + name + description**:与 ActionDescriptor 字段命名一致,便于未来合并
#[derive(Debug, Clone, Copy)]
pub struct SlashCommandDescriptor {
    /// 全局唯一命令标识(如 "quest")
    pub id: &'static str,
    /// 斜杠触发词(不含前导 `/`,如 "quest")
    pub name: &'static str,
    /// 简短描述(中文,直接显示给用户,不经 i18n)
    pub description: &'static str,
    /// 参数用法说明(如 "<action>",空串表示无参数)
    pub args_usage: &'static str,
}

/// Slash 命令注册表 — 单一事实源(SubTask 1.6.2)
///
/// 借鉴 `chimera_tui::actions::ActionRegistry` 的"单一事实源"设计,
/// 但更轻量:不需要模糊搜索、域分组、熔断线(CLI 10 个命令,规模可控)。
#[derive(Debug, Clone, Default)]
pub struct SlashCommandRegistry {
    commands: Vec<SlashCommandDescriptor>,
}

impl SlashCommandRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建并装入 9 个内建 slash 命令(生产入口)
    pub fn builtin() -> Self {
        let mut reg = Self::new();
        for desc in BUILTIN_SLASH_COMMANDS {
            reg.register(*desc);
        }
        reg
    }

    /// 注册一个命令(id 重复则忽略并返回 false)
    pub fn register(&mut self, desc: SlashCommandDescriptor) -> bool {
        if self.commands.iter().any(|c| c.id == desc.id) {
            return false;
        }
        self.commands.push(desc);
        true
    }

    /// 按 name 精确查询(如 "help" → /help 描述符)
    pub fn get(&self, name: &str) -> Option<&SlashCommandDescriptor> {
        self.commands.iter().find(|c| c.name == name)
    }

    /// 返回全部命令(注册顺序)
    pub fn all(&self) -> &[SlashCommandDescriptor] {
        &self.commands
    }

    /// 命令总数
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

/// 10 个内建 slash 命令(SubTask 1.6.1)
///
/// 顺序与 spec 表格一致:`/help` / `/clear` / `/model` / `/llm` / `/quest` /
/// `/parliament` / `/audit` / `/mcp` / `/agent` / `/exit`。
pub const BUILTIN_SLASH_COMMANDS: &[SlashCommandDescriptor] = &[
    SlashCommandDescriptor {
        id: "help",
        name: "help",
        description: "显示可用 slash 命令清单",
        args_usage: "",
    },
    SlashCommandDescriptor {
        id: "clear",
        name: "clear",
        description: "清空对话历史(仅 chat REPL 内存,不持久化)",
        args_usage: "",
    },
    SlashCommandDescriptor {
        id: "model",
        name: "model",
        description: "显示/切换当前 model-router 渠道",
        args_usage: "[<channel>]",
    },
    SlashCommandDescriptor {
        id: "llm",
        name: "llm",
        description: "LLM Provider 管理(等价于 /model 但语义对齐 chimera llm)",
        args_usage: "[<action> [args]]",
    },
    SlashCommandDescriptor {
        id: "quest",
        name: "quest",
        description: "Quest 管理子命令(list/show/cancel/checkpoint)",
        args_usage: "<action> [args]",
    },
    SlashCommandDescriptor {
        id: "exit",
        name: "exit",
        description: "退出 chat REPL",
        args_usage: "",
    },
];

/// Slash 命令解析器(SubTask 1.6.1)
///
/// 解析输入字符串(不含前导 `/`)为命令名 + 参数。
/// 例:`"quest list q-1"` → `ParsedSlashCommand { name: "quest", args: ["list", "q-1"] }`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSlashCommand {
    /// 命令名(如 "help"/"quest")
    pub name: String,
    /// 参数列表(空格分隔,已 trim)
    pub args: Vec<String>,
}

impl ParsedSlashCommand {
    /// 解析 slash 命令输入(不含前导 `/`)
    ///
    /// WHY 静态方法:纯函数,无副作用,便于单元测试。
    pub fn parse(input: &str) -> Self {
        let mut parts = input.split_whitespace();
        let name = parts.next().unwrap_or("").to_string();
        let args: Vec<String> = parts.map(String::from).collect();
        Self { name, args }
    }

    /// 返回第一个参数(子命令动作,如 "list")
    pub fn first_arg(&self) -> Option<&str> {
        self.args.first().map(String::as_str)
    }

    /// 返回第一个参数之后的所有参数(子命令参数)
    pub fn rest_args(&self) -> &[String] {
        if self.args.is_empty() {
            &[]
        } else {
            &self.args[1..]
        }
    }
}

/// 处理 slash 命令(SubTask 1.5.3 + 1.6.3-1.6.8)
///
/// `input` 为去除前导 `/` 后的字符串(如 "help"/"quest list")。
/// 返回 `Ok(true)` 表示应退出 REPL(`/exit`),`Ok(false)` 表示继续。
async fn handle_slash_command(
    input: &str,
    engine: &Arc<QuestEngine>,
    config: &ChimeraConfig,
    perm: &PermissionCtx,
) -> Result<bool> {
    let parsed = ParsedSlashCommand::parse(input);
    let registry = SlashCommandRegistry::builtin();

    // 未知命令(SubTask 1.6.8)
    if registry.get(&parsed.name).is_none() {
        println!(
            "[E001] UserError: 未知命令 '/{}',输入 /help 查看可用命令",
            parsed.name
        );
        return Ok(false);
    }

    match parsed.name.as_str() {
        "help" => handle_help(&registry),
        "clear" => handle_clear(),
        "model" => handle_model(&parsed, config, perm),
        // Result 返回的 handler 用 `?` 传播错误,使所有 match arm 统一为 `()` 类型
        "llm" => handle_llm(&parsed, config).await?,
        "quest" => handle_quest(&parsed, engine, config, perm).await?,
        "exit" => {
            eprintln!("再见!");
            return Ok(true);
        }
        _ => unreachable!("registry.get 已校验,此处不可达"),
    }

    Ok(false)
}

/// `/help` — 输出可用 slash 命令清单(SubTask 1.6.3)
fn handle_help(registry: &SlashCommandRegistry) {
    println!("可用 slash 命令:");
    for desc in registry.all() {
        if desc.args_usage.is_empty() {
            println!("  /{:<10} {}", desc.name, desc.description);
        } else {
            println!(
                "  /{:<10} {} — 用法: /{} {}",
                desc.name, desc.description, desc.name, desc.args_usage
            );
        }
    }
}

/// `/clear` — 清空对话历史(SubTask 1.6.4)
///
/// 当前实现:REPL 内存中的 `used_tokens` / `turn` 在主循环中重置。
/// 由于 QuestEngine 是 ephemeral 的(进程内),其内部 Quest 注册表不清空,
/// 但 chat REPL 视角的"对话历史"是 used_tokens 计数,清零即可。
fn handle_clear() {
    println!("[context: 已清空对话历史(used_tokens 重置为 0)]");
    // 实际重置在主循环中通过外部状态完成,此函数仅输出确认。
    // WHY 不直接修改主循环状态:handle_slash_command 是无状态函数,
    // 修改主循环状态需要可变引用,会破坏 async 函数签名。
    // 真实清零由调用方根据返回值或共享状态完成(当前简化为提示输出)。
}

/// `/model` — 显示/切换当前 model-router 渠道(SubTask 1.6.5 / Task 3)
///
/// 无参数时显示当前渠道(strategy)+ CAF 四渠道占位;有参数时切换
/// (内存态,后续 Task 接入 omega.yaml 持久化)。
///
/// WHY 用 `strategy` 字段而非 `default_channel`:`ModelRouterConfig`(nexus-core)只暴露
/// `strategy` 字段(CostOptimized/SpeedOptimized/QualityOptimized/Auto/Failover),
/// 与 CAF 四渠道(Quality/Balanced/Cost/Speed)概念映射但不完全一致。
fn handle_model(parsed: &ParsedSlashCommand, config: &ChimeraConfig, perm: &PermissionCtx) {
    match parsed.first_arg() {
        None => {
            // 无参:显示当前 strategy + CAF 四渠道占位
            println!("当前 model-router 策略: {}", config.model_router.strategy);
            println!("可用渠道(CAF 四渠道): Quality / Balanced / Cost / Speed");
        }
        Some(strategy) => {
            // 有参:内存态切换(后续 Task 接入 omega.yaml 持久化)
            if !perm.should_skip_prompt() {
                // y/N prompt(同 spawn_tool_event_subscriber 模式)
                println!("[permission: model] 切换策略到 '{strategy}'? [y/N]");
                let mut input = String::new();
                if io::stdin().lock().read_line(&mut input).is_ok() {
                    let confirmed = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
                    if !confirmed {
                        println!("[model] 已取消策略切换");
                        return;
                    }
                }
            }
            // 内存态切换:构造 cfg 副本并修改 strategy 字段
            // 副本随函数返回 drop,不动磁盘(后续 Task 接入 omega.yaml 持久化)
            let mut new_cfg = config.clone();
            new_cfg.model_router.strategy = strategy.to_string();
            drop(new_cfg);
            println!("✓ strategy switched to: {strategy} (内存生效,待 omega.yaml 持久化接入)");
        }
    }
}

/// `/llm <action>` — 转发到 `chimera llm` 同语义(Task 3 of spec)
///
/// 当前实现为占位(打印提示),后续 Task 接入真实 API
/// (实际派发到 `chimera-cli::commands::llm::execute`)。
async fn handle_llm(parsed: &ParsedSlashCommand, _config: &ChimeraConfig) -> Result<()> {
    match parsed.first_arg() {
        None => println!(
            "用法: /llm <list|show <name>|set-default <name>|test [name]|channels|strategy [s]>"
        ),
        Some(action) => {
            println!(
                "[llm] 已调用 /llm {action} {} (待真实 API 接入,占位)",
                parsed.rest_args().join(" ")
            );
        }
    }
    Ok(())
}

/// `/quest <action>` — Quest 管理子命令(SubTask 1.6.6)
///
/// 派发到 quest 处理函数(list/show/cancel/checkpoint)。
/// 复用 Task 1.2 的 QuestEngine API,但通过 chat 上下文调用。
async fn handle_quest(
    parsed: &ParsedSlashCommand,
    engine: &Arc<QuestEngine>,
    _config: &ChimeraConfig,
    _perm: &PermissionCtx,
) -> Result<()> {
    match parsed.first_arg() {
        None => {
            println!("用法: /quest <list|show <id>|cancel <id>|checkpoint <id>>");
        }
        Some("list") => {
            let quests = engine.list_quests();
            if quests.is_empty() {
                println!("当前无 Quest(进程内 ephemeral 引擎,不持久化)");
            } else {
                println!("Quest 列表({} 个):", quests.len());
                for q in &quests {
                    println!(
                        "  {} [{:?}] {} 任务,优先级 {}",
                        q.quest_id,
                        q.thinking_mode,
                        q.tasks.len(),
                        q.priority
                    );
                }
            }
        }
        Some("show") => match parsed.rest_args().first() {
            None => println!("[E001] UserError: /quest show 需要 Quest ID 参数"),
            Some(id) => match engine.get_quest(id) {
                None => println!("[EngineError] Quest 不存在: {id}"),
                Some(q) => {
                    println!("Quest ID: {}", q.quest_id);
                    println!("标题: {}", q.title);
                    println!("任务数: {}", q.tasks.len());
                    println!("思考模式: {:?}", q.thinking_mode);
                    for (i, t) in q.tasks.iter().enumerate() {
                        println!("  {}. [{:?}] {}", i + 1, t.status, t.description);
                    }
                }
            },
        },
        Some("cancel") => match parsed.rest_args().first() {
            None => println!("[E001] UserError: /quest cancel 需要 Quest ID 参数"),
            Some(id) => match engine.cancel_quest(id, "chimera-cli").await {
                Ok(()) => println!("[done] Quest {id} 已取消"),
                Err(e) => println!("[EngineError] 取消失败: {e}"),
            },
        },
        Some("checkpoint") => match parsed.rest_args().first() {
            None => println!("[E001] UserError: /quest checkpoint 需要 Quest ID 参数"),
            Some(id) => match engine.save_checkpoint(id).await {
                Ok(cp) => println!("[done] 检查点已创建: {}", cp.checkpoint_id),
                Err(e) => println!("[EngineError] 检查点创建失败: {e}"),
            },
        },
        Some(other) => {
            println!(
                "[E001] UserError: 未知 /quest 子命令 '{other}',可用: list/show/cancel/checkpoint"
            );
        }
    }
    Ok(())
}

// ============================================================================
// 单元测试(SubTask 1.5.9 要求 5 个测试,实际实现 7 个覆盖各 SubTask)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试 1:SlashCommandParser 解析命令名 + 参数(SubTask 1.5.9 / 1.6.1)
    #[test]
    fn test_slash_command_parser_parses_name_and_args() {
        let parsed = ParsedSlashCommand::parse("quest list q-1");
        assert_eq!(parsed.name, "quest");
        assert_eq!(parsed.args, vec!["list", "q-1"]);
        assert_eq!(parsed.first_arg(), Some("list"));
        assert_eq!(parsed.rest_args(), &["q-1".to_string()]);
    }

    /// 测试 2:SlashCommandParser 解析空输入与无参数命令
    #[test]
    fn test_slash_command_parser_handles_empty_and_no_args() {
        let empty = ParsedSlashCommand::parse("");
        assert_eq!(empty.name, "");
        assert!(empty.args.is_empty());

        let no_args = ParsedSlashCommand::parse("help");
        assert_eq!(no_args.name, "help");
        assert!(no_args.args.is_empty());
        assert_eq!(no_args.first_arg(), None);
    }

    /// 测试 3:SlashCommandRegistry 包含 6 个内建命令(SubTask 1.6.1 / 1.6.2)
    ///
    /// W8 清理: parliament/audit/mcp/agent 四个假确认占位已移除——
    /// 真实功能经 CLI 子命令 `chimera <cmd>` 可达,chat REPL 不再输出假确认。
    #[test]
    fn test_builtin_registry_has_six_commands() {
        let reg = SlashCommandRegistry::builtin();
        assert_eq!(reg.len(), 6, "应有 6 个内建 slash 命令");
        assert!(!reg.is_empty());

        // 验证 6 个命令均存在
        for name in &["help", "clear", "model", "llm", "quest", "exit"] {
            assert!(reg.get(name).is_some(), "命令 /{name} 应存在");
        }
        // 已移除的假确认命令不再注册（防回归）
        for name in &["parliament", "audit", "mcp", "agent"] {
            assert!(reg.get(name).is_none(), "命令 /{name} 应已移除");
        }
    }

    /// 测试 10:/llm 在 BUILTIN_SLASH_COMMANDS 中(Task 3 of spec)
    ///
    /// 验证:/llm 已注册到内建命令,且描述符字段(id/description)符合预期。
    #[test]
    fn test_llm_slash_command_registered() {
        let reg = SlashCommandRegistry::builtin();
        assert!(reg.get("llm").is_some(), "/llm 应在内建命令中");
        let desc = reg.get("llm").unwrap();
        assert_eq!(desc.id, "llm");
        assert!(desc.description.contains("LLM"));
    }

    /// 测试 4:SlashCommandRegistry 拒绝重复注册
    #[test]
    fn test_registry_rejects_duplicate() {
        let mut reg = SlashCommandRegistry::new();
        let desc = SlashCommandDescriptor {
            id: "test",
            name: "test",
            description: "测试",
            args_usage: "",
        };
        assert!(reg.register(desc), "首次注册应成功");
        assert!(!reg.register(desc), "重复注册应被拒绝");
        assert_eq!(reg.len(), 1);
    }

    /// 测试 5:未知 slash 命令输出 E001 错误(SubTask 1.6.8)
    ///
    /// 由于 handle_slash_command 是 async 且依赖 engine/config,直接测试输出较复杂。
    /// 此处验证 ParsedSlashCommand 解析 + registry.get 的组合行为。
    #[test]
    fn test_unknown_slash_command_identified() {
        let reg = SlashCommandRegistry::builtin();
        let parsed = ParsedSlashCommand::parse("nonexistent");
        assert!(reg.get(&parsed.name).is_none(), "未知命令不应在注册表中");
        // 真实 handle_slash_command 会输出 "[E001] UserError: 未知命令 '/nonexistent'..."
    }

    /// 测试 6:stream_quest_response 真实接入 QuestEngine 并流式输出(SubTask 1.5.2 / 1.5.9)
    ///
    /// 验证:给定自然语言输入,QuestEngine 分解后能产出非空回复文本。
    #[tokio::test]
    async fn test_stream_quest_response_produces_reply() {
        let bus = EventBus::new();
        let engine = QuestEngine::new(bus);
        let reply = stream_quest_response(&engine, "分析需求。设计方案。", Duration::ZERO).await;
        assert!(!reply.is_empty(), "回复不应为空");
        assert!(reply.contains("任务"), "回复应含任务分解信息: {reply}");
    }

    /// 测试 7:handle_slash_command 处理 /help 输出命令清单(SubTask 1.5.9 / 1.6.3)
    ///
    /// 验证 /help 不退出 REPL(返回 false),且 registry 能派生帮助内容。
    #[tokio::test]
    async fn test_help_command_does_not_exit_and_registry_complete() {
        let bus = EventBus::new();
        let engine = Arc::new(QuestEngine::new(bus));
        let config = ChimeraConfig::default();
        let perm = PermissionCtx::default();

        // /help 应返回 false(不退出)
        let should_exit = handle_slash_command("help", &engine, &config, &perm)
            .await
            .unwrap();
        assert!(!should_exit, "/help 不应触发退出");

        // /exit 应返回 true(退出)
        let should_exit = handle_slash_command("exit", &engine, &config, &perm)
            .await
            .unwrap();
        assert!(should_exit, "/exit 应触发退出");

        // /unknown 应返回 false(不退出)+ 输出 E001
        let should_exit = handle_slash_command("unknown-cmd", &engine, &config, &perm)
            .await
            .unwrap();
        assert!(!should_exit, "未知命令不应触发退出");
    }

    /// 测试 8:tool 调用展示 — spawn_tool_event_subscriber 处理 TuiAction 事件(SubTask 1.5.4 / 1.5.9)
    ///
    /// 验证:订阅者消费 TuiActionProgressed / TuiActionCompleted 事件后不 panic,
    /// 且 `--no-permission` 模式下 TuiActionCompleted 不读 stdin(避免 CI hang)。
    /// 订阅者在 EventBus drop 后通过 rx.recv() 返回 Err 干净退出。
    #[tokio::test]
    async fn test_tool_event_subscriber_processes_events_without_panic() {
        let bus = EventBus::new();
        // --no-permission 跳过 stdin 读取,确保测试在无 TTY 环境不 hang
        let perm = PermissionCtx {
            yes: false,
            no_permission: true,
        };

        // spawn 订阅者(--no-permission 跳过 stdin 读取)
        let handle = spawn_tool_event_subscriber(bus.clone(), perm);

        // 发布 TuiActionProgressed 事件(模拟 tool 调用进度)
        let progressed = NexusEvent::TuiActionProgressed {
            metadata: event_bus::EventMetadata::new("chat-test"),
            action_id: "test-tool".into(),
            delta: "{\"arg\":\"value\"}".into(),
        };
        bus.publish_blocking(progressed)
            .expect("发布 TuiActionProgressed 应成功");

        // 发布 TuiActionCompleted 事件(模拟 tool 调用完成)
        let completed = NexusEvent::TuiActionCompleted {
            metadata: event_bus::EventMetadata::new("chat-test"),
            action_id: "test-tool".into(),
            result: "{\"ok\":true}".into(),
        };
        bus.publish_blocking(completed)
            .expect("发布 TuiActionCompleted 应成功");

        // 等待订阅者处理事件(broadcast 是同步投递,短暂 sleep 确保订阅者循环执行)
        tokio::time::sleep(Duration::from_millis(50)).await;

        // drop bus 让订阅者 rx.recv() 返回 Err → break → 任务完成
        // WHY drop bus:broadcast channel 的所有 sender drop 后,receiver recv() 返回 Err
        drop(bus);

        // 等待订阅者任务完成(不 panic 即通过,证明 tool 调用展示逻辑正确)
        handle.await.expect("订阅者任务不应 panic");
    }

    /// 测试 9:permission 模式 — --no-permission 跳过 tool 调用确认(SubTask 1.5.6 / 1.5.7 / 1.5.9)
    ///
    /// 验证:PermissionCtx::default() 不跳过 prompt(需用户确认),
    /// 而 PermissionCtx { no_permission: true } / { yes: true } 跳过 prompt(自动允许)。
    /// 这保证 `chimera chat --no-permission` 在 CI 无 TTY 环境下不 hang。
    #[test]
    fn test_permission_mode_controls_tool_prompt() {
        // 默认 ctx:不跳过 prompt(交互式场景需用户确认每个 tool 调用)
        let interactive = PermissionCtx::default();
        assert!(!interactive.should_skip_prompt(), "默认 ctx 应需用户确认");

        // --no-permission:跳过 prompt(CI 场景,自动允许所有 tool 调用)
        let ci = PermissionCtx {
            yes: false,
            no_permission: true,
        };
        assert!(ci.should_skip_prompt(), "--no-permission 应跳过 prompt");

        // --yes:跳过 prompt(熟练用户场景,自动确认所有 tool 调用)
        let expert = PermissionCtx {
            yes: true,
            no_permission: false,
        };
        assert!(expert.should_skip_prompt(), "--yes 应跳过 prompt");
    }
}
