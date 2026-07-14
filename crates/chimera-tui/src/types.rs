//! TUI 核心类型 — 面板标识与应用状态
//!
//! 对应架构层:L10 Interface
//! 对应创新点:无(用户交互入口)
//!
//! # 设计决策(WHY)
//! - `PanelId` 为 enum:主面板(Quest/Parliament/Budget/Memory/Security/Health/Log/Help)
//!   语义清晰,匹配 §6 架构红线的"禁止功能标志"——面板是 UI 模式的离散投影。
//! - `TuiState` 为状态结构体:封装运行标志、输入缓冲、弹窗栈等,
//!   支持纯逻辑测试(无需终端)。
//! - `current_panel` 字段已移除(M1 清理项 #2):当前面板以 `FocusManager`
//!   为唯一来源,`TuiApp` 通过 `current_panel()` 方法对外暴露,避免双来源不一致。
use std::collections::{HashSet, VecDeque};

use crate::data::{BudgetMetrics, HealthMetrics, MemoryMetrics, SecurityState};
use crate::popup::{PopupStack, Severity};
use chrono::{DateTime, Utc};
use event_bus::{NexusEvent, VoteValue};
use nexus_core::Quest;
use serde::{Deserialize, Serialize};

// ============================================================
// 面板标识 — 主面板枚举
// ============================================================

/// 面板标识 — Chimera TUI 的主面板
///
/// - `Quest`:Quest 任务面板,显示任务列表与进度
/// - `Parliament`:议会面板,显示议员投票与共识
/// - `Budget`:预算面板,显示预算级别与消耗
/// - `Memory`:记忆面板,显示缓存命中率与上下文窗口
/// - `Security`:安全面板,显示 Skeptic 否决与红队审计
/// - `Health`:健康面板,显示事件速率与健康评分
/// - `Log`:日志面板,显示系统日志流
/// - `Help`:帮助面板,显示快捷键说明
/// - `Decay`:衰减面板,显示衰减系数与历史(P2.1 TUI v1.7-omega)
/// - `EventStream`:事件流面板,全量事件流虚拟滚动(P2.2)
/// - `Router`:路由统计面板,三路由器命中率与延迟(P2.3)
/// - `McpNodes`:MCP 节点面板,节点状态与心跳(P2.4)
/// - `Chtc`:CHTC 适配器面板,跨平台兼容性评分(P2.5)
/// - `Timeline`:时间轴面板,P7 历史回放(v1.8+ 接口占位)
///
/// WHY Copy + PartialEq:面板标识频繁参与比较与传递,Copy 避免克隆开销。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PanelId {
    /// Quest 任务面板 — 显示任务列表与进度
    Quest,
    /// 议会面板 — 显示议员投票与共识
    Parliament,
    /// 预算面板 — 显示预算级别与消耗
    Budget,
    /// 记忆面板 — 显示缓存命中率与上下文窗口
    Memory,
    /// 安全面板 — 显示 Skeptic 否决与红队审计
    Security,
    /// 健康面板 — 显示事件速率与健康评分
    Health,
    /// 日志面板 — 显示系统日志流
    Log,
    /// 帮助面板 — 显示快捷键说明
    Help,
    /// 衰减面板 — 显示衰减系数与历史(P2.1 TUI v1.7-omega)
    Decay,
    /// 事件流面板 — 全量事件流(虚拟滚动,P2.2)
    EventStream,
    /// 路由统计面板 — 三路由器命中率与延迟(P2.3)
    Router,
    /// MCP 节点面板 — 节点状态与心跳(P2.4)
    McpNodes,
    /// CHTC 适配器面板 — 跨平台兼容性评分(P2.5)
    Chtc,
    /// 时间轴面板 — P7 历史回放(v1.8+ 接口占位)
    Timeline,
}

