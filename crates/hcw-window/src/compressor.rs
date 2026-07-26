//! 上下文压缩器 — 基于重要性评分的 Top-N 保留压缩
//!
//! 对应架构层:L2 Memory
//! 对应创新点:HCW §上下文压缩
//! 对应任务: P3-W10.3(D1 修复 — selector 权重外置)
//!
//! # 核心职责
//! - 按重要性评分(权重由 `SelectorPolicy` 注入,默认 0.4×时近性 + 0.3×频次 + 0.3×任务相关性)排序条目
//! - 贪心保留 Top-N 使总大小 ≤ target_size
//! - 返回 CompressionReport(原始/压缩后大小、保留/丢弃条目数、压缩倍率)
//!
//! # 重要性评分公式
//! `score = w1 × recency + w2 × frequency + w3 × relevance`
//! - 权重 `(w1, w2, w3)` 由 `HcwConfig.selector_policy` 注入(P3-W10.3 D1 修复):
//!   - `SelectorPolicy::Static(weights)`: 编译进二进制的常量(默认 0.4/0.3/0.3,fallback)
//!   - `SelectorPolicy::Learned { version, weights }`: omega-learner 异步下发值(P4 S4 接缝)
//! - `recency`(时近性):1.0 - (Δt / time_span),最新的为 1.0,最旧的为 0.0
//! - `frequency`(频次):access_count / max_access_count,最高频为 1.0
//! - `relevance`(任务相关性):CLV 余弦相似度,clamp 到 [0.0, 1.0]
//!
//! # 设计决策(WHY)
//! - **非语义压缩**:Week 3 阶段用重要性评分 Top-N,Week 6 NMC 后接入语义压缩
//! - **至少保留 1 个条目**:即使所有条目都 > target_size,也保留最高分条目,
//!   避免压缩后上下文为空(此时 compressed_size 可能 > target_size,触发窗口升级)
//! - **compression_ratio = original/compressed**:压缩比(>1.0,越大压缩越多),
//!   任务要求"压缩率 > 3×"即 compression_ratio > 3.0。
//!   `compressed_size == 0` 时取 `f32::MAX`(非 INFINITY,避免序列化失败)
//! - **P3-W10.3 D1 修复**:权重从 `compressor_weights: (f32, f32, f32)` 硬编码常量
//!   升级为 `selector_policy: SelectorPolicy` 注入式策略(C4 合规:fallback 编译进二进制)

use std::sync::Arc;

use chrono::{DateTime, Utc};
use nexus_contracts::SelectorWeights;
use nexus_core::CLV;

use crate::types::{CompressionReport, ContextEntry, HcwConfig};

/// 上下文压缩器 — 基于重要性评分的 Top-N 保留
///
/// 纯函数式压缩器,无内部状态,`compress` 为关联函数。
///
/// # 性能基准
/// - 100K Token 压缩到 32K,压缩率 > 3×(compression_ratio > 3.0)
/// - 端到端压缩率 > 4×(128K → 32K,compression_ratio > 4.0)
pub struct ContextCompressor;

/// 无 CLV 时的默认相关性(中性值 0.5)
///
/// WHY `pub(crate)`: P4-W13.3.2 `SelectorLearnerHolder::compute_importance_with_policy`
/// 复用此常量,确保 `ContextCompressor` 与 `SelectorLearnerHolder` 的"无 CLV 中性值"一致,
/// 避免 magic number 分散导致行为漂移。
pub(crate) const DEFAULT_RELEVANCE: f32 = 0.5;

