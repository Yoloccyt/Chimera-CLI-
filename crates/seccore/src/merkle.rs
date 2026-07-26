//! Spec Merkle 完整性校验 — HarnessSpec 哈希与 Merkle 树归约（P4-W15.1.3）
//!
//! 对应架构层: **L4 Security**（seccore）
//! 对应 ADR: **ADR-031**（Harness-as-Spec + omega-learner 边界）
//! 对应任务: **P4-W15.1.3**（spec 与代码同级 Merkle 完整性校验，复用 seccore audit.rs）
//!
//! # 核心职责
//!
//! 1. **单 spec 哈希计算**: 接受 `HarnessSpec::canonical_merkle_input()` 返回的规范化字符串，
//!    计算 SHA-256 哈希（与 audit.rs `hash_simple_string` 同算法）
//! 2. **Merkle 根归约**: 接受多个 spec canonical 输入，构建 Merkle 二叉树并返回根哈希
//! 3. **完整性验证**: 比较 spec 哈希与预期哈希，检测篡改
//! 4. **Merkle 根验证**: 比较 Merkle 根与预期根，检测批量篡改
//!
//! # 设计决策（WHY）
//!
//! - **复用 audit.rs SHA-256**: 使用同款 `sha2::Sha256` + `hex::encode` 实现，
//!    避免重复造轮子，确保哈希算法一致（与决策链 step_hash 等同源）
//!
//! - **公共函数而非方法**: SpecLoader 与 SpecRegistry 需要调用，但 seccore
//!   不应依赖 L0 nexus-contracts（依赖铁律 L4→L0 允许，但 merkle.rs
//!   保持通用性，接受 &str 而非 HarnessSpec 引用，避免类型耦合）
//!
//! - **二叉树归约**: Merkle 树采用经典二叉树结构（每两个子节点哈希拼接后哈希），
//!   奇数节点复制最后一个节点配对（与 Bitcoin Merkle 树算法一致）
//!
//! - **空输入处理**: 空输入返回空字符串的 SHA-256（确定性常量），
//!   与 audit.rs `hash_decision_chain` 对空链的处理一致
//!
//! - **无错误返回**: SHA-256 与 hex 编码均为无失败操作，函数返回 String/bool
//!   简化调用方处理（无 Result 包装开销）
//!
//! # 流程图
//!
//! ```text
//! HarnessSpec.canonical_merkle_input() → String
//!                      │
//!                      ▼
//!            hash_spec_canonical_input()
//!                      │
//!                      ▼
//!            SHA-256 哈希（hex 编码 64 字符）
//!                      │
//!                      ▼
//!            verify_spec_integrity() → bool
//! ```
//!
//! # 多 spec Merkle 树（用于 SpecRegistry 谱系校验）
//!
//! ```text
//! 输入: [spec1_hash, spec2_hash, spec3_hash, spec4_hash]
//!                 │
//!                 ▼
//!      ┌─────────┴─────────┐
//!      │                   │
//!   hash(1+2)           hash(3+4)
//!      │                   │
//!      └─────────┬─────────┘
//!                │
//!                ▼
//!          Merkle Root
//! ```
//!
//! # 防注入保证（P4-W15.1.4 铺垫）
//!
//! - 所有函数为 `&[&str]` / `&str` 输入（不可变借用）
//! - 不修改输入数据，不写文件，不构造路径
//! - 哈希计算确定性：相同输入恒产生相同输出，攻击者无法操控

use sha2::{Digest, Sha256};

// ============================================================
// 内部辅助函数（与 audit.rs hash_simple_string 同算法）
// ============================================================

/// 计算字符串的 SHA-256 哈希（hex 编码 64 字符）
///
/// WHY 内部函数: 与 audit.rs `hash_simple_string` 算法一致，
/// 避免重复实现并确保哈希算法同源（若 audit.rs 升级算法，本函数同步升级）
fn hash_string(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hex::encode(hasher.finalize())
}

