//! input — TUI 输入路由(ADR-029,v3.1 §4.3)
//!
//! 对应架构层:L10 Interface
//!
//! # 模块职责
//! - `router` 模块:`InputRouter` 三态路由状态机 + `RouteTarget` 路由目标枚举。
//!   决定每个按键在 Normal/Insert/Command 模式下应交由谁处理。
//!
//! # 与既有输入处理的关系
//! M0 提供路由骨架与三态路由表(D 类快照测试覆盖);M2 正式接线,替换
//! `app.rs` 内联的按键分发逻辑,是 `app.rs` 主循环拆分(EventLoop/Renderer/
//! InputRouter)的第一步。

pub mod router;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub use router::{InputRouter, PaneDir, RouteTarget, RouterMode};

/// 键位模式 — 表驱动派发的匹配键(单一原语,app 层弹窗表与面板层键表共用)
///
/// # 设计决策(WHY)
/// - **与原 match 链逐字节等价**:模式语义即原手写链的两种写法——裸
///   `match key.code` arm(忽略修饰键)与 `if modifiers.contains(CONTROL)`
///   守卫。等价性是"27 面板用户可见行为零变化"红线的前提,新增模式前
///   必须先确认它不隐式收窄/放宽既有命中集合。
/// - **集中定义而非各处特判**:键表只声明"哪个键",命中规则只有本文件
///   一处实现,避免每张表各自重写匹配逻辑造成双源漂移。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyPattern {
    /// 仅匹配 KeyCode,忽略修饰键 —— 等价原 `match key.code` arm
    /// (如弹窗内 Ctrl+Q 与 q 同效关闭,该既有行为由此模式原样保留)
    Code(KeyCode),
    /// 匹配 KeyCode 且要求**含** CONTROL —— 等价原
    /// `if key.modifiers.contains(KeyModifiers::CONTROL)` 守卫
    /// (WHY contains 而非精确相等:与原守卫逐字节一致,Ctrl+Shift+A 等
    /// 组合键的命中集合不发生变化)
    Ctrl(KeyCode),
    /// 匹配任意字符键(`KeyCode::Char(_)`)—— 等价原 `KeyCode::Char(c) =>` 绑定
    /// arm;载荷 `c` 由处理器从按键事件自行提取
    AnyChar,
}

impl KeyPattern {
    /// 判定按键是否命中本模式(逐字节对齐原 match/guard 语义)
    pub fn matches(self, key: KeyEvent) -> bool {
        match self {
            KeyPattern::Code(code) => key.code == code,
            KeyPattern::Ctrl(code) => {
                key.code == code && key.modifiers.contains(KeyModifiers::CONTROL)
            }
            KeyPattern::AnyChar => matches!(key.code, KeyCode::Char(_)),
        }
    }
}

/// 键派发规则 — 派发表的一行:键位模式 → 语义动作 `A`
///
/// WHY 泛型:app 层弹窗表(动作 = `PopupKeyAction` 枚举)与面板层键表
/// (动作 = fn 指针)共用同一行结构与查找原语——"同一套表驱动派发机制",
/// 动作语义由各上下文私有。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyRule<A> {
    /// 命中键位(语义约束见 [`KeyPattern`])
    pub pattern: KeyPattern,
    /// 命中后执行的语义动作
    pub action: A,
}

/// 在键表中查找按键对应的语义动作(首个命中者生效,等价 match 链 arm 顺序)
///
/// # 返回值
/// 命中返回 `Some(动作)`;未命中返回 `None`(调用方决定兜底语义,
/// 面板键表兜底 = 死键无操作,与原 `_ => None` arm 一致)
pub fn lookup_key_action<A: Copy>(table: &[KeyRule<A>], key: KeyEvent) -> Option<A> {
    table
        .iter()
        .find(|r| r.pattern.matches(key))
        .map(|r| r.action)
}
