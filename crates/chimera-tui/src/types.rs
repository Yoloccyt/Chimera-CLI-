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
/// - `OsaSparse`:OSA 稀疏度可视化面板,OMEGA Ω-Sparse 定律可视化
/// - `ClvVector`:CLV 向量可视化面板,512 维潜在向量摘要展示
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
    /// OSA 稀疏度可视化面板 — OMEGA Ω-Sparse 定律可视化
    ///
    /// 展示 OmniSparseMasksComputed 事件的平均稀疏度 + context 维度活跃文件列表。
    OsaSparse,
    /// CLV 向量可视化面板 — 512 维潜在向量摘要展示
    ///
    /// 展示 ClvSnapshotReported 事件的 8 分块热图 + L2 范数 + Top-8 维度。
    ClvVector,
    /// 系统资源监控面板 — CPU/内存/磁盘/网络实时指标
    ///
    /// 展示 sysinfo 采集的 OS 级系统资源使用情况。
    ResourceMonitor,
    /// 指标仪表盘面板 — 5×2 网格 + 可绑定数据源(v1.8-omega Task 2.2)
    ///
    /// 5×2 网格:左列 5 个 sparkline 实时指标,右列 5 个 gauge 当前值;
    /// 每个 cell 可独立绑定 `TuiDataSource` + `VizChartKind`,复用
    /// `viz/` 组件库渲染。
    MetricsDashboard,
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
            PanelId::OsaSparse => "OsaSparse",
            PanelId::ClvVector => "ClvVector",
            PanelId::ResourceMonitor => "ResourceMonitor",
            PanelId::MetricsDashboard => "MetricsDashboard",
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
            PanelId::OsaSparse => " OSA Sparse ",
            PanelId::ClvVector => " CLV Vector ",
            PanelId::ResourceMonitor => " Resources ",
            PanelId::MetricsDashboard => " Metrics Dashboard ",
        }
    }

    /// 切换到下一个面板(循环顺序)
    ///
    /// 完整循环(18 面板):
    /// Quest → Parliament → Budget → Memory → Security → Health → Log → Help
    /// → Decay → EventStream → Router → McpNodes → Chtc → Timeline
    /// → OsaSparse → ClvVector → ResourceMonitor → MetricsDashboard → Quest
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
            PanelId::Timeline => PanelId::OsaSparse,
            PanelId::OsaSparse => PanelId::ClvVector,
            PanelId::ClvVector => PanelId::ResourceMonitor,
            PanelId::ResourceMonitor => PanelId::MetricsDashboard,
            PanelId::MetricsDashboard => PanelId::Quest,
        }
    }

    /// 切换到上一个面板(循环顺序)
    pub fn prev(&self) -> PanelId {
        match self {
            PanelId::Quest => PanelId::MetricsDashboard,
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
            PanelId::OsaSparse => PanelId::Timeline,
            PanelId::ClvVector => PanelId::OsaSparse,
            PanelId::ResourceMonitor => PanelId::ClvVector,
            PanelId::MetricsDashboard => PanelId::ResourceMonitor,
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
// 布局模式 — 主区域 panel 排列方式(P6.2 布局模板)
// ============================================================

/// 布局模式 — 控制主区域的 panel 排列方式
///
/// WHY 三种布局:
/// - SinglePane:专注模式,当前面板全屏,适合深度查看单一面板(如 EventStream 万级事件)
/// - DualPane:对比模式,主面板 + 侧边栏,适合边查看边监控(默认布局)
/// - TriplePane:全监控模式,主面板 + 侧边栏 + 底部日志,适合多面板协同观察
///
/// WHY 派生 Serialize/Deserialize:`TuiState` 派生了 serde,作为其字段的
/// `LayoutMode` 必须同步派生,否则 `#[derive(Serialize)]` 缺少 trait bound 编译失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum LayoutMode {
    /// 单面板全屏(专注模式)
    SinglePane,
    /// 双面板:主面板 + 侧边栏(对比模式)
    ///
    /// WHY 默认值:用户首次启动 TUI 时应看到完整界面(tabs + main + status_bar),
    /// 知晓有 13 个面板可切换。SinglePane 是用户主动按 `l` 切换的专注模式,
    /// 不适合作为默认值 — 否则用户不知道有其他面板存在。
    #[default]
    DualPane,
    /// 三面板:主面板 + 侧边栏 + 底部日志(全监控模式)
    TriplePane,
}

impl LayoutMode {
    /// 返回布局模式的人类可读名称
    pub fn as_str(&self) -> &'static str {
        match self {
            LayoutMode::SinglePane => "single",
            LayoutMode::DualPane => "dual",
            LayoutMode::TriplePane => "triple",
        }
    }

    /// 循环切换到下一个布局模式(SinglePane → DualPane → TriplePane → SinglePane)
    ///
    /// WHY 循环顺序:从专注 → 对比 → 全监控 → 回到专注,符合用户逐步增加信息密度的需求
    pub fn next(&self) -> Self {
        match self {
            LayoutMode::SinglePane => LayoutMode::DualPane,
            LayoutMode::DualPane => LayoutMode::TriplePane,
            LayoutMode::TriplePane => LayoutMode::SinglePane,
        }
    }
}

