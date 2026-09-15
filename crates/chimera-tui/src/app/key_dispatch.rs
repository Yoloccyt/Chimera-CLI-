//! app::key_dispatch — 表驱动按键派发表(执行层,M6 方向4-A 重构)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **与 InputRouter 分层分工,零键位双源**:`input::router` 的 InputRouter
//!   仍是"按键归属"的单一事实源(ADR-029,决定按键交给谁);本模块是其下游
//!   的"归属后执行层"——把路由目标 / 弹窗键位 / 动作 id 的 **match 链**
//!   收编为**声明式键表 + 单点执行器**。键位语义不变,变的是表达方式
//!   (链 → 表),因此"同键同面板 → 同动作"的行为等价逐字节成立。
//! - **三层派发表,同一套原语**:
//!   1. 弹窗键表(键 → `PopupKeyAction` 语义动作):滚动型 / ActionMenu /
//!      ConfigMenu 三表,副作用集中在 `apply_popup_action` 单点解释;
//!   2. `ROUTE_TARGET_TABLE`(路由目标判别式 → 处理器):Normal/GPrefix
//!      上下文的 RouteTarget 执行表,每判别式恰一行;
//!   3. `LOCAL_ACTION_TABLE`(action_id → 处理器):`dispatch_action`
//!      本地臂的单一事实源,未命中即编排兜底(发布 TuiActionRequested)。
//!   三表共用 `input::KeyPattern` / `input::KeyRule` / `lookup_key_action`
//!   原语(TaskManagerPanel 的面板键表为同一机制的第 4 个采用者)。
//! - **完整性由测试守护**:每个表配"无空路由 / 无重复注册"断言,变体清单
//!   由穷举 match 编译期驱动(router.rs / types.rs 新增变体即编译失败)。
//!
//! # 行为等价论证(27 面板用户可见行为零变化红线)
//! 本模块全部处理器体为 event_loop.rs 原 match arm 的**逐字节搬迁**
//! (键位判定经 `KeyPattern` 与原 `match key.code` / `if ctrl` 守卫对齐,
//! 见 input/mod.rs 各模式语义注释);既有单测(app/tests.rs 键位行为全集 +
//! router.rs 快照测试 + task_manager.rs 面板键测试)构成等价性回归网。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::TuiApp;
use super::event_loop::{ratio_preset_next, tick_preset_next};
use crate::input::{lookup_key_action, InputRouter, KeyPattern, KeyRule, RouteTarget, RouterMode};
use crate::popup::{PopupKind, Severity};
use crate::types::{InputMode, PanelId, TuiCommand};
use event_bus::{ActionSource, EventMetadata, NexusEvent};

/// P1-2(评估报告 v2):TuiActionRequested 本地兜底超时(编排器未接线场景)
///
/// WHY 2s:正常编排器回发 Completed/Failed 在毫秒级,2s 足够区分
/// “编排器正在执行”与“无消费者”;standalone 模式(TUI 独立运行)下
/// TuiActionRequested 无人消费,超时后状态栏提示避免用户无感知。
const ACTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

// ============================================================
// 第一层:弹窗键表(键 → PopupKeyAction 语义动作)
// ============================================================

/// 弹窗层语义动作 — 键表只映射"键 → 动作",副作用集中在单点执行器解释
///
/// WHY 枚举 + 单执行器:新增弹窗交互 = 增一个枚举变体 + 增表行 + 增执行器
/// arm,编译器穷尽检查防漏;键位声明(表)与语义实现(执行器)分离,
/// 键表可脱离实现单独审计(完整性测试逐行校验无重复/无空路由)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PopupKeyAction {
    /// 关闭当前弹窗(Esc/q)
    Close,
    /// Enter:确认弹窗且选中 Yes 时执行关联命令,否则仅关闭
    ConfirmOrClose,
    /// 滚动型弹窗:向上滚 1 行
    ScrollUp,
    /// 滚动型弹窗:向下滚 1 行
    ScrollDown,
    /// 滚动型弹窗:直达顶部
    ScrollTop,
    /// 滚动型弹窗:直达底部
    ScrollBottom,
    /// 滚动型弹窗:向上翻页(-10 行)
    PageUp,
    /// 滚动型弹窗:向下翻页(+10 行)
    PageDown,
    /// 确认弹窗:左/右切换 Yes/No
    ToggleConfirm,
    /// 动作菜单:选中项上移
    MenuUp,
    /// 动作菜单:选中项下移
    MenuDown,
    /// 动作菜单 Enter:派发选中动作(source=Panel,复用三入口统一派发管线)
    MenuExecute,
    /// 配置菜单 Enter:就地循环选中项并刷新条目(菜单常驻以便连续编辑)
    ConfigCycle,
}

/// 弹窗键表的一行(语义动作私属于本模块,故具名别名而非复用泛型推断)
type PopupKeyRule = KeyRule<PopupKeyAction>;

/// 滚动型弹窗(Detail/Help/Confirm 等)键表 —— 等价旧 `handle_popup_key`
/// 的 match 链(Esc/q 关闭、Enter 确认或关闭、方向/Home/End/翻页、左右切换确认)
const SCROLL_POPUP_KEY_TABLE: &[PopupKeyRule] = &[
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Esc),
        action: PopupKeyAction::Close,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Char('q')),
        action: PopupKeyAction::Close,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Enter),
        action: PopupKeyAction::ConfirmOrClose,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Up),
        action: PopupKeyAction::ScrollUp,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Down),
        action: PopupKeyAction::ScrollDown,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Home),
        action: PopupKeyAction::ScrollTop,
    },
    // PS-3(I-5):翻页与首尾导航 —— 长详情越界滚动后无需等量按 Up 回看;
    // End 传 u16::MAX 表示"直达末尾",渲染帧会把越界值收敛到实际上限。
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::End),
        action: PopupKeyAction::ScrollBottom,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::PageUp),
        action: PopupKeyAction::PageUp,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::PageDown),
        action: PopupKeyAction::PageDown,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Left),
        action: PopupKeyAction::ToggleConfirm,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Right),
        action: PopupKeyAction::ToggleConfirm,
    },
];

/// 动作菜单键表(§4.5 入口三:面板动作)—— ↑↓/kj 移动选中项,Enter 派发选中
/// 动作(经 `DispatchAction`,source=Panel,复用三入口统一派发与反馈管线),
/// Esc/q 关闭。与旧 `handle_action_menu_key` 的 match 链逐字节等价。
const ACTION_MENU_KEY_TABLE: &[PopupKeyRule] = &[
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Esc),
        action: PopupKeyAction::Close,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Char('q')),
        action: PopupKeyAction::Close,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Up),
        action: PopupKeyAction::MenuUp,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Char('k')),
        action: PopupKeyAction::MenuUp,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Down),
        action: PopupKeyAction::MenuDown,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Char('j')),
        action: PopupKeyAction::MenuDown,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Enter),
        action: PopupKeyAction::MenuExecute,
    },
];

