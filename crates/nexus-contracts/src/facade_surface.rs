//! facade 值级面单源声明宏 —— L0-A（ADR-新-1 落码载体）
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts umbrella）
//!
//! # WHY 用宏消除双镜像
//! 顶层值级导出与 `pub mod prelude` 曾是两份手工维护的镜像清单
//! （漂移实证：tests/prelude_surface_test.rs 文件头记录的 2026-10-15 失守事件）。
//! 本宏让每个 leaf 模块的导出符号**只声明一份**，按条目尾的派发面标记
//! 同时生成顶层 `pub use` 与 prelude 再导出：
//! - `prelude` —— 该族全部符号进 prelude glob 面（"同族完备"由构造保证，
//!   物理上不可能只收伴生漏收主类型）；
//! - `top` —— 只进顶层命名空间（泛用名族的有意取舍，防消费方 glob 冲突）。
//!
//! "prelude 无孤儿"同理由构造保证：prelude 条目与顶层条目引用同一符号列表。
//! 两条曾经靠解析测试抓漂移的不变量降级为结构性事实，测试保留 planted-defect
//! 自检与基线等价锁（tests/prelude_surface_test.rs）。
//!
//! # 宏形状约束
//! 符号列表不允许尾逗号——`macro_rules` 转录层的可选逗号重复无元变量驱动
//! （编译期即拒），调用处统一 `{ A, B }` 形态。

/// 值级 facade 单源声明宏（crate-private，仅 lib.rs 调用一次）。
macro_rules! l0_surface {
    ($($module:ident $face:ident { $($sym:ident),+ })+) => {
        $(
            pub use $module::{ $($sym),+ };
        )+
        pub mod prelude {
            //! 预导出模块 — 常用类型的便捷导入
            //!
            //! # 与顶层的关系（口径，ADR-217 D-35 / ADR-新-1）
            //! prelude = **顶层值级导出的高频子集**，由 `l0_surface!` 宏从同一份
            //! 符号声明生成：条目标记 `prelude` 的族整体进本模块，标记 `top` 的
            //! 族只进顶层。无孤儿与同族完备两条不变量由构造保证（不再是两份
            //! 靠解析对拍的手工清单）。
            //!
            //! # 示例
            //! ```
            //! use nexus_contracts::prelude::*;
            //!
            //! let id = ToolId::new("tool-1");
            //! let mask: SparseMask<ToolId> = SparseMask::empty();
            //! ```
            $(
                crate::facade_surface::l0_surface!(@prelude_entry $module $face [ $($sym),+ ]);
            )+
        }
    };
    // 标记 `prelude` 的条目在 prelude 模块内生成同列表再导出
    (@prelude_entry $module:ident prelude [ $($sym:ident),+ ]) => {
        pub use crate::$module::{ $($sym),+ };
    };
    // 标记 `top` 的条目在 prelude 中无对应物
    (@prelude_entry $module:ident top [ $($sym:ident),+ ]) => {};
}

pub(crate) use l0_surface;
