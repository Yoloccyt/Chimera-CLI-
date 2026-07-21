//! engine::layout::presets — 四种布局模式预设与命令面板 overlay(ADR-029,v3.1 M1.4)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **四模式对齐 v3 §2.1**:IDE 三面板 / Chat 搜索框(默认 2 栏)/ Vim 分屏 /
//!   Focus 全屏,覆盖交互式 TUI 的主要场景。
//! - **默认 2 栏 = Chat**:呼应用户北极星"界面极致简洁"——默认 Chat 主区 + 单一
//!   上下文面板,其余面板收入"视图"按需呼出(§4.6 一屏一事)。
//! - **命令面板 overlay 预留**:`centered_overlay` 计算居中矩形,供统一命令面板
//!   (Ctrl+P)悬浮于任意模式之上——落实"所有命令集成到一个命令面板"的呈现层。
//! - **用 `flex::split` 直接切分**:内建固定布局用一/二级 split 表达最清晰;
//!   动态/用户自定义嵌套布局用 `node::LayoutTree`(两者共享同一切分原语)。

use crate::engine::layout::constraint::{Constraint, Direction};
use crate::engine::layout::flex::split;
use crate::engine::rect::Rect;

/// IDE 模式左侧会话树宽度(字符)
const IDE_SIDEBAR_WIDTH: u16 = 20;
/// IDE 模式右侧上下文面板宽度(字符)
const IDE_CONTEXT_WIDTH: u16 = 28;
/// Chat/默认模式右侧单一上下文面板宽度(字符)
const CHAT_CONTEXT_WIDTH: u16 = 30;
/// 底部状态栏高度(行)
const STATUS_HEIGHT: u16 = 1;
/// 顶部横幅高度(行)
const BANNER_HEIGHT: u16 = 1;

/// 布局模式 —— v3 §2.1 四种模式(独立于旧 `types::LayoutMode` 三态,M2 桥接)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneMode {
    /// IDE 三面板:横幅 + 左会话树 + 中主区 + 右上下文 + 状态栏
    Ide,
    /// Chat 搜索框(默认 2 栏):主区 + 单一上下文 + 状态栏
    Chat,
    /// Vim 分屏:左右两等分编辑区 + 底部命令行
    VimSplit,
    /// Focus 全屏:单一主区 + 状态栏
    Focus,
}

impl Default for PaneMode {
    /// 默认 Chat 2 栏(极致简洁北极星)
    fn default() -> Self {
        PaneMode::Chat
    }
}

/// 计算后的命名区域 —— `main`/`status` 恒存在,模式特有区为 `Option`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Regions {
    /// 顶部横幅(IDE 有;其余 None)
    pub banner: Option<Rect>,
    /// 左侧会话树(IDE 有;其余 None)
    pub sidebar: Option<Rect>,
    /// 主内容区(所有模式恒存在)
    pub main: Rect,
    /// 上下文/次要区(Chat 单栏 / IDE 右栏 / Vim 右分屏;Focus None)
    pub context: Option<Rect>,
    /// 底部状态栏 / 命令行(所有模式恒存在)
    pub status: Rect,
}

/// 按模式计算命名区域
pub fn regions_for(mode: PaneMode, viewport: Rect) -> Regions {
    match mode {
        PaneMode::Ide => ide(viewport),
        PaneMode::Chat => chat(viewport),
        PaneMode::VimSplit => vim_split(viewport),
        PaneMode::Focus => focus(viewport),
    }
}

/// Focus 全屏:主区占满,底部 1 行状态栏
fn focus(viewport: Rect) -> Regions {
    let rows = split(
        viewport,
        Direction::Vertical,
        &[Constraint::Flex(1), Constraint::Fixed(STATUS_HEIGHT)],
    );
    Regions {
        banner: None,
        sidebar: None,
        main: rows[0],
        context: None,
        status: rows[1],
    }
}

/// Chat / 默认 2 栏:body(主区 + 右上下文)+ 底部状态栏
fn chat(viewport: Rect) -> Regions {
    let rows = split(
        viewport,
        Direction::Vertical,
        &[Constraint::Flex(1), Constraint::Fixed(STATUS_HEIGHT)],
    );
    let cols = split(
        rows[0],
        Direction::Horizontal,
        &[Constraint::Flex(1), Constraint::Fixed(CHAT_CONTEXT_WIDTH)],
    );
    Regions {
        banner: None,
        sidebar: None,
        main: cols[0],
        context: Some(cols[1]),
        status: rows[1],
    }
}

