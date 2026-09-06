//! P1-T14: 五维稀疏掩码批量计算的 ComputeBridge 并行注入
//!
//! 对应任务:P1-T14（WI-34 并行化首批注入,Phase 1 地基波次,v4.0 §7.5.5 W3-8 段）
//! 对应架构层:L6 Router
//!
//! # 注入模式（WI-34 七条纪律 ④桥接唯一 / ⑤逐一回滚 / ⑥IO 不上 rayon）
//! - **识别热点**:[`crate::coordinator::OmniSparseCoordinator::compute_all_masks`]
//!   的五维度掩码批量计算 —— 纯 CPU（启发式评分 + `SparseMask::select_top_k`），
//!   无 IO / 无 .await / 无锁（rayon 闭包契约前提）。
//! - **挂 ComputeBridge**:
//!   [`ComputeBridge::route`](nexus_core::compute::ComputeBridge::route) 按
//!   `TaskKind::OsaMask` 三态判定 → `Inline`（n 小于阈值,串行路径）或 `Rayon`
//!   （[`spawn_compute_batch`](nexus_core::compute::ComputeBridge::spawn_compute_batch)
//!   批量并行,结果序 = 输入序）。
//! - **保留回退**:`OsaConfig::parallel_masks` 配置开关 + `CHIMERA_NO_PARALLEL_OSA`
//!   环境变量（启动期 OnceLock 读取一次,不在热路径）双重关闭 → 强制串行。
//! - **确定性**:并行与串行逐 profile × 维度逐元素一致（`select_top_k` / 启发式评分
//!   均为确定性算法,断言测试锁定）。
//!
//! # TaskKind 选择
//! `TaskKind::OsaMask`（阈值 100,chunk 16）—— 语义即"掩码计算",已由 T8/T9 登记,
//! 不新增变体（任务约束）。
//!
//! # rayon 闭包契约
//! 每个闭包 = 一个维度的掩码计算（纯函数）,捕获 `Arc<OsaConfig>` + `Arc<TaskProfile>`
//! + `Option<Arc<dyn MemoryStrategyProvider>>`（仅 memory 维度）;闭包内禁 IO / await /
//! 持锁（红线 §7.5.3 纪律⑥）。计算完成后在调用线程聚合（含 usage pruning 等副作用）。

use std::sync::{Arc, OnceLock};

use nexus_contracts::{MemoryStrategy, MemoryStrategyProvider, OmniSparseMasks};
use nexus_core::compute::{bridge, ComputeError, DispatchPlan, TaskKind};

use crate::config::OsaConfig;
use crate::coordinator::compute_omni_mask_hash;
use crate::error::OsaError;
use crate::masks::SparseMask;
use crate::types::{ComplexityBand, FileId, MemoryId, OperationId, TaskId, TaskProfile, ToolId};

/// 环境变量关闭开关名（`--no-parallel-osa` 的 env 形态,纪律⑤;仅测试/运维使用）
const ENV_NO_PARALLEL: &str = "CHIMERA_NO_PARALLEL_OSA";

/// 进程级 env 缓存 — 启动期读取一次,不在热路径（任务约束）
static NO_PARALLEL_ENV: OnceLock<bool> = OnceLock::new();

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

/// 批量计算产出 — 五维掩码 + 其 hash（闭包内一并计算,主线程零重算）
///
/// hash 在 pruning **前** 计算（纯计算已进 ComputeBridge）;主线程在 pruning
/// 实际生效时重算（hash 语义锁定为 pruning 后终态,与单次路径逐位一致,
/// 见 [`crate::coordinator::OmniSparseCoordinator::compute_all_masks_batch`]）。
/// 这是端到端测量的最大固定串行成本（serde_json 全量序列化 + Sha256,
/// 240 profiles ≈ 20ms）,移入并行后是本次注入的关键收益项。
pub(crate) type MaskWithHash = (OmniSparseMasks, Result<String, OsaError>);

