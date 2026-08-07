//! 共享列表导航状态辅助函数
//!
//! 用于 Log / Parliament / Quest 等可滚动选择列表面板,消除重复的
//! 选中索引限制、滚动偏移调整、方向键/滚轮导航逻辑。
//!
//! # 设计决策(WHY)
//! - 三个面板原本各自持有 `selected` / `scroll_offset` 与完全相同的
//!   `clamp_selected` / `adjust_scroll` / Up/Down/滚轮处理代码。
//! - 提取为无状态辅助函数,避免改变各面板的字段布局,从而保持既有
//!   单元测试与外部调用者不变。

use crossterm::event::{KeyCode, MouseEventKind};

/// 将选中索引限制在 `[0, max)` 范围内。
///
/// 当 `max == 0` 时返回 0,表示没有可选项目。
pub fn clamp_selected(selected: usize, max: usize) -> usize {
    if max == 0 {
        0
    } else if selected >= max {
        max - 1
    } else {
        selected
    }
}

/// 根据当前选中项与可见行数调整滚动偏移,使选中项始终位于可视区域内。
pub fn adjust_scroll(selected: usize, scroll_offset: usize, visible_rows: usize) -> usize {
    if visible_rows == 0 {
        return scroll_offset;
    }
    if selected < scroll_offset {
        selected
    } else if selected >= scroll_offset + visible_rows {
        selected.saturating_sub(visible_rows - 1)
    } else {
        scroll_offset
    }
}

/// 根据方向键移动选中索引,返回新的索引。
///
/// 仅处理 `KeyCode::Up` / `KeyCode::Down`;其他按键返回 `None`,
/// 便于调用方继续处理 Enter / '?' 等面板特定按键。
pub fn handle_key_navigation(key: KeyCode, selected: usize, item_count: usize) -> Option<usize> {
    match key {
        KeyCode::Up => Some(move_selection(selected, -1, item_count)),
        KeyCode::Down => Some(move_selection(selected, 1, item_count)),
        _ => None,
    }
}

/// 翻页大小(行)— 单次 PgUp/PgDn 移动的行数
///
/// WHY 固定 10:`handle_key` 调用点不持有可见行数上下文(渲染时才拿到 area),
/// 用固定页大小提供一致的翻页体验,避免为每个面板引入"最近可见行数"状态。
pub const PAGE_SIZE: usize = 10;

/// 上翻一页:选中索引回退 `page_size` 行,钳制在 [0, item_count)。
pub fn page_up(selected: usize, page_size: usize, item_count: usize) -> usize {
    move_selection(selected, -(page_size as isize), item_count)
}

/// 下翻一页:选中索引前进 `page_size` 行,钳制在 [0, item_count)。
pub fn page_down(selected: usize, page_size: usize, item_count: usize) -> usize {
    move_selection(selected, page_size as isize, item_count)
}

/// 根据翻页键移动选中索引,返回新的索引。
///
/// 仅处理 `KeyCode::PageUp` / `KeyCode::PageDown`;其他按键返回 `None`,
/// 与 `handle_key_navigation` 一致,便于调用方继续处理面板特定按键。
pub fn handle_key_page_navigation(
    key: KeyCode,
    selected: usize,
    item_count: usize,
) -> Option<usize> {
    match key {
        KeyCode::PageUp => Some(page_up(selected, PAGE_SIZE, item_count)),
        KeyCode::PageDown => Some(page_down(selected, PAGE_SIZE, item_count)),
        _ => None,
    }
}

/// 根据鼠标滚轮事件移动选中索引,返回新的索引。
///
/// 仅处理 `ScrollUp` / `ScrollDown`;其他事件返回 `None`。
pub fn handle_mouse_scroll(
    kind: MouseEventKind,
    selected: usize,
    item_count: usize,
) -> Option<usize> {
    match kind {
        MouseEventKind::ScrollUp => Some(move_selection(selected, -1, item_count)),
        MouseEventKind::ScrollDown => Some(move_selection(selected, 1, item_count)),
        _ => None,
    }
}

/// 在 `item_count` 范围内按 `delta` 移动选中索引。
///
/// 当列表为空时保持为 0;不会越界。
pub fn move_selection(selected: usize, delta: isize, item_count: usize) -> usize {
    if item_count == 0 {
        return 0;
    }
    let new = if delta < 0 {
        selected.saturating_sub(delta.unsigned_abs())
    } else {
        selected.saturating_add(delta as usize)
    };
    clamp_selected(new, item_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_up_clamps_at_zero() {
        assert_eq!(page_up(0, 10, 25), 0);
        assert_eq!(page_up(5, 10, 25), 0);
        assert_eq!(page_up(12, 10, 25), 2);
    }

    #[test]
    fn page_down_clamps_at_last() {
        assert_eq!(page_down(0, 10, 25), 10);
        assert_eq!(page_down(20, 10, 25), 24);
        assert_eq!(page_down(30, 10, 25), 24);
    }

    #[test]
    fn page_navigation_empty_list_stays_zero() {
        assert_eq!(page_up(3, 10, 0), 0);
        assert_eq!(page_down(3, 10, 0), 0);
    }

    #[test]
    fn page_navigation_ignores_other_keys() {
        assert_eq!(handle_key_page_navigation(KeyCode::Enter, 1, 10), None);
        assert_eq!(handle_key_page_navigation(KeyCode::Up, 1, 10), None);
    }
}
