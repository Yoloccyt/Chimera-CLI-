//! 路径工具的集成契约 — `expand_tilde` 的平台无关性与幂等性
//!
//! 设计约束（见 `src/path_util.rs`):展开逻辑分为"读环境变量的公开入口"与
//! "注入 home 的私有内核"。集成测试只能触碰公开入口,故本文件**只断言与
//! 环境无关的契约**（非 `~` 前缀路径原样返回、不可展开的 `~` 形式原样返回、
//! 幂等性),唯一依赖环境变量的用例以条件断言把两种合法结果都写清,
//! 保证在 Linux/Windows 且无论 HOME/USERPROFILE 是否设置都确定通过。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code

use std::path::{Path, PathBuf};

use nexus_core::path_util::expand_tilde;

#[test]
fn non_tilde_paths_are_returned_verbatim_on_every_platform() {
    // 契约:无 `~` 前缀 → 原样返回（Unix/Windows 两种分隔符与盘符形式均不得改写）
    let cases = [
        "/absolute/path.db",
        "/var/lib/chimera/memory.sqlite",
        "C:\\Users\\chimera\\memory.db",
        "D:\\Chimera CLI\\tmp\\x.bin",
        "relative/path.db",
        "relative\\path.db",
        "./dot/relative.db",
        "..\\up\\one.db",
    ];
    for raw in cases {
        let path = PathBuf::from(raw);
        assert_eq!(expand_tilde(&path), path, "{raw} 无 ~ 前缀时不得被改写");
    }
}

#[test]
fn tilde_forms_outside_home_shorthand_are_not_expanded() {
    // 契约:只有 `~` / `~/` / `~\` 是 home 简写;其余 `~` 形式（用户名、路径中段）
    // 无论 HOME 是否设置都必须原样返回 —— 这是平台无关的确定性断言
    let cases = [
        "~user/memory.db",  // 用户名形式（shell 才展开）
        "~root/.chimera",   // 同上
        "some/~/nested.db", // 非前缀位置
        "a~b/c.db",         // `~` 不在首位
    ];
    for raw in cases {
        let path = PathBuf::from(raw);
        assert_eq!(
            expand_tilde(&path),
            path,
            "{raw} 不属于 home 简写,必须原样返回"
        );
    }
}

#[test]
fn expand_tilde_is_idempotent_for_paths_it_does_not_expand() {
    // 契约:不展开的路径二次调用结果稳定（不会在第二次意外改写）
    let path = Path::new("/data/chimera/wiki.db");
    let once = expand_tilde(path);
    let twice = expand_tilde(&once);
    assert_eq!(once, twice);
    assert_eq!(once, path);
}

#[test]
fn expand_tilde_handles_empty_and_separator_only_paths() {
    // 契约:退化输入不得 panic,且无 `~` 前缀一律原样返回
    for raw in ["", "/", "\\", ".", ".."] {
        let path = PathBuf::from(raw);
        assert_eq!(expand_tilde(&path), path, "{raw:?} 必须原样返回");
    }
}

#[test]
fn bare_tilde_resolves_to_the_configured_home_or_stays_literal() {
    // 契约（双分支显式化,消除环境依赖）:
    // - 存在 HOME/USERPROFILE → 裸 `~` 展开为该值;
    // - 两者皆缺 → 原样返回 `~`（调用方自行处理）。
    // 环境变量在本进程内只读（无测试改写它）,故该条件断言不构成 flaky 源。
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok();
    let expanded = expand_tilde(Path::new("~"));
    match home {
        Some(h) => assert_eq!(
            expanded,
            PathBuf::from(h),
            "有 home 环境变量时裸 ~ 必须展开为它"
        ),
        None => assert_eq!(
            expanded,
            PathBuf::from("~"),
            "无 home 环境变量时裸 ~ 必须原样返回"
        ),
    }
}
