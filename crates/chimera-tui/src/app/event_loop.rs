//! 事件处理循环 — 键盘路由、动作派发与终端主循环
//!
//! 包含 [`TuiApp::run`]、[`TuiApp::event_loop`]、键盘处理、
//! 动作派发(`dispatch_action`)、事件发布(`publish_*`)等方法。
//!
//! 对应架构层:L10 Interface

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, Stdout};
use std::time::Duration;

use super::{TuiApp, COMPANION_MIN_WIDTH, COMPANION_WIDTH, IDE_CONTEXT_WIDTH, IDE_SIDEBAR_WIDTH};
use crate::command_palette::CommandPaletteModel;
use crate::config::TuiConfig;
use crate::data::ExportFormat;
use crate::error::TuiError;
use crate::input::{InputRouter, PaneDir, RouteTarget, RouterMode};
use crate::popup::{PopupKind, Severity};
use crate::types::{InputMode, LayoutMode, PanelId, TuiCommand};
use event_bus::{ActionSource, EventMetadata, NexusEvent, VoteValue};

impl TuiApp {
    /// 处理键盘事件,按当前输入模式和弹窗状态路由到对应处理器。
    pub fn handle_key_event(&mut self, key: KeyEvent) {
        // WHY 检查 KeyEventKind:crossterm 在 Windows 上会触发 Release 事件,
        // 只处理 Press 避免重复响应(平台兼容性)
        if key.kind != KeyEventKind::Press {
            return;
        }

        // 弹窗激活时:优先处理弹窗级交互
        if !self.state.popup_stack.is_empty() {
            self.handle_popup_key(key);
            return;
        }

        // M2.2:统一命令面板打开时,键盘事件全部路由给面板(模糊检索/导航/执行),
        // 与 InputRouter 的 Command 模式语义一致(Esc 关闭 / ↑↓ 选择 / Enter 执行)。
        // Task 1.15.4:palette 移至 chat_session
        if self.chat_session.palette.is_some() {
            self.handle_palette_key(key);
            return;
        }

        // 命令/搜索模式:委托给命令面板(`:` 带参命令 / `/` 关键字过滤)
        if matches!(
            self.state.input_mode,
            InputMode::Command | InputMode::Search
        ) {
            if let Some(cmd) = self.command_palette.handle_key(key, &mut self.state) {
                self.apply_command(cmd);
            }
            return;
        }

        // Insert 模式(M3a):原始文本输入,经 InputRouter 的 Insert 表路由
        if self.state.input_mode == InputMode::Insert {
            self.handle_insert_key(key);
            return;
        }

        // 普通模式:经 InputRouter 计算路由目标,再由 apply_route_target 执行。
        // WHY 经路由器:按键归属(全局键/模式切换/面板跳转/焦点面板)的单一事实源在
        // InputRouter,app 只负责"执行意图",与"面板表达意图、App 执行"设计一致。
        // g / Ctrl+W 前缀为瞬态:置位后下一键在对应模式解析,解析后立即复位。
        let mode = if self.state.g_prefix {
            RouterMode::GPrefix
        } else if self.state.w_prefix {
            RouterMode::WPrefix
        } else {
            RouterMode::Normal
        };
        let mut target = InputRouter::route(mode, key);
        // 消费瞬态前缀态(在 apply 前复位,保证本键若为 EnterMode(前缀) 的置位不被清)。
        self.state.g_prefix = false;
        self.state.w_prefix = false;
        // g 前缀遇非预期次键:退出前缀态后,该键按面板级委托处理(与旧 handle_global_key
        // `_ => return false` 行为一致——不重新触发 Normal 全局键,避免 `gq` 误退出)。
        // WPrefix 遇非预期次键(ExitMode)直接取消,不回退委托(Ctrl+W 后随机键 = 取消)。
        if mode == RouterMode::GPrefix && target == RouteTarget::ExitMode {
            target = RouteTarget::FocusPanel;
        }
        self.apply_route_target(target, key);
    }

    /// 执行 InputRouter 计算出的路由目标(Normal/GPrefix 上下文)
    ///
    /// WHY 集中执行:路由器只表达"按键归属意图",具体副作用(退出/切面板/滚动/
    /// 主题/比例/派发动作/进入模式/面板委托)在此统一落地,与"面板表达意图、
    /// App 执行"设计一致。Insert/Command 模式专属目标(InsertChar/Palette*/Submit 等)
    /// 不会由 Normal/GPrefix 路由产生,此处按无操作兜底以保证穷尽匹配。
    fn apply_route_target(&mut self, target: RouteTarget, key: KeyEvent) {
        match target {
            RouteTarget::Quit => self.quit(),
            RouteTarget::PanelJump(id) => self.switch_panel_to(id),
            RouteTarget::FocusCycle { forward } => {
                if forward {
                    self.switch_panel_next();
                } else {
                    self.switch_panel_prev();
                }
            }
            RouteTarget::ScrollTop => {
                let focused = self.focus_manager.focused();
                if let Some(idx) = self.panel_index(focused) {
                    self.panels[idx].scroll_to_top(&mut self.state);
                }
            }
            RouteTarget::ScrollBottom => {
                let focused = self.focus_manager.focused();
                if let Some(idx) = self.panel_index(focused) {
                    self.panels[idx].scroll_to_bottom(&mut self.state);
                }
            }
            RouteTarget::ThemeCycle => self.cycle_theme_action(),
            RouteTarget::RatioAdjust { increase } => self.adjust_main_panel_ratio(increase),
            // Action 支持的全局键统一经派发桥接(locale/layout/companion/help/export);
            // 均在 dispatch_action 有本地 arm,不会回退发事件。
            RouteTarget::GlobalAction(action_id) => {
                self.dispatch_action(action_id, "{}".to_string(), ActionSource::Panel);
            }
            // 模式入口:`:` 命令栏 / `/` 搜索 / Ctrl+P palette / `i` Insert / `g` 前缀
            RouteTarget::EnterCommandBar => {
                self.state.input_mode = InputMode::Command;
                self.state.input_buffer.clear();
            }
            RouteTarget::EnterSearch => {
                self.state.input_mode = InputMode::Search;
                self.state.input_buffer.clear();
            }
            RouteTarget::OpenPalette => self.open_palette(),
            RouteTarget::OpenActionMenu => self.open_action_menu(),
            RouteTarget::EnterMode(RouterMode::Insert) => {
                self.state.input_mode = InputMode::Insert;
                self.state.input_buffer.clear();
            }
            RouteTarget::EnterMode(RouterMode::GPrefix) => {
                self.state.g_prefix = true;
            }
            RouteTarget::EnterMode(RouterMode::WPrefix) => {
                self.state.w_prefix = true;
            }
            // Ctrl+W 前缀方向导航:按窗格几何切换活跃窗格(h/l 左右,j/k 上下)
            RouteTarget::FocusPaneDir(dir) => self.focus_pane_dir(dir),
            // 交由当前活跃窗格(Stage 2 伴随焦点感知)处理
            RouteTarget::FocusPanel => self.delegate_key_to_active_panel(key),
            // 以下目标不由 Normal/GPrefix 路由产生(Insert/Command 模式专属或已在上游处理),
            // 兜底无操作以保证穷尽匹配。
            RouteTarget::EnterMode(RouterMode::Normal | RouterMode::Command)
            | RouteTarget::ExitMode
            | RouteTarget::InsertChar(_)
            | RouteTarget::PaletteInput(_)
            | RouteTarget::PaletteMove { .. }
            | RouteTarget::Backspace
            | RouteTarget::Submit
            | RouteTarget::Ignored => {}
        }
    }

