//! KVBSR 路由延迟基准测试
//!
//! 对应 SubTask 11.1:引入 criterion 基准测试框架
//!
//! # 基准测试矩阵
//!
//! | 场景              | 规模             | CLV 输入             | 测量指标            |
//! |-------------------|------------------|----------------------|---------------------|
//! | `build_blocks`    | 100/300/1000     | N/A(工具+共现矩阵)   | 块构建延迟           |
//! | `route`           | 100/300/1000     | 随机向量(有区分度)    | 路由延迟 P50/P95/P99 |
//! | `route_zero_clv`  | 300              | 零向量               | 退化输入路由延迟      |
//! | `route_ones_clv`  | 300              | 全 1 向量            | 均匀输入路由延迟      |
//!
//! # 两级路由参数(默认配置)
//! - 第一级:块级 Top-3(`config.top_blocks = 3`)
//! - 第二级:块内 Top-8(`config.top_tools = 8`)
//!
//! WHY 使用 block_on:`route`/`build_blocks` 为 async fn,
//! criterion 默认同步,通过 `Runtime::new().block_on()` 在同步上下文中调用。

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use event_bus::EventBus;
use kvbsr_router::{CoOccurrenceMatrix, KVBlockSemanticRouter, ToolVector};
use nexus_core::CLV;

// ────────────────────────────────────────────────────────────
// 测试数据构造
// ────────────────────────────────────────────────────────────

/// 构造指定规模的工具向量与共现矩阵
///
/// 每个块的基向量在不同维度上有高值,确保块间区分度。
/// 块内工具共现 150 次 > 默认阈值 100,确保块内工具被聚类到同一块。
///
/// # 参数
/// - `num_blocks`:块数量
/// - `tools_per_block`:每块工具数
/// - `dim`:向量维度(默认 64)
fn make_test_data(
    num_blocks: usize,
    tools_per_block: usize,
    dim: usize,
) -> (Vec<ToolVector>, CoOccurrenceMatrix) {
    let mut tools = Vec::with_capacity(num_blocks * tools_per_block);
    let mut co = CoOccurrenceMatrix::new();

    for bi in 0..num_blocks {
        // 每个块在 2 个独特维度上有高值,确保块间区分度
        let mut base = vec![0.0_f32; dim];
        base[(bi * 4) % dim] = 1.0;
        base[(bi * 4 + 1) % dim] = 1.0;
        for ti in 0..tools_per_block {
            let mut vector = base.clone();
            // 添加小扰动,保持块内相似度
            for v in vector.iter_mut() {
                *v += (ti as f32 * 0.01) - 0.1;
            }
            tools.push(ToolVector::new(format!("tool-{bi}-{ti}"), vector, 100));
        }
        // 块内工具共现 > 阈值(150 > 100),确保聚类到同一块
        for ti in 0..tools_per_block {
            for tj in (ti + 1)..tools_per_block {
                co.insert(format!("tool-{bi}-{ti}"), format!("tool-{bi}-{tj}"), 150);
            }
        }
    }

    (tools, co)
}

/// 构造 100 工具测试数据(5 块 × 20 工具/块)
fn make_100_tools() -> (Vec<ToolVector>, CoOccurrenceMatrix) {
    make_test_data(5, 20, 64)
}

/// 构造 300 工具测试数据(15 块 × 20 工具/块)
fn make_300_tools() -> (Vec<ToolVector>, CoOccurrenceMatrix) {
    make_test_data(15, 20, 64)
}

/// 构造 1000 工具测试数据(50 块 × 20 工具/块)
///
/// WHY 单维度基向量:50 块需要 50 个独特维度,64 维足够(50 < 64),无重叠
fn make_1000_tools() -> (Vec<ToolVector>, CoOccurrenceMatrix) {
    let num_blocks = 50;
    let tools_per_block = 20;
    let dim = 64;
    let mut tools = Vec::with_capacity(num_blocks * tools_per_block);
    let mut co = CoOccurrenceMatrix::new();

    for bi in 0..num_blocks {
        let mut base = vec![0.0_f32; dim];
        base[bi] = 1.0; // 单维度基向量,50 块 < 64 维,无重叠
        for ti in 0..tools_per_block {
            let mut vector = base.clone();
            for v in vector.iter_mut() {
                *v += (ti as f32 * 0.01) - 0.1;
            }
            tools.push(ToolVector::new(format!("tool-{bi}-{ti}"), vector, 100));
        }
        for ti in 0..tools_per_block {
            for tj in (ti + 1)..tools_per_block {
                co.insert(format!("tool-{bi}-{ti}"), format!("tool-{bi}-{tj}"), 150);
            }
        }
    }

    (tools, co)
}