/// IDE 三面板:横幅 + [左会话树 | 主区 | 右上下文] + 状态栏
fn ide(viewport: Rect) -> Regions {
    let rows = split(
        viewport,
        Direction::Vertical,
        &[
            Constraint::Fixed(BANNER_HEIGHT),
            Constraint::Flex(1),
            Constraint::Fixed(STATUS_HEIGHT),
        ],
    );
    let cols = split(
        rows[1],
        Direction::Horizontal,
        &[
            Constraint::Fixed(IDE_SIDEBAR_WIDTH),
            Constraint::Flex(1),
            Constraint::Fixed(IDE_CONTEXT_WIDTH),
        ],
    );
    Regions {
        banner: Some(rows[0]),
        sidebar: Some(cols[0]),
        main: cols[1],
        context: Some(cols[2]),
        status: rows[2],
    }
}

/// Vim 分屏:上部左右两等分 + 底部命令行
fn vim_split(viewport: Rect) -> Regions {
    let rows = split(
        viewport,
        Direction::Vertical,
        &[Constraint::Flex(1), Constraint::Fixed(STATUS_HEIGHT)],
    );
    let cols = split(
        rows[0],
        Direction::Horizontal,
        &[Constraint::Flex(1), Constraint::Flex(1)],
    );
    Regions {
        banner: None,
        sidebar: None,
        main: cols[0],
        context: Some(cols[1]),
        status: rows[1],
    }
}

/// 计算居中 overlay 矩形(命令面板悬浮层),按视口宽/高百分比取尺寸并居中
///
/// WHY 居中 overlay:统一命令面板(Ctrl+P)以模态悬浮于任意布局之上,
/// 不打乱底层区域,契合"渐进披露 + 一个命令面板集成所有命令"。
pub fn centered_overlay(area: Rect, width_pct: u16, height_pct: u16) -> Rect {
    let w = (area.width as u32 * width_pct.min(100) as u32 / 100) as u16;
    let h = (area.height as u32 * height_pct.min(100) as u32 / 100) as u16;
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_has_main_and_status_only() {
        let r = regions_for(PaneMode::Focus, Rect::new(0, 0, 80, 24));
        assert!(r.banner.is_none() && r.sidebar.is_none() && r.context.is_none());
        assert_eq!(r.main, Rect::new(0, 0, 80, 23));
        assert_eq!(r.status, Rect::new(0, 23, 80, 1));
    }

    #[test]
    fn chat_default_is_two_column() {
        let r = regions_for(PaneMode::Chat, Rect::new(0, 0, 100, 24));
        // 主区 + 右上下文两栏,底部状态栏
        assert_eq!(r.main, Rect::new(0, 0, 70, 23));
        assert_eq!(r.context, Some(Rect::new(70, 0, 30, 23)));
        assert_eq!(r.status, Rect::new(0, 23, 100, 1));
        assert!(r.sidebar.is_none() && r.banner.is_none());
    }

    #[test]
    fn ide_has_all_five_regions() {
        let r = regions_for(PaneMode::Ide, Rect::new(0, 0, 100, 30));
        assert!(r.banner.is_some() && r.sidebar.is_some() && r.context.is_some());
        // 横幅在顶,状态栏在底,三栏在中间行
        assert_eq!(r.banner.unwrap(), Rect::new(0, 0, 100, 1));
        assert_eq!(r.sidebar.unwrap(), Rect::new(0, 1, 20, 28));
        assert_eq!(r.main, Rect::new(20, 1, 52, 28));
        assert_eq!(r.context.unwrap(), Rect::new(72, 1, 28, 28));
        assert_eq!(r.status, Rect::new(0, 29, 100, 1));
    }

    #[test]
    fn vim_split_is_two_equal_panes() {
        let r = regions_for(PaneMode::VimSplit, Rect::new(0, 0, 80, 24));
        assert_eq!(r.main, Rect::new(0, 0, 40, 23));
        assert_eq!(r.context, Some(Rect::new(40, 0, 40, 23)));
        assert_eq!(r.status, Rect::new(0, 23, 80, 1));
    }

    #[test]
    fn centered_overlay_is_centered_and_sized() {
        let o = centered_overlay(Rect::new(0, 0, 100, 40), 60, 50);
        assert_eq!(o, Rect::new(20, 10, 60, 20));
    }

    #[test]
    fn default_pane_mode_is_chat() {
        assert_eq!(PaneMode::default(), PaneMode::Chat);
    }
}
