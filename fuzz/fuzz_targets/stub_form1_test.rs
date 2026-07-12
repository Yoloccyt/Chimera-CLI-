//! Stub 宏覆盖测试:Form 1 — |bytes| 无类型标注
//!
//! 验证 fuzz/src/lib.rs 中的 stub 宏覆盖 libfuzzer-sys 0.4 的
//! `(|$bytes:ident| $body:block)` 签名形式(无类型标注,默认 &[u8])。
//! 此文件为回归测试,确保未来宏修改不会破坏 form 1 兼容性。
//!
//! 非 Windows-GNU 环境使用真正的 libfuzzer-sys(已支持此形式)。

#[cfg(not(all(target_os = "windows", target_env = "gnu")))]
use libfuzzer_sys::fuzz_target;

#[cfg(all(target_os = "windows", target_env = "gnu"))]
use chimera_fuzz::fuzz_target;

fuzz_target!(|bytes| {
    let _ = bytes.len();
});