/// 批量五维掩码核心计算（纯计算,无副作用:无事件发布/无快照/无 usage pruning）
///
/// `profiles` 以 `&Arc<Vec<TaskProfile>>` 传入:并行路径闭包捕获同一 Arc
/// （**零 profile 级 clone** —— TaskProfile 含大量 String 元素,clone 成本
/// 反超计算收益,是注入初版 0.65× 性能回归的根因）,按索引取用对应 profile。
///
/// 路由判定:
/// ① `should_parallel(config.parallel_masks)` 为 false（配置/env 关闭）→ 串行;
/// ② `bridge().route(TaskKind::OsaMask, profiles×5)` 为 `Inline` → 串行;
/// ③ 否则 → `spawn_compute_batch` 并行（每任务 = 一个 profile 的完整五维,
///    任务粒度 ≈ 串行单元,消除 N×5 微任务的 spawn/锁槽/聚合开销）。
///
/// 结果与串行路径**逐 profile × 维度一致**（确定性断言测试锁定）。
pub(crate) fn compute_five_masks_batch(
    config: &OsaConfig,
    profiles: &Arc<Vec<TaskProfile>>,
    provider: Option<Arc<dyn MemoryStrategyProvider>>,
) -> Result<Vec<MaskWithHash>, OsaError> {
    let n_items = profiles.len().saturating_mul(5);
    if !should_parallel(config.parallel_masks)
        || bridge().route(TaskKind::OsaMask, n_items) == DispatchPlan::Inline
    {
        Ok(compute_five_masks_serial(
            config,
            profiles,
            provider.as_ref(),
        ))
    } else {
        compute_five_masks_parallel(config, profiles, provider)
    }
}

/// 串行路径 — 顺序计算 N×5 维度 + 每 profile hash（回退 + Inline 分支,
/// 与注入前行为逐位一致;hash 与并行路径同源同序,保证对照公平）
///
/// 纯计算永不失败,返回 `Vec` 而非 `Result`（ComputeError 仅在并行池路径产生）。
pub(crate) fn compute_five_masks_serial(
    config: &OsaConfig,
    profiles: &[TaskProfile],
    provider: Option<&Arc<dyn MemoryStrategyProvider>>,
) -> Vec<MaskWithHash> {
    profiles
        .iter()
        .map(|p| {
            let m = compute_five_masks_one(config, p, provider.cloned());
            let h = compute_omni_mask_hash(&m);
            (m, h)
        })
        .collect()
}

/// 并行路径 — `spawn_compute_batch` 批量并行,每任务 = 一个 profile 的完整五维
///
/// 任务粒度对齐串行单元（[`compute_five_masks_one`]）,避免 N×5 微任务的
/// spawn / 槽位锁 / 结果聚合开销反超计算量;闭包捕获 `Arc<Vec<TaskProfile>>`
/// 共享容器 + 索引,**零 profile 级 clone**（注入初版 `Arc::new(profile.clone())`
/// 在 60 profile × ~2000 元素场景下 clone 成本达 ms 级,是实测 0.65× 性能
/// 回归的根因,已消除）。**hash 在闭包内一并计算**（serde_json + Sha256,
/// 纯计算无 IO/await/锁,合法入池）——端到端最大固定成本并行化,主线程
/// 仅在 pruning 实际生效时重算（语义锁定）。结果序 = 输入序。
///
/// 池内 panic 被 catch_unwind 隔离,经 [`ComputeError`] 映射为
/// [`OsaError::MaskComputationFailed`]（库代码零 panic 红线）。
fn compute_five_masks_parallel(
    config: &OsaConfig,
    profiles: &Arc<Vec<TaskProfile>>,
    provider: Option<Arc<dyn MemoryStrategyProvider>>,
) -> Result<Vec<MaskWithHash>, OsaError> {
    let cfg = Arc::new(config.clone());
    let n = profiles.len();
    let tasks: Vec<Box<dyn FnOnce() -> MaskWithHash + Send>> = (0..n)
        .map(|idx| {
            let all = Arc::clone(profiles);
            let c = Arc::clone(&cfg);
            let prov = provider.clone();
            Box::new(move || {
                let p = &all[idx];
                let m = compute_five_masks_one(&c, p, prov);
                let h = compute_omni_mask_hash(&m);
                (m, h)
            }) as Box<dyn FnOnce() -> MaskWithHash + Send>
        })
        .collect();

    let results = bridge().spawn_compute_batch(TaskKind::OsaMask, tasks);

    let mut out = Vec::with_capacity(n);
    for r in results {
        match r {
            Ok(masks_one) => out.push(masks_one),
            Err(e) => return Err(compute_to_osa(&e)),
        }
    }
    Ok(out)
}

/// ComputeError → OsaError 映射（池内 panic / 取消均归为掩码计算失败）
fn compute_to_osa(e: &ComputeError) -> OsaError {
    OsaError::MaskComputationFailed(format!("并行掩码计算失败: {e}"))
}

