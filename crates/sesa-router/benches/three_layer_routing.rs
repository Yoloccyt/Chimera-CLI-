//! 三层路由组合基准 — SESA 激活 → KVBSR 路由 → FaaE 工具选择
//!
//! 对应 Task 4 (W7-Carryover-1):三层路由组合基准验证
//!
//! # 验收标准
//! - 1000 工具规模下,三层串联 p95 ≤ 5ms
//! - SESA 单层 p95 ≤ 5ms(Week 7 附录 C.1 已达标)
//! - KVBSR 单层 route_1000_tools 基准已存在(benches/route.rs)
//! - FaaE 单层 route_100_candidates 基准已存在(benches/route.rs)
//!
//! # 串联流程
//! 1. **SESA 激活**:从 256 专家中稀疏激活 Top-8(严格 < 40% 稀疏度)
//!    - 输出:SesaMask + SparsityProfile(激活位索引代表"热"专家域)
//! 2. **KVBSR 路由**:两级块路由(50 块 × 20 工具 = 1000 工具)
//!    - 输入:CLV(512 维,内部截取前 64 维)
//!    - 输出:RoutingResult.selected_tools(Top-8 候选工具 ID)
//! 3. **FaaE 工具选择**:从 KVBSR 候选中精筛最终工具
//!    - 输入:查询向量(64 维)+ 候选工具 ID 列表
//!    - 输出:RoutingResult.routed_tool(最终路由工具)+ 置信度
//!
//! # 运行
//! ```bash
//! # 仅编译验证(不运行)
//! cargo bench -p sesa-router --bench three_layer_routing --jobs 1 -- --no-run
//!
//! # 快速模式运行
//! cargo bench -p sesa-router --bench three_layer_routing --jobs 1 -- \
//!     --warm-up-time 1 --measurement-time 3 --sample-size 10
//! ```
//!
//! # 架构红线
//! - `#![forbid(unsafe_code)]`:与 workspace 所有 crate 一致
//! - 所有 async 通过 `Runtime::block_on` 在同步 criterion 上下文中调用
//! - 不修改 KVBSR/FaaE/SESA 源码,仅作为外部基准调用公共 API

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use event_bus::EventBus;
// SESA(本 crate,L6 Router)
use sesa_router::{ActivationRequest, ExpertDescriptor, SesaConfig, SesaRouter};
// KVBSR(L6 同层,dev-dependency)
use kvbsr_router::{CoOccurrenceMatrix, KVBlockSemanticRouter, ToolVector};
// FaaE(L6 同层,dev-dependency)
use faae_router::{ExpertProfile, FaaeRouter, ToolId as FaaeToolId};
// CLV(L1 Core)
use nexus_core::CLV;

// === 常量:1000 工具规模配置 ===

/// 工具总数(1000 工具规模)
const NUM_TOOLS: usize = 1000;
/// KVBSR 块数(50 块 × 20 工具/块 = 1000 工具)
const NUM_BLOCKS: usize = 50;
/// 每块工具数
const TOOLS_PER_BLOCK: usize = 20;
/// 语义向量维度(与 KVBSR block_vector_dim / FaaE expert_vector 对齐)
const DIM: usize = 64;
/// SESA 掩码容量上限(256-bit 位向量)
const SESA_EXPERT_COUNT: usize = 256;

/// 构造 1000 工具的语义向量(50 块 × 20 工具/块)
///
/// WHY 单维度基向量:50 块需要 50 个独特维度,64 维足够(50 < 64),无重叠。
/// 块内工具添加微小扰动保持块内区分度,但块间正交确保路由准确率。
fn make_tool_vectors() -> Vec<(String, Vec<f32>)> {
    (0..NUM_TOOLS)
        .map(|i| {
            let block = i / TOOLS_PER_BLOCK;
            let mut v = vec![0.0_f32; DIM];
            v[block] = 1.0; // 块基向量(块间正交)
                            // 微小扰动:保持块内区分度,不影响块间路由
            v[block] += (i as f32) * 0.0001;
            (format!("tool-{i}"), v)
        })
        .collect()
}

/// 构造 KVBSR 共现矩阵(块内工具共现频率高,驱动块构建)
fn make_cooccurrence() -> CoOccurrenceMatrix {
    let mut co = CoOccurrenceMatrix::new();
    for bi in 0..NUM_BLOCKS {
        // 块内工具两两共现,频率 150(高于块构建阈值)
        for ti in 0..TOOLS_PER_BLOCK {
            for tj in (ti + 1)..TOOLS_PER_BLOCK {
                let a = format!("tool-{}", bi * TOOLS_PER_BLOCK + ti);
                let b = format!("tool-{}", bi * TOOLS_PER_BLOCK + tj);
                co.insert(a, b, 150);
            }
        }
    }
    co
}

