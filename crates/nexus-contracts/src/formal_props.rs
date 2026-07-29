//! 形式化属性定义框架 — FormalVerifier L4 骨架的基础类型
//!
//! 对应架构层: L0 Contracts（纯类型定义，零逻辑）
//! 对应 ADR: ADR-033（L0 nexus-contracts 契约层）
//!
//! # 设计决策(WHY)
//!
//! - **纯类型零逻辑**: 遵循 ADR-033 约束；形式化验证的实际执行逻辑
//!   在 L4 FormalVerifier（`formal-verifier` crate），本文件仅承载跨层共享类型
//! - **消费层**: L4 formal-verifier（验证器实现）/ L6 omega-learner
//!   （进化闭环中查询验证结果）/ L8 parliament（审议时参考属性满足状态）
//! - **属性类别封闭枚举**: 当前四类属性覆盖 GSOE 谱系、AEGIS 单调性、
//!   Parliament 共识、通用不变量；新增类别需扩展枚举并更新 FormalVerifier

use serde::{Deserialize, Serialize};

/// 形式化属性类别 — 验证器按类别分派验证策略
///
/// WHY 封闭枚举而非字符串标签: 类别集合决定验证策略的分派路径，
/// 枚举提供编译期穷尽检查（`match` 必须覆盖所有变体），避免消费层
/// 字符串模糊匹配导致的遗漏。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertyCategory {
    /// 谱系完整性 — GSOE 谱系图必须保持 DAG 性质（无环、祖先闭包收敛）
    LineageIntegrity,
    /// 评分单调性 — AEGIS Critic 适应度→评分映射单调不减
    ScoreMonotonicity,
    /// 共识安全性 — Parliament 否决权不可被多数票覆盖（安全阀性质）
    ConsensusSafety,
    /// 不变量保持 — 通用不变量在状态转换后仍成立
    InvariantPreservation,
}

/// 验证方法 — 属性验证采用的技术手段
///
/// WHY 区分方法: 不同方法的可信度与成本差异显著；
/// PropTest 提供统计置信度，ManualProof 提供数学确定性，
/// Hybrid 结合两者优势（先 proptest 快速排除，再手动证明剩余路径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationMethod {
    /// proptest 属性测试 — 基于随机生成的统计验证
    PropTest,
    /// 手动证明 — 基于逻辑推理的确定性验证
    ManualProof,
    /// 混合方式 — proptest 快速排除 + 手动证明剩余路径
    Hybrid,
}

/// 验证结果 — 单次形式化验证的输出
///
/// # 语义
///
/// - `Satisfied`: 属性在采样范围内全部满足（`samples_tested` 记录采样数）
/// - `Violated`: 发现反例，属性被违反（`counterexample` 描述反例细节）
/// - `Skipped`: 验证因前置条件不满足而跳过（`reason` 说明跳过原因）
///
/// WHY 携带 `samples_tested`: proptest 结果为统计性质，
/// 消费方需知晓采样规模以评估置信度（100 次 vs 10000 次差异显著）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationResult {
    /// 验证通过 — 属性在 `samples_tested` 次采样中全部满足
    Satisfied {
        /// 采样测试次数
        samples_tested: u64,
    },
    /// 验证失败 — 发现反例
    Violated {
        /// 反例描述（人类可读，供调试与审计）
        counterexample: String,
        /// 采样测试次数（发现反例前已通过的采样数）
        samples_tested: u64,
    },
    /// 验证跳过 — 前置条件不满足
    Skipped {
        /// 跳过原因（如 "依赖模块未就绪" 或 "属性类别不适用"）
        reason: String,
    },
}

impl VerificationResult {
    /// 验证是否通过（`Satisfied` 变体返回 `true`）
    #[must_use]
    pub fn is_satisfied(&self) -> bool {
        matches!(self, Self::Satisfied { .. })
    }

    /// 验证是否失败（`Violated` 变体返回 `true`）
    #[must_use]
    pub fn is_violated(&self) -> bool {
        matches!(self, Self::Violated { .. })
    }

    /// 验证是否被跳过（`Skipped` 变体返回 `true`）
    #[must_use]
    pub fn is_skipped(&self) -> bool {
        matches!(self, Self::Skipped { .. })
    }
}

/// 不变量规格 — 定义单个形式化不变量的元数据
///
/// # 核心语义
///
/// - `id`: 不变量唯一标识（建议 "inv-" 前缀 + kebab-case）
/// - `description`: 人类可读的自然语言描述
/// - `category`: 属性类别（决定 FormalVerifier 分派哪种验证策略）
/// - `owner_crate`: 不变量所属的 crate（验证逻辑的实现位置）
/// - `verification_method`: 验证方法（proptest / manual / hybrid）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvariantSpec {
    /// 不变量唯一标识（建议 "inv-" 前缀 + kebab-case）
    pub id: String,
    /// 人类可读描述（自然语言，供开发者与审计员阅读）
    pub description: String,
    /// 属性类别（决定验证策略分派）
    pub category: PropertyCategory,
    /// 所属 crate（验证逻辑的实现位置，如 "gsoe-evolution"）
    pub owner_crate: String,
    /// 验证方法（proptest / manual / hybrid）
    pub verification_method: VerificationMethod,
}