/// 计算两个 hex 字符串拼接后的 SHA-256 哈希
///
/// WHY 用于 Merkle 树归约: 两个子节点的哈希值拼接后再哈希，
/// 形成父节点哈希。拼接前不做分隔符处理（hex 字符串定长 64 字符，
/// 无拼接歧义风险）
fn hash_pair(left: &str, right: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(left.as_bytes());
    hasher.update(right.as_bytes());
    hex::encode(hasher.finalize())
}

// ============================================================
// 公共 API（P4-W15.1.3 核心）
// ============================================================

/// 计算单个 spec canonical 输入的 SHA-256 哈希
///
/// # 输入
/// - `canonical_input`: `HarnessSpec::canonical_merkle_input()` 返回的规范化字符串
///   （字段按固定顺序拼接，字段间用 `\x00` 分隔）
///
/// # 返回
/// - 64 字符 hex 编码的 SHA-256 哈希字符串
///
/// # 确定性保证
///
/// 相同输入恒产生相同输出（SHA-256 算法保证），可用于：
/// - 跨进程 spec 完整性校验
/// - SpecRegistry 谱系追踪（版本间哈希比较）
/// - CI 否决门（哈希不匹配则拒绝 spec 变更）
///
/// # 示例
///
/// ```
/// use seccore::merkle::hash_spec_canonical_input;
///
/// let input = "meta.name=test\x00meta.version=1";
/// let hash1 = hash_spec_canonical_input(input);
/// let hash2 = hash_spec_canonical_input(input);
/// assert_eq!(hash1, hash2, "相同输入应产生相同哈希");
/// assert_eq!(hash1.len(), 64, "SHA-256 哈希应为 64 字符 hex");
/// ```
pub fn hash_spec_canonical_input(canonical_input: &str) -> String {
    hash_string(canonical_input)
}

/// 计算多个 spec canonical 输入的 Merkle 根（二叉树归约）
///
/// # 算法
///
/// 1. 对每个输入计算 SHA-256 哈希（叶子节点）
/// 2. 两两拼接后哈希（父节点），奇数节点复制最后一个配对
/// 3. 重复直到只剩一个节点（根哈希）
///
/// # 边界情况
///
/// - **空输入**: 返回空字符串的 SHA-256（确定性常量，与 audit.rs 一致）
/// - **单输入**: 直接返回该输入的哈希（无需归约）
/// - **奇数输入**: 最后一层复制最后一个节点配对（Bitcoin Merkle 树算法）
///
/// # 参数
/// - `inputs`: spec canonical 输入字符串切片数组
///
/// # 返回
/// - 64 字符 hex 编码的 Merkle 根哈希
///
/// # 示例
///
/// ```
/// use seccore::merkle::compute_merkle_root;
///
/// let inputs = ["spec1", "spec2", "spec3", "spec4"];
/// let root = compute_merkle_root(&inputs);
/// assert_eq!(root.len(), 64);
///
/// // 相同输入恒产生相同根
/// let root2 = compute_merkle_root(&inputs);
/// assert_eq!(root, root2);
/// ```
pub fn compute_merkle_root(inputs: &[&str]) -> String {
    // 空输入：返回空字符串的 SHA-256
    if inputs.is_empty() {
        return hash_string("");
    }

    // 第 1 层：对每个输入计算叶子哈希
    let mut layer: Vec<String> = inputs.iter().map(|s| hash_string(s)).collect();

    // 二叉树归约：直到只剩一个节点
    while layer.len() > 1 {
        let mut next_layer: Vec<String> = Vec::with_capacity(layer.len().div_ceil(2));

        let mut i = 0;
        while i < layer.len() {
            if i + 1 < layer.len() {
                // 配对：left + right
                next_layer.push(hash_pair(&layer[i], &layer[i + 1]));
                i += 2;
            } else {
                // 奇数节点：复制最后一个配对（left == right）
                next_layer.push(hash_pair(&layer[i], &layer[i]));
                i += 1;
            }
        }

        layer = next_layer;
    }

    // 仅剩一个节点即根哈希
    layer.into_iter().next().unwrap_or_else(|| hash_string(""))
}

