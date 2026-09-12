//! TUI 命令面板 — 底部输入栏渲染 + 遗留命令解析器
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 命令面板为无状态解析器:输入状态保存在 `TuiState` 中,
//!   便于面板与 `TuiApp` 统一访问。
//! - M3 扩展命令解析,支持 `:find`/`:filter`/`:level`/`:refresh` 等
//!   过滤器命令。
//! - IT-01(2026-09-08 批次-B):遗留 `InputMode::Command/Search` 交互入口
//!   (handle_key/submit)已删除——生产零 setter 的死路径;本文件保留
//!   render(Insert/Slash 底栏)与 parse_command/parse_legacy 解析器
//!   (斜杠回退 `:budget` B3 契约依赖)。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::actions::codegen::{palette_entries, PaletteEntry};
use crate::actions::ActionRegistry;
use crate::popup::Severity;
use crate::types::{InputMode, PanelId, TuiCommand, TuiState};
use event_bus::VoteValue;

/// 命令面板 — 解析并执行底部输入栏的命令
///
/// M1 为无状态结构体;未来可在此扩展命令历史、自动补全等状态。
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct CommandPalette;

impl CommandPalette {
    /// 创建新的命令面板
    pub fn new() -> Self {
        Self
    }

    /// 根据当前输入模式渲染底部输入栏
    pub fn render(&self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let (prefix, title) = match state.input_mode {
            // Concord W2:斜杠命令模式底部栏(补全列表由 slash_surface 渲染于上方)
            InputMode::Slash => (
                "/".to_string(),
                crate::t!("slash.surface.title").to_string(),
            ),
            // Insert(M3a):底部显示聊天输入行 `> {buffer}`(复用同一渲染路径)
            InputMode::Insert => {
                if let Some(pending) = &state.pending_action {
                    // F-5:palette 参数输入态——标题显示目标动作 i18n 标题,
                    // 明确"正在为此动作收集 query"(仅此态按帧查注册表,约 21 条,廉价)。
                    let title = ActionRegistry::with_builtin_domains()
                        .get(&pending.action_id)
                        .map(|d| format!(" {} ", crate::i18n::tr(d.title_key)))
                        .unwrap_or_else(|| format!(" {} ", pending.action_id));
                    ("> ".to_string(), title)
                } else {
                    ("> ".to_string(), " Chat ".to_string())
                }
            }
            InputMode::Normal => return,
        };

        let content = format!("{}{}", prefix, state.input_buffer);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .style(Style::default().fg(Color::Yellow));
        let paragraph = Paragraph::new(Line::from(content)).block(block);
        paragraph.render(area, buf);
    }

    /// 解析命令字符串
    ///
    /// 支持的命令(冒号可省略,因为进入命令模式时已输入冒号):
    /// - `quest`/`parliament`/`budget`/`memory`/`security`/`health`/`log`/`help`/`monitor`:切换面板
    /// - `quit`:退出应用
    /// - `find <keyword>`:设置关键字过滤器
    /// - `filter <topic>`:设置主题过滤器
    /// - `level <severity>`:设置级别过滤器
    /// - `refresh`:发布 `RefreshStateRequested` 控制请求事件,由上游决定是否重载/清空过滤器
    ///
    /// Concord W2:本解析器保留为遗留命令回退通道——斜杠解析未命中时经
    /// `parse_legacy` 桥接到此处,保证 `:` 废弃窗口期零功能断裂。
    /// IT-01(批次-B):交互入口 handle_key/submit 已删除(生产零 setter 死路径),
    /// 本解析器仅经 parse_legacy 由 Slash 回退调用。
    pub(crate) fn parse_legacy(input: &str, state: &mut TuiState) -> Option<TuiCommand> {
        Self::parse_command(input, state)
    }