/// 配置菜单键表(§4.5 收尾)—— ↑↓/kj 移选,Enter 就地循环选中项(菜单常驻),
/// Esc/q 关闭。与旧 `handle_config_menu_key` 的 match 链逐字节等价。
const CONFIG_MENU_KEY_TABLE: &[PopupKeyRule] = &[
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Esc),
        action: PopupKeyAction::Close,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Char('q')),
        action: PopupKeyAction::Close,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Up),
        action: PopupKeyAction::MenuUp,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Char('k')),
        action: PopupKeyAction::MenuUp,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Down),
        action: PopupKeyAction::MenuDown,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Char('j')),
        action: PopupKeyAction::MenuDown,
    },
    KeyRule {
        pattern: KeyPattern::Code(KeyCode::Enter),
        action: PopupKeyAction::ConfigCycle,
    },
];

impl TuiApp {
    /// 处理弹窗激活时的键盘事件(表驱动:按弹窗种类选表,按键命中即执行语义动作)
    ///
    /// ActionMenu / ConfigMenu 有独立选择/执行语义(↑↓ 移选、Enter 派发/循环),
    /// 与滚动型弹窗(Detail/Help,↑↓ 滚动、Enter 关闭)分流,避免 Up/Down 被
    /// scroll 语义占用 —— 分流判定与键表选择与旧三条 match 链逐字节等价。
    pub(crate) fn handle_popup_key(&mut self, key: KeyEvent) {
        let table: &[PopupKeyRule] = match self.state.popup_stack.current() {
            Some(PopupKind::ActionMenu { .. }) => ACTION_MENU_KEY_TABLE,
            Some(PopupKind::ConfigMenu { .. }) => CONFIG_MENU_KEY_TABLE,
            _ => SCROLL_POPUP_KEY_TABLE,
        };
        // 未命中任何键位 = 旧 `_ => {}` arm:弹窗吞掉按键,无操作
        if let Some(action) = lookup_key_action(table, key) {
            self.apply_popup_action(action);
        }
    }

    /// 执行弹窗语义动作(键表的单点解释器:表只表达"键 → 动作",副作用只此一处)
    fn apply_popup_action(&mut self, action: PopupKeyAction) {
        match action {
            PopupKeyAction::Close => {
                self.state.popup_stack.pop();
            }
            PopupKeyAction::ConfirmOrClose => {
                // 确认弹窗且选中 Yes 时执行关联命令
                if let Some(PopupKind::Confirm {
                    on_confirm,
                    confirmed,
                    ..
                }) = self.state.popup_stack.current()
                {
                    if *confirmed {
                        let cmd = on_confirm.clone();
                        self.state.popup_stack.pop();
                        self.apply_confirm_command(&cmd);
                    } else {
                        self.state.popup_stack.pop();
                    }
                } else {
                    self.state.popup_stack.pop();
                }
            }
            PopupKeyAction::ScrollUp => {
                self.state.popup_stack.scroll_current(-1);
            }
            PopupKeyAction::ScrollDown => {
                self.state.popup_stack.scroll_current(1);
            }
            PopupKeyAction::ScrollTop => {
                self.state.popup_stack.scroll_to(0);
            }
            PopupKeyAction::ScrollBottom => {
                self.state.popup_stack.scroll_to(u16::MAX);
            }
            PopupKeyAction::PageUp => {
                self.state.popup_stack.scroll_current(-10);
            }
            PopupKeyAction::PageDown => {
                self.state.popup_stack.scroll_current(10);
            }
            PopupKeyAction::ToggleConfirm => {
                self.state.popup_stack.toggle_confirm();
            }
            PopupKeyAction::MenuUp => {
                self.move_menu_selection(false);
            }
            PopupKeyAction::MenuDown => {
                self.move_menu_selection(true);
            }
            PopupKeyAction::MenuExecute => {
                // 取选中 action_id,关闭菜单后经统一派发桥接执行(source=Panel)
                let action_id = self.state.popup_stack.action_menu_selected_id();
                self.state.popup_stack.pop();
                if let Some(action_id) = action_id {
                    self.apply_command(TuiCommand::DispatchAction {
                        action_id,
                        payload: "{}".to_string(),
                        source: ActionSource::Panel,
                    });
                }
            }
            PopupKeyAction::ConfigCycle => {
                if let Some(idx) = self.state.popup_stack.config_menu_selected() {
                    self.cycle_config_item(idx);
                    // 循环后刷新条目显示当前值,菜单常驻以便连续编辑
                    let entries = self.config_menu_entries();
                    self.state.popup_stack.set_config_menu_entries(entries);
                }
            }
        }
    }

    /// 菜单选中项移动(动作菜单 / 配置菜单共用 `↑↓/kj` 键语义;两者条目模型
    /// 不同,popup_stack 的 `move_*_selection` 按弹窗种类分流,此处按当前
    /// 弹窗种类路由——与旧 `handle_action_menu_key` / `handle_config_menu_key`
    /// 各自调用各自方法的行为逐字节等价)
    fn move_menu_selection(&mut self, down: bool) {
        if matches!(
            self.state.popup_stack.current(),
            Some(PopupKind::ConfigMenu { .. })
        ) {
            self.state.popup_stack.move_config_menu_selection(down);
        } else {
            self.state.popup_stack.move_action_menu_selection(down);
        }
    }

    /// 打开配置速调菜单(config.edit)—— 列出运行时可调配置项,就地循环编辑
    pub(crate) fn open_config_menu(&mut self) {
        let entries = self.config_menu_entries();
        self.state.popup_stack.push(PopupKind::config_menu(entries));
    }

    /// 组装配置菜单条目(固定顺序 [主题, 占比, Tick],值取自当前 config)
    ///
    /// WHY 顺序固定:`cycle_config_item` 按下标循环对应项,顺序须与本函数一致。
    fn config_menu_entries(&self) -> Vec<(String, String)> {
        vec![
            (
                crate::t!("status.theme").to_string(),
                self.config.theme.as_str().to_string(),
            ),
            (
                crate::t!("status.ratio").to_string(),
                // Task 1.15.4:main_panel_ratio 经 getter 方法读取(委托 pane_manager)
                crate::render::percent_summary(self.main_panel_ratio()),
            ),
            (
                crate::t!("status.tick").to_string(),
                format!("{}ms (重启生效)", self.config.tick_interval_ms),
            ),
        ]
    }

    /// 循环指定配置项(0=主题即时生效,1=占比即时生效,2=tick 重启生效)
    ///
    /// WHY 仅这 3 项:核验确认仅 theme/main_panel_ratio/tick_interval_ms 可运行时安全修改;
    /// 下标顺序须与 `config_menu_entries` 一致。
    fn cycle_config_item(&mut self, idx: usize) {
        match idx {
            0 => {
                self.config.theme = self.config.theme.next();
                // 主题即时生效:全面板 mark_dirty 触发下一帧重绘
                for panel_id in self.focus_manager.panels() {
                    self.state.mark_dirty(*panel_id);
                }
            }
            // Task 1.15.4:main_panel_ratio 写入经 pane_manager(读取用 getter 方法)
            1 => self.pane_manager.main_panel_ratio = ratio_preset_next(self.main_panel_ratio()),
            2 => self.config.tick_interval_ms = tick_preset_next(self.config.tick_interval_ms),
            _ => {}
        }
    }

    // ============================================================
    // Ctrl+L 全局中英切换热键(三处特判收口)
    // ============================================================

