//! InputRouter — 输入路由状态机(ADR-029,v3.1 §4.3)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **提级为正式任务而非技术债**:一旦引入 Insert/Vim 模式与可交互面板,
//!   按键所有权(全局快捷键 vs 面板动作 vs 聊天原始输入)立即复杂化。
//!   显式三态路由表是交互式 TUI 的前置条件,集中决定"谁拥有这个按键"。
//! - **纯函数、无状态**:`route` 仅依据 (模式, 按键) 计算路由目标,不持有状态,
//!   便于 D 类快照测试穷举验证(每种模式 × 每组按键 → 断言目标)。
//! - **路由与语义分离**:路由器只决定按键归属(交给谁),具体业务语义由
//!   接收方(全局处理器 / 焦点面板 / 输入缓冲 / 命令面板)执行。
//! - **RouterMode 独立于 app 的 InputMode**:M0 阶段 `InputMode`(Normal/Command/
//!   Search)尚未扩展 Insert/Vim,路由器先用自有三态枚举表达路由概念,
//!   M2 接线时再桥接到扩展后的 `InputMode`,避免过早改动共享类型破坏既有序列化。

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::actions::ActionRegistry;
use crate::types::PanelId;

/// 路由模式 — 三态路由表的输入模式维度(§4.3)
///
/// WHY 三态:覆盖交互式 TUI 的全部按键归属场景——Normal 导航/触发、
/// Insert 原始文本输入、Command 命令面板检索。Vim 模式在 M2+ 作为 Normal
/// 的子模式扩展,当前不单列。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RouterMode {
    /// 普通模式:全局快捷键 / 焦点面板动作 / 焦点轮转 / 模式切换
    Normal,
    /// 输入模式:聊天/表单原始字符输入
    Insert,
    /// 命令模式:命令面板模糊搜索
    Command,
    /// g 前缀等待态(瞬态):`g` 之后的第二个键(gg/g1-6)在此模式解析。
    ///
    /// WHY 独立态而非 Normal 子逻辑:路由器无状态,两键序列(g + 次键)的
    /// "已按下 g"状态由调用方(app)以模式形式持有,次键经本模式解析。
    /// 注意:GPrefix 非用户可见输入模式,仅为路由的瞬态前缀态。
    GPrefix,
    /// Ctrl+W 前缀等待态(瞬态):`Ctrl+W` 之后的次键(h/j/k/l/w)在此模式解析。
    ///
    /// WHY 独立态:与 GPrefix 同理,"已按下 Ctrl+W"状态由 app 以 `w_prefix` 持有,
    /// 次键经本模式解析为方向窗格焦点或循环。
    WPrefix,
    /// 斜杠命令模式(Concord W2):`/` 补全输入态,与 Command 态同构的纯机械
    /// 路由(Esc/方向/Enter/Backspace/Char/Tab);业务分流由 slash_parser 承担。
    Slash,
}

/// 窗格方向(Ctrl+W 前缀方向导航)
///
/// WHY 独立枚举:方向导航目标与窗格几何相关,由 app 按矩形位置解析为最近邻
/// 窗格;路由器只表达"往哪个方向",不涉及几何。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneDir {
    /// 左
    Left,
    /// 右
    Right,
    /// 上
    Up,
    /// 下
    Down,
}