// ============================================================
// 排序模式 — 任务管理面板的列表排序策略(Task 1.4)
// ============================================================

/// 排序模式 — TaskManagerPanel 的 Quest 列表排序策略
///
/// WHY 三种排序模式覆盖任务管理三大典型场景:
/// - `Priority`:运维关注 — 优先处理高优先级任务(默认,与 spec 一致)
/// - `Status`:状态管理 — 区分 Pending/Running/Paused/Completed 队列
/// - `CreatedAt`:时间追溯 — 最近任务在前,便于追溯新问题
///
/// WHY 派生 `Copy + Hash + Eq`:排序键需参与 HashMap 索引、Vec 排序、
/// `==` 比较;`Copy` 避免克隆开销(枚举小)。
///
/// WHY `#[default]`:与 `TuiConfig::task_manager_default_sort` 默认值契约一致
/// (spec §Requirement "任务管理面板" — 默认按优先级降序)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SortMode {
    /// 按优先级降序(默认,运维关注高优先级任务)
    #[default]
    Priority,
    /// 按状态分组(Pending → Running → Paused → Completed)
    Status,
    /// 按创建时间降序(最新任务在前)
    CreatedAt,
}

impl SortMode {
    /// 返回排序模式的人类可读名称(小写,用于配置显示)
    pub fn as_str(&self) -> &'static str {
        match self {
            SortMode::Priority => "priority",
            SortMode::Status => "status",
            SortMode::CreatedAt => "created_at",
        }
    }

    /// 循环切换到下一个排序模式(Priority → Status → CreatedAt → Priority)
    ///
    /// WHY 循环顺序:从运维关注(优先级)→ 状态管理 → 时间追溯 → 回到优先级,
    /// 符合运维人员逐步切换视角的需求。
    pub fn next(&self) -> Self {
        match self {
            SortMode::Priority => SortMode::Status,
            SortMode::Status => SortMode::CreatedAt,
            SortMode::CreatedAt => SortMode::Priority,
        }
    }
}

impl std::fmt::Display for SortMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================
// 高层命令 — 面板返回的语义化动作
// ============================================================