    /// Ctrl+L 全局中英切换热键的收口(M6:此前 popup / Slash / palette 三处
    /// 各自内联特判,现收敛为单点;各调用点仍传原 `ActionSource`,事件源语义
    /// 零变化 —— 弹窗路径=Panel,Slash/palette 路径=Palette,与旧实现一致)
    ///
    /// # 参数
    /// - `key`: 当前按键事件
    /// - `source`: 命中时派发 `system.toggle_locale` 的动作来源(逐路径保留)
    ///
    /// # 返回值
    /// 命中热键并派发返回 `true`(调用方应直接 `return`);未命中返回 `false`,
    /// 按键走原上下文路径(弹窗/Slash 各自的键表)
    pub(crate) fn try_locale_hotkey(&mut self, key: KeyEvent, source: ActionSource) -> bool {
        if key.code == KeyCode::Char('l') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.dispatch_action("system.toggle_locale", "{}".to_string(), source);
            true
        } else {
            false
        }
    }

    // ============================================================
    // 第二层:RouteTarget 执行表(路由目标判别式 → 处理器)
    // ============================================================

    /// 执行 InputRouter 计算出的路由目标(Normal/GPrefix 上下文)
    ///
    /// WHY 集中执行:路由器只表达"按键归属意图",具体副作用(退出/切面板/滚动/
    /// 主题/比例/派发动作/进入模式/面板委托)在此统一落地,与"面板表达意图、
    /// App 执行"设计一致。Insert/Command 模式专属目标(InsertChar/Palette*/Submit 等)
    /// 不会由 Normal/GPrefix 路由产生,表内以无操作处理器兜底以保证完备。
    ///
    /// 表驱动(M6):按判别式查 `ROUTE_TARGET_TABLE` 执行;未命中(新增变体
    /// 未登记)静默无操作 —— 完整性单测 `route_target_table_complete_and_unique`
    /// 以穷举代表值守护"每判别式恰一行",杜绝静默漂移。
    pub(crate) fn apply_route_target(&mut self, target: RouteTarget, key: KeyEvent) {
        let handler = ROUTE_TARGET_TABLE
            .iter()
            .find(|e| {
                std::mem::discriminant(&e.representative)
                    == std::mem::discriminant(&target)
            })
            .map(|e| e.handler);
        if let Some(handler) = handler {
            handler(self, target, key);
        }
    }

    /// 处理 Slash 模式按键:经 RouterMode::Slash 纯机械路由到输入/选择/提交
    pub(crate) fn handle_slash_key(&mut self, key: KeyEvent) {
        match InputRouter::route(RouterMode::Slash, key) {
            RouteTarget::PaletteInput(c) => {
                self.state.input_buffer.push(c);
                // 输入变化 → 候选列表重算,选中项复位首项(与主流补全交互一致)
                self.state.slash_selected = 0;
            }
            RouteTarget::Backspace => {
                self.state.input_buffer.pop();
                self.state.slash_selected = 0;
            }
            RouteTarget::PaletteMove { down } => {
                let reg = crate::actions::SlashCommandRegistry::with_builtin_commands();
                let count = crate::slash_surface::candidates(&reg, &self.state.input_buffer).len();
                if count > 0 {
                    let sel = self.state.slash_selected;
                    self.state.slash_selected = if down {
                        (sel + 1) % count
                    } else {
                        // 上移循环:0 → 末项(与命令面板导航体验一致)
                        (sel + count - 1) % count
                    };
                }
            }
            RouteTarget::SlashComplete => self.slash_tab_complete(),
            RouteTarget::Submit => self.submit_slash(),
            RouteTarget::ExitMode => {
                self.state.input_mode = InputMode::Normal;
                self.state.input_buffer.clear();
                self.state.slash_selected = 0;
            }
            _ => {}
        }
    }

    /// 处理 Insert 模式按键(M3a):经 InputRouter 的 Insert 表路由到输入缓冲操作
    ///
    /// WHY 独立方法:Insert 是原始文本输入,与 Normal 的按键归属语义不同
    /// (字符进缓冲、Enter 提交、Esc 退出),单独处理避免与 apply_route_target 混杂。
    /// M3a 阶段 Submit 为占位(不发事件),M3b 接入 Chat 面板后改为发 TuiChatSubmitted。
    pub(crate) fn handle_insert_key(&mut self, key: KeyEvent) {
        match InputRouter::route(RouterMode::Insert, key) {
            RouteTarget::InsertChar(c) => self.state.input_buffer.push(c),
            RouteTarget::Backspace => {
                self.state.input_buffer.pop();
            }
            // Concord W4 T4.5:@ 引用补全——末尾词以 @ 起始时替换为首个候选;
            // 无候选时不改动缓冲(诚实降级,不伪造文件引用)
            RouteTarget::MentionComplete => {
                let tail = crate::mention::extract_mention_tail(&self.state.input_buffer);
                if let Some((start, prefix)) = tail {
                    let cands = crate::mention::mention_candidates(&self.state, &prefix);
                    if let Some(first) = cands.first() {
                        self.state.input_buffer.truncate(start);
                        self.state.input_buffer.push_str(first);
                    }
                }
            }
            // Concord W6 T6.2:composer 历史 ↑ 回溯(首次保存草稿,到顶保持)
            RouteTarget::HistoryPrev => {
                let mut h = crate::composer_history::ComposerHistory::from_entries(
                    self.state.input_history.clone(),
                );
                h.pos = self.state.history_pos;
                h.draft = self.state.history_draft.clone();
                if let Some(text) = h.prev(&self.state.input_buffer) {
                    self.state.input_buffer = text;
                }
                self.state.history_pos = h.pos;
                self.state.history_draft = h.draft;
            }
            // Concord W6 T6.2:composer 历史 ↓ 前进(回底恢复草稿)
            RouteTarget::HistoryNext => {
                let mut h = crate::composer_history::ComposerHistory::from_entries(
                    self.state.input_history.clone(),
                );
                h.pos = self.state.history_pos;
                h.draft = self.state.history_draft.clone();
                if let Some(text) = h.forward() {
                    self.state.input_buffer = text;
                }
                self.state.history_pos = h.pos;
                self.state.history_draft = h.draft;
            }
            RouteTarget::ExitMode => {
                // F-5:Esc 取消 palette 参数输入流(pending 动作不再派发)
                self.state.pending_action = None;
                self.state.input_mode = InputMode::Normal;
                self.state.input_buffer.clear();
            }
            // Insert 下仍允许极少数全局键(如 Ctrl+L 中英切换),经派发桥接
            RouteTarget::GlobalAction(action_id) => {
                self.dispatch_action(action_id, "{}".to_string(), ActionSource::Chat);
            }
            RouteTarget::Submit => {
                let text = self.state.input_buffer.trim().to_string();
                // F-5:palette 参数输入流优先——存在 pending 动作时,Insert 缓冲
                // 收集的是该动作的 query(非 Chat 消息)。提交以 {"query": text}
                // 经三入口统一派发后回到 Normal(一次性动作,不形成 REPL)。
                if let Some(pending) = self.state.pending_action.clone() {
                    if !text.is_empty() {
                        self.state.pending_action = None;
                        let payload = serde_json::json!({ "query": text }).to_string();
                        self.dispatch_action(&pending.action_id, payload, pending.source);
                        self.state.input_mode = InputMode::Normal;
                        self.state.input_buffer.clear();
                    }
                    // 空输入:不派发、不丢失 pending,等待继续输入(Esc 取消)
                    return;
                }

                // Concord W4 T4.5:! shell 直通 — HonestTodo 占位(红线:所有外部
                // 调用须经 SecCore 沙箱 + Decay 衰减;安全派发管道未接线前
                // 不伪造直通,也不把 ! 命令当普通 Chat 消息发送)
                if text.starts_with('!') {
                    self.state
                        .set_status(crate::t!("shell.todo").to_string(), Severity::Warning);
                    self.state.input_buffer.clear();
                    return;
                }

                // M3b:非空输入发布 TuiChatSubmitted(经 EventBus 回环由 ChatSync 追加用户消息),
                // 自动切到 Chat 面板;保持 Insert 模式形成 chat REPL(Esc 退出)。
                if !text.is_empty() {
                    // Concord W6 T6.2:提交入史(去重/容量语义在导航器内)
                    self.commit_input_history(&text);
                    // 以 `/` 开头视为斜杠命令,提取命令名(首个空白前的词)
                    let slash_command = text
                        .strip_prefix('/')
                        .map(|rest| rest.split_whitespace().next().unwrap_or("").to_string());
                    self.publish_control_event(NexusEvent::TuiChatSubmitted {
                        metadata: EventMetadata::new("chimera-tui"),
                        // Task 1.15.4:chat_session_id 移至 chat_session
                        session_id: self.chat_session.chat_session_id.clone(),
                        query: text,
                        slash_command,
                    });
                    // Concord W3 T3.2:Chat 模式下会话流已全屏,不再切面板;
                    // Dashboard 模式保持原行为(提交后自动切到 Chat 面板)
                    if self.state.view_mode == crate::types::ViewMode::Dashboard {
                        self.switch_panel_to(PanelId::Chat);
                    }
                }
                self.state.input_buffer.clear();
            }
            // 其余(Ignored 等)在 Insert 下无操作
            _ => {}
        }
    }

    /// 命令面板打开时的键盘处理(M2.2)
    ///
    /// 语义对齐 `InputRouter` 的 Command 模式:Esc 关闭 / ↑↓ 选择 / Enter 执行
    /// 选中动作(经 `DispatchAction` 统一派发,source=Palette)/ 退格 / 字符过滤。
    ///
    /// WHY 逐分支分别借用 `self.palette`:Esc/Enter 需写 `self.palette = None`,
    /// 而导航键需 `&mut` 模型;分开借用避免在同一作用域同时持有可变
    /// 引用与重赋值的借用冲突。
    pub(crate) fn handle_palette_key(&mut self, key: KeyEvent) {
        // I-A(2026-09-06 复评):palette 打开时 Ctrl+L 此前被吞 —— 其余
        // 全部输入模式(Insert/Slash/遗留 Command)均可中英切换,唯独
        // palette 不行,行为不一致;复用 Slash 模式的同义派发(本地臂
        // 立即生效),palette 保持打开(检索列表 i18n 随刷新)。
        // WHY 置于路由之前:route_command 对 Ctrl 组合返回 Ignored(检索缓冲
        // 只收纯字符),Ctrl+L 若走路由会被吞,故同 Slash 模式先行特判。
        if key.code == KeyCode::Char('l') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.dispatch_action(
                "system.toggle_locale",
                "{}".to_string(),
                ActionSource::Palette,
            );
            return;
        }

        // PS-3(I-1):键处理改经 InputRouter 的 Command 路由表。
        // WHY 合并双源:此前本方法手写 match,与 router.rs 的 route_command
        // 语义重复但实现分离 —— 正是 INV-K 不变量要消灭的"键位双源"。
        // 此前 route_command 仅被自身单测覆盖(InputMode 无 Command 变体,
        // 无任何生产路径传入 RouterMode::Command),属死代码;改经路由后
        // 二者合一,palette 键语义与全局键位表同源自洽。
        match InputRouter::route(RouterMode::Command, key) {
            RouteTarget::ExitMode => {
                // Task 1.15.4:palette 移至 chat_session
                self.chat_session.palette = None;
            }
            RouteTarget::Submit => {
                // 先取选中动作 id(&'static str,不借用模型),关闭面板后统一派发。
                let action_id = self
                    .chat_session
                    .palette
                    .as_ref()
                    .and_then(|m| m.selected_action())
                    .map(str::to_string);
                // F-5:需 query 的动作分流到 Insert 参数收集态(不直接发空 payload)。
                // 判定在关闭面板前完成(模型持有 Registry 单一事实源)。
                let requires_query = self
                    .chat_session
                    .palette
                    .as_ref()
                    .map(|m| m.selected_action_requires_query())
                    .unwrap_or(false);
                self.chat_session.palette = None;
                if let Some(action_id) = action_id {
                    if requires_query {
                        self.state.pending_action = Some(crate::types::PendingAction {
                            action_id,
                            source: ActionSource::Palette,
                        });
                        self.state.input_mode = InputMode::Insert;
                        self.state.input_buffer.clear();
                    } else {
                        self.apply_command(TuiCommand::DispatchAction {
                            action_id,
                            payload: "{}".to_string(),
                            source: ActionSource::Palette,
                        });
                    }
                }
            }
            RouteTarget::PaletteMove { down } => {
                if let Some(m) = self.chat_session.palette.as_mut() {
                    m.move_selection(down);
                }
            }
            RouteTarget::Backspace => {
                if let Some(m) = self.chat_session.palette.as_mut() {
                    m.on_backspace();
                }
            }
            // 仅纯字符进入检索缓冲(route_command 已排除 Ctrl 组合)。
            RouteTarget::PaletteInput(c) => {
                if let Some(m) = self.chat_session.palette.as_mut() {
                    m.on_input(c);
                }
            }
            // Release 事件与其余未绑定键:忽略(Ignored)。
            _ => {}
        }
    }
}