impl InvariantSpec {
    /// 构造不变量规格
    pub fn new(
        id: impl Into<String>,
        description: impl Into<String>,
        category: PropertyCategory,
        owner_crate: impl Into<String>,
        verification_method: VerificationMethod,
    ) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            category,
            owner_crate: owner_crate.into(),
            verification_method,
        }
    }
}

/// 形式化属性 — 不变量规格与最近验证结果的聚合体
///
/// # 核心语义
///
/// - `spec`: 不变量的元数据规格（类别、所属 crate、验证方法等）
/// - `last_result`: 最近一次验证的结果（`None` 表示尚未执行验证）
///
/// WHY 聚合 `last_result`: FormalVerifier 在每次验证后更新此字段，
/// 消费方（Parliament 审议、omega-learner 进化决策）可直接查询最新状态，
/// 无需额外存储层。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormalProperty {
    /// 不变量规格
    pub spec: InvariantSpec,
    /// 最近验证结果（`None` = 尚未执行验证）
    pub last_result: Option<VerificationResult>,
}

impl FormalProperty {
    /// 构造形式化属性（初始无验证结果）
    pub fn new(spec: InvariantSpec) -> Self {
        Self {
            spec,
            last_result: None,
        }
    }

    /// 更新验证结果（FormalVerifier 在每次验证后调用）
    #[must_use]
    pub fn with_result(mut self, result: VerificationResult) -> Self {
        self.last_result = Some(result);
        self
    }

