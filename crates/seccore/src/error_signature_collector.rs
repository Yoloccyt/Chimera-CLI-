//! 错误签名收集器 — OpenMLE 结构化收集 + 哈希去重聚类（设计文档 §9.3）
//!
//! 对应架构层: **L4 Security**（seccore 子模块）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §9.3
//! 对应论文: 清华 OpenMLE（错误签名 + 结构化收集 + 哈希去重聚类）
//! 对应 ADR: ADR-049 决策 1（error-signature-collector 落点 seccore，内嵌模块）
//!
//! # 核心职责
//!
//! 从执行输出中提取结构化错误签名（铁律7），支持哈希去重与聚类：
//! - **5 个已知模式正则**: CompilationError / RuntimePanic / AssertionFailure /
//!   TestFailure / Timeout
//! - **通用回退**: error/failed/panic 关键词命中
//! - **SHA-256 哈希**: `compute_error_hash` 取前 16 位十六进制（补 Phase 0 L0 遗留，
//!   与 L3 `idx_error_hash` 对齐）
//! - **频率统计**: signature_frequency + error_type_frequency 聚类
//!
//! # 设计约束（铁律）
//!
//! - **铁律7**: 错误签名结构化收集，支持哈希去重和聚类
//! - **D-3**: 哈希计算用 sha2（seccore 已有，复用 audit.rs 模式），与 L3 idx_error_hash
//!   对齐（同为 SHA-256 前 16 位十六进制）
//! - **纯函数提取**: extract 不修改输入输出，仅更新内部频率统计

use std::collections::HashMap;

use nexus_contracts::experience_card::ErrorSignature;
use regex::Regex;
use sha2::{Digest, Sha256};

/// 错误摘要最大长度（截取前 100 字符，避免过长签名）
const SUMMARY_MAX_LEN: usize = 100;

/// 计算错误签名哈希 — SHA-256 前 16 位十六进制（D-3，补 Phase 0 L0 遗留）
///
/// 哈希内容: `error_type || \x00 || summary`（分隔符防字段拼接歧义）。
/// 与 L3 `ExperienceCardStorage` 的 `idx_error_hash` 索引对齐（同为前 16 位）。
///
/// # 参数
/// - `error_type`: 错误类型（如 "CompilationError"）
/// - `summary`: 错误摘要（已截取前 100 字符）
///
/// # 返回
/// SHA-256 哈希的前 16 位十六进制字符串（32 字符 SHA-256 的前 16 字符）
pub fn compute_error_hash(error_type: &str, summary: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(error_type.as_bytes());
    hasher.update(b"\x00");
    hasher.update(summary.as_bytes());
    hex::encode(hasher.finalize())[..16].to_string()
}

/// 错误签名收集器 — 结构化提取 + 哈希去重聚类
///
/// 维护已知模式正则与频率统计；`extract` 提取签名并更新频率。
#[derive(Debug)]
pub struct ErrorSignatureCollector {
    /// 已知模式正则: (正则, error_type)
    known_patterns: Vec<(Regex, String)>,
    /// 签名频率: hash → count
    signature_frequency: HashMap<String, u32>,
    /// 签名 → 错误类型映射（get_frequent_signatures 用）
    signature_to_type: HashMap<String, String>,
    /// 错误类型频率: error_type → count
    error_type_frequency: HashMap<String, u32>,
}

impl ErrorSignatureCollector {
    /// 创建收集器 — 注册 5 个已知模式正则
    pub fn new() -> Self {
        // 5 个已知模式正则: (正则, error_type)
        let patterns: Vec<(Regex, String)> = vec![
            // 1. 编译错误: error[E0308]: mismatched types
            (
                Regex::new(r"error\[(?P<type>E\d+)\]:\s*(?P<summary>.+)").expect("正则编译失败"),
                "CompilationError".to_string(),
            ),
            // 2. 运行时 panic: thread 'main' panicked at src/x.rs:42, message
            (
                Regex::new(r"thread '\w+' panicked at (?P<location>.+?),\s*(?P<summary>.+)")
                    .expect("正则编译失败"),
                "RuntimePanic".to_string(),
            ),
            // 3. 断言失败: assertion failed: xxx
            (
                Regex::new(r"assertion failed:\s*(?P<summary>.+)").expect("正则编译失败"),
                "AssertionFailure".to_string(),
            ),
            // 4. 测试失败: test result: FAILED. 2 failed, 3 passed
            (
                Regex::new(r"test result: FAILED\.\s*(?P<summary>\d+ failed, \d+ passed)")
                    .expect("正则编译失败"),
                "TestFailure".to_string(),
            ),
            // 5. 超时: timeout after 5000ms
            (
                Regex::new(r"timeout after (?P<duration>\d+ms)").expect("正则编译失败"),
                "Timeout".to_string(),
            ),
        ];
        Self {
            known_patterns: patterns,
            signature_frequency: HashMap::new(),
            signature_to_type: HashMap::new(),
            error_type_frequency: HashMap::new(),
        }
    }

