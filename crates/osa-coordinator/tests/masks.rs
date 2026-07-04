//! SparseMask<T> 单元测试 — 验证稀疏掩码容器的核心方法
//!
//! 对应 SubTask 4.10:验证 is_active / select_top_k / empty / full 方法
//! 对应 SubTask 13.9:is_active HashSet O(1) 性能基准
//! 对应 SubTask 13.10:select_top_k 语义测试(按 scores 选 Top-K)

use osa_coordinator::SparseMask;
use std::time::Instant;

#[test]
fn test_empty_mask_is_all_sparse() {
    let mask: SparseMask<String> = SparseMask::empty();
    assert!(mask.active_ids.is_empty(), "空掩码不应有活跃项");
    assert_eq!(mask.sparsity_ratio, 1.0, "空掩码稀疏度应为 1.0");
    assert_eq!(mask.active_count(), 0, "空掩码活跃数应为 0");
}

#[test]
fn test_empty_mask_is_active_always_false() {
    let mask: SparseMask<String> = SparseMask::empty();
    assert!(
        !mask.is_active(&"any-id".to_string()),
        "空掩码 is_active 应始终返回 false"
    );
    assert!(!mask.is_active(&"".to_string()));
}

#[test]
fn test_full_mask_no_sparsity() {
    let ids = vec![
        "tool-1".to_string(),
        "tool-2".to_string(),
        "tool-3".to_string(),
    ];
    let mask = SparseMask::full(ids.clone());
    assert_eq!(mask.active_ids, ids, "全掩码应保留所有 ID");
    assert_eq!(mask.sparsity_ratio, 0.0, "全掩码稀疏度应为 0.0");
    assert_eq!(mask.active_count(), 3);
}

#[test]
fn test_full_mask_empty_input_returns_empty() {
    let mask: SparseMask<String> = SparseMask::full(Vec::new());
    assert!(mask.active_ids.is_empty(), "空输入的全掩码应退化为空掩码");
    assert_eq!(mask.sparsity_ratio, 1.0, "空输入的全掩码稀疏度应为 1.0");
}

#[test]
fn test_full_mask_is_active_always_true() {
    let ids = vec!["a".to_string(), "b".to_string()];
    let mask = SparseMask::full(ids.clone());
    for id in &ids {
        assert!(
            mask.is_active(id),
            "全掩码 is_active 应对包含的 ID 返回 true"
        );
    }
    assert!(
        !mask.is_active(&"c".to_string()),
        "全掩码 is_active 应对未包含的 ID 返回 false"
    );
}

#[test]
fn test_select_top_k_preserves_order() {
    // 评分降序,前 2 个为 Top-K
    let ids = vec![
        "a".to_string(),
        "b".to_string(),
        "c".to_string(),
        "d".to_string(),
    ];
    let scores = vec![0.9, 0.8, 0.5, 0.1];
    let mask = SparseMask::select_top_k(&ids, &scores, 2);
    assert_eq!(
        mask.active_ids,
        vec!["a".to_string(), "b".to_string()],
        "Top-K 应按评分降序保留前 K 个"
    );
}

#[test]
fn test_select_top_k_sparsity_calculation() {
    // 4 个中选 2 个:sparsity = 1 - 2/4 = 0.5
    let ids = vec![
        "a".to_string(),
        "b".to_string(),
        "c".to_string(),
        "d".to_string(),
    ];
    let scores = vec![0.9, 0.8, 0.5, 0.1];
    let mask = SparseMask::select_top_k(&ids, &scores, 2);
    assert!(
        (mask.sparsity_ratio - 0.5).abs() < 1e-6,
        "4 选 2 稀疏度应为 0.5"
    );
}

#[test]
fn test_select_top_k_k_exceeds_length() {
    let ids = vec!["a".to_string(), "b".to_string()];
    let scores = vec![0.5, 0.5];
    let mask = SparseMask::select_top_k(&ids, &scores, 10);
    assert_eq!(mask.active_ids, ids, "K 超过长度时应保留全部");
    assert_eq!(mask.sparsity_ratio, 0.0, "K 超过长度时稀疏度应为 0.0");
}

#[test]
fn test_select_top_k_empty_input() {
    let mask: SparseMask<String> = SparseMask::select_top_k(&[], &[], 5);
    assert!(mask.active_ids.is_empty(), "空输入应返回空掩码");
    assert_eq!(mask.sparsity_ratio, 1.0, "空输入稀疏度应为 1.0");
}

