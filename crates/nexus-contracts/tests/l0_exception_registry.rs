//! ADR-033「受控纯逻辑」例外登记表完整性交叉校验（承 v3.5 W3 子模块化 / G-99 纯度门）。
//!
//! 对应 `ADR-033` 追加节 2026-10-09 自陈的「机器门缺口」中「单测」那一半
//! （`ADR-212` 决策 3 已落地符号级 `check_l0_purity.py`；本测试锁定例外登记表一致性）。
//!
//! 不变量：
//! 1. 每个登记名必须是 crate 根可达的声明模块（防重命名/删除后登记表与实况漂移）；
//! 2. 登记表与「实质性受控逻辑模块」集**双向相等**（v3.5 深度复审 B-P1-7：既不得漏登已
//!    含实质逻辑的模块，也不得把无实质逻辑的模块虚登）；
//! 3. W3 子模块化后的 10 个组模块声明均存在（结构锁：防回退为 53 平铺模块）。
//!
//! 本测试为 dev 上下文，读 `lib.rs` 用 `include_str!`（编译期内嵌，无运行期 IO），
//! 不违反 L0 生产纯度（G-99 仅扫描 `src/**`，不扫描 `tests/`）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test-unit idiom; E-5 targets production code

use nexus_contracts::ADR_033_EXCEPTION_MODULES;

/// crate 根 lib.rs 源码（编译期内嵌）。
const LIB: &str = include_str!("../src/lib.rs");

/// 实质性受控逻辑模块集（与 `ADR_033_EXCEPTION_MODULES` 双向相等；新增/删除
/// 含实质逻辑的模块时，必须同步修订本表与 lib.rs 登记表，否则本测红）。
const SUBSTANTIVE_LOGIC_MODULES: &[&str] = &[
    "test_scale",
    "archive_monotonicity",
    "capability_token",
    "experience_card",
    "harness_spec",
    "platform_grounding",
    "behavior_contract",
    "blueprint",
    "util",
    "mcsm",
    "sensitive",
    "calibration",
    "recall_fusion",
    "skill_lifecycle",
    "tool_plan",
    "errors",
    "command_validation",
    // ADR-218 决策 1/2：以下 11 项由 G-119 从源码派现（此前登记表与实况脱节 11 项）。
    // 本表与 lib.rs 的 ADR_033_EXCEPTION_MODULES 必须**同批**修改，否则不变量 2 红。
    "affinity",
    "price_vector",
    "quota",
    "token_evidence",
    "density",
    "policy",
    "reward",
    "masks",
    "formal_props",
    "harness_dimensions",
    "variant",
    // R12 W0 订正（ADR-296）：与 lib.rs `ADR_033_EXCEPTION_MODULES` 同批加入。
    // 该模块落点在 sibling crate（nexus-contracts-mca/src/mca/），原本只登记在
    // lib.rs 一侧而本表未同步 ⇒ 两个不变量一红（多登记）一绿（无根声明那条也红）。
    // 基线口径：本表与 lib.rs 登记表必须**同批**修改，否则本测试逐字判红。
    "context_capability",
];

/// W3 子模块化后的 10 个组模块（结构锁，防回退平铺）。
const GROUP_MODULES: &[&str] = &[
    "sparse",
    "spec",
    "seams",
    "core_domain",
    "memory",
    "safety",
    "mca",
    "vector_math",
    "protocol",
    "testutil",
];

/// ADR-222 D2 已拆为 sibling crate 的组：组名 -> sibling lib 名。
/// 结构锁对它们接受 `pub use <sibling>::<group>;` 形态（facade 保持旧路径）。
const SPLIT_GROUPS: &[(&str, &str)] = &[
    ("core_domain", "nexus_contracts_core"),
    ("sparse", "nexus_contracts_sparse"),
    ("safety", "nexus_contracts_safety"),
    ("seams", "nexus_contracts_seams"),
    ("mca", "nexus_contracts_mca"),
    ("memory", "nexus_contracts_memory"),
];