    /// 属性是否已验证通过
    #[must_use]
    pub fn is_verified(&self) -> bool {
        self.last_result
            .as_ref()
            .is_some_and(VerificationResult::is_satisfied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── PropertyCategory 测试 ──

    #[test]
    fn test_property_category_debug_clone_eq() {
        let cat = PropertyCategory::LineageIntegrity;
        let cloned = cat;
        assert_eq!(cat, cloned);
        // 确保 Debug 实现不 panic
        let debug_str = format!("{cat:?}");
        assert!(debug_str.contains("LineageIntegrity"));
    }

    #[test]
    fn test_property_category_all_variants_distinct() {
        let variants = [
            PropertyCategory::LineageIntegrity,
            PropertyCategory::ScoreMonotonicity,
            PropertyCategory::ConsensusSafety,
            PropertyCategory::InvariantPreservation,
        ];
        for (i, a) in variants.iter().enumerate() {
            for (j, b) in variants.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn test_property_category_serde_roundtrip() {
        let cat = PropertyCategory::ConsensusSafety;
        let json = serde_json::to_string(&cat).expect("序列化失败");
        let back: PropertyCategory = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(cat, back);
        // snake_case 约定
        assert!(json.contains("consensus_safety"));
    }

    // ── VerificationMethod 测试 ──

    #[test]
    fn test_verification_method_debug_clone_eq() {
        let method = VerificationMethod::PropTest;
        let cloned = method;
        assert_eq!(method, cloned);
        let debug_str = format!("{method:?}");
        assert!(debug_str.contains("PropTest"));
    }

    #[test]
    fn test_verification_method_serde_roundtrip() {
        let method = VerificationMethod::Hybrid;
        let json = serde_json::to_string(&method).expect("序列化失败");
        let back: VerificationMethod = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(method, back);
        assert!(json.contains("hybrid"));
    }

    // ── VerificationResult 测试 ──

    #[test]
    fn test_verification_result_satisfied() {
        let result = VerificationResult::Satisfied {
            samples_tested: 1000,
        };
        assert!(result.is_satisfied());
        assert!(!result.is_violated());
        assert!(!result.is_skipped());
    }

    #[test]
    fn test_verification_result_violated() {
        let result = VerificationResult::Violated {
            counterexample: "cycle detected in lineage graph".into(),
            samples_tested: 42,
        };
        assert!(!result.is_satisfied());
        assert!(result.is_violated());
        assert!(!result.is_skipped());
    }

    #[test]
    fn test_verification_result_skipped() {
        let result = VerificationResult::Skipped {
            reason: "dependency not ready".into(),
        };
        assert!(!result.is_satisfied());
        assert!(!result.is_violated());
        assert!(result.is_skipped());
    }

    #[test]
    fn test_verification_result_serde_roundtrip() {
        let satisfied = VerificationResult::Satisfied {
            samples_tested: 500,
        };
        let json = serde_json::to_string(&satisfied).expect("序列化失败");
        let back: VerificationResult = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(satisfied, back);

        let violated = VerificationResult::Violated {
            counterexample: "monotonicity broken at x=5".into(),
            samples_tested: 100,
        };
        let json = serde_json::to_string(&violated).expect("序列化失败");
        let back: VerificationResult = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(violated, back);
    }

    // ── InvariantSpec 测试 ──

    #[test]
    fn test_invariant_spec_construction() {
        let spec = InvariantSpec::new(
            "inv-lineage-dag",
            "谱系图必须保持 DAG 性质",
            PropertyCategory::LineageIntegrity,
            "gsoe-evolution",
            VerificationMethod::PropTest,
        );
        assert_eq!(spec.id, "inv-lineage-dag");
        assert_eq!(spec.category, PropertyCategory::LineageIntegrity);
        assert_eq!(spec.owner_crate, "gsoe-evolution");
        assert_eq!(spec.verification_method, VerificationMethod::PropTest);
    }

    #[test]
    fn test_invariant_spec_serde_roundtrip() {
        let spec = InvariantSpec::new(
            "inv-score-mono",
            "评分单调不减",
            PropertyCategory::ScoreMonotonicity,
            "gsoe-evolution",
            VerificationMethod::Hybrid,
        );
        let json = serde_json::to_string(&spec).expect("序列化失败");
        let back: InvariantSpec = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(spec, back);
    }

    // ── FormalProperty 测试 ──

    #[test]
    fn test_formal_property_new_has_no_result() {
        let spec = InvariantSpec::new(
            "inv-test",
            "test invariant",
            PropertyCategory::InvariantPreservation,
            "test-crate",
            VerificationMethod::ManualProof,
        );
        let prop = FormalProperty::new(spec);
        assert!(prop.last_result.is_none());
        assert!(!prop.is_verified());
    }

    #[test]
    fn test_formal_property_with_satisfied_result() {
        let spec = InvariantSpec::new(
            "inv-test",
            "test invariant",
            PropertyCategory::InvariantPreservation,
            "test-crate",
            VerificationMethod::PropTest,
        );
        let prop = FormalProperty::new(spec).with_result(VerificationResult::Satisfied {
            samples_tested: 1000,
        });
        assert!(prop.is_verified());
        assert!(prop.last_result.is_some());
    }

    #[test]
    fn test_formal_property_with_violated_result_not_verified() {
        let spec = InvariantSpec::new(
            "inv-test",
            "test invariant",
            PropertyCategory::ConsensusSafety,
            "parliament",
            VerificationMethod::PropTest,
        );
        let prop = FormalProperty::new(spec).with_result(VerificationResult::Violated {
            counterexample: "veto overridden".into(),
            samples_tested: 50,
        });
        assert!(!prop.is_verified());
        assert!(prop.last_result.is_some());
    }

    #[test]
    fn test_formal_property_serde_roundtrip() {
        let spec = InvariantSpec::new(
            "inv-consensus-veto",
            "Parliament 否决权不可覆盖",
            PropertyCategory::ConsensusSafety,
            "parliament",
            VerificationMethod::ManualProof,
        );
        let prop = FormalProperty::new(spec)
            .with_result(VerificationResult::Satisfied { samples_tested: 1 });
        let json = serde_json::to_string(&prop).expect("序列化失败");
        let back: FormalProperty = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(prop, back);
    }

    // ── Hash trait 测试（PropertyCategory / VerificationMethod 可用于 HashMap key） ──

    #[test]
    fn test_property_category_as_hashmap_key() {
        use std::collections::HashMap;
        let mut map = HashMap::new();
        map.insert(PropertyCategory::LineageIntegrity, "gsoe");
        map.insert(PropertyCategory::ConsensusSafety, "parliament");
        assert_eq!(map.get(&PropertyCategory::LineageIntegrity), Some(&"gsoe"));
        assert_eq!(
            map.get(&PropertyCategory::ConsensusSafety),
            Some(&"parliament")
        );
    }
}

// ================================================================
// proptest 属性测试 — InvariantSpec / VerificationResult / PropertyCategory 不变量
//
// 对应任务: T6-6 proptest 属性测试集成
// 验证的不变量:
// 1. InvariantSpec serde roundtrip(任意字符串输入)
// 2. VerificationResult::Satisfied serde roundtrip
// 3. VerificationResult::Violated serde roundtrip(任意字符串)
// 4. PropertyCategory Hash 一致性(相等值 → 相等 hash)
// ================================================================

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    /// 辅助: 将 u8 索引映射到 PropertyCategory 枚举变体
    fn category_from_idx(idx: u8) -> PropertyCategory {
        match idx % 4 {
            0 => PropertyCategory::LineageIntegrity,
            1 => PropertyCategory::ScoreMonotonicity,
            2 => PropertyCategory::ConsensusSafety,
            _ => PropertyCategory::InvariantPreservation,
        }
    }

    /// 辅助: 将 u8 索引映射到 VerificationMethod 枚举变体
    fn method_from_idx(idx: u8) -> VerificationMethod {
        match idx % 3 {
            0 => VerificationMethod::PropTest,
            1 => VerificationMethod::ManualProof,
            _ => VerificationMethod::Hybrid,
        }
    }

    fn calc_hash<T: Hash>(val: &T) -> u64 {
        let mut hasher = DefaultHasher::new();
        val.hash(&mut hasher);
        hasher.finish()
    }

    proptest! {
        /// 不变量 1: InvariantSpec JSON serde roundtrip — 任意字符串输入后
        /// 反序列化结果与原始值相等(id / description / owner_crate 字段保持)
        #[test]
        fn prop_invariant_spec_json_roundtrip(
            id in "[a-z]{1,20}",
            desc in "[A-Za-z ]{1,100}",
            cat_idx in 0u8..16u8,
            owner in "[a-z\\-]{1,30}",
            method_idx in 0u8..12u8,
        ) {
            let spec = InvariantSpec {
                id: id.clone(),
                description: desc.clone(),
                category: category_from_idx(cat_idx),
                owner_crate: owner.clone(),
                verification_method: method_from_idx(method_idx),
            };
            let json = serde_json::to_string(&spec)?;
            let parsed: InvariantSpec = serde_json::from_str(&json)?;
            prop_assert_eq!(spec.id, parsed.id);
            prop_assert_eq!(spec.description, parsed.description);
            prop_assert_eq!(spec.category, parsed.category);
            prop_assert_eq!(spec.owner_crate, parsed.owner_crate);
            prop_assert_eq!(spec.verification_method, parsed.verification_method);
        }

        /// 不变量 2: VerificationResult::Satisfied serde roundtrip —
        /// 任意 samples_tested 值经 JSON 序列化/反序列化后保持不变
        #[test]
        fn prop_verification_result_satisfied_roundtrip(
            samples in 0u64..1_000_000u64,
        ) {
            let result = VerificationResult::Satisfied { samples_tested: samples };
            let json = serde_json::to_string(&result)?;
            let parsed: VerificationResult = serde_json::from_str(&json)?;
            prop_assert!(parsed.is_satisfied());
            prop_assert_eq!(result, parsed);
        }

        /// 不变量 3: VerificationResult::Violated serde roundtrip —
        /// 任意 counterexample 字符串与 samples_tested 值保持不变
        #[test]
        fn prop_verification_result_violated_roundtrip(
            counterexample in "[A-Za-z0-9 _\\-]{1,200}",
            samples in 0u64..1_000_000u64,
        ) {
            let result = VerificationResult::Violated {
                counterexample: counterexample.clone(),
                samples_tested: samples,
            };
            let json = serde_json::to_string(&result)?;
            let parsed: VerificationResult = serde_json::from_str(&json)?;
            prop_assert!(parsed.is_violated());
            prop_assert_eq!(result, parsed);
        }

        /// 不变量 4: PropertyCategory Hash 一致性 —
        /// 相等的 PropertyCategory 值必须产生相等的 hash(HashMap key 安全性)
        #[test]
        fn prop_property_category_hash_consistency(
            idx_a in 0u8..16u8,
            idx_b in 0u8..16u8,
        ) {
            let cat_a = category_from_idx(idx_a);
            let cat_b = category_from_idx(idx_b);
            // 如果两个 category 相等,它们的 hash 必须相等
            if cat_a == cat_b {
                prop_assert_eq!(
                    calc_hash(&cat_a),
                    calc_hash(&cat_b),
                    "equal PropertyCategory values must have equal hashes"
                );
            }
        }

        /// 不变量 5: FormalProperty serde roundtrip — 含可选 last_result
        #[test]
        fn prop_formal_property_roundtrip(
            id in "[a-z]{1,20}",
            desc in "[A-Za-z ]{1,80}",
            cat_idx in 0u8..8u8,
            owner in "[a-z\\-]{1,20}",
            method_idx in 0u8..6u8,
            has_result in proptest::bool::ANY,
            samples in 0u64..10_000u64,
        ) {
            let spec = InvariantSpec {
                id,
                description: desc,
                category: category_from_idx(cat_idx),
                owner_crate: owner,
                verification_method: method_from_idx(method_idx),
            };
            let prop = if has_result {
                FormalProperty::new(spec).with_result(VerificationResult::Satisfied {
                    samples_tested: samples,
                })
            } else {
                FormalProperty::new(spec)
            };
            let json = serde_json::to_string(&prop)?;
            let parsed: FormalProperty = serde_json::from_str(&json)?;
            prop_assert_eq!(prop, parsed);
        }
    }
}