impl PanelId {
    /// 返回面板的人类可读名称
    pub fn as_str(&self) -> &'static str {
        match self {
            PanelId::Quest => "Quest",
            PanelId::Parliament => "Parliament",
            PanelId::Budget => "Budget",
            PanelId::Memory => "Memory",
            PanelId::Security => "Security",
            PanelId::Health => "Health",
            PanelId::Log => "Log",
            PanelId::Help => "Help",
            PanelId::Decay => "Decay",
            PanelId::EventStream => "EventStream",
            PanelId::Router => "Router",
            PanelId::McpNodes => "McpNodes",
            PanelId::Chtc => "Chtc",
            PanelId::Timeline => "Timeline",
        }
    }

    /// 返回面板的标题(用于渲染边框)
    pub fn title(&self) -> &'static str {
        match self {
            PanelId::Quest => " Quest Tasks ",
            PanelId::Parliament => " Parliament ",
            PanelId::Budget => " Budget ",
            PanelId::Memory => " Memory ",
            PanelId::Security => " Security ",
            PanelId::Health => " Health ",
            PanelId::Log => " System Log ",
            PanelId::Help => " Help ",
            PanelId::Decay => " Decay ",
            PanelId::EventStream => " Event Stream ",
            PanelId::Router => " Router Stats ",
            PanelId::McpNodes => " MCP Nodes ",
            PanelId::Chtc => " CHTC Adapters ",
            PanelId::Timeline => " Timeline ",
        }
    }

    /// 切换到下一个面板(循环顺序)
    ///
    /// 完整循环(14 面板):
    /// Quest → Parliament → Budget → Memory → Security → Health → Log → Help
    /// → Decay → EventStream → Router → McpNodes → Chtc → Timeline → Quest
    pub fn next(&self) -> PanelId {
        match self {
            PanelId::Quest => PanelId::Parliament,
            PanelId::Parliament => PanelId::Budget,
            PanelId::Budget => PanelId::Memory,
            PanelId::Memory => PanelId::Security,
            PanelId::Security => PanelId::Health,
            PanelId::Health => PanelId::Log,
            PanelId::Log => PanelId::Help,
            PanelId::Help => PanelId::Decay,
            PanelId::Decay => PanelId::EventStream,
            PanelId::EventStream => PanelId::Router,
            PanelId::Router => PanelId::McpNodes,
            PanelId::McpNodes => PanelId::Chtc,
            PanelId::Chtc => PanelId::Timeline,
            PanelId::Timeline => PanelId::Quest,
        }
    }

    /// 切换到上一个面板(循环顺序)
    pub fn prev(&self) -> PanelId {
        match self {
            PanelId::Quest => PanelId::Timeline,
            PanelId::Parliament => PanelId::Quest,
            PanelId::Budget => PanelId::Parliament,
            PanelId::Memory => PanelId::Budget,
            PanelId::Security => PanelId::Memory,
            PanelId::Health => PanelId::Security,
            PanelId::Log => PanelId::Health,
            PanelId::Help => PanelId::Log,
            PanelId::Decay => PanelId::Help,
            PanelId::EventStream => PanelId::Decay,
            PanelId::Router => PanelId::EventStream,
            PanelId::McpNodes => PanelId::Router,
            PanelId::Chtc => PanelId::McpNodes,
            PanelId::Timeline => PanelId::Chtc,
        }
    }
}

impl std::fmt::Display for PanelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================
// 输入模式 — 命令面板/搜索面板/普通模式
// ============================================================

/// 输入模式 — 控制底部输入栏的行为
///
/// - `Normal`:普通模式,底部显示状态栏
/// - `Command`:命令模式(由 `:` 触发),解析并执行面板切换/退出等命令
/// - `Search`:搜索模式(由 `/` 触发),M1 为占位,仅接受输入
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InputMode {
    /// 普通模式
    Normal,
    /// 命令模式
    Command,
    /// 搜索模式
    Search,
}

// ============================================================
// 高层命令 — 面板返回的语义化动作
// ============================================================

