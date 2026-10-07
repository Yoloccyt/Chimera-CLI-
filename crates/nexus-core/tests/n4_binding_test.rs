//! WI-G6b 接线测试 — N-4 批量失败语义绑定（DEFERRED_UNTIL_WIRING）一致性守卫
//!
//! # 背景（W4-2 批 B,2026-10-07;基座 = ADR-196 §8 认证）
//! N-4（7 点位批量失败语义统一改造）按负收益红线暂缓——`cpu-plane-c1c2-scale-bounding-
//! 2026-10-02.md` §3 裁定「N-4 应绑定到『接线到真实流量』的功能决策之后」；
//! `cpu-plane-closeout-2026-10-04.md` §7 以 7 点位姿态盘点 + 显式登记结案。
//! 本测试把该绑定从**报告散文**转正为**机器钩**（WI-G6b）:
//!
//! 1. `n4_binding_state_matches_wiring_signal` —— 绑定状态 ⇔ G-87 组合根口径
//!    接线信号（`scripts/compute_offload_reachability_freeze.txt` 的
//!    WIRED_COMPOSITION 行数）。零接线 ⇒ 维持 DEFERRED；**接线出现 ⇒ 本测试翻红**,
//!    强制重新裁决 N-4（不许静默漂移）。
//! 2. `n4_anchor_inventory_matches_code` —— 7 点位盘点与代码不失联：每个锚点文件
//!    必须存在且仍调 `.spawn_compute_batch(`（点位消失/迁移 ⇒ 盘点基线失联,翻红）。
//! 3. `n4_registry_is_wellformed` —— 登记册 schema 守卫（唯一绑定行 / 枚举姿势 /
//!    可解析触发器）。
//!
//! # 回滚
//! 删除本文件 + `scripts/n4_binding_registry.txt`（ADR-196 §9 转正注记同步撤除）
//! —— 「断开绑定」,零生产代码面。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code

use std::path::PathBuf;

/// 绑定登记册（include_str:编译期钉入,缺失即编译失败——防"测试悄悄没了")
const REGISTRY: &str = include_str!("../../../scripts/n4_binding_registry.txt");
/// G-87 可达性 freeze（组合根口径裁决的唯一机器基线）
const FREEZE: &str = include_str!("../../../scripts/compute_offload_reachability_freeze.txt");

/// 解析 freeze 中 WIRED_COMPOSITION 行数（列格式见 freeze 头注:path::fn|verdict|anchors|api）
fn wired_composition_count(freeze: &str) -> usize {
    freeze
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter(|l| l.split('|').nth(1) == Some("WIRED_COMPOSITION"))
        .count()
}

/// 解析绑定行 → (state, trigger_threshold)，触发器方言 `wired_composition>=N`
fn parse_binding_row() -> (String, usize) {
    let row = REGISTRY
        .lines()
        .find(|l| l.starts_with("binding:n4_batch_failure_semantics|"))
        .expect("绑定行缺失：scripts/n4_binding_registry.txt 应含唯一 n4 绑定行");
    let fields: Vec<&str> = row.split('|').collect();
    assert_eq!(
        fields.len(),
        4,
        "绑定行 schema: binding:<key>|<state>|<trigger>|<owner>"
    );
    let state = fields[1].to_string();
    let trigger = fields[2];
    let threshold: usize = trigger
        .strip_prefix("wired_composition>=")
        .expect("触发器方言须为 wired_composition>=N")
        .trim()
        .parse()
        .expect("触发器阈值须为整数");
    (state, threshold)
}

/// 核心绑定：状态 ⇔ 接线信号。零接线维持 DEFERRED；信号触达 ⇒ 强制翻红重新裁决。
#[test]
fn n4_binding_state_matches_wiring_signal() {
    let wired = wired_composition_count(FREEZE);
    let (state, threshold) = parse_binding_row();
    if wired >= threshold {
        assert_ne!(
            state, "DEFERRED_UNTIL_WIRING",
            "N-4 绑定触发：G-87 WIRED_COMPOSITION={wired} >= {threshold} —— \
             「接线到真实流量」功能决策已发生,必须重新裁决 N-4 批量失败语义\
             （见 ADR-196 §9 / cpu-plane-c1c2 §3）,不得继续静默暂缓"
        );
    } else {
        assert_eq!(
            state, "DEFERRED_UNTIL_WIRING",
            "零接线信号（WIRED_COMPOSITION={wired} < {threshold}）下绑定应维持 \
             DEFERRED_UNTIL_WIRING —— 状态被改动须与裁决记录同批（不得只改登记册）"
        );
    }
}

/// 盘点失联守卫：7 点位锚点文件必须存在且仍调 spawn_compute_batch。
#[test]
fn n4_anchor_inventory_matches_code() {
    let root: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    let mut anchors = 0usize;
    for line in REGISTRY.lines() {
        let Some(rest) = line.strip_prefix("anchor:") else {
            continue;
        };
        let fields: Vec<&str> = rest.split('|').collect();
        assert_eq!(
            fields.len(),
            3,
            "锚行 schema: anchor:<path>|<posture>|<api>"
        );
        let rel = fields[0];
        let path = root.join(rel);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("锚点文件缺失 {rel}: {e} —— 7 点位盘点失联"));
        assert!(
            text.contains(".spawn_compute_batch("),
            "{rel} 不再调 spawn_compute_batch —— 点位消失/迁移,姿态基线失联,须重新裁决 N-4"
        );
        anchors += 1;
    }
    assert_eq!(
        anchors, 7,
        "锚点数必须 = 7（cpu-plane-closeout-2026-10-04.md §7 逐字盘点）"
    );
}

/// schema 守卫：姿势枚举合法 + 绑定行唯一 + 注释外无杂行。
#[test]
fn n4_registry_is_wellformed() {
    const POSTURES: [&str; 3] = ["fail-fast", "item-wise", "in-place-fallback"];
    let mut binding_rows = 0usize;
    let mut rows = 0usize;
    for line in REGISTRY.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        rows += 1;
        if line.starts_with("binding:") {
            binding_rows += 1;
        } else if let Some(rest) = line.strip_prefix("anchor:") {
            let posture = rest.split('|').nth(1).unwrap_or("");
            assert!(
                POSTURES.contains(&posture),
                "未知姿势类 {posture:?}（合法值: {POSTURES:?}）"
            );
        } else {
            panic!("未知行前缀: {line}");
        }
    }
    assert_eq!(binding_rows, 1, "绑定行必须唯一");
    assert_eq!(rows, 8, "登记册 = 1 绑定行 + 7 锚行");
}
