//! 斜杠命令解析器 — Concord W2 · T2.1(命令三分层分流)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **纯函数、无状态**:解析与分流不持有状态,输入 = (文本, 注册表),
//!   输出 = `ParsedSlash`/`DispatchPlan`,便于单测穷举与 proptest 属性验证。
//! - **两词命令优先**:命令词允许空格子命令(`quest pause q-1`),先试
//!   两词精确匹配再试单词,避免 `quest` 截获 `quest pause`。
//! - **未命中回退 Legacy**:不在命令表中的输入交给遗留命令栏解析器
//!   (`CommandPalette::parse_command`),保证 `:` 废弃窗口期零功能断裂。
//! - **诚实反馈**:已登记但尚未接线的命令(如 `/compact` 依赖 W3+ 后端)
//!   分流到 `SlashEffect::HonestTodo`,由执行层给出诚实状态提示,不伪造功能。

use crate::actions::slash_registry::{SlashCommandDesc, SlashCommandRegistry, SlashTier};

/// 斜杠输入解析结果
#[derive(Debug, PartialEq)]
pub enum ParsedSlash<'a> {
    /// 命中命令表(精确或别名),携带参数串(已 trim,可为空)
    Command {
        /// 命中的命令描述(借用注册表)
        desc: &'a SlashCommandDesc,
        /// 命令词之后的参数串
        args: String,
    },
    /// 未命中命令表 — 回退遗留命令栏解析(不丢功能)
    Legacy(String),
    /// 空输入(仅 `/` 或空白)
    Empty,
}

/// 解析斜杠/冒号前缀的命令文本
///
/// # 参数
/// - `input`:原始输入(可含前导 `/` 或 `:`;`:` 为废弃窗口期别名)
/// - `reg`:斜杠命令注册表(单一事实源,W1 T1.1 落地)
///
/// # 返回
/// 见 [`ParsedSlash`]。两词命令(如 `quest pause`)优先于单词匹配。
pub fn parse<'a>(input: &str, reg: &'a SlashCommandRegistry) -> ParsedSlash<'a> {
    let stripped = input
        .strip_prefix('/')
        .or_else(|| input.strip_prefix(':'))
        .unwrap_or(input);
    // trim 仅用于空判定与 Legacy 字符串;切词直接在未 trim 串上
    // split_whitespace(自带首尾空白跳过,避免 trim→split 冗余链)
    let body = stripped.trim();
    if body.is_empty() {
        return ParsedSlash::Empty;
    }

    let tokens: Vec<&str> = stripped.split_whitespace().collect();
    // 两词命令优先:先试 "quest pause" 再试 "quest"
    if tokens.len() >= 2 {
        let two_word = format!("{} {}", tokens[0], tokens[1]);
        if let Some(desc) = reg.get(&two_word) {
            return ParsedSlash::Command {
                desc,
                args: tokens[2..].join(" "),
            };
        }
    }
    if let Some(desc) = reg.get(tokens[0]) {
        return ParsedSlash::Command {
            desc,
            args: tokens[1..].join(" "),
        };
    }
    ParsedSlash::Legacy(body.to_string())
}

/// 本地即时效果 — Instant 层命令的副作用枚举(执行层在 event_loop 落地)
#[derive(Debug, PartialEq)]
pub enum SlashEffect {
    /// 关键字过滤(Some=设置,None=清除)——承接原 EnterSearch 语义
    Search(Option<String>),
    /// 主题过滤(参数合法性由执行层校验)
    FilterTopic(String),
    /// 级别过滤(参数合法性由执行层校验)
    FilterLevel(String),
    /// 主题循环(与 `t` 键同效)
    ThemeCycle,
    /// 布局循环(与 `l` 键 view.switch_layout 同效)
    LayoutCycle,
    /// 按名称切换面板(执行层做名称→PanelId 模糊解析)
    PanelByName(String),
    /// 切换视图模式(Concord W3 T3.4:/chat /dashboard 改接视图模式)
    SwitchView(crate::types::ViewMode),
    /// 专注视图(Concord W4 T4.5:/focus —— Dashboard 切 SinglePane,
    /// Chat 态诚实提示已聚焦)
    FocusView,
    /// 中英切换(与 Ctrl+L 同效)
    LocaleToggle,
    /// 打开帮助面板
    Help,
    /// 状态总览(诚实汇总当前可读状态)
    Status,
    /// 配置健康自检(诚实汇总配置要点)
    Doctor,
    /// 退出 TUI
    Exit,
    /// 已登记未接线 — 诚实反馈(不伪造功能)
    HonestTodo,
}

