//! Windows-GNU stable 工具链 stub 宏方案
//!
//! WHY 此文件存在:libfuzzer-sys 的 `fuzz_target!` 宏需要 nightly 工具链
//! + libFuzzer C++ 库(FuzzerExtFunctionsWindows.cpp 仅适配 MSVC)。
//! Windows-GNU + stable 环境下 `cargo check --manifest-path fuzz/Cargo.toml`
//! 会因 libfuzzer-sys 编译失败而无法通过。
//!
//! 本文件提供一个条件编译替代方案:
//! - Windows + stable → 使用 stub 宏,将 fuzz_target! 展开为普通 fn + main()
//! - 非 Windows 或 nightly → 使用真正的 libfuzzer_sys::fuzz_target!
//!
//! 使用方式:
//! 在 fuzz_targets/*.rs 中将 `use libfuzzer_sys::fuzz_target;` 替换为:
//! ```rust,ignore
//! #[cfg_attr(all(windows, not(feature = "nightly")), use crate::fuzz_target)]
//! #[cfg_attr(not(all(windows, not(feature = "nightly"))), use libfuzzer_sys::fuzz_target)]
//! ```
//! 或在 Cargo.toml 中通过 target-specific 依赖切换。
//!
//! 当前实现策略:fuzz/Cargo.toml 通过 `[target.'cfg(windows)'.dependencies]`
//! 在 Windows 上排除 libfuzzer-sys 依赖,改用本 stub。
//! 但由于 fuzz target 文件直接 `use libfuzzer_sys::fuzz_target`,
//! 需要在 Windows 上提供一个同名模块作为替代。
//!
//! 验证范围:仅验证 fuzz target body 的语法正确性(类型检查 + 借用检查),
//! 不执行实际模糊测试。实际 fuzz 运行委托 Linux CI(见 .github/workflows/fuzz.yml)。

#![allow(unused)]
#![allow(clippy::all)]

/// stub 替代 `libfuzzer_sys::fuzz_target!` 宏
///
/// 将 `fuzz_target!(|data: &[u8]| { ... })` 展开为:
/// ```rust,ignore
/// fn fuzz_target_body(data: &[u8]) { ... }
/// fn main() {
///     fuzz_target_body(&[]);
/// }
/// ```
///
/// 这样 `cargo check` 能验证 body 的语法正确性,无需 nightly / libFuzzer。
#[macro_export]
macro_rules! fuzz_target {
    (| $arg:ident : $ty:ty | $body:block) => {
        fn fuzz_target_body($arg: $ty) $body

        fn main() {
            // 传入空数据,仅验证 body 能编译通过
            fuzz_target_body(<$ty>::default());
        }
    };
}
