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
use crate::engine::layout::PaneMode;
use crate::error::TuiError;
use crate::popup::{PopupStack, Severity};
use chrono::{DateTime, Utc};
use event_bus::{ActionSource, ChatStatus, NexusEvent, VoteValue};
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
    /// 系统信息面板 — 主机与 Chimera 进程只读视图(v1.8-omega Task 3.1)
    ///
    /// 展示 OS / CPU 型号 / 核心数 / 总内存 / 启动时间 / Chimera 进程 PID + RSS。
    /// 主机信息仅构造时采集,进程信息按 `TuiConfig.sysinfo_refresh_interval_ms`
    /// 周期刷新(默认 5s)。数据源:`sysinfo` 0.32 crate(纯 Rust 跨平台)。
    Sysinfo,
    /// 对话面板 — 交互式 Agent 对话(M3b):渲染对话历史 + 会话状态指示器。
    ///
    /// 输入经全局 Insert 模式(`i` 进入)在底部输入行完成,Enter 提交发布
    /// `TuiChatSubmitted`;响应经 `ChatSync` 消费 `TuiChatResponseChunk`/
    /// `TuiChatCompleted`/`TuiChatStatusChanged` 驱动。
    Chat,
    /// 自评仪表盘面板 — 五维度 Harness 自我评估(polish-v2.7 P1-5,ADR-049)
    ///
    /// 展示 RuntimeAuditor 发布的 `HarnessReportGenerated` 五维评分
    /// (任务理解/可控执行/变更验证/可靠交付/经验沉淀)
    /// 与最近 `AuditFindingRaised` 审计发现列表,数据从 `latest_events` 派生。
    SelfAssessment,

    /// DAG 可视化面板(polish-v2.7 closure Stage B-10,北大 DataFlow)
    ///
    /// 展示 `quest_list` 中各 Quest 的任务 DAG 层级树
    /// (依赖深度缩进 + 状态标记 + 依赖边标注),数据零管道侵入。
    DagViz,
    /// PVL 过程评分面板（Task 3.7:L10 → L7 向下依赖）
    ///
    /// 展示 PVL 九维度过程评分（快手 KAT,ADR-049）：
    /// 真实执行/覆盖率/验证通过/置信度/效率/重试纪律/产出实质性/零孤儿/沙箱清洁。
    /// 数据来源：`pvl_layer::pvl_score()`。
    PvlScore,
    /// 任务管理面板（Task 3.9:L10 → L9 向下依赖）
    ///
    /// 展示 Quest CRUD 控制台 + 四象限稳定分工（ADR-027）状态。
    /// 数据来源：`chimera_mas::quadrant_status()`。
    TaskManager,
    /// 超窗兜底面板 — 展示 OverWindowFallbackTriggered 触发记录(P1,ADR-072)
    ///
    /// 数据来源：`latest_events` 过滤超窗触发事件（零管道侵入，同 SelfAssessment/DagViz）。
    OverWindow,
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
            PanelId::Sysinfo => "Sysinfo",
            PanelId::Chat => "Chat",
            PanelId::SelfAssessment => "SelfAssessment",
            PanelId::DagViz => "DagViz",
            PanelId::PvlScore => "PvlScore",
            PanelId::TaskManager => "TaskManager",
            PanelId::OverWindow => "OverWindow",
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
            PanelId::Sysinfo => " System Info ",
            PanelId::Chat => " Chat ",
            PanelId::SelfAssessment => " Self Assessment ",
            PanelId::DagViz => " DAG Viz ",
            PanelId::PvlScore => " PVL Score ",
            PanelId::TaskManager => " Task Manager ",
            PanelId::OverWindow => " OverWindow ",
        }
    }

    /// 已注册面板的焦点环顺序 — 焦点遍历的单一事实源(Concord T1.4,P5① 收口)
    ///
    /// WHY 单一事实源:`PanelId::next/prev` 与 `TuiApp` 面板注册序此前双源
    /// 维护,新增/下线面板漏改任一处即静默漂移(INV-F 实锤:静态环曾含
    /// 未注册面板且相对顺序颠倒)。现规定:本表是唯一声明点,
    /// `next/prev` 派生自本表,`TuiApp` 注册序也派生自本表。
    /// 未列入的 PanelId 变体 = 未注册面板(不参与焦点环)。
    pub const REGISTERED_FOCUS_ORDER: &[PanelId] = &[
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
        // Concord T1.4:Timeline 接线注册(§7.3 未注册双面板接线清单)
        PanelId::Timeline,
        PanelId::OsaSparse,
        PanelId::ClvVector,
        PanelId::ResourceMonitor,
        PanelId::MetricsDashboard,
        // Concord T1.4:Sysinfo 接线注册(数据与 ResourceMonitor 互补)
        PanelId::Sysinfo,
        PanelId::Chat,
        PanelId::SelfAssessment,
        PanelId::DagViz,
        PanelId::PvlScore,
        PanelId::TaskManager,
        PanelId::OverWindow,
    ];

    /// 切换到下一个面板(循环顺序,派生自 `REGISTERED_FOCUS_ORDER`)
    ///
    /// 未注册变体(不在焦点环内)回退到环首面板,避免孤立分支。
    pub fn next(&self) -> PanelId {
        let order = Self::REGISTERED_FOCUS_ORDER;
        match order.iter().position(|p| p == self) {
            Some(i) => order[(i + 1) % order.len()],
            None => order[0],
        }
    }

    /// 切换到上一个面板(循环顺序,派生自 `REGISTERED_FOCUS_ORDER`)
    pub fn prev(&self) -> PanelId {
        let order = Self::REGISTERED_FOCUS_ORDER;
        match order.iter().position(|p| p == self) {
            Some(i) => order[(i + order.len() - 1) % order.len()],
            None => order[order.len() - 1],
        }
    }
}

