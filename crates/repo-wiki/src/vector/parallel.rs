//! P1-T14: 批量 KNN 检索的 ComputeBridge 并行注入
//!
//! 对应任务:P1-T14（WI-34 并行化第 2 批补全,v4.0 §7.5.1 L-a）
//! 对应架构层:L5 Knowledge
//! 对应注入表:W8-9「repo-wiki KNN 并行;扫描成本降 ≥85%」
//!
//! # 注入模式（WI-34 七条纪律 ④桥接唯一 / ⑤逐一回滚 / ⑥IO 不上 rayon）
//! - **识别热点**:[`crate::vector::VectorIndex`] 的批量 KNN 检索路径 —— 对
//!   `queries × vectors` 全量余弦相似度计算 + Top-K 选择（`select_nth_unstable_by`）,
//!   纯 CPU 密集（无 IO / await / 持锁）,单查询 O(n·d) 遍历,批量下查询间完全独立。
//! - **快照分离,计算并行**:批量路径主线程一次性快照 `(id, Vec<f32>)` 为
//!   `Arc<Vec<(String, Vec<f32>)>>`（RwLock 读锁在快照窗口内短暂持有,不跨闭包边界;
//!   快照后按 id 排序 —— HashMap 迭代序不确定会影响分数 tie 的相对顺序,
//!   排序保证并行/串行逐元素一致的确定性,Ω₂）;rayon 闭包内仅做**纯计算**
//!   （余弦相似度 + Top-K 选择,无 IO / await / 持锁）。
//! - **结果保序**:查询 i 的结果必须对应输入 i —— `spawn_compute_batch` 槽位写入
//!   保证结果序 = 输入序,chunk 内保序 + chunk 间按段序拼接,双重保序;
//!   测试用顺序断言（结果[0] ↔ queries[0]）锁定。
//! - **挂 ComputeBridge**:[`ComputeBridge::route`](nexus_core::compute::ComputeBridge::route)
//!   按 `TaskKind::KnnSearch`（阈值 5,000,已登记）三态判定,`n_items = q × v`
//!   （总相似度计算数）→ `Inline`（低于阈值,串行）或 `Rayon`
//!   （[`spawn_compute_batch`](nexus_core::compute::ComputeBridge::spawn_compute_batch)
//!   批量并行）。
//! - **保留回退**:`VectorIndex::parallel_search` 配置开关 + `CHIMERA_NO_PARALLEL_WIKI`
//!   环境变量（启动期 OnceLock 读取一次,不在热路径）双重关闭 → 强制串行;
//!   并行计算失败（理论不可达:闭包纯计算）防御性回退串行。
//!
//! # 任务粒度
//! `CHUNK = 8` 个 query 一个任务（128 queries → 16 任务,8 线程池两批摊平,
//! 负载均衡细粒度;阈值表 KnnSearch 登记的 chunk 256 是参考,spawn 按 item
//! 直接调度 —— chunk 是调用方自由参数,过大 → 任务数 < 线程数,并行度不足,
//! 实测教训 CHUNK=256 时 128 queries → 1 任务 → 0.98×;CHUNK=16 → 3.12×;
//! CHUNK=8 → 3.87×）;每个闭包处理一个 chunk 的全部 KNN（chunk 次余弦遍历 + Top-K）。
//!
//! # rayon 闭包契约
//! 闭包捕获 `Arc<Vec<Vec<f32>>>`（queries,零复制）+ `Arc<Vec<(String, Vec<f32>)>>`
//! （vectors,零复制）+ `top_k` + 索引范围,仅调 [`knn_top_k`] 纯计算;
//! 闭包内禁 IO / await / 持锁（红线 §7.5.3 纪律⑥）。
//!
//! # 失败语义
//! KNN 纯计算无错误面（维度校验在调用方 `search_batch` 预检,原子性:零计算零检索）。
//! 池内 panic 被 catch_unwind 隔离（理论不可达）,防御性整体回退串行。
//! 结果一致性断言:并行输出与串行输出逐 query 逐位一致（score `to_bits()` +
//! 顺序 + id 全等）。

