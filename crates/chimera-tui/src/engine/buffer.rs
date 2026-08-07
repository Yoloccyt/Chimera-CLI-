//! engine::buffer — 单元格缓冲与双缓冲脏行追踪(ADR-029,v3.1 自研渲染引擎 L3)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **行主序 `Vec<Cell>`**:缓存友好,`index = y * width + x`,顺序 diff 时命中率高。
//! - **双缓冲 + swap**:back 缓冲渲染下一帧,与 front 逐格 diff 后仅输出变化,
//!   交换缓冲避免每帧重新分配(§9.2 RenderOptimizer)。
//! - **DirtyTracker 行级脏标记**:流式 token 只标记光标行 dirty,渲染时非脏行
//!   直接复用,把重绘量从 O(W×H) 降到 O(dirty_rows × W)(§B 性能视角)。
//! - **Cell 内联 Style(M1 门禁决策保留)**:benchmark-first 实测
//!   `diff_incremental_5pct@80×24` 约 16µs(远低于 100µs 目标),故 M1 **不**做
//!   Cell→u16 StylePool 索引化(遵循"性能可证伪 + 不过度工程化");若未来
//!   更大终端/更密样式使 diff 逼近预算,再引 `StylePool`(设计见 style.rs)。

use crate::engine::rect::Rect;
use crate::engine::style::Style;

/// 单元格 — 终端一个字符位的内容与样式
///
/// WHY `symbol: char`:M0 每格单字符,暂不支持宽字符簇(emoji/CJK 组合);
/// M1 视需要升级为 `SmolStr`/宽度标记。默认空格 + 默认样式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// 字符内容
    pub symbol: char,
    /// 样式
    pub style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            symbol: ' ',
            style: Style::new(),
        }
    }
}

impl Cell {
    /// 宽字符续格哨兵(M3 输出接线前置)
    ///
    /// ratatui 用"空 symbol"占据宽字符(CJK/emoji)第 2 列;compat 转换时把该
    /// 续格映射为本哨兵,`TerminalWriter` 渲染时跳过不输出。WHY 不用空格:
    /// 若把续格映射为空格,diff 会在宽字符后输出空格,终端光标已位于宽字符
    /// 右列之后,回移打印空格会覆盖汉字右半格(默认中文 UI 必现的渲染破损)。
    pub const WIDE_CONTINUATION: char = '\0';

    /// 以字符构造(默认样式)
    pub fn new(symbol: char) -> Self {
        Self {
            symbol,
            style: Style::new(),
        }
    }

    /// 重置为默认(空格 + 默认样式)
    pub fn reset(&mut self) {
        self.symbol = ' ';
        self.style = Style::new();
    }

    /// 是否为宽字符续格哨兵(writer 渲染时应跳过输出)
    pub fn is_wide_continuation(&self) -> bool {
        self.symbol == Self::WIDE_CONTINUATION
    }
}

/// 单元格缓冲区 — 覆盖一个矩形区域的字符网格
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Buffer {
    /// 缓冲覆盖的区域
    pub area: Rect,
    /// 行主序单元格数组(长度 = area.area())
    pub cells: Vec<Cell>,
}

impl Buffer {
    /// 创建覆盖指定区域的空缓冲(全部默认单元格)
    pub fn empty(area: Rect) -> Self {
        let len = area.area() as usize;
        Self {
            area,
            cells: vec![Cell::default(); len],
        }
    }

    /// 将 (x, y) 绝对坐标转为 cells 下标;越界返回 None(防御边界)
    ///
    /// WHY 相对 area 原点:调用方用绝对终端坐标,内部换算为相对下标,
    /// 保证子区域渲染时坐标语义统一。
    fn index_of(&self, x: u16, y: u16) -> Option<usize> {
        if x < self.area.x || y < self.area.y || x >= self.area.right() || y >= self.area.bottom() {
            return None;
        }
        let rel_x = (x - self.area.x) as usize;
        let rel_y = (y - self.area.y) as usize;
        Some(rel_y * self.area.width as usize + rel_x)
    }

    /// 读取指定坐标单元格(越界返回 None)
    pub fn get(&self, x: u16, y: u16) -> Option<&Cell> {
        self.index_of(x, y).map(|i| &self.cells[i])
    }

    /// 设置指定坐标单元格(越界静默忽略,不 panic)
    ///
    /// WHY 越界忽略:渲染裁剪后偶发越界写入是常见情形(如文本超出面板),
    /// 忽略比 panic 更健壮,由布局层负责裁剪。
    pub fn set(&mut self, x: u16, y: u16, cell: Cell) {
        if let Some(i) = self.index_of(x, y) {
            self.cells[i] = cell;
        }
    }

    /// 在 (x, y) 起始横向写入字符串(超出右边界截断),统一样式
    ///
    /// 返回实际写入的下一个 x 坐标(便于连续写入)。
    pub fn set_string(&mut self, x: u16, y: u16, s: &str, style: Style) -> u16 {
        let mut cur_x = x;
        for ch in s.chars() {
            if cur_x >= self.area.right() {
                break;
            }
            self.set(cur_x, y, Cell { symbol: ch, style });
            cur_x = cur_x.saturating_add(1);
        }
        cur_x
    }