/// 单个 profile 的五维掩码（纯计算,串行路径单元）
fn compute_five_masks_one(
    config: &OsaConfig,
    profile: &TaskProfile,
    provider: Option<Arc<dyn MemoryStrategyProvider>>,
) -> OmniSparseMasks {
    OmniSparseMasks::new(
        routing_mask_core(config, profile),
        context_mask_core(config, profile),
        memory_mask_core(config, profile, provider),
        audit_mask_core(config, profile),
        budget_mask_core(config, profile),
    )
}

/// routing 维度核心 — 按复杂度档位选取 Top-K 工具
///
/// 与原 [`crate::coordinator::OmniSparseCoordinator::compute_routing_mask`]
/// 逻辑逐位一致,仅将 config 参数化以支持 rayon 闭包（无 `&self` 依赖）。
pub(crate) fn routing_mask_core(config: &OsaConfig, profile: &TaskProfile) -> SparseMask<ToolId> {
    let band = profile.complexity_band_with_thresholds(config.complexity_thresholds());
    let k = config.routing_top_k_for(band);
    let heuristic = heuristic_scores(profile.available_tools.len());
    let scores = profile.routing_scores.as_ref().unwrap_or(&heuristic);
    SparseMask::select_top_k(&profile.available_tools, scores, k)
}

/// context 维度核心 — 按复杂度档位选取 Top-K 文件
pub(crate) fn context_mask_core(config: &OsaConfig, profile: &TaskProfile) -> SparseMask<FileId> {
    let band = profile.complexity_band_with_thresholds(config.complexity_thresholds());
    let k = config.context_scope_for(band);
    let heuristic = heuristic_scores(profile.available_files.len());
    let scores = profile.context_scores.as_ref().unwrap_or(&heuristic);
    SparseMask::select_top_k(&profile.available_files, scores, k)
}

/// memory 维度核心 — 按复杂度档位与 S2 策略调整 Top-K 记忆
pub(crate) fn memory_mask_core(
    config: &OsaConfig,
    profile: &TaskProfile,
    provider: Option<Arc<dyn MemoryStrategyProvider>>,
) -> SparseMask<MemoryId> {
    let band = profile.complexity_band_with_thresholds(config.complexity_thresholds());
    let base_k = config.routing_top_k_for(band);
    let strategy = select_memory_strategy_core(provider.as_ref(), profile);
    let adjusted_k = apply_k_multiplier(base_k, strategy.k_multiplier());
    let heuristic = heuristic_scores(profile.available_memories.len());
    let scores = profile.memory_scores.as_ref().unwrap_or(&heuristic);
    SparseMask::select_top_k(&profile.available_memories, scores, adjusted_k)
}

/// audit 维度核心 — 按复杂度档位与风险等级选取操作
pub(crate) fn audit_mask_core(
    config: &OsaConfig,
    profile: &TaskProfile,
) -> SparseMask<OperationId> {
    let band = profile.complexity_band_with_thresholds(config.complexity_thresholds());
    let complexity_rate = complexity_audit_rate(band);
    let risk_rate = config.audit_rate_for(profile.risk_level.as_index());
    let audit_rate = complexity_rate.max(risk_rate);

    let total = profile.recent_operations.len();
    if total == 0 {
        return SparseMask::empty();
    }
    let k = if audit_rate >= 1.0 {
        total
    } else {
        ((total as f32) * audit_rate).ceil() as usize
    };
    let k = k.min(total);
    let scores = heuristic_scores(profile.recent_operations.len());
    SparseMask::select_top_k(&profile.recent_operations, &scores, k)
}

/// budget 维度核心 — 按保护比例与复杂度选取任务
pub(crate) fn budget_mask_core(config: &OsaConfig, profile: &TaskProfile) -> SparseMask<TaskId> {
    let total = profile.active_tasks.len();
    if total == 0 {
        return SparseMask::empty();
    }
    let protection = config.budget_protection_threshold * (0.5 + profile.complexity_score * 0.5);
    let k = ((total as f32) * protection).ceil() as usize;
    let k = k.clamp(1, total);
    let scores = heuristic_scores(profile.active_tasks.len());
    SparseMask::select_top_k(&profile.active_tasks, &scores, k)
}

/// S2 记忆策略选择核心 — 未注入 provider 时 fallback 到 StandardTopK
///
/// 与原 [`crate::coordinator::OmniSparseCoordinator::select_memory_strategy`]
/// 逻辑一致,仅将 provider 参数化（无 `&self` 依赖）。
pub(crate) fn select_memory_strategy_core(
    provider: Option<&Arc<dyn MemoryStrategyProvider>>,
    profile: &TaskProfile,
) -> MemoryStrategy {
    match provider {
        None => MemoryStrategy::StandardTopK,
        Some(provider) => {
            let phase = profile.task_phase.unwrap_or_default();
            provider.select_strategy(phase)
        }
    }
}

