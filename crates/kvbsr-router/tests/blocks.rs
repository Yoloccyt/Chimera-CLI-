//! SubTask 5.12:语义块构建器单元测试
//!
//! 验证 `BlockBuilder::build_blocks` 在 300 工具规模下的聚类行为:
//! - 300 工具聚类为 10-30 个语义块(15 块 × 20 工具)
//! - 块向量维度 = 64
//! - 块一致性 ∈ [0.0, 1.0]
//! - 块 ID 为 UUIDv7 格式(36 字符,可解析)
//! - 所有工具都被分配到某个块(无遗漏)
//! - 块内工具数量 = 20(每块 20 工具,与测试数据设计一致)
//!
//! # 测试数据设计(见 tests/common/mod.rs)
//! - 15 块 × 20 工具 = 300 工具
//! - 块内共现 150 次(> 阈值 100),块间无共现
//! - 期望:聚类后得到 15 个块,每块 20 工具

mod common;

use common::{NUM_BLOCKS, TOOLS_PER_BLOCK, TOTAL_TOOLS, VECTOR_DIM};
use kvbsr_router::{BlockBuilder, CoOccurrenceMatrix, KvbsrConfig, ToolId, ToolVector};
use uuid::Uuid;

/// 验证 300 工具聚类为 15 个语义块(在 10-30 范围内)
///
/// 测试数据设计:15 块 × 20 工具,块内共现 150(> 阈值 100),块间无共现。
/// 期望聚类结果:15 个块,每块 20 工具。
#[test]
fn test_blocks_count_in_expected_range() {
    let (tools, co) = common::generate_test_data();
    assert_eq!(tools.len(), TOTAL_TOOLS);

    let builder = BlockBuilder::new(KvbsrConfig::default());
    let blocks = builder.build_blocks(tools, &co);

    // 期望 15 个块(在 10-30 范围内,符合 SubTask 5.12 要求)
    assert_eq!(
        blocks.len(),
        NUM_BLOCKS,
        "300 工具应聚类为 {} 个块,实际 {} 个",
        NUM_BLOCKS,
        blocks.len()
    );
    assert!(
        blocks.len() >= 10 && blocks.len() <= 30,
        "块数量 {} 不在 10-30 范围内",
        blocks.len()
    );
}

/// 验证块向量维度 = 64
#[test]
fn test_block_vector_dimension_is_64() {
    let (tools, co) = common::generate_test_data();
    let builder = BlockBuilder::new(KvbsrConfig::default());
    let blocks = builder.build_blocks(tools, &co);

    assert!(!blocks.is_empty(), "块列表不应为空");
    for (i, block) in blocks.iter().enumerate() {
        assert_eq!(
            block.dimension(),
            VECTOR_DIM,
            "块 {} 的向量维度应为 {},实际 {}",
            i,
            VECTOR_DIM,
            block.dimension()
        );
    }
}

/// 验证块一致性 ∈ [0.0, 1.0]
///
/// 块内工具向量 = 基向量 + 小扰动,与块向量(加权平均)的余弦相似度应较高。
#[test]
fn test_block_coherence_in_valid_range() {
    let (tools, co) = common::generate_test_data();
    let builder = BlockBuilder::new(KvbsrConfig::default());
    let blocks = builder.build_blocks(tools, &co);

    for block in &blocks {
        assert!(
            block.block_coherence >= 0.0 && block.block_coherence <= 1.0,
            "块 {} 的一致性 {} 不在 [0.0, 1.0] 范围内",
            block.block_id,
            block.block_coherence
        );
        // 块内工具向量 = 基向量 + 小扰动,一致性应较高(> 0.5)
        assert!(
            block.block_coherence > 0.5,
            "块 {} 的一致性 {} 过低,可能聚类错误",
            block.block_id,
            block.block_coherence
        );
    }
}

/// 验证块 ID 为 UUIDv7 格式(36 字符,可解析为 Uuid)
#[test]
fn test_block_id_is_uuidv7_format() {
    let (tools, co) = common::generate_test_data();
    let builder = BlockBuilder::new(KvbsrConfig::default());
    let blocks = builder.build_blocks(tools, &co);

    for block in &blocks {
        // UUIDv7 字符串长度 36(含连字符)
        assert_eq!(
            block.block_id.len(),
            36,
            "块 ID '{}' 长度不为 36",
            block.block_id
        );
        // 可解析为 Uuid
        assert!(
            Uuid::parse_str(&block.block_id).is_ok(),
            "块 ID '{}' 不是有效的 UUID",
            block.block_id
        );
        // UUIDv7 的 version 字段应为 7(SortRand)
        let uuid = Uuid::parse_str(&block.block_id).expect("已验证可解析");
        assert_eq!(
            uuid.get_version(),
            Some(uuid::Version::SortRand),
            "块 ID '{}' 不是 UUIDv7",
            block.block_id
        );
    }
}

