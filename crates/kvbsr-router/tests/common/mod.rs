//! KVBSR 测试公共辅助模块 — 测试数据生成与工具函数
//!
//! 提供 300 工具规模的测试数据生成函数,供 blocks/router/rebalancer 测试复用。
//!
//! # 测试数据设计
//! - 15 块 × 20 工具 = 300 工具
//! - 每个块的基向量在 2 个独特维度上有高值(确保块间区分度)
//! - 块内工具向量 = 基向量 + 小扰动(确保块内相似度)
//! - 块内共现 > 阈值(150),块间无共现
//! - 20 条标注用例:CLV 前 64 维 = 块基向量 + 小扰动,正确工具 = tool-{bi}-10

// 允许未使用的函数:本模块被多个测试文件共享(blocks/router/rebalancer),
// 某些函数可能仅在部分测试文件中使用,在每个测试文件的独立编译单元中
// 会产生 dead_code 警告。此处统一抑制,避免 clippy `-D warnings` 失败。
#![allow(dead_code)]

use kvbsr_router::{CoOccurrenceMatrix, ToolId, ToolVector};
use nexus_core::CLV;

/// 块数量(15 块,在 10-30 范围内)
pub const NUM_BLOCKS: usize = 15;

/// 每块工具数量(15 × 20 = 300 工具)
pub const TOOLS_PER_BLOCK: usize = 20;

/// 总工具数(300)
pub const TOTAL_TOOLS: usize = NUM_BLOCKS * TOOLS_PER_BLOCK;

/// 块/工具向量维度(64)
pub const VECTOR_DIM: usize = 64;

/// 生成块基向量 — 每个块在 2 个独特维度上有高值
///
/// WHY:独特维度确保块间余弦相似度接近 0,一级路由能准确区分块
pub fn block_base_vector(block_index: usize) -> Vec<f32> {
    let mut base = vec![0.0_f32; VECTOR_DIM];
    let d1 = (block_index * 4) % VECTOR_DIM;
    let d2 = (block_index * 4 + 1) % VECTOR_DIM;
    base[d1] = 1.0;
    base[d2] = 1.0;
    base
}

/// 生成测试工具向量与共现矩阵(15 块 × 20 工具 = 300 工具)
///
/// 返回 (tools, co_occurrence):
/// - tools:300 个 ToolVector,块内工具向量 = 基向量 + 小扰动
/// - co_occurrence:块内工具共现 150 次(> 阈值 100),块间无共现
pub fn generate_test_data() -> (Vec<ToolVector>, CoOccurrenceMatrix) {
    let mut tools = Vec::with_capacity(TOTAL_TOOLS);
    let mut co = CoOccurrenceMatrix::new();

    for bi in 0..NUM_BLOCKS {
        let base = block_base_vector(bi);
        for ti in 0..TOOLS_PER_BLOCK {
            // 块内工具向量 = 基向量 + 小扰动(ti × 0.01 - 0.1)
            // ti=10 时扰动=0,向量=基向量(与无扰动 CLV 完全匹配)
            let mut vector = base.clone();
            for v in vector.iter_mut() {
                *v += (ti as f32 * 0.01) - 0.1;
            }
            tools.push(ToolVector::new(format!("tool-{bi}-{ti}"), vector, 100));
        }
        // 块内工具共现 > 阈值(150 > 100)
        for ti in 0..TOOLS_PER_BLOCK {
            for tj in (ti + 1)..TOOLS_PER_BLOCK {
                co.insert(format!("tool-{bi}-{ti}"), format!("tool-{bi}-{tj}"), 150);
            }
        }
    }

    (tools, co)
}

/// 为指定块生成 CLV(前 64 维 = 块基向量 + 小扰动)
///
/// WHY:CLV 前 64 维与块基向量高度相似,路由器截取前 64 维后
/// 能准确匹配到对应块,确保路由准确率
pub fn generate_clv_for_block(block_index: usize, seed: f32) -> CLV {
    let mut clv_vec = vec![0.0_f32; CLV::DIMENSION];
    let base = block_base_vector(block_index);
    // 扰动 = seed × 0.01(小扰动,确保 CLV 与块基向量高度相似)
    for (i, &v) in base.iter().enumerate() {
        clv_vec[i] = v + seed * 0.01;
    }
    CLV::from_vec(clv_vec).expect("CLV 维度应为 512")
}

/// 生成 20 条标注测试用例(CLV, 正确工具 ID)
///
/// 每条用例的 CLV 匹配某个块,正确工具 = tool-{bi}-10
/// (ti=10 时工具向量 = 块基向量,与 CLV 最相似)
pub fn generate_test_cases() -> Vec<(CLV, String)> {
    let mut cases = Vec::with_capacity(20);
    for i in 0..20 {
        let bi = i % NUM_BLOCKS;
        let clv = generate_clv_for_block(bi, i as f32);
        // 正确工具 = tool-{bi}-10(向量 = 块基向量,与 CLV 最相似)
        let correct_tool = format!("tool-{bi}-10");
        cases.push((clv, correct_tool));
    }
    cases
}

/// 全量扫描基线 — 计算 CLV 与所有工具的相似度并排序
///
/// WHY:用于加速比测试,模拟无块路由的全量扫描。
/// 返回 `Vec<ToolId>` 与 newtype 化的 `ToolVector.tool_id` 类型一致,
/// 避免测试中不必要的 `String` ↔ `ToolId` 转换。
pub fn full_scan_baseline(clv: &CLV, tools: &[ToolVector], top_k: usize) -> (Vec<ToolId>, f32) {
    let start = std::time::Instant::now();
    let query = &clv.as_slice()[..VECTOR_DIM];
    let mut scored: Vec<(ToolId, f32)> = tools
        .iter()
        .map(|t| {
            let sim = cosine_sim(query, &t.vector);
            (t.tool_id.clone(), sim)
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let selected: Vec<ToolId> = scored
        .iter()
        .take(top_k)
        .map(|(id, _)| id.clone())
        .collect();
    let elapsed_ms = start.elapsed().as_secs_f32() * 1000.0;
    (selected, elapsed_ms)
}

/// 计算两个向量的余弦相似度(测试辅助)
fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    if len == 0 {
        return 0.0;
    }
    let mut dot = 0.0_f32;
    let mut na = 0.0_f32;
    let mut nb = 0.0_f32;
    for i in 0..len {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    let na = na.sqrt();
    let nb = nb.sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    (dot / (na * nb)).clamp(-1.0, 1.0)
}