/// 验证单个 spec 的完整性（哈希比对）
///
/// # 流程
/// 1. 对 `canonical_input` 计算当前 SHA-256 哈希
/// 2. 与 `expected_hash` 比较（大小写敏感）
///
/// # 参数
/// - `canonical_input`: 当前 spec 的 canonical 字符串
/// - `expected_hash`: 预期哈希（64 字符 hex）
///
/// # 返回
/// - `true`: 哈希匹配，spec 未被篡改
/// - `false`: 哈希不匹配，spec 已被篡改或 expected_hash 无效
///
/// # 示例
///
/// ```
/// use seccore::merkle::{hash_spec_canonical_input, verify_spec_integrity};
///
/// let input = "meta.name=test\x00meta.version=1";
/// let expected = hash_spec_canonical_input(input);
/// assert!(verify_spec_integrity(input, &expected));
///
/// // 篡改输入应导致验证失败
/// let tampered = "meta.name=HACKED\x00meta.version=1";
/// assert!(!verify_spec_integrity(tampered, &expected));
/// ```
pub fn verify_spec_integrity(canonical_input: &str, expected_hash: &str) -> bool {
    let actual_hash = hash_spec_canonical_input(canonical_input);
    actual_hash == expected_hash
}

/// 验证 Merkle 根的完整性（批量校验）
///
/// # 流程
/// 1. 对 `inputs` 计算 Merkle 根
/// 2. 与 `expected_root` 比较
///
/// # 参数
/// - `inputs`: 多个 spec canonical 输入字符串切片数组
/// - `expected_root`: 预期 Merkle 根哈希（64 字符 hex）
///
/// # 返回
/// - `true`: 根哈希匹配，全部 spec 未被篡改
/// - `false`: 根哈希不匹配，存在篡改
///
/// # 示例
///
/// ```
/// use seccore::merkle::{compute_merkle_root, verify_merkle_root};
///
/// let inputs = ["spec1", "spec2", "spec3"];
/// let root = compute_merkle_root(&inputs);
/// assert!(verify_merkle_root(&inputs, &root));
///
/// // 任一输入篡改导致验证失败
/// let tampered_inputs = ["spec1", "HACKED", "spec3"];
/// assert!(!verify_merkle_root(&tampered_inputs, &root));
/// ```
pub fn verify_merkle_root(inputs: &[&str], expected_root: &str) -> bool {
    let actual_root = compute_merkle_root(inputs);
    actual_root == expected_root
}

