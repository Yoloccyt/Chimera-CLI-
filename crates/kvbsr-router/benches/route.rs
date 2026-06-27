//! KVBSR 路由基准测试
//!
//! 对应 SubTask 11.1:引入 criterion 基准测试框架
//!
//! 基准场景:构造 300 工具(15 块 × 20 工具/块),测量 `KVBlockSemanticRouter::route` 延迟。
//!
//! WHY 使用 block_on:`route` 为 async fn, criterion 默认同步,
//! 通过 `Runtime::new().block_on()` 在同步上下文中调用 async 方法。

use criterion::{criterion_group, criterion_main, Criterion};
use event_bus::EventBus;
use kvbsr_router::{CoOccurrenceMatrix, KVBlockSemanticRouter, ToolVector};
use nexus_core::CLV;

/// 构造 300 工具的测试数据(15 块 × 20 工具/块)
fn make_test_data() -> (Vec<ToolVector>, CoOccurrenceMatrix) {
    let num_blocks = 15;
    let tools_per_block = 20;
    let dim = 64;
    let mut tools = Vec::new();
    let mut co = CoOccurrenceMatrix::new();

    // 每个块的基向量:在不同维度上有高值,确保块间区分度
    for bi in 0..num_blocks {
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
        // 块内工具共现 > 阈值
        for ti in 0..tools_per_block {
            for tj in (ti + 1)..tools_per_block {
                co.insert(format!("tool-{bi}-{ti}"), format!("tool-{bi}-{tj}"), 150);
            }
        }
    }

    (tools, co)
}

/// 基准:KVBSR 两级路由(300 工具)
fn bench_route(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建应成功");

    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);
    // 在基准循环外初始化块列表(避免每次迭代重建)
    rt.block_on(async {
        let (tools, co) = make_test_data();
        router.build_blocks(tools, co).await.expect("块构建应成功");
    });

    // 构造查询 CLV:前 64 维匹配块 0 的基向量
    let mut clv_vec = vec![0.0_f32; 512];
    clv_vec[0] = 1.0;
    clv_vec[1] = 1.0;
    let clv = CLV::from_vec(clv_vec).expect("CLV 构造应成功");

    c.bench_function("route_300_tools", |b| {
        b.iter(|| {
            rt.block_on(router.route(&clv)).expect("路由应成功");
        });
    });
}

/// 构造 1000 工具的测试数据(50 块 × 20 工具/块)
///
/// WHY 单维度基向量:50 块需要 50 个独特维度,64 维足够(50 < 64),无重叠
fn make_scale_test_data() -> (Vec<ToolVector>, CoOccurrenceMatrix) {
    let num_blocks = 50;
    let tools_per_block = 20;
    let dim = 64;
    let mut tools = Vec::new();
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

/// 基准:KVBSR 两级路由(1000 工具)— SubTask 15.10 规模基准
fn bench_route_1000_tools(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建应成功");

    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);
    rt.block_on(async {
        let (tools, co) = make_scale_test_data();
        router.build_blocks(tools, co).await.expect("块构建应成功");
    });

    // 构造查询 CLV:前 64 维匹配块 0 的基向量(单维度)
    let mut clv_vec = vec![0.0_f32; 512];
    clv_vec[0] = 1.0;
    let clv = CLV::from_vec(clv_vec).expect("CLV 构造应成功");

    c.bench_function("route_1000_tools", |b| {
        b.iter(|| {
            rt.block_on(router.route(&clv)).expect("路由应成功");
        });
    });
}

criterion_group!(benches, bench_route, bench_route_1000_tools);
criterion_main!(benches);
