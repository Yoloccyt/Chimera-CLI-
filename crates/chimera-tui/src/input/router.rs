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
    /// 焦点轮转(Tab 正向 / Shift+Tab 反向)
    FocusCycle {
        /// true = 下一个面板,false = 上一个面板
        forward: bool,
    },
    /// 交由当前焦点面板处理(Enter 下钻 / Space 切换 / 列表导航等)
    FocusPanel,
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
        }
    }

    /// Normal 模式路由:退出 > 全局快捷键 > 比例调整 > 模式切换 > g 前缀/滚动 >
    /// 数字/F 键面板跳转 > UI 切换 > 焦点轮转 > 焦点面板
    ///
    /// WHY 完整覆盖:本表是 Normal 模式按键归属的单一事实源,与 app 的
    /// `handle_global_key` 逐键对齐(M3 接线时替换内联分发)。Action 支持的键
    /// (locale/help/export/layout/companion)走 `GlobalAction`,统一经派发桥接;
    /// 纯 UI 机械键(退出/面板跳转/滚动/主题/比例)用专用目标变体。
    fn route_normal(key: KeyEvent) -> RouteTarget {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            // 退出(普通模式)
            KeyCode::Char('q') | KeyCode::Esc => RouteTarget::Quit,
            // 全局快捷键(Action 支持,最高优先级;带 CONTROL guard 先于同字符普通键)
            KeyCode::Char('l') if ctrl => RouteTarget::GlobalAction("system.toggle_locale"),
            KeyCode::Char('e') if ctrl => RouteTarget::GlobalAction("export.run"),
            KeyCode::Char('p') if ctrl => RouteTarget::EnterMode(RouterMode::Command),
            // Ctrl+方向:主面板比例调整
            KeyCode::Up if ctrl => RouteTarget::RatioAdjust { increase: true },
            KeyCode::Down if ctrl => RouteTarget::RatioAdjust { increase: false },
            // 模式切换
            KeyCode::Char(':') => RouteTarget::EnterMode(RouterMode::Command),
            KeyCode::Char('i') => RouteTarget::EnterMode(RouterMode::Insert),
            KeyCode::Char('?') => RouteTarget::GlobalAction("system.open_help"),
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
            // UI 切换(layout/companion 为 Action;theme 为纯 UI)
            KeyCode::Char('t') => RouteTarget::ThemeCycle,
            KeyCode::Char('l') => RouteTarget::GlobalAction("view.switch_layout"),
            KeyCode::Char('\\') => RouteTarget::GlobalAction("view.toggle_companion"),
            KeyCode::Char('E') => RouteTarget::GlobalAction("export.run"),
            // 焦点轮转
            KeyCode::Tab => RouteTarget::FocusCycle { forward: true },
            KeyCode::BackTab => RouteTarget::FocusCycle { forward: false },
            // 其余按键(Enter/Space/j/k/方向键 等)交由当前焦点面板处理
            _ => RouteTarget::FocusPanel,
        }
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

    /// Insert 模式路由:Esc 退出 > 少数全局键 > 提交/退格 > 原始字符
    fn route_insert(key: KeyEvent) -> RouteTarget {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => RouteTarget::ExitMode,
            // Insert 模式仍允许极少数全局键(中英切换),不打断输入流的其余快捷键忽略
            KeyCode::Char('l') if ctrl => RouteTarget::GlobalAction("system.toggle_locale"),
            KeyCode::Enter => RouteTarget::Submit,
            KeyCode::Backspace => RouteTarget::Backspace,
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
            RouteTarget::EnterMode(RouterMode::Command)
        );
    }

    #[test]
    fn normal_mode_switches_and_focus_cycle() {
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char(':'))),
            RouteTarget::EnterMode(RouterMode::Command)
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
            RouteTarget::GlobalAction("view.toggle_companion")
        );
        assert_eq!(
            InputRouter::route(RouterMode::Normal, key(KeyCode::Char('E'))),
            RouteTarget::GlobalAction("export.run")
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
}
