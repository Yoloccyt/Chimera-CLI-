//! Clap 子命令定义 — NEXUS-OMEGA CLI 的命令行界面
//!
//! 命令树:
//! ```text
//! chimera
//!   ├── help [command]          # EXAMPLES 一级入口(Task 5 of spec)
//!   ├── run <prompt>          # 运行单次任务
//!   ├── chat                   # 流式 REPL 对话
//!   ├── tui                    # 启动 TUI 交互界面
//!   ├── quest <action>         # Quest 管理
//!   │     ├── list             # 列出所有 Quest
//!   │     ├── show <id>        # 查看 Quest 详情
//!   │     ├── cancel <id>      # 取消 Quest
//!   │     └── checkpoint <id>  # 创建检查点
//!   ├── config <action>        # 配置管理
//!   │     ├── init             # 生成默认 omega.yaml
//!   │     ├── list             # 列出当前配置
//!   │     ├── show             # 显示完整配置(JSON)
//!   │     └── path             # 显示配置文件路径
//!   ├── wiki <query>           # Wiki 查询
//!   ├── parliament <proposal>  # 议会审议
//!   ├── mcp <action>           # MCP 量子网格管理
//!   │     ├── list             # 列出所有 MCP 服务器
//!   │     ├── serve            # 启动 MCP 服务器
//!   │     ├── call <s> <t> [args]  # 调用 MCP 工具
//!   │     └── inspect <server> # 服务器详情
//!   ├── audit                  # 红队安全审计
//!   ├── agent <action>         # Agent 生命周期管理
//!   │     ├── list             # 列出所有 Agent
//!   │     ├── spawn --quadrant # 创建 Agent
//!   │     ├── inspect <id>     # Agent 详情
//!   │     └── cancel <id>      # 取消 Agent
//!   ├── doctor                 # 系统健康检查
//!   ├── completions <shell>    # 生成 shell 补全
//!   └── llm <action>           # LLM Provider 管理(Task 2 of spec)
//!         ├── list             # 列出已配置 Provider
//!         ├── show <name>      # Provider 详情
//!         ├── set-default <n>  # 设置默认 Provider
//!         ├── test [name]      # 探测连通性
//!         ├── channels         # 4 路由渠道
//!         └── strategy [name]  # 显示/设置 model-router 策略
//! ```

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// 顶层 CLI 解析结构
///
/// 使用 `Option<Commands>` 而非 `Commands`,使得无子命令时仍可显示帮助
/// (符合 §6 红线:避免暴力加载,无命令时不应执行任何重活)。
#[derive(Parser, Debug)]
#[command(
    name = "chimera",
    version,
    about = "NEXUS-OMEGA AI Coding Agent — 全维稀疏架构的下一代编码代理",
    long_about = "NEXUS-OMEGA AI Coding Agent — 全维稀疏架构的下一代编码代理\n\
\n\
基于 OMEGA 四定律(Ω-Sparse / Ω-Compress / Ω-Evolve / Ω-Event)构建,\n\
提供 Quest 长期任务管理、多模型议会审议、MCP 量子网格、红队安全审计等能力。\n\
默认启动 TUI 交互界面,也可通过子命令进行脚本化操作。",
    // Task 5 of spec: 禁用 clap 自动生成的 `help` 子命令,避免与我们的 `Help` 变体冲突
    // (我们的 `chimera help` 专门输出 EXAMPLES,而非 clap 默认的子命令帮助文本)。
    disable_help_subcommand = true,
    after_long_help = "EXAMPLES:\n  \
chimera run \"实现一个 hello world 函数\"      # 运行单次任务\n  \
chimera --json quest list                       # JSON 格式列出 Quest\n  \
chimera --yes agent cancel <agent-id>           # 取消 Agent(跳过确认)\n  \
chimera doctor                                  # 5 维度健康检查\n  \
chimera completions bash > /etc/bash_completion.d/chimera  # 生成补全脚本"
)]
pub struct Cli {
    /// 子命令(可选,缺省时启动 TUI 交互界面)
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// 配置文件路径(默认 ~/.chimera/omega.yaml)
    ///
    /// 全局参数,可在任意子命令前使用,如 `chimera --config ./x.yaml run "hi"`
    #[arg(long, global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// 启用详细日志(等价于 RUST_LOG=debug)
    #[arg(long, short = 'v', global = true)]
    pub verbose: bool,