    /// 处理 Insert 模式按键(M3a):经 InputRouter 的 Insert 表路由到输入缓冲操作
    ///
    /// WHY 独立方法:Insert 是原始文本输入,与 Normal 的按键归属语义不同
    /// (字符进缓冲、Enter 提交、Esc 退出),单独处理避免与 apply_route_target 混杂。
    /// M3a 阶段 Submit 为占位(不发事件),M3b 接入 Chat 面板后改为发 TuiChatSubmitted。
    fn handle_insert_key(&mut self, key: KeyEvent) {
        match InputRouter::route(RouterMode::Insert, key) {
            RouteTarget::InsertChar(c) => self.state.input_buffer.push(c),
            RouteTarget::Backspace => {
                self.state.input_buffer.pop();
            }
            RouteTarget::ExitMode => {
                self.state.input_mode = InputMode::Normal;
                self.state.input_buffer.clear();
            }
            // Insert 下仍允许极少数全局键(如 Ctrl+L 中英切换),经派发桥接
            RouteTarget::GlobalAction(action_id) => {
                self.dispatch_action(action_id, "{}".to_string(), ActionSource::Chat);
            }
            RouteTarget::Submit => {
                // M3b:非空输入发布 TuiChatSubmitted(经 EventBus 回环由 ChatSync 追加用户消息),
                // 自动切到 Chat 面板;保持 Insert 模式形成 chat REPL(Esc 退出)。
                let text = self.state.input_buffer.trim().to_string();
                if !text.is_empty() {
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
                    self.switch_panel_to(PanelId::Chat);
                }
                self.state.input_buffer.clear();
            }
            // 其余(Ignored 等)在 Insert 下无操作
            _ => {}
        }
    }

    /// 循环切换主题 Dark → Light → HighContrast → Dark(P6.1,原 `t` 全局键)
    ///
    /// WHY 提取为方法:M3a 将 `t` 键路由为 `RouteTarget::ThemeCycle`,执行逻辑集中于此,
    /// 与 `cycle_layout_action` 等其他 *_action 方法风格一致(DRY)。切换后标记所有面板
    /// dirty 触发重绘以应用新配色。
    fn cycle_theme_action(&mut self) {
        let new_theme = self.config.theme.next();
        self.config.theme = new_theme;
        // 标记所有已注册面板为 dirty,确保下一帧重绘
        for panel_id in self.focus_manager.panels() {
            self.state.mark_dirty(*panel_id);
        }
        self.state.status_message = Some((
            format!("{}: {}", crate::t!("status.theme"), new_theme.as_str()),
            Severity::Info,
        ));
    }

    /// 将按键委托给当前活跃窗格处理(Stage 2 伴随焦点感知)
    ///
    /// WHY 活跃窗格优先:面板级键路由到 `active_pane` 指向的窗格面板(M3d 多窗格);
    /// 单窗格时即主焦点面板。路由器返回 `FocusPanel`(非全局键)时经此委托。
    fn delegate_key_to_active_panel(&mut self, key: KeyEvent) {
        // M3d:路由到当前活跃窗格对应的面板(active_pane 是 pane_panels 循环序下标);
        // 越界(窗格数收缩未及钳制)兜底回主焦点面板。
        // Task 1.15.4:active_pane 移至 pane_manager
        let panes = self.pane_panels();
        let target = panes
            .get(self.pane_manager.active_pane)
            .copied()
            .unwrap_or_else(|| self.focus_manager.focused());
        if let Some(idx) = self.panel_index(target) {
            if let Some(cmd) = self.panels[idx].handle_key(key, &mut self.state) {
                self.apply_command(cmd);
            }
        }
    }

    /// 打开统一命令面板(M2.2)
    ///
    /// WHY 复用既有模型:若已存在(之前打开过)则仅 `open()` 复位 query/选择,
    /// 保留其 `ActionRegistry` 副本;首次打开才用内建六域注册表构造,
    /// 避免每次打开都重建注册表(约 21 条描述)。
    fn open_palette(&mut self) {
        // Task 1.15.4:palette 移至 chat_session
        let mut model = self
            .chat_session
            .palette
            .take()
            .unwrap_or_else(CommandPaletteModel::with_builtin_domains);
        model.open();
        self.chat_session.palette = Some(model);
    }

    /// 打开焦点面板的上下文动作菜单(§4.5 入口三:面板动作)
    ///
    /// WHY 从 Registry 组装:动作集由 `panel_context_actions(焦点面板)` 精选,
    /// 展示标题经 `ActionRegistry` + i18n 解析(与命令面板/斜杠同源,locale 感知)。
    /// 选中经 `DispatchAction{source:Panel}` 统一派发,复用三入口执行/反馈管线。
    fn open_action_menu(&mut self) {
        let focused = self.focus_manager.focused();
        let ids = crate::actions::panel_context_actions(focused);
        let registry = crate::actions::ActionRegistry::with_builtin_domains();
        let entries: Vec<(String, String)> = ids
            .into_iter()
            .map(|id| {
                let title = registry
                    .get(id)
                    .map(|d| crate::i18n::tr(d.title_key).to_string())
                    .unwrap_or_else(|| id.to_string());
                (id.to_string(), title)
            })
            .collect();
        self.state
            .popup_stack
            .push(PopupKind::action_menu(focused.as_str(), entries));
    }

