//! L0 值级 facade 一致性锁 —— L0-A（ADR-新-1）改版
//!
//! # 锁什么（改版后）
//! 顶层值级导出与 `pub mod prelude` 自 L0-A 起由 `l0_surface!` 宏从**同一份
//! 符号声明**生成（`crates/nexus-contracts/src/facade_surface.rs`），旧版
//! "两份手工清单对拍"的漂移面已被结构性消除："prelude 无孤儿"与"同族完备"
//! 由构造保证（同一符号列表物理上只有一份，prelude 成员资格是条目级标记）。
//!
//! 漂移风险并未归零，只是换了形状，本锁改守三处真实的剩余风险：
//!
//! 1. **声明面 ↔ 重构前基线等价**：`l0_surface!` 声明必须与
//!    `tests/fixtures/l0_surface_baseline.json`（L0-A 实施时从旧 lib.rs
//!    值级面抓取的基线，48 族 / 221 顶层值 / 199 prelude 值）逐项相等——
//!    这是宏化没有悄悄改公共面的等价锁。
//! 2. **无游离字面量**：lib.rs 不得再出现字面 `pub use <leaf>::{…};` 值级
//!    语句（那是旧双清单时代的形态）；绕过宏手写 re-export 会让双镜像复活，
//!    本锁直接判红。
//! 3. **声明良构**：模块名唯一、标记 ∈ {prelude, top}、符号非空且不重复
//!    （宏对这些形状要么编译错要么静默吞错，提前在测试里点名）。
//!
//! 真树与夹具共用同一组纯判定函数，避免"测试专用副本"式假绿。
#![allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

/// 一条 `l0_surface!` 声明条目：`module face { sym, … }`。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    module: String,
    face: String,
    syms: Vec<String>,
}