/// Quest 控制动作 — TaskManagerPanel 与 quest-engine 双向控制的动作枚举
///
/// WHY 独立 enum:`TuiCommand::RequestQuestPause` 等是面板直接发出的命令,
/// `QuestAction` 是为 TaskManagerPanel 设计的"控制动作"概念,可在不同面板间复用
/// (如未来 ParliamentPanel 审批后也通过 QuestControl 触发动作)。
///
/// `SetPriority(u8)` 使用 0-10 用户面范围(spec 明确),与既有
/// `RequestQuestPriorityChange { new_priority: u8 }` 的 0-255 内部范围不同。
/// 范围映射在 `TuiApp::apply_command` 中桥接。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestAction {
    /// 暂停 Quest
    Pause,
    /// 恢复 Quest
    Resume,
    /// 终止 Quest(破坏性操作)
    Terminate,
    /// 设置优先级,值域 [0, 10]
    SetPriority(u8),
}

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
    /// 请求取消指定 Quest(M4 双向控制扩展 — 破坏性操作,需二次确认)
    ///
    /// WHY 独立变体:cancel 是不可逆操作,`apply_command` 会弹出 Confirm 弹窗,
    /// 操作员确认后才通过 `apply_confirm_command` 发布 `QuestCancelRequested`。
    /// 与 pause/resume 一致走确认流程,避免误触导致任务丢失。
    RequestQuestCancel(String),
    /// 请求调整 Quest 优先级(M4 双向控制扩展 — 非破坏性操作,直接发布)
    ///
    /// WHY 直接发布:优先级调整可逆(+/- 互补),无需二次确认摩擦,
    /// `apply_command` 直接调用 `publish_priority_change` 发布事件。
    /// 边界检查(0/255)由面板在构造命令时完成,避免无效请求占用带宽。
    RequestQuestPriorityChange {
        /// 目标 Quest ID
        quest_id: String,
        /// 新优先级(0-255,边界检查由面板在构造命令时完成)
        new_priority: u8,
    },
    /// 请求对提案投票(M4 双向控制)
    RequestVote {
        /// 目标提案 ID
        proposal_id: String,
        /// 投票值
        vote: VoteValue,
    },
    /// 请求刷新状态(M4 双向控制)
    RequestRefresh,
    /// 设置 tick 间隔(毫秒,P4.3 可调 tick 暴露)
    ///
    /// 取值范围 [100, 1000](与 `TuiConfig::validate` 一致)。
    /// WHY 仅更新配置:`tokio::time::interval` 创建后不可修改周期,
    /// 运行中的 `DataPipeline` 无法安全重建,故本命令只更新
    /// `TuiConfig.tick_interval_ms`,在下次启动时生效。
    SetTickInterval(u16),
    /// 跳转到 EventStream 面板并按 quest_id 筛选事件(P5 跨面板联动)
    ///
    /// WHY 独立变体而非复用 `SwitchPanel`:Quest→EventStream 跳转需原子完成
    /// 两个操作 — (1) 设置 `filter_keyword` 筛选该 Quest 相关事件,
    /// (2) 切换到 EventStream 面板。若用 `SwitchPanel` 则面板无法表达
    /// "设置 filter"的意图,且 filter 设置与面板切换应作为原子操作由
    /// `apply_command` 统一执行,避免 filter 设置后面板切换失败导致状态不一致。
    JumpToEventStream {
        /// 目标 Quest ID,作为 EventStream 的筛选关键字
        quest_id: String,
    },
    /// Quest 控制命令(TaskManagerPanel,M3-2)
    ///
    /// WHY 独立变体:TaskManagerPanel 提供完整的 Quest CRUD 控制(P/T/↑/↓/Enter),
    /// 既有 `RequestQuestPause`/`RequestQuestResume`/`RequestQuestCancel` 三个
    /// 独立变体无法表达完整动作空间(缺少 `Terminate` 与 `SetPriority` 抽象)。
    /// 统一的 `QuestControl { id, action }` 形式便于:
    /// - 未来新增动作(如 `Clone`/`Archive`)只需扩展 `QuestAction` enum
    /// - 跨面板复用 ParliamentPanel 审批后也能触发同一动作空间
    /// - 测试与文档按"动作"而非"按键"组织
    ///
    /// 桥接:`TuiApp::apply_command` 将 `QuestAction` 映射到既有确认弹窗
    /// (Pause/Resume/Terminate)或直接发布优先级变更(SetPriority)。
    QuestControl {
        /// 目标 Quest ID
        id: String,
        /// 控制动作
        action: QuestAction,
    },
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

/// Timeline 面板的历史快照 — 周期性记录系统关键指标
///
/// 由 DataPipeline 按 snapshot_interval_s 周期生成,
/// 容量上限 max_snapshots(默认 100),FIFO 丢弃最旧快照。
///
/// WHY 含 f32 字段(budget_utilization/decay_coefficient):仅派生 PartialEq,
/// 不派生 Eq(项目红线:浮点字段不满足 Eq)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineSnapshot {
    /// 快照时间戳
    pub timestamp: DateTime<Utc>,
    /// 事件总数(累计)
    pub event_count: u64,
    /// 事件速率(每秒事件数,自上一快照以来)
    pub event_rate: u64,
    /// 预算利用率 [0.0, 1.0]
    pub budget_utilization: f32,
    /// 健康分 [0, 100]
    pub health_score: u8,
    /// 衰减系数 [0.0, 1.0]
    pub decay_coefficient: f32,
}