    fn parse_command(input: &str, state: &mut TuiState) -> Option<TuiCommand> {
        let cmd = input.strip_prefix(':').unwrap_or(input).trim();
        if cmd.is_empty() {
            return None;
        }

        // 先处理无参数命令,避免被下面的 split 逻辑覆盖
        match cmd {
            "quest" => return Some(TuiCommand::SwitchPanel(PanelId::Quest)),
            "parliament" => return Some(TuiCommand::SwitchPanel(PanelId::Parliament)),
            "budget" => return Some(TuiCommand::SwitchPanel(PanelId::Budget)),
            "memory" => return Some(TuiCommand::SwitchPanel(PanelId::Memory)),
            "security" => return Some(TuiCommand::SwitchPanel(PanelId::Security)),
            "health" => return Some(TuiCommand::SwitchPanel(PanelId::Health)),
            "log" => return Some(TuiCommand::SwitchPanel(PanelId::Log)),
            "help" => return Some(TuiCommand::SwitchPanel(PanelId::Help)),
            "monitor" => return Some(TuiCommand::SwitchPanel(PanelId::ResourceMonitor)),
            "chat" => return Some(TuiCommand::SwitchPanel(PanelId::Chat)),
            "quit" => return Some(TuiCommand::Quit),
            "refresh" => return Some(TuiCommand::RequestRefresh),
            _ => {}
        }

        // 处理带参数命令
        let mut parts = cmd.splitn(2, ' ');
        let name = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim();

        match name {
            "find" => {
                if arg.is_empty() {
                    state.set_status("find requires an argument", Severity::Error);
                    return None;
                }
                state.filter_keyword = Some(arg.to_lowercase());
                None
            }
            "filter" => {
                if arg.is_empty() {
                    state.set_status("filter requires an argument", Severity::Error);
                    return None;
                }
                if is_valid_topic(arg) {
                    state.filter_topic = Some(arg.to_lowercase());
                    None
                } else {
                    state.set_status(
                        format!("invalid topic '{}': expected quest|security|memory|health|parliament|budget|system", arg),
                        Severity::Error,
                    );
                    None
                }
            }
            "level" => {
                if arg.is_empty() {
                    state.set_status("level requires an argument", Severity::Error);
                    return None;
                }
                let level = arg.to_lowercase();
                if matches!(level.as_str(), "info" | "warn" | "error" | "critical") {
                    state.filter_level = Some(level);
                    None
                } else {
                    state.set_status(
                        format!("invalid level '{}': expected info|warn|error|critical", arg),
                        Severity::Error,
                    );
                    None
                }
            }
            "pause" => {
                if arg.is_empty() {
                    state.set_status("pause requires a quest id", Severity::Error);
                    return None;
                }
                // M4 review fix:统一走 TuiCommand::RequestQuestPause,
                // 由 TuiApp::apply_command 负责弹出确认框,避免两条控制路径并存。
                Some(TuiCommand::RequestQuestPause(arg.to_string()))
            }
            "resume" => {
                if arg.is_empty() {
                    state.set_status("resume requires a quest id", Severity::Error);
                    return None;
                }
                Some(TuiCommand::RequestQuestResume(arg.to_string()))
            }
            "vote" => Self::parse_vote_command(arg, state),
            // Task 5:quest 子命令(cancel/priority)
            // WHY 委托到独立方法:`quest` 既是面板切换命令(无参数,已在上方 match 处理),
            // 又是子命令前缀(`quest cancel <id>`/`quest priority <id> <level>`)。
            // 此处 arg 非空(否则上方无参数 match 已拦截),交给子命令解析器分派。
            "quest" => Self::parse_quest_subcommand(arg, state),
            _ => {
                state.set_status(format!("unknown command '{}'", cmd), Severity::Error);
                None
            }
        }
    }