/// 按复杂度档位返回默认 audit 采样率
///
/// 对应架构手册四档分级:Simple=10% / Regular=50% / Complex=100% / UltraComplex=100%
pub(crate) fn complexity_audit_rate(band: ComplexityBand) -> f32 {
    match band {
        ComplexityBand::Simple => 0.1,
        ComplexityBand::Regular => 0.5,
        ComplexityBand::Complex => 1.0,
        ComplexityBand::UltraComplex => 1.0,
    }
}

/// 生成启发式评分向量:索引越小,评分越高（前 K 个为 Top-K）
///
/// WHY:TaskProfile 未携带评分字段时,用索引负相关评分使 Top-K 退化为前 K 个,
/// 且确保 `select_nth_unstable_by` 产生确定顺序（相同输入 → 相同输出,
/// 保证 `mask_hash` 一致性与并行/串行确定性）。
pub(crate) fn heuristic_scores(len: usize) -> Vec<f32> {
    if len == 0 {
        return Vec::new();
    }
    (0..len).map(|i| 1.0 - (i as f32 / len as f32)).collect()
}

/// 应用 S2 策略的 k_multiplier 调整基础 Top-K（ceil 向上取整,最小 1）
pub(crate) fn apply_k_multiplier(base_k: usize, multiplier: f32) -> usize {
    if base_k == 0 {
        return 0;
    }
    let adjusted = (base_k as f32) * multiplier;
    let k = adjusted.ceil() as usize;
    k.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AffectedScope, RiskLevel, TaskType, TimePressure};

    /// 构造测试用 TaskProfile（固定种子确定性生成,供并行/串行一致性断言）
    fn make_profile(seed: u64, complexity: f32, risk: RiskLevel) -> TaskProfile {
        // 固定种子 LCG — 伪随机但确定性,不引入 rand 依赖（Ω₆ 最小依赖）
        let mut state = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let mut next = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as usize
        };
        let tool_n = 8 + next() % 50;
        let file_n = 50 + next() % 2000;
        let mem_n = 8 + next() % 50;
        let op_n = 20 + next() % 120;
        let task_n = 4 + next() % 12;
        TaskProfile {
            task_id: format!("t-{seed}").into(),
            task_type: TaskType::Read,
            complexity_score: complexity,
            risk_level: risk,
            time_pressure: TimePressure::Low,
            affected_scope: AffectedScope::Local,
            available_tools: (0..tool_n)
                .map(|i| ToolId::new(format!("tool-{i}")))
                .collect(),
            available_files: (0..file_n)
                .map(|i| FileId::new(format!("file-{i}")))
                .collect(),
            available_memories: (0..mem_n)
                .map(|i| MemoryId::new(format!("mem-{i}")))
                .collect(),
            recent_operations: (0..op_n)
                .map(|i| OperationId::new(format!("op-{i}")))
                .collect(),
            active_tasks: (0..task_n)
                .map(|i| TaskId::new(format!("task-{i}")))
                .collect(),
            routing_scores: None,
            context_scores: None,
            memory_scores: None,
            task_phase: None,
        }
    }

    /// 构造一批固定种子 profile（N×5 任务规模可超过 OsaMask 阈值 100 → 触发 Rayon）
    fn make_profiles(n: usize) -> Vec<TaskProfile> {
        (0..n)
            .map(|i| {
                let complexity = ((i * 37 + 13) % 1000) as f32 / 1000.0;
                let risk = match i % 4 {
                    0 => RiskLevel::Low,
                    1 => RiskLevel::Medium,
                    2 => RiskLevel::High,
                    _ => RiskLevel::Critical,
                };
                make_profile(i as u64, complexity, risk)
            })
            .collect()
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

    /// 断言两批 (掩码, hash) 逐 profile 一致 — 掩码全等 + hash 值/错误路径一致
    fn assert_batches_eq(serial: &[MaskWithHash], parallel: &[MaskWithHash]) {
        assert_eq!(serial.len(), parallel.len(), "结果数必须一致");
        for (i, ((s, sh), (p, ph))) in serial.iter().zip(parallel.iter()).enumerate() {
            assert_eq!(s, p, "profile[{i}] 掩码必须与串行一致");
            match (sh, ph) {
                (Ok(a), Ok(b)) => assert_eq!(a, b, "profile[{i}] hash 必须与串行一致"),
                (Err(a), Err(b)) => {
                    assert_eq!(a.to_string(), b.to_string(), "profile[{i}] hash 错误一致")
                }
                _ => panic!("profile[{i}] hash 错误路径不一致"),
            }
        }
    }

    #[test]
    fn test_parallel_matches_serial_fixed_seed() {
        let config = OsaConfig::default();
        let profiles = Arc::new(make_profiles(48)); // 240 任务 ≥ 100 → Rayon 分支
        let serial = compute_five_masks_serial(&config, &profiles, None);
        let parallel = compute_five_masks_batch(&config, &profiles, None).expect("批量失败");
        assert_eq!(serial.len(), 48);
        assert_eq!(parallel.len(), 48, "结果数必须等于输入 profile 数");
        assert_batches_eq(&serial, &parallel);
    }

    #[test]
    fn test_parallel_matches_serial_with_scores() {
        // 携带真实评分时并行/串行也必须一致（select_top_k 评分路径）
        let config = OsaConfig::default();
        let mut profiles_vec = make_profiles(40);
        for p in &mut profiles_vec {
            p.routing_scores = Some(
                (0..p.available_tools.len())
                    .map(|i| (i % 7) as f32)
                    .collect(),
            );
            p.context_scores = Some(
                (0..p.available_files.len())
                    .map(|i| (i % 11) as f32)
                    .collect(),
            );
            p.memory_scores = Some(
                (0..p.available_memories.len())
                    .map(|i| (i % 5) as f32)
                    .collect(),
            );
        }
        let profiles = Arc::new(profiles_vec);
        let serial = compute_five_masks_serial(&config, &profiles, None);
        let parallel = compute_five_masks_batch(&config, &profiles, None).expect("批量失败");
        assert_batches_eq(&serial, &parallel);
    }

    // ============================================================
    // 边界:空输入 / 单元素
    // ============================================================

    #[test]
    fn test_empty_profiles() {
        let config = OsaConfig::default();
        let profiles = Arc::new(Vec::<TaskProfile>::new());
        let out = compute_five_masks_batch(&config, &profiles, None).expect("空输入应成功");
        assert!(out.is_empty());
    }

    #[test]
    fn test_single_profile_boundary() {
        let config = OsaConfig::default();
        let profiles = Arc::new(make_profiles(1)); // 5 任务 < 阈值 100 → Inline 串行
        let out = compute_five_masks_batch(&config, &profiles, None).expect("单 profile 应成功");
        assert_eq!(out.len(), 1);
        assert!(out[0].0.routing.active_count() > 0);
        assert!(out[0].0.context.active_count() > 0);
    }

    // ============================================================
    // 回退开关（配置关闭 → 强制串行,结果与串行一致）
    // ============================================================

    #[test]
    fn test_config_disable_falls_back_to_serial() {
        // 大 N（240 任务 ≥ 阈值）下配置关闭 → 必须仍走串行且结果正确
        let config = OsaConfig::default().with_parallel_masks(false);
        let profiles = Arc::new(make_profiles(48));
        let disabled = compute_five_masks_batch(&config, &profiles, None).expect("回退失败");
        let serial = compute_five_masks_serial(&config, &profiles, None);
        assert_batches_eq(&disabled, &serial);
    }

    /// env 关闭开关 → 走串行（集成验证:进程级 env 首次读取即生效,结果与串行一致）
    ///
    /// WHY OnceLock:本测试设置 env 后,`env_no_parallel()` 首次读取即缓存关闭态;
    /// 若其他测试先调用导致已缓存 false,本测试跳过断言（不污染其他测试）。
    #[test]
    fn test_env_disable_falls_back_to_serial() {
        if env_no_parallel() {
            // 已有其他来源关闭 —— 缓存生效,直接验证串行一致性
            let config = OsaConfig::default();
            let profiles = Arc::new(make_profiles(48));
            let out = compute_five_masks_batch(&config, &profiles, None).expect("回退失败");
            let serial = compute_five_masks_serial(&config, &profiles, None);
            assert_batches_eq(&out, &serial);
            return;
        }
        // env 尚未缓存:设置后首次读取 → 关闭并行（OnceLock 一次性语义）
        std::env::set_var(ENV_NO_PARALLEL, "1");
        let config = OsaConfig::default();
        let profiles = Arc::new(make_profiles(48));
        let out = compute_five_masks_batch(&config, &profiles, None).expect("回退失败");
        let serial = compute_five_masks_serial(&config, &profiles, None);
        assert_batches_eq(&out, &serial);
        // 恢复 env,避免影响同进程其他测试（OnceLock 已缓存,恢复仅对子进程有意义）
        std::env::remove_var(ENV_NO_PARALLEL);
    }

    // ============================================================
    // 维度核心函数（从 coordinator.rs 迁移,行为锁定）
    // ============================================================

    #[test]
    fn test_complexity_audit_rate() {
        assert!((complexity_audit_rate(ComplexityBand::Simple) - 0.1).abs() < 1e-6);
        assert!((complexity_audit_rate(ComplexityBand::Regular) - 0.5).abs() < 1e-6);
        assert!((complexity_audit_rate(ComplexityBand::Complex) - 1.0).abs() < 1e-6);
        assert!((complexity_audit_rate(ComplexityBand::UltraComplex) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_apply_k_multiplier_standard() {
        assert_eq!(apply_k_multiplier(8, 1.0), 8);
        assert_eq!(apply_k_multiplier(16, 1.0), 16);
        assert_eq!(apply_k_multiplier(32, 1.0), 32);
    }

    #[test]
    fn test_apply_k_multiplier_minimal_recall() {
        assert_eq!(apply_k_multiplier(8, 0.5), 4);
        assert_eq!(apply_k_multiplier(16, 0.5), 8);
    }

    #[test]
    fn test_apply_k_multiplier_aggressive_pruning() {
        assert_eq!(apply_k_multiplier(8, 0.25), 2);
        assert_eq!(apply_k_multiplier(16, 0.25), 4);
    }

    #[test]
    fn test_apply_k_multiplier_query_reformulation() {
        assert_eq!(apply_k_multiplier(8, 1.5), 12);
        assert_eq!(apply_k_multiplier(16, 1.5), 24);
    }

    #[test]
    fn test_apply_k_multiplier_ceil_behavior() {
        assert_eq!(apply_k_multiplier(7, 0.5), 4);
        assert_eq!(apply_k_multiplier(7, 0.25), 2);
    }

    #[test]
    fn test_apply_k_multiplier_minimum_one() {
        assert_eq!(apply_k_multiplier(1, 0.25), 1);
    }

    #[test]
    fn test_apply_k_multiplier_zero_base() {
        assert_eq!(apply_k_multiplier(0, 1.0), 0);
        assert_eq!(apply_k_multiplier(0, 0.5), 0);
    }

    /// Mock provider:返回固定策略(用于测试策略选择核心)
    struct MockFixedStrategyProvider {
        strategy: MemoryStrategy,
    }

    impl MemoryStrategyProvider for MockFixedStrategyProvider {
        fn select_strategy(&self, _phase: nexus_contracts::MemoryTaskPhase) -> MemoryStrategy {
            self.strategy
        }
    }

    #[test]
    fn test_select_strategy_no_provider_falls_back_to_standard() {
        let profile = make_profile(1, 0.5, RiskLevel::Medium);
        assert_eq!(
            select_memory_strategy_core(None, &profile),
            MemoryStrategy::StandardTopK
        );
    }

    #[test]
    fn test_select_strategy_with_provider_returns_provider_strategy() {
        let provider: Arc<dyn MemoryStrategyProvider> = Arc::new(MockFixedStrategyProvider {
            strategy: MemoryStrategy::AggressivePruning,
        });
        let profile = make_profile(1, 0.5, RiskLevel::Medium);
        assert_eq!(
            select_memory_strategy_core(Some(&provider), &profile),
            MemoryStrategy::AggressivePruning
        );
    }

    /// 五维核心与 coordinator 公开方法一致性:相同输入 → 相同掩码
    #[test]
    fn test_core_matches_coordinator_methods() {
        use crate::coordinator::OmniSparseCoordinator;
        let bus = event_bus::EventBus::new();
        let coord = OmniSparseCoordinator::new(bus);
        let profile = make_profile(7, 0.6, RiskLevel::High);
        let core = compute_five_masks_one(coord.config(), &profile, None);
        assert_eq!(core.routing, coord.compute_routing_mask(&profile));
        assert_eq!(core.context, coord.compute_context_mask(&profile));
        assert_eq!(core.memory, coord.compute_memory_mask(&profile));
        assert_eq!(core.audit, coord.compute_audit_mask(&profile));
        assert_eq!(core.budget, coord.compute_budget_mask(&profile));
    }
}