impl std::fmt::Display for PanelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================
// 对话消息 — Chat 面板历史条目(M3b)
// ============================================================

/// 对话消息角色 — 区分用户输入与 Agent 回答(M3b)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatRole {
    /// 用户提交的查询
    User,
    /// Agent 流式回答
    Assistant,
}

/// 对话消息 — Chat 面板的单条历史(M3b)
///
/// WHY 仅 role + content:M3b 只需区分角色与文本;时间戳/工具调用摘要等
/// 留待 M3c 编排器接入后按需扩展(YAGNI)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// 消息角色(用户/Agent)
    pub role: ChatRole,
    /// 消息文本内容(Assistant 流式追加)
    pub content: String,
}

// ============================================================
// 输入模式 — 命令面板/搜索面板/普通模式
// ============================================================

/// 输入模式 — 控制底部输入栏的行为
///
/// - `Normal`:普通模式,底部显示状态栏
/// - `Command`:命令模式(由 `:` 触发),解析并执行面板切换/过滤/投票等带参命令
/// - `Search`:搜索模式(由 `/` 触发),关键字过滤
/// - `Insert`:插入模式(由 `i` 触发,M3a),原始文本输入(为 M3b Chat 提交铺路)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InputMode {
    /// 普通模式
    Normal,
    /// 命令模式
    Command,
    /// 搜索模式
    Search,
    /// 插入模式(原始文本输入,M3a 引入;Submit 于 M3b 接入 Chat)
    Insert,
    /// 斜杠命令模式(Concord W2):`/` 第一公民入口,补全列表 + 三分层执行;
    /// `:` 废弃窗口期同进本模式(一次性弃用提示)。
    Slash,
}

/// 视图模式(Concord W3 T3.2):会话优先的双模式分层(ADR-076)
///
/// - `Chat`:第一默认——会话流全屏 + composer 底栏(Conversation-First)
/// - `Dashboard`:第二默认——既有 25 面板驾驶舱(资产下沉不推倒)
///
/// `\` 键或 `/chat` `/dashboard` 命令互切;状态随 TuiState 持久化。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ViewMode {
    /// 会话模式(第一默认,ADR-076 裁定)
    #[default]
    Chat,
    /// 仪表盘模式(25 面板驾驶舱,资产下沉)
    Dashboard,
}

// ============================================================
// 布局模式 — 主区域 panel 排列方式(P6.2 布局模板)
// ============================================================

/// 布局模式 — 控制主区域的 panel 排列方式
///
/// WHY 四种布局:
/// - SinglePane:专注模式,当前面板全屏,适合深度查看单一面板(如 EventStream 万级事件)
/// - DualPane:对比模式,主面板 + 侧边栏,适合边查看边监控(默认布局)
/// - TriplePane:全监控模式,主面板 + 侧边栏 + 底部日志,适合多面板协同观察
/// - VimSplit:分屏模式,左右两等分窗格,适合并排对照(M3d Vim 风格多窗格)
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
    /// 分屏:左右两等分窗格(M3d,Vim 风格双分屏)
    ///
    /// WHY 第 4 变体:M3d 多窗格将 VimSplit 纳入 `l` 键循环,映射到
    /// `PaneMode::VimSplit`(左右等分);serde 附加变体,旧配置正常加载。
    VimSplit,
}

