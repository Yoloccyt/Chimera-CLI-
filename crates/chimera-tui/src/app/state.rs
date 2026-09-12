//! 状态管理 — 数据更新、面板切换与辅助访问器
//!
//! 包含 [`TuiApp::update`]、面板切换、伴随面板/窗格访问器、
//! FPS 计算等状态管理方法。
//!
//! 对应架构层:L10 Interface

use std::sync::Arc;
use std::time::Duration;

use super::TuiApp;
use crate::data::DataSnapshot;
use crate::popup::Severity;
use crate::types::{PanelId, TuiState};

/// 同步 OSA / CLV / Timeline 可视化字段(DataSnapshot → TuiState)
///
/// WHY 独立纯函数(FC-1,2026-09-06 四维评估):DataPipeline 早已把稀疏度/
/// CLV 摘要/Timeline 周期快照写入 DataSnapshot,但 update 的字段同步清单曾
/// 遗漏,致 OsaSparse / ClvVector / Timeline 三面板生产环境恒空;抽取为
/// 模块内纯函数,便于 proptest 直接对任意快照字段做镜像属性验证,
/// 也避免 update 方法随字段增长继续膨胀。
fn sync_visualization_fields(state: &mut TuiState, snapshot: &DataSnapshot) {
    state.osa_sparsity = snapshot.osa_sparsity;
    state.osa_context_mask = snapshot.osa_context_mask.clone();
    state.osa_sparsity_history = snapshot.osa_sparsity_history.clone();
    state.clv_summary = snapshot.clv_summary.clone();
    state.recall_needle_at_8 = snapshot.recall_needle_at_8;
    state.recall_position_bias = snapshot.recall_position_bias;
    state.recall_chain_success = snapshot.recall_chain_success;
    state.timeline_snapshots = snapshot.timeline_snapshots.clone();
}

/// 声明式 dirty 面板标记宏(Task 1.17)
///
/// 替代 `mark_dirty_panels_from_snapshot` 中手动 `!=` 比较 12+ 字段的样板代码,
/// 以声明式语法定义"状态字段 → 面板"映射,宏自动生成比较 + 标记代码。
///
/// # 语法
/// ```text
/// dirty_map!($state, $snapshot,
///     $( $( $state_field : $snap_field ),+ => $( $panel ),+ );* $(;)?
/// );
/// ```
/// - 每个 arm 以 `;` 分隔
/// - 字段对以 `,` 分隔(多字段任一变化即标记)
/// - `state_field:snap_field` 指定状态字段名与快照字段名(相同也要显式写出)
/// - 面板以 `,` 分隔(多个面板同时标记)
///
/// # 生成代码
/// 对每个 arm 生成:
/// ```text
/// if state.field1 != snap.field1 || state.field2 != snap.field2 {
///     state.mark_dirty(Panel1);
///     state.mark_dirty(Panel2);
/// }
/// ```
///
/// # 使用限制
/// - 仅适用于无条件映射;有条件的(如 `monitor_paused` 守卫)仍需手动 if 块
/// - 字段必须实现 `PartialEq`
macro_rules! dirty_map {
    ($state:expr, $snapshot:expr,
     $( $( $sf:ident : $nf:ident ),+ => $( $panel:expr ),+ );* $(;)?
    ) => {
        $(
            if $( $state.$sf != $snapshot.$nf )||+ {
                $(
                    $state.mark_dirty($panel);
                )+
            }
        )*
    };
}