    /// 重置全部单元格为默认(每帧渲染前清空 back 缓冲)
    pub fn reset(&mut self) {
        for c in &mut self.cells {
            c.reset();
        }
    }

    /// 调整缓冲到新区域(尺寸变化时重建,内容丢弃)
    pub fn resize(&mut self, area: Rect) {
        self.area = area;
        self.cells = vec![Cell::default(); area.area() as usize];
    }
}

/// 双缓冲 — front(已呈现)/ back(下一帧),diff 后交换
#[derive(Debug, Clone)]
pub struct DoubleBuffer {
    front: Buffer,
    back: Buffer,
}

impl DoubleBuffer {
    /// 创建覆盖指定区域的双缓冲
    pub fn new(area: Rect) -> Self {
        Self {
            front: Buffer::empty(area),
            back: Buffer::empty(area),
        }
    }

    /// 当前已呈现帧(diff 的基准)
    pub fn front(&self) -> &Buffer {
        &self.front
    }

    /// 当前已呈现帧的可变引用(单遍 diff 后原地演进为最新帧,免 swap)
    pub fn front_mut(&mut self) -> &mut Buffer {
        &mut self.front
    }

    /// 下一帧可变引用(组件渲染写入目标)
    pub fn back_mut(&mut self) -> &mut Buffer {
        &mut self.back
    }

    /// 清空 back 缓冲(每帧渲染前调用)
    pub fn clear_back(&mut self) {
        self.back.reset();
    }

    /// 交换 front/back(输出 diff 后调用,使 back 成为新基准)
    pub fn swap(&mut self) {
        std::mem::swap(&mut self.front, &mut self.back);
    }

    /// 尺寸变化时重建双缓冲
    pub fn resize(&mut self, area: Rect) {
        self.front.resize(area);
        self.back.resize(area);
    }
}

/// 脏行追踪器 — 面板级行粒度失效标记
///
/// WHY 行粒度:比面板级更细(减少无变化行的重绘),比单元格级更省(位向量小、
/// 更新快)。流式 token 追加只标记光标行,增量渲染代价 O(1 行)。
#[derive(Debug, Clone, Default)]
pub struct DirtyTracker {
    /// 每行是否脏(下标 = 相对行号)
    rows: Vec<bool>,
}

impl DirtyTracker {
    /// 创建覆盖 `height` 行的追踪器(初始全脏,保证首帧完整渲染)
    pub fn new(height: u16) -> Self {
        Self {
            rows: vec![true; height as usize],
        }
    }

    /// 标记某行为脏(越界忽略)
    pub fn mark(&mut self, row: u16) {
        if let Some(slot) = self.rows.get_mut(row as usize) {
            *slot = true;
        }
    }

    /// 标记全部行为脏(如主题/locale 切换需全量重绘)
    pub fn mark_all(&mut self) {
        for r in &mut self.rows {
            *r = true;
        }
    }

    /// 查询某行是否脏(越界视为不脏)
    pub fn is_dirty(&self, row: u16) -> bool {
        self.rows.get(row as usize).copied().unwrap_or(false)
    }

    /// 是否存在任何脏行
    pub fn any(&self) -> bool {
        self.rows.iter().any(|&d| d)
    }

    /// 清除全部脏标记(本帧渲染完成后调用)
    pub fn clear(&mut self) {
        for r in &mut self.rows {
            *r = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_and_out_of_bounds() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        buf.set(1, 1, Cell::new('X'));
        assert_eq!(buf.get(1, 1).unwrap().symbol, 'X');
        // 越界写入被忽略,读取越界返回 None
        buf.set(10, 10, Cell::new('Y'));
        assert!(buf.get(10, 10).is_none());
    }

    #[test]
    fn set_string_truncates_at_right_edge() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        let next = buf.set_string(0, 0, "hello", Style::new());
        assert_eq!(next, 3); // 只写下 3 个字符
        assert_eq!(buf.get(0, 0).unwrap().symbol, 'h');
        assert_eq!(buf.get(2, 0).unwrap().symbol, 'l');
    }

    #[test]
    fn double_buffer_swap_exchanges_front_back() {
        let mut db = DoubleBuffer::new(Rect::new(0, 0, 2, 1));
        db.back_mut().set(0, 0, Cell::new('A'));
        db.swap();
        // swap 后原 back 成为 front
        assert_eq!(db.front().get(0, 0).unwrap().symbol, 'A');
    }

    #[test]
    fn dirty_tracker_marks_and_clears() {
        let mut dt = DirtyTracker::new(3);
        assert!(dt.any()); // 初始全脏
        dt.clear();
        assert!(!dt.any());
        dt.mark(1);
        assert!(dt.is_dirty(1));
        assert!(!dt.is_dirty(0));
    }
}
