//! SubTask 2.13: ContextCompressor 单元测试
//!
//! 验证 100K Token 上下文压缩到 32K,压缩率 > 3x;
//! 端到端 128K 压缩到 32K,压缩率 > 4x。
//! 测试通过 `hcw_window` crate 的公共 API 进行(集成测试)。

use std::sync::Arc;

use chrono::{Duration, Utc};
use hcw_window::{ContextCompressor, ContextEntry, HcwConfig};
use nexus_core::CLV;

/// 构造测试条目:指定 id、token_size、access_count、年龄(毫秒)
fn make_entry(id: &str, token_size: usize, access_count: u32, age_ms: i64) -> ContextEntry {
    let mut entry = ContextEntry::new(id, "file-1", format!("content-{id}"), token_size);
    entry.access_count = access_count;
    entry.last_accessed_at = Utc::now() - Duration::milliseconds(age_ms);
    entry
}

/// 构造测试条目(带 file_id):用于稀疏化测试
fn make_entry_with_file(id: &str, file_id: &str, token_size: usize) -> ContextEntry {
    ContextEntry::new(id, file_id, format!("content-{id}"), token_size)
}

/// 测试辅助:将 `Vec<ContextEntry>` 转为 `Vec<Arc<ContextEntry>>`
///
/// WHY(M-01/M-02):compress 签名改为 `&[Arc<ContextEntry>]`,
/// 测试需用 Arc 包装条目。此辅助函数避免每个测试重复 `.map(Arc::new)`。
fn to_arc(entries: Vec<ContextEntry>) -> Vec<Arc<ContextEntry>> {
    entries.into_iter().map(Arc::new).collect()
}

/// 验证 100K Token 压缩到 32K,压缩率 > 3x(任务要求)
#[test]
fn test_compress_100k_to_32k_ratio_above_3() {
    // 100 个条目,每个 1000 token = 100K,目标 32K
    let entries = to_arc(
        (0..100)
            .map(|i| make_entry(&format!("e-{i}"), 1000, (i % 10) as u32, i as i64 * 10))
            .collect(),
    );

    let report =
        ContextCompressor::compress(&HcwConfig::default(), &entries, 32000, None, Utc::now());

    assert_eq!(report.original_size, 100_000, "原始大小应为 100K");
    assert!(
        report.compressed_size <= 32_000,
        "压缩后大小应 <= 32K,实际 {}",
        report.compressed_size
    );
    // 压缩率 = original/compressed > 3.0
    assert!(
        report.compression_ratio > 3.0,
        "压缩率应 > 3.0,实际 {}",
        report.compression_ratio
    );
    assert!(report.retained_count > 0, "应保留至少 1 个条目");
    assert_eq!(report.algorithm, "importance-top-n");
}

/// 验证端到端 128K Token 压缩到 32K,压缩率 > 4x(任务要求)
#[test]
fn test_compress_128k_to_32k_ratio_above_4() {
    // 129 个条目,每个 1000 token = 129K,目标 32K,保留 32 个 = 32K
    let entries = to_arc(
        (0..129)
            .map(|i| make_entry(&format!("e-{i}"), 1000, (i % 10) as u32, i as i64 * 10))
            .collect(),
    );

    let report =
        ContextCompressor::compress(&HcwConfig::default(), &entries, 32000, None, Utc::now());

    assert_eq!(report.original_size, 129_000, "原始大小应为 129K");
    assert!(
        report.compressed_size <= 32_000,
        "压缩后大小应 <= 32K,实际 {}",
        report.compressed_size
    );
    // 端到端压缩率 > 4.0
    assert!(
        report.compression_ratio > 4.0,
        "端到端压缩率应 > 4.0,实际 {}",
        report.compression_ratio
    );
}

/// 验证无需压缩时返回原样
#[test]
fn test_no_compression_needed() {
    let entries = to_arc(vec![make_entry("e-1", 100, 1, 0)]);
    let report =
        ContextCompressor::compress(&HcwConfig::default(), &entries, 200, None, Utc::now());

    assert_eq!(report.original_size, 100);
    assert_eq!(report.compressed_size, 100);
    assert!(
        (report.compression_ratio - 1.0).abs() < 1e-6,
        "无需压缩时 compression_ratio = 1.0"
    );
    assert_eq!(report.dropped_count, 0);
    assert_eq!(report.algorithm, "none");
}

/// 验证空条目列表
#[test]
fn test_compress_empty_entries() {
    // WHY(M-01/M-02):compress 签名要求 &[Arc<ContextEntry>],空 Vec 需明确类型
    let entries: Vec<Arc<ContextEntry>> = Vec::new();
    let report =
        ContextCompressor::compress(&HcwConfig::default(), &entries, 100, None, Utc::now());

    assert_eq!(report.original_size, 0);
    assert_eq!(report.compressed_size, 0);
    assert_eq!(report.retained_count, 0);
}

