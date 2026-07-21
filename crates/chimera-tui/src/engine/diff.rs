//! engine::diff — 双缓冲差异计算(ADR-029,v3.1 自研渲染引擎 L3)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **最小化重绘**:逐单元格比较 front/back,仅对变化单元格生成 `Change`,
//!   低带宽环境下把输出量从整帧降到"变化格数"(§9.2 核心)。
//! - **Span 合并(M1.2)**:同一行内 x 连续的变化格合并为 `Change::Span`(≥2 格),
//!   使 writer 只需一次光标移动(MoveTo)即可连续输出整个 run。单格变化仍为
//!   `Change::Cell`,避免为单格引入 Vec 分配开销。
//! - **区域不一致 = 全量重绘**:front/back 区域不同(终端 resize)时无法逐格对应,
//!   直接输出 back 全部单元格(同样按行合并 Span),由 writer 全量刷新。
//! - **`apply` 供测试与回退**:将 `Change` 应用回 Buffer,用于 proptest 幂等校验
//!   (应用 diff 后 front == back)与未来 full-redraw fallback。

use crate::engine::buffer::{Buffer, Cell};

/// 单元格变化 — diff 的最小输出单元
///
/// WHY 携带完整目标单元格:writer 直接据此定位光标、设置样式、写字符,
/// 无需回查 back 缓冲,输出阶段与 diff 阶段解耦。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// 单元格变化:在 (x, y) 写入 symbol + style
    Cell {
        /// 列坐标(绝对)
        x: u16,
        /// 行坐标(绝对)
        y: u16,
        /// 目标单元格内容
        cell: Cell,
    },
    /// 连续变化段:同一行 `y` 上从 `x` 开始的一段 x 连续单元格(M1.2)
    ///
    /// WHY 携带 `cells` 而非只存长度:writer 需逐格取 symbol+style,且 span 内
    /// 各格样式可不同(writer 内部再做样式分段去重)。长度 = `cells.len()`。
    Span {
        /// 起始列坐标(绝对)
        x: u16,
        /// 行坐标(绝对)
        y: u16,
        /// 从 x 开始的连续单元格(终端光标自动后移,无需逐格 MoveTo)
        cells: Vec<Cell>,
    },
}

/// 差异引擎 — 无状态,`compute`/`apply` 为纯函数
#[derive(Debug, Clone, Copy, Default)]
pub struct DiffEngine;

impl DiffEngine {
    /// 计算 front → back 的差异变化列表(同行 x 连续变化格合并为 Span)
    ///
    /// 前置:通常 front/back 区域一致(同一 DoubleBuffer);区域不一致时
    /// 退化为 back 全量输出(终端 resize 场景)。合并规则:同行内 x 连续的
    /// 变化格合并为 `Change::Span`(≥2 格),单格仍为 `Change::Cell`。
    pub fn compute(front: &Buffer, back: &Buffer) -> Vec<Change> {
        // 区域不一致:无法逐格对应,输出 back 全部单元格做全量重绘
        if front.area != back.area {
            return Self::full(back);
        }
        // 仅对 fc != bc 的格累积成 run;遇未变格或行切换时 flush
        Self::coalesce(back, |i| front.cells[i] != back.cells[i])
    }

    /// 输出缓冲全部单元格为变化(全量重绘,同样按行合并 Span)
    fn full(buf: &Buffer) -> Vec<Change> {
        // 全量重绘:每个格都视为"变化",按行合并为 Span
        Self::coalesce(buf, |_| true)
    }

    /// 按行扫描 `buf`,对 `is_changed(i)` 为真的格累积成连续 run 并 flush 为 Change
    ///
    /// WHY 抽取:增量 diff 与全量重绘仅"哪些格算变化"不同,run 合并逻辑完全一致,
    /// 提取为单一扫描避免重复实现。行主序下下标 i 还原相对 (x, y):
    /// 用 usize 取模/除避免 `i as u16` 在大缓冲下回绕。
    fn coalesce(buf: &Buffer, is_changed: impl Fn(usize) -> bool) -> Vec<Change> {
        let width = buf.area.width as usize;
        if width == 0 {
            return Vec::new();
        }
        let base_x = buf.area.x;
        let base_y = buf.area.y;
        let mut changes = Vec::new();
        let mut run: Vec<Cell> = Vec::new();
        let mut run_x = 0u16;
        let mut run_y = 0u16;
        for (i, cell) in buf.cells.iter().enumerate() {
            if !is_changed(i) {
                flush_run(&mut changes, run_x, run_y, &mut run);
                continue;
            }
            let ax = base_x + (i % width) as u16;
            let ay = base_y + (i / width) as u16;
            // 连续条件:同行(ay==run_y)且 x 紧接(ax == 起点 + 已累积长度)
            let contiguous =
                !run.is_empty() && ay == run_y && ax == run_x.saturating_add(run.len() as u16);
            if !contiguous {
                flush_run(&mut changes, run_x, run_y, &mut run);
                run_x = ax;
                run_y = ay;
            }
            run.push(cell.clone());
        }
        flush_run(&mut changes, run_x, run_y, &mut run);
        changes
    }

    /// 将变化列表应用到目标缓冲(测试幂等校验 / full-redraw fallback 用)
    pub fn apply(changes: &[Change], target: &mut Buffer) {
        for change in changes {
            match change {
                Change::Cell { x, y, cell } => target.set(*x, *y, cell.clone()),
                Change::Span { x, y, cells } => {
                    for (offset, cell) in cells.iter().enumerate() {
                        target.set(x.saturating_add(offset as u16), *y, cell.clone());
                    }
                }
            }
        }
    }
}

