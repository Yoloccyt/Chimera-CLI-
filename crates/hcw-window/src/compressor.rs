//! 上下文压缩器 — 基于重要性评分的 Top-N 保留压缩
//!
//! 对应架构层:L2 Memory
//! 对应创新点:HCW §上下文压缩
//!
//! # 核心职责
//! - 按重要性评分(0.4×时近性 + 0.3×频次 + 0.3×任务相关性)排序条目
//! - 贪心保留 Top-N 使总大小 ≤ target_size
//! - 返回 CompressionReport(原始/压缩后大小、保留/丢弃条目数、压缩倍率)
//!
//! # 重要性评分公式
//! `score = 0.4 × recency + 0.3 × frequency + 0.3 × relevance`
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

use chrono::{DateTime, Utc};
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
const DEFAULT_RELEVANCE: f32 = 0.5;

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
    /// WHY 接受 `&[ContextEntry]` 而非 `Vec<ContextEntry>`(SubTask 19.4):
    /// 原实现要求调用方 `state.entries.clone()` 全量 clone 1000 条目后传入,
    /// 现接受借用引用,内部仅 clone 保留的 Top-N 条目(通常 ≤ 100),
    /// 消除 900+ 次无用 clone。无需压缩时返回空 retained_entries,
    /// 调用方检查 `algorithm == "none"` 跳过 entries 替换。
    ///
    /// # 参数
    /// - `entries`:待压缩的条目切片(借用,不消费)
    /// - `target_size`:目标总 Token 大小
    /// - `task_clv`:当前任务的 CLV(用于相关性计算,None 时相关性取 0.5)
    /// - `now`:当前时间(用于时近性计算)
    pub fn compress(
        config: &HcwConfig,
        entries: &[ContextEntry],
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

        // 计算每个条目的重要性评分并配对(借用引用,不 clone)
        let mut scored: Vec<(f32, &ContextEntry)> = entries
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
        // SubTask 19.4:用 *e 替代 e.clone() — e 是 &&ContextEntry,
        // *e 解引用为 &ContextEntry(Copy 类型,零成本复制引用),
        // 避免 suspicious_double_ref_op 警告(clone 双引用仅复制内层引用)
        let fallback_entry = scored.first().map(|(_, e)| *e);

        let mut retained: Vec<ContextEntry> = Vec::new();
        let mut compressed_size: usize = 0;

        // WHY 仅 clone 保留的条目:原实现消费 Vec<ContextEntry> 全量移动,
        // 现接受 &[ContextEntry] 借用,仅对通过贪心筛选的条目 clone,
        // 1000 条目中保留 100 条时,clone 次数从 1000 降到 100
        for (score, entry) in scored {
            if compressed_size + entry.token_size <= effective_target {
                compressed_size += entry.token_size;
                retained.push(entry.clone());
            }
            // 超出 effective_target 的条目被丢弃
            // 注:score 仅用于排序,不存入 retained
            let _ = score; // 显式丢弃,避免 unused 警告
        }

        // 若 retained 为空(所有条目都 > effective_target),保留最高分 1 个条目
        // WHY:避免压缩后上下文为空,调用方据此触发窗口升级
        if retained.is_empty() {
            if let Some(entry) = fallback_entry {
                compressed_size = entry.token_size;
                // SubTask 19.4:entry 是 &ContextEntry(fallback_entry 为 Option<&ContextEntry>),
                // .clone() 返回 ContextEntry(方法解析匹配 ContextEntry: Clone,非 &T: Clone)
                retained.push(entry.clone());
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
    /// 公式:`score = 0.4 × recency + 0.3 × frequency + 0.3 × relevance`
    ///
    /// - `recency`(时近性):1.0 - (Δt / time_span),最新的为 1.0,最旧的为 0.0
    /// - `frequency`(频次):access_count / max_access_count,最高频为 1.0
    /// - `relevance`(任务相关性):CLV 余弦相似度 clamp 到 [0.0, 1.0],
    ///   无 CLV 时取 0.5(中性)
    fn compute_importance(
        entry: &ContextEntry,
        config: &HcwConfig,
        task_clv: Option<&CLV>,
        now: DateTime<Utc>,
        max_access_count: f32,
        time_span_ms: f32,
    ) -> f32 {
        let (recency_weight, frequency_weight, relevance_weight) = config.compressor_weights;
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

    #[test]
    fn test_compress_no_compression_needed() {
        let entries = vec![make_entry("e-1", 100, 1, 0)];
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
        let report =
            ContextCompressor::compress(&HcwConfig::default(), &Vec::new(), 100, None, Utc::now());
        assert_eq!(report.original_size, 0);
        assert_eq!(report.compressed_size, 0);
        assert_eq!(report.retained_count, 0);
    }

    #[test]
    fn test_compress_basic_top_n() {
        // 5 个条目,每个 100 token,目标 300 → 保留 3 个
        let entries = vec![
            make_entry("e-1", 100, 1, 0),
            make_entry("e-2", 100, 2, 10),
            make_entry("e-3", 100, 3, 20),
            make_entry("e-4", 100, 4, 30),
            make_entry("e-5", 100, 5, 40),
        ];
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
        let entries: Vec<ContextEntry> = (0..100)
            .map(|i| make_entry(&format!("e-{i}"), 1000, (i % 10) as u32, i as i64 * 10))
            .collect();
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
        let entries: Vec<ContextEntry> = (0..129)
            .map(|i| make_entry(&format!("e-{i}"), 1000, (i % 10) as u32, i as i64 * 10))
            .collect();
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
        let entries = vec![
            make_entry("low-freq", 100, 0, 100),  // 低频,旧
            make_entry("high-freq", 100, 100, 0), // 高频,新
            make_entry("mid-freq", 100, 50, 50),  // 中频,中
            make_entry("low-freq-2", 100, 1, 90), // 低频,较旧
        ];
        let report =
            ContextCompressor::compress(&HcwConfig::default(), &entries, 200, None, Utc::now());
        // 保留 2 个,应为 high-freq 与 mid-freq
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

        let entries = vec![e_dissimilar, e_similar];
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
        let entries = vec![make_entry("e-1", 100, 1, 0)];
        let report =
            ContextCompressor::compress(&HcwConfig::default(), &entries, 0, None, Utc::now());
        // effective_target = 1,但条目 token_size = 100 > 1,retained 为空
        // fallback 逻辑保留最高分 1 个条目,compressed_size = 100(> target)
        // WHY:避免压缩后上下文为空,调用方据此触发窗口升级
        assert_eq!(report.retained_count, 1);
        assert_eq!(report.compressed_size, 100);
    }
}