    /// 解析 `quest <subcommand> [args]` 子命令(Task 5)
    ///
    /// 支持的子命令:
    /// - `cancel <quest_id>`:请求取消 Quest(破坏性操作,由 `apply_command` 弹确认框)
    /// - `priority <quest_id> <level>`:调整优先级(level 为 0-255 整数,直接发布)
    ///
    /// WHY 独立方法:`quest` 作为子命令前缀需要二级 splitn 解析,
    /// 与单层参数命令(find/filter/level/pause/resume/vote)结构不同,
    /// 集中处理避免 parse_command 主体膨胀(§6.1 单函数 ≤200 行红线)。
    fn parse_quest_subcommand(arg: &str, state: &mut TuiState) -> Option<TuiCommand> {
        // arg 非空保证:parse_command 上方无参数 match 已拦截裸 `quest` 命令
        let mut parts = arg.splitn(2, ' ');
        let sub = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("").trim();

        match sub {
            "cancel" => {
                if rest.is_empty() {
                    state.set_status("quest cancel requires a quest id", Severity::Error);
                    return None;
                }
                Some(TuiCommand::RequestQuestCancel(rest.to_string()))
            }
            "priority" => Self::parse_quest_priority(rest, state),
            _ => {
                state.set_status(
                    format!("unknown quest subcommand '{}'", sub),
                    Severity::Error,
                );
                None
            }
        }
    }

    /// 解析 `quest priority <quest_id> <level>` 的参数(Task 5)
    ///
    /// level 必须是 0-255 的整数(u8 范围),`parse::<u8>()` 同时拒绝:
    /// - 非数字字符串(如 "abc")
    /// - 超出 u8 范围的数字(如 "999" > 255)
    ///
    /// WHY 用 u8 而非 u16 + 手动范围检查:u8 的 parse 天然实现 0-255 边界,
    /// 避免重复校验逻辑(§4.1 "避免防御性代码"原则)。
    fn parse_quest_priority(arg: &str, state: &mut TuiState) -> Option<TuiCommand> {
        if arg.is_empty() {
            state.set_status(
                "quest priority requires a quest id and level",
                Severity::Error,
            );
            return None;
        }

        let mut parts = arg.splitn(2, ' ');
        let quest_id = parts.next().unwrap_or("").trim();
        let level_str = parts.next().unwrap_or("").trim();

        if quest_id.is_empty() {
            state.set_status("quest priority requires a quest id", Severity::Error);
            return None;
        }

        if level_str.is_empty() {
            state.set_status("quest priority requires a level (0-255)", Severity::Error);
            return None;
        }

        match level_str.parse::<u8>() {
            Ok(level) => Some(TuiCommand::RequestQuestPriorityChange {
                quest_id: quest_id.to_string(),
                new_priority: level,
            }),
            Err(_) => {
                state.set_status(
                    format!("invalid priority '{}': expected 0-255 integer", level_str),
                    Severity::Error,
                );
                None
            }
        }
    }

    /// 解析 `:vote <yes|no|abstain> <proposal-id>` 命令
    ///
    /// 返回 `TuiCommand::RequestVote`;若参数非法则设置状态消息并返回 None。
    fn parse_vote_command(arg: &str, state: &mut TuiState) -> Option<TuiCommand> {
        if arg.trim().is_empty() {
            state.set_status(
                "vote requires a vote value and proposal id",
                Severity::Error,
            );
            return None;
        }

        let mut parts = arg.splitn(2, ' ');
        let vote_str = parts.next().unwrap_or("").trim();
        let proposal_id = parts.next().unwrap_or("").trim();

        let vote = match vote_str.parse::<VoteValue>() {
            Ok(v) => v,
            Err(()) => {
                state.set_status(
                    format!("invalid vote '{}': expected yes|no|abstain", vote_str),
                    Severity::Error,
                );
                return None;
            }
        };

        if proposal_id.is_empty() {
            state.set_status("vote requires a proposal id", Severity::Error);
            return None;
        }

        // M4 review fix:统一走 TuiCommand::RequestVote,由 TuiApp 负责确认弹窗。
        Some(TuiCommand::RequestVote {
            proposal_id: proposal_id.to_string(),
            vote,
        })
    }
}

