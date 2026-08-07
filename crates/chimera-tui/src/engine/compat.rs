//! engine::compat — ratatui ↔ 自研引擎边界翻译(ADR-029,v3.1 M1)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **边界翻译而非改面板签名**:现有 20 个面板均以 `Panel::render(state, area,
//!   buf)` 写入 ratatui `Buffer`。本模块在渲染完成后把 ratatui `Buffer` 逐格翻译
//!   为自研 `engine::Buffer`,交由自研 diff/writer 输出。面板代码与 ~40 测试**零改动**,
//!   这是"渐进迁移、随时可回滚"的关键(否决"自研 Buffer 暴露 ratatui API"方案:
//!   那需 20 面板 + 40 测试全部 re-import,回归风险极高)。
//! - **类型同构直译**:`engine::Rect`/`Color`/`Modifier` 刻意与 ratatui 对齐,
//!   翻译为穷尽 `match` / 逐位映射,零歧义、编译期完备(参照 writer.rs 反向映射)。
//! - **无损修饰**:M1.1 已将 `Modifier` 拓宽为 u16 全集,ratatui 9 种修饰可无损翻译。
//! - **宽字符(CJK/emoji)**:ratatui 用"skip cell"(空 symbol)占据宽字符的第 2 列。
//!   M1 自研 `Cell` 为单 `char`,skip cell 映射为空格(占位,避免错位);完整宽字符
//!   簇支持留待后续里程碑(见 buffer.rs `Cell` 设计注释)。

use ratatui::buffer::Buffer as RatBuffer;
use ratatui::layout::Rect as RatRect;
use ratatui::style::{Color as RatColor, Modifier as RatModifier, Style as RatStyle};

use crate::engine::buffer::{Buffer, Cell, DirtyTracker};
use crate::engine::diff::Change;
use crate::engine::rect::Rect;
use crate::engine::style::{Color, Modifier, Style};

/// 将 ratatui `Rect` 翻译为自研 `Rect`(字段同构直译)
pub fn from_ratatui_rect(r: RatRect) -> Rect {
    Rect::new(r.x, r.y, r.width, r.height)
}

/// 将自研 `Rect` 翻译为 ratatui `Rect`(供 M2 布局引擎向 ratatui 面板传递区域)
pub fn to_ratatui_rect(r: Rect) -> RatRect {
    RatRect::new(r.x, r.y, r.width, r.height)
}

