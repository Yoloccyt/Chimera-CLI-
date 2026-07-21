//! engine::style — 颜色 / 修饰 / 样式与去重池(ADR-029,v3.1 自研渲染引擎 L3)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **纯 safe Rust**:`#![forbid(unsafe_code)]` 铁律下,`Modifier` 用 `u8` 位标志
//!   手写实现(不引 bitflags 依赖),`Color` 为普通枚举,零 unsafe。
//! - **StylePool 去重**:运行时典型 TUI 仅 20-40 种唯一样式,池化为 `u16` 索引后,
//!   `Cell` 可只存 2 字节样式索引而非内联完整 `Style`(M1 Buffer 优化的前置)。
//! - **`patch` 合并语义**:子样式覆盖父样式的已设字段(fg/bg/modifier 叠加),
//!   对齐 ratatui `Style::patch`,便于组件层样式继承。

use std::collections::HashMap;

/// 终端颜色 — 16 基础色 + 256 索引色 + 24-bit RGB
///
/// WHY 覆盖三档:基础色兼容所有终端;`Indexed` 用于 256 色终端;`Rgb` 用于
/// truecolor 终端(如渐变阈值着色)。`Reset` 表示恢复终端默认色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Color {
    /// 恢复终端默认前景/背景
    #[default]
    Reset,
    /// 黑
    Black,
    /// 红
    Red,
    /// 绿
    Green,
    /// 黄
    Yellow,
    /// 蓝
    Blue,
    /// 品红
    Magenta,
    /// 青
    Cyan,
    /// 灰(亮黑)
    Gray,
    /// 深灰
    DarkGray,
    /// 亮红
    LightRed,
    /// 亮绿
    LightGreen,
    /// 亮黄
    LightYellow,
    /// 亮蓝
    LightBlue,
    /// 亮品红
    LightMagenta,
    /// 亮青
    LightCyan,
    /// 白
    White,
    /// 256 色板索引色
    Indexed(u8),
    /// 24-bit 真彩色
    Rgb(u8, u8, u8),
}

/// 文本修饰位标志 — 加粗/暗淡/斜体/下划线/反色/删除线/闪烁/隐藏(可组合)
///
/// WHY `u16` 位标志:与 ratatui `Modifier` 全集对齐(9 种修饰需 ≥ 9 位,u8 不够),
/// 保证 `engine::compat` 从 ratatui 翻译时修饰无损;`u16` 仍 `Copy`/`Hash`,
/// 不引 bitflags 依赖,`Style` 参与 StylePool 去重不受影响。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifier(u16);

impl Modifier {
    /// 无修饰
    pub const NONE: Modifier = Modifier(0);
    /// 加粗
    pub const BOLD: Modifier = Modifier(1 << 0);
    /// 暗淡
    pub const DIM: Modifier = Modifier(1 << 1);
    /// 斜体
    pub const ITALIC: Modifier = Modifier(1 << 2);
    /// 下划线
    pub const UNDERLINED: Modifier = Modifier(1 << 3);
    /// 反色(前景/背景互换)
    pub const REVERSED: Modifier = Modifier(1 << 4);
    /// 删除线
    pub const CROSSED_OUT: Modifier = Modifier(1 << 5);
    /// 慢速闪烁
    pub const SLOW_BLINK: Modifier = Modifier(1 << 6);
    /// 快速闪烁
    pub const RAPID_BLINK: Modifier = Modifier(1 << 7);
    /// 隐藏(不可见字符)
    pub const HIDDEN: Modifier = Modifier(1 << 8);

    /// 是否包含指定修饰位
    pub const fn contains(self, other: Modifier) -> bool {
        (self.0 & other.0) == other.0
    }

    /// 并入修饰位(返回新值,不可变)
    pub const fn union(self, other: Modifier) -> Modifier {
        Modifier(self.0 | other.0)
    }

    /// 是否为空(无任何修饰)
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// 单元格样式 — 前景色 / 背景色 / 修饰
///
/// WHY 字段用 `Option<Color>`:`None` 表示"不指定,继承已有",支持 `patch` 叠加;
/// `Some(Reset)` 才是显式恢复默认。这与 ratatui 语义一致,便于兼容层桥接。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Style {
    /// 前景色(None = 不指定)
    pub fg: Option<Color>,
    /// 背景色(None = 不指定)
    pub bg: Option<Color>,
    /// 文本修饰
    pub modifier: Modifier,
}

