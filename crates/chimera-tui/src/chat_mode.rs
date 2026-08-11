//! ChatMode — 会话模式视图编排(Concord W3 · T3.2,ADR-076)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! Conversation-First:Chat 模式为第一默认视图——会话流全屏、composer 底栏、
//! statusline 三区域,与主流 Agent CLI 交互同构;既有 25 面板驾驶舱下沉为
//! `ViewMode::Dashboard`,资产不推倒(方案 §5 设计理念)。
//!
//! 布局走既有 `engine::layout::flex::split` 求解器(与 Dashboard 同一求解器,
//! 单流布局是约束的退化情形),不引入第二套布局系统。

use ratatui::layout::Rect;

use crate::engine::layout::{split, Constraint, Direction};
use crate::engine::{from_ratatui_rect, to_ratatui_rect};

/// 会话模式区域布局结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatLayout {
    /// 会话流区域(弹性占满剩余)
    pub stream: Rect,
    /// ModeBanner 条件行(Concord W6 T6.1:Plan/Auto 态 Some,Normal 态 None)
    pub banner: Option<Rect>,
    /// composer 输入区(固定 3 行:边框 2 + 输入 1)
    pub composer: Rect,
    /// statusline(固定 1 行)
    pub status: Rect,
}

/// 计算会话模式区域布局
///
/// # 参数
/// - `area`:整个终端可视区域
/// - `banner_visible`:是否预留 ModeBanner 条件行(Plan/Auto 态 true;
///   Normal 态 false 时布局与 W3 逐字节一致,零冲击面)
///
/// # 返回
/// 自上而下区域;banner 行在 stream 与 composer 之间(方案 §7.2);
/// 极小视口时求解器自然压缩,不 panic。
pub fn split_chat_layout(area: Rect, banner_visible: bool) -> ChatLayout {
    let eng_area = from_ratatui_rect(area);
    if banner_visible {
        let parts = split(
            eng_area,
            Direction::Vertical,
            &[
                Constraint::Flex(1),  // 会话流:占满剩余
                Constraint::Fixed(1), // ModeBanner 条件行
                Constraint::Fixed(3), // composer:3 行输入栏
                Constraint::Fixed(1), // statusline:1 行
            ],
        );
        ChatLayout {
            stream: to_ratatui_rect(parts[0]),
            banner: Some(to_ratatui_rect(parts[1])),
            composer: to_ratatui_rect(parts[2]),
            status: to_ratatui_rect(parts[3]),
        }
    } else {
        let parts = split(
            eng_area,
            Direction::Vertical,
            &[
                Constraint::Flex(1),  // 会话流:占满剩余
                Constraint::Fixed(3), // composer:3 行输入栏
                Constraint::Fixed(1), // statusline:1 行
            ],
        );
        ChatLayout {
            stream: to_ratatui_rect(parts[0]),
            banner: None,
            composer: to_ratatui_rect(parts[1]),
            status: to_ratatui_rect(parts[2]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_splits_into_three_regions() {
        let area = Rect::new(0, 0, 80, 24);
        let layout = split_chat_layout(area, false);
        // composer 3 行 + status 1 行固定,流占剩余 20 行
        assert_eq!(layout.stream.height, 20);
        assert_eq!(layout.composer.height, 3);
        assert_eq!(layout.status.height, 1);
        assert_eq!(layout.banner, None, "Normal 态无 banner 行");
        // 区域自上而下无重叠
        assert_eq!(layout.stream.y, 0);
        assert_eq!(layout.composer.y, 20);
        assert_eq!(layout.status.y, 23);
        // 宽度铺满
        assert_eq!(layout.stream.width, 80);
    }

    #[test]
    fn layout_with_banner_reserves_one_line() {
        let area = Rect::new(0, 0, 80, 24);
        let layout = split_chat_layout(area, true);
        let banner = layout.banner.expect("banner 行应存在");
        assert_eq!(banner.height, 1);
        // 守恒:四区总高 = 视口高;流被压缩 1 行
        assert_eq!(layout.stream.height, 19);
        assert_eq!(banner.y, 19);
        assert_eq!(layout.composer.y, 20);
        assert_eq!(layout.status.y, 23);
        assert_eq!(layout.composer.height, 3, "banner 不应挤压 composer");
        assert_eq!(layout.status.height, 1, "banner 不应挤压 statusline");
    }

    #[test]
    fn layout_small_viewport_no_panic() {
        // 极小视口不 panic,区域高度自然压缩
        let layout = split_chat_layout(Rect::new(0, 0, 20, 4), false);
        assert!(layout.status.height <= 1);
        let layout_b = split_chat_layout(Rect::new(0, 0, 20, 5), true);
        assert!(layout_b.banner.is_some());
    }
}
