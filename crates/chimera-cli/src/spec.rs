//! 命令装配分类 — 编译期穷尽的单一分派表
//!
//! WHY 独立模块：`commands::dispatch` 需要在装配 `AppContext` 之前判断
//! "本命令是否真的需要它"。把分类放在独立模块并用无通配 `match`，
//! 新增 `Commands` 变体时编译器会强制补分类，避免"新命令默认付全量装配税"。
//!
//! 本模块现回答两个**正交**问题（工具链性能与并行调度重构 D1/D5）：
//! 1. 是否需要 `AppContext`，以及需要什么分片策略（[`CommandPlan`]）；
//! 2. 是否需要完整 [`crate::config::ChimeraConfig`]（[`needs_full_config`]）。
//!
//! WHY 把"分片策略"并入 `CommandPlan`：`composition::build` 内部会经
//! `GqepExecutor::new` / `SccCache::new` 无条件 `try_enable_sharding`，one-shot
//! 命令（run/exec/quest/…）为此白付 64 个常驻 worker + 16384 事件槽的启动税。
//! 把"该命令是否为长驻宿主"的判据放在这张穷尽表里，编译器保证新命令被显式归类。

use crate::cli::{Commands, ConfigAction};

/// 命令所需的装配级别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandClass {
    /// 不需要 AppContext：help / completions / config / cost / wiki / grep / audit / llm / mcp
    NoContext,
    /// 需要完整 AppContext：run / exec / chat / tui / quest / parliament / agent / doctor / serve / acp / None
    FullContext,
}

/// 事件分片策略（宿主感知，见性能重构 D1）。
///
/// - [`ShardingPolicy::Auto`]：长驻宿主（TUI / Chat / Serve / Acp）保留分片扇出，
///   以吸收持续高频 `Unordered` 事件。
/// - [`ShardingPolicy::Off`]：one-shot 命令屏蔽 leaf 的 `try_enable_sharding`，
///   进程内事件走既有单流 broadcast（漏发率语义不变，仅关并行扇出）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShardingPolicy {
    /// 屏蔽 leaf opt-in（one-shot：低事件量、秒级退出，64 worker 纯税）
    Off,
    /// 允许 leaf opt-in（长驻宿主：多发布者 / 持续事件流）
    Auto,
}

/// 命令装配计划：[`CommandClass`] 的细化，额外携带分片策略载荷。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandPlan {
    /// 不需要 AppContext（分片策略无消费者，省略）
    NoContext,
    /// 需要 AppContext + 指定分片策略
    FullContext {
        /// 该命令装配 `AppContext` 时应采用的事件分片策略（D1）
        sharding: ShardingPolicy,
    },
}

impl CommandPlan {
    /// 降级为 [`CommandClass`]（兼容既有只关心"要不要 AppContext"的调用面）。
    #[must_use]
    pub fn class(self) -> CommandClass {
        match self {
            Self::NoContext => CommandClass::NoContext,
            Self::FullContext { .. } => CommandClass::FullContext,
        }
    }
}

/// 唯一穷尽分派表：新增 `Commands` 变体时编译器强制补全分类 + 策略。
///
/// `None` 表示无子命令，`main` 会进入默认 TUI，故为 `FullContext { Auto }`。
#[must_use]
pub fn plan(cmd: Option<&Commands>) -> CommandPlan {
    use Commands::*;

    match cmd {
        // 长驻宿主：多发布者 / 持续事件流 → Auto
        None | Some(Chat) | Some(Tui { .. }) | Some(Serve) | Some(Acp) => {
            CommandPlan::FullContext {
                sharding: ShardingPolicy::Auto,
            }
        }
        // one-shot：秒级退出 / 低事件量 → Off（不付 64 worker 启动税）
        Some(Run { .. })
        | Some(Exec { .. })
        | Some(Quest { .. })
        | Some(Parliament { .. })
        | Some(Agent { .. })
        | Some(Doctor { .. }) => CommandPlan::FullContext {
            sharding: ShardingPolicy::Off,
        },
        // ADR-273：`mcp call` 需要 AppContext —— 复用组合根唯一 ToolRuntime 的
        // policy+sandbox 两环做准入前置（此前命令面直调 mesh，无策略/沙箱）。
        // one-shot（秒级退出）⇒ Off。必须排在下面的 `Mcp { .. }` 之前。
        Some(Commands::Mcp {
            action: crate::cli::McpAction::Call { .. },
        }) => CommandPlan::FullContext {
            sharding: ShardingPolicy::Off,
        },
        // 不装配 AppContext → 分片策略无意义
        Some(
            Help { .. }
            | Completions { .. }
            | Config { .. }
            | Cost
            | Wiki { .. }
            | Grep { .. }
            | Audit { .. }
            | Llm { .. }
            | Mcp { .. },
        ) => CommandPlan::NoContext,
    }
}