/// RouteTarget 执行表的一条规则:判别式 → 处理器
///
/// WHY 以代表值存储 + 判别式匹配:`RouteTarget` 多数变体带载荷
/// (PanelJump(PanelId)/GlobalAction(&str)/EnterMode(RouterMode)...),
/// 执行语义按**判别式**分派,载荷由处理器从实参 `target` 提取;
/// 代表值的载荷仅为占位,不参与匹配(见 `apply_route_target`)。
struct RouteTargetEntry {
    /// 该判别式的代表目标(仅 `mem::discriminant` 参与匹配)
    representative: RouteTarget,
    /// 执行处理器(签名同旧 match arm 语义:`(app, 目标, 触发键)`)
    handler: fn(&mut TuiApp, RouteTarget, KeyEvent),
}

/// Normal/GPrefix 上下文的 RouteTarget 执行表(event_loop.rs 旧
/// `apply_route_target` match 链的声明式等价:每个判别式恰一行,无空路由;
/// 新增变体须同步本表与完整性单测,否则查表落空静默无操作)。
const ROUTE_TARGET_TABLE: &[RouteTargetEntry] = &[
    RouteTargetEntry {
        representative: RouteTarget::Quit,
        handler: TuiApp::route_execute_quit,
    },
    RouteTargetEntry {
        representative: RouteTarget::PanelJump(PanelId::Quest),
        handler: TuiApp::route_execute_panel_jump,
    },
    RouteTargetEntry {
        representative: RouteTarget::FocusCycle { forward: true },
        handler: TuiApp::route_execute_focus_cycle,
    },
    RouteTargetEntry {
        representative: RouteTarget::ScrollTop,
        handler: TuiApp::route_execute_scroll,
    },
    RouteTargetEntry {
        representative: RouteTarget::ScrollBottom,
        handler: TuiApp::route_execute_scroll,
    },
    RouteTargetEntry {
        representative: RouteTarget::ThemeCycle,
        handler: TuiApp::route_execute_theme_cycle,
    },
    RouteTargetEntry {
        representative: RouteTarget::RatioAdjust { increase: true },
        handler: TuiApp::route_execute_ratio_adjust,
    },
    RouteTargetEntry {
        representative: RouteTarget::GlobalAction("representative.action"),
        handler: TuiApp::route_execute_global_action,
    },
    RouteTargetEntry {
        representative: RouteTarget::EnterSlash,
        handler: TuiApp::route_execute_enter_slash,
    },
    RouteTargetEntry {
        representative: RouteTarget::OpenPalette,
        handler: TuiApp::route_execute_open_palette,
    },
    RouteTargetEntry {
        representative: RouteTarget::OpenActionMenu,
        handler: TuiApp::route_execute_open_action_menu,
    },
    RouteTargetEntry {
        representative: RouteTarget::EnterMode(RouterMode::Normal),
        handler: TuiApp::route_execute_enter_mode,
    },
    RouteTargetEntry {
        representative: RouteTarget::FocusPaneDir(crate::input::PaneDir::Left),
        handler: TuiApp::route_execute_focus_pane_dir,
    },
    RouteTargetEntry {
        representative: RouteTarget::FocusPanel,
        handler: TuiApp::route_execute_focus_panel,
    },
    RouteTargetEntry {
        representative: RouteTarget::ToggleViewMode,
        handler: TuiApp::route_execute_toggle_view_mode,
    },
    // —— 以下目标不由 Normal/GPrefix 路由产生(Insert/Command/Slash 模式专属
    // 或已在上游处理),以无操作处理器兜底保证"每判别式恰一行"的完备不变量 ——
    RouteTargetEntry {
        representative: RouteTarget::ExitMode,
        handler: route_no_op,
    },
    RouteTargetEntry {
        representative: RouteTarget::InsertChar('x'),
        handler: route_no_op,
    },
    RouteTargetEntry {
        representative: RouteTarget::PaletteInput('x'),
        handler: route_no_op,
    },
    RouteTargetEntry {
        representative: RouteTarget::PaletteMove { down: true },
        handler: route_no_op,
    },
    RouteTargetEntry {
        representative: RouteTarget::Backspace,
        handler: route_no_op,
    },
    RouteTargetEntry {
        representative: RouteTarget::Submit,
        handler: route_no_op,
    },
    RouteTargetEntry {
        representative: RouteTarget::SlashComplete,
        handler: route_no_op,
    },
    RouteTargetEntry {
        representative: RouteTarget::MentionComplete,
        handler: route_no_op,
    },
    RouteTargetEntry {
        representative: RouteTarget::HistoryPrev,
        handler: route_no_op,
    },
    RouteTargetEntry {
        representative: RouteTarget::HistoryNext,
        handler: route_no_op,
    },
    RouteTargetEntry {
        representative: RouteTarget::Ignored,
        handler: route_no_op,
    },
];