use std::sync::{Arc, OnceLock};

use nexus_core::compute::{bridge, DispatchPlan, TaskKind};

/// 环境变量关闭开关名（纪律⑤;仅测试/运维使用）
const ENV_NO_PARALLEL: &str = "CHIMERA_NO_PARALLEL_WIKI";

/// 进程级 env 缓存 — 启动期读取一次,不在热路径（任务约束）
static NO_PARALLEL_ENV: OnceLock<bool> = OnceLock::new();

/// 并行分块大小 — 任务粒度（每任务 query 数）。KNN 单 query 计算较重
/// （512 向量 × 64 维余弦 + Top-K 选择 ≈ 30μs+）,chunk 越小任务数越多、
/// 负载均衡越好;阈值表 KnnSearch 登记的 chunk 为 256,但 spawn 按 item
/// 直接调度（chunk 是调用方自由参数）,实测 CHUNK=256 时 128 queries
/// → 仅 1 个任务,完全无并行（0.98×）;CHUNK=16 → 8 任务实测 3.12×;
/// CHUNK=8 → 16 任务实测 3.87×（本机 8 线程池,本版本保留）;
/// 实测曲线 256→16→8 单调提升,任务数（≥ 线程数×2）为并行度主因。
const CHUNK: usize = 8;

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

/// 单查询 KNN 纯计算 — 对快照内全部向量计算余弦相似度 + Top-K 选择
/// （与 [`crate::vector::VectorIndex::search`] 逐分支一致:select_nth_unstable_by
/// 替代 sort_by 的工程约定,前 K 无序集合 + 最终 K log K 稳定降序排序）。
///
/// `vectors` 为已按 id 排序的快照（调用方保证）→ 分数 tie 时相对顺序确定
/// （`sort_by` 稳定排序保留快照顺序）→ 并行/串行输出逐位一致。
#[must_use]
pub(crate) fn knn_top_k(
    query: &[f32],
    vectors: &[(String, Vec<f32>)],
    top_k: usize,
) -> Vec<(String, f32)> {
    let mut scored: Vec<(String, f32)> = vectors
        .iter()
        .map(|(id, vec)| (id.clone(), nexus_core::cosine_similarity_slices(query, vec)))
        .collect();

    // Top-K 选择用 select_nth_unstable_by (O(n)),仅对前 K 做 K-log-K 排序
    // （与 VectorIndex::search 同一工程约定,零漂移）
    if top_k < scored.len() {
        scored.select_nth_unstable_by(top_k, |a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    scored.truncate(top_k);
    // 前 K 元素已是无序的 Top-K 集合,这里做最终降序排序(K log K,稳定 → tie 保序)
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
}

/// 批量 KNN 检索计算核心（ComputeBridge 路由判定入口,供 VectorIndex 调用）
///
/// `queries` / `vectors` 以 `&Arc<Vec<_>>` 传入:并行闭包 `Arc::clone` 共享
/// 容器 + 索引范围,**零向量级复制**。`top_k` 为标量 Copy。
///
/// 路由判定:
/// ① `should_parallel(parallel_enabled)` 为 false（配置/env 关闭）→ 串行;
/// ② `bridge().route(TaskKind::KnnSearch, n_items)` 为 `Inline` → 串行
///    （`n_items = queries.len() × vectors.len()` = 总相似度计算数）;
/// ③ 否则 → `spawn_compute_batch` 并行（CHUNK 分组,段内保序 + 段间按序拼接）。
///
/// 返回 `Vec<Vec<(String, f32)>>`,**结果序 = 输入 query 序**（查询 i 的结果
/// 对应输入 i,顺序断言锁定）。KNN 纯计算无错误面;并行意外失败（理论不可达:
/// 闭包纯计算）防御性回退串行。
#[must_use]
pub(crate) fn knn_core(
    queries: &Arc<Vec<Vec<f32>>>,
    vectors: &Arc<Vec<(String, Vec<f32>)>>,
    top_k: usize,
    parallel_enabled: bool,
) -> Vec<Vec<(String, f32)>> {
    let q = queries.len();
    if q == 0 {
        return Vec::new();
    }
    let n_items = q.saturating_mul(vectors.len());
    if !should_parallel(parallel_enabled)
        || bridge().route(TaskKind::KnnSearch, n_items) == DispatchPlan::Inline
    {
        knn_batch_serial(queries, vectors, top_k)
    } else {
        match knn_batch_parallel(queries, vectors, top_k) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "并行批量 KNN 检索失败,回退串行");
                knn_batch_serial(queries, vectors, top_k)
            }
        }
    }
}