/// 构造查询向量(64 维,匹配块 0 的基向量)
fn make_query_vector() -> Vec<f32> {
    let mut v = vec![0.0_f32; DIM];
    v[0] = 1.0;
    v
}

/// 构造 512 维 CLV(前 64 维匹配块 0,KVBSR 内部截取前 64 维)
fn make_clv() -> CLV {
    let mut v = vec![0.0_f32; 512];
    v[0] = 1.0;
    CLV::from_vec(v).expect("CLV 构造应成功")
}

/// 三层路由组合基准:SESA 激活 → KVBSR 路由 → FaaE 工具选择
///
/// 测量 1000 工具规模下三层串联的端到端延迟。
///
/// # 设置(循环外)
/// - SESA:注册 256 专家(掩码容量上限)
/// - KVBSR:构建 50 块 × 20 工具 = 1000 工具
/// - FaaE:注册 1000 专家
///
/// # 测量(循环内)
/// 1. SESA activate(256 专家,Top-8,5ms 超时)
/// 2. KVBSR route(&CLV)→ Top-8 候选工具
/// 3. ToolId 转换(kvbsr → faae)
/// 4. FaaE route(&query, &candidates)→ 最终工具
fn bench_three_layer_routing(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建应成功");
    let bus = EventBus::new();

    // === 1. SESA 设置:注册 256 专家(掩码容量上限) ===
    let sesa_router = SesaRouter::new(SesaConfig::default());
    for i in 0..SESA_EXPERT_COUNT {
        let v = vec![(i as f32) * 0.01; DIM];
        let expert = ExpertDescriptor::new(format!("expert-{i}"), v);
        let _ = sesa_router.register_expert(expert);
    }
    assert_eq!(
        sesa_router.expert_count(),
        SESA_EXPERT_COUNT,
        "SESA 注册应满 256"
    );

    // === 2. KVBSR 设置:构建 50 块 × 20 工具 = 1000 工具 ===
    let kvbsr_router = KVBlockSemanticRouter::new(bus.clone());
    let tools: Vec<ToolVector> = make_tool_vectors()
        .into_iter()
        .map(|(id, v)| ToolVector::new(id, v, 100))
        .collect();
    let co = make_cooccurrence();
    rt.block_on(async {
        kvbsr_router
            .build_blocks(tools, co)
            .await
            .expect("KVBSR 块构建应成功");
    });

    // === 3. FaaE 设置:注册 1000 专家 ===
    let faae_router = FaaeRouter::new(bus.clone());
    rt.block_on(async {
        for (id, v) in make_tool_vectors() {
            let profile = ExpertProfile::new(id, v, vec!["bench".into()], 1.0);
            faae_router.register_expert(profile).await;
        }
    });

    // === 基准测量:三层串联 ===
    let query = make_query_vector();
    let clv = make_clv();

    c.bench_function("three_layer_1000_tools", |b| {
        b.iter(|| {
            // Layer 1: SESA 激活(256 专家,Top-8,5ms 超时)
            let sesa_req = ActivationRequest::new("bench-3layer", query.clone(), 8, 5);
            let (mask, _profile) = rt
                .block_on(sesa_router.activate(sesa_req))
                .expect("SESA 激活应成功");
            black_box(mask);

            // Layer 2: KVBSR 两级路由(1000 工具 → Top-8 候选)
            let kvbsr_result = rt
                .block_on(kvbsr_router.route(&clv))
                .expect("KVBSR 路由应成功");
            black_box(&kvbsr_result.selected_tools);

            // Layer 3: FaaE 精筛(KVBSR 候选 → 最终工具)
            // 转换 kvbsr_router::ToolId → faae_router::ToolId(均为 String newtype)
            let candidates: Vec<FaaeToolId> = kvbsr_result
                .selected_tools
                .iter()
                .map(|t| FaaeToolId::new(t.as_str()))
                .collect();
            let faae_result = rt
                .block_on(faae_router.route(&query, &candidates))
                .expect("FaaE 路由应成功");
            black_box(faae_result);
        });
    });
}

criterion_group!(benches, bench_three_layer_routing);
criterion_main!(benches);