/// 无操作处理器:该判别式在 Normal/GPrefix 上下文无执行语义(旧 match 的
/// 显式 `| ... => {}` 兜底组的表驱动等价)
fn route_no_op(_app: &mut TuiApp, _target: RouteTarget, _key: KeyEvent) {}

impl TuiApp {
    /// 执行 Quit 目标(Concord W4/W5:Chat 视图 Esc/q 走失焦/rewind 链;
    /// Dashboard 保留 q/Esc 退出肌肉记忆;quit_requires_confirm 开启时先弹确认框)
    fn route_execute_quit(&mut self, target: RouteTarget, key: KeyEvent) {
        debug_assert!(matches!(target, RouteTarget::Quit));
        // Concord W4/W5:Chat 视图 Esc 与 q 均不退出,走失焦/rewind 链
        // (方案 §7.4:q | 退出(Dashboard)/失焦(Chat));退出统一走
        // /exit。Dashboard 保留 q/Esc 退出肌肉记忆,零回归。
        if (key.code == KeyCode::Esc || key.code == KeyCode::Char('q'))
            && self.state.view_mode == crate::types::ViewMode::Chat
        {
            self.handle_chat_esc();
            return;
        }
        // 退出安全(quit_requires_confirm):开启时先弹确认框,左/右键切到 Yes
        // 后 Enter 才真正退出;默认关闭保持 q/Esc 立即退出行为零回归。
        // 确认命令复用 apply_confirm_command 已有的 "quit" 分支。
        if self.config.quit_requires_confirm {
            self.state.popup_stack.push(PopupKind::Confirm {
                prompt: crate::t!("status.quit_confirm").to_string(),
                on_confirm: "quit".into(),
                confirmed: false,
            });
        } else {
            self.quit();
        }
    }

    /// 执行 PanelJump 目标(数字键/F 键/g 前缀面板直达)
    fn route_execute_panel_jump(&mut self, target: RouteTarget, _key: KeyEvent) {
        let RouteTarget::PanelJump(id) = target else {
            return;
        };
        // I-2(2026-09-06 评估):Chat 视图全屏渲染会话流,面板切换
        // 对用户不可见(仅状态栏面板名变化,体感"按了没反应")。
        // 与其静默切走,不如诚实提示 Dashboard 路径;数字键/F 键
        // 同经此 arm,一并覆盖。
        if self.state.view_mode == crate::types::ViewMode::Chat {
            self.state
                .set_status(crate::t!("hint.panel_switch_in_chat"), Severity::Info);
        } else if self.panel_index(id).is_none() {
            // 未注册面板的跳转不再静默失败:状态栏提示,避免死键无感知
            // (如 g5 → Timeline,TimelinePanel 有实现但未进入面板循环)。
            self.state
                .set_status(format!("Panel {id:?} is not registered"), Severity::Warning);
        } else {
            self.switch_panel_to(id);
        }
    }

    /// 执行 FocusCycle 目标(Tab 正向 / Shift+Tab 反向焦点轮转;
    /// Chat 视图下 Tab 提示 / Shift+Tab 循环审批模式)
    fn route_execute_focus_cycle(&mut self, target: RouteTarget, _key: KeyEvent) {
        let RouteTarget::FocusCycle { forward } = target else {
            return;
        };
        // Concord W4 T4.1:Chat 视图下 Shift+Tab 循环审批模式
        // (方案 §7.4 模式内分义);Dashboard 保留原焦点环语义
        if !forward && self.state.view_mode == crate::types::ViewMode::Chat {
            self.cycle_approval_mode();
        } else if forward && self.state.view_mode == crate::types::ViewMode::Chat {
            // I-2:Chat 视图下 Tab 与数字键同理 —— 面板切换不可见,
            // 诚实提示而非静默切换(与 Shift+Tab=审批模式形成对称)
            self.state
                .set_status(crate::t!("hint.panel_switch_in_chat"), Severity::Info);
        } else if forward {
            self.switch_panel_next();
        } else {
            self.switch_panel_prev();
        }
    }