    /// 提取错误签名 — 模式匹配 → 哈希 → 更新频率
    ///
    /// # 参数
    /// - `output`: 执行输出（stdout/stderr）
    /// - `location`: 错误位置（外部传入，如 "src/lib.rs:42"）
    ///
    /// # 返回
    /// - `Some(ErrorSignature)`: 匹配到已知模式或通用回退
    /// - `None`: 无错误信号
    pub fn extract(&mut self, output: &str, location: &str) -> Option<ErrorSignature> {
        // 已知模式匹配——先收集匹配结果(error_type + summary)再更新频率，
        // 避免遍历 &self.known_patterns 的不可变借用与 update_frequency 可变借用冲突
        let mut matched: Option<(String, String)> = None;
        for (pattern, error_type) in &self.known_patterns {
            if let Some(captures) = pattern.captures(output) {
                let summary = captures
                    .name("summary")
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_else(|| output.lines().next().unwrap_or("").to_string());
                matched = Some((error_type.clone(), truncate_summary(&summary)));
                break;
            }
        }
        if let Some((error_type, summary)) = matched {
            let hash = compute_error_hash(&error_type, &summary);
            self.update_frequency(&hash, &error_type);
            return Some(ErrorSignature {
                error_type: error_type.into_boxed_str(),
                error_location: location.to_string().into_boxed_str(),
                error_summary: summary.into_boxed_str(),
                error_hash: hash.into_boxed_str(),
            });
        }
        // 通用回退: error/failed/panic 关键词
        let keywords = [
            ("error", "GenericError"),
            ("Error", "GenericError"),
            ("ERROR", "GenericError"),
            ("failed", "GenericFailure"),
            ("Failed", "GenericFailure"),
            ("panic", "GenericPanic"),
            ("Panic", "GenericPanic"),
        ];
        for (keyword, error_type) in &keywords {
            if output.contains(keyword) {
                let first_line = output.lines().next().unwrap_or("");
                let summary = truncate_summary(first_line);
                let hash = compute_error_hash(error_type, &summary);
                self.update_frequency(&hash, error_type);
                return Some(ErrorSignature {
                    error_type: error_type.to_string().into_boxed_str(),
                    error_location: location.to_string().into_boxed_str(),
                    error_summary: summary.into_boxed_str(),
                    error_hash: hash.into_boxed_str(),
                });
            }
        }
        None
    }

    /// 更新频率统计（签名 + 错误类型）
    fn update_frequency(&mut self, hash: &str, error_type: &str) {
        *self
            .signature_frequency
            .entry(hash.to_string())
            .or_insert(0) += 1;
        self.signature_to_type
            .entry(hash.to_string())
            .or_insert_with(|| error_type.to_string());
        *self
            .error_type_frequency
            .entry(error_type.to_string())
            .or_insert(0) += 1;
    }

    /// 获取高频签名 — (hash, error_type, count)，按频率阈值过滤
    pub fn get_frequent_signatures(&self, threshold: u32) -> Vec<(String, String, u32)> {
        self.signature_frequency
            .iter()
            .filter(|(_, count)| **count >= threshold)
            .map(|(hash, count)| {
                let error_type = self
                    .signature_to_type
                    .get(hash)
                    .cloned()
                    .unwrap_or_default();
                (hash.clone(), error_type, *count)
            })
            .collect()
    }

    /// 错误类型频率只读访问（可观测性）
    pub fn error_type_frequency(&self) -> &HashMap<String, u32> {
        &self.error_type_frequency
    }