/// 校验主题参数是否合法
///
/// Concord W2:`pub(crate)` 供斜杠执行层复用同一校验(避免双源漂移)。
pub(crate) fn is_valid_topic(topic: &str) -> bool {
    matches!(
        topic.to_lowercase().as_str(),
        "quest" | "security" | "memory" | "health" | "parliament" | "budget" | "system"
    )
}

// ============================================================
// CommandPaletteModel — 统一命令面板数据模型(v3.1 M1.5,用户北极星)
// ============================================================

/// 统一命令面板数据模型 — Registry 驱动的模糊检索面板
///
/// # 设计决策(WHY)
/// - **单一事实源驱动**:候选项由 `ActionRegistry` 经 `codegen::palette_entries`
///   生成,与斜杠命令/帮助同源,杜绝"所有命令集成于一个面板"时的清单漂移。
/// - **纯逻辑、不接线渲染/输入**:M1.5 只提供数据模型 + 状态迁移(query/选择),
///   渲染与 InputRouter 接线留 M2;便于单测穷举验证可发现性(§8.3)。
/// - **自持 Registry 副本**:`ActionRegistry` 派生 Clone(约 21 条描述,克隆廉价),
///   模型自持一份避免生命周期纠缠,重过滤时本地查询。
#[derive(Debug, Clone)]
pub struct CommandPaletteModel {
    /// 当前检索输入
    query: String,
    /// 当前 query 下的过滤结果(Registry 驱动)
    entries: Vec<PaletteEntry>,
    /// 当前选中项下标(钳制在 [0, entries.len()))
    selected: usize,
    /// 动作注册表(单一事实源)
    registry: ActionRegistry,
}

impl CommandPaletteModel {
    /// 以指定注册表构造(初始空 query,展示全部动作)
    pub fn new(registry: ActionRegistry) -> Self {
        let mut model = Self {
            query: String::new(),
            entries: Vec::new(),
            selected: 0,
            registry,
        };
        model.refilter();
        model
    }

    /// 以内建六域注册表构造(生产入口)
    pub fn with_builtin_domains() -> Self {
        Self::new(ActionRegistry::with_builtin_domains())
    }

    /// 打开面板:清空 query、复位选择、展示全部动作
    pub fn open(&mut self) {
        self.query.clear();
        self.selected = 0;
        self.refilter();
    }

    /// 追加一个检索字符并重新过滤
    pub fn on_input(&mut self, c: char) {
        self.query.push(c);
        self.selected = 0;
        self.refilter();
    }