// ============================================================
// 单元测试（P4-W15.1.3）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // === hash_spec_canonical_input 测试 ===

    #[test]
    fn test_hash_deterministic() {
        let input = "meta.name=test\x00meta.version=1";
        let hash1 = hash_spec_canonical_input(input);
        let hash2 = hash_spec_canonical_input(input);
        assert_eq!(hash1, hash2, "相同输入应产生相同哈希");
    }

    #[test]
    fn test_hash_length() {
        let input = "test";
        let hash = hash_spec_canonical_input(input);
        assert_eq!(hash.len(), 64, "SHA-256 hex 编码应为 64 字符");
    }

    #[test]
    fn test_hash_different_inputs_produce_different_hashes() {
        let hash1 = hash_spec_canonical_input("input1");
        let hash2 = hash_spec_canonical_input("input2");
        assert_ne!(hash1, hash2, "不同输入应产生不同哈希");
    }

    #[test]
    fn test_hash_empty_string() {
        let hash = hash_spec_canonical_input("");
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_hash_known_value() {
        // SHA-256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        let hash = hash_spec_canonical_input("hello");
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    // === compute_merkle_root 测试 ===

    #[test]
    fn test_merkle_root_empty_input() {
        let root = compute_merkle_root(&[]);
        // 空输入返回 SHA-256("")
        assert_eq!(
            root,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_merkle_root_single_input() {
        let inputs = ["single_spec"];
        let root = compute_merkle_root(&inputs);
        let single_hash = hash_spec_canonical_input("single_spec");
        // 单输入时根哈希等于该输入的哈希
        assert_eq!(root, single_hash);
    }

    #[test]
    fn test_merkle_root_two_inputs() {
        let inputs = ["spec1", "spec2"];
        let root = compute_merkle_root(&inputs);

        let h1 = hash_spec_canonical_input("spec1");
        let h2 = hash_spec_canonical_input("spec2");
        let expected_root = hash_pair(&h1, &h2);

        assert_eq!(root, expected_root);
    }

    #[test]
    fn test_merkle_root_four_inputs() {
        let inputs = ["spec1", "spec2", "spec3", "spec4"];
        let root = compute_merkle_root(&inputs);

        let h1 = hash_spec_canonical_input("spec1");
        let h2 = hash_spec_canonical_input("spec2");
        let h3 = hash_spec_canonical_input("spec3");
        let h4 = hash_spec_canonical_input("spec4");

        let parent1 = hash_pair(&h1, &h2);
        let parent2 = hash_pair(&h3, &h4);
        let expected_root = hash_pair(&parent1, &parent2);

        assert_eq!(root, expected_root);
    }

    #[test]
    fn test_merkle_root_three_inputs_odd_count() {
        // 奇数输入：最后一个节点复制配对
        let inputs = ["spec1", "spec2", "spec3"];
        let root = compute_merkle_root(&inputs);

        let h1 = hash_spec_canonical_input("spec1");
        let h2 = hash_spec_canonical_input("spec2");
        let h3 = hash_spec_canonical_input("spec3");

        let parent1 = hash_pair(&h1, &h2);
        // 奇数节点 h3 复制配对：hash(h3 + h3)
        let parent2 = hash_pair(&h3, &h3);
        let expected_root = hash_pair(&parent1, &parent2);

        assert_eq!(root, expected_root);
    }

    #[test]
    fn test_merkle_root_deterministic() {
        let inputs = ["spec1", "spec2", "spec3"];
        let root1 = compute_merkle_root(&inputs);
        let root2 = compute_merkle_root(&inputs);
        assert_eq!(root1, root2, "相同输入应产生相同根哈希");
    }

    #[test]
    fn test_merkle_root_order_matters() {
        // 顺序不同则根哈希不同
        let inputs1 = ["spec1", "spec2"];
        let inputs2 = ["spec2", "spec1"];
        let root1 = compute_merkle_root(&inputs1);
        let root2 = compute_merkle_root(&inputs2);
        assert_ne!(root1, root2, "顺序不同应产生不同根哈希");
    }

    #[test]
    fn test_merkle_root_large_input_set() {
        // 大量输入测试（100 个 spec）
        let inputs: Vec<String> = (0..100).map(|i| format!("spec_{}", i)).collect();
        let input_refs: Vec<&str> = inputs.iter().map(|s| s.as_str()).collect();
        let root = compute_merkle_root(&input_refs);
        assert_eq!(root.len(), 64);
    }

    // === verify_spec_integrity 测试 ===

    #[test]
    fn test_verify_spec_integrity_success() {
        let input = "meta.name=test\x00meta.version=1";
        let expected = hash_spec_canonical_input(input);
        assert!(verify_spec_integrity(input, &expected));
    }

    #[test]
    fn test_verify_spec_integrity_tampered_input() {
        let original = "meta.name=test\x00meta.version=1";
        let expected = hash_spec_canonical_input(original);

        // 篡改输入
        let tampered = "meta.name=HACKED\x00meta.version=1";
        assert!(!verify_spec_integrity(tampered, &expected));
    }

    #[test]
    fn test_verify_spec_integrity_wrong_expected_hash() {
        let input = "test";
        let wrong_expected = "0".repeat(64);
        assert!(!verify_spec_integrity(input, &wrong_expected));
    }

    #[test]
    fn test_verify_spec_integrity_empty_input() {
        let expected = hash_spec_canonical_input("");
        assert!(verify_spec_integrity("", &expected));
    }

    #[test]
    fn test_verify_spec_integrity_case_sensitive() {
        let input = "test";
        let expected = hash_spec_canonical_input(input);
        let uppercase_expected = expected.to_uppercase();
        // hex 编码是小写，大写应不匹配
        assert!(!verify_spec_integrity(input, &uppercase_expected));
    }

    // === verify_merkle_root 测试 ===

    #[test]
    fn test_verify_merkle_root_success() {
        let inputs = ["spec1", "spec2", "spec3"];
        let root = compute_merkle_root(&inputs);
        assert!(verify_merkle_root(&inputs, &root));
    }

    #[test]
    fn test_verify_merkle_root_tampered_input() {
        let original = ["spec1", "spec2", "spec3"];
        let root = compute_merkle_root(&original);

        let tampered = ["spec1", "HACKED", "spec3"];
        assert!(!verify_merkle_root(&tampered, &root));
    }

    #[test]
    fn test_verify_merkle_root_added_input() {
        let original = ["spec1", "spec2"];
        let root = compute_merkle_root(&original);

        // 添加额外输入
        let with_extra = ["spec1", "spec2", "spec3"];
        assert!(!verify_merkle_root(&with_extra, &root));
    }

    #[test]
    fn test_verify_merkle_root_removed_input() {
        let original = ["spec1", "spec2", "spec3"];
        let root = compute_merkle_root(&original);

        // 移除一个输入
        let with_less = ["spec1", "spec2"];
        assert!(!verify_merkle_root(&with_less, &root));
    }

    #[test]
    fn test_verify_merkle_root_empty_inputs() {
        let root = compute_merkle_root(&[]);
        assert!(verify_merkle_root(&[], &root));
    }

    #[test]
    fn test_verify_merkle_root_single_input() {
        let inputs = ["single"];
        let root = compute_merkle_root(&inputs);
        assert!(verify_merkle_root(&inputs, &root));
    }

    #[test]
    fn test_verify_merkle_root_wrong_expected() {
        let inputs = ["spec1", "spec2"];
        let wrong_root = "0".repeat(64);
        assert!(!verify_merkle_root(&inputs, &wrong_root));
    }

    // === 与 audit.rs 一致性测试 ===

    #[test]
    fn test_hash_algorithm_consistent_with_audit() {
        // merkle.rs 的 hash_string 应与 audit.rs 的 hash_simple_string 算法一致
        // 两者均使用 SHA-256 + hex::encode
        let input = "consistency_test";
        let hash = hash_spec_canonical_input(input);

        // 重新计算并验证（间接验证算法一致）
        let mut hasher = Sha256::new();
        hasher.update(input.as_bytes());
        let expected = hex::encode(hasher.finalize());

        assert_eq!(hash, expected);
    }

    // === 端到端完整性验证测试 ===

    #[test]
    fn test_end_to_end_spec_integrity_flow() {
        // 模拟 HarnessSpec canonical_merkle_input() 返回的字符串
        let canonical = "meta.name=test-spec\x00meta.version=1\x00meta.immutable=false";

        // 1. 计算哈希并存储
        let stored_hash = hash_spec_canonical_input(canonical);

        // 2. 后续验证：相同输入应通过
        assert!(verify_spec_integrity(canonical, &stored_hash));

        // 3. 篡改检测：修改任一字段应失败
        let tampered = "meta.name=HACKED\x00meta.version=1\x00meta.immutable=false";
        assert!(!verify_spec_integrity(tampered, &stored_hash));
    }

    #[test]
    fn test_end_to_end_merkle_root_lineage_verification() {
        // 模拟 SpecRegistry 中多个 spec 版本的 Merkle 谱系校验
        let v1 = "meta.name=spec\x00meta.version=1";
        let v2 = "meta.name=spec\x00meta.version=2";
        let v3 = "meta.name=spec\x00meta.version=3";

        let lineage = [v1, v2, v3];
        let root = compute_merkle_root(&lineage);

        // 1. 完整谱系应通过校验
        assert!(verify_merkle_root(&lineage, &root));

        // 2. 中间版本篡改应失败
        let tampered_lineage = [v1, "meta.name=spec\x00meta.version=HACKED", v3];
        assert!(!verify_merkle_root(&tampered_lineage, &root));

        // 3. 缺失版本应失败
        let partial_lineage = [v1, v2];
        assert!(!verify_merkle_root(&partial_lineage, &root));
    }
}