/// 计算单个条目的重要性评分 — 共享公式(P4-W13.3.2 提取)
///
/// 公式: `score = w1 × recency + w2 × frequency + w3 × relevance`
///
/// - `recency`(时近性): 1.0 - (Δt / time_span),最新的为 1.0,最旧的为 0.0
/// - `frequency`(频次): access_count / max_access_count,最高频为 1.0
/// - `relevance`(任务相关性): CLV 余弦相似度 clamp 到 [0.0, 1.0],
///   无 CLV 时取 `DEFAULT_RELEVANCE`(中性 0.5)
///
/// # 设计决策(WHY 提取为 pub(crate) 自由函数)
///
/// 原实现 `ContextCompressor::compute_importance` 是私有方法,权重从 `&HcwConfig`
/// 读取(配置时固定)。P4-W13.3.2 `SelectorLearnerHolder` 需要运行时可变策略
/// (omega-learner 异步下发的 `SelectorPolicy::Learned`),若复制公式会产生
/// 冗余代码(违反 §全局指令:杜绝冗余代码)。
///
/// 提取为 `pub(crate)` 自由函数后:
/// - `ContextCompressor::compute_importance` 传入 `config.selector_policy.weights()`
/// - `SelectorLearnerHolder::compute_importance_with_policy` 传入 `self.current_policy().weights()`
///
/// 二者共享同一公式,避免行为漂移。
///
/// # 参数
/// - `entry`: 待评分的上下文条目
/// - `weights`: 权重三元组(由 `SelectorPolicy::weights()` 提供)
/// - `task_clv`: 当前任务的 CLV(None 时相关性取中性 0.5)
/// - `now`: 当前时间(用于时近性计算)
/// - `max_access_count`: 最大访问次数(用于频次归一化,调用方确保 > 0)
/// - `time_span_ms`: 时间跨度毫秒(用于时近性归一化,调用方确保 > 0)
pub(crate) fn compute_importance_score(
    entry: &ContextEntry,
    weights: SelectorWeights,
    task_clv: Option<&CLV>,
    now: DateTime<Utc>,
    max_access_count: f32,
    time_span_ms: f32,
) -> f32 {
    let (recency_weight, frequency_weight, relevance_weight) = weights.as_tuple();

    // 时近性:最近访问的条目评分更高
    let delta_ms = (now - entry.last_accessed_at).num_milliseconds().max(0) as f32;
    let recency = 1.0 - (delta_ms / time_span_ms).min(1.0);

    // 频次:高频访问的条目评分更高
    let frequency = entry.access_count as f32 / max_access_count;

    // 任务相关性:CLV 余弦相似度 clamp 到 [0.0, 1.0]
    let relevance = match (task_clv, entry.clv.as_ref()) {
        (Some(task), Some(entry_clv)) => task.cosine_similarity(entry_clv).clamp(0.0, 1.0),
        // 无 CLV 时取中性值 0.5,避免阻塞压缩流程
        _ => DEFAULT_RELEVANCE,
    };

    recency_weight * recency + frequency_weight * frequency + relevance_weight * relevance
}