    /// 兼容 v2.8.0-omega 退出码行为(所有错误统一返回 1,不区分错误类别)
    ///
    /// WHY:v2.9.0-omega 引入 ExitCode 矩阵(0-6)是 BREAKING 变更,
    /// 依赖 `$? -eq 1` 判断失败的 v2.8.0 脚本会因新的退出码 2-6 而误判成功。
    /// 此 flag 提供 2 个版本周期的兼容窗口(计划 v2.11.0-omega 移除),
    /// 详见 ADR-060。类似 ADR-029 TUI v3.1 的 BREAKING 处理模式。
    #[arg(long, global = true, hide = true)]
    pub legacy_exit_code: bool,

    // === v2.9.0-omega 全局 flag ===
    /// 以 JSON 格式输出命令结果
    ///
    /// 启用后,命令输出遵循 envelope schema:
    /// - 成功:`{ "status": "ok", "data": <payload> }`
    /// - 错误(stderr):`{ "status": "error", "error": { "kind", "message" }, "exit_code": <N> }`
    ///
    /// 便于脚本程序化消费(`chimera quest list --json | jq`),详见 `output.rs` schema 文档。
    #[arg(long, global = true)]
    pub json: bool,

    /// 自动确认所有 permission prompt
    ///
    /// 跳过 `quest cancel` / `agent cancel` / `mcp call` 等破坏性命令的交互式确认,
    /// 适合熟练用户快速操作。语义上假设用户已知晓操作影响并主动确认。
    #[arg(long, global = true)]
    pub yes: bool,

    /// 自动允许所有操作,不弹 permission prompt
    ///
    /// CI 友好的 fail-open 模式:假设运行环境无交互能力(无 TTY),
    /// 自动允许所有操作。与 `--yes` 的区别:`--no-permission` 假设无 TTY,
    /// 未来可触发更详细的审计日志(当前实现效果与 `--yes` 相同)。
    #[arg(long = "no-permission", global = true)]
    pub no_permission: bool,

    /// 禁用 ANSI 颜色输出
    ///
    /// 启用后,所有彩色输出 helper(`print_success` / `print_error` 等)
    /// 仅输出纯文本前缀(`✓` / `✗` / `⚠` / `ℹ`),不包含 ANSI 颜色码。
    /// 等价于设置 `NO_COLOR=1` 环境变量(遵循 https://no-color.org 规范)。
    /// CI 友好:避免日志解析器被 ANSI 转义序列干扰。
    #[arg(long = "no-color", global = true)]
    pub no_color: bool,

    /// 关闭启动 banner(品牌 ASCII art)
    ///
    /// 启用后,CLI 启动时不再向 stderr 打印 `banner::print()` 输出的彩条标题。
    /// 适用于 CI 日志截屏、自动化测试断言 stdout 纯数据等场景。
    /// 与 `--no-color` / `--no-permission` 命名风格保持一致(否定语义 kebab-case)。
    #[arg(long = "no-banner", global = true)]
    pub no_banner: bool,

    /// 预览模式:只输出操作预览不实际执行
    ///
    /// 启用后,破坏性命令(`quest cancel` / `agent cancel` / `mcp call`)
    /// 只输出 `[dry-run] 将执行 X 操作,不执行` 预览到 stderr,不调用真实 API。
    /// 便于 CI 中验证命令参数正确性而不产生副作用,也可用于训练脚本预演。
    ///
    /// WHY 全局 flag 而非子命令级:与 `--yes` / `--no-permission` 一致,
    /// 用户可在任意破坏性命令前组合使用(如 `chimera --dry-run --yes quest cancel <id>`)。
    /// dry-run 检查在 permission prompt 之后,确保预览前仍经过权限确认。
    #[arg(long = "dry-run", global = true)]
    pub dry_run: bool,
}