/// 路由目标 — 一个按键应交由谁处理(§4.3 按键归属)
///
/// WHY 枚举而非直接执行:路由器只表达"归属意图",由 `TuiApp` 统一执行,
/// 与 `Panel::handle_key -> Option<TuiCommand>` 的"面板表达意图、App 执行"
/// 设计一致,避免路由器直接操作全局状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteTarget {
    /// 全局快捷键触发的动作(携带 action_id,如 "system.toggle_locale")
    GlobalAction(&'static str),
    /// 进入指定路由模式(如 `:`/Ctrl+P 进入 Command,`i` 进入 Insert)
    EnterMode(RouterMode),
    /// 退出当前模式返回 Normal(Esc)
    ExitMode,
    /// 打开统一命令面板 overlay(Normal: Ctrl+P)
    ///
    /// WHY 独立于斜杠命令栏:overlay 是 Registry 驱动的"模糊选一个动作",
    /// 而 `/` 命令栏是"命令表三分层执行",二者交互不同,路由目标必须区分。
    OpenPalette,
    /// 打开焦点面板的上下文动作菜单 overlay(Normal: `a`)
    ///
    /// WHY 独立于 OpenPalette:命令面板是全局模糊搜索,本目标是"焦点面板精选动作"
    /// 的上下文菜单(§4.5),动作集由 `actions::panel_context_actions(焦点面板)` 决定,
    /// 需运行时焦点上下文,故不用静态 `GlobalAction`。
    OpenActionMenu,
    /// 进入斜杠命令栏(Concord W2:`/` 第一公民入口;`:` 废弃窗口期同进,
    /// app 据按键字符判定是否展示一次性弃用提示)—— 替代原 EnterCommandBar
    /// (vi 式 `:` 命令栏)与 EnterSearch(`/` 搜索)两个目标:搜索语义由
    /// `/search` 命令承接,遗留文本命令由 slash_parser 的 Legacy 回退承接。
    EnterSlash,
    /// Slash 模式:补全选中项的前缀补全(Concord W2:Tab 键)
    SlashComplete,
    /// Chat⇄Dashboard 视图模式互切(Concord W3 T3.4:`\` 键,方案 §7.4
    /// 复用原 companion 键;view.toggle_companion 保留但改经命令面板访问)
    ToggleViewMode,
    /// Insert 模式 @ 引用补全(Concord W4 T4.5:Tab 键;末尾词以 @ 起始时
    /// 补全为首个候选,否则无操作)
    MentionComplete,
    /// Insert 模式 composer 历史回溯(Concord W6 T6.2:↑ 上一条)
    HistoryPrev,
    /// Insert 模式 composer 历史前进(Concord W6 T6.2:↓ 下一条/回底恢复草稿)
    HistoryNext,
    /// 焦点轮转(Tab 正向 / Shift+Tab 反向)
    FocusCycle {
        /// true = 下一个面板,false = 上一个面板
        forward: bool,
    },
    /// 交由当前焦点面板处理(Enter 下钻 / Space 切换 / 列表导航等)
    FocusPanel,
    /// Ctrl+W 前缀方向导航:按方向切换活跃窗格(几何解析在 app)
    FocusPaneDir(PaneDir),
    /// Insert 模式:输入一个字符到输入缓冲
    InsertChar(char),
    /// Command 模式:输入一个字符到命令面板检索缓冲
    PaletteInput(char),
    /// Command 模式:移动候选选择(down=true 下移,false 上移)
    PaletteMove {
        /// true = 下移候选,false = 上移候选
        down: bool,
    },
    /// 删除输入缓冲末尾字符(Insert/Command 共用)
    Backspace,
    /// 提交当前内容(Insert 提交聊天 / Command 执行选中动作)
    Submit,
    /// 退出应用(Normal: q / Esc)
    Quit,
    /// 跳转到指定面板(数字键 / F 键 / g 前缀扩展面板)
    PanelJump(PanelId),
    /// 滚动当前面板到顶部(GPrefix: g g)
    ScrollTop,
    /// 滚动当前面板到底部(Normal: G)
    ScrollBottom,
    /// 循环切换主题(Normal: t)—— 主题为纯 UI 切换,非注册表动作
    ThemeCycle,
    /// 调整主面板显示比例(Normal: Ctrl+↑/Ctrl+↓)
    RatioAdjust {
        /// true = 增大主面板占比,false = 减小
        increase: bool,
    },
    /// 未路由 — 忽略此按键(如 Release 事件、无绑定的按键)
    Ignored,
}

/// 输入路由器 — 无状态,`route` 为纯函数
#[derive(Debug, Clone, Copy, Default)]
pub struct InputRouter;

