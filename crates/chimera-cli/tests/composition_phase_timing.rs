//! 组合根分段计时在场锁（ADR-275 / W9）
//!
//! 两件事同时锁：
//! 1. 5 个构造段的计时字段与 `composition phase timings` 日志必须在场
//!    （否则「装配并行」这类决策又会退回到没有数据的直觉判断）；
//! 2. 组合根**不得**出现装配期并行原语（`thread::spawn` / `join!` / `thread::scope`）——
//!    ADR-275 D2 已按实测账否决（四段合计 1.31ms vs 3 次 spawn ≈0.88ms）。
//!    这条是负向锁：一旦有人偷偷并行，本测试先红，而不是等到 SLO 漂移才发现。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom
#![forbid(unsafe_code)]

use std::path::Path;

fn composition_source() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("composition.rs");
    std::fs::read_to_string(&path).expect("应能读取 composition.rs")
}

#[test]
fn composition_phase_timing_present() {
    let src = composition_source();

    assert!(
        src.contains("\"composition phase timings\""),
        "分段计时日志必须在场（ADR-275 D1）"
    );
    for field in ["gea_ms", "gqep_ms", "sched_ms", "scc_ms", "tool_runtime_ms"] {
        assert!(
            src.matches(field).count() >= 1,
            "缺分段字段 {field} —— 装配成本无法归因"
        );
    }

    // 负向锁：装配期并行已被否决（ADR-275 D2）。
    // WHY 只看代码行：本仓注释里会引用这两个名字来解释「为什么不用」——
    // 裸词扫描会把说明文字当成违规（与 ADR-271 的同类教训一致）。
    let code: String = src
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in ["thread::spawn", "join!", "thread::scope"] {
        assert!(
            !code.contains(forbidden),
            "组合根不得引入装配期并行原语 `{forbidden}`（ADR-275 D2 已按实测账否决）"
        );
    }
}
