//! P1-T14: 批量专家评分的 ComputeBridge 并行注入
//!
//! 对应任务:P1-T14（WI-34 并行化首批注入,Phase 1 地基波次,v4.0 §7.5.5 W3-8 段）
//! 对应架构层:L6 Router
//!
//! # 注入模式（WI-34 七条纪律 ④桥接唯一 / ⑤逐一回滚 / ⑥IO 不上 rayon）
//! - **识别热点**:[`crate::router::FaaeRouter::route`] 步骤 2 的批量专家评分
//!   —— 对每个候选专家计算 `cosine_similarity_slices(query, expert_vector) × priority`。
//!   纯 CPU（64 维向量余弦 + 乘法）,无 IO;但原实现循环内持有 tokio 读锁 await,
//!   不满足 rayon 闭包契约（闭包内禁 await/持锁）→ 改造为**快照分离**:
//!   主线程异步读锁收集 `ScoreInput{tool_id, expert_vector, priority}` 快照,
//!   释放锁后在纯数据上做批量评分（并行或串行）。
//! - **挂 ComputeBridge**:
//!   [`ComputeBridge::route`](nexus_core::compute::ComputeBridge::route) 按
//!   `TaskKind::Generic` 三态判定 → `Inline`（n 小于阈值,串行路径）或 `Rayon`
//!   （[`spawn_compute_batch`](nexus_core::compute::ComputeBridge::spawn_compute_batch)
//!   批量并行,结果序 = 输入序）。
//! - **保留回退**:`FaaeConfig::parallel_expert_scoring` 配置开关 +
//!   `CHIMERA_NO_PARALLEL_FAAE` 环境变量（启动期 OnceLock 读取一次,不在热路径）
//!   双重关闭 → 强制串行。
//! - **确定性**:并行与串行逐元素一致（cosine_similarity_slices 为纯函数,
//!   断言测试锁定）。
//!
//! # TaskKind 选择
//! `TaskKind::Generic`（阈值 10000,chunk 64）—— 任务约束"不新增 TaskKind 变体";
//! 专家评分语义与现有 ClvSimilarity 相近,但 ClvSimilarity 语义为"CLV 相似度
//! 配对（CLV × 快照）",评分对象是"专家 × 查询"批量打分,用 Generic 更贴切
//! （Generic 为通用纯计算批处理,阈值 10000 与批量专家场景吻合）。
//!
//! # 并行粒度（CHUNK 分组）
//! 单次评分仅 64 维余弦,若逐条 spawn 微任务,12000 条的调度开销将反超计算量。
//! 故按 `CHUNK = 64`（与 Generic 表 chunk 对齐）分组,每个 rayon 闭包处理一个
//! chunk 内的全部评分,显著降低任务调度开销。
//!
//! # rayon 闭包契约
//! 闭包捕获 `Arc<Vec<f32>>`（query）+ `Vec<ScoreInput>`（chunk 快照）,
//! 仅做纯计算;禁 IO / await / 持锁（红线 §7.5.3 纪律⑥）。

use std::sync::{Arc, OnceLock};

use nexus_core::compute::{bridge, DispatchPlan, TaskKind};

use crate::error::FaaeError;
use crate::types::ToolId;

/// 环境变量关闭开关名（`--no-parallel-faae` 的 env 形态,纪律⑤;仅测试/运维使用）
const ENV_NO_PARALLEL: &str = "CHIMERA_NO_PARALLEL_FAAE";

/// 进程级 env 缓存 — 启动期读取一次,不在热路径（任务约束）
static NO_PARALLEL_ENV: OnceLock<bool> = OnceLock::new();

/// 并行分块大小 — 与 Generic 表 chunk(64)对齐,避免微任务调度开销反超计算量
const CHUNK: usize = 64;

/// 评分输入快照 — 由主线程异步读锁收集,评分阶段无锁无 await
///
/// pub:供 bench（`faae_parallel_bench.rs` 热点层直测）与集成方构造/消费评分快照。
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreInput {
    /// 工具 ID
    pub tool_id: ToolId,
    /// 专家向量(64 维,评分纯计算用)
    pub expert_vector: Vec<f32>,
    /// 优先级权重(final = sim × priority)
    pub priority: f32,
}