impl Style {
    /// 空样式(所有字段未指定)
    pub const fn new() -> Self {
        Self {
            fg: None,
            bg: None,
            modifier: Modifier::NONE,
        }
    }

    /// 设置前景色(链式)
    pub const fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    /// 设置背景色(链式)
    pub const fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    /// 追加修饰(链式)
    pub const fn add_modifier(mut self, m: Modifier) -> Self {
        self.modifier = self.modifier.union(m);
        self
    }

    /// 用另一个样式覆盖本样式的已设字段(子覆盖父)
    ///
    /// WHY:组件层样式继承——父样式提供默认,子样式只覆盖需要变更的字段,
    /// 未设字段(None)保留父值,修饰位取并集。
    pub fn patch(mut self, other: Style) -> Style {
        if other.fg.is_some() {
            self.fg = other.fg;
        }
        if other.bg.is_some() {
            self.bg = other.bg;
        }
        self.modifier = self.modifier.union(other.modifier);
        self
    }
}

/// 样式去重池 — 将 `Style` 内部化为稳定 `u16` 索引
///
/// WHY 池化:Buffer 每个 Cell 若内联 `Style`(约 8 字节)在 80×24 下即 15KB;
/// 池化后 Cell 只存 2 字节索引,80×24 降至约 4KB,且 diff 比较退化为 u16 比较。
/// M1 Buffer 优化将采用本池;M0 先建立池与去重语义并测试。
#[derive(Debug, Clone, Default)]
pub struct StylePool {
    /// 索引 → 样式(下标即 StyleId)
    styles: Vec<Style>,
    /// 样式 → 索引,保证同一样式只入池一次
    index: HashMap<Style, u16>,
}

impl StylePool {
    /// 创建空池(默认样式预置为索引 0,保证 Cell 默认值零成本)
    pub fn new() -> Self {
        let mut pool = Self {
            styles: Vec::new(),
            index: HashMap::new(),
        };
        // 预置默认样式为 id 0,使 Cell 默认样式无需额外 intern
        pool.intern(Style::new());
        pool
    }

    /// 内部化样式,返回其稳定索引;重复样式复用既有索引
    ///
    /// # Panics
    /// 唯一样式数超过 `u16::MAX`(65535)时——TUI 场景不可能达到,若达到说明
    /// 存在样式泄漏 bug,`expect` 主动暴露而非静默截断。
    pub fn intern(&mut self, style: Style) -> u16 {
        if let Some(&id) = self.index.get(&style) {
            return id;
        }
        let id = u16::try_from(self.styles.len())
            .expect("StylePool 唯一样式数超过 u16::MAX,疑似样式泄漏");
        self.styles.push(style);
        self.index.insert(style, id);
        id
    }

    /// 按索引取回样式;越界返回默认样式(防御边界)
    pub fn get(&self, id: u16) -> Style {
        self.styles.get(id as usize).copied().unwrap_or_default()
    }

    /// 池内唯一样式数
    pub fn len(&self) -> usize {
        self.styles.len()
    }

    /// 池是否为空(创建后至少含默认样式,故恒为 false;保留以满足 clippy)
    pub fn is_empty(&self) -> bool {
        self.styles.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_bitflags_compose() {
        let m = Modifier::BOLD.union(Modifier::UNDERLINED);
        assert!(m.contains(Modifier::BOLD));
        assert!(m.contains(Modifier::UNDERLINED));
        assert!(!m.contains(Modifier::ITALIC));
        assert!(Modifier::NONE.is_empty());
    }

    #[test]
    fn style_patch_child_overrides_parent() {
        let parent = Style::new().fg(Color::White).bg(Color::Black);
        let child = Style::new().fg(Color::Red).add_modifier(Modifier::BOLD);
        let merged = parent.patch(child);
        assert_eq!(merged.fg, Some(Color::Red)); // 子覆盖
        assert_eq!(merged.bg, Some(Color::Black)); // 父保留
        assert!(merged.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn style_pool_dedups_and_reuses_index() {
        let mut pool = StylePool::new();
        let s = Style::new().fg(Color::Cyan);
        let id1 = pool.intern(s);
        let id2 = pool.intern(s);
        assert_eq!(id1, id2, "同一样式应复用索引");
        assert_eq!(pool.get(id1), s);
        // 默认样式占 id 0,新样式从 1 起
        assert_eq!(pool.get(0), Style::new());
    }
}