impl ContextCompressor {
    /// 压缩上下文条目到目标大小
    ///
    /// 流程:
    /// 1. 计算原始总大小与条目数
    /// 2. 若原始大小 ≤ target_size 或条目为空,直接返回(无需压缩,retained_entries 为空)
    /// 3. 计算每个条目的重要性评分(0.4×时近性 + 0.3×频次 + 0.3×任务相关性)
    /// 4. 按评分降序排序
    /// 5. 贪心保留 Top-N 使总大小 ≤ target_size
    /// 6. 若 retained 为空(所有条目都 > target_size),保留最高分 1 个条目
    /// 7. 返回 CompressionReport
    ///
    /// WHY:至少保留 1 个条目 — 避免压缩后上下文为空,此时 compressed_size
    /// 可能 > target_size,调用方(HcwWindow)据此触发窗口升级
    ///
    /// WHY 接受 `&[Arc<ContextEntry>]` 而非 `Vec<ContextEntry>`(SubTask 19.4 + M-01/M-02):
    /// 原实现要求调用方 `state.entries.clone()` 全量 clone 1000 条目后传入,
    /// 现接受 `&[Arc<ContextEntry>]` 借用引用,内部仅 `Arc::clone` 保留的 Top-N 条目
    /// (通常 ≤ 100),消除 900+ 次无用 clone。无需压缩时返回空 retained_entries,
    /// 调用方检查 `algorithm == "none"` 跳过 entries 替换。
    ///
    /// WHY(M-01/M-02):返回的 `retained_entries: Vec<Arc<ContextEntry>>` 与
    /// `HcwState.entries` 同类型,调用方 `state.entries = report.retained_entries`
    /// 是零拷贝移动赋值。compressor 内部从 `&Arc<ContextEntry>` 用 `Arc::clone`
    /// 推入 retained(仅引用计数 O(1)),避免 content String 深拷贝。
    ///
    /// # 参数
    /// - `entries`:待压缩的条目切片(`&[Arc<ContextEntry>]` 借用,不消费)
    /// - `target_size`:目标总 Token 大小
    /// - `task_clv`:当前任务的 CLV(用于相关性计算,None 时相关性取 0.5)
    /// - `now`:当前时间(用于时近性计算)
    pub fn compress(
        config: &HcwConfig,
        entries: &[Arc<ContextEntry>],
        target_size: usize,
        task_clv: Option<&CLV>,
        now: DateTime<Utc>,
    ) -> CompressionReport {
        let original_count = entries.len();
        let original_size: usize = entries.iter().map(|e| e.token_size).sum();

        // 边界:无需压缩(原始大小 ≤ 目标 或 条目为空)
        // WHY 返回空 retained_entries:调用方检查 algorithm == "none" 跳过替换,
        // 避免"无需压缩时仍全量 clone entries"的无谓开销
        if original_size <= target_size || entries.is_empty() {
            return CompressionReport {
                original_size,
                compressed_size: original_size,
                compression_ratio: 1.0,
                original_count,
                retained_count: original_count,
                dropped_count: 0,
                retained_entries: Vec::new(),
                algorithm: "none".into(),
            };
        }

        // 边界:target_size 为 0,保留最高分 1 个条目(避免空上下文)
        let effective_target = if target_size == 0 { 1 } else { target_size };

        // 计算归一化所需的统计量
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
            .unwrap_or(now);
        let newest = entries
            .iter()
            .map(|e| e.last_accessed_at)
            .max()
            .unwrap_or(now);
        // 时间跨度(毫秒),为 0 时所有条目时近性相同(取 1.0)
        let time_span_ms = (newest - oldest).num_milliseconds().max(1) as f32;

        // 计算每个条目的重要性评分并配对(借用 Arc 引用,不 clone)
        // WHY(M-01/M-02):scored 存 `&Arc<ContextEntry>` 而非 `&ContextEntry`,
        // 以便后续 `retained.push(Arc::clone(entry))` 零拷贝推入。
        // compute_importance 接受 `&ContextEntry`,通过 Deref coercion
        // 自动将 `&Arc<ContextEntry>` 解引用为 `&ContextEntry`。
        let mut scored: Vec<(f32, &Arc<ContextEntry>)> = entries
            .iter()
            .map(|e| {
                let score = Self::compute_importance(
                    e,
                    config,
                    task_clv,
                    now,
                    max_access_count,
                    time_span_ms,
                );
                (score, e)
            })
            .collect();

        // === SubTask 13.7:用 select_nth_unstable_by 部分排序替代全排序 ===
        //
        // WHY:原 `sort_by` 全排序 O(n log n),但仅需 Top-K(K << n)。
        // 优化:先估计 K(最多保留的条目数上界),用 `select_nth_unstable_by`
        // 找到 Top-K(O(n)),仅对 Top-K 排序(O(K log K)),
        // 总复杂度 O(n + K log K) < O(n log n)。
        //
        // K 的估计:贪心保留按评分降序,每个保留条目 token_size >= min_token_size,
        // 所以最多保留 effective_target / min_token_size 个。用此作为 K 的上界,
        // 确保 Top-K 包含所有可能被贪心保留的条目,语义与全排序一致。
        let min_token_size = scored
            .iter()
            .map(|(_, e)| e.token_size)
            .min()
            .unwrap_or(1)
            .max(1);
        let estimated_k = (effective_target / min_token_size).min(scored.len()).max(1);

        // 用 select_nth_unstable_by 找到 Top-K(评分最高的 K 个),O(n)
        // 调用后 scored[..K] 是 Top-K(无序),scored[K..] 是评分较低的条目
        {
            let (top_k, ..) = scored.select_nth_unstable_by(estimated_k - 1, |a, b| {
                b.0.partial_cmp(&a.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.1.token_size.cmp(&b.1.token_size))
            });
            // 对 Top-K 排序(降序,评分相同按 token_size 升序),O(K log K)
            top_k.sort_by(|a, b| {
                b.0.partial_cmp(&a.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.1.token_size.cmp(&b.1.token_size))
            });
        }

        // 截断到 Top-K,丢弃评分较低的条目(不在贪心保留范围内)
        scored.truncate(estimated_k);

        // 贪心保留 Top-N 使总大小 ≤ effective_target
        // 在贪心前保留最高分条目作为 fallback(避免压缩后上下文为空)
        // WHY:若所有条目 token_size > effective_target,retained 为空,
        // 此时用最高分条目作为 fallback,调用方据此触发窗口升级
        //
        // WHY(M-01/M-02):scored 元素是 `(f32, &Arc<ContextEntry>)`,
        // `*e` 解引用为 `&Arc<ContextEntry>`(Copy 类型,零成本复制引用)。
        // fallback_entry 类型为 `Option<&Arc<ContextEntry>>`,与 retained 同源,
        // 后续 `Arc::clone(entry)` 零拷贝推入。
        let fallback_entry = scored.first().map(|(_, e)| *e);

        let mut retained: Vec<Arc<ContextEntry>> = Vec::new();
        let mut compressed_size: usize = 0;

        // WHY(M-01/M-02):用 `Arc::clone` 替代 `entry.clone()` —
        // entry 是 `&Arc<ContextEntry>`,`Arc::clone` 仅增加引用计数(O(1)),
        // 避免 `ContextEntry` 深拷贝(content String 被复制)。
        // 1000 条目保留 100 条时,从 100 次 String clone 降为 100 次 refcount inc。
        // token_size 等字段访问通过 Arc 的 Deref 自动解引用,无需修改。
        for (score, entry) in scored {
            if compressed_size + entry.token_size <= effective_target {
                compressed_size += entry.token_size;
                retained.push(Arc::clone(entry));
            }
            // 超出 effective_target 的条目被丢弃
            // 注:score 仅用于排序,不存入 retained
            let _ = score; // 显式丢弃,避免 unused 警告
        }

        // 若 retained 为空(所有条目都 > effective_target),保留最高分 1 个条目
        // WHY:避免压缩后上下文为空,调用方据此触发窗口升级
        if retained.is_empty() {
            if let Some(entry) = fallback_entry {
                // entry: &Arc<ContextEntry>(fallback_entry 为 Option<&Arc<ContextEntry>>)
                // token_size 通过 Arc Deref 访问
                compressed_size = entry.token_size;
                retained.push(Arc::clone(entry));
            }
        }

        let retained_count = retained.len();
        let dropped_count = original_count - retained_count;

        // 压缩比 = original / compressed(>1.0,越大压缩越多)
        // WHY(SubTask 14.6):compressed_size == 0 时返回 f32::MAX(非 INFINITY,避免序列化失败)。
        // 实际上 compress 函数中 fallback 逻辑确保 compressed_size > 0(至少保留 1 个条目),
        // 此处的 f32::MAX 分支为防御性处理,保持与 apply_sparse_mask 的一致性。
        let compression_ratio = if compressed_size > 0 {
            original_size as f32 / compressed_size as f32
        } else {
            f32::MAX
        };

        CompressionReport {
            original_size,
            compressed_size,
            compression_ratio,
            original_count,
            retained_count,
            dropped_count,
            retained_entries: retained,
            algorithm: "importance-top-n".into(),
        }
    }