/// 不变量 1：每个登记名都以 `pub use <group>::<leaf>;` 形式声明于 crate 根，
/// 即 `lib.rs` 必含子串 `::<name>;`。facade/prelude 对模块的引用一律带 `::{`
/// 或 `::Item`（PascalCase），故 `::<name>;`（snake_case + 紧跟分号）唯一命中
/// 组 re-export 行，不误判。
#[test]
fn every_registered_exception_is_a_crate_root_declared_module() {
    for &name in ADR_033_EXCEPTION_MODULES {
        let needle = format!("::{name};");
        assert!(
            LIB.contains(&needle),
            "ADR-033 例外登记名 `{name}` 在 crate 根无对应 `pub use <group>::{name};` 声明——\
             模块可能已被重命名/移除，登记表与实况漂移"
        );
    }
}

/// 不变量 2（双向锁）：登记表与「实质性逻辑模块」集恰好相等——
/// 既杜绝「含实质逻辑却未登记」（B-P1-6 名实不符），也杜绝「无实质逻辑却虚登」。
#[test]
fn registry_equals_substantive_logic_set() {
    let mut reg: Vec<&str> = ADR_033_EXCEPTION_MODULES.to_vec();
    let mut want: Vec<&str> = SUBSTANTIVE_LOGIC_MODULES.to_vec();
    reg.sort_unstable();
    want.sort_unstable();
    let missing: Vec<&&str> = want.iter().filter(|m| !reg.contains(m)).collect();
    let extra: Vec<&&str> = reg.iter().filter(|m| !want.contains(m)).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "例外登记表与实质性逻辑集不一致：漏登记={missing:?} / 多登记={extra:?}（须同批修改本表与 lib.rs ADR_033_EXCEPTION_MODULES）"
    );
}

/// 不变量 3：10 个组模块必须在 crate 根可见——未拆分的组用 `pub mod <group>;`，
/// 已按 ADR-222 D2 拆为 sibling 的组用 `pub use <sibling>::<group>;`（facade）。
/// 两种情况都锁住「53 平铺模块 → 内聚子树」结构，防止静默回退。
#[test]
fn nested_group_structure_is_present() {
    for &group in GROUP_MODULES {
        let decl = format!("pub mod {group};");
        let ok = match SPLIT_GROUPS.iter().find(|(g, _)| *g == group) {
            Some((_, krate)) => {
                let facade = format!("pub use {krate}::{group};");
                LIB.contains(&decl) || LIB.contains(&facade)
            }
            None => LIB.contains(&decl),
        };
        assert!(
            ok,
            "组 `{group}` 未在 lib.rs 以 `pub mod`（自有）或 `pub use <sibling>::{group};`\
             （ADR-222 D2 拆分）声明——结构被回退或漂移"
        );
    }
}

/// 附加：登记表无重复、全部为合法 snake_case 模块名（卫生检查）。
#[test]
fn registry_is_deduped_and_snake_case() {
    let mut seen = std::collections::HashSet::new();
    for &name in ADR_033_EXCEPTION_MODULES {
        assert!(seen.insert(name), "登记表存在重复项 `{name}`");
        assert!(
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                && !name.is_empty(),
            "登记名 `{name}` 非合法 snake_case 模块名"
        );
    }
    assert!(
        ADR_033_EXCEPTION_MODULES.len() >= SUBSTANTIVE_LOGIC_MODULES.len(),
        "登记表不应少于实质性逻辑模块集",
    );
}