fn lib_src() -> String {
    let path = format!("{}/src/lib.rs", env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读取 {path} 失败: {e} —— 本锁的论域就是该文件"))
}

/// 去掉 `//` 行注释（声明体内无字符串字面量，行注释即全部噪声）。
fn strip_line_comments(src: &str) -> String {
    src.lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 切出 `l0_surface! { … }` 声明体（花括号配平）。
fn declaration_body(src: &str) -> String {
    let anchor = src
        .find("l0_surface! {")
        .expect("lib.rs 必须含 `l0_surface! {` 声明（本锁的锚点）");
    let mut depth = 0usize;
    let mut end = None;
    for (i, ch) in src[anchor..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(anchor + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end.expect("l0_surface! 声明花括号不配平");
    src[anchor + "l0_surface! {".len()..end].to_string()
}

/// 声明体词法切分：标识符与标点各自成 token。
fn tokenize(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in src.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            cur.push(ch);
        } else {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            if matches!(ch, '{' | '}' | ',' | ';') {
                out.push(ch.to_string());
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 解析声明体为条目列表（顺序保留，供规模读数与排错）。
fn parse_entries(body: &str) -> Result<Vec<Entry>, String> {
    let toks = tokenize(&strip_line_comments(body));
    let mut i = 0usize;
    let mut entries = Vec::new();
    while i < toks.len() {
        let module = toks[i].clone();
        if !is_ident(&module) {
            return Err(format!("位置 {i}: 期望模块名标识符，实测 `{}`", toks[i]));
        }
        i += 1;
        if i >= toks.len() {
            return Err(format!("模块 {module} 后缺少派发面标记"));
        }
        let face = toks[i].clone();
        if face != "prelude" && face != "top" {
            return Err(format!(
                "模块 {module} 的派发面标记 `{}` 非法（应为 prelude|top）",
                face
            ));
        }
        i += 1;
        if i >= toks.len() || toks[i] != "{" {
            return Err(format!("模块 {module} 的标记后缺少 `{{`"));
        }
        i += 1;
        let mut syms = Vec::new();
        loop {
            if i >= toks.len() {
                return Err(format!("模块 {module} 的符号列表未闭合"));
            }
            if toks[i] == "}" {
                i += 1;
                break;
            }
            if toks[i] == "," {
                i += 1;
                continue;
            }
            if is_ident(&toks[i]) {
                syms.push(toks[i].clone());
                i += 1;
            } else {
                return Err(format!(
                    "模块 {module} 的符号列表混入非法 token `{}`",
                    toks[i]
                ));
            }
        }
        entries.push(Entry { module, face, syms });
    }
    Ok(entries)
}

fn is_ident(s: &str) -> bool {
    let mut ch = s.chars();
    match ch.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    ch.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 顶层视图：模块 → 符号集（全部条目；宏为每个条目都生成顶层 re-export）。
fn top_view(entries: &[Entry]) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for e in entries {
        out.entry(e.module.clone())
            .or_default()
            .extend(e.syms.iter().cloned());
    }
    out
}

/// prelude 视图：仅 `prelude` 标记条目的模块 → 符号集。
fn prelude_view(entries: &[Entry]) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for e in entries.iter().filter(|e| e.face == "prelude") {
        out.entry(e.module.clone())
            .or_default()
            .extend(e.syms.iter().cloned());
    }
    out
}

/// 去掉 `//` 行注释后扫描 lib.rs 中的字面 `pub use <path>::{…};`
/// 值级语句（旧双清单时代的形态；宏化后源码不得存在——注释里的示例字样不算）。
fn stray_literal_value_reexports(src: &str) -> Vec<String> {
    let src = strip_line_comments(src);
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(rel) = src[i..].find("pub use ") {
        let start = i + rel + "pub use ".len();
        let Some(end) = src[start..].find(';') else {
            break;
        };
        let stmt = &src[start..start + end];
        i = start + end + 1;
        if let Some(open) = stmt.find("::{") {
            let path = stmt[..open].trim();
            out.push(format!("{path}::{{…}}"));
        }
    }
    out
}

/// 良构性 violations（真树与夹具共用）。
fn wellformed_violations(entries: &[Entry]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for e in entries {
        *seen.entry(e.module.clone()).or_default() += 1;
        if e.syms.is_empty() {
            out.push(format!("{}: 空符号列表", e.module));
        }
        let mut uniq: BTreeSet<&str> = BTreeSet::new();
        for s in &e.syms {
            if !uniq.insert(s.as_str()) {
                out.push(format!("{}: 符号 {s} 重复", e.module));
            }
        }
    }
    for (m, n) in seen {
        if n > 1 {
            out.push(format!("{m}: 重复声明 {n} 次"));
        }
    }
    out
}

fn load_baseline() -> serde_json::Value {
    let path = format!(
        "{}/tests/fixtures/l0_surface_baseline.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读取基线 {path} 失败: {e} —— L0-A 等价锁的固定点"));
    serde_json::from_str(&text).expect("基线 JSON 必须可解析")
}

fn json_map_to_sets(v: &serde_json::Value) -> BTreeMap<String, BTreeSet<String>> {
    v.as_object()
        .expect("基线面必须是对象")
        .iter()
        .map(|(k, arr)| {
            let set = arr
                .as_array()
                .expect("基线族必须是数组")
                .iter()
                .map(|s| s.as_str().expect("基线符号必须是字符串").to_string())
                .collect();
            (k.clone(), set)
        })
        .collect()
}

#[test]
fn surface_matches_pre_refactor_baseline() {
    let entries = parse_entries(&declaration_body(&lib_src())).expect("声明必须可解析");
    let baseline = load_baseline();
    let want_top = json_map_to_sets(&baseline["top"]);
    let want_pre = json_map_to_sets(&baseline["prelude"]);
    let got_top = top_view(&entries);
    let got_pre = prelude_view(&entries);
    assert_eq!(
        got_top, want_top,
        "l0_surface! 声明的顶层面与重构前基线不等价（宏化悄悄改了公共面？）"
    );
    assert_eq!(
        got_pre, want_pre,
        "l0_surface! 声明的 prelude 面与重构前基线不等价（派发面标记漂移？）"
    );
}

#[test]
fn no_stray_literal_value_reexports() {
    let stray = stray_literal_value_reexports(&lib_src());
    assert!(
        stray.is_empty(),
        "lib.rs 出现字面值级 `pub use <leaf>::{{…}};`（绕过宏会让双镜像复活）: {stray:?}"
    );
}

#[test]
fn declaration_is_wellformed() {
    let entries = parse_entries(&declaration_body(&lib_src())).expect("声明必须可解析");
    let bad = wellformed_violations(&entries);
    assert!(bad.is_empty(), "l0_surface! 声明良构性破坏: {bad:?}");
}

/// 论域自证：解析器必须看得见真实声明（若锚点漂移导致解析为空集，
/// 等价断言会因"两侧都空"而恒真——这条把它变成红）。同时打印规模读数。
#[test]
fn parser_sees_the_real_surface() {
    let entries = parse_entries(&declaration_body(&lib_src())).expect("声明必须可解析");
    let top = top_view(&entries);
    let pre = prelude_view(&entries);
    let top_n: usize = top.values().map(BTreeSet::len).sum();
    let pre_n: usize = pre.values().map(BTreeSet::len).sum();
    println!(
        "[facade-scale] entries={} families_top={} value_top={} families_prelude={} value_prelude={} top_only_families={}",
        entries.len(),
        top.len(),
        top_n,
        pre.len(),
        pre_n,
        top.keys().filter(|k| !pre.contains_key(*k)).count()
    );
    assert!(
        entries.len() >= 45 && entries.len() <= 55,
        "声明条目数异常（{}），解析锚点可能已漂移",
        entries.len()
    );
    assert!(
        top.contains_key("behavior_contract") && top.contains_key("experience_card"),
        "顶层应含 behavior_contract / experience_card 族（实测键集未命中，锚点漂移）"
    );
    assert!(
        !pre.contains_key("tool_plan") && !pre.contains_key("variant"),
        "tool_plan / variant 是有意只进顶层的族，不得出现在 prelude 视图"
    );
}

/// 正对照：本锁的每个判定都必须**会触发**。夹具同时埋下
/// ①游离字面量 ②重复模块 ③重复符号 ④非法派发面标记，逐一点名。
#[test]
fn lock_bites_on_planted_defects() {
    let stray_src = "pub use ghost_module::{GhostType};\n";
    assert_eq!(
        stray_literal_value_reexports(stray_src),
        vec!["ghost_module::{…}".to_string()]
    );

    let body = r#"
        capability_token prelude { CapabilityToken }
        capability_token prelude { CapabilityToken, SeamId }
        masks top { SparseMask, SparseMask }
    "#;
    let entries = parse_entries(body).expect("夹具主体应可解析");
    let bad = wellformed_violations(&entries);
    assert!(
        bad.iter()
            .any(|m| m.contains("capability_token: 重复声明 2 次")),
        "重复模块必须被点名: {bad:?}"
    );
    assert!(
        bad.iter().any(|m| m.contains("SparseMask 重复")),
        "重复符号必须被点名: {bad:?}"
    );
    let err = parse_entries("omni_masks sideways { OmniSparseMasks }")
        .expect_err("非法派发面标记必须解析失败");
    assert!(err.contains("sideways"), "解析错误必须点名非法标记: {err}");
}