/// 高层命令 — 由面板或命令面板产生,由 `TuiApp` 统一解释执行
///
/// WHY 引入命令抽象:将"按键语义"与"应用动作"解耦,
/// 后续 M3/M4 的控制事件可在此基础上扩展,而不影响面板实现。
#[derive(Debug, Clone, PartialEq)]
pub enum TuiCommand {
    /// 退出应用
    Quit,
    /// 切换到指定面板
    SwitchPanel(PanelId),
    /// 显示帮助面板
    ShowHelp,
    /// 打开弹窗
    OpenPopup(crate::popup::PopupKind),
    /// 请求暂停指定 Quest(M4 双向控制)
    RequestQuestPause(String),
    /// 请求恢复指定 Quest(M4 双向控制)
    RequestQuestResume(String),
    /// 请求对提案投票(M4 双向控制)
    RequestVote {
        /// 目标提案 ID
        proposal_id: String,
        /// 投票值
        vote: VoteValue,
    },
    /// 请求刷新状态(M4 双向控制)
    RequestRefresh,
}

// ============================================================
// 新面板数据类型 — P2 TUI v1.7-omega 共享基础设施
// ============================================================
//
// WHY 镜像 event-bus 类型而非直接复用:chimera-tui(L10)只依赖 L1 的
// event-bus + nexus-core,理论上可以直接复用 RouterStatsPayload 等类型。
// 但为了保持 TUI 内部状态的可演进性(例如未来添加 TUI 专有的展示字段),
// 并与现有 BudgetMetrics/MemoryMetrics 模式保持一致(均镜像 L9/L2 类型),
// 这里采用独立类型定义。同步逻辑在 data.rs 的 *Sync 同步器中完成。
// 参见 §2.2 依赖铁律:L10→L1 允许,但类型定义不跨层泄漏。

/// 衰减指标 — Decay 面板的数据视图(P2.1)
///
/// 镜像 `NexusEvent::DecayMetricsReported` 的载荷,由 `DecaySync` 填充。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecayMetrics {
    /// 当前衰减系数 [0.0, 1.0],1.0 表示无衰减
    pub coefficient: f32,
    /// 本周期内触发衰减的最近事件摘要
    pub recent_events: Vec<String>,
    /// 本衰减周期开始时间,None 表示尚未收到任何衰减事件
    pub cycle_start: Option<DateTime<Utc>>,
}

impl Default for DecayMetrics {
    fn default() -> Self {
        // WHY coefficient=1.0:无衰减事件时默认满血,避免面板显示误导性低系数
        Self {
            coefficient: 1.0,
            recent_events: Vec::new(),
            cycle_start: None,
        }
    }
}

/// 路由器统计信息 — Router 面板的单路由器数据视图(P2.3)
///
/// 镜像 `event_bus::RouterStatsPayload`,避免 L10→L1 类型强耦合,
/// 同时与 BudgetMetrics 模式保持一致。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouterStatsInfo {
    /// 命中率 [0.0, 1.0]
    pub hit_rate: f32,
    /// P50 延迟(微秒)
    pub p50_latency_us: u64,
    /// P95 延迟(微秒)
    pub p95_latency_us: u64,
    /// P99 延迟(微秒)
    pub p99_latency_us: u64,
    /// 热点能力列表(能力 ID,调用次数)
    pub hot_capabilities: Vec<(String, u64)>,
}

impl Default for RouterStatsInfo {
    fn default() -> Self {
        Self {
            hit_rate: 0.0,
            p50_latency_us: 0,
            p95_latency_us: 0,
            p99_latency_us: 0,
            hot_capabilities: Vec::new(),
        }
    }
}

/// 路由器指标 — 三路由器(KVBSR/SESA/FaaE)聚合视图(P2.3)
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RouterMetrics {
    /// KVBSR 路由器统计
    pub kvbsr_stats: RouterStatsInfo,
    /// SESA 路由器统计
    pub sesa_stats: RouterStatsInfo,
    /// FaaE 路由器统计
    pub faae_stats: RouterStatsInfo,
}