impl LayoutMode {
    /// 返回布局模式的人类可读名称
    pub fn as_str(&self) -> &'static str {
        match self {
            LayoutMode::SinglePane => "single",
            LayoutMode::DualPane => "dual",
            LayoutMode::TriplePane => "triple",
            LayoutMode::VimSplit => "vim",
        }
    }

    /// 循环切换到下一个布局模式(Single → Dual → Triple → VimSplit → Single)
    ///
    /// WHY 循环顺序:专注 → 对比 → 全监控 → 分屏 → 回到专注,符合用户逐步增加
    /// 信息密度、最后进入 Vim 风格分屏协作的需求(M3d 将 VimSplit 纳入循环)
    pub fn next(&self) -> Self {
        match self {
            LayoutMode::SinglePane => LayoutMode::DualPane,
            LayoutMode::DualPane => LayoutMode::TriplePane,
            LayoutMode::TriplePane => LayoutMode::VimSplit,
            LayoutMode::VimSplit => LayoutMode::SinglePane,
        }
    }

    /// 将旧布局模式别名映射到 v3 `PaneMode`(M2 增量3,别名共存)
    ///
    /// WHY 别名而非替换:`LayoutMode` 保留为存储与 `l` 键循环驱动,`PaneMode` 作为
    /// 展示层派生——伴随面板等多窗格特性据此判断是否有 context 区。
    /// - `SinglePane => Focus`(全屏,无 context)
    /// - `DualPane => Chat`(主区 + 单一 context)
    /// - `TriplePane => Ide`(主区 + 侧栏 + context,M3d 三窗格)
    /// - `VimSplit => VimSplit`(左右等分,M3d 双分屏)
    pub fn to_pane_mode(self) -> PaneMode {
        match self {
            LayoutMode::SinglePane => PaneMode::Focus,
            LayoutMode::DualPane => PaneMode::Chat,
            LayoutMode::TriplePane => PaneMode::Ide,
            LayoutMode::VimSplit => PaneMode::VimSplit,
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
    /// 导出当前面板数据
    Export,
    /// 派发一个 Action(v3.1 三入口统一派发点,ADR-029)
    ///
    /// WHY 统一派发:Chat 斜杠命令 / 命令面板 / 面板上下文动作三入口最终都
    /// 归结为"派发某个 action_id + payload",由 `apply_command` 统一发布
    /// `NexusEvent::TuiActionRequested`,经 EventBus 交 chimera-cli 编排。
    /// `source` 标识触发入口,用于审计与 UI 反馈定位。既有 Quest 控制变体
    /// (RequestQuestPause 等)将在 M3 逐步迁移为本变体调用,当前并存兼容。
    DispatchAction {
        /// 动作标识(须存在于 ActionRegistry,如 "quest.pause")
        action_id: String,
        /// 动作参数(JSON 编码;无参数为 "{}")
        payload: String,
        /// 触发入口(Chat/Palette/Panel)
        source: ActionSource,
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
///
/// # P2-11 扩展(2026-07-28)
///
/// 新增 `fallback_count_delta` 字段,反映本周期 `DecayLearnerHolder`
/// 触发 fallback 的次数。TUI 可据此显示 learner 健康度告警。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecayMetrics {
    /// 当前衰减系数 [0.0, 1.0],1.0 表示无衰减
    pub coefficient: f32,
    /// 本周期内触发衰减的最近事件摘要
    pub recent_events: Vec<String>,
    /// 本衰减周期开始时间,None 表示尚未收到任何衰减事件
    pub cycle_start: Option<DateTime<Utc>>,
    /// P2-11: 本周期 fallback 触发次数(异常回退层 + 熔断入口层)
    ///
    /// 持续 > 0 表明 learner 不稳定,TUI 可显示告警提示运维介入。
    #[serde(default)]
    pub fallback_count_delta: u64,
}

impl Default for DecayMetrics {
    fn default() -> Self {
        // WHY coefficient=1.0:无衰减事件时默认满血,避免面板显示误导性低系数
        Self {
            coefficient: 1.0,
            recent_events: Vec::new(),
            cycle_start: None,
            fallback_count_delta: 0,
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
// TickMode — DataPipeline 低带宽自适应 tick 模式
// ============================================================

/// DataPipeline tick 模式 — 用于低带宽自适应
///
/// WHY 自适应 tick:当事件积压量超过阈值时自动从 Normal(250ms)切换到
/// Eco(1000ms),降低 CPU 占用与渲染压力。积压量回落到阈值一半以下
/// 且连续 3 tick 稳定后,自动切回 Normal 模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TickMode {
    /// 正常模式(250ms tick)
    #[default]
    Normal,
    /// 节能模式(1000ms tick),高负载时自动切换
    Eco,
}

impl TickMode {
    /// 返回状态栏展示文本
    pub fn display(&self) -> &'static str {
        match self {
            TickMode::Normal => "Normal",
            TickMode::Eco => "Eco",
        }
    }
}

/// 监控面板 sparkline 显示时间窗 — `monitor.time_window` 循环切换
///
/// WHY 三档:16/32/64 点对应"近况/中期/全窗"三种时间跨度,循环切换让
/// 操作员在窄 sparkline 上按需聚焦最近趋势或查看完整历史
///(数据源 `sys_metrics_history` 上限 64)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MonitorWindow {
    /// 近况:最近 16 点
    Short,
    /// 中期:最近 32 点
    Medium,
    /// 全窗:最近 64 点(默认,= sys_metrics_history 上限,零回归)
    #[default]
    Long,
}

impl MonitorWindow {
    /// 返回显示点数
    pub fn points(&self) -> usize {
        match self {
            MonitorWindow::Short => 16,
            MonitorWindow::Medium => 32,
            MonitorWindow::Long => 64,
        }
    }

    /// 循环到下一档(Short → Medium → Long → Short)
    pub fn next(&self) -> Self {
        match self {
            MonitorWindow::Short => MonitorWindow::Medium,
            MonitorWindow::Medium => MonitorWindow::Long,
            MonitorWindow::Long => MonitorWindow::Short,
        }
    }

    /// 返回状态栏展示标签(点数)
    pub fn label(&self) -> &'static str {
        match self {
            MonitorWindow::Short => "16",
            MonitorWindow::Medium => "32",
            MonitorWindow::Long => "64",
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
    /// 当前视图模式(Concord W3:Chat 第一默认 / Dashboard 驾驶舱下沉)
    ///
    /// `#[serde(default)]`:旧状态文件无本字段时反序列化得 Chat 默认。
    #[serde(default)]
    pub view_mode: ViewMode,
    /// 当前审批模式(Concord W4 T4.1:Normal/Plan/Auto 三态,ADR-074)
    ///
    /// `#[serde(default)]`:旧状态文件无本字段时反序列化得 Normal 默认。
    #[serde(default)]
    pub approval_mode: crate::approval_mode::ApprovalMode,
    /// composer 输入历史(Concord W6 T6.2:↑↓ 回溯;队尾最新,容量 100)
    #[serde(default)]
    pub input_history: std::collections::VecDeque<String>,
    /// 历史导航位置(Concord W6 T6.2;None = 未处于回溯态)
    #[serde(default)]
    pub history_pos: Option<usize>,
    /// 进入回溯前的草稿(回底时恢复,Concord W6 T6.2)
    #[serde(default)]
    pub history_draft: String,
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
    /// 预算指标是否陈旧(Concord T1.7,从快照同步;Budget 面板置灰依据)
    #[serde(default)]
    pub budget_metrics_stale: bool,
    /// 斜杠补全列表当前选中项索引(Concord W2;每次输入变化时钳制复位)
    #[serde(default)]
    pub slash_selected: usize,
    /// `:` 弃用提示是否已展示过(Concord W2,一次性提示;R1 缓解)
    #[serde(default)]
    pub colon_deprecation_shown: bool,
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
    /// Ctrl+W 前缀状态(M3 后续):按下 `Ctrl+W` 后进入等待,下一键(h/j/k/l/w)
    /// 决定方向窗格焦点或循环;其他键取消前缀。与 `g_prefix` 同为瞬态路由前缀。
    pub w_prefix: bool,
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
    // === PROBE P0.4:HCW 召回读数(由 HcwRecallReported 事件同步,None = 未收到报告) ===
    /// 多针召回率 needle_recall@8 ∈ [0,1]
    #[serde(default)]
    pub recall_needle_at_8: Option<f32>,
    /// 位置偏置比 ∈ [0,1]
    #[serde(default)]
    pub recall_position_bias: Option<f32>,
    /// 链路成功率 ∈ [0,1]
    #[serde(default)]
    pub recall_chain_success: Option<f32>,
    // === P8 ResourceMonitor 面板新增字段 ===
    /// 系统资源指标(数据驱动 ResourceMonitor / Health 面板)
    pub sys_metrics: SystemMetrics,
    /// 系统资源指标历史(sparkline: CPU 使用率 × 10 的 u64 表示)
    pub sys_metrics_history: Vec<u64>,
    /// 当前 tick 模式(Normal/Eco),状态栏展示用
    #[serde(default)]
    pub tick_mode: TickMode,
    // === M3b Chat 面板字段 ===
    /// 对话历史(数据驱动 Chat 面板,由 ChatSync 单一拥有并经 DataSnapshot 同步)
    #[serde(default)]
    pub chat_messages: Vec<ChatMessage>,
    /// 当前对话会话状态(思考中/工具执行中/空闲,驱动 Chat 状态指示器)
    #[serde(default)]
    pub chat_status: ChatStatus,
    /// 已上屏的 Action 反馈序号(P0 交互链;与 DataSnapshot.action_feedback_seq 比对,
    /// 仅当快照序号更大时才把 Action 结果/错误刷到 status_message,避免每 tick 重复)
    #[serde(default)]
    pub last_action_feedback_seq: u64,
    /// 监控采样是否暂停(monitor.pause_sampling;UI 本地冻结显示)
    ///
    /// WHY UI 本地:暂停时 `app.update()` 跳过覆盖 `sys_metrics`/`sys_metrics_history`,
    /// 保留冻结快照供检视;采样管线继续运行(廉价),不改 pipeline/event-bus(§2.2)。
    #[serde(default)]
    pub monitor_paused: bool,
    /// 监控 sparkline 显示时间窗(monitor.time_window 循环切换)
    #[serde(default)]
    pub monitor_window: MonitorWindow,
    /// CLV 热图值域是否自适应(viz.switch_dimension 焦点为 ClvVector 时)
    ///
    /// false = 固定[-1,1](默认);true = 按 block_means 实际 min/max 自适应着色。
    #[serde(default)]
    pub clv_heatmap_autoscale: bool,
    // === P1-W2.2 Critical 旁路通道丢弃计数 ===
    /// Critical 旁路通道(mpsc 4096)累计丢弃事件数(0 = 无丢弃,> 0 触发告警)
    ///
    /// 来源:DataSnapshot.critical_event_dropped_count → app.update() 同步。
    /// EventStream 面板顶部据此显示 "CRITICAL 事件丢弃: N" 告警行。
    #[serde(default)]
    pub critical_event_dropped_count: u64,
    // === P2 性能(P-1/P-3):快照 revision 追踪 ===
    /// 最近一次已同步的 `DataSnapshot.revision`(0 = 尚未同步 / 测试桩)
    ///
    /// WHY 性能:事件循环轮询(100ms)快于数据 tick(250ms),revision 未变时
    /// `TuiApp::update` 跳过整包字段拷贝,EventStream 过滤缓存也以它为失效键。
    /// 不持久化(serde skip):运行时派生值,重启后从 0 重新开始。
    #[serde(skip, default)]
    pub last_snapshot_revision: u64,
    /// Palette 参数输入流待派发动作(F-5)
    ///
    /// palette 选中需 query 的动作(agent.chat / quest.start / overwindow.run)时
    /// 置位并进入 Insert 参数收集态;提交以 `{"query": text}` 派发后清除,Esc 取消。
    /// 会话瞬态,不持久化(serde skip)。
    #[serde(skip, default)]
    pub pending_action: Option<PendingAction>,
    // === P1-2(评估报告 v2):TuiActionRequested 本地兜底超时 ===
    /// 已派发待确认动作的截止时刻(编排器未接线场景的本地兜底)
    ///
    /// WHY:dispatch_action 兜底发布 `TuiActionRequested` 时记录
    /// `now + ACTION_TIMEOUT`;若无消费者(standalone 模式)且超时无
    /// `TuiActionCompleted/Failed` 回发,`TuiApp::update` 中的超时检测
    /// 在状态栏提示“编排器未接线”。收到终态反馈时清除,避免误报。
    /// 会话瞬态,不持久化(serde skip)。
    #[serde(skip, default)]
    pub pending_action_deadline: Option<std::time::Instant>,
}

/// Palette 参数输入流待派发动作(F-5)
///
/// 携带动作 id 与触发入口,供 Insert 提交时构造 `{"query": text}` payload 并经
/// `dispatch_action` 统一派发(三入口一致性;`source` 用于审计与反馈定位)。
#[derive(Debug, Clone, PartialEq)]
pub struct PendingAction {
    /// 动作 id(须存在于 ActionRegistry,如 "agent.chat")
    pub action_id: String,
    /// 触发入口(palette 参数流固定为 Palette)
    pub source: event_bus::ActionSource,
}

impl TuiState {
    /// 创建新的初始状态(默认 Quest 面板,运行中)
    pub fn new() -> Self {
        Self {
            running: true,
            input_mode: InputMode::Normal,
            // Concord W3 T3.2:会话模式为第一默认(ADR-076)
            view_mode: ViewMode::Chat,
            // Concord W4 T4.1:审批模式默认 Normal(执行前确认)
            approval_mode: crate::approval_mode::ApprovalMode::Normal,
            // Concord W6 T6.2:composer 历史初始为空,导航未激活
            input_history: std::collections::VecDeque::new(),
            history_pos: None,
            history_draft: String::new(),
            input_buffer: String::new(),
            frame_count: 0,
            quest_list: Vec::new(),
            paused_quest_count: 0,
            budget: BudgetMetrics::default(),
            // Concord T1.7:初始无数据源接入时预算指标视为陈旧(诚实展示)
            budget_metrics_stale: true,
            // Concord W2:斜杠补全选中项与 `:` 弃用提示状态
            slash_selected: 0,
            colon_deprecation_shown: false,
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
            w_prefix: false,
            decay_history: Vec::new(),
            // P6.2 布局模板默认值(DualPane,见 LayoutMode::default 的 WHY 注释)
            layout_mode: LayoutMode::default(),
            // P7 OsaSparse / ClvVector 面板默认值(未收到事件时为 None / 空)
            osa_sparsity: None,
            osa_context_mask: Vec::new(),
            osa_sparsity_history: Vec::new(),
            clv_summary: None,
            // PROBE P0.4:HCW 召回读数默认值(未收到报告时为 None)
            recall_needle_at_8: None,
            recall_position_bias: None,
            recall_chain_success: None,
            // P8 ResourceMonitor 面板默认值
            sys_metrics: SystemMetrics::default(),
            sys_metrics_history: Vec::new(),
            tick_mode: TickMode::default(),
            // M3b Chat 面板默认值(空历史 + 空闲状态)
            chat_messages: Vec::new(),
            chat_status: ChatStatus::default(),
            last_action_feedback_seq: 0,
            // M3 监控/视图控制默认值(= 当前行为,零回归)
            monitor_paused: false,
            monitor_window: MonitorWindow::default(),
            clv_heatmap_autoscale: false,
            // P1-W2.2:Critical 旁路通道丢弃计数(0 = 无丢弃)
            critical_event_dropped_count: 0,
            last_snapshot_revision: 0,
            // F-5:无待派发参数动作
            pending_action: None,
            // P1-2:无待确认动作(超时兜底未启动)
            pending_action_deadline: None,
        }
    }

    /// 将 TuiState 的布局相关字段序列化保存到 YAML 文件
    ///
    /// WHY 只保存布局字段:运行时数据(quest_list/latest_events/metrics)由
    /// DataPipeline 从 event-bus 重新填充,无需持久化。
    pub fn save_to_file(&self, path: &std::path::Path) -> Result<(), TuiError> {
        // 创建父目录（最佳努力,目录已存在时忽略错误）
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let yaml = serde_yaml::to_string(self).map_err(|e| TuiError::ConfigError {
            detail: format!("YAML serialize failed: {e}"),
        })?;
        std::fs::write(path, yaml).map_err(|e| TuiError::ConfigError {
            detail: format!("write state file failed: {e}"),
        })?;
        Ok(())
    }

    /// 从 YAML 文件加载 TuiState 的布局字段
    ///
    /// WHY 只恢复布局字段:运行时数据由 DataPipeline 重新填充。
    /// 加载失败时降级为默认状态,不阻塞启动。
    pub fn load_from_file(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(yaml) => match serde_yaml::from_str::<Self>(&yaml) {
                Ok(saved) => {
                    tracing::info!(
                        path = %path.display(),
                        "TuiState restored from file"
                    );
                    // 只恢复"视图/布局字段"(与 `apply_view_fields` 白名单一致):
                    // 运行时数据(quest_list / latest_events / metrics / histories /
                    // chat_messages 等)一律重置为初始值,由 DataPipeline 从 event-bus
                    // 重新填充。WHY:陈旧状态文件(如本机 ~/.chimera/tui_state.yaml 残留
                    // 的 budget_history / frame_count)若被恢复,会污染新会话的 dirty
                    // 首帧判定,并破坏测试封闭性(incremental_render_test 曾因此误标
                    // 无关面板);本实现同时兑现了 save_to_file 文档"只保存布局字段"
                    // 的契约(旧实现仅重置 4 个字段,语义与文档不符)。
                    Self {
                        layout_mode: saved.layout_mode,
                        // Concord W3 T3.4:视图模式属"视图字段",随状态文件保留
                        // 用户选择(旧文件无本字段时 serde 默认 Chat)
                        view_mode: saved.view_mode,
                        // Concord W4 T4.1:审批模式同属会话策略字段(旧文件默认 Normal)
                        approval_mode: saved.approval_mode,
                        // Concord W6 T6.2:composer 历史跨会话保留(导航位置复位,
                        // 回溯态不跨会话延续)
                        input_history: saved.input_history,
                        history_pos: None,
                        history_draft: String::new(),
                        filter_keyword: saved.filter_keyword,
                        filter_topic: saved.filter_topic,
                        filter_level: saved.filter_level,
                        monitor_window: saved.monitor_window,
                        clv_heatmap_autoscale: saved.clv_heatmap_autoscale,
                        ..Self::new()
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "Failed to deserialize TuiState, falling back to default"
                    );
                    Self::new()
                }
            },
            Err(_) => {
                // 文件不存在是正常情况(首次启动),不记录警告
                Self::new()
            }
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

    #[test]
    fn layout_mode_aliases_to_pane_mode() {
        // M2 增量3:LayoutMode → PaneMode 别名映射(别名共存)
        assert_eq!(LayoutMode::SinglePane.to_pane_mode(), PaneMode::Focus);
        assert_eq!(LayoutMode::DualPane.to_pane_mode(), PaneMode::Chat);
        assert_eq!(LayoutMode::TriplePane.to_pane_mode(), PaneMode::Ide);
        assert_eq!(LayoutMode::VimSplit.to_pane_mode(), PaneMode::VimSplit);
    }

    #[test]
    fn monitor_window_cycles_and_maps_points() {
        // 默认 Long(= 全 64 点,零回归)
        assert_eq!(MonitorWindow::default(), MonitorWindow::Long);
        // 循环:Short → Medium → Long → Short
        assert_eq!(MonitorWindow::Short.next(), MonitorWindow::Medium);
        assert_eq!(MonitorWindow::Medium.next(), MonitorWindow::Long);
        assert_eq!(MonitorWindow::Long.next(), MonitorWindow::Short);
        // 点数映射
        assert_eq!(MonitorWindow::Short.points(), 16);
        assert_eq!(MonitorWindow::Medium.points(), 32);
        assert_eq!(MonitorWindow::Long.points(), 64);
    }

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
        // Concord T1.4:next/prev 派生自 REGISTERED_FOCUS_ORDER 单一事实源,
        // 逐边断言改为"全环逐元素验证 + 关键边抽查"(避免与源表双维护)。
        let order = PanelId::REGISTERED_FOCUS_ORDER;
        for (i, p) in order.iter().enumerate() {
            assert_eq!(
                p.next(),
                order[(i + 1) % order.len()],
                "{p:?}.next() 应指向环内下一面板"
            );
        }
        // 关键边抽查(循环闭合 + 历史接入点)
        assert_eq!(PanelId::Quest.next(), PanelId::Parliament);
        assert_eq!(PanelId::OverWindow.next(), PanelId::Quest, "环必须闭合");
        assert_eq!(
            PanelId::Chtc.next(),
            PanelId::Timeline,
            "Timeline 已接线入环"
        );
        assert_eq!(
            PanelId::MetricsDashboard.next(),
            PanelId::Sysinfo,
            "Sysinfo 已接线入环"
        );
    }

    #[test]
    fn test_panel_id_prev() {
        let order = PanelId::REGISTERED_FOCUS_ORDER;
        for (i, p) in order.iter().enumerate() {
            assert_eq!(
                p.prev(),
                order[(i + order.len() - 1) % order.len()],
                "{p:?}.prev() 应指向环内上一面板"
            );
        }
        // 关键边抽查(循环闭合)
        assert_eq!(PanelId::Quest.prev(), PanelId::OverWindow);
        assert_eq!(PanelId::Timeline.prev(), PanelId::Chtc);
    }

    #[test]
    fn test_panel_id_next_prev_roundtrip() {
        // next 再 prev 应回到原面板(P8 扩展至 19 面板)
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
            PanelId::Sysinfo,
            PanelId::Chat,
            // polish-v2.7 P1-5:SelfAssessment 加入往返验证
            PanelId::SelfAssessment,
            // closure Stage B-10:DagViz 加入往返验证
            PanelId::DagViz,
            // Task 3.7/3.9 + P1:PvlScore/TaskManager/OverWindow 加入往返验证(25 面板循环,Concord T1.4)
            PanelId::PvlScore,
            PanelId::TaskManager,
            PanelId::OverWindow,
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
        assert_eq!(LayoutMode::VimSplit.as_str(), "vim");
    }

    #[test]
    fn test_layout_mode_next_cycle() {
        // Single → Dual → Triple → VimSplit → Single(M3d 4 态循环)
        assert_eq!(LayoutMode::SinglePane.next(), LayoutMode::DualPane);
        assert_eq!(LayoutMode::DualPane.next(), LayoutMode::TriplePane);
        assert_eq!(LayoutMode::TriplePane.next(), LayoutMode::VimSplit);
        assert_eq!(LayoutMode::VimSplit.next(), LayoutMode::SinglePane);
        // 完整循环验证:连续 next 四次回到起点
        let mode = LayoutMode::SinglePane;
        assert_eq!(mode.next().next().next().next(), mode);
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

    // ============================================================
    // TickMode 测试
    // ============================================================

    #[test]
    fn test_tick_mode_default() {
        assert_eq!(TickMode::default(), TickMode::Normal);
    }

    #[test]
    fn test_tick_mode_display() {
        assert_eq!(TickMode::Normal.display(), "Normal");
        assert_eq!(TickMode::Eco.display(), "Eco");
    }

    #[test]
    fn test_tick_mode_serialization() {
        // Normal 序列化/反序列化
        let mode = TickMode::Normal;
        let json = serde_json::to_string(&mode).unwrap();
        let restored: TickMode = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, TickMode::Normal);

        // Eco 序列化/反序列化
        let mode = TickMode::Eco;
        let json = serde_json::to_string(&mode).unwrap();
        let restored: TickMode = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, TickMode::Eco);
    }

    #[test]
    fn test_tui_state_tick_mode_default() {
        let state = TuiState::new();
        assert_eq!(state.tick_mode, TickMode::Normal);
    }

    #[test]
    fn test_tick_mode_serde_roundtrip() {
        // Normal ↔ JSON 往返
        let mode = TickMode::Normal;
        let json = serde_json::to_string(&mode).unwrap();
        let restored: TickMode = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, TickMode::Normal);

        // Eco ↔ JSON 往返
        let mode = TickMode::Eco;
        let json = serde_json::to_string(&mode).unwrap();
        let restored: TickMode = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, TickMode::Eco);
    }
}