// ────────────────────────────────────────────────────────────
// CLV 输入变体
// ────────────────────────────────────────────────────────────

/// 构造随机 CLV(伪随机,确定性种子)
///
/// 使用固定种子的简单 LCG 生成,确保基准测试可重复。
/// 模拟真实场景中 NMC 编码器输出的有区分度的 CLV 输入。
fn make_random_clv() -> CLV {
    let mut v = vec![0.0_f32; 512];
    // 简单 LCG 伪随机,xorshift 风格,固定种子确保可重复
    let mut state: u32 = 42;
    for val in v.iter_mut() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *val = (state as f32 / u32::MAX as f32) * 2.0 - 1.0; // [-1.0, 1.0]
    }
    CLV::from_vec(v).expect("CLV 构造应成功")
}

/// 构造零向量 CLV — 退化输入,测试路由在无信号输入下的行为
fn make_zero_clv() -> CLV {
    CLV::zero()
}

/// 构造全 1 向量 CLV — 均匀输入,所有维度等权
fn make_ones_clv() -> CLV {
    CLV::from_vec(vec![1.0_f32; 512]).expect("CLV 构造应成功")
}

/// 构造有区分度的 CLV:前 64 维匹配块 0 的基向量
fn make_targeted_clv() -> CLV {
    let mut clv_vec = vec![0.0_f32; 512];
    clv_vec[0] = 1.0;
    clv_vec[1] = 1.0;
    CLV::from_vec(clv_vec).expect("CLV 构造应成功")
}

// ────────────────────────────────────────────────────────────
// 辅助:初始化路由器并构建块
// ────────────────────────────────────────────────────────────

/// 初始化路由器并构建块,返回就绪的路由器实例
fn setup_router(
    rt: &tokio::runtime::Runtime,
    tools: Vec<ToolVector>,
    co: CoOccurrenceMatrix,
) -> KVBlockSemanticRouter {
    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);
    rt.block_on(async {
        router.build_blocks(tools, co).await.expect("块构建应成功");
    });
    router
}

// ────────────────────────────────────────────────────────────
// 基准测试:build_blocks 块构建延迟
// ────────────────────────────────────────────────────────────

/// build_blocks 基准 — 100 工具(5 块 × 20 工具/块)
fn bench_build_blocks_100(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建应成功");
    let bus = EventBus::new();

    c.bench_function("build_blocks_100_tools", |b| {
        b.iter(|| {
            let router = KVBlockSemanticRouter::new(bus.clone());
            let (tools, co) = make_100_tools();
            rt.block_on(router.build_blocks(black_box(tools), black_box(co)))
                .expect("块构建应成功");
        });
    });
}

/// build_blocks 基准 — 300 工具(15 块 × 20 工具/块)
fn bench_build_blocks_300(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建应成功");
    let bus = EventBus::new();

    c.bench_function("build_blocks_300_tools", |b| {
        b.iter(|| {
            let router = KVBlockSemanticRouter::new(bus.clone());
            let (tools, co) = make_300_tools();
            rt.block_on(router.build_blocks(black_box(tools), black_box(co)))
                .expect("块构建应成功");
        });
    });
}

/// build_blocks 基准 — 1000 工具(50 块 × 20 工具/块)
fn bench_build_blocks_1000(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建应成功");
    let bus = EventBus::new();

    c.bench_function("build_blocks_1000_tools", |b| {
        b.iter(|| {
            let router = KVBlockSemanticRouter::new(bus.clone());
            let (tools, co) = make_1000_tools();
            rt.block_on(router.build_blocks(black_box(tools), black_box(co)))
                .expect("块构建应成功");
        });
    });
}