/// 不变量 4（承 ADR-033 追加节 2026-10-09 的量体订正 / ADR-213 §6.5 对 D-14 的处置）：
/// **已登记为「受控逻辑例外」的模块，其模块头不得再以项目符号句式自称「纯类型零逻辑」**。
///
/// 为什么只判这一种句式（而非把「零逻辑」三字全局禁用）：盘上命中分两形，只有其一可机判——
///   · 自我主张形 `//! - **纯类型 + 零逻辑**: 仅类型定义…`——是对本模块性质的断言；
///   · 引用叙述形（`capability_token.rs:10`「本模块**不是**“零逻辑”」、`mcsm.rs:18`
///     「显式声明为…的例外」、`util.rs:14`「受控例外」、`archive_monotonicity.rs:22`
///     转述 ADR-033 原文）——是对该词的**谈论**，判红即假红。
/// 两形语法区别清楚（行首 `//!` + `- **纯类型` + 含 `零逻辑**`），故本测只锁前一种。
/// 未登记例外的模块使用主张形是**合法的**：登记表语义为「含实质逻辑者须登记」，
/// 无逻辑者宣称无逻辑与实况一致（如 `core_domain/domain.rs`——生产面只有
/// `default_priority`/`Default::default`，其余全在 `#[cfg(test)]` 之后，确不在表内）。
#[test]
fn registered_exception_never_claims_zero_logic() {
    // ADR-222 D2 后登记例外分布在 7 个 L0-A crate 里；只扫本 crate 会让覆盖面
    // 静默缩水（实测：只扫 umbrella 时 28 项只能解析到 12 项）。因此按 crate 名前缀
    // `nexus-contracts` 枚举全量 L0-A 源码根，不另维一份会漂移的硬编码清单。
    let roots = l0a_src_roots();
    let mut checked = 0usize;
    for src_root in &roots {
        for file in rust_files(src_root) {
            let stem = file
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            if !ADR_033_EXCEPTION_MODULES.contains(&stem.as_str()) {
                continue;
            }
            checked += 1;
            let text = std::fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("读取 {} 失败: {e}", file.display()));
            for (idx, line) in text.lines().enumerate() {
                let t = line.trim_start();
                let is_self_claim = t.starts_with("//! - **纯类型") && t.contains("零逻辑**");
                assert!(
                    !is_self_claim,
                    "模块 `{stem}` 已登记于 ADR_033_EXCEPTION_MODULES（即承认含受控实质逻辑），\
                     其模块头 {}:{} 却仍自称「纯类型零逻辑」——名实矛盾，读者会据该行误判本模块\
                     是否需要读逻辑。改为陈述实际形状（「纯类型 + 受控纯逻辑（登记例外）」）或删除该断言行。\n  {t}",
                    file.display(),
                    idx + 1,
                );
            }
        }
    }
    assert_eq!(
        checked,
        ADR_033_EXCEPTION_MODULES.len(),
        "全量 L0-A crate（{} 个）只解析到 {checked} 个登记例外的源文件，而登记表有 {} 项——\
         有登记名解析不到文件（或被重命名/移除），登记表与实况漂移",
        roots.len(),
        ADR_033_EXCEPTION_MODULES.len(),
    );
}

/// 枚举全部 L0-A crate 的 `src/` 目录（目录名以 `nexus-contracts` 开头）。
///
/// WHY 按前缀枚举而不是硬编码清单：再维一份 crate 名单就是 G-115/G-119 反复治理的
/// 「第二份手维护表」；ADR-222 D2 的命名约定（`nexus-contracts` / `nexus-contracts-<domain>`）
/// 足以让本函数自维护，新增 sibling 时无需改本测试。
fn l0a_src_roots() -> Vec<std::path::PathBuf> {
    let crates_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ 父目录")
        .to_path_buf();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&crates_dir).expect("读取 crates/ 失败") {
        let path = entry.expect("crates/ 目录项").path();
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        if name.starts_with("nexus-contracts") {
            let src = path.join("src");
            if src.is_dir() {
                out.push(src);
            }
        }
    }
    out.sort();
    assert!(
        !out.is_empty(),
        "未解析到任何 L0-A crate 源码根——本测覆盖面为零，不得读作通过"
    );
    out
}

/// 递归收集 `*.rs`。dev 上下文运行期 IO：G-99 只扫 `src/**` 生产面、不扫 `tests/`，
/// 故本 helper 不触纯度门。排序保证输出稳定。
fn rust_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rust_files(&path));
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
    out.sort();
    out
}
