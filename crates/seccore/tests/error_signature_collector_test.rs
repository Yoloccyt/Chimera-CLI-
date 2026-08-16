//! 错误签名收集器集成测试 — 顶层 API + 哈希一致性 + L0 契约协同（v3.4.0 §9.3）
//!
//! 覆盖: 顶层 API 可达性 / 5 模式端到端提取 / SHA-256 哈希一致性 /
//! L0 ErrorSignature 契约协同 / proptest 模糊输出鲁棒性

#![forbid(unsafe_code)]

use proptest::prelude::*;
use seccore::{compute_error_hash, ErrorSignatureCollector};

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    let mut collector = ErrorSignatureCollector::new();
    let sig = collector.extract("error[E0308]: mismatched types", "src/x.rs:1");
    assert!(sig.is_some());
    let hash = compute_error_hash("CompilationError", "mismatched types");
    assert_eq!(hash.len(), 16);
}

// ----------------------------------------------------------
// 5 模式端到端提取
// ----------------------------------------------------------

#[test]
fn all_five_known_patterns_extract() {
    let cases = [
        ("error[E0308]: mismatched types", "CompilationError"),
        ("thread 'main' panicked at src/x.rs:1, boom", "RuntimePanic"),
        ("assertion failed: left == right", "AssertionFailure"),
        ("test result: FAILED. 2 failed, 3 passed", "TestFailure"),
        ("operation timeout after 3000ms", "Timeout"),
    ];
    for (output, expected_type) in cases {
        let mut collector = ErrorSignatureCollector::new();
        let sig = collector.extract(output, "loc").expect("应匹配已知模式");
        assert_eq!(sig.error_type.as_ref(), expected_type, "输出: {output}");
        assert_eq!(sig.error_hash.len(), 16, "哈希应为 16 位");
    }
}

// ----------------------------------------------------------
// SHA-256 哈希一致性（D-3: 与 L3 idx_error_hash 对齐）
// ----------------------------------------------------------

#[test]
fn hash_consistency_same_input_same_hash() {
    // 相同 (error_type, summary) 必产生相同哈希（去重基础）
    let h1 = compute_error_hash("CompilationError", "mismatched types");
    let h2 = compute_error_hash("CompilationError", "mismatched types");
    assert_eq!(h1, h2);
    // 提取的签名哈希与直接计算一致
    let mut collector = ErrorSignatureCollector::new();
    let sig = collector
        .extract("error[E0308]: mismatched types", "loc")
        .expect("匹配");
    let expected = compute_error_hash("CompilationError", sig.error_summary.as_ref());
    assert_eq!(
        sig.error_hash.as_ref(),
        expected,
        "提取哈希应与直接计算一致"
    );
}

#[test]
fn hash_is_16_hex_chars() {
    let h = compute_error_hash("RuntimePanic", "index out of bounds");
    assert_eq!(h.len(), 16);
    assert!(h.chars().all(|c| c.is_ascii_hexdigit()), "应为十六进制");
}

// ----------------------------------------------------------
// L0 ErrorSignature 契约协同
// ----------------------------------------------------------

#[test]
fn extracted_signature_is_l0_contract_type() {
    // 提取的签名是 L0 nexus_contracts::ErrorSignature 类型（可直接存入 ExperienceCard）
    let mut collector = ErrorSignatureCollector::new();
    let sig = collector
        .extract("error[E0425]: cannot find value", "src/main.rs:5")
        .expect("匹配");
    // 验证 L0 契约字段完整
    assert!(!sig.error_type.is_empty());
    assert!(!sig.error_location.is_empty());
    assert!(!sig.error_summary.is_empty());
    assert!(!sig.error_hash.is_empty());
    assert_eq!(sig.error_location.as_ref(), "src/main.rs:5");
}

// ----------------------------------------------------------
// 频率统计与聚类
// ----------------------------------------------------------

#[test]
fn frequency_clustering_across_extractions() {
    let mut collector = ErrorSignatureCollector::new();
    // 同一错误提取多次 → 聚类
    for _ in 0..5 {
        collector.extract("error[E0308]: mismatched types", "loc");
    }
    // 不同错误
    collector.extract("thread 'main' panicked at x.rs:1, oops", "loc");
    assert_eq!(collector.unique_signature_count(), 2);
    let frequent = collector.get_frequent_signatures(3);
    assert_eq!(frequent.len(), 1, "只有频率≥3 的编译错误满足");
    assert_eq!(frequent[0].1, "CompilationError");
    assert_eq!(frequent[0].2, 5);
}

// ----------------------------------------------------------
// proptest: 模糊输出鲁棒性
// ----------------------------------------------------------

proptest! {
    /// 任意输出不导致 panic（鲁棒性），且哈希恒为 16 位（若提取成功）
    #[test]
    fn extract_never_panics_on_arbitrary_output(
        output in proptest::string::string_regex(".*").unwrap(),
    ) {
        let mut collector = ErrorSignatureCollector::new();
        // 不应 panic
        let result = collector.extract(&output, "fuzz-loc");
        if let Some(sig) = result {
            prop_assert_eq!(sig.error_hash.len(), 16);
            prop_assert!(!sig.error_type.is_empty());
        }
    }

    /// 哈希确定性: 同输入恒同输出
    #[test]
    fn hash_deterministic(
        error_type in proptest::string::string_regex("[A-Za-z]{1,20}").unwrap(),
        summary in proptest::string::string_regex("[A-Za-z ]{0,50}").unwrap(),
    ) {
        let h1 = compute_error_hash(&error_type, &summary);
        let h2 = compute_error_hash(&error_type, &summary);
        prop_assert_eq!(&h1, &h2);
        prop_assert_eq!(h1.len(), 16);
    }
}