/// 节点状态枚举 — MCP 节点健康状态(P2.4)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeStatus {
    /// 在线,正常服务
    Online,
    /// 降级,部分功能受限
    Degraded,
    /// 离线,不可达
    Offline,
}

/// MCP 节点状态 — McpNodes 面板的单节点视图(P2.4)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpNodeStatus {
    /// 节点 ID
    pub node_id: String,
    /// 节点状态
    pub status: NodeStatus,
    /// 节点吞吐量(每秒事务数)
    pub throughput: u64,
    /// 最近一次心跳时间,None 表示尚未收到心跳
    pub last_seen: Option<DateTime<Utc>>,
}

/// CHTC 适配器信息 — Chtc 面板的单适配器视图(P2.5)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChtcAdapterInfo {
    /// 适配器 ID
    pub adapter_id: String,
    /// 适配器类型(如 "vscode"/"jetbrains"/"vim"/"emacs"/"cli")
    pub adapter_type: String,
    /// 兼容性评分 [0, 100]
    pub compatibility_score: u8,
    /// 最近请求(请求标识, 次数)列表
    pub recent_requests: Vec<(String, u32)>,
    /// 是否在线
    pub is_online: bool,
}

/// CHTC 状态 — 5 IDE 适配器聚合视图(P2.5)
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ChtcState {
    /// 全部适配器列表
    pub adapters: Vec<ChtcAdapterInfo>,
}

/// 时间轴快照 — P7 历史回放的接口占位(v1.8+ 实现)
///
/// WHY 现在定义:让 TuiState 与 DataSnapshot 提前预留字段,
/// v1.8 实现历史回放时无需再破坏性扩展结构体。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TimelineSnapshot {
    /// 快照时间戳,None 表示尚未生成
    pub timestamp: Option<DateTime<Utc>>,
    /// 快照时点的事件总数
    pub event_count: u64,
}

// ============================================================
// TUI 状态 — 应用运行时状态
// ============================================================