    /// 签名总数（去重后）
    pub fn unique_signature_count(&self) -> usize {
        self.signature_frequency.len()
    }

    /// 提取并检查是否为重复签名 — §16.4 ErrorSignatureMatched 发布辅助
    ///
    /// 返回 `(Option<ErrorSignature>, is_repeat)`:
    /// - `is_repeat = true` 当且仅当该哈希此前已出现过(频率 > 1),
    ///   即 Debug 算子"相同错误签名兄弟"检索的事件化条件满足。
    /// - 调用方可据此决定是否发布 `ErrorSignatureMatched` 事件。
    pub fn extract_and_check_repeat(
        &mut self,
        output: &str,
        location: &str,
    ) -> (Option<ErrorSignature>, bool) {
        let sig = self.extract(output, location);
        let is_repeat = sig
            .as_ref()
            .map(|s| {
                self.signature_frequency
                    .get(s.error_hash.as_ref())
                    .copied()
                    .unwrap_or(0)
                    > 1
            })
            .unwrap_or(false);
        (sig, is_repeat)
    }

    /// §16.4 事件发布辅助:提取签名并在重复时发布 ErrorSignatureMatched
    ///
    /// 组合根调用入口:将 ErrorSignatureCollector + EventBus 桥接,
    /// 避免调用方手动判断 is_repeat + 构造事件 + 发布。
    /// 返回提取到的签名(无论是否重复)。
    pub fn extract_and_publish(
        &mut self,
        bus: &event_bus::EventBus,
        output: &str,
        location: &str,
    ) -> Option<ErrorSignature> {
        let (sig, is_repeat) = self.extract_and_check_repeat(output, location);
        if is_repeat {
            if let Some(ref s) = sig {
                // 重复签名命中 → 发布 Critical 事件(Debug 算子检索同签名兄弟)
                if let Err(e) = bus.publish_blocking(event_bus::NexusEvent::ErrorSignatureMatched {
                    metadata: event_bus::EventMetadata::new("seccore"),
                    error_hash: s.error_hash.to_string(),
                    matched_card_ids: vec![], // L3 关联卡片由消费端按需填充
                }) {
                    tracing::warn!(error = %e, "ErrorSignatureMatched 发布失败");
                }
            }
        }
        sig
    }
}

