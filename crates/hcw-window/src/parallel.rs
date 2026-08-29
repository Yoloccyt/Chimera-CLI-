//! P1-T14: 压缩评分阶段的 ComputeBridge 段间并行注入
//!
//! 对应任务:P1-T14（WI-34 并行化首批注入,Phase 1 地基波次,v4.0 §7.5.1 L-a）
//! 对应架构层:L2 Memory
//! 对应创新点:HCW(Hierarchical Context Window,分层上下文窗口)
//!
//! # 注入模式（WI-34 七条纪律 ④桥接唯一 / ⑤逐一回滚 / ⑥IO 不上 rayon）
//! - **识别热点**:[`ContextCompressor::compress`](crate::compressor::ContextCompressor::compress)
//!   的**评分阶段** —— 逐条目 `compute_importance_score`(0.4×时近性 + 0.3×频次
//!   + 0.3×任务相关性,含 512 维 CLV 余弦),纯 CPU 密集,占压缩全流程主要开销
//!   (v4.0 §7.5.1 L-a: 四层级窗口选择 / 压缩段间 2-3×【待验证】)。
//! - **段间并行,段内保序**:`entries` 按 `CHUNK = 64` 分段,每个 rayon 闭包计算
//!   一段内的**逐条目评分**(段内保序,返回 `Vec<f32>`),结果按段序拼接
//!   (**段间保序**)→ 全局评分序列与串行路径**逐元素一致**。
//!   Top-K 选择 + 贪心保留(依赖全局排序)仍串行执行,与注入前逐位一致。
//! - **挂 ComputeBridge**:[`ComputeBridge::route`](nexus_core::compute::ComputeBridge::route)
//!   按 `TaskKind::CscCollapseScore` 三态判定 → `Inline`(n 小于阈值,串行)或 `Rayon`
//!   ([`spawn_compute_batch`](nexus_core::compute::ComputeBridge::spawn_compute_batch)
//!   批量并行,结果序 = 输入序)。
//! - **保留回退**:`HcwConfig::parallel_compress` 配置开关 + `CHIMERA_NO_PARALLEL_HCW`
//!   环境变量(启动期 OnceLock 读取一次,不在热路径)双重关闭 → 强制串行;
//!   并行评分失败(理论不可达:闭包纯计算)自动回退串行评分,`compress` 签名零变化。
//! - **确定性**:并行与串行逐元素一致 + 顺序断言(段序不变,断言测试锁定)。
//!
//! # TaskKind 选择
//! `TaskKind::CscCollapseScore`(阈值 200,chunk 64)—— 任务约束"不新增 TaskKind 变体";
//! 压缩评分是对条目批量计算重要性分数,与 CscCollapseScore("压缩折叠评分")语义
//! 最接近;窗口条目数 > 200 触发 Rayon(阈值低,窗口压缩场景条目数百~数千,吻合)。
//!
//! # rayon 闭包契约
//! 闭包捕获 `Arc<Vec<Arc<ContextEntry>>>`(主线程一次性引用计数复制,条目内容零复制)
//! + 索引范围,仅调 `compute_importance_score` 纯计算(无 IO/await/持锁,红线 §7.5.3
//! 纪律⑥)。`compress` 在写锁内调用本模块,`spawn_compute_batch` 同步阻塞至完成,
//! 闭包内无锁无 await,锁在调用方持有不跨闭包边界——契约合规。

use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Utc};
use nexus_contracts::SelectorWeights;
use nexus_core::compute::{bridge, DispatchPlan, TaskKind};
use nexus_core::CLV;

use crate::compressor::compute_importance_score;
use crate::types::ContextEntry;

/// 环境变量关闭开关名(纪律⑤;仅测试/运维使用)
const ENV_NO_PARALLEL: &str = "CHIMERA_NO_PARALLEL_HCW";

/// 进程级 env 缓存 — 启动期读取一次,不在热路径(任务约束)
static NO_PARALLEL_ENV: OnceLock<bool> = OnceLock::new();

/// 并行分块大小 — 与阈值表 chunk(64)对齐,避免微任务调度开销反超计算量
const CHUNK: usize = 64;

/// 解析环境变量值 — 纯函数("1"/"true"/"on" 视为关闭,大小写不敏感)
#[must_use]
pub(crate) fn parse_no_parallel_env(value: Option<&str>) -> bool {
    value.is_some_and(|v| matches!(v, "1" | "true" | "TRUE" | "on" | "ON"))
}

/// env 关闭开关 — OnceLock 惰性读取(启动期一次,非热路径)
#[must_use]
pub(crate) fn env_no_parallel() -> bool {
    *NO_PARALLEL_ENV
        .get_or_init(|| parse_no_parallel_env(std::env::var(ENV_NO_PARALLEL).ok().as_deref()))
}