/// TUI 状态 — 应用运行时的可变状态
///
/// WHY 独立结构体:将状态与渲染逻辑分离,便于纯逻辑测试(无需终端)。
/// `running` 标志控制事件循环退出。
///
/// WHY 移除 `current_panel`(M1 清理项 #2):当前面板以 `FocusManager` 为
/// 唯一来源,避免 `TuiState` 与 `FocusManager` 双来源不一致。
///
/// WHY 移除 `Eq`: `BudgetMetrics` 等包含浮点字段(f32/f64),浮点数不满足
/// `Eq`;保留 `PartialEq` 以便测试比较与快照校验。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TuiState {
    /// 是否正在运行(false 时事件循环退出)
    pub running: bool,
    /// 当前输入模式
    pub input_mode: InputMode,
    /// 输入缓冲(命令模式/搜索模式使用)
    pub input_buffer: String,
    /// 已渲染的帧数(用于调试与性能监控)
    pub frame_count: u64,
    /// 当前 Quest 列表(数据驱动 Quest 面板)
    pub quest_list: Vec<Quest>,
    /// 当前预算指标(数据驱动 Budget 面板)
    pub budget: BudgetMetrics,
    /// 当前记忆指标(数据驱动 Memory 面板)
    pub memory_metrics: MemoryMetrics,
    /// 当前安全状态(数据驱动 Security 面板)
    pub security_state: SecurityState,
    /// 当前健康指标(数据驱动 Health 面板)
    pub health_metrics: HealthMetrics,
    /// 预算利用率历史(数据驱动 Budget Sparkline)
    pub budget_history: Vec<u64>,
    /// 缓存命中率历史(数据驱动 Memory Sparkline)
    pub memory_history: Vec<u64>,
    /// 事件速率历史(数据驱动 Health Sparkline)
    pub event_rate_history: Vec<u64>,
    /// 最近事件流(数据驱动 Parliament / Log 面板)
    pub latest_events: VecDeque<NexusEvent>,
    /// 弹窗栈(详情/通知/确认)
    pub popup_stack: PopupStack,
    /// 临时状态栏消息(内容 + 严重级别)
    pub status_message: Option<(String, Severity)>,
    /// 关键字过滤器 — 应用于 Log / Quest 面板
    pub filter_keyword: Option<String>,
    /// 主题过滤器 — 应用于 Log 面板的事件主题
    pub filter_topic: Option<String>,
    /// 级别过滤器 — 应用于 Log 面板的事件严重级别
    pub filter_level: Option<String>,
    // === P2 TUI v1.7-omega 新增字段 ===
    /// 衰减指标(数据驱动 Decay 面板)
    pub decay_metrics: DecayMetrics,
    /// 路由器指标(数据驱动 Router 面板)
    pub router_metrics: RouterMetrics,
    /// MCP 节点状态列表(数据驱动 McpNodes 面板)
    pub mcp_nodes: Vec<McpNodeStatus>,
    /// CHTC 适配器状态(数据驱动 Chtc 面板)
    pub chtc_state: ChtcState,
    /// 时间轴快照(P7 接口占位,v1.8+ 实现)
    pub timeline_snapshots: Vec<TimelineSnapshot>,
    /// FPS 显示(P4.4 性能监控)
    pub fps: u16,
    /// 增量渲染脏面板集合(P4.1,记录本帧需重绘的面板)
    pub dirty_panels: HashSet<PanelId>,
    /// 流式追加自动滚动标记(P3.4,EventStream/Log 面板用)
    pub auto_scroll: bool,
    /// g 前缀状态(P3.3):按下 `g` 后进入等待状态,下一键决定动作。
    /// - `g` + `1`-`5`:切换到后 5 个面板(EventStream/Router/McpNodes/Chtc/Timeline)
    /// - `g` + `g`:调用当前面板 scroll_to_top(gg 跳顶,与 vim 一致)
    /// - `g` + 其他键:重置前缀,将后续键委托给当前面板处理,避免卡死
    pub g_prefix: bool,
    /// 衰减历史 sparkline 数据点(系数 × 1000 的整型表示)
    pub decay_history: Vec<u64>,
}

impl TuiState {
    /// 创建新的初始状态(默认 Quest 面板,运行中)
    pub fn new() -> Self {
        Self {
            running: true,
            input_mode: InputMode::Normal,
            input_buffer: String::new(),
            frame_count: 0,
            quest_list: Vec::new(),
            budget: BudgetMetrics::default(),
            memory_metrics: MemoryMetrics::default(),
            security_state: SecurityState::default(),
            health_metrics: HealthMetrics::default(),
            budget_history: Vec::new(),
            memory_history: Vec::new(),
            event_rate_history: Vec::new(),
            latest_events: VecDeque::new(),
            popup_stack: PopupStack::new(),
            status_message: None,
            filter_keyword: None,
            filter_topic: None,
            filter_level: None,
            // P2 新增字段默认值
            decay_metrics: DecayMetrics::default(),
            router_metrics: RouterMetrics::default(),
            mcp_nodes: Vec::new(),
            chtc_state: ChtcState::default(),
            timeline_snapshots: Vec::new(),
            fps: 0,
            dirty_panels: HashSet::new(),
            auto_scroll: true,
            g_prefix: false,
            decay_history: Vec::new(),
        }
    }

    /// 退出应用(设置 running = false)
    pub fn quit(&mut self) {
        self.running = false;
    }

    /// 追加输入到缓冲
    pub fn append_input(&mut self, ch: char) {
        self.input_buffer.push(ch);
    }

    /// 清空输入缓冲
    pub fn clear_input(&mut self) {
        self.input_buffer.clear();
    }

    /// 清空所有过滤器
    pub fn clear_filters(&mut self) {
        self.filter_keyword = None;
        self.filter_topic = None;
        self.filter_level = None;
    }