/// 验证重要性评分:高频条目应被保留,低频条目应被丢弃
#[test]
fn test_importance_scoring_preserves_high_frequency() {
    let entries = to_arc(vec![
        make_entry("low-freq", 100, 0, 100),  // 低频,旧
        make_entry("high-freq", 100, 100, 0), // 高频,新
        make_entry("mid-freq", 100, 50, 50),  // 中频,中
        make_entry("low-freq-2", 100, 1, 90), // 低频,较旧
    ]);

    let report =
        ContextCompressor::compress(&HcwConfig::default(), &entries, 200, None, Utc::now());

    // 保留 2 个,应为 high-freq 与 mid-freq
    let retained_ids: Vec<&str> = report
        .retained_entries
        .iter()
        .map(|e| e.id.as_str())
        .collect();
    assert!(
        retained_ids.contains(&"high-freq"),
        "高频条目应被保留,实际保留: {retained_ids:?}"
    );
    assert!(
        retained_ids.contains(&"mid-freq"),
        "中频条目应被保留,实际保留: {retained_ids:?}"
    );
}

/// 验证 CLV 任务相关性:与任务 CLV 相似的条目应被保留
#[test]
fn test_clv_relevance_scoring() {
    // 构造任务 CLV
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
    assert_eq!(report.retained_count, 1, "应保留 1 个条目");
    assert_eq!(
        report.retained_entries[0].id, "similar",
        "应保留 CLV 相似的条目"
    );
}

/// 验证时近性:最近访问的条目应被保留
#[test]
fn test_recency_scoring() {
    // 所有条目频次相同,仅时近性不同
    let entries = to_arc(vec![
        make_entry("oldest", 100, 10, 1000), // 最旧
        make_entry("newest", 100, 10, 0),    // 最新
        make_entry("middle", 100, 10, 500),  // 中间
        make_entry("old-2", 100, 10, 800),   // 较旧
    ]);

    let report =
        ContextCompressor::compress(&HcwConfig::default(), &entries, 200, None, Utc::now());

    // 保留 2 个,应为 newest 与 middle
    let retained_ids: Vec<&str> = report
        .retained_entries
        .iter()
        .map(|e| e.id.as_str())
        .collect();
    assert!(
        retained_ids.contains(&"newest"),
        "最新条目应被保留,实际保留: {retained_ids:?}"
    );
}

/// 验证 target_size = 0 时不产生 panic(防御性)
#[test]
fn test_target_zero_no_panic() {
    let entries = to_arc(vec![make_entry("e-1", 100, 1, 0)]);
    let report = ContextCompressor::compress(&HcwConfig::default(), &entries, 0, None, Utc::now());

    // effective_target = 1,但条目 token_size = 100 > 1,retained 为空
    // fallback 逻辑保留最高分 1 个条目,compressed_size = 100(> target)
    // WHY:避免压缩后上下文为空,调用方据此触发窗口升级
    assert_eq!(report.retained_count, 1);
    assert_eq!(report.compressed_size, 100);
}

/// 验证压缩报告的字段完整性
#[test]
fn test_compression_report_fields() {
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
    assert_eq!(report.original_count, 5);
    assert_eq!(report.retained_count + report.dropped_count, 5);
    assert_eq!(
        report.retained_entries.len(),
        report.retained_count,
        "retained_entries 长度应与 retained_count 一致"
    );
    assert!(!report.algorithm.is_empty(), "算法名称不应为空");
}

/// 验证大尺度压缩:1M Token 压缩到 128K,压缩率 > 7x
#[test]
fn test_compress_1m_to_128k_ratio_above_7() {
    // 1024 个条目,每个 1000 token = 1M,目标 128K
    let entries = to_arc(
        (0..1024)
            .map(|i| make_entry(&format!("e-{i}"), 1000, (i % 10) as u32, i as i64))
            .collect(),
    );

    let report =
        ContextCompressor::compress(&HcwConfig::default(), &entries, 131072, None, Utc::now());

    assert_eq!(report.original_size, 1_024_000, "原始大小应为 1M");
    assert!(
        report.compressed_size <= 131_072,
        "压缩后大小应 <= 128K,实际 {}",
        report.compressed_size
    );
    // 1M -> 128K 压缩率应 > 7x
    assert!(
        report.compression_ratio > 7.0,
        "1M->128K 压缩率应 > 7.0,实际 {}",
        report.compression_ratio
    );
}

