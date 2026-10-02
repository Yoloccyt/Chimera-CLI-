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
//!   主线程异步读锁收集评分输入快照,释放锁后在纯数据上做批量评分
//!   （并行或串行）。快照本身的成本随后又按下述"预计算"收口为零深拷贝。
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
//! # 两条评分输入载体
//! - [`ScoreInput`]:自有三元组快照,由调用方构造（bench / 外部可构造契约）。
//!   P3 起生产侧已无调用点,登记在 `scripts/check_dead_contract_apis.py` 台账
//!   （DEFERRED,`uses=0`）,保留供同口径对照。
//! - [`ExpertScoring`]:由 `ExpertProfile::new` 预计算一次、以 `Arc` 共享的不可变
//!   三元组,`route()` 热路径用之,使快照收集退化为引用计数自增:每个候选**零分配、
//!   零深拷贝**（外层 `Vec::with_capacity(n)` 与 `Arc::new` 是与候选数无关的两次
//!   分配,故此处不说"零堆分配"）。两者共用 `score_parts` 算术与 `dispatch_plan`
//!   判定,结果逐元素一致（`test_precomputed_path_matches_score_input_path` 锁定）。
//!
//! # rayon 闭包契约
//! 闭包捕获 `Arc<Vec<f32>>`（query）+ 输入容器（chunk 快照）,
//! 仅做纯计算;禁 IO / await / 持锁（红线 §7.5.3 纪律⑥）。

use std::sync::{Arc, OnceLock};

use nexus_core::compute::{bridge, ComputeError, DispatchPlan, TaskKind};

use crate::error::FaaeError;
use crate::types::{ExpertScoring, ToolId};

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

/// 评分核心算术 — 两条评分路径（`ScoreInput` / `ExpertScoring`）的唯一实现
///
/// `query = clv[..min(clv.len(), expert_vector.len())]` 截取对齐,
/// `final = cosine_similarity_slices(query, expert_vector) × priority`。
///
/// WHY 不收 `tool_id`:评分只用向量与权重,`tool_id` 是随结果返回的标签而非算子
/// 的输入。两条入口各自把标签从自己的载体上取走,算术在此汇合。
#[must_use]
pub(crate) fn score_parts(clv: &[f32], expert_vector: &[f32], priority: f32) -> f32 {
    let query = &clv[..clv.len().min(expert_vector.len())];
    nexus_core::cosine_similarity_slices(query, expert_vector) * priority
}

/// 单个专家评分（纯函数,与 [`crate::router::FaaeRouter::route`] 原步骤 2 逐位一致）
#[must_use]
pub(crate) fn score_one(clv: &[f32], input: &ScoreInput) -> f32 {
    score_parts(clv, &input.expert_vector, input.priority)
}

/// ComputeBridge 路由判定 — 两条评分路径共用的唯一决策点
///
/// 配置/env 关闭 → 直接 `Inline`（等价于 `should_parallel` 为 false）,
/// 否则按 `TaskKind::Generic` 的规模阈值判 `Inline` / `Rayon`。
#[must_use]
fn dispatch_plan(n: usize, parallel_enabled: bool) -> DispatchPlan {
    if should_parallel(parallel_enabled) {
        bridge().route(TaskKind::Generic, n)
    } else {
        DispatchPlan::Inline
    }
}

/// 当前是否运行在多线程 tokio runtime 上（让出 worker 的前提,理由见
/// [`score_experts_batch_offloaded`]）
#[must_use]
fn on_multi_thread_runtime() -> bool {
    matches!(
        tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor()),
        Ok(tokio::runtime::RuntimeFlavor::MultiThread)
    )
}

/// 拼接各 chunk 结果,保持输入序
///
/// WHY 独立函数:两条评分路径的 chunk 任务只有输出元素类型不同,错误映射与
/// 拼接顺序完全相同,各写一份必然漂移。`T` 为单个评分结果元素
/// (`spawn_compute_batch` 的 `T` 是整个 chunk 的 `Vec<_>`)。
fn collect_chunks<T: Send + 'static>(
    results: Vec<Result<Vec<T>, ComputeError>>,
    total: usize,
) -> Result<Vec<T>, FaaeError> {
    let mut out = Vec::with_capacity(total);
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
    if dispatch_plan(n, parallel_enabled) == DispatchPlan::Inline {
        Ok(score_serial(clv, inputs))
    } else {
        score_parallel(clv, inputs)
    }
}