/// 验证所有工具都被分配到某个块(无遗漏)
///
/// 300 工具应全部出现在块的 tools 列表中,无遗漏。
#[test]
fn test_all_tools_assigned_to_blocks() {
    let (tools, co) = common::generate_test_data();
    let original_tool_ids: std::collections::HashSet<ToolId> =
        tools.iter().map(|t| t.tool_id.clone()).collect();
    let builder = BlockBuilder::new(KvbsrConfig::default());
    let blocks = builder.build_blocks(tools, &co);

    // 收集所有块中的工具 ID
    let mut assigned_tool_ids: std::collections::HashSet<ToolId> = std::collections::HashSet::new();
    for block in &blocks {
        for tid in &block.tools {
            assert!(
                assigned_tool_ids.insert(tid.clone()),
                "工具 {} 被重复分配到多个块",
                tid
            );
        }
    }

    // 所有原始工具都应被分配
    assert_eq!(
        assigned_tool_ids.len(),
        TOTAL_TOOLS,
        "分配的工具数 {} 不等于总工具数 {}",
        assigned_tool_ids.len(),
        TOTAL_TOOLS
    );
    for original_id in &original_tool_ids {
        assert!(
            assigned_tool_ids.contains(original_id),
            "工具 {} 未被分配到任何块",
            original_id
        );
    }
}

/// 验证每块工具数量 = 20(与测试数据设计一致)
#[test]
fn test_tools_per_block_count() {
    let (tools, co) = common::generate_test_data();
    let builder = BlockBuilder::new(KvbsrConfig::default());
    let blocks = builder.build_blocks(tools, &co);

    assert_eq!(blocks.len(), NUM_BLOCKS);
    for (i, block) in blocks.iter().enumerate() {
        assert_eq!(
            block.tool_count(),
            TOOLS_PER_BLOCK,
            "块 {} 的工具数应为 {},实际 {}",
            i,
            TOOLS_PER_BLOCK,
            block.tool_count()
        );
    }
}

/// 验证空工具列表返回空块列表
#[test]
fn test_empty_tools_returns_empty_blocks() {
    let builder = BlockBuilder::new(KvbsrConfig::default());
    let blocks = builder.build_blocks(Vec::new(), &CoOccurrenceMatrix::new());
    assert!(blocks.is_empty(), "空工具列表应返回空块列表");
}

/// 验证无共现记录时每个工具独立成块
#[test]
fn test_no_co_occurrence_each_tool_own_block() {
    let tools = vec![
        ToolVector::new("t1", vec![1.0; VECTOR_DIM], 100),
        ToolVector::new("t2", vec![1.0; VECTOR_DIM], 100),
        ToolVector::new("t3", vec![1.0; VECTOR_DIM], 100),
    ];
    let builder = BlockBuilder::new(KvbsrConfig::default());
    let blocks = builder.build_blocks(tools, &CoOccurrenceMatrix::new());
    assert_eq!(blocks.len(), 3, "无共现时每个工具应独立成块");
    for block in &blocks {
        assert_eq!(block.tool_count(), 1);
    }
}

/// 验证共现频率 = 阈值时不合并(严格大于才合并)
#[test]
fn test_co_occurrence_at_threshold_not_merged() {
    let tools = vec![
        ToolVector::new("t1", vec![1.0; VECTOR_DIM], 100),
        ToolVector::new("t2", vec![1.0; VECTOR_DIM], 100),
    ];
    let mut co = CoOccurrenceMatrix::new();
    // 共现 100 次 = 阈值(不满足 > 100),不应合并
    co.insert("t1", "t2", 100);
    let builder = BlockBuilder::new(KvbsrConfig::default());
    let blocks = builder.build_blocks(tools, &co);
    assert_eq!(blocks.len(), 2, "共现 = 阈值时不应合并");
}

