//! engine::rect — 几何基元(ADR-029,v3.1 自研渲染引擎 L4)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **与 ratatui 语义对齐**:`Rect`/`Size`/`Position` 字段与语义刻意对齐 ratatui,
//!   使 `engine::compat`(M1)桥接现有面板时零认知负担,现有测试可平滑迁移。
//! - **`u16` 坐标**:终端尺寸远小于 u16 上限,`u16` 足够且内存紧凑(利于 Cell 打包)。
//! - **饱和运算**:右/下边界用 `saturating_add` 防止 u16 溢出回绕导致越界 panic。

/// 二维尺寸(宽 × 高,单位:终端字符)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Size {
    /// 宽度(列数)
    pub width: u16,
    /// 高度(行数)
    pub height: u16,
}

impl Size {
    /// 构造尺寸
    pub const fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }
}

/// 二维坐标(列 x,行 y),原点在终端左上角
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Position {
    /// 列坐标(从 0 起)
    pub x: u16,
    /// 行坐标(从 0 起)
    pub y: u16,
}

impl Position {
    /// 构造坐标
    pub const fn new(x: u16, y: u16) -> Self {
        Self { x, y }
    }
}

/// 矩形区域 — 布局与渲染的基本单位
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    /// 左上角列坐标
    pub x: u16,
    /// 左上角行坐标
    pub y: u16,
    /// 宽度(列数)
    pub width: u16,
    /// 高度(行数)
    pub height: u16,
}

impl Rect {
    /// 构造矩形
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// 面积(单元格总数)——`u32` 避免 `u16 × u16` 溢出
    pub const fn area(&self) -> u32 {
        self.width as u32 * self.height as u32
    }

    /// 是否为空(宽或高为 0)
    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// 右边界(x + width,饱和防溢出)
    pub const fn right(&self) -> u16 {
        self.x.saturating_add(self.width)
    }

    /// 下边界(y + height,饱和防溢出)
    pub const fn bottom(&self) -> u16 {
        self.y.saturating_add(self.height)
    }

    /// 判断坐标是否落在矩形内(左闭右开区间)
    pub const fn contains(&self, pos: Position) -> bool {
        pos.x >= self.x && pos.x < self.right() && pos.y >= self.y && pos.y < self.bottom()
    }

    /// 求两矩形交集;无交集时返回零面积矩形
    ///
    /// WHY 交集:overlay/弹窗渲染需将子区域裁剪到父区域内,避免越界写入 Buffer。
    pub fn intersection(&self, other: Rect) -> Rect {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = self.right().min(other.right());
        let y2 = self.bottom().min(other.bottom());
        // 无交集时 x2<=x1 或 y2<=y1,用饱和减保证非负,得到零面积矩形
        Rect::new(x1, y1, x2.saturating_sub(x1), y2.saturating_sub(y1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_and_empty() {
        assert_eq!(Rect::new(0, 0, 80, 24).area(), 1920);
        assert!(Rect::new(0, 0, 0, 24).is_empty());
        assert!(!Rect::new(0, 0, 1, 1).is_empty());
    }

    #[test]
    fn contains_is_left_closed_right_open() {
        let r = Rect::new(2, 2, 3, 3); // 覆盖 x∈[2,5), y∈[2,5)
        assert!(r.contains(Position::new(2, 2)));
        assert!(r.contains(Position::new(4, 4)));
        assert!(!r.contains(Position::new(5, 4)));
        assert!(!r.contains(Position::new(1, 2)));
    }

    #[test]
    fn intersection_overlap_and_disjoint() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);
        assert_eq!(a.intersection(b), Rect::new(5, 5, 5, 5));
        // 不相交返回零面积
        let c = Rect::new(20, 20, 5, 5);
        assert!(a.intersection(c).is_empty());
    }

    #[test]
    fn right_bottom_saturate() {
        let r = Rect::new(u16::MAX - 1, u16::MAX - 1, 10, 10);
        assert_eq!(r.right(), u16::MAX);
        assert_eq!(r.bottom(), u16::MAX);
    }
}
