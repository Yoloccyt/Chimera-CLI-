//! 窗格管理器 — 多窗格布局、伴随面板与焦点窗格状态
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! Task 1.15 拆分:原 TuiApp 18 字段中,7 个与窗格/布局相关,集中到 PaneManager:
//! - 单一职责(窗格状态与布局计算)
//! - TuiApp 字段数从 18 降至 10(spec 1.15.4 要求)
//! - 后续扩展(如新增 PaneMode)无需膨胀 TuiApp

use ratatui::layout::Rect;

use crate::types::PanelId;

/// 主面板比例调整步长
pub(crate) const RATIO_STEP: f32 = 0.05;
/// 主面板比例最小值
pub(crate) const RATIO_MIN: f32 = 0.3;
/// 主面板比例最大值
pub(crate) const RATIO_MAX: f32 = 0.9;

/// 窗格管理器 — 持有面板布局、伴随面板、活跃窗格与渲染区域等视图状态
///
/// 字段对 TuiApp 内部方法公开(`pub(crate)`),便于事件循环/渲染/状态管理
/// 直接读写,避免冗余访问器方法。拆分目的是聚合相关字段,而非隐藏状态。
#[derive(Debug)]
pub struct PaneManager {
    /// 当前会话的主面板比例(从配置初始化,不持久化到文件)
    pub main_panel_ratio: f32,
    /// 伴随面板可见性(M2 增量3 Stage 1,opt-in,默认关闭)
    ///
    /// WHY 默认关闭:开启时主区右侧并排渲染伴随面板;关闭时 `render_main_panel`
    /// 行为与现状逐字节一致,保证既有 render/layout 测试零回归。
    pub companion_visible: bool,
    /// 伴随面板目标 = 最近使用的面板(切换焦点时记录切换前的面板)
    pub prev_panel: Option<PanelId>,
    /// 显式绑定的伴随面板(M2 增量3 Stage 2,None = 回退 Stage1 自动"最近使用")
    ///
    /// WHY 与 prev_panel 分离:`]` 循环绑定写入本字段并优先于自动逻辑;
    /// 未绑定时保持 Stage1 行为,零回归。
    pub bound_companion: Option<PanelId>,
    /// 活跃窗格索引(M3d 多窗格,0 = 主窗格,默认主区活跃)
    ///
    /// WHY 从 bool 升级为索引:M3d 把"主+单一伴随"2 窗格泛化为 PaneMode 驱动的
    /// 多窗格(Chat 2 / VimSplit 2 / IDE 3),`active_pane` 是 `pane_panels()` 循环序
    /// 的下标——面板级键路由到该窗格面板、渲染时高亮其边框。`w` 键环形递增,
    /// 主区焦点变化 / 窗格数收缩时复位或钳制回 0。2 窗格时 1 = 伴随,语义等价 Stage 2。
    pub active_pane: usize,
    /// 上一帧的焦点面板,用于避免每帧重复调用 `focus(true/false)`
    ///
    /// WHY M1 清理项 #5:仅在实际变化时通知面板焦点变化,减少无效回调。
    pub last_focused: Option<PanelId>,
    /// 最后一帧的终端区域,用于鼠标事件命中测试
    pub last_area: Rect,
}

impl PaneManager {
    /// 创建默认 PaneManager(从配置初始化比例)
    pub fn new(main_panel_ratio: f32) -> Self {
        Self {
            main_panel_ratio,
            companion_visible: false,
            prev_panel: None,
            bound_companion: None,
            active_pane: 0,
            last_focused: None,
            last_area: Rect::default(),
        }
    }

    /// 调整主面板比例
    ///
    /// `increase` 为 true 时增大比例,否则减小。限制在 [RATIO_MIN, RATIO_MAX]。
    pub(crate) fn adjust_main_panel_ratio(&mut self, increase: bool) {
        let delta = if increase { RATIO_STEP } else { -RATIO_STEP };
        self.main_panel_ratio = (self.main_panel_ratio + delta).clamp(RATIO_MIN, RATIO_MAX);
    }

    /// v2.9.0-omega Task 2.6:判断窄视口下是否应折叠伴随面板(响应式布局)
    ///
    /// 当终端宽度低于 `threshold` 时返回 `true`,调用方据此隐藏伴随面板,
    /// 让主面板独占宽度,提升窄终端下的可读性。
    ///
    /// WHY 阈值可配:不同面板内容密度不同,用户可通过 `TuiConfig::responsive_collapse_threshold`
    /// 调整(设为 0 禁用自动折叠,保持原有 DualPane 行为)。
    ///
    /// # 参数
    /// - `terminal_width`:当前终端宽度(列数)
    /// - `threshold`:折叠阈值(来自 `TuiConfig::responsive_collapse_threshold`)
    pub fn should_collapse_companion(&self, terminal_width: u16, threshold: u16) -> bool {
        // 阈值 0 表示禁用响应式折叠,始终返回 false
        threshold > 0 && terminal_width < threshold
    }
}