/// 验证共现频率 > 阈值时合并为同一块
#[test]
fn test_co_occurrence_above_threshold_merged() {
    let tools = vec![
        ToolVector::new("t1", vec![1.0; VECTOR_DIM], 100),
        ToolVector::new("t2", vec![1.0; VECTOR_DIM], 100),
    ];
    let mut co = CoOccurrenceMatrix::new();
    // 共现 150 次 > 阈值 100,应合并
    co.insert("t1", "t2", 150);
    let builder = BlockBuilder::new(KvbsrConfig::default());
    let blocks = builder.build_blocks(tools, &co);
    assert_eq!(blocks.len(), 1, "共现 > 阈值时应合并为同一块");
    assert_eq!(blocks[0].tool_count(), 2);
}

/// 验证传递性合并:t1-t2 共现,t2-t3 共现,三者应合并为同一块
#[test]
fn test_transitive_merge() {
    let tools = vec![
        ToolVector::new("t1", vec![1.0; VECTOR_DIM], 100),
        ToolVector::new("t2", vec![1.0; VECTOR_DIM], 100),
        ToolVector::new("t3", vec![1.0; VECTOR_DIM], 100),
    ];
    let mut co = CoOccurrenceMatrix::new();
    co.insert("t1", "t2", 150);
    co.insert("t2", "t3", 150);
    // t1-t3 无直接共现,但通过 t2 传递合并
    let builder = BlockBuilder::new(KvbsrConfig::default());
    let blocks = builder.build_blocks(tools, &co);
    assert_eq!(blocks.len(), 1, "传递性合并应将 t1-t2-t3 归入同一块");
    assert_eq!(blocks[0].tool_count(), 3);
}

/// 验证自定义配置(block_vector_dim = 128)
#[test]
fn test_custom_block_vector_dim() {
    let dim = 128;
    let tools = vec![
        ToolVector::new("t1", vec![1.0; dim], 100),
        ToolVector::new("t2", vec![1.0; dim], 100),
    ];
    let mut co = CoOccurrenceMatrix::new();
    co.insert("t1", "t2", 150);
    let config = KvbsrConfig::default().with_block_vector_dim(dim);
    let builder = BlockBuilder::new(config);
    let blocks = builder.build_blocks(tools, &co);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].dimension(), dim);
}

/// 验证块向量 = 工具向量的加权平均(权重 = usage_count)
#[test]
fn test_weighted_average_block_vector() {
    // t1 usage=100, t2 usage=300,块向量应偏向 t2
    let tools = vec![
        ToolVector::new("t1", vec![1.0, 0.0], 100),
        ToolVector::new("t2", vec![0.0, 1.0], 300),
    ];
    let mut co = CoOccurrenceMatrix::new();
    co.insert("t1", "t2", 150);
    let config = KvbsrConfig::default().with_block_vector_dim(2);
    let builder = BlockBuilder::new(config);
    let blocks = builder.build_blocks(tools, &co);
    assert_eq!(blocks.len(), 1);
    // 加权平均:(1*100 + 0*300)/400 = 0.25, (0*100 + 1*300)/400 = 0.75
    assert!((blocks[0].block_vector[0] - 0.25).abs() < 1e-5);
    assert!((blocks[0].block_vector[1] - 0.75).abs() < 1e-5);
}

/// 验证块内工具的去重(同一工具不应在同一块中出现两次)
#[test]
fn test_no_duplicate_tools_in_block() {
    let (tools, co) = common::generate_test_data();
    let builder = BlockBuilder::new(KvbsrConfig::default());
    let blocks = builder.build_blocks(tools, &co);

    for block in &blocks {
        let mut seen = std::collections::HashSet::new();
        for tid in &block.tools {
            assert!(
                seen.insert(tid.clone()),
                "工具 {} 在块 {} 中重复出现",
                tid,
                block.block_id
            );
        }
    }
}

/// 验证块间工具不重叠(同一工具不应出现在多个块中)
#[test]
fn test_no_tool_overlap_between_blocks() {
    let (tools, co) = common::generate_test_data();
    let builder = BlockBuilder::new(KvbsrConfig::default());
    let blocks = builder.build_blocks(tools, &co);

    let mut global_seen = std::collections::HashSet::new();
    for block in &blocks {
        for tid in &block.tools {
            assert!(
                global_seen.insert(tid.clone()),
                "工具 {} 出现在多个块中",
                tid
            );
        }
    }
}