    /// 命令面板打开时的键盘处理(M2.2)
    ///
    /// 语义对齐 `InputRouter` 的 Command 模式:Esc 关闭 / ↑↓ 选择 / Enter 执行
    /// 选中动作(经 `DispatchAction` 统一派发,source=Palette)/ 退格 / 字符过滤。
    ///
    /// WHY 逐分支分别借用 `self.palette`:Esc/Enter 需写 `self.palette = None`,
    /// 而导航键需 `&mut` 模型;分开借用避免在同一作用域同时持有可变
    /// 引用与重赋值的借用冲突。
    fn handle_palette_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                // Task 1.15.4:palette 移至 chat_session
                self.chat_session.palette = None;
            }
            KeyCode::Enter => {
                // 先取选中动作 id(&'static str,不借用模型),关闭面板后统一派发。
                let action_id = self
                    .chat_session
                    .palette
                    .as_ref()
                    .and_then(|m| m.selected_action())
                    .map(str::to_string);
                self.chat_session.palette = None;
                if let Some(action_id) = action_id {
                    self.apply_command(TuiCommand::DispatchAction {
                        action_id,
                        payload: "{}".to_string(),
                        source: ActionSource::Palette,
                    });
                }
            }
            KeyCode::Up => {
                if let Some(m) = self.chat_session.palette.as_mut() {
                    m.move_selection(false);
                }
            }
            KeyCode::Down => {
                if let Some(m) = self.chat_session.palette.as_mut() {
                    m.move_selection(true);
                }
            }
            KeyCode::Backspace => {
                if let Some(m) = self.chat_session.palette.as_mut() {
                    m.on_backspace();
                }
            }
            // 排除 Ctrl 组合:仅纯字符进入检索缓冲,Ctrl+X 类快捷键在面板内忽略。
            KeyCode::Char(c) if !key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                if let Some(m) = self.chat_session.palette.as_mut() {
                    m.on_input(c);
                }
            }
            _ => {}
        }
    }

    /// 命令面板是否打开(测试与外部查询用)
    pub fn palette_is_open(&self) -> bool {
        // Task 1.15.4:palette 移至 chat_session
        self.chat_session.palette.is_some()
    }

    /// 伴随面板是否可见(测试与外部查询用)
    pub fn companion_visible(&self) -> bool {
        // Task 1.15.4:companion_visible 移至 pane_manager
        self.pane_manager.companion_visible
    }

    /// 活跃窗格是否为伴随(次)窗格(测试与外部查询用)
    ///
    /// WHY 保留 bool 语义:M3d 后活跃窗格为索引,此访问器映射"活跃窗格是否为第 2 窗格"
    /// (`active_pane == 1`,2 窗格时即 companion),保持 Stage 2 测试与外部契约等价。
    pub fn companion_focused(&self) -> bool {
        // Task 1.15.4:active_pane 移至 pane_manager
        self.pane_manager.active_pane == 1
    }

    /// 当前伴随面板目标(测试与外部查询用;不含可见性判断)
    pub fn companion_panel(&self) -> Option<PanelId> {
        self.companion_target()
    }

    /// 当前活跃窗格索引(M3d,0 = 主窗格;测试与外部查询用)
    pub fn active_pane(&self) -> usize {
        // Task 1.15.4:active_pane 移至 pane_manager
        self.pane_manager.active_pane
    }

    /// 当前可见窗格数(M3d,由 PaneMode + companion_visible 决定;测试与外部查询用)
    pub fn pane_count(&self) -> usize {
        self.pane_panels().len()
    }

    /// M3d:窗格数变化(切布局 / 开关伴随)后钳制活跃窗格索引,越界则复位主区
    ///
    /// WHY 复位而非饱和:切布局 / 开关伴随是显式的视图上下文切换,回到主窗格(0)
    /// 符合直觉且避免焦点滞留已消失的次窗格;`get(active).unwrap_or` 兜底不 panic。
    fn clamp_active_pane(&mut self) {
        // Task 1.15.4:active_pane 移至 pane_manager
        if self.pane_manager.active_pane >= self.pane_panels().len() {
            self.pane_manager.active_pane = 0;
        }
    }

    /// 计算伴随面板目标:显式绑定优先,否则最近使用面板(且非当前焦点),
    /// 无历史则回退到焦点顺序中首个非焦点面板
    ///
    /// WHY 回退链:保证伴随面板开启时总有内容可显示,且永不等于主区面板。
    fn companion_target(&self) -> Option<PanelId> {
        let focused = self.focus_manager.focused();
        // Stage 2:显式绑定优先(且不等于主区面板)
        // Task 1.15.4:bound_companion / prev_panel 移至 pane_manager
        if let Some(bound) = self.pane_manager.bound_companion {
            if bound != focused {
                return Some(bound);
            }
        }
        if let Some(prev) = self.pane_manager.prev_panel {
            if prev != focused {
                return Some(prev);
            }
        }
        self.focus_manager
            .panels()
            .iter()
            .copied()
            .find(|&p| p != focused)
    }

    /// M3d:IDE 侧栏(第三窗格)目标 —— 首个既非主焦点、也非 context 的面板
    ///
    /// WHY 确定性:三窗格 IDE 需要一个稳定的第三面板;取焦点面板顺序中首个
    /// 不等于主焦点与 context 的面板,面板不足 3 个时返回 None(退化为 2 窗格)。
    fn third_pane_target(&self) -> Option<PanelId> {
        let focused = self.focus_manager.focused();
        let context = self.companion_target();
        self.focus_manager
            .panels()
            .iter()
            .copied()
            .find(|&p| p != focused && Some(p) != context)
    }

    /// M3d:是否应显示次窗格(非 Focus 单窗格)—— pane_panels 与 pane_rects 共用判定
    ///
    /// WHY 抽取:窗格计数(路由)与区域切分(渲染)必须用同一规则判断"是否多窗格",
    /// 否则会出现路由认为 2 窗格、渲染只画 1 窗格的错位。Chat 尊重 companion_visible
    /// opt-in(Stage 2 语义);VimSplit/IDE 为内在多窗格。
    fn wants_multi_pane(&self) -> bool {
        use crate::engine::layout::PaneMode;
        match self.state.layout_mode.to_pane_mode() {
            PaneMode::Focus => false,
            // Task 1.15.4:companion_visible 移至 pane_manager
            PaneMode::Chat => self.pane_manager.companion_visible,
            PaneMode::VimSplit | PaneMode::Ide => true,
        }
    }

    /// M3d:当前 PaneMode 下可见窗格的面板列表(循环序 [主, (context), (侧栏)])
    ///
    /// WHY 不含 rect:此列表供键盘路由(`delegate_key_to_active_panel`)与窗格计数
    /// (`focus_pane_action` 的 `w` 循环)使用,与几何无关;渲染另经 `pane_rects` 取区域。
    /// - Focus:[主]
    /// - Chat:companion_visible ? [主, context] : [主](Stage 2 opt-in 保留)
    /// - VimSplit:[主, context](左右等分)
    /// - Ide:[主, context, 侧栏](三窗格;目标不足时省略对应窗格,列表无空洞)
    pub(super) fn pane_panels(&self) -> Vec<PanelId> {
        use crate::engine::layout::PaneMode;
        let main = self.focus_manager.focused();
        let mut panes = vec![main];
        if !self.wants_multi_pane() {
            return panes;
        }
        if let Some(context) = self.companion_target() {
            panes.push(context);
        }
        // IDE 三窗格:在 context 之外再追加侧栏(第三面板)
        if self.state.layout_mode.to_pane_mode() == PaneMode::Ide {
            if let Some(sidebar) = self.third_pane_target() {
                panes.push(sidebar);
            }
        }
        panes
    }

    /// M3d:当前 PaneMode 下各可见窗格的区域(循环序对齐 `pane_panels`)
    ///
    /// WHY 复用引擎 split:用自研 `engine::layout::split` 按 PaneMode 切分主内容区的列
    /// (外层 tabs/status 已由 `layout()` 处理)。返回顺序与 `pane_panels` 一致
    /// ([主, context, 侧栏]);单窗格模式 / 窄视口 / 无次窗格目标退化为 `vec![area]`
    /// (与既有 companion_split 逐字节等价,零回归)。
    pub(super) fn pane_rects(&self, area: ratatui::layout::Rect) -> Vec<ratatui::layout::Rect> {
        use crate::engine::layout::{Constraint, Direction, PaneMode};
        if !self.wants_multi_pane()
            || area.width < COMPANION_MIN_WIDTH
            || self.companion_target().is_none()
        {
            return vec![area];
        }
        let eng_area = crate::engine::from_ratatui_rect(area);
        let to = crate::engine::to_ratatui_rect;
        match self.state.layout_mode.to_pane_mode() {
            // Chat:主(Flex)+ 右 context(固定宽)。循环序 [主, context]。
            PaneMode::Chat => {
                let cols = crate::engine::layout::split(
                    eng_area,
                    Direction::Horizontal,
                    &[Constraint::Flex(1), Constraint::Fixed(COMPANION_WIDTH)],
                );
                vec![to(cols[0]), to(cols[1])]
            }
            // VimSplit:左右等分(Flex1 + Flex1)。循环序 [主, context]。
            PaneMode::VimSplit => {
                let cols = crate::engine::layout::split(
                    eng_area,
                    Direction::Horizontal,
                    &[Constraint::Flex(1), Constraint::Flex(1)],
                );
                vec![to(cols[0]), to(cols[1])]
            }
            // IDE:三栏 [侧栏(左,固定) | 主(中,Flex) | context(右,固定)]。
            // 位置左中右,循环序返回 [主(中), context(右), 侧栏(左)]——各窗格自带
            // 区域,渲染位置与循环索引解耦,保证 active_pane=0 恒为主区。
            PaneMode::Ide => {
                let cols = crate::engine::layout::split(
                    eng_area,
                    Direction::Horizontal,
                    &[
                        Constraint::Fixed(IDE_SIDEBAR_WIDTH),
                        Constraint::Flex(1),
                        Constraint::Fixed(IDE_CONTEXT_WIDTH),
                    ],
                );
                vec![to(cols[1]), to(cols[2]), to(cols[0])]
            }
            // Focus 已在上方 wants_multi_pane=false 提前返回,此处兜底整块。
            PaneMode::Focus => vec![area],
        }
    }

    /// 切换伴随面板可见性(M2 增量3,供 `\` 键与命令面板 `view.toggle_companion` 共用)
    fn toggle_companion_action(&mut self) {
        // Task 1.15.4:companion_visible 移至 pane_manager
        self.pane_manager.companion_visible = !self.pane_manager.companion_visible;
        // M3d:关闭伴随会使 Chat 窗格数 2→1,钳制活跃窗格避免停留已消失的 context。
        self.clamp_active_pane();
        let state_label = if self.pane_manager.companion_visible {
            "on"
        } else {
            "off"
        };
        self.state.status_message = Some((
            format!(
                "{}: {}",
                crate::t!("action.view.toggle_companion"),
                state_label
            ),
            Severity::Info,
        ));
    }

    /// 循环绑定伴随面板到下一个非焦点面板(M2 增量3 Stage 2,供 `]` 与命令面板共用)
    ///
    /// 以焦点面板顺序从当前伴随目标起环形查找下一个 `!= 主焦点` 的面板写入
    /// `bound_companion`,并置 `companion_visible = true`(循环即意图显示)。
    fn cycle_companion_action(&mut self) {
        let focused = self.focus_manager.focused();
        // WHY 克隆为 Vec:既将读面板顺序又要随后可变写 bound_companion,
        // 克隆断开 self 借用(约 19 个 PanelId,Copy,廉价)。
        let panels: Vec<PanelId> = self.focus_manager.panels().to_vec();
        if panels.len() < 2 {
            return;
        }
        // 起点:当前伴随目标位置(无则从主焦点位置起)
        let start = self
            .companion_target()
            .and_then(|c| panels.iter().position(|&p| p == c))
            .unwrap_or_else(|| panels.iter().position(|&p| p == focused).unwrap_or(0));
        let n = panels.len();
        let next = (1..=n)
            .map(|off| panels[(start + off) % n])
            .find(|&p| p != focused);
        if let Some(target) = next {
            // Task 1.15.4:bound_companion / companion_visible 移至 pane_manager
            self.pane_manager.bound_companion = Some(target);
            self.pane_manager.companion_visible = true;
            self.state.status_message = Some((
                format!(
                    "{}: {}",
                    crate::t!("action.view.cycle_companion"),
                    target.as_str()
                ),
                Severity::Info,
            ));
        }
    }

    /// 循环切换活跃窗格(M3d,供 `w` 与命令面板共用)
    ///
    /// 在可见窗格间环形循环(main → context → sidebar → main);单窗格
    /// (Focus / Chat 未开伴随 / 面板不足)无可切换窗格时 no-op 并提示。
    fn focus_pane_action(&mut self) {
        let n = self.pane_panels().len();
        if n <= 1 {
            self.state.status_message = Some((
                format!("{}: n/a", crate::t!("action.view.focus_pane")),
                Severity::Warning,
            ));
            return;
        }
        // Task 1.15.4:active_pane 移至 pane_manager
        self.pane_manager.active_pane = (self.pane_manager.active_pane + 1) % n;
        // 窗格标签:0 = 主区,其余按序号提示(2 窗格时 1 = companion,语义等价 Stage 2)
        let pane = if self.pane_manager.active_pane == 0 {
            "main".to_string()
        } else {
            format!("pane {}", self.pane_manager.active_pane + 1)
        };
        self.state.status_message = Some((
            format!("{}: {}", crate::t!("action.view.focus_pane"), pane),
            Severity::Info,
        ));
    }

    /// 按方向切换活跃窗格(M3 后续 Ctrl+W h/j/k/l 方向导航)
    ///
    /// 用 `pane_rects` 的窗格矩形几何解析目标:在指定方向上取中心坐标最近的邻窗格。
    /// 当前预设布局均为横向列,故 h/l 生效;j/k(上下)当前无候选时 no-op。
    /// 单窗格(rects<=1)或该方向无邻居时 no-op 并提示。
    fn focus_pane_dir(&mut self, dir: PaneDir) {
        // Task 1.15.4:last_area / active_pane 移至 pane_manager
        let main = self.layout(self.pane_manager.last_area)[1];
        let rects = self.pane_rects(main);
        if rects.len() <= 1 {
            self.state.status_message = Some((
                format!("{}: n/a", crate::t!("action.view.focus_pane")),
                Severity::Warning,
            ));
            return;
        }
        let cur = rects
            .get(self.pane_manager.active_pane)
            .copied()
            .unwrap_or(rects[0]);
        let cur_cx = cur.x as i32 + cur.width as i32 / 2;
        let cur_cy = cur.y as i32 + cur.height as i32 / 2;
        // 在指定方向上找中心坐标最近的窗格(水平比 x,垂直比 y)
        let mut best: Option<(usize, i32)> = None;
        for (idx, r) in rects.iter().enumerate() {
            if idx == self.pane_manager.active_pane {
                continue;
            }
            let cx = r.x as i32 + r.width as i32 / 2;
            let cy = r.y as i32 + r.height as i32 / 2;
            let dist = match dir {
                PaneDir::Left => (cx < cur_cx).then_some(cur_cx - cx),
                PaneDir::Right => (cx > cur_cx).then_some(cx - cur_cx),
                PaneDir::Up => (cy < cur_cy).then_some(cur_cy - cy),
                PaneDir::Down => (cy > cur_cy).then_some(cy - cur_cy),
            };
            if let Some(d) = dist {
                let better = match best {
                    None => true,
                    Some((_, bd)) => d < bd,
                };
                if better {
                    best = Some((idx, d));
                }
            }
        }
        match best {
            Some((idx, _)) => {
                // Task 1.15.4:active_pane 移至 pane_manager
                self.pane_manager.active_pane = idx;
                self.state.status_message = Some((
                    format!("{}: pane {}", crate::t!("action.view.focus_pane"), idx + 1),
                    Severity::Info,
                ));
            }
            None => {
                self.state.status_message = Some((
                    format!("{}: n/a", crate::t!("action.view.focus_pane")),
                    Severity::Warning,
                ));
            }
        }
    }

    /// 切换界面语言(中英)并刷新(M2.1 提取,供 Ctrl+L 与命令面板共用)
    ///
    /// WHY 提取:Ctrl+L 快捷键与命令面板 `system.toggle_locale` 动作行为必须一致,
    /// 集中一处避免两条入口逻辑漂移(三入口统一派发)。
    fn toggle_locale_action(&mut self) {
        let locale = crate::i18n::toggle_locale();
        // 切换后全体面板文案需重绘,标记 dirty 触发下一帧重绘
        for panel_id in self.focus_manager.panels() {
            self.state.mark_dirty(*panel_id);
        }
        self.state.status_message = Some((
            format!("{}: {}", crate::t!("status.locale"), locale.short_label()),
            Severity::Info,
        ));
    }

    /// 循环切换布局模式(M2 提取,供 `l` 键与命令面板 `view.switch_layout` 共用)
    fn cycle_layout_action(&mut self) {
        let new_mode = self.state.layout_mode.next();
        self.state.layout_mode = new_mode;
        // M3d:切布局可能改变窗格数(如 IDE 3 → Focus 1),钳制活跃窗格回主区。
        self.clamp_active_pane();
        self.state.status_message = Some((
            format!("{}: {}", crate::t!("status.layout"), new_mode.as_str()),
            Severity::Info,
        ));
    }

    /// 打开帮助 overlay(M2 提取,供 `?` 键与命令面板 `system.open_help` 共用)
    ///
    /// 传入当前焦点面板的快捷键列表,帮助按上下文动态生成(§4.6 渐进披露)。
    fn open_help_action(&mut self) {
        // Task 2.5:registry 构造提前,供 shortcuts_with_registry 自动派生功能动作快捷键
        let registry = crate::actions::ActionRegistry::with_builtin_domains();
        let shortcuts = self
            .panels
            .iter()
            .find(|p| p.id() == self.focus_manager.focused())
            .map(|p| p.shortcuts_with_registry(&registry))
            .unwrap_or_default();
        // M2 增量3:帮助浮层追加 Registry 驱动的命令清单(与命令面板 Ctrl+P 同源),
        // 随 locale 动态生成。构造成本低(约 21 条),`?` 为低频操作,按需构建即可。
        let action_lines: Vec<(String, String)> =
            crate::actions::codegen::help_lines(&registry, None)
                .into_iter()
                .map(|line| (line.key, line.title))
                .collect();
        self.state
            .popup_stack
            .push(PopupKind::help_overlay_with_context_and_actions(
                &shortcuts,
                &action_lines,
            ));
    }

    /// 面板下钻:焦点面板进入 Focus 全屏(SinglePane)——§4.6 L3 三级信息层级下钻层
    ///
    /// WHY 复用 SinglePane:该布局即"当前面板全屏",语义等同下钻 Focus;`l` 键循环
    /// 布局可切回,无需额外返回栈。切布局后 `clamp_active_pane` 收敛活跃窗格到单窗格。
    fn drill_down_action(&mut self) {
        self.state.layout_mode = LayoutMode::SinglePane;
        self.clamp_active_pane();
        let focused = self.focus_manager.focused();
        self.state.status_message = Some((
            format!(
                "{}: {}",
                crate::t!("action.panel.drill_down"),
                focused.as_str()
            ),
            Severity::Info,
        ));
    }

    /// 切换监控采样暂停(monitor.pause_sampling;UI 本地冻结显示)
    ///
    /// WHY 标记 ResourceMonitor + Health dirty:两面板均展示 sys_metrics,
    /// 暂停/恢复切换需立即重绘(显/隐 PAUSED 标记 + 恢复时刷新最新数据)。
    fn toggle_monitor_pause(&mut self) {
        self.state.monitor_paused = !self.state.monitor_paused;
        self.state.mark_dirty(PanelId::ResourceMonitor);
        self.state.mark_dirty(PanelId::Health);
        let msg = if self.state.monitor_paused {
            "监控采样已暂停(显示冻结)"
        } else {
            "监控采样已恢复"
        };
        self.state.status_message = Some((msg.to_string(), Severity::Info));
    }

    /// 循环监控 sparkline 时间窗(monitor.time_window)
    fn cycle_monitor_window(&mut self) {
        self.state.monitor_window = self.state.monitor_window.next();
        self.state.mark_dirty(PanelId::ResourceMonitor);
        self.state.status_message = Some((
            format!("监控时间窗: 最近 {} 点", self.state.monitor_window.label()),
            Severity::Info,
        ));
    }

    /// 切换可视化维度(viz.switch_dimension)——按焦点面板分级
    ///
    /// WHY 分级:仅 ClvVector 有数据背书(8-block 热图值域 fixed↔autoscale);
    /// OsaSparse(仅平均稀疏度)/ MetricsDashboard(cell 默认未绑定)暂无可切维度,
    /// 给诚实反馈而非伪造(质量红线:不造无数据支撑的功能)。
    fn switch_viz_dimension(&mut self) {
        match self.focus_manager.focused() {
            PanelId::ClvVector => {
                self.state.clv_heatmap_autoscale = !self.state.clv_heatmap_autoscale;
                self.state.mark_dirty(PanelId::ClvVector);
                let mode = if self.state.clv_heatmap_autoscale {
                    "自适应(按实际值域)"
                } else {
                    "固定 [-1, 1]"
                };
                self.state.status_message = Some((format!("CLV 热图值域: {mode}"), Severity::Info));
            }
            other => {
                self.state.status_message = Some((
                    format!("{}: 当前面板暂无可切换维度", other.as_str()),
                    Severity::Info,
                ));
            }
        }
    }

    /// 应用磁盘持久化的视图(view.apply_saved)——重载 `state_file_path` 的视图字段
    ///
    /// WHY 重载而非命名视图:项目无命名视图概念,"saved view" 即退出时经
    /// `save_to_file` 存盘的单一视图快照;本动作让用户中途恢复上次保存的布局偏好。
    fn apply_saved_view_action(&mut self) {
        let path = self.config.state_file_path.clone();
        if !path.exists() {
            self.state.status_message = Some((
                "无已保存的视图(退出时自动保存布局)".to_string(),
                Severity::Info,
            ));
            return;
        }
        let saved = crate::types::TuiState::load_from_file(&path);
        self.apply_view_fields(&saved);
        // 布局可能变化(如 Triple→Single),钳制活跃窗格;全面板 dirty 确保重绘
        self.clamp_active_pane();
        for panel_id in self.focus_manager.panels() {
            self.state.mark_dirty(*panel_id);
        }
        self.state.status_message = Some(("已应用保存的视图".to_string(), Severity::Info));
    }

    /// 从另一 `TuiState` 拷贝视图字段(纯逻辑,便于测试)
    ///
    /// 仅拷贝视图偏好(布局/过滤/监控窗口/CLV 值域),不碰运行时/瞬态字段
    ///(running/events/popup/monitor_paused),避免恢复视图误改运行状态。
    pub(crate) fn apply_view_fields(&mut self, from: &crate::types::TuiState) {
        self.state.layout_mode = from.layout_mode;
        self.state.filter_keyword = from.filter_keyword.clone();
        self.state.filter_topic = from.filter_topic.clone();
        self.state.filter_level = from.filter_level.clone();
        self.state.monitor_window = from.monitor_window;
        self.state.clv_heatmap_autoscale = from.clv_heatmap_autoscale;
    }

    /// 跳转到焦点/单一 Quest 的事件流(quest.jump;TUI 本地,复用 JumpToEventStream)
    ///
    /// WHY TUI 本地:quest.jump 是"切到事件流并按 Quest 过滤"的视图导航,无需引擎;
    /// 与 QuestPanel Enter 同用 `JumpToEventStream`。命令面板/菜单无选中上下文时,
    /// 单 Quest 直接过滤,0/多 Quest 诚实切换(不臆测目标,提示经 Quest 面板精确跳转)。
    fn jump_to_quest_events_action(&mut self) {
        // §1.3b:焦点面板(Quest)有选中项时精确跳转其事件流
        if let Some(quest_id) = self.focused_selected_context_id() {
            self.apply_command(TuiCommand::JumpToEventStream { quest_id });
            return;
        }
        // 无选中上下文(焦点非列表面板):回退 quest_list 数量启发
        match self.state.quest_list.len() {
            1 => {
                let quest_id = self.state.quest_list[0].quest_id.clone();
                self.apply_command(TuiCommand::JumpToEventStream { quest_id });
            }
            0 => {
                self.switch_panel_to(PanelId::EventStream);
                self.state
                    .set_status("已切换事件流(当前无 Quest)", Severity::Info);
            }
            _ => {
                self.switch_panel_to(PanelId::EventStream);
                self.state.set_status(
                    "已切换事件流(多 Quest;Quest 面板选中后 Enter 可精确跳转)",
                    Severity::Info,
                );
            }
        }
    }

    /// 返回焦点面板当前选中项的上下文 id(§1.3b,供 quest.* 精确定位)
    ///
    /// 经 `Panel::selected_context_id` 泛化获取,不判具体面板类型;
    /// 焦点面板未覆写(展示型)或无选中项时返回 None。
    fn focused_selected_context_id(&self) -> Option<String> {
        let focused = self.focus_manager.focused();
        self.panel_index(focused)
            .and_then(|idx| self.panels[idx].selected_context_id(&self.state))
    }

    /// 若焦点面板有选中 Quest 且 payload 未含 quest_id,注入之(§1.3b 精确定位)
    ///
    /// WHY 单点富化:三入口派发 quest.pause/resume/cancel 均经 dispatch_action,
    /// 此处统一注入使"选中某 Quest 后经菜单/命令面板操作它"精确生效;
    /// cli `resolve_quest_id` 优先用 payload.quest_id,注入即命中,消除多 Quest 歧义。
    pub(crate) fn enrich_payload_with_focused_quest(&self, payload: String) -> String {
        let Some(quest_id) = self.focused_selected_context_id() else {
            return payload;
        };
        // 解析为 JSON 对象;已含 quest_id 则尊重显式指定不覆盖
        let mut obj = serde_json::from_str::<serde_json::Value>(&payload)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        if obj.contains_key("quest_id") {
            return payload;
        }
        obj.insert("quest_id".to_string(), serde_json::Value::String(quest_id));
        serde_json::to_string(&obj).unwrap_or(payload)
    }

    /// 统一派发 action_id 为具体行为(M2 增量2:三入口统一派发桥接)
    ///
    /// WHY 桥接而非仅发事件:命令面板 Enter 需产生"真实效果",但既有可用路径分两类——
    /// - **本地即时效果**(无参数):切换语言/布局、打开帮助,直接调用既有本地方法;
    /// - **需编排器消费的动作**(agent.chat/quest.*/task.* 等,多含参数):当前无本地
    ///   通路,回退发布 `TuiActionRequested`,交 chimera-cli QueryLoop 编排(M3 落地)。
    ///
    /// 面板上下文动作仍走各自的 `TuiCommand` 变体(带 quest_id 等参数),不经此桥接;
    /// 本方法只服务"无参数、来源为命令面板/斜杠"的统一入口。
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
        match action_id {
            // —— 本地即时效果(无参数,已有实现路径)——
            "system.toggle_locale" => self.toggle_locale_action(),
            "view.switch_layout" => self.cycle_layout_action(),
            "view.toggle_companion" => self.toggle_companion_action(),
            "view.cycle_companion" => self.cycle_companion_action(),
            "view.focus_pane" => self.focus_pane_action(),
            "system.open_help" => self.open_help_action(),
            // export.run:本地弹出导出格式选择框(原 `E` 键行为)——必须本地 arm,
            // 否则经 router 路由的 `E`/Ctrl+E 会回退发事件而非本地导出(避免行为回归)。
            "export.run" => self.handle_export_command(),
            // —— Phase 2 UI 本地域(UI 态,不绕道 cli,§2.2 依赖铁律)——
            // 面板下钻:焦点面板进入 Focus 全屏(SinglePane),`l` 键切回(§4.6 L3 下钻层级)。
            "panel.drill_down" => self.drill_down_action(),
            // —— UI 本地态视图/配置动作(M3/M4 已落地,不绕 cli,§2.2 依赖铁律)——
            // view.apply_saved 重载持久化视图;monitor/viz 视图控制;config.edit 配置速调菜单。
            "view.apply_saved" => self.apply_saved_view_action(),
            // M3 三大核心功能:monitor/viz 视图控制(UI 本地态,不绕 cli)
            "monitor.pause_sampling" => self.toggle_monitor_pause(),
            "monitor.time_window" => self.cycle_monitor_window(),
            "viz.switch_dimension" => self.switch_viz_dimension(),
            "config.edit" => self.open_config_menu(),
            // quest.jump:TUI 本地导航(切事件流 + 按 Quest 过滤),不经 cli(§4.6 跨面板联动)
            "quest.jump" => self.jump_to_quest_events_action(),
            // —— 编排域(quest.*/task.*/agent.chat):发布 TuiActionRequested,由 chimera-cli
            // Action 编排器消费并回发 Completed/Failed(P0 已接线,反馈经 ActionFeedbackSync 上屏)——
            _ => {
                self.publish_control_event(NexusEvent::TuiActionRequested {
                    metadata: EventMetadata::new("chimera-tui"),
                    action_id: action_id.to_string(),
                    payload,
                    source,
                });
            }
        }
    }

    /// 处理弹窗激活时的键盘事件
    fn handle_popup_key(&mut self, key: KeyEvent) {
        // ActionMenu 有独立选择/执行语义(↑↓ 移选、Enter 派发选中动作),
        // 与滚动型弹窗(Detail/Help)分流,避免 Up/Down 被 scroll 语义占用。
        if matches!(
            self.state.popup_stack.current(),
            Some(PopupKind::ActionMenu { .. })
        ) {
            self.handle_action_menu_key(key);
            return;
        }
        // ConfigMenu 就地循环编辑语义(↑↓ 移选、Enter 循环当前项、菜单常驻),同样分流
        if matches!(
            self.state.popup_stack.current(),
            Some(PopupKind::ConfigMenu { .. })
        ) {
            self.handle_config_menu_key(key);
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.state.popup_stack.pop();
            }
            KeyCode::Enter => {
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
            KeyCode::Up => {
                self.state.popup_stack.scroll_current(-1);
            }
            KeyCode::Down => {
                self.state.popup_stack.scroll_current(1);
            }
            KeyCode::Left | KeyCode::Right => {
                self.state.popup_stack.toggle_confirm();
            }
            _ => {}
        }
    }

    /// 动作菜单弹窗的键盘处理(§4.5 入口三:面板动作)
    ///
    /// ↑↓/kj 移动选中项,Enter 派发选中动作(经 `DispatchAction`,source=Panel,
    /// 复用三入口统一派发与反馈管线),Esc/q 关闭。
    fn handle_action_menu_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.state.popup_stack.pop();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.state.popup_stack.move_action_menu_selection(false);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.state.popup_stack.move_action_menu_selection(true);
            }
            KeyCode::Enter => {
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
            _ => {}
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
                format!("{:.0}%", self.main_panel_ratio() * 100.0),
            ),
            (
                crate::t!("status.tick").to_string(),
                format!("{}ms (重启生效)", self.config.tick_interval_ms),
            ),
        ]
    }

    /// 配置菜单键盘处理(§4.5 收尾)——↑↓/kj 移选,Enter 就地循环选中项,Esc/q 关
    fn handle_config_menu_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.state.popup_stack.pop();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.state.popup_stack.move_config_menu_selection(false);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.state.popup_stack.move_config_menu_selection(true);
            }
            KeyCode::Enter => {
                if let Some(idx) = self.state.popup_stack.config_menu_selected() {
                    self.cycle_config_item(idx);
                    // 循环后刷新条目显示当前值,菜单常驻以便连续编辑
                    let entries = self.config_menu_entries();
                    self.state.popup_stack.set_config_menu_entries(entries);
                }
            }
            _ => {}
        }
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

    /// 根据确认弹窗的命令字符串执行动作
    fn apply_confirm_command(&mut self, cmd: &str) {
        if cmd == "quit" {
            self.quit();
        } else if let Some(quest_id) = cmd.strip_prefix("pause:") {
            self.publish_pause(quest_id);
        } else if let Some(quest_id) = cmd.strip_prefix("resume:") {
            self.publish_resume(quest_id);
        } else if let Some(quest_id) = cmd.strip_prefix("cancel:") {
            self.publish_cancel_request(quest_id);
        } else if let Some(ids_str) = cmd.strip_prefix("batch_pause:") {
            // 批量暂停:遍历逗号分隔的 quest_id 列表,逐个发布暂停请求
            for quest_id in ids_str.split(',') {
                self.publish_pause(quest_id);
            }
        } else if let Some(ids_str) = cmd.strip_prefix("batch_resume:") {
            // 批量恢复:遍历逗号分隔的 quest_id 列表,逐个发布恢复请求
            for quest_id in ids_str.split(',') {
                self.publish_resume(quest_id);
            }
        } else if let Some(ids_str) = cmd.strip_prefix("batch_terminate:") {
            // 批量终止:遍历逗号分隔的 quest_id 列表,逐个发布终止请求
            for quest_id in ids_str.split(',') {
                self.publish_terminate(quest_id);
            }
        } else if let Some(ids_str) = cmd.strip_prefix("batch_cancel:") {
            // 批量取消:遍历逗号分隔的 quest_id 列表,逐个发布取消请求
            // WHY 逐条发布而非单事件:event-bus 的 QuestCancelRequested 事件
            // 语义为单个 Quest 的取消,保持契约一致。
            for quest_id in ids_str.split(',') {
                self.publish_cancel_request(quest_id);
            }
        } else if let Some(rest) = cmd.strip_prefix("vote:") {
            let mut parts = rest.splitn(2, ':');
            let vote_str = parts.next().unwrap_or("");
            let proposal_id = parts.next().unwrap_or("");
            if let Some(vote) = parse_vote_value(vote_str) {
                self.publish_vote(proposal_id, vote);
            } else {
                self.state.set_status(
                    format!("invalid vote in confirm command: {cmd}"),
                    Severity::Error,
                );
            }
        } else if let Some(format_str) = cmd.strip_prefix("export:") {
            let format = if format_str.contains("csv") {
                ExportFormat::Csv
            } else {
                ExportFormat::Json
            };
            self.perform_export(format);
        }
    }

    /// 处理导出命令 — 弹出格式选择弹窗
    fn handle_export_command(&mut self) {
        self.state.popup_stack.push(PopupKind::Confirm {
            prompt: "Export as CSV?".into(),
            on_confirm: "export:csv".into(),
            confirmed: true,
        });
    }

    /// 执行导出操作
    fn perform_export(&mut self, format: ExportFormat) {
        let data_source = &self.data_source;
        match data_source.snapshot() {
            Ok(snapshot) => {
                let now = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
                let home = std::env::var("USERPROFILE")
                    .or_else(|_| std::env::var("HOME"))
                    .unwrap_or_else(|_| ".".to_string());
                let export_dir = std::path::PathBuf::from(home)
                    .join(".chimera")
                    .join("exports");
                let filename = format!("quests_{}.{}", now, format.extension());
                let path = export_dir.join(&filename);
                match snapshot.export_quests_to(format, &path) {
                    Ok(()) => {
                        self.state.status_message =
                            Some((format!("Exported: {}", path.display()), Severity::Info));
                    }
                    Err(e) => {
                        self.state.status_message =
                            Some((format!("Export failed: {e}"), Severity::Error));
                    }
                }
            }
            Err(e) => {
                self.state.status_message = Some((format!("Cannot export: {e}"), Severity::Error));
            }
        }
    }

    /// 发布 Quest 暂停请求
    fn publish_pause(&mut self, quest_id: &str) {
        self.publish_control_event(NexusEvent::QuestPauseRequested {
            metadata: EventMetadata::new("chimera-tui"),
            quest_id: quest_id.to_string(),
            requested_by: "operator".to_string(),
        });
    }

    /// 发布 Quest 恢复请求
    fn publish_resume(&mut self, quest_id: &str) {
        self.publish_control_event(NexusEvent::QuestResumeRequested {
            metadata: EventMetadata::new("chimera-tui"),
            quest_id: quest_id.to_string(),
            requested_by: "operator".to_string(),
        });
    }

    /// 发布 Quest 取消请求(M4 扩展)
    ///
    /// WHY 与 pause/resume 同构:复用 EventMetadata + requested_by 模式,
    /// 由 quest-engine 消费后发布 QuestCancelled 状态变更事件。
    fn publish_cancel_request(&mut self, quest_id: &str) {
        self.publish_control_event(NexusEvent::QuestCancelRequested {
            metadata: EventMetadata::new("chimera-tui"),
            quest_id: quest_id.to_string(),
            requested_by: "operator".to_string(),
        });
    }

    /// 发布 Quest 终止请求(Task 7.2 批量操作)
    ///
    /// WHY 与 pause/resume/cancel 同构:复用 EventMetadata + requested_by 模式,
    /// 由 quest-engine 消费后终止 Quest 执行。
    fn publish_terminate(&mut self, quest_id: &str) {
        self.publish_control_event(NexusEvent::QuestCancelRequested {
            metadata: EventMetadata::new("chimera-tui"),
            quest_id: quest_id.to_string(),
            requested_by: "operator".to_string(),
        });
    }

    /// 发布 Quest 优先级变更请求(M4 扩展)
    ///
    /// WHY new_priority 由面板边界检查后传入:app 层不再重复校验,
    /// 保持职责单一(面板=输入校验,app=事件发布)。
    fn publish_priority_change(&mut self, quest_id: &str, new_priority: u8) {
        self.publish_control_event(NexusEvent::QuestPriorityChanged {
            metadata: EventMetadata::new("chimera-tui"),
            quest_id: quest_id.to_string(),
            new_priority,
            requested_by: "operator".to_string(),
        });
    }

    /// 发布投票请求
    fn publish_vote(&mut self, proposal_id: &str, vote: VoteValue) {
        self.publish_control_event(NexusEvent::VoteCastRequested {
            metadata: EventMetadata::new("chimera-tui"),
            proposal_id: proposal_id.to_string(),
            voter: "operator".to_string(),
            vote,
        });
    }

    /// 发布状态刷新请求
    fn publish_refresh(&mut self) {
        self.publish_control_event(NexusEvent::RefreshStateRequested {
            metadata: EventMetadata::new("chimera-tui"),
            requested_by: "operator".to_string(),
        });
    }

    /// 通用控制事件发布,处理 EventBus 不可用或发布失败
    ///
    /// WHY:所有 M4 控制请求走同一入口,统一设置状态栏反馈,
    /// 避免每个命令重复 error/success 处理逻辑。
    fn publish_control_event(&mut self, event: NexusEvent) {
        let type_name = event.type_name();
        match &self.event_bus {
            Some(bus) => match bus.publish_blocking(event) {
                Ok(()) => {
                    let msg = format!("{type_name} request published");
                    self.state.set_status(msg, Severity::Info);
                }
                Err(e) => {
                    self.state.set_status(
                        format!("failed to publish {type_name}: {e}"),
                        Severity::Error,
                    );
                }
            },
            None => {
                self.state
                    .set_status("event bus not available", Severity::Error);
            }
        }
    }

    /// 执行高层命令
    pub(super) fn apply_command(&mut self, cmd: TuiCommand) {
        match cmd {
            TuiCommand::Quit => self.quit(),
            TuiCommand::SwitchPanel(id) => self.switch_panel_to(id),
            TuiCommand::ShowHelp => self.switch_panel_to(PanelId::Help),
            TuiCommand::OpenPopup(kind) => self.state.popup_stack.push(kind),
            // M4:破坏性控制命令先弹出确认框,由操作员二次确认后再发布事件
            TuiCommand::RequestQuestPause(quest_id) => {
                self.state.popup_stack.push(PopupKind::control_confirm(
                    "Pause quest",
                    &quest_id,
                    format!("pause:{quest_id}"),
                ));
            }
            TuiCommand::RequestQuestResume(quest_id) => {
                self.state.popup_stack.push(PopupKind::control_confirm(
                    "Resume quest",
                    &quest_id,
                    format!("resume:{quest_id}"),
                ));
            }
            // M4 扩展:cancel 是破坏性操作,与 pause/resume 一致走确认流程
            TuiCommand::RequestQuestCancel(quest_id) => {
                self.state.popup_stack.push(PopupKind::control_confirm(
                    "Cancel quest",
                    &quest_id,
                    format!("cancel:{quest_id}"),
                ));
            }
            // M4 扩展:priority 调整是非破坏性操作,直接发布事件
            // WHY 不走确认流程:优先级 +/- 可逆,二次确认会增加操作摩擦
            TuiCommand::RequestQuestPriorityChange {
                quest_id,
                new_priority,
            } => {
                self.publish_priority_change(&quest_id, new_priority);
            }
            TuiCommand::RequestVote { proposal_id, vote } => {
                let vote_str = vote.as_str();
                self.state.popup_stack.push(PopupKind::control_confirm(
                    &format!("Vote {vote_str} on proposal"),
                    &proposal_id,
                    format!("vote:{vote_str}:{proposal_id}"),
                ));
            }
            // M4:非破坏性刷新直接发布事件
            TuiCommand::RequestRefresh => self.publish_refresh(),
            // P4.3:运行时调整 tick 间隔(更新配置,下次启动 DataPipeline 时生效)
            TuiCommand::SetTickInterval(ms) => {
                self.config.tick_interval_ms = ms;
                self.state.status_message = Some((
                    format!("Tick interval set to {}ms (restart to apply)", ms),
                    crate::popup::Severity::Info,
                ));
            }
            // P5 跨面板联动:Quest→EventStream 跳转,原子完成 filter 设置 + 面板切换
            //
            // WHY 先设置 filter 再切换:确保 EventStream 面板首次渲染时
            // 即应用筛选,避免一帧全量事件闪烁后再被过滤的视觉抖动。
            // filter_keyword 复用现有 EventStream 的关键字过滤逻辑
            // (event_matches_keyword),quest_id 作为关键字可匹配事件 JSON
            // 载荷中包含该 quest_id 的所有事件(如 QuestCreated/QuestProgressUpdated 等)。
            TuiCommand::JumpToEventStream { quest_id } => {
                self.state.filter_keyword = Some(quest_id.clone());
                self.switch_panel_to(PanelId::EventStream);
                self.state.set_status(
                    format!("Jumped to EventStream, filter: {quest_id}"),
                    Severity::Info,
                );
            }
            // M3-2 TaskManagerPanel:Quest 控制命令的桥接层
            //
            // WHY 在此桥接:TaskManagerPanel 是 L10 用户面,使用 0-10 优先级范围
            // (spec 用户面友好);底层 event-bus 沿用 0-255 内部范围(Quest.priority)。
            // 桥接公式:`priority_255 = priority_10 * 25` (0→0, 10→250),
            // 保留两端极值,中段略稀疏,符合"用户面稀疏、内部精细"的直觉。
            //
            // WHY Pause/Resume/Terminate 走确认弹窗:沿用既有
            // `RequestQuestPause` 的二次确认 UX(Week 7 M4 教训);
            // 破坏性动作(Terminate)与 Pause/Resume 一致走 Confirm,避免误触。
            // SetPriority 直接发布(非破坏性,可逆),与既有
            // `RequestQuestPriorityChange` 行为对齐。
            TuiCommand::QuestControl { id, action } => {
                use crate::types::QuestAction;
                match action {
                    QuestAction::Pause => {
                        self.state.popup_stack.push(PopupKind::control_confirm(
                            "Pause quest",
                            &id,
                            format!("pause:{id}"),
                        ));
                    }
                    QuestAction::Resume => {
                        self.state.popup_stack.push(PopupKind::control_confirm(
                            "Resume quest",
                            &id,
                            format!("resume:{id}"),
                        ));
                    }
                    QuestAction::Terminate => {
                        self.state.popup_stack.push(PopupKind::control_confirm(
                            "Terminate quest",
                            &id,
                            format!("terminate:{id}"),
                        ));
                    }
                    QuestAction::SetPriority(level) => {
                        // 用户面 [0, 10] → 内部 [0, 250] 线性映射
                        // 边界已在 TaskManagerPanel 钳制,此处仍做 defensive saturate
                        // 防止未来调用方传入越界值
                        let level = level.min(10);
                        let new_priority = (level as u16 * 25).min(u8::MAX as u16) as u8;
                        self.publish_priority_change(&id, new_priority);
                    }
                }
            }
            TuiCommand::Export => {
                self.handle_export_command();
            }
            TuiCommand::DispatchAction {
                action_id,
                payload,
                source,
            } => {
                // v3.1(ADR-029)M2 增量2:三入口统一派发桥接 —— 本地即时动作直接执行,
                // 其余回退发布 TuiActionRequested,经 EventBus 交 chimera-cli 编排(M3)。
                self.dispatch_action(&action_id, payload, source);
            }
        }
    }

    /// 渲染 UI 到 Frame
    ///
    /// WHY 接收 &mut Frame:与 ratatui 的 draw 闭包签名对齐,
    /// 支持 TestBackend 内存渲染测试(无需真实终端)。
    ///
    /// # 布局
    /// - 顶部:面板标签栏(1 行,含边框)
    /// - 中部:主面板(占 `main_panel_ratio`)
    /// - 底部:命令面板(激活时)或状态栏(普通模式)
    /// - 最上层:弹窗叠加
    ///
    /// # P4.1 增量渲染说明
    /// ratatui 的 Frame 每帧会用空白缓冲区覆盖前帧内容,因此面板渲染
    /// 本身必须每帧执行(否则对应区域会被清空)。`dirty_panels` 标记
    /// 并不跳过渲染,而是为面板内部提供"数据是否变化"的可观测信号:
    /// 面板实现可以选择在数据未变时复用上次构建的 `Text` / `Span`。
    /// 渲染结束后调用 `clear_dirty` 重置集合,保证下一帧的脏标记从零开始。
    /// 渲染层级:底层面板 → 中间层浮动面板 → 最上层弹窗叠加。
    pub fn run(&mut self) -> Result<(), TuiError> {
        // 步骤 1:启用 raw mode 与 alternate screen
        enable_raw_mode().map_err(|e| TuiError::TerminalInit(e.to_string()))?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)
            .map_err(|e| TuiError::TerminalInit(e.to_string()))?;

        // M3:按配置启用鼠标捕获
        if self.config.enable_mouse {
            execute!(stdout, event::EnableMouseCapture)
                .map_err(|e| TuiError::TerminalInit(e.to_string()))?;
        }

        // 步骤 2:创建终端
        let backend = CrosstermBackend::new(stdout);
        let mut terminal =
            Terminal::new(backend).map_err(|e| TuiError::TerminalInit(e.to_string()))?;

        // 步骤 3:事件循环
        // WHY 用 result 变量:确保终端恢复在 return 前执行,即使事件循环出错
        let result = self.event_loop(&mut terminal);

        // 步骤 4:退出时保存 TuiConfig 到 ~/.chimera/tui.yaml(最佳努力)
        // WHY 在终端恢复前保存:此时 self.config 仍持有运行时修改
        // (主题切换 `t`、tick 间隔调整),保存可持久化用户偏好。
        // WHY 同步 main_panel_ratio:运行时比例调整(Ctrl+Up/Down)更新的是
        // pane_manager.main_panel_ratio 而非 self.config.main_panel_ratio,
        // 保存前需同步回 config 以持久化用户调整。
        // WHY 保存失败不阻塞退出:配置持久化是最佳努力,失败仅记录警告。
        // Task 1.15.4:main_panel_ratio 经 getter 方法读取(委托 pane_manager)
        self.config.main_panel_ratio = self.main_panel_ratio();
        let config_path = TuiConfig::default_path();
        if let Err(e) = self.config.save_to_file(&config_path) {
            tracing::warn!(
                path = %config_path.display(),
                error = %e,
                "Failed to save TuiConfig on exit (non-blocking)"
            );
        }

        // 步骤 4.5:退出时保存 TuiState (最佳努力)
        if self.config.persist_state {
            if let Err(e) = self.state.save_to_file(&self.config.state_file_path) {
                tracing::warn!(
                    path = %self.config.state_file_path.display(),
                    error = %e,
                    "Failed to save TuiState on exit (non-blocking)"
                );
            }
        }

        // 步骤 5:恢复终端(无论事件循环成功与否)
        // WHY 恢复在 result 返回前:确保终端状态不残留,即使出错也要恢复
        let stdout = terminal.backend_mut();
        if self.config.enable_mouse {
            let _ = execute!(stdout, event::DisableMouseCapture);
        }
        disable_raw_mode().map_err(|e| TuiError::TerminalRestore(e.to_string()))?;
        execute!(stdout, LeaveAlternateScreen)
            .map_err(|e| TuiError::TerminalRestore(e.to_string()))?;

        result
    }

    /// 事件循环内部实现
    ///
    /// WHY 独立方法:将循环逻辑与终端初始化/恢复分离,职责单一
    fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> Result<(), TuiError> {
        while self.state.running {
            // 在渲染前从数据源刷新状态，确保面板显示最新快照。
            // 数据源实现内部处理去重与缓存，此调用为 O(1) 非阻塞。
            self.update();

            // 渲染当前帧
            terminal
                .draw(|f| self.render(f))
                .map_err(|e| TuiError::Render(e.to_string()))?;
            self.state.tick_frame();

            // 轮询事件(Task 1.16:poll 间隔随 tick_mode 联动)
            // - Normal:100ms(高响应,默认)
            // - Eco:1000ms(低 CPU 占用,适合后台监控场景)
            if !event::poll(self.poll_duration()).map_err(|e| TuiError::EventRead(e.to_string()))? {
                continue;
            }

            // 读取并处理事件
            let event = event::read().map_err(|e| TuiError::EventRead(e.to_string()))?;
            match event {
                Event::Key(key) => self.handle_key_event(key),
                Event::Mouse(mouse) => self.handle_mouse_event(mouse),
                _ => {}
            }
        }
        Ok(())
    }

    /// 根据当前 tick_mode 计算事件轮询间隔(Task 1.16)
    ///
    /// - `Normal`:100ms(高响应,默认)—— 适合交互式使用,保证按键延迟 < 100ms
    /// - `Eco`:1000ms(低 CPU 占用)—— 适合后台监控/长时间挂机,降低空转开销
    ///
    /// WHY 联动 tick_mode:Eco 模式下数据刷新频率本就降至 1Hz,
    /// 事件轮询保持 100ms 会造成 90% 的空轮询浪费 CPU;联动后 Eco 模式
    /// 真正实现低功耗,Normal 模式保持高响应。
    ///
    /// 抽取为独立方法便于单元测试断言两种模式的 poll 间隔。
    pub(crate) fn poll_duration(&self) -> Duration {
        match self.state.tick_mode {
            crate::types::TickMode::Normal => Duration::from_millis(100),
            crate::types::TickMode::Eco => Duration::from_millis(1000),
        }
    }
}