/// 一级子命令枚举
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// 显示 EXAMPLES(默认从顶层 after_long_help 派生)
    #[command(
        long_about = "显示 EXAMPLES(默认从顶层 after_long_help 派生)\n\
\n\
`chimera help` 无参时显示 5 个顶级 EXAMPLES;\n\
`chimera help <command>` 显示子命令的 EXAMPLES。\n\
`--no-examples` 关闭示例只显示 about。",
        after_long_help = "EXAMPLES:\n  \
chimera help                                 # 5 个顶级 EXAMPLES\n  \
chimera help quest                           # quest 子命令 EXAMPLES\n  \
chimera help llm                             # llm 子命令 EXAMPLES"
    )]
    Help {
        /// 可选子命令名(如 "quest" / "llm");不传则输出顶级
        command: Option<String>,
    },
    /// 运行单次任务（不进入 Quest 长期任务流程）
    #[command(
        long_about = "将用户提示词经 QuestEngine 分解为 Quest，流式输出回复到 stdout。\n\
适合一次性代码生成/问答场景;长期任务管理请用 `chimera quest` 或 `chimera tui`。",
        after_long_help = "EXAMPLES:\n  \
chimera run \"实现一个 hello world 函数\"       # 运行单次代码生成任务\n  \
chimera --json run \"重构核心模块\"             # JSON 格式输出(便于脚本消费)\n  \
CHIMERA_RUN_CHUNK_DELAY_MS=0 chimera run \"test\"  # 禁用流式延迟加速测试"
    )]
    Run {
        /// 任务提示词(用户意图的原始文本)
        prompt: String,
    },
    /// 启动 chat REPL（非 TUI 流式对话，支持 slash 命令）
    ///
    /// 与 `chimera tui` 的区别:`chat` 是纯文本流式 REPL,无 ratatui 渲染,
    /// 适合无 TTY 环境或管道消费(`chimera chat | tee log.txt`)。
    /// 支持 9 个 slash 命令(/help / /clear / /model / /quest / /parliament /
    /// /audit / /mcp / /agent / /exit),详见 `/help` 输出。
    #[command(
        long_about = "纯文本流式 REPL，无 ratatui 渲染，适合无 TTY 环境或管道消费。\n\
支持 9 个 slash 命令（/help / /clear / /model / /quest / /parliament / /audit / /mcp / /agent / /exit）。",
        after_long_help = "EXAMPLES:\n  \
chimera chat                                   # 启动交互式对话\n  \
chimera --no-permission chat                   # 自动允许所有 tool 调用(CI 友好)\n  \
chimera chat | tee log.txt                     # 管道消费对话输出"
    )]
    Chat,
    /// 启动 TUI 交互界面（对应 `chimera-tui` crate）
    ///
    /// v3-engine M2(ADR-061):自研渲染路径默认启用,如需回退到 ratatui 路径
    /// 用于验证或排查渲染问题,可传入 `--no-v3-engine` flag(等价于设置
    /// `CHIMERA_NO_V3_ENGINE=1` 环境变量)。兼容窗口计划至 v2.11.0-omega 移除。
    #[command(
        long_about = "v3-engine M2（ADR-061）:自研渲染路径默认启用，如需回退到 ratatui 路径\n\
用于验证或排查渲染问题，可传入 `--no-v3-engine` flag（等价于设置\n\
`CHIMERA_NO_V3_ENGINE=1` 环境变量）。兼容窗口计划至 v2.11.0-omega 移除。",
        after_long_help = "EXAMPLES:\n  \
chimera tui                                    # 启动 TUI(默认启用 v3-engine)\n  \
chimera tui --no-v3-engine                     # 回退到 ratatui 渲染路径\n  \
chimera                                        # 无子命令时默认启动 TUI"
    )]
    Tui {
        /// 禁用 v3-engine 自研渲染路径,回退到 ratatui(M2 切换后 2 个版本周期兼容)
        #[arg(long = "no-v3-engine")]
        no_v3_engine: bool,
    },
    /// Quest 管理（长期任务的创建/查询/取消/检查点）
    #[command(
        long_about = "Quest 是 NEXUS-OMEGA 的长期任务单元，经 QuestEngine 分解为多个子任务。\n\
支持 list/show/cancel/checkpoint 4 个子动作。\n\
注:进程内 ephemeral 引擎不跨进程持久化，真实 Quest 管理请用 `chimera tui`。",
        after_long_help = "EXAMPLES:\n  \
chimera quest list                             # 列出所有 Quest\n  \
chimera --json quest show <quest-id>           # 查看 Quest 详情(JSON 格式)\n  \
chimera --yes quest cancel <quest-id>          # 取消 Quest(跳过确认)"
    )]
    Quest {
        /// Quest 子命令动作
        #[command(subcommand)]
        action: QuestAction,
        /// 输出 JSON 格式(机器可读,后续将统一为全局 --json)
        ///
        /// WHY 子命令级 flag:在引入全局 --json 之前提供结构化输出能力,
        /// 此 flag 提供 quest list/show 等子命令的结构化输出能力,便于脚本消费。
        /// 全局 --json 引入后此 flag 将被替代,保留 2 个版本周期兼容。
        #[arg(long)]
        json: bool,
    },
    /// 配置管理（初始化/查看/列出）
    #[command(
        long_about = "管理 omega.yaml 配置文件。支持 init/list/show/path 4 个子动作。\n\
配置优先级:CLI --config > 默认路径 > env > defaults。",
        after_long_help = "EXAMPLES:\n  \
chimera config init                            # 生成默认 omega.yaml\n  \
chimera config list                            # 列出当前生效配置项\n  \
chimera --json config show                     # 显示完整配置(JSON)"
    )]
    Config {
        /// 配置子命令动作
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Wiki 查询（对应 `repo-wiki` crate 的语义检索）
    #[command(
        long_about = "基于 FTS5 全文检索 + HNSW 向量近似最近邻搜索的混合查询。\n\
默认输出 Top-10 结果（标题 + 相似度分数 + 摘要）。",
        after_long_help = "EXAMPLES:\n  \
chimera wiki \"Quest 分解机制\"                 # 语义检索\n  \
chimera wiki --limit 20 \"OMEGA 四定律\"        # 限制返回 20 条\n  \
chimera --json wiki \"EventBus\"               # JSON 输出"
    )]
    Wiki {
        /// 查询语句(自然语言)
        query: String,
        /// 输出 JSON 格式(机器可读,后续将统一为全局 --json)
        #[arg(long)]
        json: bool,
        /// 限制返回结果数量(默认 10)
        ///
        /// WHY 默认 10:与 spec "默认输出 Top-10 结果" 对齐,
        /// 用户可通过 `--limit 20` 扩大或 `--limit 1` 缩小。
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Agent Grep — 双通道检索知识库与代码行为定位（Milestone B-5）
    #[command(
        long_about = "双通道检索：知识通道（FTS5 全文 + LIKE 降级）+ 代码通道（BGPD 三级披露）。\n\
适合快速定位「哪个模块实现了某行为」与「知识库中相关条目」。",
        after_long_help = "EXAMPLES:\n  \
chimera grep 事件总线                          # 双通道检索\n  \
chimera --json grep 沙箱                      # JSON 输出结构化报告"
    )]
    Grep {
        /// 检索模式（自然语言或关键词）
        pattern: String,
        /// 输出 JSON 格式(机器可读)
        #[arg(long)]
        json: bool,
    },
    /// 议会审议（对应 `parliament` crate，提交提案供多模型议会表决）
    #[command(
        long_about = "5 角色对抗性审议（Architect/Implementer/Skeptic/Optimizer/Reviewer），\n\
Skeptic 拥有否决权（红队防线），加权投票决定共识三态（达成/拒绝/否决）。",
        after_long_help = "EXAMPLES:\n  \
chimera parliament \"重构核心模块提升性能\"      # 提交提案审议\n  \
chimera --json parliament \"新增 Agent 协同\"   # JSON 输出审议记录\n  \
chimera parliament \"修改依赖方向\"             # 查看审议过程(stderr)+ 共识结果(stdout)"
    )]
    Parliament {
        /// 提案内容(需审议的决策描述)
        proposal: String,
        /// 输出 JSON 格式(机器可读,后续将统一为全局 --json)
        #[arg(long)]
        json: bool,
    },
    /// MCP 量子网格管理
    ///
    /// 管理 MCP 服务器注册表、启动服务器、调用工具、检查服务器详情。
    /// 对应 `mcp-mesh` crate(L10 Interface 层),CLI 直接调用进程内 ephemeral mesh。
    #[command(
        long_about = "管理 MCP 服务器注册表、启动服务器、调用工具、检查服务器详情。\n\
对应 `mcp-mesh` crate（L10 Interface 层），CLI 直接调用进程内 ephemeral mesh。",
        after_long_help = "EXAMPLES:\n  \
chimera mcp list                               # 列出所有 MCP 服务器\n  \
chimera --yes mcp call <server> <tool> [args]  # 调用 MCP 工具(跳过确认)\n  \
chimera mcp inspect <server>                   # 检查服务器详情"
    )]
    Mcp {
        /// MCP 子命令动作(list/serve/call/inspect)
        #[command(subcommand)]
        action: McpAction,
    },
    /// 红队安全审计
    ///
    /// 调用 `parliament::ahirt::AhirtRedTeam` 执行 4 类攻击向量探测
    /// (PromptInjection / CommandInjection / PrivilegeEscalation / SandboxEscape),
    /// 输出漏洞清单与修复建议。`--severity` 可过滤严重度级别。
    #[command(
        long_about = "调用 `parliament::ahirt::AhirtRedTeam` 执行 4 类攻击向量探测\n\
（PromptInjection / CommandInjection / PrivilegeEscalation / SandboxEscape），\n\
输出漏洞清单与修复建议。`--severity` 可过滤严重度级别。",
        after_long_help = "EXAMPLES:\n  \
chimera audit                                  # 执行全量红队审计\n  \
chimera --json audit                           # JSON 输出审计报告\n  \
chimera audit --severity high                  # 过滤 high 严重度漏洞"
    )]
    Audit {
        /// 输出 JSON 格式(机器可读,后续将统一为全局 --json)
        #[arg(long)]
        json: bool,
        /// 过滤严重度级别(如 "critical" / "high" / "medium" / "low")
        ///
        /// WHY 可选:默认输出全部严重度;指定级别时仅显示该级别及以上的漏洞。
        #[arg(long)]
        severity: Option<String>,
    },
    /// Agent 生命周期管理
    ///
    /// 对应 `chimera-mas` crate(L9 Quest 层),提供多 Agent 协同子系统的 CLI 入口。
    /// 支持 list/spawn/inspect/cancel 4 个子动作,`--parallel` 启用并行派发模式。
    #[command(
        long_about = "对应 `chimera-mas` crate（L9 Quest 层），提供多 Agent 协同子系统的 CLI 入口。\n\
支持 list/spawn/inspect/cancel 4 个子动作，`--parallel` 启用并行派发模式。",
        after_long_help = "EXAMPLES:\n  \
chimera agent list                             # 列出所有 Agent\n  \
chimera agent spawn --quadrant Q1 --task \"实现 hello\"  # 在 Q1 象限创建 Agent\n  \
chimera agent --parallel spawn --quadrant Q2 --task \"集成测试\"  # 并行派发 2 个 Agent\n  \
chimera --yes agent cancel <agent-id>          # 取消 Agent(跳过确认)"
    )]
    Agent {
        /// Agent 子命令动作(list/spawn/inspect/cancel)
        #[command(subcommand)]
        action: AgentAction,
        /// 并行派发多 Agent(参考 Kimi Code CLI 三 Agent 并行模式)
        ///
        /// WHY flag 而非子参数:并行模式影响 spawn/inspect 的执行语义,
        /// 作为 flag 可与任意子动作组合(如 `agent spawn --parallel --quadrant Q1`)。
        #[arg(long)]
        parallel: bool,
    },
    /// 系统健康检查
    ///
    /// 执行 6 维度健康检查:
    /// 1. 配置文件路径与有效性
    /// 2. Cargo.lock 依赖完整性
    /// 3. SQLite 数据库可读写
    /// 4. MCP 网格连通性
    /// 5. EventBus 订阅者活跃数
    /// 6. LLM Provider 健康度(Wave 2 Task 4)
    ///
    /// `--fix` 自动修复可修复项(如缺失配置文件)。
    #[command(
        long_about = "执行 6 维度健康检查:\n\
1. 配置文件路径与有效性\n\
2. Cargo.lock 依赖完整性\n\
3. SQLite 数据库可读写\n\
4. MCP 网格连通性\n\
5. EventBus 订阅者活跃数\n\
6. LLM Provider 健康度(Wave 2 Task 4)\n\
\n\
`--fix` 自动修复可修复项（如缺失配置文件）。",
        after_long_help = "EXAMPLES:\n  \
chimera doctor                                 # 执行 6 维度健康检查\n  \
chimera --json doctor                          # JSON 输出健康报告\n  \
chimera doctor --fix                           # 自动修复可修复项"
    )]
    Doctor {
        /// 输出 JSON 格式(机器可读,后续将统一为全局 --json)
        #[arg(long)]
        json: bool,
        /// 自动修复可修复项(如缺失配置文件)
        #[arg(long)]
        fix: bool,
    },
    /// 生成 shell 补全脚本
    ///
    /// 支持 bash / zsh / fish / powershell / elvish 5 种 shell。
    /// 生成的脚本写入 stdout,可重定向到 shell 补全目录:
    /// `chimera completions bash > /etc/bash_completion.d/chimera`
    #[command(
        long_about = "支持 bash / zsh / fish / powershell / elvish 5 种 shell。\n\
生成的脚本写入 stdout，可重定向到 shell 补全目录。",
        after_long_help = "EXAMPLES:\n  \
chimera completions bash > /etc/bash_completion.d/chimera  # 生成 bash 补全\n  \
chimera completions zsh > ~/.zsh/completions/_chimera      # 生成 zsh 补全\n  \
chimera completions powershell >> $PROFILE                 # 生成 PowerShell 补全"
    )]
    Completions {
        /// 目标 shell(bash/zsh/fish/powershell/elvish)
        shell: clap_complete::Shell,
    },
    /// LLM Provider 管理(统一入口,封装 mca-gateway / model-router)
    #[command(
        long_about = "LLM Provider 管理(统一入口,封装 mca-gateway / model-router)\n\
\n\
管理 Provider / Channel / Strategy,支持连通性测试与持久化配置。\n\
对应 `mca-gateway`(8 个 affinity profile)与 `model-router` 4 路由策略。",
        after_long_help = "EXAMPLES:\n  \
chimera llm list                              # 列出已配置 Provider\n  \
chimera llm test deepseek                     # 探测连通性\n  \
chimera llm set-default deepseek              # 设置默认 Provider\n  \
chimera llm strategy CostOptimized            # 切换路由策略"
    )]
    Llm {
        /// LLM 子命令动作
        #[command(subcommand)]
        action: LlmAction,
    },
}