    /// 执行 ScrollTop/ScrollBottom 目标(滚动当前焦点面板)
    fn route_execute_scroll(&mut self, target: RouteTarget, _key: KeyEvent) {
        let focused = self.focus_manager.focused();
        if let Some(idx) = self.panel_index(focused) {
            match target {
                RouteTarget::ScrollTop => self.panels[idx].scroll_to_top(&mut self.state),
                RouteTarget::ScrollBottom => self.panels[idx].scroll_to_bottom(&mut self.state),
                // 表按判别式分派,其余判别式不会落入(完整性单测守护)
                _ => {}
            }
        }
    }

    /// 执行 ThemeCycle 目标(`t` 键循环切换主题,纯 UI 机械键)
    fn route_execute_theme_cycle(&mut self, target: RouteTarget, _key: KeyEvent) {
        debug_assert!(matches!(target, RouteTarget::ThemeCycle));
        self.cycle_theme_action();
    }

    /// 执行 RatioAdjust 目标(Ctrl+↑/↓ 调整主面板占比)
    fn route_execute_ratio_adjust(&mut self, target: RouteTarget, _key: KeyEvent) {
        let RouteTarget::RatioAdjust { increase } = target else {
            return;
        };
        self.adjust_main_panel_ratio(increase);
    }

    /// 执行 GlobalAction 目标(Action 支持的全局键统一经派发桥接:
    /// locale/layout/companion/help/export;均在 dispatch_action 有本地臂,
    /// 不会回退发事件)
    fn route_execute_global_action(&mut self, target: RouteTarget, _key: KeyEvent) {
        let RouteTarget::GlobalAction(action_id) = target else {
            return;
        };
        self.dispatch_action(action_id, "{}".to_string(), ActionSource::Panel);
    }

    /// 执行 EnterSlash 目标(`/` 与 `:` 同进斜杠命令模式;`:` 为废弃窗口期
    /// 别名,展示一次性弃用提示后不重复打扰)
    fn route_execute_enter_slash(&mut self, target: RouteTarget, key: KeyEvent) {
        debug_assert!(matches!(target, RouteTarget::EnterSlash));
        let via_colon = key.code == KeyCode::Char(':');
        self.state.input_mode = InputMode::Slash;
        self.state.input_buffer.clear();
        self.state.slash_selected = 0;
        if via_colon && !self.state.colon_deprecation_shown {
            self.state.colon_deprecation_shown = true;
            self.state.set_status(
                crate::t!("status.colon_deprecated").to_string(),
                Severity::Warning,
            );
        }
    }

    /// 执行 OpenPalette 目标(Ctrl+P 打开统一命令面板 overlay)
    fn route_execute_open_palette(&mut self, target: RouteTarget, _key: KeyEvent) {
        debug_assert!(matches!(target, RouteTarget::OpenPalette));
        self.open_palette();
    }

    /// 执行 OpenActionMenu 目标(bare `a` 打开焦点面板上下文动作菜单)
    fn route_execute_open_action_menu(&mut self, target: RouteTarget, _key: KeyEvent) {
        debug_assert!(matches!(target, RouteTarget::OpenActionMenu));
        self.open_action_menu();
    }

    /// 执行 EnterMode 目标(Insert/GPrefix/WPrefix 三态入口;
    /// Normal/Command/Slash 不会由 Normal 路由产生,兜底无操作——旧 match 同)
    fn route_execute_enter_mode(&mut self, target: RouteTarget, _key: KeyEvent) {
        let RouteTarget::EnterMode(mode) = target else {
            return;
        };
        match mode {
            RouterMode::Insert => {
                self.state.input_mode = InputMode::Insert;
                self.state.input_buffer.clear();
            }
            RouterMode::GPrefix => {
                self.state.g_prefix = true;
            }
            RouterMode::WPrefix => {
                self.state.w_prefix = true;
            }
            // Normal/Command/Slash 不经 Normal 路由产生(旧 match 显式兜底组)
            _ => {}
        }
    }

    /// 执行 FocusPaneDir 目标(Ctrl+W 前缀次键:按窗格几何切活跃窗格)
    fn route_execute_focus_pane_dir(&mut self, target: RouteTarget, _key: KeyEvent) {
        let RouteTarget::FocusPaneDir(dir) = target else {
            return;
        };
        self.focus_pane_dir(dir);
    }

    /// 执行 FocusPanel 目标(交由当前活跃窗格处理,Stage 2 伴随焦点感知)
    fn route_execute_focus_panel(&mut self, _target: RouteTarget, key: KeyEvent) {
        self.delegate_key_to_active_panel(key);
    }

    /// 执行 ToggleViewMode 目标(Concord W3 T3.4:`\` 互切 Chat⇄Dashboard)
    fn route_execute_toggle_view_mode(&mut self, target: RouteTarget, _key: KeyEvent) {
        debug_assert!(matches!(target, RouteTarget::ToggleViewMode));
        self.toggle_view_mode();
    }
}

// ============================================================
// 第三层:本地动作派发表(action_id → 处理器)
// ============================================================

/// 本地即时动作派发表:`dispatch_action` 本地臂的单一事实源(action_id → 处理器;
/// 键序与 app/tests.rs 的 `LOCAL_ACTION_IDS` 分类清单一致,由单测交叉锚定)。
///
/// 表内未列出的动作一律落入编排兜底(发布 `TuiActionRequested` 等回执)——
/// "本地 or 编排"的路由分类不变量见 `dispatch_action` 文档。
const LOCAL_ACTION_TABLE: &[(&str, fn(&mut TuiApp))] = &[
    ("config.edit", TuiApp::open_config_menu),
    ("export.run", TuiApp::handle_export_command),
    ("monitor.pause_sampling", TuiApp::toggle_monitor_pause),
    ("monitor.time_window", TuiApp::cycle_monitor_window),
    ("panel.drill_down", TuiApp::drill_down_action),
    ("quest.jump", TuiApp::jump_to_quest_events_action),
    ("system.open_help", TuiApp::open_help_action),
    ("system.toggle_locale", TuiApp::toggle_locale_action),
    ("view.apply_saved", TuiApp::apply_saved_view_action),
    ("view.cycle_companion", TuiApp::cycle_companion_action),
    ("view.focus_pane", TuiApp::focus_pane_action),
    ("view.switch_layout", TuiApp::cycle_layout_action),
    ("view.toggle_companion", TuiApp::toggle_companion_action),
    ("viz.switch_dimension", TuiApp::switch_viz_dimension),
];