/// 解析环境变量值 — 纯函数（"1"/"true"/"on" 视为关闭,大小写不敏感）
#[must_use]
pub(crate) fn parse_no_parallel_env(value: Option<&str>) -> bool {
    value.is_some_and(|v| matches!(v, "1" | "true" | "TRUE" | "on" | "ON"))
}

/// env 关闭开关 — OnceLock 惰性读取（启动期一次,非热路径）
#[must_use]
pub(crate) fn env_no_parallel() -> bool {
    *NO_PARALLEL_ENV
        .get_or_init(|| parse_no_parallel_env(std::env::var(ENV_NO_PARALLEL).ok().as_deref()))
}

/// 并行开关最终判定 — 配置开关 AND 非 env 关闭（任一关闭 → 串行回退）
#[must_use]
pub(crate) fn should_parallel(config_flag: bool) -> bool {
    config_flag && !env_no_parallel()
}

/// 单个专家评分（纯函数,与 [`crate::router::FaaeRouter::route`] 原步骤 2 逐位一致）
///
/// `query = clv[..min(clv.len(), expert_vector.len())]` 截取对齐,
/// `final = cosine_similarity_slices(query, expert_vector) × priority`。
#[must_use]
pub(crate) fn score_one(clv: &[f32], input: &ScoreInput) -> f32 {
    let query = &clv[..clv.len().min(input.expert_vector.len())];
    let sim = nexus_core::cosine_similarity_slices(query, &input.expert_vector);
    sim * input.priority
}

/// 批量专家评分入口（ComputeBridge 路由判定）
///
/// 路由判定:
/// ① `should_parallel(config_flag)` 为 false（配置/env 关闭）→ 串行;
/// ② `bridge().route(TaskKind::Generic, n)` 为 `Inline` → 串行;
/// ③ 否则 → `spawn_compute_batch` 并行（CHUNK 分组）。
///
/// `inputs` 以 `&Arc<Vec<ScoreInput>>` 传入:并行闭包捕获同一 Arc + 索引范围,
/// **零输入级 clone**（ScoreInput 含 tool_id String + expert_vector Vec<f32>,
/// 注入初版 `chunk.to_vec()` 在 12000 专家场景下全量复制 ~5ms,是实测 0.76×
/// 性能回归的根因,已消除;输出 ToolId clone 两路径公平）。
///
/// 结果与串行路径**逐元素一致**（确定性断言测试锁定）。
/// 计算失败（池内 panic 经 catch_unwind 隔离）映射为
/// [`FaaeError::RoutingFailed`]（不新增错误变体,红线:不改 FaaeError 语义）。
///
/// pub:热点核心入口,供 bench 热点层（`faae_parallel_bench.rs`）直接对照
/// 串行/并行收益——端到端 `route()` 含快照读锁收集等注入边界外前置
/// （async 读锁 + expert_vector 全量 clone,禁入 rayon）,会稀释注入收益,
/// 热点层才是与 v4.0 §7.5.1 预估（L-a 3-6×）对齐的测量口径。
pub fn score_experts_batch(
    clv: &[f32],
    inputs: &Arc<Vec<ScoreInput>>,
    parallel_enabled: bool,
) -> Result<Vec<(ToolId, f32)>, FaaeError> {
    let n = inputs.len();
    if !should_parallel(parallel_enabled)
        || bridge().route(TaskKind::Generic, n) == DispatchPlan::Inline
    {
        Ok(score_serial(clv, inputs))
    } else {
        score_parallel(clv, inputs)
    }
}

/// 串行路径 — 顺序评分（回退 + Inline 分支,与注入前行为逐位一致）
#[must_use]
fn score_serial(clv: &[f32], inputs: &[ScoreInput]) -> Vec<(ToolId, f32)> {
    inputs
        .iter()
        .map(|input| (input.tool_id.clone(), score_one(clv, input)))
        .collect()
}

