//! ActionDescriptor — 单个可交互动作的元描述(ADR-029,v3.1)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **Action 是功能的单一事实源**:每个可交互功能注册为一个 `ActionDescriptor`,
//!   斜杠命令 / 命令面板 / 面板上下文动作三入口全部从注册表派生(见 `codegen`),
//!   杜绝"三处手写功能清单导致漂移腐化"。
//! - **标题/描述存 i18n key 而非字符串**:locale 切换时 palette 与帮助文案随之
//!   切换(§4.7),`ActionDescriptor` 只存 key,展示时经 `i18n::tr` 解析。
//! - **`id` 用 `&'static str`**:Action 均为编译期已知,静态 id 零分配,且与
//!   `NexusEvent::TuiActionRequested.action_id: String` 通过 `.to_string()` 桥接。

use serde::{Deserialize, Serialize};

/// 动作域 — 按功能职责分包,防止 Registry 沦为上帝对象(§4.2)
///
/// WHY 分域:Action 按域(Quest/Task/Export/View/System/Config)分文件注册,
/// `ActionRegistry` 只做索引;新增 Action 落到对应域文件,评审聚焦单域。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActionDomain {
    /// Quest 与 Agent 对话域(agent.chat / quest.start/pause/resume/cancel/jump)
    Quest,
    /// 任务生命周期域(task.create/pause/resume/cancel/set_priority)
    Task,
    /// 数据导出域(export.run,3 格式 × 3 目标)
    Export,
    /// 视图与下钻域(view.switch_layout / view.apply_saved / panel.drill_down)
    View,
    /// 系统域(system.toggle_locale / system.open_help / monitor.* / viz.*)
    System,
    /// 配置域(config.edit,TuiBible 项)
    Config,
}

impl ActionDomain {
    /// 返回域的稳定标识(用于日志/分组展示)
    pub fn as_str(&self) -> &'static str {
        match self {
            ActionDomain::Quest => "quest",
            ActionDomain::Task => "task",
            ActionDomain::Export => "export",
            ActionDomain::View => "view",
            ActionDomain::System => "system",
            ActionDomain::Config => "config",
        }
    }
}

/// 动作元描述 — 三入口共享的功能契约
///
/// # 字段
/// - `id`:全局唯一动作标识(如 `"quest.pause"`),与斜杠命令/事件 action_id 一致
/// - `domain`:所属功能域
/// - `title_key` / `desc_key`:i18n 资源 key(展示时经 `i18n::tr` 解析,支持中英)
/// - `slash`:斜杠命令触发词(如 `"quest pause"`);`None` 表示不暴露斜杠入口
/// - `default_key`:默认全局/面板快捷键(如 `"Ctrl+E"`);`None` 表示无默认键
/// - `requires_context`:是否需要运行时上下文(如焦点 Quest/Task 的 id)才能执行
/// - `is_core`:是否属于"核心 10 功能"(§八 可达性验收:核心功能须 ≤3 键可达)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionDescriptor {
    /// 全局唯一动作标识(如 "quest.pause")
    pub id: &'static str,
    /// 所属功能域
    pub domain: ActionDomain,
    /// 标题的 i18n key(如 "action.quest.pause")
    pub title_key: &'static str,
    /// 描述的 i18n key(帮助面板 hover 展示;可与 title_key 复用)
    pub desc_key: &'static str,
    /// 斜杠命令触发词(不含前导 `/`);None 表示不提供斜杠入口
    pub slash: Option<&'static str>,
    /// 默认快捷键字符串(如 "Ctrl+E");None 表示无默认快捷键
    pub default_key: Option<&'static str>,
    /// 是否需要运行时上下文(焦点项 id 等)才能执行
    pub requires_context: bool,
    /// 是否为核心 10 功能(可达性验收基线)
    pub is_core: bool,
}

impl ActionDescriptor {
    /// 便捷构造:非核心、无快捷键、无上下文要求的常规动作
    ///
    /// WHY 便捷构造:多数域动作参数相同,减少各域注册表的样板噪声,
    /// 需要定制(核心/快捷键/上下文)时再用结构体字面量覆盖字段。
    pub const fn new(
        id: &'static str,
        domain: ActionDomain,
        title_key: &'static str,
        slash: Option<&'static str>,
    ) -> Self {
        Self {
            id,
            domain,
            title_key,
            // 默认描述复用标题 key,域文件可按需指定更详细的 desc_key
            desc_key: title_key,
            slash,
            default_key: None,
            requires_context: false,
            is_core: false,
        }
    }
}