impl TuiApp {
    /// 统一派发 action_id 为具体行为(M2 增量2:三入口统一派发桥接)
    ///
    /// WHY 桥接而非仅发事件:命令面板 Enter 需产生"真实效果",但既有可用路径分两类——
    /// - **本地即时效果**(无参数):切换语言/布局、打开帮助,直接调用既有本地方法
    ///   (经 [`LOCAL_ACTION_TABLE`] 查表执行,表即本地臂单一事实源);
    /// - **需编排器消费的动作**(agent.chat/quest.*/task.* 等,多含参数):当前无本地
    ///   通路,回退发布 `TuiActionRequested`,交 chimera-cli QueryLoop 编排(M3 落地)。
    ///
    /// 面板上下文动作仍走各自的 `TuiCommand` 变体(带 quest_id 等参数),不经此桥接;
    /// 本方法只服务"无参数、来源为命令面板/斜杠"的统一入口。
    ///
    /// # 路由分类不变量(PS-2 F-7,守护见 `mod tests` 的「动作路由分类不变量」一节)
    /// 本函数的路由分类 = "表内本地执行 / 表外发布编排器"(`_ =>` 兜底)。
    /// 忘记为新增的本地动作登记表行,会静默落入兜底 → 事件发往
    /// 无 handler 的编排器 → 悬挂至 [`ACTION_TIMEOUT`] → 用户看到误导性超时提示。
    ///
    /// **因此:新增/删除本地动作时,必须同步更新 [`LOCAL_ACTION_TABLE`] 与
    /// `src/app/tests.rs` 中 `LOCAL_ACTION_IDS` / `ORCHESTRATED_ACTION_IDS`
    /// 两份清单**——该测试会逐动作实调本函数,断言"声明为本地者不发布、
    /// 声明为编排者必发布",并校验清单与 `ActionRegistry` 全集一致(遗漏即测试红);
    /// 本模块完整性单测再交叉锚定"表键集合 == LOCAL_ACTION_IDS"。
    pub(crate) fn dispatch_action(
        &mut self,
        action_id: &str,
        payload: String,
        source: ActionSource,
    ) {
        // §1.3b:quest.pause/resume/cancel 若焦点面板有选中 Quest,注入 quest_id 精确定位;
        // 其余动作(agent.chat/quest.start 需 query、task.* 已推迟)payload 原样透传。
        let payload = if matches!(action_id, "quest.pause" | "quest.resume" | "quest.cancel") {
            self.enrich_payload_with_focused_quest(payload)
        } else {
            payload
        };
        // 本地即时动作:查表命中即执行(原 match 本地臂的表驱动等价)
        if let Some(handler) = LOCAL_ACTION_TABLE
            .iter()
            .find(|(id, _)| *id == action_id)
            .map(|(_, handler)| handler)
        {
            handler(self);
            return;
        }
        // —— 编排域(quest.*/task.*/agent.chat):发布 TuiActionRequested,由 chimera-cli
        // Action 编排器消费并回发 Completed/Failed(P0 已接线,反馈经 ActionFeedbackSync 上屏)——
        // P1:为本次请求生成唯一 request_id 作为回执配对主键。
        // WHY 先自增后格式化:首个请求为 "tui-1",与人类计数一致,便于审计。
        self.state.action_request_seq += 1;
        let request_id = format!("tui-{}", self.state.action_request_seq);
        // 记录本次请求的截止时刻;收到同一 request_id 的终态反馈时由
        // `update` 精准移除(不再全局清空,避免并发请求互相覆盖)
        self.state.pending_actions.insert(
            request_id.clone(),
            std::time::Instant::now() + ACTION_TIMEOUT,
        );
        self.publish_control_event(NexusEvent::TuiActionRequested {
            metadata: EventMetadata::new("chimera-tui"),
            request_id,
            action_id: action_id.to_string(),
            payload,
            source,
        });
    }
}