#[test]
fn test_select_top_k_zero_k() {
    let ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let scores = vec![0.5, 0.5, 0.5];
    let mask = SparseMask::select_top_k(&ids, &scores, 0);
    assert!(mask.active_ids.is_empty(), "K=0 应返回空掩码");
    assert_eq!(mask.sparsity_ratio, 1.0, "K=0 稀疏度应为 1.0");
}

#[test]
fn test_is_active_with_numeric_type() {
    // 验证泛型支持数值类型(i32 满足 Eq + Hash)
    let ids = vec![1, 2, 3, 4, 5];
    let scores = vec![0.9, 0.8, 0.7, 0.6, 0.5];
    let mask = SparseMask::select_top_k(&ids, &scores, 3);
    assert!(mask.is_active(&1), "数值类型 is_active 应正常工作");
    assert!(mask.is_active(&3));
    assert!(!mask.is_active(&4), "未选中的 ID 应返回 false");
    assert!(!mask.is_active(&100));
}

#[test]
fn test_is_active_with_tuple_type() {
    // 验证泛型支持元组类型((&str, i32) 满足 Eq + Hash)
    let ids = vec![("a", 1), ("b", 2), ("c", 3)];
    let scores = vec![0.9, 0.8, 0.5];
    let mask = SparseMask::select_top_k(&ids, &scores, 2);
    assert!(mask.is_active(&("a", 1)));
    assert!(mask.is_active(&("b", 2)));
    assert!(!mask.is_active(&("c", 3)));
}

#[test]
fn test_serde_roundtrip_preserves_state() {
    let ids = vec!["x".to_string(), "y".to_string(), "z".to_string()];
    let scores = vec![0.9, 0.5, 0.1];
    let original = SparseMask::select_top_k(&ids, &scores, 2);
    let json = serde_json::to_string(&original).expect("序列化失败");
    let restored: SparseMask<String> = serde_json::from_str(&json).expect("反序列化失败");
    assert_eq!(original, restored, "序列化往返应保持状态一致");
    // 反序列化后 active_set 应已重建,is_active 应正常工作
    assert!(
        restored.is_active(&"x".to_string()),
        "反序列化后 is_active 应正常工作"
    );
    assert!(restored.is_active(&"y".to_string()));
    assert!(!restored.is_active(&"z".to_string()));
}

#[test]
fn test_sparsity_method_returns_ratio() {
    let ids = vec!["a".to_string(), "b".to_string()];
    let scores = vec![0.9, 0.1];
    let mask = SparseMask::select_top_k(&ids, &scores, 1);
    assert!(
        (mask.sparsity() - 0.5).abs() < 1e-6,
        "sparsity() 应返回 sparsity_ratio"
    );
}

// ============================================================
// SubTask 13.10:select_top_k 语义测试 — 按 scores 选 Top-K
// ============================================================

/// 验证 select_top_k 按 scores 降序选 Top-K,而非取前 K 个
///
/// 给定 scores = [0.1, 0.9, 0.5, 0.8], k=2,
/// Top-K 应为 [0.9, 0.8] 对应的 ID(即 "b" 和 "d"),按评分降序排列
#[test]
fn test_select_top_k_semantics_by_scores() {
    let ids = vec![
        "a".to_string(),
        "b".to_string(),
        "c".to_string(),
        "d".to_string(),
    ];
    let scores = vec![0.1, 0.9, 0.5, 0.8];
    let mask = SparseMask::select_top_k(&ids, &scores, 2);

    // Top-K 应为评分最高的 2 个:0.9(b) 和 0.8(d),按评分降序
    assert_eq!(
        mask.active_ids,
        vec!["b".to_string(), "d".to_string()],
        "Top-K 应按 scores 降序选取,期望 [b(0.9), d(0.8)]"
    );
    assert_eq!(mask.active_count(), 2);
    assert!(mask.is_active(&"b".to_string()));
    assert!(mask.is_active(&"d".to_string()));
    assert!(!mask.is_active(&"a".to_string()));
    assert!(!mask.is_active(&"c".to_string()));
}

/// 验证 select_top_k 不消费 ids(借用语义,调用方可复用)
#[test]
fn test_select_top_k_borrows_ids_reusable() {
    let ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let scores = vec![0.3, 0.9, 0.5];
    let mask = SparseMask::select_top_k(&ids, &scores, 1);
    // ids 仍可用(未被 move)
    assert_eq!(ids.len(), 3, "select_top_k 应借用 ids,调用后 ids 仍可用");
    assert_eq!(
        mask.active_ids,
        vec!["b".to_string()],
        "Top-1 应为评分最高的 b(0.9)"
    );
}

