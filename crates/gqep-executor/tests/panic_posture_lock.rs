//! W5 / ADR-271 — panic 姿态的**诚实声明锁**。
//!
//! # 为什么锁「不做什么」而不是锁「做了什么」
//!
//! `[profile.release]` 开 `panic = "abort"` 是体积 SLO（<50MB / Docker <100MB）的
//! 刻意权衡。abort 不走 unwind ⇒ **`catch_unwind` 在 release 下抓不住 panic**。
//! 于是在工具路径上加 `catch_unwind` 不会带来隔离，只会制造「已隔离」的错觉。
//!
//! 本文件因此锁三件事：
//! 1. `panic = "abort"` 仍在位（防有人为「修 panic」悄悄改 unwind，破体积 SLO）；
//! 2. 该行附近有**后果声明**（改它的人必须先看到代价，ADR-271）；
//! 3. 工具路径**零 `catch_unwind`**（防虚假安全感），且模块头声明仍在。
//!
//! 运行：`cargo test -p gqep-executor --test panic_posture_lock`

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

fn ws() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    let p = ws().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("读取 {rel} 应成功: {e}"))
}

#[test]
fn release_profile_panic_abort_is_deliberate() {
    let text = read("Cargo.toml");
    let after = text
        .split("[profile.release]")
        .nth(1)
        .expect("根 Cargo.toml 必须存在 [profile.release]");
    let head = &after[..after.len().min(1200)];
    assert!(
        head.contains("panic = \"abort\""),
        "release 档必须保持 panic=\"abort\"（体积 SLO 的刻意权衡）：\n{head}"
    );
    assert!(
        text.contains("ADR-271"),
        "Cargo.toml 必须带 ADR-271 的 panic 后果声明（防悄悄改成 unwind）"
    );
    assert!(
        text.contains("catch_unwind 在 release 下**无效**"),
        "声明必须写清「catch_unwind 在 release 下无效」这一后果"
    );
}

#[test]
fn tool_path_has_no_catch_unwind() {
    // 工具派发热路径的全部实现文件：这些位置加 catch_unwind 在 release 下都抓不住
    // panic（abort 不走 unwind），只会制造「已隔离」的错觉。
    let files = [
        "crates/gqep-executor/src/toolplan_runner.rs",
        "crates/gqep-executor/src/tool_runtime/runtime.rs",
        "crates/gqep-executor/src/tool_runtime/chain.rs",
        "crates/gqep-executor/src/tool_runtime/inproc.rs",
        "crates/gqep-executor/src/tool_runtime/batch.rs",
        "crates/chimera-cli/src/exec_plan.rs",
        "crates/chimera-cli/src/mcp_tool_executor.rs",
    ];
    for rel in files {
        let text = read(rel);
        // 匹配**调用形态**而非裸词：本仓文档会在散文里提到 catch_unwind
        // （含本模块头的 ADR-271 声明）；裸词扫描会把「解释为什么不加」误判成「加了」。
        assert!(
            !text.contains("catch_unwind("),
            "{rel} 不得引入 catch_unwind —— release panic=abort 下它抓不住 panic（ADR-271）。\
             若要真隔离，须先改全局 profile 并重开体积 SLO 裁决。"
        );
        assert!(
            !text.contains("AssertUnwindSafe"),
            "{rel} 不得引入 AssertUnwindSafe —— release 下同样无效，且包 async future 有 UB 风险面"
        );
    }
}

#[test]
fn posture_declaration_present() {
    let text = read("crates/gqep-executor/src/toolplan_runner.rs");
    assert!(
        text.contains("panic 姿态（ADR-271"),
        "toolplan_runner 模块头必须保留 panic 姿态声明"
    );
    assert!(
        text.contains("不承诺") && text.contains("panic 隔离"),
        "声明必须显式写「不承诺 panic 隔离」，而不是含糊其辞"
    );
    assert!(
        text.contains("Err 级 fail-soft"),
        "声明必须写清实际承诺的是 Err 级 fail-soft"
    );
}