/// 将 ratatui `Color` 翻译为自研 `Color`(1:1 穷尽映射)
pub fn from_ratatui_color(c: RatColor) -> Color {
    match c {
        RatColor::Reset => Color::Reset,
        RatColor::Black => Color::Black,
        RatColor::Red => Color::Red,
        RatColor::Green => Color::Green,
        RatColor::Yellow => Color::Yellow,
        RatColor::Blue => Color::Blue,
        RatColor::Magenta => Color::Magenta,
        RatColor::Cyan => Color::Cyan,
        RatColor::Gray => Color::Gray,
        RatColor::DarkGray => Color::DarkGray,
        RatColor::LightRed => Color::LightRed,
        RatColor::LightGreen => Color::LightGreen,
        RatColor::LightYellow => Color::LightYellow,
        RatColor::LightBlue => Color::LightBlue,
        RatColor::LightMagenta => Color::LightMagenta,
        RatColor::LightCyan => Color::LightCyan,
        RatColor::White => Color::White,
        RatColor::Indexed(i) => Color::Indexed(i),
        RatColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// 将 ratatui `Modifier` 翻译为自研 `Modifier`(逐位映射,9 种无损)
pub fn from_ratatui_modifier(m: RatModifier) -> Modifier {
    // 逐位检查 ratatui 修饰位,累积到自研 Modifier
    let pairs = [
        (RatModifier::BOLD, Modifier::BOLD),
        (RatModifier::DIM, Modifier::DIM),
        (RatModifier::ITALIC, Modifier::ITALIC),
        (RatModifier::UNDERLINED, Modifier::UNDERLINED),
        (RatModifier::REVERSED, Modifier::REVERSED),
        (RatModifier::CROSSED_OUT, Modifier::CROSSED_OUT),
        (RatModifier::SLOW_BLINK, Modifier::SLOW_BLINK),
        (RatModifier::RAPID_BLINK, Modifier::RAPID_BLINK),
        (RatModifier::HIDDEN, Modifier::HIDDEN),
    ];
    let mut out = Modifier::NONE;
    for (rat_bit, eng_bit) in pairs {
        if m.contains(rat_bit) {
            out = out.union(eng_bit);
        }
    }
    out
}

/// 将 ratatui `Style` 翻译为自研 `Style`(fg/bg/modifier 逐字段翻译)
pub fn from_ratatui_style(s: RatStyle) -> Style {
    Style {
        fg: s.fg.map(from_ratatui_color),
        bg: s.bg.map(from_ratatui_color),
        modifier: from_ratatui_modifier(s.add_modifier),
    }
}

/// 将 ratatui `Buffer` 整体翻译为自研 `Buffer`(逐格拷贝 symbol + style)
///
/// 前置:`rb.content()` 为行主序,长度 = `rb.area` 面积;翻译后 `engine::Buffer`
/// 覆盖相同区域(含 x/y 偏移),坐标语义一致。
pub fn from_ratatui_buffer(rb: &RatBuffer) -> Buffer {
    let area = from_ratatui_rect(rb.area);
    let mut buf = Buffer::empty(area);
    // 空区域(宽或高为 0):content() 为空,直接返回空缓冲,避免除零/越界
    if area.is_empty() {
        return buf;
    }
    let content = rb.content();
    let width = area.width as usize;
    for row in 0..area.height {
        // 行内宽字符追踪:前一格符号显示宽度 >= 2 时,当前格即其续格(ratatui
        // 0.29 不写 skip 标志,续格符号为空字符串,只能按宽度定位)。
        let mut prev_wide = false;
        for col in 0..area.width {
            let idx = (row as usize) * width + (col as usize);
            let Some(rcell) = content.get(idx) else {
                continue;
            };
            // 续格判定:前格为宽字符,或 ratatui 显式 skip(终端图形协议占位)
            let symbol = if prev_wide || rcell.skip {
                prev_wide = false;
                Cell::WIDE_CONTINUATION
            } else {
                let s = rcell.symbol();
                let ch = s.chars().next().unwrap_or(' ');
                // 显示宽度 >= 2(中文/全角/emoji)占据两列,下一格为续格
                prev_wide = unicode_width::UnicodeWidthStr::width(s) >= 2;
                ch
            };
            let style = from_ratatui_style(rcell.style());
            buf.set(area.x + col, area.y + row, Cell { symbol, style });
        }
    }
    buf
}

/// 单遍 compat + diff:逐格翻译 ratatui `Buffer` 并与引擎 `front` 比较,
/// 仅对变化格生成 `Change`(行内连续变化合并 Span),跳过中间 `Buffer` 构造。
///
/// # 前置
/// - `front.area` 与 `rb.area` 必须一致(区域变化由调用方走全量路径);
/// - `dirty` 为行级脏标记:clean 行假定与 `front` 相同,跳过翻译与比较
///   (调用方须保证该行内容确实未变,否则会漏输出导致渲染残影);
/// - 合并规则与 `diff.rs::coalesce` 一致:同行内 x 连续变化格合并为
///   `Change::Span`(≥2 格),单格保持 `Change::Cell`。
///
/// # 收益
/// 生产 v3-engine 路径原先执行“整帧 clone → 逐格转换 → 独立 diff 遍历”
/// 三次 O(W×H);本函数将转换与比较合并为单遍,且 clean 行零开销。
pub fn from_ratatui_buffer_diffed(
    front: &Buffer,
    rb: &RatBuffer,
    dirty: &DirtyTracker,
) -> Vec<Change> {
    let area = from_ratatui_rect(rb.area);
    let mut changes = Vec::new();
    // 同行内连续变化格 run(与 diff.rs 合并规则一致):
    // 单格 → Change::Cell(免 Vec 分配),≥2 格 → Change::Span(一次 MoveTo)
    let mut run: Vec<Cell> = Vec::new();
    let mut run_x = 0u16;
    let mut run_y = 0u16;

    let content = rb.content();
    let width = area.width as usize;
    for row in 0..area.height {
        // clean 行:假定与 front 相同,整行跳过(调用方保证不变量)
        if !dirty.is_dirty(row) {
            flush_run(&mut changes, run_x, run_y, &mut run);
            continue;
        }
        // 行内宽字符追踪:前一格显示宽度 >= 2 时,当前格即其续格
        // (与 from_ratatui_buffer 同一套规则,保证单遍转换结果一致)
        let mut prev_wide = false;
        for col in 0..area.width {
            let idx = (row as usize) * width + (col as usize);
            let Some(rcell) = content.get(idx) else {
                continue;
            };
            let symbol = if prev_wide || rcell.skip {
                prev_wide = false;
                Cell::WIDE_CONTINUATION
            } else {
                let s = rcell.symbol();
                let ch = s.chars().next().unwrap_or(' ');
                prev_wide = unicode_width::UnicodeWidthStr::width(s) >= 2;
                ch
            };
            let cell = Cell {
                symbol,
                style: from_ratatui_style(rcell.style()),
            };
            // 与 front 逐格比较:未变化格不产生输出
            if front.cells[idx] == cell {
                flush_run(&mut changes, run_x, run_y, &mut run);
                continue;
            }
            let ax = area.x + col;
            let ay = area.y + row;
            // 连续条件:同行且 x 紧接 run 起点 + 已累积长度
            let contiguous =
                !run.is_empty() && ay == run_y && ax == run_x.saturating_add(run.len() as u16);
            if !contiguous {
                flush_run(&mut changes, run_x, run_y, &mut run);
                run_x = ax;
                run_y = ay;
            }
            run.push(cell);
        }
        // 行尾 flush(跨行不合并)
        flush_run(&mut changes, run_x, run_y, &mut run);
    }
    changes
}

/// 将累积的连续变化格 flush 为 Change(规则与 diff.rs 一致),并清空 run
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

    #[test]
    fn rect_roundtrip() {
        let r = Rect::new(2, 3, 10, 5);
        assert_eq!(from_ratatui_rect(to_ratatui_rect(r)), r);
    }

    #[test]
    fn color_maps_one_to_one() {
        assert_eq!(from_ratatui_color(RatColor::Red), Color::Red);
        assert_eq!(from_ratatui_color(RatColor::Reset), Color::Reset);
        assert_eq!(
            from_ratatui_color(RatColor::Rgb(1, 2, 3)),
            Color::Rgb(1, 2, 3)
        );
        assert_eq!(
            from_ratatui_color(RatColor::Indexed(42)),
            Color::Indexed(42)
        );
    }

    #[test]
    fn modifier_maps_losslessly() {
        let rat = RatModifier::BOLD | RatModifier::CROSSED_OUT | RatModifier::HIDDEN;
        let eng = from_ratatui_modifier(rat);
        assert!(eng.contains(Modifier::BOLD));
        assert!(eng.contains(Modifier::CROSSED_OUT));
        assert!(eng.contains(Modifier::HIDDEN));
        assert!(!eng.contains(Modifier::ITALIC));
    }

    #[test]
    fn buffer_translation_preserves_symbols_and_style() {
        // 构造 ratatui Buffer,写入带样式的字符,翻译后逐格比对
        let area = RatRect::new(0, 0, 5, 2);
        let mut rb = RatBuffer::empty(area);
        rb.set_string(
            0,
            0,
            "hi",
            RatStyle::default()
                .fg(RatColor::Green)
                .add_modifier(RatModifier::BOLD),
        );
        let eng = from_ratatui_buffer(&rb);

        assert_eq!(eng.area, Rect::new(0, 0, 5, 2));
        let c0 = eng.get(0, 0).unwrap();
        assert_eq!(c0.symbol, 'h');
        assert_eq!(c0.style.fg, Some(Color::Green));
        assert!(c0.style.modifier.contains(Modifier::BOLD));
        // 未写入的格为空格默认样式
        assert_eq!(eng.get(4, 1).unwrap().symbol, ' ');
    }

    #[test]
    fn empty_area_returns_empty_buffer() {
        let rb = RatBuffer::empty(RatRect::new(0, 0, 0, 0));
        let eng = from_ratatui_buffer(&rb);
        assert!(eng.area.is_empty());
        assert!(eng.cells.is_empty());
    }

    #[test]
    fn wide_char_continuation_maps_to_sentinel() {
        use ratatui::widgets::Widget;

        // "中文" 在 ratatui Buffer 中占 4 列:中 + skip + 文 + skip
        let area = RatRect::new(0, 0, 6, 1);
        let mut rb = RatBuffer::empty(area);
        // WHY 用 Paragraph(与生产面板渲染路径一致):按宽度写入宽字符,
        // 续格符号为空字符串;compat 按"前格宽度 >= 2"定位续格。
        ratatui::widgets::Paragraph::new("中文").render(area, &mut rb);
        let eng = from_ratatui_buffer(&rb);
        assert_eq!(eng.get(0, 0).unwrap().symbol, '中');
        assert!(
            eng.get(1, 0).unwrap().is_wide_continuation(),
            "宽字符第 2 列应映射为续格哨兵(而非空格)"
        );
        assert_eq!(eng.get(2, 0).unwrap().symbol, '文');
        assert!(eng.get(3, 0).unwrap().is_wide_continuation());
        // 未写入格保持空格(非哨兵)
        assert_eq!(eng.get(5, 0).unwrap().symbol, ' ');
        assert!(!eng.get(5, 0).unwrap().is_wide_continuation());
    }
}