/// 串行路径 — 按输入序逐 query 应用 KNN（回退 + Inline 分支;
/// 与并行路径共用 [`knn_top_k`] 纯函数与排序后的快照,保证逐元素一致）
fn knn_batch_serial(
    queries: &Arc<Vec<Vec<f32>>>,
    vectors: &Arc<Vec<(String, Vec<f32>)>>,
    top_k: usize,
) -> Vec<Vec<(String, f32)>> {
    queries
        .iter()
        .map(|query| knn_top_k(query, vectors, top_k))
        .collect()
}

/// 并行路径 — `spawn_compute_batch` 段间 KNN 计算,结果序 = 输入序
///
/// CHUNK 分组:每个闭包处理一个 chunk 的 query（捕获 `Arc` 容器 + 索引范围,
/// 零向量复制）,返回 chunk 段内 `Vec<Vec<(String, f32)>>`（段内保序）;
/// 各 chunk 结果按段序拼接（段间保序）→ 查询 i 的结果恒对应输入 i。
///
/// 池内 panic 被 catch_unwind 隔离:理论不可达（闭包纯计算）,防御性映射为
/// Err 触发调用方回退串行。
fn knn_batch_parallel(
    queries: &Arc<Vec<Vec<f32>>>,
    vectors: &Arc<Vec<(String, Vec<f32>)>>,
    top_k: usize,
) -> Result<Vec<Vec<(String, f32)>>, String> {
    let q = queries.len();
    let n_chunks = q.div_ceil(CHUNK);
    type ChunkTask = Box<dyn FnOnce() -> Vec<Vec<(String, f32)>> + Send>;
    let tasks: Vec<ChunkTask> = (0..n_chunks)
        .map(|ci| {
            let qs = Arc::clone(queries);
            let vs = Arc::clone(vectors);
            let start = ci * CHUNK;
            let end = (start + CHUNK).min(q);
            Box::new(move || {
                qs[start..end]
                    .iter()
                    .map(|query| knn_top_k(query, &vs, top_k))
                    .collect()
            }) as Box<dyn FnOnce() -> Vec<Vec<(String, f32)>> + Send>
        })
        .collect();

    let results = bridge().spawn_compute_batch(TaskKind::KnnSearch, tasks);

    // 按段序拼接（段间保序）,总长度 = 输入 query 数
    let mut out = Vec::with_capacity(q);
    for r in results {
        match r {
            Ok(chunk_out) => out.extend(chunk_out),
            Err(e) => return Err(format!("并行批量 KNN chunk 计算异常: {e}")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 确定性伪随机向量（同 wiki_knn_slo bench 口径:分量 ∈ (0,1),无零向量）
    fn make_vector(id: u64, dim: usize) -> Vec<f32> {
        (0..dim)
            .map(|j| {
                let h = id
                    .wrapping_mul(7)
                    .wrapping_add((j as u64).wrapping_mul(13))
                    .wrapping_mul(31);
                let v = (h % 100003) as f32 / 100003.0;
                v + 0.001
            })
            .collect()
    }

    /// 构造批量输入（queries + 已按 id 排序的 vectors 快照）
    type VectorStore = Vec<(String, Vec<f32>)>;
    fn make_items(q: usize, v: usize, dim: usize) -> (Vec<Vec<f32>>, VectorStore) {
        let queries: Vec<Vec<f32>> = (0..q).map(|i| make_vector(i as u64, dim)).collect();
        let mut vectors: Vec<(String, Vec<f32>)> = (0..v)
            .map(|i| (format!("vec-{i:05}"), make_vector(i as u64, dim)))
            .collect();
        // 快照确定性:按 id 排序（与 VectorIndex::search_batch 一致）
        vectors.sort_by(|a, b| a.0.cmp(&b.0));
        (queries, vectors)
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
        if !env_no_parallel() {
            assert!(should_parallel(true));
        }
        assert!(!should_parallel(false));
    }

    // ============================================================
    // 纯函数行为锁定（与 VectorIndex::search 语义一致）
    // ============================================================

    #[test]
    fn test_knn_top_k_basic() {
        let queries = [vec![1.0, 0.0, 0.0, 0.0]];
        let vectors = vec![
            ("b".to_string(), vec![0.0, 1.0, 0.0, 0.0]),
            ("a".to_string(), vec![1.0, 0.0, 0.0, 0.0]),
            ("c".to_string(), vec![0.9, 0.1, 0.0, 0.0]),
        ];
        let out = knn_top_k(&queries[0], &vectors, 2);
        assert_eq!(out.len(), 2);
        // 与 query 相同的向量应排第一（相似度 1.0）
        assert_eq!(out[0].0, "a");
        assert!((out[0].1 - 1.0).abs() < 1e-5);
        // 降序:out[0] >= out[1]
        assert!(out[0].1 >= out[1].1);
    }

    #[test]
    fn test_knn_top_k_top_k_larger_than_size() {
        let vectors = vec![("a".to_string(), vec![1.0, 0.0, 0.0, 0.0])];
        let out = knn_top_k(&[1.0, 0.0, 0.0, 0.0], &vectors, 10);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "a");
    }

    #[test]
    fn test_knn_top_k_empty_vectors() {
        let out = knn_top_k(&[1.0, 0.0, 0.0, 0.0], &[], 5);
        assert!(out.is_empty());
    }

    #[test]
    fn test_knn_top_k_tie_deterministic() {
        // 分数 tie:两个完全相同的向量 → 稳定排序保留快照顺序（确定性,Ω₂）
        let vectors = vec![
            ("z".to_string(), vec![1.0, 0.0]),
            ("a".to_string(), vec![1.0, 0.0]),
            ("m".to_string(), vec![1.0, 0.0]),
        ];
        let out1 = knn_top_k(&[1.0, 0.0], &vectors, 3);
        let out2 = knn_top_k(&[1.0, 0.0], &vectors, 3);
        assert_eq!(out1, out2, "tie 场景必须确定（同快照同输入同输出）");
        // 快照顺序 z < a < m → 稳定排序输出顺序 z, a, m
        assert_eq!(
            out1.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            vec!["z", "a", "m"]
        );
    }

    // ============================================================
    // 并行 vs 串行一致性（快照语义,逐 query 逐位 + 顺序断言）
    // ============================================================

    /// 装配:构造 q 个 query × v 个向量,分别走串行与并行核心,逐 query 断言
    fn assert_serial_matches_parallel(q: usize, v: usize, dim: usize) {
        let (queries, vectors) = make_items(q, v, dim);
        let qs = Arc::new(queries);
        let vs = Arc::new(vectors);
        let serial = knn_core(&qs, &vs, 5, false);
        let parallel = knn_core(&qs, &vs, 5, true);
        assert_eq!(serial.len(), q, "串行结果数必须等于输入 query 数");
        assert_eq!(parallel.len(), q, "并行结果数必须等于输入 query 数");
        for (i, (s, p)) in serial.iter().zip(parallel.iter()).enumerate() {
            assert_eq!(s.len(), p.len(), "query[{i}] top-k 数必须一致");
            for (j, (s_id, s_score)) in s.iter().enumerate() {
                let (p_id, p_score) = &p[j];
                assert_eq!(s_id, p_id, "query[{i}] hit[{j}] id 必须与串行一致(含顺序)");
                assert_eq!(
                    s_score.to_bits(),
                    p_score.to_bits(),
                    "query[{i}] hit[{j}] score 必须与串行逐位一致"
                );
            }
        }
    }

    #[test]
    fn test_parallel_matches_serial_large_batch() {
        // 128 × 512 = 65_536 ≥ KnnSearch 阈值 5_000 → Rayon 分支
        assert_serial_matches_parallel(128, 512, 64);
    }

    #[test]
    fn test_parallel_matches_serial_small_batch() {
        // 4 × 64 = 256 < 阈值 → Inline 串行,结果一致
        assert_serial_matches_parallel(4, 64, 64);
    }

    #[test]
    fn test_parallel_matches_serial_boundary_over_threshold() {
        // 100 × 51 = 5_100 ≥ 阈值 5_000 → Rayon 分支（阈值边界验证）
        assert_serial_matches_parallel(100, 51, 64);
    }

    // ============================================================
    // 边界:空输入 / 单 query / 非整 chunk
    // ============================================================

    #[test]
    fn test_empty_queries() {
        let qs = Arc::new(Vec::new());
        let vs = Arc::new(Vec::new());
        let out = knn_core(&qs, &vs, 5, true);
        assert!(out.is_empty());
    }

    #[test]
    fn test_single_query() {
        let (queries, vectors) = make_items(1, 64, 64);
        let qs = Arc::new(queries);
        let vs = Arc::new(vectors);
        let out = knn_core(&qs, &vs, 5, true);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), 5);
    }

    #[test]
    fn test_odd_chunk_boundary() {
        // 257 = 16 个整 chunk(16×16) + 1 个残块 → 跨块拼接顺序正确
        // 257 × 64 = 16_448 ≥ 阈值 → Rayon 分支
        assert_serial_matches_parallel(257, 64, 64);
    }

    // ============================================================
    // 回退开关（配置关闭 → 强制串行,结果一致）
    // ============================================================

    #[test]
    fn test_config_disable_falls_back_to_serial() {
        let (queries, vectors) = make_items(128, 512, 64);
        let qs = Arc::new(queries);
        let vs = Arc::new(vectors);
        let disabled = knn_core(&qs, &vs, 5, false);
        let direct_serial = knn_batch_serial(&qs, &vs, 5);
        assert_eq!(disabled.len(), direct_serial.len());
        for (i, (a, b)) in disabled.iter().zip(direct_serial.iter()).enumerate() {
            assert_eq!(a.len(), b.len(), "query[{i}] top-k 数必须一致");
            for (j, (a_id, a_score)) in a.iter().enumerate() {
                let (b_id, b_score) = &b[j];
                assert_eq!(a_id, b_id, "query[{i}] hit[{j}] id");
                assert_eq!(
                    a_score.to_bits(),
                    b_score.to_bits(),
                    "query[{i}] hit[{j}] score"
                );
            }
        }
    }

    /// env 关闭开关 → 走串行（集成验证:进程级 env 首次读取即生效）
    ///
    /// WHY OnceLock:本测试设置 env 后,`env_no_parallel()` 首次读取即缓存关闭态;
    /// 若其他测试先调用导致已缓存 false,本测试跳过断言（不污染其他测试）。
    #[test]
    fn test_env_disable_falls_back_to_serial() {
        if env_no_parallel() {
            // 已有其他来源关闭 —— 缓存生效,直接验证串行一致性
            let (queries, vectors) = make_items(128, 512, 64);
            let qs = Arc::new(queries);
            let vs = Arc::new(vectors);
            let out = knn_core(&qs, &vs, 5, true);
            let serial = knn_batch_serial(&qs, &vs, 5);
            assert_eq!(out.len(), serial.len());
            return;
        }
        // env 尚未缓存:设置后首次读取 → 关闭并行（OnceLock 一次性语义）
        std::env::set_var(ENV_NO_PARALLEL, "1");
        let (queries, vectors) = make_items(128, 512, 64);
        let qs = Arc::new(queries);
        let vs = Arc::new(vectors);
        let out = knn_core(&qs, &vs, 5, true);
        let serial = knn_batch_serial(&qs, &vs, 5);
        assert_eq!(out.len(), serial.len());
        // 恢复 env,避免影响同进程其他测试（OnceLock 已缓存,恢复仅对子进程有意义）
        std::env::remove_var(ENV_NO_PARALLEL);
    }
}
