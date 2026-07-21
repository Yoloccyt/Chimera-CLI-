//! engine::writer — 变化列表输出到终端(ADR-029,v3.1 自研渲染引擎 L2 边界)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **保留 crossterm 作终端后端**:`#![forbid(unsafe_code)]` 铁律下,raw mode /
//!   ANSI 序列 / 跨平台差异由 crossterm(其 unsafe 封装在自身 crate)承担,
//!   自研引擎只负责"算出变化",输出交给 crossterm,不重写 TTY 层。
//! - **泛型 `W: Write`**:输出目标抽象为 `io::Write`,生产用 stdout,测试用
//!   `Vec<u8>` 断言 ANSI 输出,无需真实终端(契合 CI 无 TTY)。
//! - **M1.2 样式去重 + Span**:帧内追踪 `current_style`,仅当相邻格样式变化
//!   时才输出 Reset+SetStyle;`Change::Span` 一次 MoveTo 连续输出整行 run。
//!   两项合计在低带宽场景显著减少 ANSI 字节与光标移动指令。

use std::io::{self, Write};

use crossterm::style::{
    Attribute, Color as CtColor, Print, ResetColor, SetAttribute, SetBackgroundColor,
    SetForegroundColor,
};
use crossterm::{cursor::MoveTo, queue};

use crate::engine::diff::Change;
use crate::engine::style::{Color, Modifier, Style};

/// 终端写出器 — 将 `Change` 列表转为 ANSI 序列写入输出目标
#[derive(Debug)]
pub struct TerminalWriter<W: Write> {
    /// 输出目标(stdout / 测试缓冲)
    out: W,
    /// 当前终端样式状态(帧内追踪,仅样式变化时才输出 SetStyle)
    ///
    /// WHY `Option`:每帧起点置 `None`,强制首个输出格写全样式,
    /// 不依赖上一帧残留的终端状态。
    current_style: Option<Style>,
}

impl<W: Write> TerminalWriter<W> {
    /// 以指定输出目标构造
    pub fn new(out: W) -> Self {
        Self {
            out,
            current_style: None,
        }
    }

    /// 渲染一批变化到终端并 flush
    ///
    /// 处理 `Change::Cell`(单格:MoveTo + 样式 + Print)与 `Change::Span`
    /// (一次 MoveTo + 连续 Print,终端光标自动后移);样式经 `current_style`
    /// 去重,连续同样式格不重复输出 SetStyle。
    pub fn render(&mut self, changes: &[Change]) -> io::Result<()> {
        // 每帧起点重置样式追踪:首个输出格强制写全样式
        self.current_style = None;
        for change in changes {
            match change {
                Change::Cell { x, y, cell } => {
                    queue!(self.out, MoveTo(*x, *y))?;
                    self.apply_style_if_changed(cell.style)?;
                    queue!(self.out, Print(cell.symbol))?;
                }
                Change::Span { x, y, cells } => {
                    // 一次 MoveTo 定位行首,随后逐格 Print(终端光标自动后移)
                    queue!(self.out, MoveTo(*x, *y))?;
                    for cell in cells {
                        self.apply_style_if_changed(cell.style)?;
                        queue!(self.out, Print(cell.symbol))?;
                    }
                }
            }
        }
        self.out.flush()
    }

    /// 仅当样式与当前追踪不同时输出:先 Reset 清残留,再设置 fg/bg/修饰
    ///
    /// WHY 变化时全量 Reset:从 BOLD 切到非 BOLD 等需先 Reset 才能清除旧修饰,
    /// 避免属性渗染;连续同样式时直接返回,不输出任何样式序列。
    fn apply_style_if_changed(&mut self, style: Style) -> io::Result<()> {
        if self.current_style == Some(style) {
            return Ok(());
        }
        queue!(self.out, SetAttribute(Attribute::Reset), ResetColor)?;
        if let Some(fg) = style.fg {
            queue!(self.out, SetForegroundColor(to_ct_color(fg)))?;
        }
        if let Some(bg) = style.bg {
            queue!(self.out, SetBackgroundColor(to_ct_color(bg)))?;
        }
        apply_modifiers(&mut self.out, style.modifier)?;
        self.current_style = Some(style);
        Ok(())
    }

    /// 取回底层输出目标(测试用:断言写入内容)
    pub fn into_inner(self) -> W {
        self.out
    }
}