impl InputRouter {
    /// 根据当前模式与按键计算路由目标(§4.3 三态路由表)
    ///
    /// # 优先级(高 → 低)
    /// - Normal:全局快捷键 → 模式切换 → 焦点轮转 → 焦点面板动作
    /// - Insert:Esc 退出 → 少数全局键(Ctrl+L)→ 提交/退格 → 原始字符
    /// - Command:Esc 关闭 → 上下选择 → 提交 → 退格 → 检索字符
    pub fn route(mode: RouterMode, key: KeyEvent) -> RouteTarget {
        // Windows crossterm 会触发 Release 事件,必须过滤避免重复响应
        // (§4.4 平台兼容性:仅处理 Press,Repeat 视同 Press 由调用方决定)
        if key.kind == KeyEventKind::Release {
            return RouteTarget::Ignored;
        }
        match mode {
            RouterMode::Normal => Self::route_normal(key),
            RouterMode::Insert => Self::route_insert(key),
            RouterMode::Command => Self::route_command(key),
            RouterMode::GPrefix => Self::route_gprefix(key),
            RouterMode::WPrefix => Self::route_wprefix(key),
            RouterMode::Slash => Self::route_slash(key),
        }
    }

    /// Normal 模式路由:退出 > 机械键(面板/滚动/比例/模式) > codegen 全局动作键 > 焦点面板
    ///
    /// WHY 完整覆盖:本表是 Normal 模式按键归属的单一事实源。Action 支持的
    /// 全局键(locale/help/export/layout/companion 等)不再手写分支,而是由
    /// `global_action_for` 从 ActionRegistry codegen 键位表派生(Concord T1.3,
    /// P5② 键位双源收口);声明在 `ActionDescriptor.default_key`/`alias_keys`,
    /// 机械键(退出/面板跳转/滚动/主题/比例)仍用专用目标变体。
    fn route_normal(key: KeyEvent) -> RouteTarget {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            // 退出(普通模式)
            KeyCode::Char('q') | KeyCode::Esc => RouteTarget::Quit,
            // 命令面板 overlay(非动作,专用目标)
            KeyCode::Char('p') if ctrl => RouteTarget::OpenPalette,
            // Ctrl+方向:主面板比例调整
            KeyCode::Up if ctrl => RouteTarget::RatioAdjust { increase: true },
            KeyCode::Down if ctrl => RouteTarget::RatioAdjust { increase: false },
            // 模式切换:Concord W2 — `/` 与 `:` 同进斜杠命令模式(命令翻转;
            // `:` 为废弃窗口期别名,app 侧展示一次性弃用提示,R1 缓解)
            KeyCode::Char(':') | KeyCode::Char('/') => RouteTarget::EnterSlash,
            KeyCode::Char('i') => RouteTarget::EnterMode(RouterMode::Insert),
            // g 前缀 / 滚动到底
            KeyCode::Char('g') => RouteTarget::EnterMode(RouterMode::GPrefix),
            KeyCode::Char('G') => RouteTarget::ScrollBottom,
            // 数字键 1-9:前 9 个面板跳转
            KeyCode::Char('1') => RouteTarget::PanelJump(PanelId::Quest),
            KeyCode::Char('2') => RouteTarget::PanelJump(PanelId::Parliament),
            KeyCode::Char('3') => RouteTarget::PanelJump(PanelId::Budget),
            KeyCode::Char('4') => RouteTarget::PanelJump(PanelId::Memory),
            KeyCode::Char('5') => RouteTarget::PanelJump(PanelId::Security),
            KeyCode::Char('6') => RouteTarget::PanelJump(PanelId::Health),
            KeyCode::Char('7') => RouteTarget::PanelJump(PanelId::Log),
            KeyCode::Char('8') => RouteTarget::PanelJump(PanelId::Help),
            KeyCode::Char('9') => RouteTarget::PanelJump(PanelId::Decay),
            // F1-F8:面板跳转(F4/F5 未映射,回退焦点面板)
            KeyCode::F(1) => RouteTarget::PanelJump(PanelId::Quest),
            KeyCode::F(2) => RouteTarget::PanelJump(PanelId::Parliament),
            KeyCode::F(3) => RouteTarget::PanelJump(PanelId::Budget),
            KeyCode::F(6) => RouteTarget::PanelJump(PanelId::Memory),
            KeyCode::F(7) => RouteTarget::PanelJump(PanelId::Security),
            KeyCode::F(8) => RouteTarget::PanelJump(PanelId::Health),
            // 纯 UI 机械键:主题循环(非注册表动作)
            KeyCode::Char('t') => RouteTarget::ThemeCycle,
            // Ctrl+W 前缀:进入方向窗格导航态;必须在 codegen 查表之前,
            // 避免 Ctrl+W 被 'w' 绑定(view.focus_pane)截获
            KeyCode::Char('w') if ctrl => RouteTarget::EnterMode(RouterMode::WPrefix),
            // 面板上下文动作菜单:bare `a` 唤出焦点面板精选动作(Ctrl+A 归面板多选)
            KeyCode::Char('a') if !ctrl => RouteTarget::OpenActionMenu,
            // 焦点轮转
            KeyCode::Tab => RouteTarget::FocusCycle { forward: true },
            KeyCode::BackTab => RouteTarget::FocusCycle { forward: false },
            // Concord W3 T3.4:`\` 互切 Chat⇄Dashboard 视图模式(方案 §7.4 复用
            // 原 companion 键;view.toggle_companion 动作保留但改经命令面板访问)
            KeyCode::Char('\\') => RouteTarget::ToggleViewMode,
            // Concord T1.3:全局动作键由 codegen 键位表派生(default_key/alias_keys
            // 声明驱动);未命中则交由当前焦点面板处理
            _ => Self::global_action_for(key).unwrap_or(RouteTarget::FocusPanel),
        }
    }

    /// 从 codegen 键位表查找按键对应的全局动作(Concord T1.3 第四通道消费点)
    ///
    /// WHY 每次查表而非缓存:键位表仅 ~8 条,按键事件频率远低于帧预算,
    /// 线性查找开销可忽略;避免引入静态缓存的生命周期/测试复杂度。
    /// 键位声明在 `actions/domains/*.rs` 的 `default_key`/`alias_keys`,
    /// 经 `codegen::key_bindings` 派生——声明即事实源,INV-K 不变量守护。
    fn global_action_for(key: KeyEvent) -> Option<RouteTarget> {
        let reg = ActionRegistry::with_builtin_domains();
        crate::actions::codegen::key_bindings(&reg)
            .into_iter()
            .find(|b| b.key.code == key.code && b.key.modifiers == key.modifiers)
            .map(|b| RouteTarget::GlobalAction(b.action_id))
    }

    /// GPrefix 模式路由:`g` 之后的次键(g 前缀两键序列)
    ///
    /// - `g` → 滚动到顶(gg)
    /// - `1-6` → 扩展面板跳转(EventStream/Router/McpNodes/Chtc/Timeline/ResourceMonitor)
    /// - 其余 → 退出前缀态(调用方回到 Normal,该键可另行处理或忽略)
    fn route_gprefix(key: KeyEvent) -> RouteTarget {
        match key.code {
            KeyCode::Char('g') => RouteTarget::ScrollTop,
            KeyCode::Char('1') => RouteTarget::PanelJump(PanelId::EventStream),
            KeyCode::Char('2') => RouteTarget::PanelJump(PanelId::Router),
            KeyCode::Char('3') => RouteTarget::PanelJump(PanelId::McpNodes),
            KeyCode::Char('4') => RouteTarget::PanelJump(PanelId::Chtc),
            KeyCode::Char('5') => RouteTarget::PanelJump(PanelId::Timeline),
            KeyCode::Char('6') => RouteTarget::PanelJump(PanelId::ResourceMonitor),
            _ => RouteTarget::ExitMode,
        }
    }

    /// WPrefix 模式路由:`Ctrl+W` 之后的次键(方向窗格导航 + 循环)
    ///
    /// - `h`/`l` → 左/右方向窗格焦点(几何解析在 app)
    /// - `j`/`k` → 下/上方向(当前横向布局无垂直邻居,app 侧 no-op)
    /// - `w` → 循环切换活跃窗格(经 codegen 键位表查 view.focus_pane,与 Normal 同源)
    /// - 其余 → 退出前缀态(取消,不触发动作)
    fn route_wprefix(key: KeyEvent) -> RouteTarget {
        match key.code {
            KeyCode::Char('h') => RouteTarget::FocusPaneDir(PaneDir::Left),
            KeyCode::Char('l') => RouteTarget::FocusPaneDir(PaneDir::Right),
            KeyCode::Char('j') => RouteTarget::FocusPaneDir(PaneDir::Down),
            KeyCode::Char('k') => RouteTarget::FocusPaneDir(PaneDir::Up),
            // 'w' 声明键(view.focus_pane)经 codegen 查表,与 Normal 模式同源
            KeyCode::Char('w') => Self::global_action_for(key).unwrap_or(RouteTarget::ExitMode),
            _ => RouteTarget::ExitMode,
        }
    }

    /// Insert 模式路由:Esc 退出 > 极少数全局键 > 提交/退格 > 原始字符
    fn route_insert(key: KeyEvent) -> RouteTarget {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => RouteTarget::ExitMode,
            // Insert 模式仅保留 Ctrl+L 一个全局键(声明在 domains/system.rs);
            // WHY 不走 codegen 查表:Insert 的全局键子集刻意最小化(不打断
            // 输入流),Ctrl+E 等其余声明键在此模式必须保持 Ignored(有测试守护)。
            KeyCode::Char('l') if ctrl => RouteTarget::GlobalAction("system.toggle_locale"),
            KeyCode::Enter => RouteTarget::Submit,
            KeyCode::Backspace => RouteTarget::Backspace,
            // Concord W4 T4.5:Insert 态 Tab = @ 引用补全(末尾词 @ 起始时)
            KeyCode::Tab => RouteTarget::MentionComplete,
            // Concord W6 T6.2:Insert 态 ↑↓ = composer 历史回溯/前进
            // (Slash 态 ↑↓ 为补全导航,不受影响)
            KeyCode::Up => RouteTarget::HistoryPrev,
            KeyCode::Down => RouteTarget::HistoryNext,
            // 普通字符(排除 Ctrl 组合)进入输入缓冲
            KeyCode::Char(c) if !ctrl => RouteTarget::InsertChar(c),
            _ => RouteTarget::Ignored,
        }
    }

    /// Command 模式路由:Esc 关闭 > 上下选择 > 提交 > 退格 > 检索字符
    fn route_command(key: KeyEvent) -> RouteTarget {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => RouteTarget::ExitMode,
            KeyCode::Up => RouteTarget::PaletteMove { down: false },
            KeyCode::Down => RouteTarget::PaletteMove { down: true },
            KeyCode::Enter => RouteTarget::Submit,
            KeyCode::Backspace => RouteTarget::Backspace,
            KeyCode::Char(c) if !ctrl => RouteTarget::PaletteInput(c),
            _ => RouteTarget::Ignored,
        }
    }

    /// Slash 模式路由(Concord W2):与 Command 态同构 + Tab 前缀补全
    ///
    /// 纯机械路由:补全候选过滤/三分层分流均由 slash_parser 与
    /// SlashCommandSurface 承担,路由器只决定按键归属。
    fn route_slash(key: KeyEvent) -> RouteTarget {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => RouteTarget::ExitMode,
            KeyCode::Up => RouteTarget::PaletteMove { down: false },
            KeyCode::Down => RouteTarget::PaletteMove { down: true },
            KeyCode::Enter => RouteTarget::Submit,
            KeyCode::Tab => RouteTarget::SlashComplete,
            KeyCode::Backspace => RouteTarget::Backspace,
            KeyCode::Char(c) if !ctrl => RouteTarget::PaletteInput(c),
            _ => RouteTarget::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造 Press 按键(默认无修饰符)
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// 构造带 Ctrl 修饰的 Press 按键
    fn ctrl_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    // === D 类快照测试:Normal 模式路由表 ===

    #[test]
    fn normal_global_shortcuts_take_priority() {
        assert_eq!(
            InputRouter::route(RouterMode::Normal, ctrl_key(KeyCode::Char('l'))),
            RouteTarget::GlobalAction("system.toggle_locale")
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, ctrl_key(KeyCode::Char('e'))),
            RouteTarget::GlobalAction("export.run")
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, ctrl_key(KeyCode::Char('p'))),
            RouteTarget::OpenPalette
        );
    }

    #[test]
    fn normal_mode_switches_and_focus_cycle() {
        // Concord W2:`/` 与 `:` 同进斜杠命令模式(命令翻转;`:` 为废弃窗口别名)
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char(':'))),
            RouteTarget::EnterSlash
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char('/'))),
            RouteTarget::EnterSlash
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char('i'))),
            RouteTarget::EnterMode(RouterMode::Insert)
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Tab)),
            RouteTarget::FocusCycle { forward: true }
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::BackTab)),
            RouteTarget::FocusCycle { forward: false }
        );
    }

    #[test]
    fn normal_other_keys_go_to_focus_panel() {
        // Enter/Space/导航键在 Normal 下归焦点面板
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Enter)),
            RouteTarget::FocusPanel
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char(' '))),
            RouteTarget::FocusPanel
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char('j'))),
            RouteTarget::FocusPanel
        );
    }

    // === D 类快照测试:Insert 模式路由表 ===

    #[test]
    fn insert_routes_chars_and_controls() {
        assert_eq!(
            InputRouter::route(RouterMode::Insert, key(KeyCode::Char('x'))),
            RouteTarget::InsertChar('x')
        );
        assert_eq!(
            InputRouter::route(RouterMode::Insert, key(KeyCode::Enter)),
            RouteTarget::Submit
        );
        assert_eq!(
            InputRouter::route(RouterMode::Insert, key(KeyCode::Esc)),
            RouteTarget::ExitMode
        );
        // Insert 下仍允许 Ctrl+L 中英切换
        assert_eq!(
            InputRouter::route(RouterMode::Insert, ctrl_key(KeyCode::Char('l'))),
            RouteTarget::GlobalAction("system.toggle_locale")
        );
        // Insert 下其他 Ctrl 组合被忽略(不打断输入流)
        assert_eq!(
            InputRouter::route(RouterMode::Insert, ctrl_key(KeyCode::Char('e'))),
            RouteTarget::Ignored
        );
    }

    // === D 类快照测试:Command 模式路由表 ===

    #[test]
    fn command_routes_search_and_selection() {
        assert_eq!(
            InputRouter::route(RouterMode::Command, key(KeyCode::Char('q'))),
            RouteTarget::PaletteInput('q')
        );
        assert_eq!(
            InputRouter::route(RouterMode::Command, key(KeyCode::Down)),
            RouteTarget::PaletteMove { down: true }
        );
        assert_eq!(
            InputRouter::route(RouterMode::Command, key(KeyCode::Up)),
            RouteTarget::PaletteMove { down: false }
        );
        assert_eq!(
            InputRouter::route(RouterMode::Command, key(KeyCode::Enter)),
            RouteTarget::Submit
        );
        assert_eq!(
            InputRouter::route(RouterMode::Command, key(KeyCode::Esc)),
            RouteTarget::ExitMode
        );
    }

    // === D 类快照测试:Normal 模式完备化(M2 item② router 完备) ===

    #[test]
    fn normal_quit_scroll_theme() {
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char('q'))),
            RouteTarget::Quit
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Esc)),
            RouteTarget::Quit
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char('G'))),
            RouteTarget::ScrollBottom
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char('t'))),
            RouteTarget::ThemeCycle
        );
    }

    #[test]
    fn normal_number_keys_jump_first_nine_panels() {
        let cases = [
            ('1', PanelId::Quest),
            ('2', PanelId::Parliament),
            ('3', PanelId::Budget),
            ('4', PanelId::Memory),
            ('5', PanelId::Security),
            ('6', PanelId::Health),
            ('7', PanelId::Log),
            ('8', PanelId::Help),
            ('9', PanelId::Decay),
        ];
        for (c, pid) in cases {
            assert_eq!(
                InputRouter::route(RouterMode::Normal, key(KeyCode::Char(c))),
                RouteTarget::PanelJump(pid),
                "数字键 '{c}' 应跳转到 {pid:?}"
            );
        }
    }

    #[test]
    fn normal_fkeys_jump_panels() {
        let cases = [
            (1, PanelId::Quest),
            (2, PanelId::Parliament),
            (3, PanelId::Budget),
            (6, PanelId::Memory),
            (7, PanelId::Security),
            (8, PanelId::Health),
        ];
        for (n, pid) in cases {
            assert_eq!(
                InputRouter::route(RouterMode::Normal, key(KeyCode::F(n))),
                RouteTarget::PanelJump(pid),
                "F{n} 应跳转到 {pid:?}"
            );
        }
    }

    #[test]
    fn normal_ui_toggles_route_to_actions() {
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char('l'))),
            RouteTarget::GlobalAction("view.switch_layout")
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char('\\'))),
            RouteTarget::ToggleViewMode,
            "Concord W3 T3.4:`\\` 互切 Chat⇄Dashboard(原 companion 键复用)"
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char('E'))),
            RouteTarget::GlobalAction("export.run")
        );
    }

    #[test]
    fn normal_stage2_companion_keys_route_to_actions() {
        // Stage 2 新增键:] 循环绑定伴随,w 切换窗格焦点——经 GlobalAction 桥接到
        // dispatch_action 本地方法(view.cycle_companion / view.focus_pane),与 handle_global_key 对齐。
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char(']'))),
            RouteTarget::GlobalAction("view.cycle_companion")
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char('w'))),
            RouteTarget::GlobalAction("view.focus_pane")
        );
    }

    #[test]
    fn normal_colon_and_ctrl_p_are_distinct_targets() {
        // Concord W2:斜杠命令栏与 Ctrl+P palette overlay 为两个独立入口,不可混同。
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char(':'))),
            RouteTarget::EnterSlash
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, ctrl_key(KeyCode::Char('p'))),
            RouteTarget::OpenPalette
        );
    }

    #[test]
    fn slash_mode_routes_like_command_plus_tab_complete() {
        // Concord W2:Slash 态与 Command 态同构 + Tab 前缀补全目标
        assert_eq!(
            InputRouter::route(RouterMode::Slash, key(KeyCode::Esc)),
            RouteTarget::ExitMode
        );
        assert_eq!(
            InputRouter::route(RouterMode::Slash, key(KeyCode::Up)),
            RouteTarget::PaletteMove { down: false }
        );
        assert_eq!(
            InputRouter::route(RouterMode::Slash, key(KeyCode::Down)),
            RouteTarget::PaletteMove { down: true }
        );
        assert_eq!(
            InputRouter::route(RouterMode::Slash, key(KeyCode::Enter)),
            RouteTarget::Submit
        );
        assert_eq!(
            InputRouter::route(RouterMode::Slash, key(KeyCode::Tab)),
            RouteTarget::SlashComplete
        );
        assert_eq!(
            InputRouter::route(RouterMode::Slash, key(KeyCode::Backspace)),
            RouteTarget::Backspace
        );
        assert_eq!(
            InputRouter::route(RouterMode::Slash, key(KeyCode::Char('q'))),
            RouteTarget::PaletteInput('q')
        );
        // Ctrl 组合不进输入缓冲(与 Command/Insert 态语义一致)
        assert_eq!(
            InputRouter::route(RouterMode::Slash, ctrl_key(KeyCode::Char('e'))),
            RouteTarget::Ignored
        );
    }

    #[test]
    fn normal_ctrl_arrows_adjust_ratio() {
        assert_eq!(
            InputRouter::route(RouterMode::Normal, ctrl_key(KeyCode::Up)),
            RouteTarget::RatioAdjust { increase: true }
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, ctrl_key(KeyCode::Down)),
            RouteTarget::RatioAdjust { increase: false }
        );
    }

    #[test]
    fn ctrl_l_takes_priority_over_plain_l_layout() {
        // 带 Ctrl 的 l → locale;不带 → 布局。验证 CONTROL guard 顺序正确。
        assert_eq!(
            InputRouter::route(RouterMode::Normal, ctrl_key(KeyCode::Char('l'))),
            RouteTarget::GlobalAction("system.toggle_locale")
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char('l'))),
            RouteTarget::GlobalAction("view.switch_layout")
        );
    }

    #[test]
    fn normal_g_enters_gprefix_mode() {
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char('g'))),
            RouteTarget::EnterMode(RouterMode::GPrefix)
        );
    }

    #[test]
    fn gprefix_routes_scroll_and_extended_panels() {
        assert_eq!(
            InputRouter::route(RouterMode::GPrefix, key(KeyCode::Char('g'))),
            RouteTarget::ScrollTop
        );
        let cases = [
            ('1', PanelId::EventStream),
            ('2', PanelId::Router),
            ('3', PanelId::McpNodes),
            ('4', PanelId::Chtc),
            ('5', PanelId::Timeline),
            ('6', PanelId::ResourceMonitor),
        ];
        for (c, pid) in cases {
            assert_eq!(
                InputRouter::route(RouterMode::GPrefix, key(KeyCode::Char(c))),
                RouteTarget::PanelJump(pid),
                "g+{c} 应跳转到扩展面板 {pid:?}"
            );
        }
        // 非预期次键:退出前缀态
        assert_eq!(
            InputRouter::route(RouterMode::GPrefix, key(KeyCode::Char('z'))),
            RouteTarget::ExitMode
        );
    }

    #[test]
    fn release_events_are_ignored_in_all_modes() {
        // Windows crossterm Release 事件在任何模式都不路由
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, release),
            RouteTarget::Ignored
        );
        assert_eq!(
            InputRouter::route(RouterMode::Insert, release),
            RouteTarget::Ignored
        );
        assert_eq!(
            InputRouter::route(RouterMode::Command, release),
            RouteTarget::Ignored
        );
    }

    #[test]
    fn normal_ctrl_w_enters_wprefix_plain_w_cycles() {
        // Ctrl+W → 方向导航前缀态;plain w → 循环(二者共存,与 Vim 一致)
        assert_eq!(
            InputRouter::route(RouterMode::Normal, ctrl_key(KeyCode::Char('w'))),
            RouteTarget::EnterMode(RouterMode::WPrefix)
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char('w'))),
            RouteTarget::GlobalAction("view.focus_pane")
        );
    }

    #[test]
    fn wprefix_routes_directions_and_cycle() {
        let dirs = [
            ('h', PaneDir::Left),
            ('l', PaneDir::Right),
            ('j', PaneDir::Down),
            ('k', PaneDir::Up),
        ];
        for (c, dir) in dirs {
            assert_eq!(
                InputRouter::route(RouterMode::WPrefix, key(KeyCode::Char(c))),
                RouteTarget::FocusPaneDir(dir),
                "Ctrl+W {c} 应解析为方向 {dir:?}"
            );
        }
        // w → 循环(与 Normal w 一致)
        assert_eq!(
            InputRouter::route(RouterMode::WPrefix, key(KeyCode::Char('w'))),
            RouteTarget::GlobalAction("view.focus_pane")
        );
        // 非预期次键:退出前缀态(取消)
        assert_eq!(
            InputRouter::route(RouterMode::WPrefix, key(KeyCode::Char('z'))),
            RouteTarget::ExitMode
        );
    }

    #[test]
    fn normal_bare_a_opens_action_menu_ctrl_a_falls_to_panel() {
        // bare `a` → 面板动作菜单;Ctrl+A 不匹配(归面板多选,零回归)
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char('a'))),
            RouteTarget::OpenActionMenu
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, ctrl_key(KeyCode::Char('a'))),
            RouteTarget::FocusPanel
        );
    }
}