/// 将累积的连续变化格 flush 为 Change(1 格 → Cell,≥2 格 → Span),并清空 run
///
/// WHY 单格保留 Cell:避免为单格变化引入 Vec 分配开销,且保持既有单格语义
/// (现有测试依赖单格变化仍为 Change::Cell)。
fn flush_run(changes: &mut Vec<Change>, x: u16, y: u16, run: &mut Vec<Cell>) {
    let cells = std::mem::take(run);
    match cells.len() {
        0 => {}
        1 => changes.push(Change::Cell {
            x,
            y,
            cell: cells.into_iter().next().expect("len==1"),
        }),
        _ => changes.push(Change::Span { x, y, cells }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::rect::Rect;
    use proptest::prelude::*;

    /// 统计一个 Change 覆盖的单元格数(Cell=1,Span=cells.len())
    fn covered_cells(c: &Change) -> usize {
        match c {
            Change::Cell { .. } => 1,
            Change::Span { cells, .. } => cells.len(),
        }
    }

    #[test]
    fn identical_buffers_produce_no_changes() {
        let a = Buffer::empty(Rect::new(0, 0, 4, 2));
        let b = Buffer::empty(Rect::new(0, 0, 4, 2));
        assert!(DiffEngine::compute(&a, &b).is_empty());
    }

    #[test]
    fn single_cell_change_detected_at_correct_coords() {
        let front = Buffer::empty(Rect::new(0, 0, 4, 2));
        let mut back = Buffer::empty(Rect::new(0, 0, 4, 2));
        back.set(2, 1, Cell::new('Z'));
        let changes = DiffEngine::compute(&front, &back);
        assert_eq!(changes.len(), 1);
        // 单格变化仍为 Change::Cell(非 Span)
        match &changes[0] {
            Change::Cell { x, y, cell } => {
                assert_eq!((*x, *y), (2, 1));
                assert_eq!(cell.symbol, 'Z');
            }
            other => panic!("单格变化应为 Change::Cell,实际: {other:?}"),
        }
    }

    #[test]
    fn contiguous_changes_merge_into_span() {
        // 位置 1/2/3 连续变化 → 一个 Span;位置 4 不变;位置 5 单独变化 → 一个 Cell
        let front = Buffer::empty(Rect::new(0, 0, 6, 1));
        let mut back = Buffer::empty(Rect::new(0, 0, 6, 1));
        back.set(1, 0, Cell::new('a'));
        back.set(2, 0, Cell::new('b'));
        back.set(3, 0, Cell::new('c'));
        back.set(5, 0, Cell::new('z'));
        let changes = DiffEngine::compute(&front, &back);
        assert_eq!(changes.len(), 2, "应为一个 Span + 一个 Cell");
        match &changes[0] {
            Change::Span { x, y, cells } => {
                assert_eq!((*x, *y), (1, 0));
                assert_eq!(cells.len(), 3);
                assert_eq!(cells[0].symbol, 'a');
                assert_eq!(cells[2].symbol, 'c');
            }
            other => panic!("连续 3 格应合并为 Span,实际: {other:?}"),
        }
        match &changes[1] {
            Change::Cell { x, y, cell } => {
                assert_eq!((*x, *y), (5, 0));
                assert_eq!(cell.symbol, 'z');
            }
            other => panic!("孤立单格应为 Cell,实际: {other:?}"),
        }
    }

    #[test]
    fn area_mismatch_triggers_full_redraw() {
        let front = Buffer::empty(Rect::new(0, 0, 2, 2));
        let back = Buffer::empty(Rect::new(0, 0, 4, 4));
        // 区域不同 → 全量输出 back 全部 16 格(按行合并为 Span 后,覆盖格数仍为 16)
        let changes = DiffEngine::compute(&front, &back);
        let covered: usize = changes.iter().map(covered_cells).sum();
        assert_eq!(covered, 16, "全量重绘应覆盖全部 16 格");
        // 4×4 全量重绘 → 每行 4 格合并为 1 个 Span,共 4 个 Span
        assert_eq!(changes.len(), 4, "每行应合并为单个 Span");
    }

    proptest! {
        /// 幂等性:对 front 应用 (front→back) 的 diff 后,结果必等于 back
        ///
        /// WHY 该不变量:是差异渲染正确性的根本保证——只要它成立,
        /// "只输出变化"就永远等价于"输出整帧",不会产生渲染残影。
        #[test]
        fn diff_then_apply_reconstructs_back(
            front_syms in prop::collection::vec(0usize..4, 32),
            back_syms in prop::collection::vec(0usize..4, 32),
        ) {
            const ALPHABET: [char; 4] = ['a', 'b', 'c', ' '];
            let area = Rect::new(0, 0, 8, 4); // 8×4 = 32 格
            let mut front = Buffer::empty(area);
            let mut back = Buffer::empty(area);
            for (i, &s) in front_syms.iter().enumerate() {
                front.cells[i] = Cell::new(ALPHABET[s]);
            }
            for (i, &s) in back_syms.iter().enumerate() {
                back.cells[i] = Cell::new(ALPHABET[s]);
            }
            let changes = DiffEngine::compute(&front, &back);
            let mut reconstructed = front.clone();
            DiffEngine::apply(&changes, &mut reconstructed);
            prop_assert_eq!(reconstructed, back);
        }
    }
}