impl Default for TimelineSnapshot {
    fn default() -> Self {
        // WHY health_score=100 / decay_coefficient=1.0:与 DecayMetrics::default 保持一致,
        // 无数据时显示"满血"状态,避免面板误导性低分。
        Self {
            timestamp: Utc::now(),
            event_count: 0,
            event_rate: 0,
            budget_utilization: 0.0,
            health_score: 100,
            decay_coefficient: 1.0,
        }
    }
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
    /// 暂停 Quest 数(从 QuestPaused/QuestResumed 事件派生,数据驱动 Health 面板)
    pub paused_quest_count: usize,
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
    // === P6.2 布局模板新增字段 ===
    /// 当前布局模式(P6.2 布局模板)
    ///
    /// WHY 默认 DualPane:启动时显示完整界面(tabs + main + status_bar),
    /// 用户按 `l` 可切换到 TriplePane(全监控)或 SinglePane(专注模式)。
    pub layout_mode: LayoutMode,
    // === P7 OsaSparse / ClvVector 面板新增字段 ===
    /// OSA 平均稀疏度 [0.0, 1.0](None = 未收到事件)
    pub osa_sparsity: Option<f32>,
    /// OSA context 维度活跃文件 ID 列表
    pub osa_context_mask: Vec<String>,
    /// OSA 稀疏度历史(容量 256,FIFO)
    pub osa_sparsity_history: Vec<u64>,
    /// CLV 摘要(None = 未收到事件)
    pub clv_summary: Option<event_bus::ClvSummary>,
    // === P8 ResourceMonitor 面板新增字段 ===
    /// 系统资源指标(数据驱动 ResourceMonitor / Health 面板)
    pub sys_metrics: SystemMetrics,
    /// 系统资源指标历史(sparkline: CPU 使用率 × 10 的 u64 表示)
    pub sys_metrics_history: Vec<u64>,
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
            paused_quest_count: 0,
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
            // P6.2 布局模板默认值(DualPane,见 LayoutMode::default 的 WHY 注释)
            layout_mode: LayoutMode::default(),
            // P7 OsaSparse / ClvVector 面板默认值(未收到事件时为 None / 空)
            osa_sparsity: None,
            osa_context_mask: Vec::new(),
            osa_sparsity_history: Vec::new(),
            clv_summary: None,
            // P8 ResourceMonitor 面板默认值
            sys_metrics: SystemMetrics::default(),
            sys_metrics_history: Vec::new(),
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

    // ============================================================
    // P4.1 增量渲染 — dirty_panels 标记 API
    // ============================================================
    //
    // WHY 采用"数据驱动"标记策略:仅当某个面板绑定的数据字段在本帧发生
    // 变化时才将其加入 `dirty_panels`,避免"每帧全量重建 Text/Span"的浪费。
    // 由于 ratatui 的 Frame 每帧都会用空白缓冲区覆盖前帧内容,面板渲染
    // 本身仍必须每帧执行(否则该面板区域会被清空)。本标记的实际用途:
    // 1) 为面板内部提供缓存失效信号(数据未变时可复用上次构建的 Text/Span);
    // 2) 为后续 P4.2/P6.1 等性能优化提供统一的数据变化检测入口;
    // 3) 为测试提供可观测的"哪些面板本次更新过数据"。

    /// 标记指定面板为 dirty(数据已变化,需要刷新内部缓存)
    pub fn mark_dirty(&mut self, panel: PanelId) {
        self.dirty_panels.insert(panel);
    }

    /// 判断指定面板是否被标记为 dirty
    pub fn is_dirty(&self, panel: PanelId) -> bool {
        self.dirty_panels.contains(&panel)
    }

    /// 取出当前 dirty 面板集合并清空(消费语义)
    ///
    /// WHY take 而非借用:渲染结束时调用,既提供可见性又确保下一帧
    /// 从空集合开始,避免历史脏标记残留影响下一轮的判断。
    pub fn take_dirty(&mut self) -> HashSet<PanelId> {
        std::mem::take(&mut self.dirty_panels)
    }

    /// 清空 dirty 面板集合
    ///
    /// WHY 与 `take_dirty` 并存:调用方不关心集合内容、只想"重置"时
    /// 使用此方法,语义更直观,且不会触发 HashSet 的移动分配。
    pub fn clear_dirty(&mut self) {
        self.dirty_panels.clear();
    }
}

