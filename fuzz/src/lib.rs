//! Chimera fuzz crate lib target
//!
//! WHY 此文件存在:fuzz crate 需要一个 lib target 来承载 Windows-GNU 下的
//! `fuzz_target!` stub 宏(见下方宏定义)。
//!
//! ## 平台条件策略
//!
//! | 平台 | libfuzzer-sys | fuzz_target! 来源 | 行为 |
//! |------|---------------|-------------------|------|
//! | Linux / macOS / Windows-MSVC | 引入(Cargo.toml 条件依赖) | libfuzzer_sys crate | 实际 fuzz 运行 |
//! | Windows-GNU (MinGW) | 不引入 | 本 crate stub 宏 | cargo check 验证语法 |
//!
//! Windows-GNU 下无法编译 libfuzzer-sys 的 C++ 源码
//! (FuzzerExtFunctionsWindows.cpp 使用 MSVC 特定的 `__builtin_function_start`
//! 和 `/alternatename` linker pragma,MinGW g++ 无法解析)。
//! stub 宏将 fuzz body 编译为闭包 `let _probe = |data: &[u8]| { ... }`,
//! 让 `cargo check` 验证 fuzz 逻辑的语法和类型正确性,但不链接 libFuzzer。
//! 实际 fuzz 运行委托 Linux CI(.github/workflows/fuzz.yml)。
//!
//! ## 支持的签名形式(与 libfuzzer-sys 0.4 对齐)
//!
//! | 形式 | 语法 | stub 行为 |
//! |------|------|-----------|
//! | Form 1 | `\|bytes\| { ... }` | 闭包 `\|bytes: &[u8]\| { ... }` |
//! | Form 2 | `\|data: &[u8]\| { ... }` | 闭包 `\|data: &[u8]\| { ... }` |
//! | Form 3 | `\|data: CustomType\| { ... }` | 闭包 `\|data: CustomType\| { ... }` |
//!
//! Form 3 不检查 Arbitrary trait bound(stub 环境无 libfuzzer_sys),
//! 仅验证类型名称有效性和 body 类型安全性。

// WHY #[macro_export]:让 6 个 bin target 能通过 `use chimera_fuzz::fuzz_target`
// 引入此 stub 宏。#[macro_export] 将宏导出到 crate root。
//
// WHY #[cfg(all(target_os = "windows", target_env = "gnu"))]:
// 仅在 Windows-GNU 环境编译此宏。其他环境(libfuzzer-sys 可用的平台)
// 不编译此宏,避免与 libfuzzer_sys::fuzz_target 产生名称冲突。
#[cfg(all(target_os = "windows", target_env = "gnu"))]
#[macro_export]
macro_rules! fuzz_target {
    // Form 1: |bytes| 无类型标注(默认 &[u8])
    // 对应 libfuzzer-sys 0.4 的 `(|$bytes:ident| $body:block)` 规则。
    // stub 将 bytes 类型显式标注为 &[u8],与真实宏行为一致。
    (|$data:ident| $body:block) => {
        fn main() {
            let _probe = |$data: &[u8]| $body;
        }
    };

    // Form 2: |data: &[u8]| 显式字节切片(最常用,6 个 fuzz target 均使用)
    // 对应 libfuzzer-sys 0.4 的 `(|$data:ident: &[u8]| $body:block)` 规则。
    (|$data:ident: &[u8]| $body:block) => {
        fn main() {
            let _probe = |$data: &[u8]| $body;
        }
    };

    // Form 3: |data: CustomType| 任意 Arbitrary 类型
    // 对应 libfuzzer-sys 0.4 的 `(|$data:ident: $dty:ty| $body:block)` 规则。
    // WHY 闭包不执行:Arbitrary 类型需 libFuzzer 运行时反序列化原始字节,
    // stub 环境无运行时支持。$dty 出现在闭包签名中让编译器验证:
    // 1. 类型名称有效 2. body 中对 $data 的操作类型安全。
    // 不检查 Arbitrary trait bound(libfuzzer_sys 不可用,无法引用 trait),
    // 真正的 trait bound 检查由非 Windows-GNU 环境的 libfuzzer-sys 完成。
    (|$data:ident: $dty:ty| $body:block) => {
        fn main() {
            let _probe = |$data: $dty| $body;
        }
    };
}