// ────────────────────────────────────────────────────────────
// 基准测试:route 路由延迟(不同规模 × 不同 CLV 输入)
// ────────────────────────────────────────────────────────────

/// route 基准 — 100 工具,随机 CLV 输入
///
/// 两级路由:块级 Top-3 + 块内 Top-8
fn bench_route_100(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建应成功");
    let (tools, co) = make_100_tools();
    let router = setup_router(&rt, tools, co);
    let clv = make_random_clv();

    c.bench_function("route_100_tools_random_clv", |b| {
        b.iter(|| {
            rt.block_on(router.route(black_box(&clv)))
                .expect("路由应成功");
        });
    });
}

/// route 基准 — 300 工具,随机 CLV 输入(主基准)
fn bench_route_300(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建应成功");
    let (tools, co) = make_300_tools();
    let router = setup_router(&rt, tools, co);
    let clv = make_random_clv();

    c.bench_function("route_300_tools_random_clv", |b| {
        b.iter(|| {
            rt.block_on(router.route(black_box(&clv)))
                .expect("路由应成功");
        });
    });
}

/// route 基准 — 1000 工具,随机 CLV 输入
fn bench_route_1000(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建应成功");
    let (tools, co) = make_1000_tools();
    let router = setup_router(&rt, tools, co);
    let clv = make_random_clv();

    c.bench_function("route_1000_tools_random_clv", |b| {
        b.iter(|| {
            rt.block_on(router.route(black_box(&clv)))
                .expect("路由应成功");
        });
    });
}

// ────────────────────────────────────────────────────────────
// 基准测试:route 不同 CLV 输入变体(300 工具规模)
// ────────────────────────────────────────────────────────────

/// route 基准 — 300 工具,零向量 CLV(退化输入)
///
/// 零向量时所有余弦相似度为 0,路由结果取决于部分排序的稳定性,
/// 测试路由在无信号输入下的延迟表现。
fn bench_route_zero_clv(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建应成功");
    let (tools, co) = make_300_tools();
    let router = setup_router(&rt, tools, co);
    let clv = make_zero_clv();

    c.bench_function("route_300_tools_zero_clv", |b| {
        b.iter(|| {
            rt.block_on(router.route(black_box(&clv)))
                .expect("路由应成功");
        });
    });
}

/// route 基准 — 300 工具,全 1 向量 CLV(均匀输入)
///
/// 全 1 向量时所有方向的投影相同,测试路由在均匀输入下的延迟。
fn bench_route_ones_clv(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建应成功");
    let (tools, co) = make_300_tools();
    let router = setup_router(&rt, tools, co);
    let clv = make_ones_clv();

    c.bench_function("route_300_tools_ones_clv", |b| {
        b.iter(|| {
            rt.block_on(router.route(black_box(&clv)))
                .expect("路由应成功");
        });
    });
}

/// route 基准 — 300 工具,目标定向 CLV(匹配块 0 基向量)
///
/// 前 64 维匹配块 0 的基向量,测试路由在最佳匹配输入下的延迟。
fn bench_route_targeted_clv(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建应成功");
    let (tools, co) = make_300_tools();
    let router = setup_router(&rt, tools, co);
    let clv = make_targeted_clv();

    c.bench_function("route_300_tools_targeted_clv", |b| {
        b.iter(|| {
            rt.block_on(router.route(black_box(&clv)))
                .expect("路由应成功");
        });
    });
}

// ────────────────────────────────────────────────────────────
// Criterion 注册
// ────────────────────────────────────────────────────────────

criterion_group!(
    benches,
    // build_blocks 块构建延迟(3 个规模梯度)
    bench_build_blocks_100,
    bench_build_blocks_300,
    bench_build_blocks_1000,
    // route 路由延迟(3 个规模梯度,随机 CLV)
    bench_route_100,
    bench_route_300,
    bench_route_1000,
    // route 不同 CLV 输入变体(300 工具规模)
    bench_route_zero_clv,
    bench_route_ones_clv,
    bench_route_targeted_clv,
);
criterion_main!(benches);