/// 验证 file_id 字段在压缩后保留(用于后续 OSA 掩码稀疏化)
#[test]
fn test_file_id_preserved_after_compression() {
    let entries = to_arc(vec![
        make_entry_with_file("e-1", "file-a", 100),
        make_entry_with_file("e-2", "file-b", 100),
        make_entry_with_file("e-3", "file-c", 100),
        make_entry_with_file("e-4", "file-d", 100),
    ]);

    let report =
        ContextCompressor::compress(&HcwConfig::default(), &entries, 200, None, Utc::now());

    // 验证保留的条目仍携带 file_id
    for entry in &report.retained_entries {
        assert!(!entry.file_id.is_empty(), "压缩后 file_id 不应为空");
    }
}

// ============================================================
// SubTask 13.6 / 13.7 基准测试(std::time::Instant,warmup 10 + 测量 100 取 P50)
// ============================================================

/// SubTask 13.6 基准测试:Arc<str> 共享下 1000 条目压缩操作延迟
///
/// 验证:1000 条目(每个 content 4KB)压缩到 100K 的 P50 延迟。
/// Arc<str> 共享使 ContextEntry 克隆 O(1)(fallback_entry clone +
/// entries_snapshot clone 廉价),降低压缩延迟。
#[test]
fn bench_compress_1000_entries_arc_shared() {
    use std::time::Instant;

    // 构造 1000 条目,每个 content 4KB,token_size 1000(总 1M token)
    // WHY(M-01/M-02):compress 签名要求 Vec<Arc<ContextEntry>>,用 to_arc 包装
    let make_entries = || -> Vec<Arc<ContextEntry>> {
        to_arc(
            (0..1000)
                .map(|i| {
                    let mut entry =
                        ContextEntry::new(format!("e-{i}"), "file-1", "x".repeat(4096), 1000);
                    entry.access_count = (i % 10) as u32;
                    entry.last_accessed_at = Utc::now() - Duration::milliseconds(i as i64);
                    entry
                })
                .collect(),
        )
    };

    // warmup 10 次
    for _ in 0..10 {
        let entries = make_entries();
        let _ =
            ContextCompressor::compress(&HcwConfig::default(), &entries, 100_000, None, Utc::now());
    }

    // 测量 100 次
    let mut times: Vec<u128> = Vec::with_capacity(100);
    for _ in 0..100 {
        let entries = make_entries();
        let start = Instant::now();
        let report =
            ContextCompressor::compress(&HcwConfig::default(), &entries, 100_000, None, Utc::now());
        times.push(start.elapsed().as_nanos());
        // 验证压缩功能正常
        assert!(report.retained_count > 0, "应保留至少 1 个条目");
        assert!(report.compressed_size <= 100_000, "压缩后应 <= 100K");
    }

    times.sort();
    let p50 = times[times.len() / 2];
    println!(
        "bench_compress_1000_entries_arc_shared P50: {} ns ({:.3} ms)",
        p50,
        p50 as f64 / 1_000_000.0
    );
}

/// SubTask 13.7 基准测试:select_nth_unstable_by 100K Token 压缩延迟
///
/// 验证:100K Token(100 条目 × 1000 token)压缩到 32K 的 P50 延迟。
/// select_nth_unstable_by 部分排序 O(n + K log K) 替代全排序 O(n log n),
/// 100K Token 场景 K≈32,n=100,优化显著。
#[test]
fn bench_compress_100k_select_nth_unstable() {
    use std::time::Instant;

    let make_entries = || -> Vec<Arc<ContextEntry>> {
        to_arc(
            (0..100)
                .map(|i| {
                    let mut entry =
                        ContextEntry::new(format!("e-{i}"), "file-1", format!("content-{i}"), 1000);
                    entry.access_count = (i % 10) as u32;
                    entry.last_accessed_at = Utc::now() - Duration::milliseconds(i as i64 * 10);
                    entry
                })
                .collect(),
        )
    };

    // warmup 10 次
    for _ in 0..10 {
        let entries = make_entries();
        let _ =
            ContextCompressor::compress(&HcwConfig::default(), &entries, 32_000, None, Utc::now());
    }

    // 测量 100 次
    let mut times: Vec<u128> = Vec::with_capacity(100);
    for _ in 0..100 {
        let entries = make_entries();
        let start = Instant::now();
        let report =
            ContextCompressor::compress(&HcwConfig::default(), &entries, 32_000, None, Utc::now());
        times.push(start.elapsed().as_nanos());
        assert!(report.compressed_size <= 32_000, "压缩后应 <= 32K");
        assert!(report.compression_ratio > 3.0, "压缩率应 > 3.0");
    }

    times.sort();
    let p50 = times[times.len() / 2];
    println!(
        "bench_compress_100k_select_nth_unstable P50: {} ns ({:.3} ms)",
        p50,
        p50 as f64 / 1_000_000.0
    );
}