/// 并行开关最终判定 — 配置开关 AND 非 env 关闭(任一关闭 → 串行回退)
#[must_use]
pub(crate) fn should_parallel(config_flag: bool) -> bool {
    config_flag && !env_no_parallel()
}

/// 评分阶段入口（ComputeBridge 路由判定 + 段间并行,段内保序）
///
/// 路由判定:
/// ① `should_parallel(config_flag)` 为 false(配置/env 关闭)→ 串行;
/// ② `bridge().route(TaskKind::CscCollapseScore, n)` 为 `Inline` → 串行;
/// ③ 否则 → 段间并行评分(CHUNK 分组)。
///
/// **保序保证**:每个 chunk 段内按输入序逐条目评分(段内保序),各段按段序拼接
/// (段间保序),返回的 `Vec<f32>` 与串行路径 `entries.iter().map(compute_importance_score)`
/// **逐元素一致**(确定性断言测试锁定,含顺序断言)。
///
/// **失败回退**:并行评分失败(池内 panic 经 catch_unwind 隔离,理论不可达——
/// 闭包纯计算)自动回退串行评分,保证 `compress` 签名零变化(无 Result)。
///
/// `entries` 以切片借用传入;并行路径在主线程一次性 `Arc::new(entries.to_vec())`
/// 做引用计数复制(仅 N 次 refcount inc,条目内容 Arc 零复制),闭包内零输入复制。
pub(crate) fn score_entries(
    entries: &[Arc<ContextEntry>],
    weights: SelectorWeights,
    task_clv: Option<&CLV>,
    now: DateTime<Utc>,
    max_access_count: f32,
    time_span_ms: f32,
    parallel_enabled: bool,
) -> Vec<f32> {
    if !should_parallel(parallel_enabled)
        || bridge().route(TaskKind::CscCollapseScore, entries.len()) == DispatchPlan::Inline
    {
        return score_serial(
            entries,
            weights,
            task_clv,
            now,
            max_access_count,
            time_span_ms,
        );
    }
    match score_parallel(
        entries,
        weights,
        task_clv,
        now,
        max_access_count,
        time_span_ms,
    ) {
        Ok(scores) => scores,
        Err(e) => {
            tracing::warn!(error = %e, "并行压缩评分失败,回退串行评分");
            score_serial(
                entries,
                weights,
                task_clv,
                now,
                max_access_count,
                time_span_ms,
            )
        }
    }
}

/// 串行路径 — 顺序评分(回退 + Inline 分支,与注入前行为逐位一致)
#[must_use]
fn score_serial(
    entries: &[Arc<ContextEntry>],
    weights: SelectorWeights,
    task_clv: Option<&CLV>,
    now: DateTime<Utc>,
    max_access_count: f32,
    time_span_ms: f32,
) -> Vec<f32> {
    entries
        .iter()
        .map(|e| {
            compute_importance_score(e, weights, task_clv, now, max_access_count, time_span_ms)
        })
        .collect()
}