/// 将 vote 字符串解析为 VoteValue
///
/// WHY:确认弹窗的 `on_confirm` 只能传递字符串,解码时需要与
/// CommandPalette 编码时使用的 `yes|no|abstain` 保持一致。
/// 委托给 `VoteValue::from_str` 以保证唯一真实来源。
fn parse_vote_value(s: &str) -> Option<VoteValue> {
    s.parse().ok()
}

/// 主面板占比预设循环:0.5 → 0.6 → 0.7 → 0.8 → 0.5(非预设值归入最近档后前进)
///
/// WHY 预设循环而非步进:config.edit 菜单单键循环需确定性档位;非预设值(经 Ctrl+方向
/// 步进产生)归入最近档再前进,保证循环闭合。返回值 ∈ [0.5, 0.8] 满足 validate 的 (0,1)。
pub(crate) fn ratio_preset_next(current: f32) -> f32 {
    const PRESETS: [f32; 4] = [0.5, 0.6, 0.7, 0.8];
    let nearest = PRESETS
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (**a - current).abs().total_cmp(&(**b - current).abs()))
        .map(|(i, _)| i)
        .unwrap_or(0);
    PRESETS[(nearest + 1) % PRESETS.len()]
}

/// Tick 间隔(ms)预设循环:100 → 250 → 500 → 1000 → 100(非预设值归入最近档后前进)
///
/// 返回值 ∈ [100, 1000] 满足 TuiConfig::validate 的 tick_interval_ms 范围。
pub(crate) fn tick_preset_next(current: u16) -> u16 {
    const PRESETS: [u16; 4] = [100, 250, 500, 1000];
    let nearest = PRESETS
        .iter()
        .enumerate()
        .min_by_key(|(_, &p)| p.abs_diff(current))
        .map(|(i, _)| i)
        .unwrap_or(0);
    PRESETS[(nearest + 1) % PRESETS.len()]
}