    /// 设置状态栏消息
    pub fn set_status(&mut self, message: impl Into<String>, severity: Severity) {
        self.status_message = Some((message.into(), severity));
    }

    /// 增加帧计数
    pub fn tick_frame(&mut self) {
        self.frame_count += 1;
    }
}

impl Default for TuiState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // PanelId 测试
    // ============================================================

    #[test]
    fn test_panel_id_as_str() {
        assert_eq!(PanelId::Quest.as_str(), "Quest");
        assert_eq!(PanelId::Parliament.as_str(), "Parliament");
        assert_eq!(PanelId::Budget.as_str(), "Budget");
        assert_eq!(PanelId::Memory.as_str(), "Memory");
        assert_eq!(PanelId::Security.as_str(), "Security");
        assert_eq!(PanelId::Health.as_str(), "Health");
        assert_eq!(PanelId::Log.as_str(), "Log");
        assert_eq!(PanelId::Help.as_str(), "Help");
    }

    #[test]
    fn test_panel_id_title() {
        assert_eq!(PanelId::Quest.title(), " Quest Tasks ");
        assert_eq!(PanelId::Budget.title(), " Budget ");
        assert_eq!(PanelId::Memory.title(), " Memory ");
    }

    #[test]
    fn test_panel_id_next() {
        assert_eq!(PanelId::Quest.next(), PanelId::Parliament);
        assert_eq!(PanelId::Parliament.next(), PanelId::Budget);
        assert_eq!(PanelId::Budget.next(), PanelId::Memory);
        assert_eq!(PanelId::Memory.next(), PanelId::Security);
        assert_eq!(PanelId::Security.next(), PanelId::Health);
        assert_eq!(PanelId::Health.next(), PanelId::Log);
        assert_eq!(PanelId::Log.next(), PanelId::Help);
        // P2 扩展:Help → Decay(不再是 Help → Quest)
        assert_eq!(PanelId::Help.next(), PanelId::Decay);
        assert_eq!(PanelId::Decay.next(), PanelId::EventStream);
        assert_eq!(PanelId::EventStream.next(), PanelId::Router);
        assert_eq!(PanelId::Router.next(), PanelId::McpNodes);
        assert_eq!(PanelId::McpNodes.next(), PanelId::Chtc);
        assert_eq!(PanelId::Chtc.next(), PanelId::Timeline);
        // 循环:Timeline → Quest
        assert_eq!(PanelId::Timeline.next(), PanelId::Quest);
    }

    #[test]
    fn test_panel_id_prev() {
        assert_eq!(PanelId::Parliament.prev(), PanelId::Quest);
        assert_eq!(PanelId::Budget.prev(), PanelId::Parliament);
        assert_eq!(PanelId::Memory.prev(), PanelId::Budget);
        assert_eq!(PanelId::Security.prev(), PanelId::Memory);
        assert_eq!(PanelId::Health.prev(), PanelId::Security);
        assert_eq!(PanelId::Log.prev(), PanelId::Health);
        assert_eq!(PanelId::Help.prev(), PanelId::Log);
        // P2 扩展:Decay → Help(不再是 Quest → Help)
        assert_eq!(PanelId::Decay.prev(), PanelId::Help);
        assert_eq!(PanelId::EventStream.prev(), PanelId::Decay);
        assert_eq!(PanelId::Router.prev(), PanelId::EventStream);
        assert_eq!(PanelId::McpNodes.prev(), PanelId::Router);
        assert_eq!(PanelId::Chtc.prev(), PanelId::McpNodes);
        assert_eq!(PanelId::Timeline.prev(), PanelId::Chtc);
        // 循环:Quest → Timeline
        assert_eq!(PanelId::Quest.prev(), PanelId::Timeline);
    }

    #[test]
    fn test_panel_id_next_prev_roundtrip() {
        // next 再 prev 应回到原面板(P2 扩展至 14 面板)
        for panel in [
            PanelId::Quest,
            PanelId::Parliament,
            PanelId::Budget,
            PanelId::Memory,
            PanelId::Security,
            PanelId::Health,
            PanelId::Log,
            PanelId::Help,
            PanelId::Decay,
            PanelId::EventStream,
            PanelId::Router,
            PanelId::McpNodes,
            PanelId::Chtc,
            PanelId::Timeline,
        ] {
            assert_eq!(panel.next().prev(), panel);
            assert_eq!(panel.prev().next(), panel);
        }
    }

    #[test]
    fn test_panel_id_display() {
        assert_eq!(PanelId::Quest.to_string(), "Quest");
    }

    #[test]
    fn test_panel_id_serde_roundtrip() {
        let panel = PanelId::Budget;
        let json = serde_json::to_string(&panel).unwrap();
        let restored: PanelId = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, panel);
    }

    #[test]
    fn test_state_filters_roundtrip() {
        let mut state = TuiState::new();
        state.filter_keyword = Some("foo".into());
        state.filter_topic = Some("security".into());
        state.filter_level = Some("critical".into());
        let json = serde_json::to_string(&state).unwrap();
        let restored: TuiState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.filter_keyword, Some("foo".into()));
        assert_eq!(restored.filter_topic, Some("security".into()));
        assert_eq!(restored.filter_level, Some("critical".into()));
    }

    #[test]
    fn test_state_clear_filters() {
        let mut state = TuiState::new();
        state.filter_keyword = Some("foo".into());
        state.filter_topic = Some("security".into());
        state.filter_level = Some("critical".into());
        state.clear_filters();
        assert!(state.filter_keyword.is_none());
        assert!(state.filter_topic.is_none());
        assert!(state.filter_level.is_none());
    }

    #[test]
    fn test_state_set_status() {
        let mut state = TuiState::new();
        state.set_status("error", Severity::Error);
        assert_eq!(
            state.status_message,
            Some(("error".into(), Severity::Error))
        );
    }

    // ============================================================
    // InputMode 测试
    // ============================================================

    #[test]
    fn test_input_mode_equality() {
        assert_eq!(InputMode::Normal, InputMode::Normal);
        assert_ne!(InputMode::Normal, InputMode::Command);
    }

    // ============================================================
    // TuiCommand 测试
    // ============================================================

    #[test]
    fn test_tui_command_variants() {
        let cmd = TuiCommand::SwitchPanel(PanelId::Budget);
        assert_eq!(cmd, TuiCommand::SwitchPanel(PanelId::Budget));
    }

    // ============================================================
    // TuiState 测试
    // ============================================================

    #[test]
    fn test_state_new() {
        let state = TuiState::new();
        assert!(state.running);
        assert_eq!(state.input_mode, InputMode::Normal);
        assert!(state.input_buffer.is_empty());
        assert_eq!(state.frame_count, 0);
        assert!(state.popup_stack.is_empty());
        assert!(state.status_message.is_none());
        assert_eq!(state.health_metrics.health_score, 100);
    }

    #[test]
    fn test_state_quit() {
        let mut state = TuiState::new();
        assert!(state.running);
        state.quit();
        assert!(!state.running);
    }

    #[test]
    fn test_state_input_buffer() {
        let mut state = TuiState::new();
        state.append_input('a');
        state.append_input('b');
        state.append_input('c');
        assert_eq!(state.input_buffer, "abc");
        state.clear_input();
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn test_state_tick_frame() {
        let mut state = TuiState::new();
        assert_eq!(state.frame_count, 0);
        state.tick_frame();
        state.tick_frame();
        state.tick_frame();
        assert_eq!(state.frame_count, 3);
    }

    #[test]
    fn test_state_serde_roundtrip() {
        let state = TuiState::new();
        let json = serde_json::to_string(&state).unwrap();
        let restored: TuiState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, state);
    }
}