impl Default for TuiState {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// P8 系统资源指标类型 — CPU/内存/磁盘/网络聚合视图
// ============================================================
//
// 由 SysMetricsCollector 采集，供 ResourceMonitor 和 Health 面板使用。

/// 系统资源指标 — CPU/内存/磁盘/网络的聚合视图
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SystemMetrics {
    /// CPU 指标
    pub cpu: CpuMetrics,
    /// 内存指标
    pub memory: MemMetrics,
    /// 磁盘指标
    pub disk: DiskMetrics,
    /// 网络指标
    pub network: NetworkMetrics,
}

/// CPU 指标 — 全局使用率与每核使用率
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CpuMetrics {
    /// 全局 CPU 使用率百分比 [0.0, 100.0]
    pub global_usage: f32,
    /// 各核 CPU 使用率百分比
    pub per_core_usage: Vec<f32>,
    /// 逻辑核心数
    pub core_count: usize,
}

/// 内存指标 — 物理内存与交换空间
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MemMetrics {
    /// 总物理内存(字节)
    pub total_bytes: u64,
    /// 已用物理内存(字节)
    pub used_bytes: u64,
    /// 可用物理内存(字节)
    pub available_bytes: u64,
    /// 内存使用率百分比 [0.0, 100.0]
    pub usage_percent: f32,
    /// 交换空间总大小(字节)
    pub swap_total_bytes: u64,
    /// 交换空间已用(字节)
    pub swap_used_bytes: u64,
}

/// 磁盘指标 — 读写速率
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DiskMetrics {
    /// 磁盘读取速率(字节/秒，基于两次采集差值的瞬时估算)
    pub read_bytes_per_sec: u64,
    /// 磁盘写入速率(字节/秒)
    pub write_bytes_per_sec: u64,
    /// 累计读取字节
    pub total_read_bytes: u64,
    /// 累计写入字节
    pub total_write_bytes: u64,
}

/// 网络指标 — 接收/发送速率
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct NetworkMetrics {
    /// 接收速率(字节/秒)
    pub rx_bytes_per_sec: u64,
    /// 发送速率(字节/秒)
    pub tx_bytes_per_sec: u64,
    /// 累计接收字节
    pub total_rx_bytes: u64,
    /// 累计发送字节
    pub total_tx_bytes: u64,
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
        // P3 扩展:Timeline → OsaSparse(不再是 Timeline → Quest)
        assert_eq!(PanelId::Timeline.next(), PanelId::OsaSparse);
        assert_eq!(PanelId::OsaSparse.next(), PanelId::ClvVector);
        // 循环:ClvVector → ResourceMonitor
        assert_eq!(PanelId::ClvVector.next(), PanelId::ResourceMonitor);
        // 循环:ResourceMonitor → MetricsDashboard
        assert_eq!(PanelId::ResourceMonitor.next(), PanelId::MetricsDashboard);
        // 循环:MetricsDashboard → Quest(Task 2.2 新增)
        assert_eq!(PanelId::MetricsDashboard.next(), PanelId::Quest);
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
        // P3 扩展:OsaSparse → Timeline,ClvVector → OsaSparse
        assert_eq!(PanelId::OsaSparse.prev(), PanelId::Timeline);
        assert_eq!(PanelId::ClvVector.prev(), PanelId::OsaSparse);
        assert_eq!(PanelId::ResourceMonitor.prev(), PanelId::ClvVector);
        // Task 2.2:MetricsDashboard → ResourceMonitor
        assert_eq!(PanelId::MetricsDashboard.prev(), PanelId::ResourceMonitor);
        // 循环:Quest → MetricsDashboard(不再是 Quest → ResourceMonitor)
        assert_eq!(PanelId::Quest.prev(), PanelId::MetricsDashboard);
    }

