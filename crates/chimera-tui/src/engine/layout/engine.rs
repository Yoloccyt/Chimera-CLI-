//! engine::layout::engine — 布局引擎门面(ADR-029,v3.1 M1.4)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **门面持状态、计算纯函数**:`LayoutEngine` 只保存当前模式 + 视口;区域计算
//!   委托 `presets`(纯函数)。M2 渲染循环持有一个 LayoutEngine,resize 时更新
//!   viewport、切换模式时更新 mode,每帧调用 `regions()` 取区域,职责单一、易测。
//! - **命令面板 overlay 一等公民**:`palette_overlay()` 在任意模式下均可计算,
//!   落实"统一命令面板悬浮于一切布局之上"的北极星。

use crate::engine::layout::presets::{centered_overlay, regions_for, PaneMode, Regions};
use crate::engine::rect::Rect;

/// 命令面板 overlay 默认宽度百分比(视口宽的 60%)
const PALETTE_WIDTH_PCT: u16 = 60;
/// 命令面板 overlay 默认高度百分比(视口高的 60%)
const PALETTE_HEIGHT_PCT: u16 = 60;

/// 布局引擎门面 —— 持有当前模式 + 视口,计算命名区域与命令面板 overlay
#[derive(Debug, Clone, Copy)]
pub struct LayoutEngine {
    /// 当前布局模式
    mode: PaneMode,
    /// 当前终端视口区域
    viewport: Rect,
}

impl LayoutEngine {
    /// 以指定模式与视口构造
    pub fn new(mode: PaneMode, viewport: Rect) -> Self {
        Self { mode, viewport }
    }

    /// 当前布局模式
    pub fn mode(&self) -> PaneMode {
        self.mode
    }

    /// 切换布局模式(view.switch_layout 动作触发)
    pub fn set_mode(&mut self, mode: PaneMode) {
        self.mode = mode;
    }

    /// 当前视口
    pub fn viewport(&self) -> Rect {
        self.viewport
    }

    /// 更新视口(终端 resize 时调用)
    pub fn set_viewport(&mut self, viewport: Rect) {
        self.viewport = viewport;
    }

    /// 计算当前模式下的命名区域
    pub fn regions(&self) -> Regions {
        regions_for(self.mode, self.viewport)
    }

    /// 计算命令面板居中 overlay 区域(悬浮于当前布局之上)
    pub fn palette_overlay(&self) -> Rect {
        centered_overlay(self.viewport, PALETTE_WIDTH_PCT, PALETTE_HEIGHT_PCT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_dispatches_regions_by_mode() {
        let eng = LayoutEngine::new(PaneMode::Focus, Rect::new(0, 0, 80, 24));
        let r = eng.regions();
        // Focus 模式:仅 main + status
        assert!(r.context.is_none());
        assert_eq!(r.main, Rect::new(0, 0, 80, 23));
    }

    #[test]
    fn set_mode_and_viewport_update_state() {
        let mut eng = LayoutEngine::new(PaneMode::Chat, Rect::new(0, 0, 80, 24));
        eng.set_mode(PaneMode::Ide);
        eng.set_viewport(Rect::new(0, 0, 100, 30));
        assert_eq!(eng.mode(), PaneMode::Ide);
        assert_eq!(eng.viewport(), Rect::new(0, 0, 100, 30));
        // IDE 模式有横幅
        assert!(eng.regions().banner.is_some());
    }

    #[test]
    fn palette_overlay_is_centered() {
        let eng = LayoutEngine::new(PaneMode::Chat, Rect::new(0, 0, 100, 40));
        // 60% × 60% 居中
        assert_eq!(eng.palette_overlay(), Rect::new(20, 8, 60, 24));
    }
}
