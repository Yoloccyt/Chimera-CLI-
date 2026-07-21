//! components::traits — 组件系统契约(ADR-029,v3.1 自研渲染引擎 L5)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **声明式 `view`**:面板只声明"长什么样"(返回 `LayoutNode` 布局树),
//!   由框架负责 diff/paint,替代 ratatui 命令式 `render(area, buf)`。
//! - **`actions()` 暴露上下文动作**:面板声明自己支持的 Action id 列表,供面板
//!   上下文动作菜单与 ? 帮助从 `ActionRegistry` 取描述(§4.5 交互扩展),
//!   落实"无只读死面板"铁律——每个面板至少暴露一个交互动作。
//! - **`ViewContext` 携带渲染上下文**:area/theme/locale/is_focused 一次性传入,
//!   避免面板各自读取全局状态,便于测试注入。
//! - **M0 骨架**:`LayoutNode` 为最小占位树,M2 补充具体绘制指令与内建组件。

use crate::config::Theme;
use crate::engine::rect::Rect;
use crate::i18n::Locale;
use crate::types::PanelId;

/// 视图上下文 — 渲染一帧时传入组件的只读环境
#[derive(Debug, Clone, Copy)]
pub struct ViewContext {
    /// 组件被分配的绘制区域
    pub area: Rect,
    /// 当前主题(颜色方案来源)
    pub theme: Theme,
    /// 当前界面语言(文案经 i18n 解析)
    pub locale: Locale,
    /// 组件是否持有焦点(决定是否高亮边框/启用输入)
    pub is_focused: bool,
}

impl ViewContext {
    /// 构造视图上下文
    pub fn new(area: Rect, theme: Theme, locale: Locale, is_focused: bool) -> Self {
        Self {
            area,
            theme,
            locale,
            is_focused,
        }
    }
}

/// 布局节点 — 声明式布局树(M0 最小占位,M2 补充绘制指令)
///
/// WHY 树形:对齐 v3 组件系统——容器节点递归排列子节点,叶子节点占据区域并
/// 由具体组件绘制。M0 仅表达结构与区域,M2 增加文本/图表等叶子内容与 Flexbox
/// 约束,接入 `engine::layout`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutNode {
    /// 叶子节点 — 占据一个区域,由组件在该区域绘制具体内容
    Leaf {
        /// 分配区域
        area: Rect,
    },
    /// 容器节点 — 递归排列子节点
    Container {
        /// 容器区域
        area: Rect,
        /// 子节点(M2 增加方向/约束)
        children: Vec<LayoutNode>,
    },
}

impl LayoutNode {
    /// 返回该节点占据的区域
    pub fn area(&self) -> Rect {
        match self {
            LayoutNode::Leaf { area } => *area,
            LayoutNode::Container { area, .. } => *area,
        }
    }
}

/// 组件面板契约 — 自研引擎下所有面板的统一接口(替代旧 `panels::Panel`)
///
/// WHY 与旧 `Panel` trait 并存:M2 起旧面板经 `LegacyPanelAdapter` 适配为本 trait,
/// 逐面板迁移;M5 全部迁移后移除旧 trait。要求 `Send` 以兼容未来 async 渲染。
pub trait ComponentPanel: Send {
    /// 面板唯一标识(复用既有 `PanelId`,保证与焦点管理/事件映射一致)
    fn id(&self) -> PanelId;

    /// 声明本面板的视图布局树
    fn view(&self, ctx: &ViewContext) -> LayoutNode;

    /// 声明本面板暴露的上下文动作 id 列表(默认空)
    ///
    /// WHY 默认空但鼓励覆盖:交互式 TUI 铁律要求"无只读死面板",功能性面板
    /// 应至少返回 `"panel.drill_down"` 等一个动作;纯展示面板可保持空并由框架
    /// 提供通用下钻。返回的 id 须存在于 `ActionRegistry`。
    fn actions(&self) -> Vec<&'static str> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小组件示例 — 验证 trait 可实现、view 返回布局树、actions 可声明
    struct SampleQuestPanel;

    impl ComponentPanel for SampleQuestPanel {
        fn id(&self) -> PanelId {
            PanelId::Quest
        }
        fn view(&self, ctx: &ViewContext) -> LayoutNode {
            LayoutNode::Leaf { area: ctx.area }
        }
        fn actions(&self) -> Vec<&'static str> {
            vec!["quest.pause", "panel.drill_down"]
        }
    }

    #[test]
    fn component_panel_view_and_actions() {
        let panel = SampleQuestPanel;
        let ctx = ViewContext::new(Rect::new(0, 0, 40, 10), Theme::Dark, Locale::Zh, true);
        let node = panel.view(&ctx);
        assert_eq!(node.area(), Rect::new(0, 0, 40, 10));
        assert_eq!(panel.id(), PanelId::Quest);
        // 功能面板暴露交互动作(无只读死面板铁律)
        assert!(panel.actions().contains(&"quest.pause"));
    }
}