#[cfg(test)]
mod state_persistence_tests {
    use super::*;

    /// 测试正常保存/恢复往返
    #[test]
    fn test_state_roundtrip() {
        let mut state = TuiState::new();
        state.layout_mode = LayoutMode::TriplePane;
        state.filter_keyword = Some("test".to_string());
        state.running = false;
        // 运行时字段故意塞入非默认值:验证恢复后必须被重置(视图/布局契约)
        state.budget_history = vec![1, 2, 3];
        state.latest_events.push_back(NexusEvent::CacheHit {
            metadata: event_bus::EventMetadata::new("chimera-tui"),
            cache_key: "roundtrip-key".into(),
        });

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tui_state.yaml");
        state.save_to_file(&path).unwrap();
        assert!(path.exists());

        let loaded = TuiState::load_from_file(&path);
        assert_eq!(loaded.layout_mode, LayoutMode::TriplePane);
        assert_eq!(loaded.filter_keyword, Some("test".to_string()));
        // 运行时字段应重置(陈旧状态不得污染新会话)
        assert!(loaded.running);
        assert!(
            loaded.budget_history.is_empty(),
            "budget_history 不应从文件恢复"
        );
        assert!(
            loaded.latest_events.is_empty(),
            "latest_events 不应从文件恢复"
        );
    }

    /// 测试文件不存在时降级为默认状态
    #[test]
    fn test_load_nonexistent_file() {
        let state = TuiState::load_from_file(std::path::Path::new("/nonexistent/tui_state.yaml"));
        assert!(state.running);
        assert_eq!(state.layout_mode, LayoutMode::DualPane);
    }

    /// 测试反序列化失败时降级且不 panic
    #[test]
    fn test_load_corrupted_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tui_state.yaml");
        std::fs::write(&path, "{{{ invalid yaml").unwrap();

        let state = TuiState::load_from_file(&path);
        assert!(state.running);
        assert_eq!(state.layout_mode, LayoutMode::DualPane);
    }
}
