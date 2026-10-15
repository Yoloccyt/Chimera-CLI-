//! L0 值级 facade 一致性锁（ADR-217 决策 1 / D-35）
//!
//! # 锁什么
//! `lib.rs` 的公共面有两层值级导出：**顶层**（`pub use <leaf>::{A, B, …}`）与
//! **`pub mod prelude`**（便捷 glob 入口）。二者关系是刻意的「prelude = 顶层的
//! 高频子集」，但此前只有注释承诺、**无任何机器判据**——2026-10-15 三轮审计实测
//! 该承诺已经失守（prelude 收了 `ContractCheckOutcome/ContractContext/ContractExample`
//! 三个伴生类型，却漏收同模块主类型 `BehaviorContract`）。本文件把两条不变量变成
//! 可执行断言：
//!
//! 1. **无孤儿**：prelude 的每个值名都必须在顶层出现（否则 prelude 成为第二套门面，
//!    上层按 prelude 路径消费的名字在顶层找不到）。
//! 2. **同族完备**：某 leaf 模块只要在 prelude 里有**任一**成员，则该模块在顶层与
//!    prelude 的成员集合必须相等（"只收伴生、漏收主类型"正是本锁要抓的形状）。
//!
//! 整族缺席（`blueprint` 4 / `scheduler_contract` 9 / `tool_plan` 7 / `variant` 2，
//! 共 **22 项**，由本文件 `parser_sees_the_real_surface` 的 `[facade-scale]` 读数产出）
//! **不判**——那是"高频子集"的有意取舍（泛用名进 glob 会扩大消费方冲突面），不是漂移。
//!
//! # 为什么解析放在测试里而不是门脚本里
//! G-107 只管两段式**模块级** `pub use <group>::<leaf>;`（53 条，值级不在其辖域）；
//! 值级同集性属 L0 内部一致性问题，锁在 crate 自身测试里，不新增门轴（ADR-213 R8.3
//! 「分母不同、禁止互抄基线」）。本文件读 `src/lib.rs` 源码文本，属测试面 I/O
//! （G-99 纯度门辖区为 `src/**`，测试 crate 不受其约束）。
#![allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code

use std::collections::BTreeMap;
use std::fs;

/// 解析一段源码里的**值级** `pub use <path>::{A, B, …};` 语句，按归一化模块路径聚合。
///
/// 归一化只保留路径末段（顶层写 `behavior_contract::`、prelude 写 `crate::behavior_contract::`，
/// 二者指同一 leaf 模块；W3 后 leaf 名在两组内唯一，末段归一不会碰撞）。
fn value_groups(src: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut i = 0;
    while let Some(rel) = src[i..].find("pub use ") {
        let start = i + rel + "pub use ".len();
        // 取到本条语句结束（';'）；跨行语句由此覆盖
        let rest = &src[start..];
        let Some(end) = rest.find(';') else { break };
        let stmt = &rest[..end];
        i = start + end + 1;
        let Some(open) = stmt.find("::{") else {
            continue; // 模块级 `pub use a::b;` —— 不属值级面
        };
        let path = stmt[..open].trim();
        let body = &stmt[open + 3..];
        let body = match body.rfind('}') {
            Some(pos) => &body[..pos],
            None => continue,
        };
        let key = match path.rsplit("::").next() {
            Some(k) if !k.is_empty() => k.to_string(),
            _ => continue,
        };
        for token in body.split(',') {
            let name = token.split("//").next().unwrap_or("").trim();
            if !name.is_empty() && is_ident(name) {
                out.entry(key.clone()).or_default().push(name.to_string());
            }
        }
    }
    out
}