/// 分流计划 — 解析后的执行路由
#[derive(Debug, PartialEq)]
pub enum DispatchPlan {
    /// Instant 层:本地即时效果
    Instant(SlashEffect),
    /// Orchestrated 层:桥接遗留命令栏同义命令(保留确认弹窗等既有行为)
    LegacyFallback(String),
    /// Agent 层:提示词模板 i18n 键(执行层预置进 composer)
    AgentTemplate(&'static str),
}

/// 按命令三分层生成分流计划
///
/// # 参数
/// - `desc`:命中的命令描述
/// - `args`:参数串(trim 后)
///
/// # 分流规则
/// - Instant:已接线的本地效果逐一映射;未接线的 → `HonestTodo`
/// - Orchestrated:quest 控制类桥接遗留命令(`pause/resume/vote/quest cancel/
///   priority`),复用既有确认弹窗与参数校验;其余编排命令 → `HonestTodo`
/// - Agent:提示词模板入 composer
pub fn plan(desc: &SlashCommandDesc, args: &str) -> DispatchPlan {
    // 冲突消解(W2 遗留兼容):会话域 /resume 与遗留 `resume <quest-id>` 同名——
    // 带参时几乎必为 quest 恢复意图,桥接遗留路径(保留确认弹窗);
    // 无参时保持会话恢复语义(未接线 → 诚实反馈)。
    if desc.name == "resume" && !args.is_empty() {
        return DispatchPlan::LegacyFallback(format!("resume {args}"));
    }
    match desc.tier {
        SlashTier::Instant => DispatchPlan::Instant(instant_effect(desc.name, args)),
        SlashTier::Orchestrated => orchestrated_plan(desc.name, args),
        SlashTier::Agent => DispatchPlan::AgentTemplate(agent_template_key(desc.name)),
    }
}

/// Instant 层命令名 → 本地效果映射
fn instant_effect(name: &str, args: &str) -> SlashEffect {
    match name {
        // 搜索迁移:/search <kw> 承接原 EnterSearch;空参清除
        "search" => {
            if args.is_empty() {
                SlashEffect::Search(None)
            } else {
                SlashEffect::Search(Some(args.to_lowercase()))
            }
        }
        "filter" => SlashEffect::FilterTopic(args.to_string()),
        "level" => SlashEffect::FilterLevel(args.to_string()),
        "theme" => SlashEffect::ThemeCycle,
        "layout" => SlashEffect::LayoutCycle,
        "panel" => SlashEffect::PanelByName(args.to_string()),
        // W2 过渡:ChatMode 属 W3;此前 /chat 映射为 Chat 面板切换(既承接
        // 遗留 `:chat` 面板词,又是当前最接近的诚实行为)
        "chat" => SlashEffect::SwitchView(crate::types::ViewMode::Chat),
        "dashboard" => SlashEffect::SwitchView(crate::types::ViewMode::Dashboard),
        "focus" => SlashEffect::FocusView,
        "locale" => SlashEffect::LocaleToggle,
        "help" => SlashEffect::Help,
        "status" => SlashEffect::Status,
        "doctor" => SlashEffect::Doctor,
        "exit" => SlashEffect::Exit,
        // 已登记未接线(vim/statusline/mention/debug-config/
        // new/clear/rename/export/diff/undo/redo/model/mode/plan/permissions 等;
        // Concord W4:shell 直通经 submit_slash 前置 HonestTodo 处理)
        _ => SlashEffect::HonestTodo,
    }
}

/// Orchestrated 层分流:quest 控制类桥接遗留命令,其余诚实反馈
fn orchestrated_plan(name: &str, args: &str) -> DispatchPlan {
    let legacy = match name {
        // 遗留命令栏同义命令(保留确认弹窗与参数校验路径)
        "quest pause" => Some(format!("pause {args}")),
        "quest resume" => Some(format!("resume {args}")),
        "quest cancel" => Some(format!("quest cancel {args}")),
        "quest priority" => Some(format!("quest priority {args}")),
        "quest vote" => Some(format!("vote {args}")),
        _ => None,
    };
    match legacy {
        Some(cmd) => DispatchPlan::LegacyFallback(cmd.trim().to_string()),
        // list/show/checkpoint/agent 系列等编排命令待后端通道接线(W3+)
        None => DispatchPlan::Instant(SlashEffect::HonestTodo),
    }
}

/// Agent 层命令 → 提示词模板 i18n 键
fn agent_template_key(name: &str) -> &'static str {
    match name {
        "review" => "slash.agent_template.review",
        "init" => "slash.agent_template.init",
        "side" => "slash.agent_template.side",
        // feedback 及未来 agent 命令统一回落到 side 模板语义(旁路问答)
        _ => "slash.agent_template.feedback",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn reg() -> SlashCommandRegistry {
        SlashCommandRegistry::with_builtin_commands()
    }

    // === parse 基础行为 ===

    #[test]
    fn parse_strips_slash_and_colon_prefixes() {
        let r = reg();
        assert_eq!(
            parse("/theme", &r),
            parse("theme", &r),
            "斜杠前缀应与无前缀等价"
        );
        assert_eq!(
            parse(":theme", &r),
            parse("theme", &r),
            "冒号前缀(废弃窗口别名)应与无前缀等价"
        );
    }

    #[test]
    fn parse_empty_input() {
        let r = reg();
        assert_eq!(parse("/", &r), ParsedSlash::Empty);
        assert_eq!(parse(":", &r), ParsedSlash::Empty);
        assert_eq!(parse("   ", &r), ParsedSlash::Empty);
        assert_eq!(parse("", &r), ParsedSlash::Empty);
    }

    #[test]
    fn parse_single_word_command_with_args() {
        let r = reg();
        match parse("/search hello world", &r) {
            ParsedSlash::Command { desc, args } => {
                assert_eq!(desc.name, "search");
                assert_eq!(args, "hello world");
            }
            other => panic!("应命中 search 命令,got {other:?}"),
        }
    }

    #[test]
    fn parse_two_word_command_takes_priority() {
        let r = reg();
        // "quest pause q-1":两词命令优先,不被单词分支截获
        match parse("/quest pause q-1", &r) {
            ParsedSlash::Command { desc, args } => {
                assert_eq!(desc.name, "quest pause");
                assert_eq!(args, "q-1");
            }
            other => panic!("应命中两词命令 quest pause,got {other:?}"),
        }
        // 裸 "quest" 不在表内(面板切换为遗留命令)→ Legacy
        assert!(matches!(parse("/quest", &r), ParsedSlash::Legacy(_)));
    }

    #[test]
    fn parse_alias_hits_canonical_command() {
        let r = reg();
        match parse("/quit", &r) {
            ParsedSlash::Command { desc, .. } => assert_eq!(desc.name, "exit"),
            other => panic!("别名 quit 应命中 exit,got {other:?}"),
        }
        match parse("/find foo", &r) {
            ParsedSlash::Command { desc, args } => {
                assert_eq!(desc.name, "search");
                assert_eq!(args, "foo");
            }
            other => panic!("别名 find 应命中 search,got {other:?}"),
        }
    }

    #[test]
    fn parse_unknown_command_falls_to_legacy() {
        let r = reg();
        assert_eq!(
            parse("/frobnicate x", &r),
            ParsedSlash::Legacy("frobnicate x".to_string())
        );
        // 遗留裸词同样回退(parse_command 继续负责面板切换词)
        assert_eq!(
            parse("budget", &r),
            ParsedSlash::Legacy("budget".to_string())
        );
    }

    // === plan 三分层分流 ===

    #[test]
    fn plan_instant_wired_commands() {
        let r = reg();
        let get = |name: &str| r.get(name).unwrap_or_else(|| panic!("{name} 应在表内"));
        assert_eq!(
            plan(get("search"), "Foo"),
            DispatchPlan::Instant(SlashEffect::Search(Some("foo".into())))
        );
        assert_eq!(
            plan(get("search"), ""),
            DispatchPlan::Instant(SlashEffect::Search(None))
        );
        assert_eq!(
            plan(get("theme"), ""),
            DispatchPlan::Instant(SlashEffect::ThemeCycle)
        );
        assert_eq!(
            plan(get("panel"), "Budget"),
            DispatchPlan::Instant(SlashEffect::PanelByName("Budget".into()))
        );
        assert_eq!(
            plan(get("help"), ""),
            DispatchPlan::Instant(SlashEffect::Help)
        );
        assert_eq!(
            plan(get("exit"), ""),
            DispatchPlan::Instant(SlashEffect::Exit)
        );
    }

    #[test]
    fn plan_instant_unwired_gives_honest_todo() {
        let r = reg();
        // 未接线的 instant 命令诚实反馈(/chat 已在 W2 过渡映射为面板切换)
        for name in ["vim", "statusline"] {
            let desc = r.get(name).unwrap_or_else(|| panic!("{name} 应在表内"));
            assert_eq!(
                plan(desc, ""),
                DispatchPlan::Instant(SlashEffect::HonestTodo),
                "{name} 未接线应诚实反馈"
            );
        }
    }

    #[test]
    fn plan_resume_conflict_resolution() {
        let r = reg();
        // 带参 resume → 桥接遗留 quest 恢复(保留确认弹窗,零功能断裂)
        assert_eq!(
            plan(r.get("resume").unwrap(), "q-9"),
            DispatchPlan::LegacyFallback("resume q-9".into())
        );
        // 无参 resume → 会话恢复语义(未接线诚实反馈)
        assert_eq!(
            plan(r.get("resume").unwrap(), ""),
            DispatchPlan::Instant(SlashEffect::HonestTodo)
        );
        // /chat W3 改接:切换视图模式(W2 过渡的 Chat 面板切换退役)
        assert_eq!(
            plan(r.get("chat").unwrap(), ""),
            DispatchPlan::Instant(SlashEffect::SwitchView(crate::types::ViewMode::Chat))
        );
        assert_eq!(
            plan(r.get("dashboard").unwrap(), ""),
            DispatchPlan::Instant(SlashEffect::SwitchView(crate::types::ViewMode::Dashboard))
        );
        // /focus W4 接线:专注视图效果
        assert_eq!(
            plan(r.get("focus").unwrap(), ""),
            DispatchPlan::Instant(SlashEffect::FocusView)
        );
    }

    #[test]
    fn plan_orchestrated_quest_control_bridges_legacy() {
        let r = reg();
        assert_eq!(
            plan(r.get("quest pause").unwrap(), "q-1"),
            DispatchPlan::LegacyFallback("pause q-1".into())
        );
        assert_eq!(
            plan(r.get("quest resume").unwrap(), "q-2"),
            DispatchPlan::LegacyFallback("resume q-2".into())
        );
        assert_eq!(
            plan(r.get("quest cancel").unwrap(), "q-3"),
            DispatchPlan::LegacyFallback("quest cancel q-3".into())
        );
        assert_eq!(
            plan(r.get("quest vote").unwrap(), "yes p-1"),
            DispatchPlan::LegacyFallback("vote yes p-1".into())
        );
        // 无遗留对等的编排命令 → 诚实反馈
        assert_eq!(
            plan(r.get("quest list").unwrap(), ""),
            DispatchPlan::Instant(SlashEffect::HonestTodo)
        );
        assert_eq!(
            plan(r.get("compact").unwrap(), ""),
            DispatchPlan::Instant(SlashEffect::HonestTodo)
        );
    }

    #[test]
    fn plan_agent_commands_carry_template_keys() {
        let r = reg();
        assert_eq!(
            plan(r.get("review").unwrap(), ""),
            DispatchPlan::AgentTemplate("slash.agent_template.review")
        );
        assert_eq!(
            plan(r.get("init").unwrap(), ""),
            DispatchPlan::AgentTemplate("slash.agent_template.init")
        );
        assert_eq!(
            plan(r.get("side").unwrap(), ""),
            DispatchPlan::AgentTemplate("slash.agent_template.side")
        );
        assert_eq!(
            plan(r.get("feedback").unwrap(), ""),
            DispatchPlan::AgentTemplate("slash.agent_template.feedback")
        );
    }

    // === proptest 属性 ===

    proptest! {
        /// 属性:前缀 strip 幂等 —— 对任意命令表条目,`/x`、`:x`、`x` 三形态
        /// 解析结果一致(命令翻转的等价性保证)。
        #[test]
        fn prefix_strip_equivalence(idx in 0usize..crate::actions::slash_registry::BUILTIN_COMMANDS.len()) {
            let name = crate::actions::slash_registry::BUILTIN_COMMANDS[idx].name;
            let r = reg();
            let a = parse(&format!("/{name}"), &r);
            let b = parse(&format!(":{name}"), &r);
            let c = parse(name, &r);
            prop_assert_eq!(&a, &b);
            prop_assert_eq!(&b, &c);
            match a {
                ParsedSlash::Command { desc, args } => {
                    prop_assert!(desc.name == name || desc.aliases.contains(&name));
                    prop_assert!(args.is_empty());
                }
                _ => prop_assert!(false, "内建命令 {name} 应命中"),
            }
        }

        /// 属性:参数保真 —— 任意参数串在解析后原样保留(trim/空白折叠除外)。
        #[test]
        fn args_preserved(arg in "[a-zA-Z0-9_\\- ]{0,40}") {
            let r = reg();
            let input = format!("/search {arg}");
            match parse(&input, &r) {
                ParsedSlash::Command { args, .. } => {
                    prop_assert_eq!(args, arg.split_whitespace().collect::<Vec<_>>().join(" "));
                }
                _ => prop_assert!(false, "/search 应命中"),
            }
        }
    }
}
