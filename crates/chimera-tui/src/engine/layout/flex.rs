//! engine::layout::flex — 二维区域切分(ADR-029,v3.1 M1.4)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **无缝平铺**:`split` 将父区域沿主轴切成若干子区域,子区域首尾相接、完全
//!   覆盖父区域(不重叠、不越界、不留空),交叉轴取满父区域尺寸。是布局树递归
//!   切分的单层原语。
//! - **复用 1D 求解**:主轴长度分配委托 `constraint::solve`(和恒等于主轴总长),
//!   本模块只负责把一维长度序列铺成二维矩形。

use crate::engine::layout::constraint::{solve, Constraint, Direction};
use crate::engine::rect::Rect;

/// 沿 `direction` 将 `area` 切分为与 `constraints` 等长的子区域序列
///
/// 水平切分:各子区域等高(= area.height),宽度由 solve 分配,x 依次递进;
/// 垂直切分:各子区域等宽(= area.width),高度由 solve 分配,y 依次递进。
pub fn split(area: Rect, direction: Direction, constraints: &[Constraint]) -> Vec<Rect> {
    match direction {
        Direction::Horizontal => {
            let widths = solve(area.width, constraints);
            let mut x = area.x;
            widths
                .into_iter()
                .map(|w| {
                    let rect = Rect::new(x, area.y, w, area.height);
                    x = x.saturating_add(w);
                    rect
                })
                .collect()
        }
        Direction::Vertical => {
            let heights = solve(area.height, constraints);
            let mut y = area.y;
            heights
                .into_iter()
                .map(|h| {
                    let rect = Rect::new(area.x, y, area.width, h);
                    y = y.saturating_add(h);
                    rect
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn horizontal_split_tiles_width_and_keeps_height() {
        let area = Rect::new(0, 0, 100, 24);
        let parts = split(
            area,
            Direction::Horizontal,
            &[Constraint::Fixed(30), Constraint::Flex(1)],
        );
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], Rect::new(0, 0, 30, 24));
        assert_eq!(parts[1], Rect::new(30, 0, 70, 24));
        // 交叉轴(高度)取满
        assert!(parts.iter().all(|r| r.height == 24));
    }

    #[test]
    fn vertical_split_tiles_height_and_keeps_width() {
        let area = Rect::new(2, 3, 40, 20);
        let parts = split(
            area,
            Direction::Vertical,
            &[
                Constraint::Fixed(3),
                Constraint::Flex(1),
                Constraint::Fixed(1),
            ],
        );
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], Rect::new(2, 3, 40, 3));
        assert_eq!(parts[1], Rect::new(2, 6, 40, 16));
        assert_eq!(parts[2], Rect::new(2, 22, 40, 1));
    }

    proptest! {
        /// 不变量:水平切分后子区域首尾相接、覆盖父宽、不越界(布局正确性根本保证)
        #[test]
        fn horizontal_split_is_seamless_tiling(
            w in 1u16..300,
            h in 1u16..80,
            specs in prop::collection::vec(0u8..5, 1..6),
        ) {
            let area = Rect::new(0, 0, w, h);
            let constraints: Vec<Constraint> = specs
                .iter()
                .map(|&k| match k {
                    0 => Constraint::Fixed(7),
                    1 => Constraint::Percent(20),
                    2 => Constraint::Min(3),
                    3 => Constraint::Max(15),
                    _ => Constraint::Flex(1),
                })
                .collect();
            let parts = split(area, Direction::Horizontal, &constraints);

            // 覆盖父宽:各子宽之和 == area.width
            let sum_w: u32 = parts.iter().map(|r| r.width as u32).sum();
            prop_assert_eq!(sum_w, w as u32);
            // 首尾相接 + 不越界 + 等高
            let mut cursor = area.x;
            for r in &parts {
                prop_assert_eq!(r.x, cursor);
                prop_assert_eq!(r.y, area.y);
                prop_assert_eq!(r.height, area.height);
                prop_assert!(r.right() <= area.right());
                cursor = r.right();
            }
            prop_assert_eq!(cursor, area.right());
        }
    }
}