/// 并行路径 — `spawn_compute_batch` 段间评分,结果序 = 输入序(段内保序 + 段间按序拼接)
///
/// CHUNK 分组:每个闭包处理一个 chunk 的评分(捕获 `Arc<Vec<Arc<ContextEntry>>>` +
/// 索引范围,条目内容零复制),返回 chunk 段内 `Vec<f32>`(段内保序);
/// 各 chunk 结果按段序拼接 → 与串行逐元素一致。
///
/// 池内 panic 被 catch_unwind 隔离:理论不可达(闭包纯计算),防御性映射为 Err 触发
/// 调用方回退串行。
fn score_parallel(
    entries: &[Arc<ContextEntry>],
    weights: SelectorWeights,
    task_clv: Option<&CLV>,
    now: DateTime<Utc>,
    max_access_count: f32,
    time_span_ms: f32,
) -> Result<Vec<f32>, String> {
    // 主线程一次性引用计数复制(仅 N 次 Arc refcount inc,内容零复制);闭包内零复制
    let all: Arc<Vec<Arc<ContextEntry>>> = Arc::new(entries.to_vec());
    // CLV 克隆(仅 512×f32 = 2KB 栈上数组,跨线程闭包需 'static 所有权)
    let task_clv_owned = task_clv.cloned();

    let n_chunks = all.len().div_ceil(CHUNK);
    let tasks: Vec<Box<dyn FnOnce() -> Vec<f32> + Send>> = (0..n_chunks)
        .map(|ci| {
            let all = Arc::clone(&all);
            let task_clv = task_clv_owned.clone();
            let start = ci * CHUNK;
            let end = (start + CHUNK).min(all.len());
            Box::new(move || {
                all[start..end]
                    .iter()
                    .map(|e| {
                        compute_importance_score(
                            e,
                            weights,
                            task_clv.as_ref(),
                            now,
                            max_access_count,
                            time_span_ms,
                        )
                    })
                    .collect()
            }) as Box<dyn FnOnce() -> Vec<f32> + Send>
        })
        .collect();

    let results = bridge().spawn_compute_batch(TaskKind::CscCollapseScore, tasks);

    // 按段序拼接(段间保序),总长度 = 输入数
    let mut out = Vec::with_capacity(all.len());
    for r in results {
        match r {
            Ok(chunk_out) => out.extend(chunk_out),
            Err(e) => {
                return Err(format!("并行压缩评分 chunk 计算异常: {e}"));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::HcwConfig;

    /// 构造测试条目(确定性:递增 access_count/年龄,部分带 CLV 以含余弦相似度成本)
    fn make_entries(n: usize) -> Vec<ContextEntry> {
        (0..n)
            .map(|i| {
                let mut entry = ContextEntry::new(
                    format!("e-{i}"),
                    "file-1",
                    format!("content-{i}"),
                    100 + (i % 5) * 10,
                );
                entry.access_count = (i % 7) as u32;
                entry.last_accessed_at = Utc::now() - chrono::Duration::milliseconds(i as i64 * 13);
                // 每 3 条带确定性 CLV(512 维,含余弦相似度计算路径)
                if i % 3 == 0 {
                    let v: Vec<f32> = (0..512)
                        .map(|j| ((i * 31 + j * 7) % 1000) as f32 / 1000.0)
                        .collect();
                    entry.clv = Some(CLV::from_vec(v).expect("512 维"));
                }
                entry
            })
            .collect()
    }

    /// 测试辅助:`Vec<ContextEntry>` → `Vec<Arc<ContextEntry>>`
    fn to_arc(entries: Vec<ContextEntry>) -> Vec<Arc<ContextEntry>> {
        entries.into_iter().map(Arc::new).collect()
    }

    /// 固定任务 CLV(非 None,走余弦相似度相关性路径)
    fn task_clv() -> CLV {
        let v: Vec<f32> = (0..512)
            .map(|j| ((j * 11) % 1000) as f32 / 1000.0)
            .collect();
        CLV::from_vec(v).expect("512 维")
    }

    /// 评分参数一次性固定装配（确定性,与 compressor 调用方同构）
    ///
    /// WHY(P1-T14 实测教训):原实现每次调用独立采样 `Utc::now()`,serial/parallel
    /// 两路调用间差几毫秒 → `delta_ms` 取整边界差 1ms → recency f32 最后一位不一致
    /// → 逐元素断言误报。改为 `now = newest`（从条目自身导出,完全确定性）,
    /// 任意两次调用返回相同参数,serial/parallel 以相同基准评分。
    fn score_params(
        entries: &[Arc<ContextEntry>],
    ) -> (SelectorWeights, DateTime<Utc>, f32, f32, CLV) {
        let weights = HcwConfig::default().selector_policy.weights();
        let max_access_count = entries
            .iter()
            .map(|e| e.access_count)
            .max()
            .unwrap_or(0)
            .max(1) as f32;
        let oldest = entries
            .iter()
            .map(|e| e.last_accessed_at)
            .min()
            .unwrap_or(Utc::now());
        let newest = entries
            .iter()
            .map(|e| e.last_accessed_at)
            .max()
            .unwrap_or(Utc::now());
        let time_span_ms = (newest - oldest).num_milliseconds().max(1) as f32;
        // now 固定为 newest(确定性),不用 Utc::now()(两次调用间漂移 → 微差误报)
        (weights, newest, max_access_count, time_span_ms, task_clv())
    }

    /// 评分入口装配(与 compressor 调用方一致;serial/parallel 共用固定参数)
    fn score_all(entries: &[Arc<ContextEntry>], config_flag: bool) -> Vec<f32> {
        let (weights, now, max_access_count, time_span_ms, clv) = score_params(entries);
        score_entries(
            entries,
            weights,
            Some(&clv),
            now,
            max_access_count,
            time_span_ms,
            config_flag,
        )
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
        // 配置开启 + env 未关 → 并行(进程级 OnceLock,测试环境默认未设置 → false)
        if !env_no_parallel() {
            assert!(should_parallel(true));
        }
        // 配置关闭 → 强制串行(无论 env)
        assert!(!should_parallel(false));
    }

    // ============================================================
    // 并行 vs 串行一致性 + 顺序断言(段序不变)
    // ============================================================

    #[test]
    fn test_parallel_matches_serial_large_batch() {
        // 2000 条目(≥ CscCollapseScore 阈值 200 → Rayon 分支)
        let entries = to_arc(make_entries(2_000));
        let serial = score_all(&entries, false);
        let parallel = score_all(&entries, true);
        assert_eq!(serial.len(), 2_000, "评分数必须等于条目数");
        assert_eq!(parallel.len(), 2_000);
        // 逐元素一致 + 顺序断言(段序不变,并行不得重排评分)
        for (i, (s, p)) in serial.iter().zip(parallel.iter()).enumerate() {
            assert_eq!(
                s.to_bits(),
                p.to_bits(),
                "entry[{i}] 评分必须与串行逐元素一致(含顺序)"
            );
        }
    }

    #[test]
    fn test_parallel_matches_serial_small_batch() {
        // 小批量(< 阈值)→ Inline 串行,结果一致
        let entries = to_arc(make_entries(90));
        let serial = score_all(&entries, false);
        let parallel = score_all(&entries, true);
        assert_eq!(serial, parallel, "小批量(Inline 串行)结果必须一致");
    }

    // ============================================================
    // 边界:空输入 / 单元素 / 非整 chunk
    // ============================================================

    #[test]
    fn test_empty_entries() {
        let entries: Vec<Arc<ContextEntry>> = Vec::new();
        let out = score_all(&entries, true);
        assert!(out.is_empty());
    }

    #[test]
    fn test_single_entry() {
        let entries = to_arc(make_entries(1));
        let out = score_all(&entries, true);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn test_odd_chunk_boundary() {
        // 65 个条目 = 1 个整 chunk(64) + 1 个残块 → 跨块拼接顺序正确
        let entries = to_arc(make_entries(65));
        let serial = score_all(&entries, false);
        let parallel = score_all(&entries, true);
        assert_eq!(serial, parallel, "非整 chunk 边界结果必须与串行一致");
    }

    // ============================================================
    // 回退开关(配置关闭 → 强制串行,结果与串行一致)
    // ============================================================

    #[test]
    fn test_config_disable_falls_back_to_serial() {
        // 大 N(≥ 阈值)下配置关闭 → 必须仍走串行且结果正确
        let entries = to_arc(make_entries(2_000));
        let disabled = score_all(&entries, false);
        let serial = score_serial_without_bridge(&entries);
        assert_eq!(disabled, serial, "配置关闭后结果必须与串行路径一致");
    }

    /// 对照:直接串行评分(不经并行模块,等价注入前行为;与 `score_all` 同固定参数)
    fn score_serial_without_bridge(entries: &[Arc<ContextEntry>]) -> Vec<f32> {
        let (weights, now, max_access_count, time_span_ms, clv) = score_params(entries);
        entries
            .iter()
            .map(|e| {
                compute_importance_score(
                    e,
                    weights,
                    Some(&clv),
                    now,
                    max_access_count,
                    time_span_ms,
                )
            })
            .collect()
    }

    /// env 关闭开关 → 走串行(集成验证:进程级 env 首次读取即生效)
    ///
    /// WHY OnceLock:本测试设置 env 后,`env_no_parallel()` 首次读取即缓存关闭态;
    /// 若其他测试先调用导致已缓存 false,本测试跳过断言(不污染其他测试)。
    #[test]
    fn test_env_disable_falls_back_to_serial() {
        if env_no_parallel() {
            // 已有其他来源关闭 —— 缓存生效,直接验证串行一致性
            let entries = to_arc(make_entries(2_000));
            let out = score_all(&entries, true);
            assert_eq!(out.len(), 2_000);
            return;
        }
        // env 尚未缓存:设置后首次读取 → 关闭并行(OnceLock 一次性语义)
        std::env::set_var(ENV_NO_PARALLEL, "1");
        let entries = to_arc(make_entries(2_000));
        let out = score_all(&entries, true);
        let serial = score_serial_without_bridge(&entries);
        assert_eq!(out, serial, "env 关闭后结果必须与串行路径一致");
        // 恢复 env,避免影响同进程其他测试(OnceLock 已缓存,恢复仅对子进程有意义)
        std::env::remove_var(ENV_NO_PARALLEL);
    }

    // ============================================================
    // 配置开关与路由接线
    // ============================================================

    #[test]
    fn test_config_flag_wires_through() {
        let entries = to_arc(make_entries(2_000));
        let config = HcwConfig::default();
        // 配置默认 true → 路由判定走 should_parallel(config.parallel_compress)
        let out = score_all(&entries, config.parallel_compress);
        assert_eq!(out.len(), 2_000);
        // 配置 false → 强制串行
        let disabled = HcwConfig::default().with_parallel_compress(false);
        assert!(!disabled.parallel_compress);
        let out = score_all(&entries, disabled.parallel_compress);
        assert_eq!(out.len(), 2_000);
    }
}