    #[test]
    fn test_panel_id_next_prev_roundtrip() {
        // next 再 prev 应回到原面板(P8 扩展至 18 面板)
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
            PanelId::OsaSparse,
            PanelId::ClvVector,
            PanelId::ResourceMonitor,
            PanelId::MetricsDashboard,
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
    fn test_panel_id_osa_sparse() {
        let p = PanelId::OsaSparse;
        assert_eq!(p.as_str(), "OsaSparse");
        assert_eq!(p.title(), " OSA Sparse ");
        // 验证循环:OsaSparse 的下一个是 ClvVector,前一个是 Timeline
        assert_eq!(p.next(), PanelId::ClvVector);
        assert_eq!(p.prev(), PanelId::Timeline);
    }

    #[test]
    fn test_panel_id_clv_vector() {
        let p = PanelId::ClvVector;
        assert_eq!(p.as_str(), "ClvVector");
        assert_eq!(p.title(), " CLV Vector ");
        // 验证循环:ClvVector 的下一个是 ResourceMonitor,前一个是 OsaSparse
        assert_eq!(p.next(), PanelId::ResourceMonitor);
        assert_eq!(p.prev(), PanelId::OsaSparse);
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
        let mut state = TuiState::new();
        // 设置 P7 新字段非默认值,验证 OSA/CLV 字段序列化往返一致
        state.osa_sparsity = Some(0.45);
        state.osa_context_mask = vec!["file1.rs".into(), "file2.rs".into()];
        state.osa_sparsity_history = vec![100, 200];
        state.clv_summary = Some(event_bus::ClvSummary {
            block_means: vec![0.1; 8],
            l2_norm: 2.5,
            top_dims: vec![(0, 0.8)],
        });
        let json = serde_json::to_string(&state).unwrap();
        let restored: TuiState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, state);
    }

    // ============================================================
    // LayoutMode 测试(P6.2 布局模板)
    // ============================================================

    #[test]
    fn test_layout_mode_default() {
        assert_eq!(LayoutMode::default(), LayoutMode::DualPane);
    }

    #[test]
    fn test_layout_mode_as_str() {
        assert_eq!(LayoutMode::SinglePane.as_str(), "single");
        assert_eq!(LayoutMode::DualPane.as_str(), "dual");
        assert_eq!(LayoutMode::TriplePane.as_str(), "triple");
    }

    #[test]
    fn test_layout_mode_next_cycle() {
        // SinglePane → DualPane → TriplePane → SinglePane
        assert_eq!(LayoutMode::SinglePane.next(), LayoutMode::DualPane);
        assert_eq!(LayoutMode::DualPane.next(), LayoutMode::TriplePane);
        assert_eq!(LayoutMode::TriplePane.next(), LayoutMode::SinglePane);
        // 完整循环验证:连续 next 三次回到起点
        let mode = LayoutMode::SinglePane;
        assert_eq!(mode.next().next().next(), mode);
    }

    #[test]
    fn test_tui_state_layout_mode_default() {
        let state = TuiState::new();
        assert_eq!(state.layout_mode, LayoutMode::DualPane);
    }

    // ============================================================
    // P7 TimelineSnapshot / OsaSparse / ClvVector 测试
    // ============================================================

    #[test]
    fn test_timeline_snapshot_default() {
        let snap = TimelineSnapshot::default();
        assert_eq!(snap.event_count, 0);
        assert_eq!(snap.event_rate, 0);
        assert_eq!(snap.health_score, 100);
        assert_eq!(snap.decay_coefficient, 1.0);
    }

    #[test]
    fn test_tui_state_new_has_osa_clv_fields() {
        let state = TuiState::new();
        assert!(state.osa_sparsity.is_none());
        assert!(state.osa_context_mask.is_empty());
        assert!(state.osa_sparsity_history.is_empty());
        assert!(state.clv_summary.is_none());
    }

    #[test]
    fn test_tui_state_osa_sparsity_update() {
        let mut state = TuiState::new();
        state.osa_sparsity = Some(0.45);
        state.osa_context_mask = vec!["file1.rs".into(), "file2.rs".into()];
        state.osa_sparsity_history.push(100);
        assert_eq!(state.osa_sparsity, Some(0.45));
        assert_eq!(state.osa_context_mask.len(), 2);
        assert_eq!(state.osa_sparsity_history.len(), 1);
    }

    #[test]
    fn test_tui_state_clv_summary_update() {
        let mut state = TuiState::new();
        let summary = event_bus::ClvSummary {
            block_means: vec![0.1; 8],
            l2_norm: 2.5,
            top_dims: vec![(0, 0.8)],
        };
        state.clv_summary = Some(summary);
        assert!(state.clv_summary.is_some());
        let s = state.clv_summary.as_ref().unwrap();
        assert_eq!(s.block_means.len(), 8);
        assert!((s.l2_norm - 2.5).abs() < 1e-5);
    }
}