/// Quest 子命令动作
#[derive(Subcommand, Debug)]
pub enum QuestAction {
    /// 列出所有 Quest(含进行中/已完成/已取消)
    List,
    /// 查看 Quest 详情
    Show {
        /// Quest ID
        id: String,
    },
    /// 取消 Quest(会触发检查点保存)
    Cancel {
        /// Quest ID
        id: String,
    },
    /// 为 Quest 创建检查点(对应 LHQP 长期持久化)
    Checkpoint {
        /// Quest ID
        id: String,
    },
}

/// Config 子命令动作
#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// 生成默认 omega.yaml 到指定路径(默认 ~/.aether/omega.yaml)
    Init,
    /// 列出当前生效的配置项(键值对形式)
    List,
    /// 显示完整配置(JSON 格式,便于脚本消费)
    Show,
    /// 显示配置文件路径(实际加载的文件)
    Path,
}

/// MCP 子命令动作
#[derive(Subcommand, Debug)]
pub enum McpAction {
    /// 列出所有已注册的 MCP 服务器
    List,
    /// 启动 MCP 服务器(暴露 L10 mcp-mesh 入口)
    ///
    /// WHY 当前为 NotImplemented:真实服务器启动需要绑定网络端口、
    /// 加载 TLS 证书等配置,不适合 CLI 一次性启动。
    /// 生产环境请使用 `chimera tui` 或独立部署 mcp-mesh 服务。
    Serve,
    /// 调用指定 MCP 服务器的工具
    ///
    /// 触发 permission prompt(除非 `--yes` / `--no-permission`),
    /// 确认后通过 `McpMesh::execute_transaction` 调用工具。
    Call {
        /// 服务器 ID
        server: String,
        /// 工具名称
        tool: String,
        /// 工具参数(可变参数,JSON 字符串形式)
        #[arg(num_args = 0..)]
        args: Vec<String>,
    },
    /// 检查 MCP 服务器详情(注册时间 / 心跳 / 支持工具清单)
    Inspect {
        /// 服务器 ID
        server: String,
    },
}

