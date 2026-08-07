//! engine::output — v3-engine 输出路径接线(M3,ADR-072 实施收口)
//!
//! 对应架构层:L10 Interface
//!
//! # 职责
//! `V3Output` 把"一帧自研 Buffer"转换为终端 ANSI 输出:
//! 与 `DoubleBuffer.front` 逐格 diff → `TerminalWriter` 写出变化 → swap。
//! 输出目标抽象为 `&mut dyn Write`,生产传 stdout,测试传 `Vec<u8>`。
//!
//! # 设计决策(WHY)
//! - **复用已就绪的引擎组件**:buffer/diff/writer 此前仅有测试引用,M3 让它们
//!   进入生产路径(评估遗留"机制存在、收益未兑现"的收口点)。
//! - **区域变化 = 全量重绘**:终端 resize 时 `front.area != back.area`,由
//!   `DiffEngine::full` 输出 back 全部单元格;同时重建双缓冲。
//! - **与 ratatui 的边界**:调用方负责把 ratatui 帧缓冲经 `from_ratatui_buffer`
//!   翻译为自研 Buffer(面板代码与测试零改动,渐进迁移可回滚)。

use std::io::{self, Write};

use super::buffer::{Buffer, DirtyTracker};
use super::compat::{from_ratatui_buffer, from_ratatui_buffer_diffed, from_ratatui_rect};
use super::diff::DiffEngine;
use super::writer::TerminalWriter;

/// v3-engine 输出状态 — 帧间双缓冲 + 首帧标记
#[derive(Debug)]
pub struct V3Output {
    /// front(已呈现)/ back(下一帧)双缓冲
    double: super::buffer::DoubleBuffer,
    /// 是否尚未完成首次输出(首帧强制全量,避免依赖未知终端残留)
    first: bool,
}

impl Default for V3Output {
    fn default() -> Self {
        Self::new()
    }
}

impl V3Output {
    /// 创建空输出状态(首帧按全量处理)
    pub fn new() -> Self {
        Self {
            double: super::buffer::DoubleBuffer::new(super::rect::Rect::default()),
            first: true,
        }
    }

    /// 渲染一帧:diff + 写出变化 + swap
    ///
    /// # 参数
    /// - `back`:本帧完整缓冲(调用方从 ratatui 帧经 compat 转换而来)
    /// - `out`:输出目标(stdout / 测试缓冲)
    ///
    /// 首帧或尺寸变化时输出全量;后续帧仅输出变化格(行内连续变化合并 Span)。
    pub fn render(&mut self, back: Buffer, out: &mut dyn Write) -> io::Result<()> {
        // 尺寸变化或首帧:重建双缓冲并对 back 全量输出
        let full = self.first || self.double.front().area != back.area;
        if self.double.front().area != back.area {
            self.double.resize(back.area);
        }

        let changes = if full {
            DiffEngine::full(&back)
        } else {
            DiffEngine::compute(self.double.front(), &back)
        };

        let mut writer = TerminalWriter::new(out);
        writer.render(&changes)?;

        // 本帧成为新基准(先写 back 再 swap,避免与渲染借用冲突)
        *self.double.back_mut() = back;
        self.double.swap();
        self.first = false;
        Ok(())
    }

    /// 单遍渲染:直接接收 ratatui 帧缓冲,内部完成 compat 转换 + diff 合并
    ///
    /// # 参数
    /// - `rb`:本帧 ratatui `Buffer`(面板渲染产物,尚未经 compat 转换)
    /// - `dirty`:行级脏标记,clean 行假定与已呈现帧相同(零转换/零比较开销);
    ///   `mark_all` 等价于全量比较(首帧/区域变化由内部自动走全量路径)
    /// - `out`:输出目标(stdout / 测试缓冲)
    ///
    /// # 与 `render` 的关系
    /// `render` 接收已转换的 `Buffer`(测试与兼容路径);本方法把 compat 与 diff
    /// 合并为单遍 O(未跳过行 × W),且 front 原地演进(免 back 构造与 swap),
    /// 是生产 v3-engine 路径的推荐入口(评估报告 P0-1)。
    pub fn render_diffed(
        &mut self,
        rb: &ratatui::buffer::Buffer,
        dirty: &DirtyTracker,
        out: &mut dyn Write,
    ) -> io::Result<()> {
        let area = from_ratatui_rect(rb.area);
        // 首帧或区域变化:全量路径(重建双缓冲 + 全量转换 + 全量输出)
        if self.first || self.double.front().area != area {
            if self.double.front().area != area {
                self.double.resize(area);
            }
            let back = from_ratatui_buffer(rb);
            let changes = DiffEngine::full(&back);
            let mut writer = TerminalWriter::new(out);
            writer.render(&changes)?;
            *self.double.back_mut() = back;
            self.double.swap();
            self.first = false;
            return Ok(());
        }
        // 增量:单遍 compat+diff(clean 行跳过),front 原地演进为最新帧
        let changes = from_ratatui_buffer_diffed(self.double.front(), rb, dirty);
        let mut writer = TerminalWriter::new(out);
        writer.render(&changes)?;
        DiffEngine::apply(&changes, self.double.front_mut());
        self.first = false;
        Ok(())
    }

    /// 当前已呈现帧的只读引用(测试断言用)
    pub fn front(&self) -> &Buffer {
        self.double.front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::buffer::Cell;
    use crate::engine::rect::Rect;

    fn frame(width: u16, height: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, width, height))
    }

    #[test]
    fn first_frame_writes_full_buffer() {
        let mut back = frame(4, 1);
        back.set(0, 0, Cell::new('X'));
        let mut out_state = V3Output::new();
        let mut sink = Vec::<u8>::new();
        out_state.render(back, &mut sink).unwrap();
        assert!(sink.contains(&b'X'), "首帧应输出全部内容");
    }

    #[test]
    fn identical_frame_produces_no_output() {
        let mut out_state = V3Output::new();
        let mut sink = Vec::<u8>::new();
        let mut f = frame(4, 1);
        f.set(0, 0, Cell::new('A'));
        out_state.render(f.clone(), &mut sink).unwrap();

        // 相同帧:diff 为空,写出空(仅 flush)
        let mut sink2 = Vec::<u8>::new();
        out_state.render(f, &mut sink2).unwrap();
        assert!(sink2.is_empty(), "相同帧不应输出任何变化");
    }

    #[test]
    fn changed_frame_outputs_only_delta() {
        let mut out_state = V3Output::new();
        let mut sink = Vec::<u8>::new();
        let mut f = frame(4, 1);
        f.set(0, 0, Cell::new('A'));
        out_state.render(f, &mut sink).unwrap();

        let mut f2 = frame(4, 1);
        f2.set(0, 0, Cell::new('B'));
        f2.set(1, 0, Cell::new('C'));
        let mut sink2 = Vec::<u8>::new();
        out_state.render(f2, &mut sink2).unwrap();
        assert!(
            sink2.contains(&b'B') && sink2.contains(&b'C'),
            "应输出变化格 B/C"
        );
        assert!(!sink2.contains(&b'A'), "不应重复输出未变化的旧内容 A");
    }

    #[test]
    fn resize_forces_full_redraw() {
        let mut out_state = V3Output::new();
        let mut sink = Vec::<u8>::new();
        out_state.render(frame(2, 1), &mut sink).unwrap();

        let mut back = frame(3, 1);
        back.set(2, 0, Cell::new('Z'));
        let mut sink2 = Vec::<u8>::new();
        out_state.render(back, &mut sink2).unwrap();
        assert!(sink2.contains(&b'Z'), "尺寸变化应全量输出新区域");
    }
}
