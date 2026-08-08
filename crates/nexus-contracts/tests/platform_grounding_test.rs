//! PlatformGroundingSpec 测试（Milestone B-4，北大 NL2Pipeline gap 解）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §5.1 P2 / §6 B-4）：
//! 平台接地规格将平台/环境约束固化为可审计契约（BehaviorContract 覆盖部分
//! 语义，本模块补齐平台维度），供 RuntimeAuditor 第 0 维度（契约遵守）消费。

#![forbid(unsafe_code)]

use nexus_contracts::platform_grounding::{
    GroundingCategory, GroundingCheckOutcome, PlatformGroundingSpec,
};

/// 从文档骨架提取：解析 "PG-<category>: <desc>" 行
#[test]
fn from_doc_extracts_requirement_skeleton() {
    let doc = "\
# 平台约束文档
PG-ENV: CARGO_HOME 指向项目工具链
PG-TOOLCHAIN: GNU 工具链 stable-x86_64-pc-windows-gnu
PG-PATH: msys64/mingw64/bin 在 PATH 中
非接地行应被忽略
";
    let spec = PlatformGroundingSpec::from_doc("pg-win", "windows-gnu", doc);
    assert_eq!(spec.requirements.len(), 3, "应提取 3 条要求: {spec:?}");
    assert_eq!(spec.requirements[0].category, GroundingCategory::Env);
    assert_eq!(spec.requirements[1].category, GroundingCategory::Toolchain);
}

/// 全部要求满足 → Grounded
#[test]
fn check_grounded_when_all_requirements_met() {
    let spec = PlatformGroundingSpec::from_doc(
        "pg-1",
        "windows-gnu",
        "PG-ENV: 工具链就位\nPG-PATH: 路径可用",
    );
    let observed = vec!["工具链就位".to_string(), "路径可用".to_string()];
    assert_eq!(spec.check(&observed), GroundingCheckOutcome::Grounded);
}

/// 缺失要求 → Violated 且列出缺失项
#[test]
fn check_violated_lists_missing_requirements() {
    let spec = PlatformGroundingSpec::from_doc(
        "pg-1",
        "windows-gnu",
        "PG-ENV: 工具链就位\nPG-PATH: 路径可用",
    );
    let observed = vec!["工具链就位".to_string()];
    match spec.check(&observed) {
        GroundingCheckOutcome::Violated { missing } => {
            assert_eq!(missing.len(), 1);
            assert!(missing[0].contains("路径可用"));
        }
        other => panic!("应 Violated: {other:?}"),
    }
}

/// 空文档 → 空规格（Grounded 退化：无要求即无违反）
#[test]
fn empty_doc_yields_grounded() {
    let spec = PlatformGroundingSpec::from_doc("pg-empty", "linux", "");
    assert!(spec.requirements.is_empty());
    assert_eq!(spec.check(&[]), GroundingCheckOutcome::Grounded);
}

/// 五类标记全覆盖解析
#[test]
fn all_categories_parse_from_doc() {
    let spec = PlatformGroundingSpec::from_doc(
        "pg-all",
        "linux",
        "PG-ENV: env\nPG-TOOLCHAIN: toolchain\nPG-PATH: path\nPG-PERMISSION: perm\nPG-CONFIG: cfg",
    );
    assert_eq!(spec.requirements.len(), 5);
    let categories: Vec<_> = spec.requirements.iter().map(|r| r.category).collect();
    assert!(categories.contains(&GroundingCategory::Env));
    assert!(categories.contains(&GroundingCategory::Toolchain));
    assert!(categories.contains(&GroundingCategory::Path));
    assert!(categories.contains(&GroundingCategory::Permission));
    assert!(categories.contains(&GroundingCategory::Config));
}

/// serde 序列化往返
#[test]
fn spec_serde_roundtrip() {
    let spec = PlatformGroundingSpec::from_doc("pg-rt", "darwin", "PG-ENV: 工具链就位");
    let json = serde_json::to_string(&spec).expect("序列化应成功");
    let back: PlatformGroundingSpec = serde_json::from_str(&json).expect("反序列化应成功");
    assert_eq!(back, spec);
}

/// 空观测 → 全部要求缺失
#[test]
fn check_violated_on_empty_observation() {
    let spec = PlatformGroundingSpec::from_doc("pg-1", "windows-gnu", "PG-ENV: 工具链就位");
    assert!(matches!(spec.check(&[]), GroundingCheckOutcome::Violated { .. }));
}