/// 并行路径 — `spawn_compute_batch` 批量评分,结果序 = 输入序
///
/// CHUNK 分组:每个闭包处理一个 chunk 的评分（捕获 `Arc<Vec<ScoreInput>>` +
/// 索引范围,零输入复制）,避免微任务调度开销。
/// 池内 panic 被 catch_unwind 隔离,经 ComputeError 映射为路由失败错误。
fn score_parallel(
    clv: &[f32],
    inputs: &Arc<Vec<ScoreInput>>,
) -> Result<Vec<(ToolId, f32)>, FaaeError> {
    let query_arc: Arc<Vec<f32>> = Arc::new(clv.to_vec());
    let n_chunks = inputs.len().div_ceil(CHUNK);
    type ChunkTask = Box<dyn FnOnce() -> Vec<(ToolId, f32)> + Send>;
    let tasks: Vec<ChunkTask> = (0..n_chunks)
        .map(|ci| {
            let q = Arc::clone(&query_arc);
            let all = Arc::clone(inputs);
            let start = ci * CHUNK;
            let end = (start + CHUNK).min(all.len());
            Box::new(move || {
                all[start..end]
                    .iter()
                    .map(|input| (input.tool_id.clone(), score_one(&q, input)))
                    .collect()
            }) as Box<dyn FnOnce() -> Vec<(ToolId, f32)> + Send>
        })
        .collect();

    let results = bridge().spawn_compute_batch(TaskKind::Generic, tasks);

    let mut out = Vec::with_capacity(inputs.len());
    for r in results {
        match r {
            Ok(chunk_out) => out.extend(chunk_out),
            Err(e) => {
                return Err(FaaeError::RoutingFailed {
                    reason: format!("并行专家评分失败: {e}"),
                })
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FaaeConfig;

    /// 构造测试用评分输入（固定种子 LCG 确定性生成）
    fn make_inputs(n: usize) -> Vec<ScoreInput> {
        (0..n)
            .map(|i| {
                let seed = i as u64;
                let mut state = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let mut next = move || {
                    state = state
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    (state >> 33) as usize
                };
                ScoreInput {
                    tool_id: ToolId::new(format!("tool-{i}")),
                    expert_vector: (0..64).map(|_| (next() % 1000) as f32 / 1000.0).collect(),
                    priority: (next() % 100) as f32 / 100.0 + 0.01,
                }
            })
            .collect()
    }

    /// 固定种子 CLV（64 维确定性）
    fn make_clv() -> Vec<f32> {
        (0..64).map(|i| ((i * 13) % 1000) as f32 / 1000.0).collect()
    }

    // ============================================================
    // env 开关 / 判定逻辑
    // ============================================================

    #[test]
    fn test_parse_no_parallel_env() {
        assert!(parse_no_parallel_env(Some("1")));
        assert!(parse_no_parallel_env(Some("true")));
        assert!(parse_no_parallel_env(Some("TRUE")));
        assert!(parse_no_parallel_env(Some("on")));
        assert!(!parse_no_parallel_env(Some("0")));
        assert!(!parse_no_parallel_env(Some("false")));
        assert!(!parse_no_parallel_env(Some("yes")));
        assert!(!parse_no_parallel_env(None));
    }

    #[test]
    fn test_should_parallel_gating() {
        // 配置开启 + env 未关 → 并行
        // 注意:env_no_parallel() 依赖进程级 OnceLock（测试环境默认未设置 → false）
        if !env_no_parallel() {
            assert!(should_parallel(true));
        }
        // 配置关闭 → 强制串行（无论 env）
        assert!(!should_parallel(false));
    }

    // ============================================================
    // 并行 vs 串行一致性（固定种子随机输入）
    // ============================================================

    #[test]
    fn test_parallel_matches_serial_fixed_seed() {
        let clv = make_clv();
        let inputs = Arc::new(make_inputs(12_000)); // 12_000 ≥ Generic 阈值 10_000 → Rayon 分支
        let serial = score_serial(&clv, &inputs);
        let parallel = score_experts_batch(&clv, &inputs, true).expect("批量评分失败");
        assert_eq!(serial.len(), 12_000);
        assert_eq!(parallel.len(), 12_000, "结果数必须等于输入数");
        for (i, (s, p)) in serial.iter().zip(parallel.iter()).enumerate() {
            assert_eq!(s.0, p.0, "tool[{i}] ID 必须一致");
            // f32 全等:同一纯函数同输入同输出,并行不得引入舍入差异
            assert_eq!(s.1, p.1, "tool[{i}] 评分必须与串行逐元素一致");
        }
    }

    #[test]
    fn test_parallel_matches_serial_small_batch() {
        // 小批量（< 阈值）→ Inline 串行,结果一致
        let clv = make_clv();
        let inputs = Arc::new(make_inputs(90)); // KVBSR 粗筛典型规模
        let serial = score_serial(&clv, &inputs);
        let parallel = score_experts_batch(&clv, &inputs, true).expect("批量评分失败");
        assert_eq!(serial, parallel, "小批量(Inline 串行)结果必须一致");
    }

    // ============================================================
    // 边界:空输入 / 单元素 / 非整 chunk
    // ============================================================

    #[test]
    fn test_empty_inputs() {
        let clv = make_clv();
        let inputs = Arc::new(Vec::<ScoreInput>::new());
        let out = score_experts_batch(&clv, &inputs, true).expect("空输入应成功");
        assert!(out.is_empty());
    }

    #[test]
    fn test_single_input() {
        let clv = make_clv();
        let inputs = Arc::new(make_inputs(1));
        let out = score_experts_batch(&clv, &inputs, true).expect("单输入应成功");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, ToolId::new("tool-0"));
    }

    #[test]
    fn test_odd_chunk_boundary() {
        // 65 个输入 = 1 个整 chunk(64) + 1 个残块 → 跨块结果拼接顺序正确
        let clv = make_clv();
        let inputs = Arc::new(make_inputs(65));
        let serial = score_serial(&clv, &inputs);
        let parallel = score_experts_batch(&clv, &inputs, true).expect("批量评分失败");
        assert_eq!(serial, parallel, "非整 chunk 边界结果必须与串行一致");
    }

    // ============================================================
    // 回退开关（配置关闭 → 强制串行,结果与串行一致）
    // ============================================================

    #[test]
    fn test_config_disable_falls_back_to_serial() {
        // 大 N（≥ 阈值）下配置关闭 → 必须仍走串行且结果正确
        let clv = make_clv();
        let inputs = Arc::new(make_inputs(12_000));
        let disabled = score_experts_batch(&clv, &inputs, false).expect("回退失败");
        let serial = score_serial(&clv, &inputs);
        assert_eq!(disabled, serial, "配置关闭后结果必须与串行路径一致");
    }

    /// env 关闭开关 → 走串行（集成验证:进程级 env 首次读取即生效,结果与串行一致）
    ///
    /// WHY OnceLock:本测试设置 env 后,`env_no_parallel()` 首次读取即缓存关闭态;
    /// 若其他测试先调用导致已缓存 false,本测试跳过断言（不污染其他测试）。
    #[test]
    fn test_env_disable_falls_back_to_serial() {
        if env_no_parallel() {
            // 已有其他来源关闭 —— 缓存生效,直接验证串行一致性
            let clv = make_clv();
            let inputs = Arc::new(make_inputs(12_000));
            let out = score_experts_batch(&clv, &inputs, true).expect("回退失败");
            let serial = score_serial(&clv, &inputs);
            assert_eq!(out, serial);
            return;
        }
        // env 尚未缓存:设置后首次读取 → 关闭并行（OnceLock 一次性语义）
        std::env::set_var(ENV_NO_PARALLEL, "1");
        let clv = make_clv();
        let inputs = Arc::new(make_inputs(12_000));
        let out = score_experts_batch(&clv, &inputs, true).expect("回退失败");
        let serial = score_serial(&clv, &inputs);
        assert_eq!(out, serial, "env 关闭后结果必须与串行路径一致");
        // 恢复 env,避免影响同进程其他测试（OnceLock 已缓存,恢复仅对子进程有意义）
        std::env::remove_var(ENV_NO_PARALLEL);
    }

    // ============================================================
    // 配置开关与路由接线
    // ============================================================

    #[test]
    fn test_config_flag_wires_through() {
        let clv = make_clv();
        let inputs = Arc::new(make_inputs(12_000));
        let config = FaaeConfig::default();
        // 配置默认 true → 路由判定走 should_parallel(config.parallel_expert_scoring)
        let _ = score_experts_batch(&clv, &inputs, config.parallel_expert_scoring)
            .expect("默认配置评分失败");
        // 配置 false → 强制串行
        let disabled = FaaeConfig::default().with_parallel_expert_scoring(false);
        let out = score_experts_batch(&clv, &inputs, disabled.parallel_expert_scoring)
            .expect("关闭后评分失败");
        assert_eq!(out.len(), 12_000);
    }
}
