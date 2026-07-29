//! 状态管理 — 数据更新、面板切换与辅助访问器
//!
//! 包含 [`TuiApp::update`]、面板切换、伴随面板/窗格访问器、
//! FPS 计算等状态管理方法。
//!
//! 对应架构层:L10 Interface

use std::time::Duration;

use super::{TuiApp, FPS_DISPLAY_MAX, FPS_WINDOW_SIZE, RATIO_MAX, RATIO_MIN, RATIO_STEP};
use crate::data::DataSnapshot;
use crate::popup::Severity;
use crate::types::PanelId;

impl TuiApp {
    /// 从数据源拉取最新快照并更新内部状态,含 P4.1 脏面板标记检测。
    pub fn update(&mut self) {
        match self.data_source.snapshot() {
            Ok(snapshot) => {
                // P4.1:在覆盖状态前检测哪些面板数据发生变化,先打 dirty 标记
                self.mark_dirty_panels_from_snapshot(&snapshot);

                self.state.quest_list = snapshot.quest_list;
                self.state.paused_quest_count = snapshot.paused_quest_count;
                self.state.budget = snapshot.budget_metrics;
                self.state.memory_metrics = snapshot.memory_metrics;
                self.state.security_state = snapshot.security_state;
                self.state.health_metrics = snapshot.health_metrics;
                self.state.budget_history = snapshot.budget_history;
                self.state.memory_history = snapshot.memory_history;
                self.state.event_rate_history = snapshot.event_rate_history;
                self.state.latest_events = snapshot.latest_events;
                // P2 新增字段同步:DataSnapshot → TuiState
                self.state.decay_metrics = snapshot.decay_metrics;
                self.state.router_metrics = snapshot.router_metrics;
                self.state.mcp_nodes = snapshot.mcp_nodes;
                self.state.chtc_state = snapshot.chtc_state;
                self.state.decay_history = snapshot.decay_history;
                // P8 ResourceMonitor 面板字段同步:DataSnapshot → TuiState
                // M3 monitor.pause_sampling:暂停时跳过覆盖,保留冻结快照供检视(UI 本地冻结)
                if !self.state.monitor_paused {
                    self.state.sys_metrics = snapshot.sys_metrics.clone();
                    self.state.sys_metrics_history = snapshot.sys_metrics_history.clone();
                }
                // Task 6:同步 tick 模式,供状态栏展示
                self.state.tick_mode = snapshot.tick_mode;
                // M3b:同步对话历史与状态到 TuiState(供 Chat 面板渲染)
                self.state.chat_messages = snapshot.chat_messages;
                self.state.chat_status = snapshot.chat_status;
                // P0 交互链:新 Action 终态反馈(seq 递增)时上屏 status_message,
                // 比对 seq 只上屏一次;错误用 Error 级,成功用 Info 级。
                if snapshot.action_feedback_seq > self.state.last_action_feedback_seq {
                    if let Some((msg, is_error)) = &snapshot.action_feedback {
                        let severity = if *is_error {
                            Severity::Error
                        } else {
                            Severity::Info
                        };
                        self.state.status_message = Some((msg.clone(), severity));
                    }
                    self.state.last_action_feedback_seq = snapshot.action_feedback_seq;
                }
                // P1-W2.2:同步 Critical 旁路通道丢弃计数(EventStream 面板告警显示)
                self.state.critical_event_dropped_count = snapshot.critical_event_dropped_count;
            }
            Err(e) => {
                // M1 清理项 #4:数据源失败时向用户展示状态栏警告,而非静默忽略。
                self.state.status_message =
                    Some((format!("data source unavailable: {e}"), Severity::Warning));
            }
        }
    }