    /// 删除末尾字符并重新过滤(退格)
    pub fn on_backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
        self.refilter();
    }

    /// 移动选择(down=true 下移,false 上移),两端钳制不回绕
    pub fn move_selection(&mut self, down: bool) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() - 1;
        self.selected = if down {
            (self.selected + 1).min(last)
        } else {
            self.selected.saturating_sub(1)
        };
    }

    /// 当前检索输入
    pub fn query(&self) -> &str {
        &self.query
    }

    /// 当前过滤结果
    pub fn entries(&self) -> &[PaletteEntry] {
        &self.entries
    }

    /// 当前选中项下标
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// 当前选中的条目(空结果时 None)
    pub fn selected_entry(&self) -> Option<&PaletteEntry> {
        self.entries.get(self.selected)
    }

    /// 当前选中的动作 id(供 Enter 执行 → DispatchAction)
    pub fn selected_action(&self) -> Option<&'static str> {
        self.selected_entry().map(|e| e.action_id)
    }

    /// 当前选中动作是否需要 query 参数(palette 参数输入流分流,F-5)
    ///
    /// 返回 true 时 Enter 应进入 Insert 参数收集态(提交后以 {"query": text}
    /// 派发);false 时维持既有空 payload 直发。缺省 false,行为零回归。
    pub fn selected_action_requires_query(&self) -> bool {
        self.selected_entry()
            .and_then(|e| self.registry.get(e.action_id))
            .map(|d| d.requires_query)
            .unwrap_or(false)
    }

    /// 按当前 query 重新过滤,并将选择下标钳制到有效范围
    fn refilter(&mut self) {
        self.entries = palette_entries(&self.registry, &self.query);
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_panel_commands() {
        assert_eq!(
            CommandPalette::parse_command("quest", &mut TuiState::new()),
            Some(TuiCommand::SwitchPanel(PanelId::Quest))
        );
        assert_eq!(
            CommandPalette::parse_command("parliament", &mut TuiState::new()),
            Some(TuiCommand::SwitchPanel(PanelId::Parliament))
        );
        assert_eq!(
            CommandPalette::parse_command("budget", &mut TuiState::new()),
            Some(TuiCommand::SwitchPanel(PanelId::Budget))
        );
        assert_eq!(
            CommandPalette::parse_command("memory", &mut TuiState::new()),
            Some(TuiCommand::SwitchPanel(PanelId::Memory))
        );
        assert_eq!(
            CommandPalette::parse_command("security", &mut TuiState::new()),
            Some(TuiCommand::SwitchPanel(PanelId::Security))
        );
        assert_eq!(
            CommandPalette::parse_command("health", &mut TuiState::new()),
            Some(TuiCommand::SwitchPanel(PanelId::Health))
        );
        assert_eq!(
            CommandPalette::parse_command("log", &mut TuiState::new()),
            Some(TuiCommand::SwitchPanel(PanelId::Log))
        );
        assert_eq!(
            CommandPalette::parse_command("help", &mut TuiState::new()),
            Some(TuiCommand::SwitchPanel(PanelId::Help))
        );
    }

    #[test]
    fn test_parse_quit_command() {
        assert_eq!(
            CommandPalette::parse_command("quit", &mut TuiState::new()),
            Some(TuiCommand::Quit)
        );
    }

    #[test]
    fn test_parse_unknown_command() {
        let mut state = TuiState::new();
        assert_eq!(CommandPalette::parse_command("foo", &mut state), None);
        assert!(
            state
                .status_message
                .as_ref()
                .unwrap()
                .0
                .contains("unknown command"),
            "status should report unknown command"
        );
    }

    #[test]
    fn test_parse_find_command() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("find error", &mut state);
        assert_eq!(cmd, None);
        assert_eq!(state.filter_keyword, Some("error".into()));
    }

    #[test]
    fn test_parse_filter_command_valid() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("filter security", &mut state);
        assert_eq!(cmd, None);
        assert_eq!(state.filter_topic, Some("security".into()));
    }

    #[test]
    fn test_parse_filter_command_invalid() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("filter foo", &mut state);
        assert_eq!(cmd, None);
        assert!(state.filter_topic.is_none());
        assert!(
            state
                .status_message
                .as_ref()
                .unwrap()
                .0
                .contains("invalid topic"),
            "status should report invalid topic"
        );
    }

    #[test]
    fn test_parse_level_command_valid() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("level critical", &mut state);
        assert_eq!(cmd, None);
        assert_eq!(state.filter_level, Some("critical".into()));
    }

    #[test]
    fn test_parse_level_command_invalid() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("level foo", &mut state);
        assert_eq!(cmd, None);
        assert!(state.filter_level.is_none());
        assert!(
            state
                .status_message
                .as_ref()
                .unwrap()
                .0
                .contains("invalid level"),
            "status should report invalid level"
        );
    }

    #[test]
    fn test_parse_refresh_command_returns_request() {
        let mut state = TuiState::new();
        state.filter_keyword = Some("foo".into());
        state.filter_topic = Some("security".into());
        state.filter_level = Some("critical".into());

        // M4:refresh 现在作为控制请求发布,由上游订阅者决定是否清空过滤器,
        // 命令面板本身不再直接修改过滤器状态。
        let cmd = CommandPalette::parse_command("refresh", &mut state);
        assert_eq!(cmd, Some(TuiCommand::RequestRefresh));
        assert!(state.filter_keyword.is_some());
        assert!(state.filter_topic.is_some());
        assert!(state.filter_level.is_some());
    }

    #[test]
    fn test_parse_missing_argument() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("find", &mut state);
        assert_eq!(cmd, None);
        assert!(
            state
                .status_message
                .as_ref()
                .unwrap()
                .0
                .contains("requires an argument"),
            "status should report missing argument"
        );
    }

    // ===== IT-01(批次-B):quest 子命令解析下沉(原 tests/command_palette_test.rs)=====
    //
    // 口径声明:原集成测试经 submit() 假绿范式(手工强制 InputMode::Command,
    // IT-03)驱动;交互入口删除后下沉为 parser 纯语法单测,经 parse_command
    // 直接驱动。submit 的"复位 Normal + 清缓冲"语义随交互入口一并退役——
    // `:` 遗留命令现由 Slash 模式回退经 parse_legacy 到达此处,模式管理归
    // Slash 路径所有。

    #[test]
    fn test_parse_quest_cancel_command() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("quest cancel quest-001", &mut state);
        assert_eq!(
            cmd,
            Some(TuiCommand::RequestQuestCancel("quest-001".to_string()))
        );
    }

    #[test]
    fn test_parse_quest_cancel_with_complex_id() {
        // 验证含连字符/数字的 quest_id 正常解析
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("quest cancel q-abc-123-xyz", &mut state);
        assert_eq!(
            cmd,
            Some(TuiCommand::RequestQuestCancel("q-abc-123-xyz".to_string()))
        );
    }

    #[test]
    fn test_quest_cancel_missing_id_shows_error() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("quest cancel", &mut state);
        assert_eq!(cmd, None);
        let (msg, sev) = state
            .status_message
            .expect("error status should be set for missing quest id");
        assert_eq!(sev, Severity::Error);
        assert!(
            msg.contains("quest id") || msg.contains("requires"),
            "status should report missing quest id, got: {msg}"
        );
    }

    #[test]
    fn test_parse_quest_priority_command() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("quest priority quest-001 200", &mut state);
        assert_eq!(
            cmd,
            Some(TuiCommand::RequestQuestPriorityChange {
                quest_id: "quest-001".to_string(),
                new_priority: 200,
            })
        );
    }

    #[test]
    fn test_parse_quest_priority_boundary_zero() {
        // 边界值 0(u8 下限)应接受
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("quest priority quest-001 0", &mut state);
        assert_eq!(
            cmd,
            Some(TuiCommand::RequestQuestPriorityChange {
                quest_id: "quest-001".to_string(),
                new_priority: 0,
            })
        );
    }

    #[test]
    fn test_parse_quest_priority_boundary_max() {
        // 边界值 255(u8 上限)应接受
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("quest priority quest-001 255", &mut state);
        assert_eq!(
            cmd,
            Some(TuiCommand::RequestQuestPriorityChange {
                quest_id: "quest-001".to_string(),
                new_priority: 255,
            })
        );
    }

    #[test]
    fn test_quest_priority_invalid_level_shows_error() {
        // 999 > 255(u8 上限),parse::<u8>() 必然失败
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("quest priority quest-001 999", &mut state);
        assert_eq!(cmd, None);
        let (msg, sev) = state
            .status_message
            .expect("error status should be set for invalid level");
        assert_eq!(sev, Severity::Error);
        assert!(
            msg.contains("invalid priority") || msg.contains("0-255"),
            "status should report invalid priority, got: {msg}"
        );
    }

    #[test]
    fn test_quest_priority_non_numeric_level_shows_error() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("quest priority quest-001 abc", &mut state);
        assert_eq!(cmd, None);
        let (msg, sev) = state
            .status_message
            .expect("error status should be set for non-numeric level");
        assert_eq!(sev, Severity::Error);
        assert!(
            msg.contains("invalid priority") || msg.contains("0-255"),
            "status should report invalid priority, got: {msg}"
        );
    }

    #[test]
    fn test_quest_priority_missing_level_shows_error() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("quest priority quest-001", &mut state);
        assert_eq!(cmd, None);
        let (msg, sev) = state
            .status_message
            .expect("error status should be set for missing level");
        assert_eq!(sev, Severity::Error);
        assert!(
            msg.contains("level") || msg.contains("requires"),
            "status should report missing level, got: {msg}"
        );
    }

    #[test]
    fn test_quest_priority_missing_all_args_shows_error() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("quest priority", &mut state);
        assert_eq!(cmd, None);
        assert_eq!(
            state.status_message.as_ref().map(|(_, sev)| *sev),
            Some(Severity::Error)
        );
    }

    #[test]
    fn test_quest_alone_still_switches_panel() {
        // `quest` 单独仍应切换到 Quest 面板,不被子命令逻辑拦截
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("quest", &mut state);
        assert_eq!(cmd, Some(TuiCommand::SwitchPanel(PanelId::Quest)));
    }

    #[test]
    fn test_quest_unknown_subcommand_shows_error() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("quest frobnicate quest-001", &mut state);
        assert_eq!(cmd, None);
        let (msg, sev) = state
            .status_message
            .expect("error status should be set for unknown subcommand");
        assert_eq!(sev, Severity::Error);
        assert!(
            msg.contains("unknown quest subcommand") || msg.contains("unknown command"),
            "status should report unknown subcommand, got: {msg}"
        );
    }

    // ===== CommandPaletteModel(M1.5 统一命令面板数据模型)=====

    #[test]
    fn palette_model_open_shows_all_actions() {
        let reg = ActionRegistry::with_builtin_domains();
        let n = reg.len();
        let model = CommandPaletteModel::new(reg);
        assert_eq!(model.entries().len(), n, "空 query 应展示全部动作");
        assert_eq!(model.query(), "");
    }

    #[test]
    fn palette_model_filters_by_query() {
        let mut model = CommandPaletteModel::with_builtin_domains();
        let all = model.entries().len();
        for c in "export".chars() {
            model.on_input(c);
        }
        assert!(model.entries().iter().any(|e| e.action_id == "export.run"));
        assert!(model.entries().len() < all, "过滤后应少于全量");
    }

    #[test]
    fn palette_model_backspace_restores_all() {
        let mut model = CommandPaletteModel::with_builtin_domains();
        let all = model.entries().len();
        // 输入不太可能匹配的字符,再逐一退格清空
        model.on_input('z');
        model.on_input('q');
        model.on_input('x');
        model.on_backspace();
        model.on_backspace();
        model.on_backspace();
        assert_eq!(model.query(), "");
        assert_eq!(model.entries().len(), all, "退格清空后应恢复全部");
    }

    #[test]
    fn palette_model_selection_clamps_at_both_ends() {
        let mut model = CommandPaletteModel::with_builtin_domains();
        // 上移到顶不越界
        model.move_selection(false);
        assert_eq!(model.selected_index(), 0);
        // 下移多次钳制在末项
        for _ in 0..100 {
            model.move_selection(true);
        }
        assert_eq!(model.selected_index(), model.entries().len() - 1);
        assert!(model.selected_action().is_some());
    }

    #[test]
    fn palette_model_every_action_discoverable_by_id() {
        // §8.3 可发现性:每个动作都能被其 id 在命令面板检索命中
        let reg = ActionRegistry::with_builtin_domains();
        let ids: Vec<&'static str> = reg.all().iter().map(|d| d.id).collect();
        for id in ids {
            let mut model = CommandPaletteModel::with_builtin_domains();
            for c in id.chars() {
                model.on_input(c);
            }
            assert!(
                model.entries().iter().any(|e| e.action_id == id),
                "动作 {id} 应能被其 id 在命令面板检索命中"
            );
        }
    }
}