// ============================================================
// 完整性单测:无空路由 / 无重复注册(PanelId enum 实测变体数驱动)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::RouterMode;
    use crate::types::TuiState;

    /// 构造 Press 按键(默认无修饰键)
    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// 全部 RouteTarget 变体的代表值(每判别式一个;载荷仅占位,
    /// 匹配与分发只认判别式,见 `RouteTargetEntry`)
    fn route_target_representatives() -> [RouteTarget; 26] {
        [
            RouteTarget::Quit,
            RouteTarget::PanelJump(PanelId::Quest),
            RouteTarget::FocusCycle { forward: true },
            RouteTarget::ScrollTop,
            RouteTarget::ScrollBottom,
            RouteTarget::ThemeCycle,
            RouteTarget::RatioAdjust { increase: true },
            RouteTarget::GlobalAction("representative.action"),
            RouteTarget::EnterSlash,
            RouteTarget::OpenPalette,
            RouteTarget::OpenActionMenu,
            RouteTarget::EnterMode(RouterMode::Insert),
            RouteTarget::FocusPaneDir(crate::input::PaneDir::Left),
            RouteTarget::FocusPanel,
            RouteTarget::ToggleViewMode,
            RouteTarget::ExitMode,
            RouteTarget::InsertChar('x'),
            RouteTarget::PaletteInput('x'),
            RouteTarget::PaletteMove { down: true },
            RouteTarget::Backspace,
            RouteTarget::Submit,
            RouteTarget::SlashComplete,
            RouteTarget::MentionComplete,
            RouteTarget::HistoryPrev,
            RouteTarget::HistoryNext,
            RouteTarget::Ignored,
        ]
    }

    /// 穷举守卫:`input/router.rs` 新增 RouteTarget 变体时,本 match 因
    /// 非穷尽而**编译失败**,强制同步代表值清单 + ROUTE_TARGET_TABLE
    /// (下方完整性测试再防表内漂移;双保险因表查询落空是静默无操作)。
    fn route_target_exhaustiveness_guard(t: RouteTarget) {
        match t {
            RouteTarget::GlobalAction(_)
            | RouteTarget::EnterMode(_)
            | RouteTarget::ExitMode
            | RouteTarget::OpenPalette
            | RouteTarget::OpenActionMenu
            | RouteTarget::EnterSlash
            | RouteTarget::SlashComplete
            | RouteTarget::ToggleViewMode
            | RouteTarget::MentionComplete
            | RouteTarget::HistoryPrev
            | RouteTarget::HistoryNext
            | RouteTarget::FocusCycle { .. }
            | RouteTarget::FocusPanel
            | RouteTarget::FocusPaneDir(_)
            | RouteTarget::InsertChar(_)
            | RouteTarget::PaletteInput(_)
            | RouteTarget::PaletteMove { .. }
            | RouteTarget::Backspace
            | RouteTarget::Submit
            | RouteTarget::Quit
            | RouteTarget::PanelJump(_)
            | RouteTarget::ScrollTop
            | RouteTarget::ScrollBottom
            | RouteTarget::ThemeCycle
            | RouteTarget::RatioAdjust { .. }
            | RouteTarget::Ignored => {}
        }
    }

    #[test]
    fn route_target_table_complete_and_unique() {
        let reps = route_target_representatives();
        // 编译期穷举驱动:新增变体未同步清单/表时,先在此编译失败
        route_target_exhaustiveness_guard(reps[0]);

        // 无空路由:表行数 == 变体数(每判别式恰一行,含 no-op 兜底组)
        assert_eq!(
            ROUTE_TARGET_TABLE.len(),
            reps.len(),
            "ROUTE_TARGET_TABLE 应为每个 RouteTarget 判别式恰设一行(无空路由)"
        );
        // 无重复注册 + 命中唯一:每个代表值恰命中一条表行
        for rep in reps {
            let hits = ROUTE_TARGET_TABLE
                .iter()
                .filter(|e| {
                    std::mem::discriminant(&e.representative) == std::mem::discriminant(&rep)
                })
                .count();
            assert_eq!(
                hits, 1,
                "{rep:?} 应恰有一条执行表条目(空路由或重复注册即红)"
            );
        }
    }

    /// 三张弹窗键表 — 逐表断言无重复注册(同表内键位两两不同)
    #[test]
    fn popup_key_tables_have_unique_patterns() {
        for (name, table) in [
            ("SCROLL_POPUP_KEY_TABLE", SCROLL_POPUP_KEY_TABLE),
            ("ACTION_MENU_KEY_TABLE", ACTION_MENU_KEY_TABLE),
            ("CONFIG_MENU_KEY_TABLE", CONFIG_MENU_KEY_TABLE),
        ] {
            for (i, a) in table.iter().enumerate() {
                for b in &table[i + 1..] {
                    assert_ne!(
                        a.pattern, b.pattern,
                        "{name} 含重复键位注册(首个命中生效,重复 arm 是死代码)"
                    );
                }
            }
        }
    }

    /// 弹窗键表行为等价抽查:迁移后的表驱动路径与旧 match 链行为一致
    /// (滚动弹窗 Esc 关闭 / Confirm 左右切换 + Enter 执行 / 动作菜单 kj 导航)
    #[test]
    fn popup_tables_behavioral_spot_checks() {
        // WHY locale 锁:本测试经 Ctrl+L 真实切换全局 locale,必须与依赖
        // 中文文案的并行渲染测试互斥,并在结束时恢复原 locale(既有 flaky 模式,
        // 同 tests.rs 的 locale-sensitive 测试)
        let _locale_guard = crate::i18n::locale_test_guard();
        let mut app = super::super::tests::make_app().expect("make_app should succeed");

        // 滚动型 Detail 弹窗:Esc 命中 Close → 弹窗栈清空
        app.state.popup_stack.push(PopupKind::Detail {
            title: "T".into(),
            content: "body".into(),
            scroll: 0,
        });
        app.handle_popup_key(press(KeyCode::Esc));
        assert!(app.state.popup_stack.is_empty());

        // Confirm 弹窗:Right 切到 Yes(confirmed=false → true),Enter 执行 on_confirm
        app.state.popup_stack.push(PopupKind::Confirm {
            prompt: "quit?".into(),
            on_confirm: "quit".into(),
            confirmed: false,
        });
        app.handle_popup_key(press(KeyCode::Right));
        app.handle_popup_key(press(KeyCode::Enter));
        assert!(app.state.popup_stack.is_empty());
        assert!(!app.state().running, "Confirm(Yes) Enter 应执行 quit 命令");

        // Ctrl+L 弹窗内全局热键:popup 保持打开,locale 切换生效(状态栏反馈)
        app.state.running = true;
        app.state.popup_stack.push(PopupKind::Detail {
            title: "T".into(),
            content: "body".into(),
            scroll: 0,
        });
        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));
        assert!(
            !app.state.popup_stack.is_empty(),
            "Ctrl+L 不应关闭弹窗(热键收口后弹窗保持打开)"
        );

        // 配置菜单:MenuDown 经弹窗种类路由到 move_config_menu_selection
        // (与动作菜单的 move_action_menu_selection 分流——表执行器的种类
        // 路由在此锁死,防两侧栈操作串线回归)
        app.handle_popup_key(press(KeyCode::Esc)); // 关闭 Detail
        app.dispatch_action("config.edit", "{}".to_string(), ActionSource::Panel);
        assert!(
            matches!(
                app.state.popup_stack.current(),
                Some(PopupKind::ConfigMenu { .. })
            ),
            "config.edit 应打开配置菜单"
        );
        app.handle_popup_key(press(KeyCode::Down));
        assert_eq!(
            app.state.popup_stack.config_menu_selected(),
            Some(1),
            "Down 应经配置菜单栈操作选中下标 1"
        );
        app.handle_popup_key(press(KeyCode::Esc));
    }

    /// 本地动作表与分类清单交叉锚定:表键集合 == tests.rs LOCAL_ACTION_IDS
    /// (后者再经行为测试锚定"本地者不发布事件",构成 表 ↔ 清单 ↔ 行为 闭环)
    #[test]
    fn local_action_table_matches_classification_list() {
        let mut table_ids: Vec<&str> = LOCAL_ACTION_TABLE.iter().map(|(id, _)| *id).collect();
        // 无重复注册
        let sorted_len = table_ids.len();
        table_ids.sort_unstable();
        table_ids.dedup();
        assert_eq!(
            table_ids.len(),
            sorted_len,
            "LOCAL_ACTION_TABLE 不应含重复 action_id"
        );

        let mut declared: Vec<&str> = super::super::tests::LOCAL_ACTION_IDS.to_vec();
        declared.sort_unstable();
        assert_eq!(
            table_ids, declared,
            "派发表键集合应与 app/tests.rs LOCAL_ACTION_IDS 分类清单一致(漂移即红)"
        );
    }

    /// 27 面板 × 常用键:每面板实例对每个常用键都给出确定性响应
    /// (Option<TuiCommand> —— 展示型面板返回 None 亦是合法路由,即"已路由到
    /// 面板且面板决定无操作",与"空路由/静默丢弃"不同);同时逐键断言应用层
    /// Normal 路由不产生 Ignored —— 所有面板经 FocusPanel 委托获得按键。
    ///
    /// PanelId::ALL 为 27 变体穷举清单(新增变体时 types.rs 穷举 match 编译失败);
    /// Esc/q 不在常用键集合内 —— Normal 全局表将它们路由为 Quit(既有语义),
    /// 面板委托权被全局键截获。
    #[test]
    fn all_27_panels_common_keys_deterministic_route() {
        assert_eq!(
            PanelId::ALL.len(),
            27,
            "PanelId 变体数锚定(ALL 清单与 enum 的同步由 types.rs 穷举测试守护)"
        );
        // 跨面板通用交互键:导航/选中/翻页/触发(与 InputRouter Normal 表正交,
        // 均非全局快捷键 → 必须落到 FocusPanel 委托链)
        let common_keys = [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Enter,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::Char(' '),
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Char('f'),
            KeyCode::Char('S'),
        ];

        // 应用层:Normal 模式常用键无 Ignored(空路由)——与面板无关,逐键一次
        for key in common_keys {
            let target = InputRouter::route(RouterMode::Normal, press(key));
            assert!(
                !matches!(target, RouteTarget::Ignored),
                "Normal 模式常用键 {key:?} 不应产生 Ignored(空路由)"
            );
            assert!(
                matches!(target, RouteTarget::FocusPanel),
                "常用键 {key:?} 应落到 FocusPanel 委托(面板第一处理权),实际 {target:?}"
            );
        }

        // 面板层:27 面板 × 常用键全量过键(不 panic/不挂起 = 确定性响应;
        // make_panel 穷举 27 变体,未注册面板(InjectionStrategy)亦有实现可构造)
        for pid in PanelId::ALL {
            let mut panel = super::super::make_panel(*pid);
            let mut state = TuiState::new();
            for key in common_keys {
                let _ = panel.handle_key(press(key), &mut state);
            }
        }
    }
}
