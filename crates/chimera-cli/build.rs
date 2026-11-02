//! chimera-cli build.rs —— `/bench` 一期：构建期从清单生成真实静态计数。
//!
//! WHY 构建期生成而非硬编码：`/bench` 面板要展示「criterion 基准总数 / SLO 阈值总数」
//! 这类**清单可派生**的事实。若把这些数手填进代码，就会踩本仓「计数硬编码漂移」红线
//! （v3.0 R3-6 / clippy I8 教训：注释与常量、清单与代码不同步）。构建期扫描真实清单
//! 生成常量，值随仓库变化自动重算，杜绝漂移。
//!
//! 产物：`$OUT_DIR/bench_consts.rs` 暴露两个常量，由 `src/bench_provider.rs` 经
//! `include!` 引入。
//!
//! # 重跑触发的粒度（W6-0 收口）
//! 计数只派生自**三类文件**：根 `Cargo.toml`（`workspace.members` 决定有哪些成员）、
//! 每个成员的 `crates/<name>/Cargo.toml`（`[[bench]]` 注册处）、
//! `scripts/perf_thresholds.toml`（阈值段）。因此本脚本按这三类**逐个文件**发
//! `cargo:rerun-if-changed`，而不是监视整个 `crates/` 目录——后者会让任何一次
//! `src/**.rs` 编辑（计数与它无关）都触发 build script 重跑与本 crate 重编，
//! 在 `jobs = 4` 的 15GB 机型上是纯粹的构建耗时与内存压力浪费。
//! 增删成员 crate 必然要改根 `Cargo.toml`，故该文件在册即可保证新成员的清单
//! 变更不会被漏掉（其自身 manifest 在被纳入 members 那次构建起即进入监视集）。
//!
//! 注：本文件刻意不使用 `unwrap`/`expect`（chimera-cli 已 opt-in workspace lint 棘轮，
//! 生产码 unwrap/expect 必须为 0；build script 亦纳入 `--all-targets` clippy），
//! 任何 IO/env 异常一律降级为「跳过生成 + cargo:warning」，绝不让构建静默产出错误数。

use std::fs;
use std::path::{Path, PathBuf};

/// 统计给定文件集合中「以 `needle` 开头（去前导空白）」的行数之和。
fn count_matching_lines(paths: impl IntoIterator<Item = PathBuf>, needle: &str) -> u32 {
    let mut total = 0u32;
    for p in paths {
        if let Ok(text) = fs::read_to_string(p) {
            total += text
                .lines()
                .filter(|line| line.trim_start().starts_with(needle))
                .count() as u32;
        }
    }
    total
}

/// 收集 `workspace_root/crates/*/Cargo.toml`（成员 crate 清单，用于统计 `[[bench]]`）。
/// 排序后返回，使 `cargo:rerun-if-changed` 的输出序稳定（可被差异比对与审查）。
fn member_manifests(workspace_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(workspace_root.join("crates")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let cand = path.join("Cargo.toml");
                if cand.is_file() {
                    out.push(cand);
                }
            }
        }
    }
    out.sort();
    out
}

/// 声明重跑触发点：只监视计数真正的来源文件（见模块头「重跑触发的粒度」）。
fn emit_rerun_signals(workspace_root: &Path) {
    let watched = std::iter::once(workspace_root.join("Cargo.toml"))
        .chain(member_manifests(workspace_root))
        .chain(std::iter::once(
            workspace_root.join("scripts").join("perf_thresholds.toml"),
        ));
    for path in watched {
        // 路径带空格（本仓根目录名含空格）时 cargo 仍按整行取值，无需引号；
        // 用绝对路径避免相对基准目录歧义。
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn main() {
    // 工作区根：本 crate 位于 <root>/crates/chimera-cli，上溯两级即根。
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");

    // 清单变更须触发重跑，防 OUT_DIR 陈旧（逐文件精确监视，不监视整目录）。
    emit_rerun_signals(&workspace_root);

    let out_dir = match std::env::var("OUT_DIR") {
        Ok(v) => PathBuf::from(v),
        Err(_) => {
            eprintln!("cargo:warning=OUT_DIR 缺失，跳过 bench 计数生成（/bench 将回落未接线态）");
            return;
        }
    };

    // 全 workspace 成员 crate 的 `[[bench]]` 注册总数（criterion 基准数）。
    let total_benches = count_matching_lines(member_manifests(&workspace_root), "[[bench]]");
    // scripts/perf_thresholds.toml 的段总数（`[[` 开头的阈值/基准门禁段）。
    let slo_total = count_matching_lines(
        std::iter::once(workspace_root.join("scripts").join("perf_thresholds.toml")),
        "[[",
    );

    let body = format!(
        "// 自动生成于 chimera-cli/build.rs —— 请勿手改；改清单即改此数。\n\
         /// 全 workspace criterion `[[bench]]` 注册总数（构建期扫描清单，非硬编码）。\n\
         pub const TOTAL_BENCHES: u32 = {total_benches};\n\
         /// scripts/perf_thresholds.toml 阈值/门禁段总数（构建期扫描，非硬编码）。\n\
         pub const SLO_THRESHOLD_TOTAL: u32 = {slo_total};\n"
    );

    if fs::write(out_dir.join("bench_consts.rs"), body).is_err() {
        eprintln!("cargo:warning=写入 bench_consts.rs 失败，/bench 将回落未接线态");
    }
}