/// SubTask 10.8:验证 10 任务并发 record_co_occurrence 计数准确
///
/// CoOccurrenceMatrix 内部用 `HashMap` 存储,非线程安全,
/// 需用 `Arc<Mutex<CoOccurrenceMatrix>>` 包装。
/// 10 个 tokio::spawn 并发 insert 不同的共现对,
/// 验证最终计数准确(无丢失、无覆盖)。
#[tokio::test]
async fn test_concurrent_record_co_occurrence() {
    use std::sync::{Arc, Mutex};

    let co = Arc::new(Mutex::new(CoOccurrenceMatrix::new()));

    // 10 任务并发 insert,每个任务插入 10 个共现对(共 100 对)
    let mut handles = Vec::with_capacity(10);
    for tid in 0..10 {
        let co_clone = co.clone();
        handles.push(tokio::spawn(async move {
            for i in 0..10 {
                let a = format!("tool-{tid}");
                let b = format!("tool-{}-{}", tid, i);
                let mut co = co_clone.lock().unwrap();
                // WHY:`CoOccurrenceMatrix::insert` 接受 `impl Into<ToolId>`,
                // `ToolId` 实现了 `From<&str>` 但未实现 `From<&String>`,
                // 用 `as_str()` 将 `&String` 转为 `&str`。
                co.insert(a.as_str(), b.as_str(), 1);
            }
        }));
    }

    // 等待所有任务完成,验证无 panic
    for handle in handles {
        handle.await.unwrap();
    }

    // 验证计数准确:10 任务 × 10 对 = 100 个共现对
    let co = co.lock().unwrap();
    assert_eq!(co.len(), 100, "应有 100 个共现对(10 任务 × 10 对)");

    // 验证每个共现对的计数为 1(无丢失、无覆盖)
    for tid in 0..10 {
        for i in 0..10 {
            let a = format!("tool-{tid}");
            let b = format!("tool-{}-{}", tid, i);
            assert_eq!(co.get(&a, &b), 1, "共现对 ({}, {}) 的计数应为 1", a, b);
        }
    }
}

/// SubTask 15.9:验证仅 1 个工具时 build_blocks 返回 1 个块
#[test]
fn test_single_tool_returns_single_block() {
    let tools = vec![ToolVector::new("t1", vec![1.0; VECTOR_DIM], 100)];
    let builder = BlockBuilder::new(KvbsrConfig::default());
    let blocks = builder.build_blocks(tools, &CoOccurrenceMatrix::new());
    assert_eq!(blocks.len(), 1, "1 个工具应返回 1 个块");
    assert_eq!(blocks[0].tool_count(), 1, "块内工具数应为 1");
    assert_eq!(blocks[0].dimension(), VECTOR_DIM, "块向量维度应与配置一致");
}

/// SubTask 15.9:验证所有工具共现频率 > 阈值时归入 1 个巨块
#[test]
fn test_all_tools_co_occurrence_form_giant_block() {
    let n = 10;
    let mut tools = Vec::with_capacity(n);
    for i in 0..n {
        tools.push(ToolVector::new(format!("t{i}"), vec![1.0; VECTOR_DIM], 100));
    }
    let mut co = CoOccurrenceMatrix::new();
    // 所有工具两两共现 150 次(> 阈值 100),应全部归入同一块
    for i in 0..n {
        for j in (i + 1)..n {
            co.insert(format!("t{i}"), format!("t{j}"), 150);
        }
    }
    let builder = BlockBuilder::new(KvbsrConfig::default());
    let blocks = builder.build_blocks(tools, &co);
    assert_eq!(blocks.len(), 1, "所有工具共现 > 阈值应归入 1 个巨块");
    assert_eq!(blocks[0].tool_count(), n, "巨块应包含所有 {n} 个工具");
}

/// SubTask 15.9:验证工具向量维度与 block_vector_dim 不匹配时返回 InvalidConfig
///
/// WHY:KVBlockSemanticRouter::build_blocks 在系统边界校验工具向量维度,
/// 维度不一致时返回 InvalidConfig(而非静默截断导致语义失真)。
#[tokio::test]
async fn test_dimension_mismatch_returns_invalid_config() {
    use event_bus::EventBus;
    use kvbsr_router::{KVBlockSemanticRouter, KvbsrError};

    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);
    // 工具向量维度 32 != 默认 block_vector_dim 64
    let tools = vec![ToolVector::new("t1", vec![1.0; 32], 100)];
    let result = router.build_blocks(tools, CoOccurrenceMatrix::new()).await;
    assert!(
        matches!(result, Err(KvbsrError::InvalidConfig(_))),
        "维度不匹配应返回 InvalidConfig 错误,实际: {result:?}"
    );
}