impl Default for ErrorSignatureCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// 截取错误摘要前 SUMMARY_MAX_LEN 字符
fn truncate_summary(s: &str) -> String {
    s.chars().take(SUMMARY_MAX_LEN).collect()
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_error_hash_deterministic_16_chars() {
        let h1 = compute_error_hash("CompilationError", "mismatched types");
        let h2 = compute_error_hash("CompilationError", "mismatched types");
        assert_eq!(h1, h2, "同输入哈希应确定");
        assert_eq!(h1.len(), 16, "哈希应为前 16 位十六进制");
        // 不同输入哈希不同
        let h3 = compute_error_hash("CompilationError", "different summary");
        assert_ne!(h1, h3);
        let h4 = compute_error_hash("RuntimePanic", "mismatched types");
        assert_ne!(h1, h4, "不同 error_type 哈希应不同");
    }

    #[test]
    fn extract_compilation_error() {
        let mut collector = ErrorSignatureCollector::new();
        let output = "error[E0308]: mismatched types\n  --> src/main.rs:10";
        let sig = collector.extract(output, "src/main.rs:10").expect("应匹配");
        assert_eq!(sig.error_type.as_ref(), "CompilationError");
        assert!(sig.error_summary.as_ref().contains("mismatched types"));
        assert_eq!(sig.error_hash.len(), 16);
    }

    #[test]
    fn extract_runtime_panic() {
        let mut collector = ErrorSignatureCollector::new();
        let output = "thread 'main' panicked at src/lib.rs:42, index out of bounds";
        let sig = collector.extract(output, "src/lib.rs:42").expect("应匹配");
        assert_eq!(sig.error_type.as_ref(), "RuntimePanic");
        assert!(sig.error_summary.as_ref().contains("index out of bounds"));
    }

    #[test]
    fn extract_assertion_failure() {
        let mut collector = ErrorSignatureCollector::new();
        let output = "assertion failed: x == y";
        let sig = collector.extract(output, "test.rs:5").expect("应匹配");
        assert_eq!(sig.error_type.as_ref(), "AssertionFailure");
        assert!(sig.error_summary.as_ref().contains("x == y"));
    }

    #[test]
    fn extract_test_failure() {
        let mut collector = ErrorSignatureCollector::new();
        let output = "test result: FAILED. 2 failed, 3 passed; 0 ignored";
        let sig = collector.extract(output, "tests/x.rs").expect("应匹配");
        assert_eq!(sig.error_type.as_ref(), "TestFailure");
        assert!(sig.error_summary.as_ref().contains("2 failed, 3 passed"));
    }

    #[test]
    fn extract_timeout() {
        let mut collector = ErrorSignatureCollector::new();
        let output = "operation timeout after 5000ms";
        let sig = collector.extract(output, "cmd").expect("应匹配");
        assert_eq!(sig.error_type.as_ref(), "Timeout");
    }

    #[test]
    fn extract_generic_fallback() {
        let mut collector = ErrorSignatureCollector::new();
        // 无已知模式，但含 "error" 关键词 → 通用回退
        let output = "some unknown error occurred in module";
        let sig = collector.extract(output, "mod.rs").expect("应通用回退");
        assert_eq!(sig.error_type.as_ref(), "GenericError");
    }

    #[test]
    fn extract_no_error_returns_none() {
        let mut collector = ErrorSignatureCollector::new();
        let output = "build succeeded, all tests passed cleanly";
        // 无 error/failed/panic 关键词（"passed" 不含 "failed"）
        let result = collector.extract(output, "build");
        assert!(result.is_none(), "无错误信号应返回 None");
    }

    #[test]
    fn extract_empty_output_returns_none() {
        let mut collector = ErrorSignatureCollector::new();
        assert!(collector.extract("", "loc").is_none());
    }

    #[test]
    fn frequency_dedup_same_signature() {
        let mut collector = ErrorSignatureCollector::new();
        let output = "error[E0308]: mismatched types";
        // 提取 3 次相同错误
        for _ in 0..3 {
            collector.extract(output, "loc");
        }
        // 哈希去重: 唯一签名数为 1，频率为 3
        assert_eq!(collector.unique_signature_count(), 1);
        let frequent = collector.get_frequent_signatures(2);
        assert_eq!(frequent.len(), 1);
        assert_eq!(frequent[0].2, 3, "频率应为 3");
        assert_eq!(frequent[0].1, "CompilationError");
    }

    #[test]
    fn frequency_threshold_filter() {
        let mut collector = ErrorSignatureCollector::new();
        // 1 次编译错误 + 3 次 panic
        collector.extract("error[E0308]: mismatched", "a");
        for _ in 0..3 {
            collector.extract("thread 'main' panicked at x.rs:1, boom", "b");
        }
        // 阈值 2: 只有 panic（频率 3）满足
        let frequent = collector.get_frequent_signatures(2);
        assert_eq!(frequent.len(), 1);
        assert_eq!(frequent[0].1, "RuntimePanic");
        // 阈值 1: 两者都满足
        let all = collector.get_frequent_signatures(1);
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn error_type_frequency_tracking() {
        let mut collector = ErrorSignatureCollector::new();
        collector.extract("error[E0308]: mismatched", "a");
        collector.extract("error[E0425]: cannot find", "b");
        let freq = collector.error_type_frequency();
        assert_eq!(freq.get("CompilationError"), Some(&2));
    }

    #[test]
    fn summary_truncated_to_max_len() {
        let mut collector = ErrorSignatureCollector::new();
        let long_summary = "x".repeat(200);
        let output = format!("assertion failed: {long_summary}");
        let sig = collector.extract(&output, "loc").expect("应匹配");
        assert_eq!(
            sig.error_summary.chars().count(),
            SUMMARY_MAX_LEN,
            "摘要应截取前 100 字符"
        );
    }

    /// §16.4 extract_and_check_repeat:首次出现不重复,第二次出现即重复
    #[test]
    fn extract_and_check_repeat_detects_duplicate() {
        let mut collector = ErrorSignatureCollector::new();
        let output = "error[E0308]: mismatched types";
        // 首次:频率 1,不重复
        let (sig1, is_repeat1) = collector.extract_and_check_repeat(output, "loc");
        assert!(sig1.is_some(), "应匹配");
        assert!(!is_repeat1, "首次出现不应标记重复");
        // 第二次:频率 2,重复
        let (sig2, is_repeat2) = collector.extract_and_check_repeat(output, "loc");
        assert!(sig2.is_some(), "应匹配");
        assert!(is_repeat2, "第二次出现应标记重复");
    }

    /// §16.4 extract_and_publish:重复签名时发布 ErrorSignatureMatched 事件
    #[tokio::test]
    async fn extract_and_publish_emits_event_on_repeat() {
        let bus = event_bus::EventBus::new();
        let mut collector = ErrorSignatureCollector::new();
        let output = "error[E0308]: mismatched types";
        // 先订阅再发布(subscribe-before-spawn 红线)
        let mut rx = bus.subscribe();
        // 首次:不发布
        let sig1 = collector.extract_and_publish(&bus, output, "loc");
        assert!(sig1.is_some());
        // 第二次:发布 ErrorSignatureMatched
        let sig2 = collector.extract_and_publish(&bus, output, "loc");
        assert!(sig2.is_some());
        // 消费事件
        let event = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("应收到事件")
            .expect("接收成功");
        assert!(
            matches!(event, event_bus::NexusEvent::ErrorSignatureMatched { .. }),
            "应为 ErrorSignatureMatched"
        );
    }

    // ----------------------------------------------------------
    // 边界测试（§9.3 已全覆盖：5 模式 + 回退 + 频率统计；此处分场景补边界）
    // ----------------------------------------------------------

    /// 通用回退各关键词大小写变体映射到正确错误类型
    #[test]
    fn generic_fallback_keyword_variants_map_correctly() {
        let cases = [
            // 输出, 预期 error_type
            ("some unknown error in module", "GenericError"),
            ("An Error occurred", "GenericError"),
            ("FATAL ERROR OCCURRED", "GenericError"),
            ("operation failed quietly", "GenericFailure"),
            ("The build Failed", "GenericFailure"),
            ("sudden panic in worker", "GenericPanic"),
            ("Panic detected", "GenericPanic"),
        ];
        for (output, expected) in cases {
            let mut collector = ErrorSignatureCollector::new();
            let sig = collector.extract(output, "loc").expect("应触发回退");
            assert_eq!(sig.error_type.as_ref(), expected, "输出: {output}");
        }
    }

    /// 已知模式优先于通用关键词回退（输出同时含两者时，模式命中先返回）
    #[test]
    fn known_pattern_takes_precedence_over_generic_keyword() {
        let mut collector = ErrorSignatureCollector::new();
        // 同时含 "error[E0308]"（已知模式）与 "failed"（通用回退失败关键词）
        let output = "error[E0308]: mismatched types\n  [INFO] some steps failed";
        let sig = collector
            .extract(output, "src/main.rs:3")
            .expect("应命中模式");
        // 先匹配 CompilationError，不应回退为 GenericFailure
        assert_eq!(sig.error_type.as_ref(), "CompilationError");
    }

    /// 从未提取过任何签名时,高频查询与类型频率应返回空
    #[test]
    fn empty_collector_reports_zero_frequencies() {
        let collector = ErrorSignatureCollector::new();
        assert!(collector.get_frequent_signatures(1).is_empty());
        assert!(collector.error_type_frequency().is_empty());
        assert_eq!(collector.unique_signature_count(), 0);
    }

    /// extract_and_check_repeat 遇到无匹配输出→(None, false)
    #[test]
    fn extract_and_check_repeat_no_match_returns_none_false() {
        let mut collector = ErrorSignatureCollector::new();
        let (sig, is_repeat) =
            collector.extract_and_check_repeat("build succeeded cleanly", "build");
        assert!(sig.is_none(), "无错误信号应返回 None");
        assert!(!is_repeat, "无匹配时不应标记重复");
    }

    /// 空 error_type 与空 summary 哈希仍确定且为 16 位
    #[test]
    fn compute_error_hash_empty_inputs_deterministic() {
        let h1 = compute_error_hash("", "");
        let h2 = compute_error_hash("", "");
        assert_eq!(h1, h2, "空输入哈希应确定");
        assert_eq!(h1.len(), 16, "哈希恒为前 16 位十六进制");
    }
}
