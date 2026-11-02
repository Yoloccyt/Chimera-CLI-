//! `CHIMERA_NO_PARALLEL_FAAE` env 关闭开关的**进程隔离**验证
//!
//! # 为何必须是独立的集成测试目标
//! `parallel::env_no_parallel()` 把 env 读进**进程级** `OnceLock`
//! (`NO_PARALLEL_ENV`)。若该用例与并行分支用例同处一个测试二进制,谁先跑决定
//! 其余测试的分支:env 用例赢 → `OnceLock` 缓存为"关闭" → 同二进制里所有
//! "并行 == 串行"的等价性断言都只在 `Inline` 分支上自我印证,**全部照样通过**。
//! cargo 为每个 `tests/*.rs` 生成独立可执行文件,进程边界就是这里需要的隔离边界。
//!
//! # 本文件锁住两件事
//! ① **env 真的被读到并把决策钉成 `Inline`**:本用例选的规模(12_000)高于
//!    `TaskKind::Generic` 阈值 10_000,无 env 时同一决策必为 `Rayon` ⇒ 断到
//!    `Inline` 只有 env 这一个可能来源,该断言有判别力。
//! ② **关闭态下评分结果逐元素正确、顺序不变**:期望值由**可精确表示**的基向量
//!    与优先级推导(cos 恰为 1.0 / 0.0,priority 恰为 0.5),不复制生产算术
//!    (`score_parts` 仍是唯一实现)。
//!
//! # 为何没有"env 未设置"的反向对照
//! 同进程内任何一次 `env_no_parallel()` 调用都会把 `OnceLock` 钉死,反向对照
//! 会让本用例自相矛盾。该对照组由 **lib 单元测试二进制**提供:那里的
//! `parallel::tests::assert_rayon_branch` 在无 env 时断言同一规模必为 `Rayon`。

#![allow(clippy::unwrap_used, clippy::expect_used)] // 集成测试惯用写法;E-5 只约束生产代码

use std::sync::Arc;

use faae_router::parallel::{dispatch_plan, score_experts_batch, ScoreInput};
use faae_router::ToolId;
use nexus_core::compute::DispatchPlan;

/// 运维侧 env 开关名 —— **故意写死字面量**(不去引用生产常量):
/// 该名字本身是对运维的公开契约,改名应当被本测试当场抓住,而不是跟着改测试。
const ENV_NO_PARALLEL: &str = "CHIMERA_NO_PARALLEL_FAAE";

/// 大于 `TaskKind::Generic` 阈值 10_000 的规模 —— 无 env 时必为 `Rayon`。
const BULK: usize = 12_000;

/// 64 维单位基向量
fn basis(dim: usize) -> Vec<f32> {
    let mut v = vec![0.0_f32; 64];
    v[dim] = 1.0;
    v
}

/// 构造评分输入 —— 偶数项与查询同向(cos 恰为 1.0),奇数项正交(cos 恰为 0.0)
///
/// WHY 选基向量:期望分数因此恒为 `priority` 或 `0.0`,两个数都是 f32 精确值,
/// 可以用 `assert_eq!` 全等断言而不引入 epsilon —— 与本 crate 其它等价性测试
/// 的断言口径一致,且不依赖任何"测试内复刻的评分算术"。
fn make_inputs(n: usize) -> Vec<ScoreInput> {
    (0..n)
        .map(|i| ScoreInput {
            tool_id: ToolId::new(format!("tool-{i}")),
            expert_vector: basis(i % 2),
            priority: if i % 2 == 0 { 0.5 } else { 0.25 },
        })
        .collect()
}

/// env 关闭 → 强制串行回退,且结果逐元素正确
#[test]
fn env_disable_falls_back_to_serial() {
    // 必须是本进程对 `NO_PARALLEL_ENV` 的**第一次**触碰:env 是惰性读取的,任何
    // 更早的读取(包括"无 env 时应为 Rayon"的反向对照)都会把开启态钉死,让下面
    // 的断言恒真。
    std::env::set_var(ENV_NO_PARALLEL, "1");

    // ① env 生效:该规模无 env 时必为 Rayon,到 Inline 只可能是 env 门控所致
    assert_eq!(
        dispatch_plan(BULK, true),
        DispatchPlan::Inline,
        "CHIMERA_NO_PARALLEL_FAAE=1 必须把 {BULK} 条的决策钉成 Inline 串行回退 \
         (无 env 时同一决策为 Rayon)"
    );

    // ② 关闭态下结果正确且保持输入序
    let clv = basis(0);
    let inputs = Arc::new(make_inputs(BULK));
    let out = score_experts_batch(&clv, &inputs, true).expect("env 关闭态下批量评分应成功");
    assert_eq!(out.len(), BULK, "结果数必须等于输入数");
    for (i, (tool_id, score)) in out.iter().enumerate() {
        assert_eq!(
            tool_id,
            &ToolId::new(format!("tool-{i}")),
            "tool[{i}] 必须保持输入序"
        );
        let expected = if i % 2 == 0 { 0.5 } else { 0.0 };
        assert_eq!(*score, expected, "tool[{i}] 评分必须等于基向量推导的精确值");
    }
}