/// Agent 子命令动作
#[derive(Subcommand, Debug)]
pub enum AgentAction {
    /// 列出所有 Agent(四象限分工 + 状态 + 当前任务)
    List,
    /// 在指定象限创建 Agent(调用 `chimera-mas::RootOrchestrator::delegate`)
    ///
    /// `--quadrant` 接受 Q1/Q2/Q3/Q4 或 Implementation/Integration/Verification/Hardening
    Spawn {
        /// 四象限分工(Q1/Q2/Q3/Q4 或完整名称)
        #[arg(long)]
        quadrant: String,
        /// 任务描述
        #[arg(long)]
        task: String,
    },
    /// 检查 Agent 详情(编制 / 当前任务 / 上下文预算 / 历史决策)
    Inspect {
        /// Agent ID
        id: String,
    },
    /// 取消 Agent 任务(触发 permission prompt,除非 `--yes`)
    Cancel {
        /// Agent ID
        id: String,
    },
}

/// LLM 子命令动作(Task 2 of spec)
#[derive(Subcommand, Debug)]
pub enum LlmAction {
    /// 列出已配置 Provider
    List,
    /// 显示 Provider 详情(endpoint / 协议 / 配额)
    Show {
        /// Provider 名称(如 deepseek / zhipu)
        name: String,
    },
    /// 设置默认 Provider(触发 permission prompt,除非 `--yes`)
    SetDefault {
        /// Provider 名称
        name: String,
    },
    /// 探测 Provider 连通性(发送最小化 prompt "ping")
    Test {
        /// Provider 名称(默认使用 default_provider)
        name: Option<String>,
    },
    /// 列出 model-router 4 路由渠道(Quality / Balanced / Cost / Speed)
    Channels,
    /// 显示/设置 model-router 策略
    Strategy {
        /// 策略名(CostOptimized / SpeedOptimized / QualityOptimized / Auto / Failover)
        strategy: Option<String>,
    },
}