impl TuiApp {
    /// 从数据源拉取最新快照并更新内部状态,含 P4.1 脏面板标记检测。
    pub fn update(&mut self) {
        // P1-2:待确认动作超时检测每帧执行(独立于 revision 跳过——
        // 编排器未接线时数据可能长期无变化,超时提示仍需按时触发)。
        self.check_action_timeout();
        match self.data_source.snapshot() {
            Ok(snapshot) => {
                // P2 性能(P-1):revision 未变化时跳过整帧字段拷贝与 dirty 标记。
                // WHY 事件循环轮询(100ms)快于数据 tick(250ms),两 tick 之间快照
                // 内容不变;此前每帧都深拷贝 latest_events/历史曲线,约 60% 的
                // update 调用是无意义拷贝。revision == 0 表示测试桩/默认快照,
                // 始终拷贝以保持既有测试语义(增量渲染测试依赖该路径)。
                if snapshot.revision != 0 && snapshot.revision == self.state.last_snapshot_revision
                {
                    return;
                }

                // P4.1:在覆盖状态前检测哪些面板数据发生变化,先打 dirty 标记
                self.mark_dirty_panels_from_snapshot(&snapshot);

                // 快照现为 Arc 共享不可变结构:字段改为 clone 提取
                // (此前是从管道克隆副本中 move,Arc 化后 move 语义不可用)。
                self.state.quest_list = snapshot.quest_list.clone();
                self.state.paused_quest_count = snapshot.paused_quest_count;
                self.state.budget = snapshot.budget_metrics.clone();
                // Concord T1.7:陈旧标志随指标同步(驱动 Budget 面板置灰)
                self.state.budget_metrics_stale = snapshot.budget_metrics_stale;
                self.state.memory_metrics = snapshot.memory_metrics.clone();
                self.state.security_state = snapshot.security_state.clone();
                self.state.health_metrics = snapshot.health_metrics.clone();
                self.state.budget_history = snapshot.budget_history.clone();
                self.state.memory_history = snapshot.memory_history.clone();
                self.state.event_rate_history = snapshot.event_rate_history.clone();
                // Arc 共享事件流(P-A):直接共享快照的 Arc,零拷贝
                // (此前解引用深拷贝 ≤256 事件/每 revision 变化)
                self.state.latest_events = Arc::clone(&snapshot.latest_events);
                // P2 新增字段同步:DataSnapshot → TuiState
                self.state.decay_metrics = snapshot.decay_metrics.clone();
                self.state.router_metrics = snapshot.router_metrics.clone();
                self.state.mcp_nodes = snapshot.mcp_nodes.clone();
                self.state.chtc_state = snapshot.chtc_state.clone();
                self.state.decay_history = snapshot.decay_history.clone();
                // P8 ResourceMonitor 面板字段同步:DataSnapshot → TuiState
                // M3 monitor.pause_sampling:暂停时跳过覆盖,保留冻结快照供检视(UI 本地冻结)
                if !self.state.monitor_paused {
                    self.state.sys_metrics = snapshot.sys_metrics.clone();
                    self.state.sys_metrics_history = snapshot.sys_metrics_history.clone();
                }
                // Task 6:同步 tick 模式,供状态栏展示
                self.state.tick_mode = snapshot.tick_mode;
                // M3b:同步对话历史与状态到 TuiState(供 Chat 面板渲染)
                self.state.chat_messages = snapshot.chat_messages.clone();
                self.state.chat_status = snapshot.chat_status;
                // FC-1(2026-09-06):同步 OSA/CLV/Timeline 可视化字段,
                // 修复三面板生产恒空的最后一公里断链
                sync_visualization_fields(&mut self.state, &snapshot);
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
                    // P1:已收到动作终态反馈,按 request_id 精准移除该请求的超时计时。
                    // 不再全局清空——否则并发/连续发起的其它请求会被误判为"已回执",
                    // 从而丢失超时兜底提示(静默失败)。
                    if let Some(rid) = &snapshot.action_feedback_request_id {
                        self.state.pending_actions.remove(rid);
                    }
                }
                // P1-W2.2:同步 Critical 旁路通道丢弃计数(EventStream 面板告警显示)
                self.state.critical_event_dropped_count = snapshot.critical_event_dropped_count;
                // PS-2(F-1):协议握手回执上屏(ADR-082)—— 状态变化时一次性提示
                //
                // WHY 变化驱动而非 seq:SEC-4 保证正常运行只到达一帧 Ack,以
                // "与本地状态不等"判定新回执,天然去重且对重连场景鲁棒。
                // 严重度映射:Full=Info(信道建立,静默记录即可见)、
                // Degraded=Warning(携降级项)、Refused=Error(版本不可调和)。
                // 异常态上屏是本闭环的存在意义(评估报告 F-1:此前 UI 无感知)。
                if snapshot.handshake != self.state.handshake {
                    if let Some(hs) = &snapshot.handshake {
                        use crate::types::HandshakeLevel;
                        let (key, severity) = match hs.level {
                            HandshakeLevel::Full => ("status.handshake.full", Severity::Info),
                            HandshakeLevel::Degraded => {
                                ("status.handshake.degraded", Severity::Warning)
                            }
                            HandshakeLevel::Refused => {
                                ("status.handshake.refused", Severity::Error)
                            }
                        };
                        let msg = match hs.level {
                            HandshakeLevel::Degraded => format!(
                                "{} {}: v{}",
                                crate::t!(key),
                                hs.degraded_items.join("/"),
                                hs.server_version
                            ),
                            _ => format!("{} v{}", crate::t!(key), hs.server_version),
                        };
                        self.state.status_message = Some((msg, severity));
                    }
                    self.state.handshake = snapshot.handshake.clone();
                }
                // PS-2(F-6):子代理失败聚合同步 + 新失败一次性告警(Critical)
                //
                // WHY 序号驱动而非每帧上屏:重试风暴下每 tick 都可能有新失败,
                // 若每帧都设 status_message,告警会被自身反复覆盖而无法阅读;
                // 以 seq 增量判定"新失败",单条告警即可提示,完整清单在安全面板。
                self.state.agent_failures = snapshot.agent_failures.clone();
                self.state.agent_failure_total = snapshot.agent_failure_total;
                if snapshot.agent_failure_seq > self.state.last_agent_failure_seq {
                    self.state.last_agent_failure_seq = snapshot.agent_failure_seq;
                    if let Some(latest) = self.state.agent_failures.first() {
                        self.state.status_message = Some((
                            format!(
                                "{} {} → {} ({})",
                                crate::t!("status.agent_failure"),
                                latest.from,
                                latest.to,
                                latest.task_id
                            ),
                            Severity::Error,
                        ));
                    }
                }
                // PS-2 批次1:议会数据同步(面板只读快照,不再直调 L8)
                self.state.parliament = snapshot.parliament.clone();
                // PS-2 批次2:GQEP 超时统计同步(面板只读快照,不再直调 L7)
                self.state.gqep_timeouts = snapshot.gqep_timeouts.clone();
                // P2 性能:记录已同步的 revision,供 update 跳过与面板过滤缓存失效判断
                self.state.last_snapshot_revision = snapshot.revision;
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
    /// - `chat_messages` / `chat_status` → Chat
    /// - `osa_*` / `recall_*` → OsaSparse;`clv_summary` → ClvVector;
    ///   `timeline_snapshots` → Timeline(FC-1,2026-09-06 补齐)
    /// - `critical_event_dropped_count` → EventStream
    fn mark_dirty_panels_from_snapshot(&mut self, snapshot: &DataSnapshot) {
        // Task 1.17.2:声明式 dirty 面板标记 — 用 `dirty_map!` 宏替代手动 `!=` 比较。
        // WHY 宏化:集中维护"字段 → PanelId"映射表,新增字段只需加一行 arm,
        // 避免手动 if 块的样板代码膨胀;多字段 OR / 多面板标记语义在宏层声明式表达。
        // WHY 仍保留一处手动 if 块:`sys_metrics` / `sys_metrics_history` 受
        // `monitor_paused` 守卫保护(暂停时冻结显示),宏不支持条件分支映射。
        // 字段对语法 `state_field:snap_field` 支持状态与快照字段名不一致
        // (如 `budget:budget_metrics`),相同名也需显式写出(`quest_list:quest_list`)。
        dirty_map!(self.state, snapshot,
            // quest_list → Quest + Health(Active Quests 从 quest_list.len() 派生)
            quest_list: quest_list => PanelId::Quest, PanelId::Health;
            // budget + budget_history → Budget(任一变化均触发)
            budget: budget_metrics, budget_history: budget_history => PanelId::Budget;
            // memory_metrics + memory_history → Memory(任一变化均触发)
            memory_metrics: memory_metrics, memory_history: memory_history => PanelId::Memory;
            // security_state → Security
            security_state: security_state => PanelId::Security;
            // health_metrics + event_rate_history + paused_quest_count → Health
            health_metrics: health_metrics, event_rate_history: event_rate_history, paused_quest_count: paused_quest_count => PanelId::Health;
            // decay_metrics + decay_history → Decay
            decay_metrics: decay_metrics, decay_history: decay_history => PanelId::Decay;
            // router_metrics → Router
            router_metrics: router_metrics => PanelId::Router;
            // mcp_nodes → McpNodes
            mcp_nodes: mcp_nodes => PanelId::McpNodes;
            // chtc_state → Chtc
            chtc_state: chtc_state => PanelId::Chtc;
            // M3b:chat_messages + chat_status → Chat
            chat_messages: chat_messages, chat_status: chat_status => PanelId::Chat;
            // P1-W2.2:critical_event_dropped_count → EventStream(顶部告警行)
            critical_event_dropped_count: critical_event_dropped_count => PanelId::EventStream;
            // FC-1(2026-09-06):OSA 稀疏度/召回读数 → OsaSparse;
            // CLV 摘要 → ClvVector;Timeline 周期快照 → Timeline
            // WHY 此前缺失:快照侧已产出但无 dirty 标记,即使补上字段同步,
            // 面板也不会因数据变化被标记重绘。
            osa_sparsity: osa_sparsity, osa_context_mask: osa_context_mask, osa_sparsity_history: osa_sparsity_history, recall_needle_at_8: recall_needle_at_8, recall_position_bias: recall_position_bias, recall_chain_success: recall_chain_success => PanelId::OsaSparse;
            clv_summary: clv_summary => PanelId::ClvVector;
            timeline_snapshots: timeline_snapshots => PanelId::Timeline;
        );

        // P8:系统资源指标变化时标记 ResourceMonitor 面板 dirty,
        // 同时标记 Health 面板(Health 面板也展示系统资源摘要)
        // M3 monitor.pause_sampling:暂停时冻结显示,不因快照变化重标 dirty(避免每 tick 重绘冻结数据)
        // 保留手动 if 块:有 monitor_paused 守卫的条件映射,dirty_map! 宏不支持条件分支。
        if !self.state.monitor_paused
            && (self.state.sys_metrics != snapshot.sys_metrics
                || self.state.sys_metrics_history != snapshot.sys_metrics_history)
        {
            self.state.mark_dirty(PanelId::ResourceMonitor);
            self.state.mark_dirty(PanelId::Health);
        }

        // WHY latest_events 同时驱动 Parliament / Log / EventStream 三面板,
        // 任一变化都需标记这三个面板,避免事件流面板错过新事件。
        // WHY Arc 直接比较(P-A):state 与 snapshot 两侧均为 Arc<VecDeque>,
        // Arc 的 PartialEq 透传内部 VecDeque 逐条比较,语义与此前解引用比较一致。
        if self.state.latest_events != snapshot.latest_events {
            self.state.mark_dirty(PanelId::Parliament);
            self.state.mark_dirty(PanelId::Log);
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
            // Task 1.15.4:委托到 pane_manager
            self.pane_manager.prev_panel = Some(before);
            // Stage 2/M3d:主区焦点变化时复位活跃窗格回主区,避免焦点滞留旧次窗格。
            self.pane_manager.active_pane = 0;
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
    ///
    /// Task 1.15.4:委托到 `PaneManager::adjust_main_panel_ratio`,避免逻辑重复。
    pub(super) fn adjust_main_panel_ratio(&mut self, increase: bool) {
        self.pane_manager.adjust_main_panel_ratio(increase);
    }

    /// 更新 FPS 移动平均(P4.4)
    ///
    /// WHY 使用移动平均:单帧耗时受 OS 调度、事件循环等待、IO 等影响波动较大,
    /// 直接显示瞬时 FPS 会让状态栏数字频繁跳动、难以阅读。固定窗口移动平均
    /// 平滑短时抖动,同时对真实帧率下降仍保持灵敏响应。
    ///
    /// Task 1.15.4:委托到 `FpsCounter::update_fps`,避免逻辑重复。
    pub(super) fn update_fps(&mut self, delta: Duration) {
        self.state.fps = self.fps_counter.update_fps(delta);
    }
}

#[cfg(test)]
mod visualization_sync_proptests {
    //! FC-1 回归护栏:任意合法快照字段经 `sync_visualization_fields`
    //! 必须完整镜像进 TuiState(防止未来字段增长再次遗漏同步)。

    use super::sync_visualization_fields;
    use crate::data::DataSnapshot;
    use crate::types::{TimelineSnapshot, TuiState};
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn sync_mirrors_snapshot_visualization_fields(
            sparsity in proptest::option::of(0.0f32..=1.0),
            mask_items in proptest::collection::vec("[a-z]{1,6}\\.rs", 0..8),
            history in proptest::collection::vec(0u64..=1000, 0..16),
            needle in proptest::option::of(0.0f32..=1.0),
            bias in proptest::option::of(0.0f32..=1.0),
            chain in proptest::option::of(0.0f32..=1.0),
            l2_norm in 0.0f32..=100.0,
            snap_count in 0usize..4,
        ) {
            let mut state = TuiState::new();
            // WHY 结构体更新语法:clippy::field_reassign_with_default 禁止
            // 对 Default 实例逐字段赋值,且声明式初始化更利于字段清单审阅
            let snapshot = DataSnapshot {
                osa_sparsity: sparsity,
                osa_context_mask: mask_items.clone(),
                osa_sparsity_history: history.clone(),
                recall_needle_at_8: needle,
                recall_position_bias: bias,
                recall_chain_success: chain,
                clv_summary: Some(event_bus::ClvSummary {
                    block_means: vec![0.0; 8],
                    l2_norm,
                    top_dims: Vec::new(),
                }),
                timeline_snapshots: (0..snap_count)
                    .map(|i| TimelineSnapshot {
                        timestamp: chrono::Utc::now(),
                        event_count: i as u64,
                        event_rate: 0,
                        budget_utilization: 0.0,
                        health_score: 100,
                        decay_coefficient: 0.0,
                    })
                    .collect(),
                ..Default::default()
            };

            sync_visualization_fields(&mut state, &snapshot);

            prop_assert_eq!(state.osa_sparsity, sparsity);
            prop_assert_eq!(state.osa_context_mask, mask_items);
            prop_assert_eq!(state.osa_sparsity_history, history);
            prop_assert_eq!(state.recall_needle_at_8, needle);
            prop_assert_eq!(state.recall_position_bias, bias);
            prop_assert_eq!(state.recall_chain_success, chain);
            prop_assert_eq!(
                state.clv_summary.as_ref().map(|s| s.l2_norm),
                Some(l2_norm)
            );
            prop_assert_eq!(state.timeline_snapshots.len(), snap_count);
        }
    }
}