/// 对命令分类（装配级别）。
///
/// 兼容既有调用面：等价 `plan(cmd).class()`。新代码优先直接用 [`plan`] 获取策略。
#[must_use]
pub fn classify(cmd: Option<&Commands>) -> CommandClass {
    plan(cmd).class()
}

/// 是否需要完整 [`crate::config::ChimeraConfig`]（与是否需要 AppContext **正交**）。
///
/// `Help` / `Completions` / `Config(Init|Path)` 四类命令不消费任何配置字段
/// （见 `commands/{help,completions,config}.rs`），无需付全量 14 段 Figment 解析税；
/// `Config(List|Show)` 需要真实配置故返回 `true`。
#[must_use]
pub fn needs_full_config(cmd: Option<&Commands>) -> bool {
    use Commands::*;
    !matches!(
        cmd,
        Some(Help { .. })
            | Some(Completions { .. })
            | Some(Config {
                action: ConfigAction::Init | ConfigAction::Path
            })
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::cli::{AgentAction, ConfigAction, LlmAction, McpAction, QuestAction};

    /// 构造全部 19 个子命令 + None，逐一断言分类。
    ///
    /// WHY 逐个覆盖：`classify` 的编译期穷尽只保证"每个变体被提到"，
    /// 不保证"分到正确的类"；本测试锁定后者。
    #[test]
    fn classify_covers_every_variant() {
        let cases: Vec<(Option<Commands>, CommandClass)> = vec![
            (None, CommandClass::FullContext),
            (
                Some(Commands::Help { command: None }),
                CommandClass::NoContext,
            ),
            (
                Some(Commands::Completions {
                    shell: clap_complete::Shell::Bash,
                }),
                CommandClass::NoContext,
            ),
            (
                Some(Commands::Config {
                    action: ConfigAction::Path,
                }),
                CommandClass::NoContext,
            ),
            (Some(Commands::Cost), CommandClass::NoContext),
            (
                Some(Commands::Wiki {
                    query: "x".into(),
                    json: false,
                    limit: 10,
                }),
                CommandClass::NoContext,
            ),
            (
                Some(Commands::Grep {
                    pattern: "x".into(),
                    json: false,
                }),
                CommandClass::NoContext,
            ),
            (
                Some(Commands::Audit {
                    json: false,
                    severity: None,
                }),
                CommandClass::NoContext,
            ),
            (
                Some(Commands::Llm {
                    action: LlmAction::List,
                }),
                CommandClass::NoContext,
            ),
            (
                Some(Commands::Mcp {
                    action: McpAction::List,
                }),
                CommandClass::NoContext,
            ),
            // ADR-273：call 需 AppContext（准入前置）；inspect 仍零装配
            (
                Some(Commands::Mcp {
                    action: McpAction::Call {
                        server: "s".into(),
                        tool: "t".into(),
                        args: vec![],
                        network: false,
                    },
                }),
                CommandClass::FullContext,
            ),
            (
                Some(Commands::Mcp {
                    action: McpAction::Inspect { server: "s".into() },
                }),
                CommandClass::NoContext,
            ),
            (
                Some(Commands::Run {
                    prompt: "x".into(),
                    execute: false,
                }),
                CommandClass::FullContext,
            ),
            (
                Some(Commands::Exec {
                    prompt: Some("x".into()),
                    plan: None,
                    plan_concurrency: 8,
                    op_timeout_ms: None,
                    plan_timeout_ms: None,
                    plan_strict: false,
                    plan_readonly: false,
                    plan_max_steps: None,
                    plan_budget_bytes: None,
                }),
                CommandClass::FullContext,
            ),
            (Some(Commands::Chat), CommandClass::FullContext),
            (
                Some(Commands::Tui {
                    no_v3_engine: false,
                    protocol: false,
                }),
                CommandClass::FullContext,
            ),
            (
                Some(Commands::Quest {
                    action: QuestAction::List,
                    json: false,
                }),
                CommandClass::FullContext,
            ),
            (
                Some(Commands::Parliament {
                    proposal: "x".into(),
                    json: false,
                }),
                CommandClass::FullContext,
            ),
            (
                Some(Commands::Agent {
                    action: AgentAction::List,
                    parallel: false,
                }),
                CommandClass::FullContext,
            ),
            (
                Some(Commands::Doctor {
                    json: false,
                    fix: false,
                }),
                CommandClass::FullContext,
            ),
            (Some(Commands::Serve), CommandClass::FullContext),
            (Some(Commands::Acp), CommandClass::FullContext),
        ];

        for (cmd, expected) in cases {
            assert_eq!(
                classify(cmd.as_ref()),
                expected,
                "命令 {:?} 的分类错误",
                cmd
            );
        }
    }

    /// `plan` 必须与 `classify` 一致，并为每个 FullContext 变体指定正确分片策略。
    ///
    /// WHY 单独锁定：分片策略是本次性能重构的核心行为（D1），`classify` 会丢弃
    /// 策略载荷，故必须有直接针对 `plan` 的断言，防止 one-shot/长驻归类被改错。
    #[test]
    fn plan_maps_hosts_to_auto_and_one_shot_to_off() {
        // 长驻宿主 → Auto
        let auto_cases: Vec<Option<Commands>> = vec![
            None,
            Some(Commands::Chat),
            Some(Commands::Tui {
                no_v3_engine: false,
                protocol: false,
            }),
            Some(Commands::Serve),
            Some(Commands::Acp),
        ];
        for cmd in auto_cases {
            assert_eq!(
                plan(cmd.as_ref()),
                CommandPlan::FullContext {
                    sharding: ShardingPolicy::Auto
                },
                "长驻宿主 {cmd:?} 应为 Auto"
            );
        }
        // one-shot → Off
        let off_cases: Vec<Option<Commands>> = vec![
            Some(Commands::Run {
                prompt: "x".into(),
                execute: false,
            }),
            Some(Commands::Exec {
                prompt: Some("x".into()),
                plan: None,
                plan_concurrency: 8,
                op_timeout_ms: None,
                plan_timeout_ms: None,
                plan_strict: false,
                plan_readonly: false,
                plan_max_steps: None,
                plan_budget_bytes: None,
            }),
            Some(Commands::Quest {
                action: QuestAction::List,
                json: false,
            }),
            Some(Commands::Parliament {
                proposal: "x".into(),
                json: false,
            }),
            Some(Commands::Agent {
                action: AgentAction::List,
                parallel: false,
            }),
            Some(Commands::Doctor {
                json: false,
                fix: false,
            }),
            // ADR-273：mcp call 是 one-shot FullContext ⇒ Off
            Some(Commands::Mcp {
                action: McpAction::Call {
                    server: "s".into(),
                    tool: "t".into(),
                    args: vec![],
                    network: false,
                },
            }),
        ];
        for cmd in off_cases {
            assert_eq!(
                plan(cmd.as_ref()),
                CommandPlan::FullContext {
                    sharding: ShardingPolicy::Off
                },
                "one-shot {cmd:?} 应为 Off"
            );
        }
        // NoContext → 无策略载荷
        assert_eq!(
            plan(Some(&Commands::Help { command: None })),
            CommandPlan::NoContext
        );
    }

    /// `plan().class()` 必须与 `classify()` 逐命令一致（委托不偏离）。
    #[test]
    fn plan_class_agrees_with_classify() {
        let cmds: Vec<Option<Commands>> = vec![
            None,
            Some(Commands::Help { command: None }),
            Some(Commands::Cost),
            Some(Commands::Run {
                prompt: "x".into(),
                execute: false,
            }),
            Some(Commands::Chat),
            Some(Commands::Serve),
            Some(Commands::Mcp {
                action: McpAction::Call {
                    server: "s".into(),
                    tool: "t".into(),
                    args: vec![],
                    network: false,
                },
            }),
        ];
        for cmd in cmds {
            assert_eq!(
                plan(cmd.as_ref()).class(),
                classify(cmd.as_ref()),
                "plan().class() 与 classify() 对 {cmd:?} 不一致"
            );
        }
    }

    /// `needs_full_config`：仅 help/completions/config(init|path) 返回 false。
    #[test]
    fn needs_full_config_only_for_consuming_commands() {
        // false 四类
        assert!(!needs_full_config(Some(&Commands::Help { command: None })));
        assert!(!needs_full_config(Some(&Commands::Completions {
            shell: clap_complete::Shell::Bash,
        })));
        assert!(!needs_full_config(Some(&Commands::Config {
            action: ConfigAction::Init
        })));
        assert!(!needs_full_config(Some(&Commands::Config {
            action: ConfigAction::Path
        })));
        // true（含需真实配置的 Config(List/Show) 与全部 FullContext）
        assert!(needs_full_config(Some(&Commands::Config {
            action: ConfigAction::List
        })));
        assert!(needs_full_config(Some(&Commands::Config {
            action: ConfigAction::Show
        })));
        assert!(needs_full_config(Some(&Commands::Cost)));
        assert!(needs_full_config(Some(&Commands::Quest {
            action: QuestAction::List,
            json: false,
        })));
        assert!(needs_full_config(None));
    }
}