/// 路由热路径的批量评分入口 —— 多线程 tokio runtime 上让出 worker 后再算
///
/// # 调用点与姊妹入口
/// 本函数是 [`score_experts_scoring_batch_offloaded`] 的"自有快照"对应物:
/// 两者共用同一 flavor 判据,只差评分输入载体(此处为自有数据的 [`ScoreInput`],
/// 姊妹入口为共享的 [`ExpertScoring`])。`route()` 步骤 2 走的是后者
/// （预计算零拷贝）。本入口保留为 [`ScoreInput`] 载体的公开包装,并被
/// T2 并发测试直接测（该测试按构造只测让出窗口,故绕开 `route()` 的快照阶段）。
///
/// 抽成独立函数而非内联在 `route()` 里,是为了让「让出 worker」这一行为可被
/// T2 单独测量:`route()` 全程还含快照收集(自身会让出一次),噪声与被测的
/// ~2ms 批窗口同量级,包在 `route()` 外测无法区分
/// 「worker 被饿死」与「快照阶段让出过一次」。
///
/// # WHY 让出 worker
/// [`score_experts_batch`] 是同步的(内层为 ComputeBridge 的 rayon
/// `pool.scope`,不是 Future),在 12000 专家规模下独占 tokio worker 数毫秒,
/// 期间该 worker 无法 poll 其它 task / 驱动 IO。多线程 runtime 上用
/// `block_in_place` 让出 worker;**计算本体仍在 ComputeBridge 专用 rayon 池内
/// 执行**,不新建 CPU 预算(§7.5.3 纪律④「桥接唯一」—— 池线程数是全局资源预算,
/// 见 `nexus_core::compute::bridge` 的单例说明与 `pool.scope` 注释)—— 故不用
/// `spawn_blocking`。
///
/// # 澄清:blocking 池参与「交接」,不参与「计算」
/// `block_in_place` 并非把 worker 闲置 —— 它把当前 worker 线程的 core 整体**交接**
/// 出去:core 的剩余生命周期经 `runtime::spawn_blocking(move || run(worker))`
/// 在 blocking 池上跑完(见 tokio `scheduler/multi_thread/worker.rs`),被占死的
/// worker 由此空出来去 poll 别的 task。参与交接 ≠ 参与计算:评分全部仍在唯一 rayon
/// 池内,blocking 池这段只执行 runtime 自身的 core 收尾。故「无第二份 CPU 预算」
/// 成立,依据是**计算不落在 blocking 池**,而非 blocking 池未被触碰。
///
/// # WHY 两分支,而非裸 `block_in_place`
/// `block_in_place` 在 current_thread runtime 上 **panic**。本 crate 既有
/// `#[tokio::test]`(默认 current_thread)与下游库消费方都可能是 current_thread;
/// faae-router 是库 crate,抬高其运行时要求等同破坏性变更。current_thread 上
/// 本就没有并发 task 可让,直调与让出语义等价,零行为漂移。生产入口
/// `chimera-cli/src/main.rs` 的 `#[tokio::main]` 无 flavor 覆盖(多线程),
/// 正是让出兑现收益的场所。
///
/// # 已知边界:flavor 判据 ≠ `block_in_place` 的真实门
/// tokio 实际把关的是内部 `allow_block_in_place` 标志,而非 runtime flavor:
/// 在 `LocalSet` 内调用时**即便 flavor 为 MultiThread 也会 panic**(LocalSet 强制
/// current_thread 语义,无可让出的 worker)。本分支只以 flavor 判别,故 LocalSet
/// 内的调用方仍会 panic —— flavor 门控的固有局限,在此登记而非静默上线。
///
/// # WHY `try_current` 而非 `current`
/// 非 tokio 执行器 poll 上层 future 时 `Handle::current()` 会 panic —— 那将
/// 回归掉今天已可用的非 tokio 消费路径,属零收益的行为收紧。`try_current` 在
/// 无 tokio 上下文时返回 `Err`,自然落到直调分支。
///
/// # 为何是 `pub` 而非 `pub(crate)`
/// 这是**先于 P3 存在的公开契约入口**,签名自落地起未变;`route()` 改走预计算路径
/// 后 crate 内不再有调用点,但"公开 API 仍在、只是当前无生产调用方"与"被死代码
/// lint 掩盖"是两件必须分开记账的事。故保持 `pub` 并把它登记进机器台账
/// (`scripts/check_dead_contract_apis.py` 的 `faae::parallel::score_experts_batch*`
/// 两行,状态 DEFERRED)—— 台账按 ADR-197 A6 的口径报"未接线",而不是让 rustc 的
/// `dead_code` 沉默。
///
/// WHY NOT 靠降级可见性解决:把可见性从 `pub` 收到 `pub(crate)` 来"消掉"告警,
/// 等于用"编译器看不见"换掉"人看得见"——公开项豁免 `dead_code` 正是 ADR-197 A6
/// 要抓的那类缺陷(编译器与测试套件双双看不见),扩大同一豁免面只会让它更难被发现。
/// 真正的账要记在台账里,不是记在可见性里。
pub fn score_experts_batch_offloaded(
    clv: &[f32],
    inputs: &Arc<Vec<ScoreInput>>,
    parallel_enabled: bool,
) -> Result<Vec<(ToolId, f32)>, FaaeError> {
    if on_multi_thread_runtime() {
        tokio::task::block_in_place(|| score_experts_batch(clv, inputs, parallel_enabled))
    } else {
        score_experts_batch(clv, inputs, parallel_enabled)
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
    collect_chunks(results, inputs.len())
}

// ============================================================================
// 预计算路径 —— 输入为 `Arc<ExpertScoring>` 共享的不可变三元组
//
// 与 `ScoreInput` 路径共用 `score_parts` 算术与 `dispatch_plan` 判定;两条路径
// 结果逐元素一致（等价性测试锁定）。差异只在"标签怎么带出去"与"要不要拷贝":
// `ExpertScoring` 由 `ExpertProfile::new` 预计算一次并以 `Arc` 共享,故
//   ① 热路径快照收集只剩引用计数自增,每个候选零分配、零深拷贝
//      （与候选数无关的两次分配:外层 `Vec::with_capacity(n)` 与 `Arc::new`）；
//   ② 结果携带 `Arc<ExpertScoring>` 而非自有 `ToolId`,消费者只为自己留存的
//      Top-K(k 通常为 8)付 `ToolId` clone,而不是为全部 n 个候选付。
// ============================================================================

/// 预计算评分入口 —— 输入为 `Arc<ExpertScoring>` 共享三元组
///
/// 路由判定与 [`score_experts_batch`] 完全一致（同一个 `dispatch_plan`）,
/// 结果序 = 输入序,元素与 `ScoreInput` 路径逐位相同。
///
/// # 返回值携带 `Arc<ExpertScoring>` 而非 `ToolId`
/// WHY:调用方只保留 Top-K,让 n 次 `ToolId`(String)堆分配降到 k 次;同时
/// `ExpertScoring` 是不可变共享的,返回 `Arc` 使调用方无法误改评分输入。
/// 需要 `ToolId` 时由调用方显式 `scoring.tool_id.clone()`。
///
/// # 保留 `ScoreInput` 入口的理由
/// `ScoreInput` 仍是 bench(`faae_parallel_bench.rs`)与外部可构造快照的公开
/// 契约,两条路径并存以便同口径对照,不构成重复实现（算术与判定均已共用）。
pub fn score_experts_scoring_batch(
    clv: &[f32],
    inputs: &Arc<Vec<Arc<ExpertScoring>>>,
    parallel_enabled: bool,
) -> Result<Vec<(Arc<ExpertScoring>, f32)>, FaaeError> {
    let n = inputs.len();
    if dispatch_plan(n, parallel_enabled) == DispatchPlan::Inline {
        Ok(score_scoring_serial(clv, inputs))
    } else {
        score_scoring_parallel(clv, inputs)
    }
}

/// 预计算路径的让出包装 —— 与 [`score_experts_batch_offloaded`] 同一 flavor 判据
///
/// 理由(`block_in_place` 为何能让出而不新建 CPU 预算、为何需要两分支、
/// LocalSet 下的已知边界)全部见 [`score_experts_batch_offloaded`] 文档,此处不重复。
pub(crate) fn score_experts_scoring_batch_offloaded(
    clv: &[f32],
    inputs: &Arc<Vec<Arc<ExpertScoring>>>,
    parallel_enabled: bool,
) -> Result<Vec<(Arc<ExpertScoring>, f32)>, FaaeError> {
    if on_multi_thread_runtime() {
        tokio::task::block_in_place(|| score_experts_scoring_batch(clv, inputs, parallel_enabled))
    } else {
        score_experts_scoring_batch(clv, inputs, parallel_enabled)
    }
}

/// 预计算路径 — 串行评分
fn score_scoring_serial(
    clv: &[f32],
    inputs: &[Arc<ExpertScoring>],
) -> Vec<(Arc<ExpertScoring>, f32)> {
    inputs
        .iter()
        .map(|s| {
            (
                Arc::clone(s),
                score_parts(clv, &s.expert_vector, s.priority),
            )
        })
        .collect()
}

/// 预计算路径 — `spawn_compute_batch` 批量评分,结果序 = 输入序
///
/// 闭包捕获 `Arc<Vec<Arc<ExpertScoring>>>` + 索引范围:只增引用计数,不拷贝向量。
fn score_scoring_parallel(
    clv: &[f32],
    inputs: &Arc<Vec<Arc<ExpertScoring>>>,
) -> Result<Vec<(Arc<ExpertScoring>, f32)>, FaaeError> {
    let query_arc: Arc<Vec<f32>> = Arc::new(clv.to_vec());
    let n_chunks = inputs.len().div_ceil(CHUNK);
    type ChunkTask = Box<dyn FnOnce() -> Vec<(Arc<ExpertScoring>, f32)> + Send>;
    let tasks: Vec<ChunkTask> = (0..n_chunks)
        .map(|ci| {
            let q = Arc::clone(&query_arc);
            let all = Arc::clone(inputs);
            let start = ci * CHUNK;
            let end = (start + CHUNK).min(all.len());
            Box::new(move || {
                all[start..end]
                    .iter()
                    .map(|s| (Arc::clone(s), score_parts(&q, &s.expert_vector, s.priority)))
                    .collect()
            }) as Box<dyn FnOnce() -> Vec<(Arc<ExpertScoring>, f32)> + Send>
        })
        .collect();

    let results = bridge().spawn_compute_batch(TaskKind::Generic, tasks);
    collect_chunks(results, inputs.len())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;
    use crate::config::FaaeConfig;
    use nexus_contracts::util::xts_top_k_by;

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

    /// 构造测试用预计算评分输入
    ///
    /// WHY 从 `make_inputs` 搬运而非另写一套 LCG:必须保证两条路径拿到的
    /// `tool_id` / `expert_vector` / `priority` **逐字段相同**,否则等价性
    /// 断言会把"数据不同"误判成"实现有 bug"。
    fn make_scoring_inputs(n: usize) -> Vec<Arc<ExpertScoring>> {
        make_inputs(n)
            .into_iter()
            .map(|i| {
                Arc::new(ExpertScoring {
                    tool_id: i.tool_id,
                    expert_vector: i.expert_vector,
                    priority: i.priority,
                })
            })
            .collect()
    }

    /// 按 `route()` 步骤 3 的**同一个**选择器取 Top-K
    ///
    /// WHY 直接调 `xts_top_k_by` 而非测试内复刻 `sort_by` + `truncate`:
    /// `route()` 选 Top-K 走的是 L0 公共助手 `xts_top_k_by`（`select_nth_unstable_by`
    /// 分区 + 前 k 段稳定排序）。测试自带一份全排版比较器,即便逐字相同,也只锁住了
    /// 一条**生产上并不走的**代码路径 —— 哪天有人换掉生产选择器,这个测试依旧全绿,
    /// 与 ADR-197 A6 要防的"锁错对象"是同一类假绿。比较器（含同分平局的原始下标
    /// 升序消歧）与 `router.rs` 步骤 3 逐字一致。
    fn top_k(rows: &[(ToolId, f32)], k: usize) -> Vec<(ToolId, f32)> {
        // 与 route() 同构：先按输入序带上原始下标,再交给公共选择器
        let mut idx: Vec<(usize, ToolId, f32)> = rows
            .iter()
            .enumerate()
            .map(|(i, (t, s))| (i, t.clone(), *s))
            .collect();
        let top = xts_top_k_by(&mut idx, k, |a, b| {
            b.2.partial_cmp(&a.2)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        })
        .to_vec();
        top.into_iter().map(|(_, t, s)| (t, s)).collect()
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

    // ============================================================
    // 预计算路径（ExpertScoring）vs 旧路径（ScoreInput）等价性
    // ============================================================

    /// 预计算路径必须与 `ScoreInput` 路径**逐元素等价**,含 Top-K 截断结果
    ///
    /// 规模覆盖四种形态:0（空）/ 1（单元素）/ 65（1 整 chunk + 残块）/
    /// 90（< Generic 阈值 10_000,Inline 串行）/ 12_000（≥ 阈值,Rayon 并行）,
    /// 每种规模再各跑"并行开启"与"强制串行"两种配置。
    ///
    /// 三层断言:
    /// ① 逐元素 `(ToolId, f32)` 全等 —— f32 用 `assert_eq!` 而非近似:
    ///    两条路径共用 `score_parts` 纯函数,同输入必须同输出(位模式相同)。
    /// ② Top-K 截断后候选序列全等(含同分平局的原始下标次序),且截断走的是
    ///    `route()` 生产同款 `xts_top_k_by` + 同一比较器(见测试内 `top_k`)。
    ///    注:本条在逐元素相等成立时是**被蕴含的**,独立价值在于它锁住了生产选择器
    ///    本身的行为(分区 + 前 k 段稳定排序)与平局次序,而不是重复验一遍算术。
    /// ③ 结果携带的是输入的**同一份** `ExpertScoring`(`Arc::ptr_eq`),
    ///    锁住"零拷贝"这一契约,而不只是"分数碰巧相同"。
    #[test]
    fn test_precomputed_path_matches_score_input_path() {
        for n in [0_usize, 1, 65, 90, 12_000] {
            for parallel_enabled in [true, false] {
                let clv = make_clv();
                let legacy = Arc::new(make_inputs(n));
                let precomputed = Arc::new(make_scoring_inputs(n));

                let legacy_out = score_experts_batch(&clv, &legacy, parallel_enabled)
                    .expect("ScoreInput 路径评分失败");
                let pre_out = score_experts_scoring_batch(&clv, &precomputed, parallel_enabled)
                    .expect("预计算路径评分失败");

                assert_eq!(legacy_out.len(), n, "n={n} 旧路径结果数必须等于输入数");
                assert_eq!(pre_out.len(), n, "n={n} 预计算路径结果数必须等于输入数");

                for (i, ((legacy_id, legacy_score), (shared, pre_score))) in
                    legacy_out.iter().zip(pre_out.iter()).enumerate()
                {
                    assert_eq!(
                        *legacy_id, shared.tool_id,
                        "n={n} parallel={parallel_enabled} tool[{i}] ToolId 必须一致"
                    );
                    assert_eq!(
                        *legacy_score, *pre_score,
                        "n={n} parallel={parallel_enabled} tool[{i}] 评分必须逐位一致"
                    );
                    assert!(
                        Arc::ptr_eq(shared, &precomputed[i]),
                        "n={n} tool[{i}] 必须共享输入的同一份 ExpertScoring（零拷贝）"
                    );
                }

                // Top-K:两路径各自截断后的候选序列（含平局次序）必须完全相同
                let pre_rows: Vec<(ToolId, f32)> = pre_out
                    .iter()
                    .map(|(s, score)| (s.tool_id.clone(), *score))
                    .collect();
                let k = 8.min(n);
                assert_eq!(
                    top_k(&legacy_out, k),
                    top_k(&pre_rows, k),
                    "n={n} parallel={parallel_enabled} Top-K 结果必须一致"
                );

                // 让出包装在无 tokio 上下文（本 `#[test]` 即是）下必须与直调等价:
                // `try_current` 失败 → 直调分支,两个 wrapper 都锁住。
                // WHY 锁这个:`route()` 改走预计算路径后,`ScoreInput` 版 wrapper
                // 已无 crate 内调用点,若不复核,它的 flavor 分支将完全裸奔。
                let legacy_offloaded =
                    score_experts_batch_offloaded(&clv, &legacy, parallel_enabled)
                        .expect("ScoreInput 让出包装评分失败");
                let pre_offloaded =
                    score_experts_scoring_batch_offloaded(&clv, &precomputed, parallel_enabled)
                        .expect("预计算让出包装评分失败");
                assert_eq!(
                    legacy_offloaded, legacy_out,
                    "n={n} parallel={parallel_enabled} 让出包装必须与直调等价"
                );
                let pre_offloaded_rows: Vec<(ToolId, f32)> = pre_offloaded
                    .iter()
                    .map(|(s, score)| (s.tool_id.clone(), *score))
                    .collect();
                assert_eq!(
                    pre_offloaded_rows, pre_rows,
                    "n={n} parallel={parallel_enabled} 让出包装必须与直调等价"
                );
            }
        }
    }
}