/// 将引擎 `Color` 映射为 crossterm `Color`
fn to_ct_color(c: Color) -> CtColor {
    match c {
        Color::Reset => CtColor::Reset,
        Color::Black => CtColor::Black,
        Color::Red => CtColor::DarkRed,
        Color::Green => CtColor::DarkGreen,
        Color::Yellow => CtColor::DarkYellow,
        Color::Blue => CtColor::DarkBlue,
        Color::Magenta => CtColor::DarkMagenta,
        Color::Cyan => CtColor::DarkCyan,
        Color::Gray => CtColor::Grey,
        Color::DarkGray => CtColor::DarkGrey,
        Color::LightRed => CtColor::Red,
        Color::LightGreen => CtColor::Green,
        Color::LightYellow => CtColor::Yellow,
        Color::LightBlue => CtColor::Blue,
        Color::LightMagenta => CtColor::Magenta,
        Color::LightCyan => CtColor::Cyan,
        Color::White => CtColor::White,
        Color::Indexed(i) => CtColor::AnsiValue(i),
        Color::Rgb(r, g, b) => CtColor::Rgb { r, g, b },
    }
}

/// 逐位应用文本修饰(加粗/暗淡/斜体/下划线/反色)
fn apply_modifiers<W: Write>(out: &mut W, m: Modifier) -> io::Result<()> {
    if m.contains(Modifier::BOLD) {
        queue!(out, SetAttribute(Attribute::Bold))?;
    }
    if m.contains(Modifier::DIM) {
        queue!(out, SetAttribute(Attribute::Dim))?;
    }
    if m.contains(Modifier::ITALIC) {
        queue!(out, SetAttribute(Attribute::Italic))?;
    }
    if m.contains(Modifier::UNDERLINED) {
        queue!(out, SetAttribute(Attribute::Underlined))?;
    }
    if m.contains(Modifier::REVERSED) {
        queue!(out, SetAttribute(Attribute::Reverse))?;
    }
    if m.contains(Modifier::CROSSED_OUT) {
        queue!(out, SetAttribute(Attribute::CrossedOut))?;
    }
    if m.contains(Modifier::SLOW_BLINK) {
        queue!(out, SetAttribute(Attribute::SlowBlink))?;
    }
    if m.contains(Modifier::RAPID_BLINK) {
        queue!(out, SetAttribute(Attribute::RapidBlink))?;
    }
    if m.contains(Modifier::HIDDEN) {
        queue!(out, SetAttribute(Attribute::Hidden))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::buffer::Cell;

    #[test]
    fn render_writes_symbol_to_sink() {
        let changes = vec![Change::Cell {
            x: 3,
            y: 1,
            cell: Cell::new('Z'),
        }];
        let mut writer = TerminalWriter::new(Vec::<u8>::new());
        writer.render(&changes).expect("render 应成功写入内存缓冲");
        let out = writer.into_inner();
        // 输出应包含被打印的字符 'Z'(ANSI 序列 + 字符字节)
        assert!(out.contains(&b'Z'), "输出应包含渲染的字符 Z");
        assert!(!out.is_empty());
    }

    #[test]
    fn empty_changes_produce_no_error() {
        let mut writer = TerminalWriter::new(Vec::<u8>::new());
        assert!(writer.render(&[]).is_ok());
    }

    #[test]
    fn span_renders_all_symbols_in_order() {
        // Span 应输出其全部连续字符
        let changes = vec![Change::Span {
            x: 0,
            y: 0,
            cells: vec![Cell::new('a'), Cell::new('b'), Cell::new('c')],
        }];
        let mut writer = TerminalWriter::new(Vec::<u8>::new());
        writer.render(&changes).unwrap();
        let out = writer.into_inner();
        assert!(out.contains(&b'a') && out.contains(&b'b') && out.contains(&b'c'));
    }

    #[test]
    fn uniform_style_span_uses_fewer_bytes_than_per_cell() {
        use crate::engine::style::{Color, Style};
        // 构造统一样式(红色前景)的单元格
        let styled = |c: char| Cell {
            symbol: c,
            style: Style::new().fg(Color::Red),
        };
        // 方案 A:合并为一个 Span(一次 MoveTo + 一次样式)
        let span = vec![Change::Span {
            x: 0,
            y: 0,
            cells: vec![styled('a'), styled('b'), styled('c'), styled('d')],
        }];
        // 方案 B:拆为四个独立单格(四次 MoveTo)
        let per_cell: Vec<Change> = "abcd"
            .chars()
            .enumerate()
            .map(|(i, ch)| Change::Cell {
                x: i as u16,
                y: 0,
                cell: styled(ch),
            })
            .collect();

        let mut w1 = TerminalWriter::new(Vec::<u8>::new());
        w1.render(&span).unwrap();
        let span_bytes = w1.into_inner();

        let mut w2 = TerminalWriter::new(Vec::<u8>::new());
        w2.render(&per_cell).unwrap();
        let cell_bytes = w2.into_inner();

        // Span 只需一次 MoveTo,字节数应少于逐格(验证光标移动去重收益)
        assert!(
            span_bytes.len() < cell_bytes.len(),
            "Span 去重后字节({})应少于逐格({})",
            span_bytes.len(),
            cell_bytes.len()
        );
        // 内容正确:四个字符均在
        for ch in *b"abcd" {
            assert!(span_bytes.contains(&ch), "Span 输出应含 {}", ch as char);
        }
    }
}