/// 验证 select_top_k 处理评分并列的情况(稳定排序,索引小的在前)
#[test]
fn test_select_top_k_tied_scores() {
    let ids = vec![
        "a".to_string(),
        "b".to_string(),
        "c".to_string(),
        "d".to_string(),
    ];
    let scores = vec![0.5, 0.5, 0.5, 0.5]; // 全部并列
    let mask = SparseMask::select_top_k(&ids, &scores, 2);
    // 评分并列时,选前 K 个(索引小的优先)
    assert_eq!(mask.active_count(), 2);
    assert!(mask.is_active(&"a".to_string()));
    assert!(mask.is_active(&"b".to_string()));
}

// ============================================================
// SubTask 13.9:is_active HashSet O(1) 性能基准
// ============================================================

/// 验证 is_active 使用 HashSet 实现 O(1) 查找,1000 个 ID 时 P50 < 100ns
///
/// WHY:原 Vec::iter().any() 为 O(n),1000 个 ID 约 1μs;
/// 改用 HashSet::contains 后为 O(1),预期 < 100ns。
///
/// 测量方法:warmup 10 次 + 100 轮(每轮 1000 次调用取平均)取 P50。
/// WHY 用 black_box:防止编译器优化掉 `is_active` 调用(否则测量的是空循环)。
/// WHY 用 1000 次循环:Windows 上 Instant::now() 开销约 100ns,
/// 1000 次调用取平均可将 Instant::now() 开销分摊到 0.1ns/次,获得准确测量
#[test]
#[ignore = "perf: run with --ignored"]
fn test_is_active_hashset_o1_performance() {
    // 构造 1000 个 ID 的掩码,选前 500 个为活跃
    let ids: Vec<String> = (0..1000).map(|i| format!("id-{i}")).collect();
    let scores: Vec<f32> = (0..1000).map(|i| 1.0 - i as f32 / 1000.0).collect();
    let mask = SparseMask::select_top_k(&ids, &scores, 500);

    // 查询一个不在活跃集中的 ID(最坏情况:HashSet 需检查桶)
    let target = "id-999".to_string();

    // Warmup 10 次(触发 CPU 缓存预热)
    for _ in 0..10 {
        let _ = std::hint::black_box(mask.is_active(&target));
    }

    // 正式测量:100 轮,每轮 1000 次调用取平均,收集 P50
    let iterations = 1000;
    let mut latencies = Vec::with_capacity(100);
    for _ in 0..100 {
        let start = Instant::now();
        for _ in 0..iterations {
            // black_box 防止编译器优化掉 is_active 调用
            let _ = std::hint::black_box(mask.is_active(&target));
        }
        // 平均延迟 = 总时间 / 1000(降低 Instant::now() 开销影响)
        latencies.push(start.elapsed().as_nanos() as f64 / iterations as f64);
    }
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = latencies[50];

    assert!(
        p50 < 500.0,
        "is_active P50 延迟 {:.2}ns 应 < 500ns(HashSet O(1) 查找,Windows 上放宽阈值)",
        p50
    );
}

/// 验证 is_active 在活跃 ID 存在时也保持 O(1) 性能
#[test]
#[ignore = "perf: run with --ignored"]
fn test_is_active_hashset_o1_performance_hit() {
    let ids: Vec<String> = (0..1000).map(|i| format!("id-{i}")).collect();
    let scores: Vec<f32> = (0..1000).map(|i| 1.0 - i as f32 / 1000.0).collect();
    let mask = SparseMask::select_top_k(&ids, &scores, 500);

    // 查询一个在活跃集中的 ID(命中情况)
    let target = "id-0".to_string();
    assert!(mask.is_active(&target));

    // Warmup
    for _ in 0..10 {
        let _ = std::hint::black_box(mask.is_active(&target));
    }

    let iterations = 1000;
    let mut latencies = Vec::with_capacity(100);
    for _ in 0..100 {
        let start = Instant::now();
        for _ in 0..iterations {
            let _ = std::hint::black_box(mask.is_active(&target));
        }
        latencies.push(start.elapsed().as_nanos() as f64 / iterations as f64);
    }
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = latencies[50];

    assert!(
        p50 < 500.0,
        "is_active(命中) P50 延迟 {:.2}ns 应 < 500ns(HashSet O(1) 查找,Windows 上放宽阈值)",
        p50
    );
}
