//! Stub 宏覆盖测试:Form 3 — |data: $dty: ty| Arbitrary 类型
//!
//! 验证 fuzz/src/lib.rs 中的 stub 宏覆盖 libfuzzer-sys 0.4 的
//! `(|$data:ident: $dty:ty| $body:block)` 签名形式(任意 Arbitrary 类型)。
//! 此文件为回归测试,确保未来宏修改不会破坏 form 3 兼容性。
//!
//! 非 Windows-GNU 环境使用真正的 libfuzzer-sys,Vec<u8> 实现 Arbitrary。

#[cfg(not(all(target_os = "windows", target_env = "gnu")))]
use libfuzzer_sys::fuzz_target;

#[cfg(all(target_os = "windows", target_env = "gnu"))]
use chimera_fuzz::fuzz_target;

fuzz_target!(|data: Vec<u8>| {
    let _ = data.len();
});