    /// 比较当前 `TuiState` 与新 `DataSnapshot` 中各面板绑定的字段,
    /// 对发生变化的字段调用 `mark_dirty`。
    ///
    /// WHY 独立方法:集中维护"字段 → PanelId"映射,避免 `update` 方法
    /// 臃肿;同时便于测试针对单个字段的变化进行断言。
    ///
    /// # 字段 → 面板映射
    /// - `quest_list` → Quest + Health(Active Quests 从 quest_list.len() 派生)
    /// - `paused_quest_count` → Health(Paused Quests 指标)
    /// - `budget_metrics` / `budget_history` → Budget
    /// - `memory_metrics` / `memory_history` → Memory
    /// - `security_state` → Security
    /// - `health_metrics` / `event_rate_history` → Health
    /// - `latest_events` → Parliament + Log + EventStream(三者共享事件流)
    /// - `decay_metrics` / `decay_history` → Decay
    /// - `router_metrics` → Router
    /// - `mcp_nodes` → McpNodes
    /// - `chtc_state` → Chtc
    fn mark_dirty_panels_from_snapshot(&mut self, snapshot: &DataSnapshot) {
        // WHY 使用 `!=` 而非哈希比较:所有 metrics 类型都已 `PartialEq`,
        // 结构化比较更易读,且无需额外引入哈希依赖。
        if self.state.quest_list != snapshot.quest_list {
            self.state.mark_dirty(PanelId::Quest);
            // quest_list 变化也影响 Health 面板的 Active Quests 指标
            self.state.mark_dirty(PanelId::Health);
        }
        if self.state.budget != snapshot.budget_metrics
            || self.state.budget_history != snapshot.budget_history
        {
            self.state.mark_dirty(PanelId::Budget);
        }
        if self.state.memory_metrics != snapshot.memory_metrics
            || self.state.memory_history != snapshot.memory_history
        {
            self.state.mark_dirty(PanelId::Memory);
        }
        if self.state.security_state != snapshot.security_state {
            self.state.mark_dirty(PanelId::Security);
        }
        if self.state.health_metrics != snapshot.health_metrics
            || self.state.event_rate_history != snapshot.event_rate_history
            || self.state.paused_quest_count != snapshot.paused_quest_count
        {
            self.state.mark_dirty(PanelId::Health);
        }
        // WHY latest_events 同时驱动 Parliament / Log / EventStream 三面板,
        // 任一变化都需标记这三个面板,避免事件流面板错过新事件。
        if self.state.latest_events != snapshot.latest_events {
            self.state.mark_dirty(PanelId::Parliament);
            self.state.mark_dirty(PanelId::Log);
            self.state.mark_dirty(PanelId::EventStream);
        }
        if self.state.decay_metrics != snapshot.decay_metrics
            || self.state.decay_history != snapshot.decay_history
        {
            self.state.mark_dirty(PanelId::Decay);
        }
        if self.state.router_metrics != snapshot.router_metrics {
            self.state.mark_dirty(PanelId::Router);
        }
        if self.state.mcp_nodes != snapshot.mcp_nodes {
            self.state.mark_dirty(PanelId::McpNodes);
        }
        if self.state.chtc_state != snapshot.chtc_state {
            self.state.mark_dirty(PanelId::Chtc);
        }
        // P8:系统资源指标变化时标记 ResourceMonitor 面板 dirty,
        // 同时标记 Health 面板(Health 面板也展示系统资源摘要)
        // M3 monitor.pause_sampling:暂停时冻结显示,不因快照变化重标 dirty(避免每 tick 重绘冻结数据)
        if !self.state.monitor_paused
            && (self.state.sys_metrics != snapshot.sys_metrics
                || self.state.sys_metrics_history != snapshot.sys_metrics_history)
        {
            self.state.mark_dirty(PanelId::ResourceMonitor);
            self.state.mark_dirty(PanelId::Health);
        }
        // M3b:对话历史或状态变化时标记 Chat 面板重绘
        if self.state.chat_messages != snapshot.chat_messages
            || self.state.chat_status != snapshot.chat_status
        {
            self.state.mark_dirty(PanelId::Chat);
        }
        // P1-W2.2:Critical 丢弃计数变化时标记 EventStream 面板重绘(顶部告警行)
        if self.state.critical_event_dropped_count != snapshot.critical_event_dropped_count {
            self.state.mark_dirty(PanelId::EventStream);
        }
    }

    /// 切换到下一个面板
    pub fn switch_panel_next(&mut self) {
        let before = self.focus_manager.focused();
        self.focus_manager.next();
        self.record_prev_panel(before);
    }

    /// 切换到上一个面板
    pub fn switch_panel_prev(&mut self) {
        let before = self.focus_manager.focused();
        self.focus_manager.prev();
        self.record_prev_panel(before);
    }

    /// 切换到指定面板
    pub fn switch_panel_to(&mut self, panel: PanelId) {
        let before = self.focus_manager.focused();
        self.focus_manager.jump_to(panel);
        self.record_prev_panel(before);
    }

    /// 记录切换前的焦点面板为伴随面板目标(仅当焦点确实变化时)
    ///
    /// WHY 仅在变化时记录:重复切到同一面板不应把伴随目标覆盖为自身,
    /// 保证 `companion_target` 始终指向"上一个不同面板"。
    fn record_prev_panel(&mut self, before: PanelId) {
        if self.focus_manager.focused() != before {
            self.prev_panel = Some(before);
            // Stage 2/M3d:主区焦点变化时复位活跃窗格回主区,避免焦点滞留旧次窗格。
            self.active_pane = 0;
        }
    }

    /// 退出应用
    pub fn quit(&mut self) {
        self.state.quit();
    }

    /// 查找面板索引
    pub(super) fn panel_index(&self, id: PanelId) -> Option<usize> {
        self.panels.iter().position(|p| p.id() == id)
    }

    /// 调整主面板比例
    ///
    /// `increase` 为 true 时增大比例,否则减小。限制在 [RATIO_MIN, RATIO_MAX]。
    pub(super) fn adjust_main_panel_ratio(&mut self, increase: bool) {
        let delta = if increase { RATIO_STEP } else { -RATIO_STEP };
        self.main_panel_ratio = (self.main_panel_ratio + delta).clamp(RATIO_MIN, RATIO_MAX);
    }

    /// 更新 FPS 移动平均(P4.4)
    ///
    /// WHY 使用移动平均:单帧耗时受 OS 调度、事件循环等待、IO 等影响波动较大,
    /// 直接显示瞬时 FPS 会让状态栏数字频繁跳动、难以阅读。固定窗口移动平均
    /// 平滑短时抖动,同时对真实帧率下降仍保持灵敏响应。
    ///
    /// WHY `VecDeque<f64>` + O(1) push/pop:窗口大小固定为 `FPS_WINDOW_SIZE`,
    /// 不需要环形缓冲区等更复杂结构,`VecDeque` 已能满足需求且语义直观。
    pub(super) fn update_fps(&mut self, delta: Duration) {
        let frame_time_ms = delta.as_secs_f64() * 1000.0;
        self.frame_times.push_back(frame_time_ms);
        while self.frame_times.len() > FPS_WINDOW_SIZE {
            self.frame_times.pop_front();
        }
        if self.frame_times.is_empty() {
            self.state.fps = 0;
            return;
        }
        let avg_ms = self.frame_times.iter().sum::<f64>() / self.frame_times.len() as f64;
        // avg_ms 为 0 仅在两帧几乎同时渲染(如调试步进)时发生,避免除零,
        // 将 FPS 记为显示上限。
        self.state.fps = if avg_ms > 0.0 {
            ((1000.0 / avg_ms).round() as u16).min(FPS_DISPLAY_MAX)
        } else {
            FPS_DISPLAY_MAX
        };
    }
}