    /// 计算单个条目的重要性评分
    ///
    /// 公式:`score = w1 × recency + w2 × frequency + w3 × relevance`
    /// (权重由 `HcwConfig.selector_policy` 注入,P3-W10.3 D1 修复)
    ///
    /// - `recency`(时近性):1.0 - (Δt / time_span),最新的为 1.0,最旧的为 0.0
    /// - `frequency`(频次):access_count / max_access_count,最高频为 1.0
    /// - `relevance`(任务相关性):CLV 余弦相似度 clamp 到 [0.0, 1.0],
    ///   无 CLV 时取 0.5(中性)
    ///
    /// WHY 委托 `compute_importance_score` (P4-W13.3.2 重构):
    /// 公式已提取为 `pub(crate)` 自由函数,供 `SelectorLearnerHolder` 共享,
    /// 避免运行时策略路径与配置时策略路径公式漂移。此处仅从 `HcwConfig`
    /// 读取 `SelectorPolicy::weights()` 后委托调用。
    fn compute_importance(
        entry: &ContextEntry,
        config: &HcwConfig,
        task_clv: Option<&CLV>,
        now: DateTime<Utc>,
        max_access_count: f32,
        time_span_ms: f32,
    ) -> f32 {
        // P3-W10.3 D1 修复:从注入式 SelectorPolicy 获取权重(取代原硬编码 compressor_weights)
        // WHY:Static 变体 = 编译进二进制的常量(fallback);Learned 变体 = omega-learner 异步下发值
        let weights = config.selector_policy.weights();
        compute_importance_score(
            entry,
            weights,
            task_clv,
            now,
            max_access_count,
            time_span_ms,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(id: &str, token_size: usize, access_count: u32, age_ms: i64) -> ContextEntry {
        let mut entry = ContextEntry::new(id, "file-1", format!("content-{id}"), token_size);
        entry.access_count = access_count;
        entry.last_accessed_at = Utc::now() - chrono::Duration::milliseconds(age_ms);
        entry
    }

    /// 测试辅助:将 `Vec<ContextEntry>` 转为 `Vec<Arc<ContextEntry>>`
    ///
    /// WHY(M-01/M-02):compress 签名改为 `&[Arc<ContextEntry>]`,
    /// 测试需用 Arc 包装条目。此辅助函数避免每个测试重复 `.map(Arc::new)`。
    fn to_arc(entries: Vec<ContextEntry>) -> Vec<Arc<ContextEntry>> {
        entries.into_iter().map(Arc::new).collect()
    }

    #[test]
    fn test_compress_no_compression_needed() {
        let entries = to_arc(vec![make_entry("e-1", 100, 1, 0)]);
        let report =
            ContextCompressor::compress(&HcwConfig::default(), &entries, 200, None, Utc::now());
        assert_eq!(report.original_size, 100);
        assert_eq!(report.compressed_size, 100);
        assert!((report.compression_ratio - 1.0).abs() < 1e-6);
        assert_eq!(report.dropped_count, 0);
        assert_eq!(report.algorithm, "none");
    }

    #[test]
    fn test_compress_empty_entries() {
        let entries: Vec<Arc<ContextEntry>> = Vec::new();
        let report =
            ContextCompressor::compress(&HcwConfig::default(), &entries, 100, None, Utc::now());
        assert_eq!(report.original_size, 0);
        assert_eq!(report.compressed_size, 0);
        assert_eq!(report.retained_count, 0);
    }

    #[test]
    fn test_compress_basic_top_n() {
        // 5 个条目,每个 100 token,目标 300 → 保留 3 个
        let entries = to_arc(vec![
            make_entry("e-1", 100, 1, 0),
            make_entry("e-2", 100, 2, 10),
            make_entry("e-3", 100, 3, 20),
            make_entry("e-4", 100, 4, 30),
            make_entry("e-5", 100, 5, 40),
        ]);
        let report =
            ContextCompressor::compress(&HcwConfig::default(), &entries, 300, None, Utc::now());
        assert_eq!(report.original_size, 500);
        assert_eq!(report.compressed_size, 300);
        assert!((report.compression_ratio - 5.0 / 3.0).abs() < 1e-6);
        assert_eq!(report.retained_count, 3);
        assert_eq!(report.dropped_count, 2);
        assert_eq!(report.algorithm, "importance-top-n");
    }

    #[test]
    fn test_compress_100k_to_32k_ratio_above_3() {
        // 100 个条目,每个 1000 token = 100K,目标 32K → 保留 32 个
        let entries: Vec<Arc<ContextEntry>> = to_arc(
            (0..100)
                .map(|i| make_entry(&format!("e-{i}"), 1000, (i % 10) as u32, i as i64 * 10))
                .collect(),
        );
        let report =
            ContextCompressor::compress(&HcwConfig::default(), &entries, 32000, None, Utc::now());
        assert_eq!(report.original_size, 100_000);
        assert!(report.compressed_size <= 32_000);
        // 压缩率 > 3×(compression_ratio = original/compressed > 3.0)
        assert!(
            report.compression_ratio > 3.0,
            "压缩率应 > 3.0,实际 {}",
            report.compression_ratio
        );
    }

    #[test]
    fn test_compress_128k_to_32k_ratio_above_4() {
        // 129 个条目,每个 1000 token = 129K,目标 32K,保留 32 个 = 32K
        let entries: Vec<Arc<ContextEntry>> = to_arc(
            (0..129)
                .map(|i| make_entry(&format!("e-{i}"), 1000, (i % 10) as u32, i as i64 * 10))
                .collect(),
        );
        let report =
            ContextCompressor::compress(&HcwConfig::default(), &entries, 32000, None, Utc::now());
        assert_eq!(report.original_size, 129_000);
        assert!(report.compressed_size <= 32_000);
        // 端到端压缩率 > 4×(compression_ratio = original/compressed > 4.0)
        assert!(
            report.compression_ratio > 4.0,
            "端到端压缩率应 > 4.0,实际 {}",
            report.compression_ratio
        );
    }

    #[test]
    fn test_compress_preserves_high_importance() {
        // 高频条目应被保留,低频条目应被丢弃
        let entries = to_arc(vec![
            make_entry("low-freq", 100, 0, 100),  // 低频,旧
            make_entry("high-freq", 100, 100, 0), // 高频,新
            make_entry("mid-freq", 100, 50, 50),  // 中频,中
            make_entry("low-freq-2", 100, 1, 90), // 低频,较旧
        ]);
        let report =
            ContextCompressor::compress(&HcwConfig::default(), &entries, 200, None, Utc::now());
        // 保留 2 个,应为 high-freq 与 mid-freq
        // WHY(M-01/M-02):retained_entries 是 Vec<Arc<ContextEntry>>,
        // .iter() 产生 &Arc<ContextEntry>,.id 通过 Deref 访问
        let retained_ids: Vec<&str> = report
            .retained_entries
            .iter()
            .map(|e| e.id.as_str())
            .collect();
        assert!(retained_ids.contains(&"high-freq"));
        assert!(retained_ids.contains(&"mid-freq"));
    }

    #[test]
    fn test_compress_with_clv_relevance() {
        let mut task_clv_vec = vec![1.0_f32; CLV::DIMENSION];
        task_clv_vec[0] = 2.0;
        let task_clv = CLV::from_vec(task_clv_vec).unwrap();

        // 构造两个条目:一个 CLV 与 task 相似,一个不相似
        let mut similar_clv = vec![1.0_f32; CLV::DIMENSION];
        similar_clv[0] = 1.9;
        let mut dissimilar_clv = vec![0.0_f32; CLV::DIMENSION];
        dissimilar_clv[256] = 1.0;

        let mut e_similar = make_entry("similar", 100, 1, 0);
        e_similar.clv = Some(CLV::from_vec(similar_clv).unwrap());
        let mut e_dissimilar = make_entry("dissimilar", 100, 1, 0);
        e_dissimilar.clv = Some(CLV::from_vec(dissimilar_clv).unwrap());

        let entries = to_arc(vec![e_dissimilar, e_similar]);
        let report = ContextCompressor::compress(
            &HcwConfig::default(),
            &entries,
            100,
            Some(&task_clv),
            Utc::now(),
        );
        // 保留 1 个,应为 similar(CLV 相似性更高)
        assert_eq!(report.retained_count, 1);
        assert_eq!(report.retained_entries[0].id, "similar");
    }

    #[test]
    fn test_compress_target_zero_preserves_one() {
        // target_size = 0,应保留至少 1 个条目(避免空上下文)
        let entries = to_arc(vec![make_entry("e-1", 100, 1, 0)]);
        let report =
            ContextCompressor::compress(&HcwConfig::default(), &entries, 0, None, Utc::now());
        // effective_target = 1,但条目 token_size = 100 > 1,retained 为空
        // fallback 逻辑保留最高分 1 个条目,compressed_size = 100(> target)
        // WHY:避免压缩后上下文为空,调用方据此触发窗口升级
        assert_eq!(report.retained_count, 1);
        assert_eq!(report.compressed_size, 100);
    }

    // ============================================================
    // P3-W10.3 D1 修复验收测试 — compressor 使用注入式 SelectorPolicy
    // ============================================================

    #[test]
    fn test_d1_compressor_uses_injected_static_weights() {
        // P3-W10.3:验证 compressor 使用注入的 Static 权重(而非硬编码)
        // 构造 3 个条目:高频旧 / 低频新 / 中频中
        // 权重 A (0.4, 0.3, 0.3) 默认均衡 → 保留结果
        // 权重 B (0.0, 1.0, 0.0) 纯频次 → 应优先保留高频条目
        use nexus_contracts::{SelectorPolicy, SelectorWeights};

        let entries = to_arc(vec![
            make_entry("high-freq-old", 100, 100, 200), // 高频,最旧
            make_entry("low-freq-new", 100, 1, 0),      // 低频,最新
            make_entry("mid-freq-mid", 100, 50, 100),   // 中频,中等
        ]);

        // 权重 A:默认均衡 (0.4 recency + 0.3 frequency + 0.3 relevance)
        let config_a = HcwConfig::default(); // Static(0.4, 0.3, 0.3)
        let report_a = ContextCompressor::compress(&config_a, &entries, 100, None, Utc::now());

        // 权重 B:纯频次 (0.0 recency + 1.0 frequency + 0.0 relevance)
        let config_b = HcwConfig::default().with_selector_policy(SelectorPolicy::static_policy(
            SelectorWeights::new(0.0, 1.0, 0.0),
        ));
        let report_b = ContextCompressor::compress(&config_b, &entries, 100, None, Utc::now());

        // 两者都保留 1 个条目(目标 100,每个条目 100)
        assert_eq!(report_a.retained_count, 1);
        assert_eq!(report_b.retained_count, 1);

        // 权重 B(纯频次)应保留 high-freq-old(access_count=100 最高)
        let retained_b = &report_b.retained_entries[0].id;
        assert_eq!(retained_b, "high-freq-old", "纯频次权重应保留高频条目");

        // 权重 A(均衡)与权重 B(纯频次)保留结果应不同
        // WHY:不同权重产生不同评分,证明权重已注入而非硬编码
        let retained_a = &report_a.retained_entries[0].id;
        // 注:权重 A 下 retained_a 可能是 high-freq-old 或 low-freq-new(取决于时近性 vs 频次权衡)
        // 关键验证:权重 B 确定性地保留 high-freq-old(纯频次权重下频次唯一决定评分)
        // 且权重 A 与权重 B 的保留结果不同(证明权重注入生效)
        assert_ne!(
            retained_a, retained_b,
            "不同权重应产生不同保留结果(D1 修复:权重已注入)"
        );
    }

    #[test]
    fn test_d1_compressor_uses_injected_learned_weights() {
        // P3-W10.3:验证 compressor 使用注入的 Learned 权重(omega-learner 异步下发)
        use nexus_contracts::{SelectorPolicy, SelectorWeights};

        let entries = to_arc(vec![
            make_entry("high-freq-old", 100, 100, 200), // 高频,最旧
            make_entry("low-freq-new", 100, 1, 0),      // 低频,最新
        ]);

        // Learned 策略:版本 42,纯频次权重
        let learned_policy = SelectorPolicy::learned(42, SelectorWeights::new(0.0, 1.0, 0.0));
        let config = HcwConfig::default().with_selector_policy(learned_policy);

        let report = ContextCompressor::compress(&config, &entries, 100, None, Utc::now());

        // 纯频次权重应保留 high-freq-old(access_count=100 > 1)
        assert_eq!(report.retained_count, 1);
        assert_eq!(
            report.retained_entries[0].id, "high-freq-old",
            "Learned 策略(纯频次)应保留高频条目"
        );

        // 验证 config 使用的是 Learned 策略
        assert!(config.selector_policy.is_learned());
        assert_eq!(config.selector_policy.version(), Some(42));
    }

    #[test]
    fn test_d1_compressor_default_fallback_matches_original_behavior() {
        // P3-W10.3:验证默认 fallback(Static 0.4, 0.3, 0.3)与原硬编码行为一致
        // 这是向后兼容性验证:默认 config 产生的压缩结果应与 D1 修复前一致
        let entries = to_arc(vec![
            make_entry("e-1", 100, 1, 0),
            make_entry("e-2", 100, 2, 10),
            make_entry("e-3", 100, 3, 20),
            make_entry("e-4", 100, 4, 30),
            make_entry("e-5", 100, 5, 40),
        ]);

        let config = HcwConfig::default();
        // 验证默认 selector_policy = Static(0.4, 0.3, 0.3)
        assert!(config.selector_policy.is_static());
        let w = config.selector_policy.weights();
        assert!((w.recency - 0.4).abs() < 1e-6);
        assert!((w.frequency - 0.3).abs() < 1e-6);
        assert!((w.relevance - 0.3).abs() < 1e-6);

        let report = ContextCompressor::compress(&config, &entries, 300, None, Utc::now());

        // 与 test_compress_basic_top_n 一致:保留 3 个,压缩率 5/3
        assert_eq!(report.original_size, 500);
        assert_eq!(report.compressed_size, 300);
        assert!((report.compression_ratio - 5.0 / 3.0).abs() < 1e-6);
        assert_eq!(report.retained_count, 3);
        assert_eq!(report.dropped_count, 2);
    }

    #[test]
    fn test_d1_compressor_learner_panic_fallback_to_static() {
        // P3-W10.3:模拟 omega-learner panic 后,compressor 用 fallback Static 权重
        use nexus_contracts::{SelectorPolicy, SelectorWeights};

        let entries = to_arc(vec![
            make_entry("high-freq-old", 100, 100, 200),
            make_entry("low-freq-new", 100, 1, 0),
        ]);

        // 1. omega-learner 下发 Learned 值(纯频次)
        let learned = SelectorPolicy::learned(1, SelectorWeights::new(0.0, 1.0, 0.0));
        let config_learned = HcwConfig::default().with_selector_policy(learned);
        let report_learned =
            ContextCompressor::compress(&config_learned, &entries, 100, None, Utc::now());
        // Learned(纯频次)保留 high-freq-old
        assert_eq!(report_learned.retained_entries[0].id, "high-freq-old");

        // 2. learner panic → 调用方本地 fallback 到 Static(默认 0.4, 0.3, 0.3)
        let config_fallback = HcwConfig::default().with_selector_policy(SelectorPolicy::fallback());
        let report_fallback =
            ContextCompressor::compress(&config_fallback, &entries, 100, None, Utc::now());

        // fallback(均衡权重)保留结果可能与 Learned 不同(证明 fallback 生效,非沿用 Learned)
        // WHY:若 fallback 未生效,会沿用 Learned 的纯频次权重,保留 high-freq-old
        //     fallback 后用均衡权重,时近性权重 0.4 会使 low-freq-new(最新)评分提升
        assert_eq!(report_fallback.retained_count, 1);
        // 验证 fallback 确实切换到 Static 权重(非 Learned)
        assert!(config_fallback.selector_policy.is_static());
        assert!(!config_fallback.selector_policy.is_learned());
    }
}