fn is_ident(s: &str) -> bool {
    let mut ch = s.chars();
    match ch.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    ch.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn flatten(map: &BTreeMap<String, Vec<String>>) -> std::collections::BTreeSet<String> {
    map.values().flatten().cloned().collect()
}

/// 从真实 crate 根定位并读取 `src/lib.rs`。
fn lib_src() -> String {
    let path = format!("{}/src/lib.rs", env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读取 {path} 失败: {e} —— 本锁的论域就是该文件"))
}

/// 以 `pub mod prelude` 声明行为界切分两段。
fn split_regions(src: &str) -> (String, String) {
    let idx = src
        .find("pub mod prelude")
        .expect("lib.rs 必须含 `pub mod prelude`（本锁的锚点）");
    (src[..idx].to_string(), src[idx..].to_string())
}

/// 两条不变量的纯判定函数（真树与夹具共用同一函数，避免"测试专用副本"式假绿）。
fn facade_violations(src: &str) -> (Vec<String>, Vec<String>) {
    let (head, prelude_region) = split_regions(src);
    let top = value_groups(&head);
    let pre = value_groups(&prelude_region);
    let top_all = flatten(&top);

    let mut orphans = Vec::new();
    for (mod_key, names) in &pre {
        for n in names {
            if !top_all.contains(n.as_str()) {
                orphans.push(format!("{mod_key}::{n}"));
            }
        }
    }

    let mut family_gaps = Vec::new();
    for (mod_key, pre_names) in &pre {
        if pre_names.is_empty() {
            continue;
        }
        if let Some(top_names) = top.get(mod_key) {
            let pre_set: std::collections::BTreeSet<&String> = pre_names.iter().collect();
            for n in top_names {
                if !pre_set.contains(n) {
                    family_gaps.push(format!("{mod_key}::{n}"));
                }
            }
        }
    }
    (orphans, family_gaps)
}

#[test]
fn prelude_has_no_orphan_values() {
    let (orphans, _) = facade_violations(&lib_src());
    assert!(
        orphans.is_empty(),
        "prelude 暴露了顶层没有的值名（prelude 成了第二套门面）: {orphans:?}"
    );
}

#[test]
fn prelude_families_are_complete() {
    let (_, gaps) = facade_violations(&lib_src());
    assert!(
        gaps.is_empty(),
        "prelude 收了某模块的伴生类型却漏收同模块其它顶层类型（同族缺项）: {gaps:?}"
    );
}

/// 正对照：本锁必须**会触发**。夹具刻意造出一名孤儿 + 一处同族缺项，
/// 断言两者都被点名——恒假守卫等于没有守卫。
#[test]
fn lock_bites_on_planted_defects() {
    let fixture = r#"
pub use behavior_contract::{BehaviorContract, ContractCheckOutcome, ContractContext};
pub use tool_plan::{ToolPlan, ToolOp};
pub mod prelude {
    pub use crate::behavior_contract::{ContractCheckOutcome, ContractContext};
    pub use crate::ghost_module::{GhostType};
    pub use crate::tool_plan::{ToolPlan, ToolOp};
}
"#;
    let (orphans, gaps) = facade_violations(fixture);
    assert_eq!(orphans, vec!["ghost_module::GhostType".to_string()]);
    assert_eq!(
        gaps,
        vec!["behavior_contract::BehaviorContract".to_string()]
    );
}

/// 论域自证：解析器必须看得见真实 lib.rs 的两组语句（若锚点/形态改动导致
/// 解析为空集，前两条断言会因"两侧都空"而恒真——这条把它变成红）。
///
/// 同时打印规模：ADR 引用的 top/prelude 计数必须由**本锁自己的解析器**产出
/// （`cargo test -- --nocapture` 读数），不接受另行手算的快照。
#[test]
fn parser_sees_the_real_surface() {
    let (head, prelude_region) = split_regions(&lib_src());
    let top = value_groups(&head);
    let pre = value_groups(&prelude_region);
    assert!(
        top.len() > 40 && pre.len() > 30,
        "lib.rs 值级语句数异常（top={} prelude={}），解析锚点可能已漂移",
        top.len(),
        pre.len()
    );
    assert!(
        top.contains_key("behavior_contract"),
        "顶层应含 behavior_contract 值组（实测键集未命中，锚点漂移）"
    );
    let top_all = flatten(&top);
    let pre_all = flatten(&pre);
    let top_only = top_all.iter().filter(|n| !pre_all.contains(*n)).count();
    let pre_only = pre_all.iter().filter(|n| !top_all.contains(*n)).count();
    println!(
        "[facade-scale] stmt_top={} stmt_prelude={} value_top={} value_prelude={} \
         top_only={} pre_only={} absent_families={} missing_by_family={:?}",
        top.len(),
        pre.len(),
        top_all.len(),
        pre_all.len(),
        top_only,
        pre_only,
        pre.keys().filter(|k| !top.contains_key(k.as_str())).count(),
        top_only_by_family(&top, &pre_all),
    );
    assert!(
        !top_all.is_empty() && !pre_all.is_empty() && pre_only == 0,
        "规模自证失败（value_top={} value_prelude={} pre_only={}）",
        top_all.len(),
        pre_all.len(),
        pre_only
    );
}

/// 按 leaf 模块聚合「顶层有、prelude 缺」的项数，供口径注释与 ADR 核对分族规模。
fn top_only_by_family(
    top: &BTreeMap<String, Vec<String>>,
    pre_all: &std::collections::BTreeSet<String>,
) -> BTreeMap<String, usize> {
    let mut per: BTreeMap<String, usize> = BTreeMap::new();
    for (family, names) in top {
        let missing = names
            .iter()
            .filter(|n| !pre_all.contains(n.as_str()))
            .count();
        if missing > 0 {
            per.insert(family.clone(), missing);
        }
    }
    per
}
