//! 变体隔离视图测试（Milestone D-1 前置：弱模型变体隔离评估的 Rust 侧载体）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §6 D-1）：
//! 弱模型验证（Qwen3.5-9B 等）的"变体隔离测试"——弱模型变体与强模型
//! 变体分开评估（不交叉污染性能基线）。隔离按变体 ID 标签约定
//! （不改 VariantContract 核心类型——领域类型稳定性，§3.3.1）。

#![forbid(unsafe_code)]

use nexus_contracts::variant::VariantId;
use parliament::variant_pool::VariantPool;

/// 注册混合变体：弱模型（weak- 标签）与强模型（无标签）
fn mixed_pool() -> VariantPool {
    let mut pool = VariantPool::new();
    pool.register(nexus_contracts::variant::VariantContract::new(
        VariantId::new("weak-qwen3.5-9b", 1),
        vec!["refactor".into()],
        0.6,
        0.1,
    ));
    pool.register(nexus_contracts::variant::VariantContract::new(
        VariantId::new("weak-qwen3.5-9b", 2),
        vec!["docs".into()],
        0.55,
        0.1,
    ));
    pool.register(nexus_contracts::variant::VariantContract::new(
        VariantId::new("strong-llm", 1),
        vec!["refactor".into()],
        0.9,
        0.05,
    ));
    pool
}

/// 隔离视图：weak 标签只返回弱模型变体（强模型不混入）
#[test]
fn isolated_view_filters_by_tag() {
    let pool = mixed_pool();
    let weak = pool.isolated("weak-");
    assert_eq!(weak.len(), 2, "应隔离出 2 个弱模型变体");
    assert!(
        weak.iter()
            .all(|c| c.variant_id.spec_name.contains("weak-")),
        "隔离视图只含 weak 标签变体"
    );
}

/// 强模型视图：不含 weak 变体（交叉污染防护）
#[test]
fn strong_view_excludes_weak_variants() {
    let pool = mixed_pool();
    let strong = pool.isolated("strong-");
    assert_eq!(strong.len(), 1);
    assert_eq!(strong[0].variant_id.spec_name, "strong-llm");
}

/// 隔离视图不修改池本身（纯查询）
#[test]
fn isolation_is_read_only() {
    let pool = mixed_pool();
    let before = pool.len();
    let _ = pool.isolated("weak-");
    assert_eq!(pool.len(), before, "隔离视图不应修改池");
}

/// 无匹配标签 → 空视图
#[test]
fn unknown_tag_yields_empty() {
    let pool = mixed_pool();
    assert!(pool.isolated("nonexistent-").is_empty());
}
