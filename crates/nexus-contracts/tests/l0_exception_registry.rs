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

/// 不变量 3：W3 子模块化的 10 个组模块均以 `pub mod <group>;` 声明于 lib.rs，
/// 锁住「53 平铺模块 → 内聚子树」结构，防止静默回退。
#[test]
fn nested_group_structure_is_present() {
    for &group in GROUP_MODULES {
        let decl = format!("pub mod {group};");
        assert!(
            LIB.contains(&decl),
            "W3 子模块化组 `{group}` 未在 lib.rs 声明（`{decl}`）——结构被回退或漂移"
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
