//! 变体契约 — Harness 变体标识与性能契约(polish-v2.7 P3-2)
//!
//! 对应架构层: L0 Contracts(新建)
//! 对应 ADR: ADR-051(Variant Pool + 规则路由)+ ADR-049 决策 1
//! 对应设计源: `chimera_ultimate_polish_v2.7.md` §4.2 / §12.3(小米变体隔离)
//!
//! # 设计决策(WHY)
//!
//! - **纯类型零逻辑**: 遵循 ADR-033 约束;变体池的存储与审议逻辑在
//!   L8 parliament(variant_pool.rs),本文件仅承载跨层共享类型
//! - **消费层**: L8 parliament(变体池与审议)/ L5 gsoe-evolution
//!   (AEGIS 产出变体登记时构造 VariantId)
//! - **变体隔离依据**: 小米 HarnessX 实测"全局单 Harness 退化 -24.3%",
//!   按任务类型隔离变体避免灾难性遗忘(方案 §1.2)

use serde::{Deserialize, Serialize};

/// Harness 变体标识 — 变体池中的唯一键
///
/// WHY 用 `spec_name + spec_version` 而非独立 hash:变体本体是
/// SpecRegistry 中登记的 HarnessSpec,复用其 `(name, version)` 主键
/// 避免双主键漂移(单一事实源原则)。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VariantId {
    /// 变体对应的 HarnessSpec 名称
    pub spec_name: String,
    /// 变体对应的 HarnessSpec 版本
    pub spec_version: u32,
}

impl VariantId {
    /// 构造变体标识
    pub fn new(spec_name: impl Into<String>, spec_version: u32) -> Self {
        Self {
            spec_name: spec_name.into(),
            spec_version,
        }
    }
}

impl std::fmt::Display for VariantId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@v{}", self.spec_name, self.spec_version)
    }
}

/// 变体性能契约 — 变体在其适用任务类型上的性能承诺
///
/// # 语义
///
/// - `task_types`:变体的适用范围(规则路由的匹配键,ADR-051)
/// - `expected_performance`:预期成功率 \[0\.0, 1\.0\](审议时的基线承诺)
/// - `max_regression`:允许的最大回归幅度(超过即触发回滚评估)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariantContract {
    /// 变体标识
    pub variant_id: VariantId,
    /// 适用任务类型集合(空 = 通用变体,兜底路由)
    pub task_types: Vec<String>,
    /// 预期成功率 \[0\.0, 1\.0\]
    pub expected_performance: f32,
    /// 允许的最大回归幅度 \[0\.0, 1\.0\](相对 expected_performance)
    pub max_regression: f32,
}

impl VariantContract {
    /// 构造变体契约(性能字段 clamp 至 \[0,1\],防御越界输入)
    pub fn new(
        variant_id: VariantId,
        task_types: Vec<String>,
        expected_performance: f32,
        max_regression: f32,
    ) -> Self {
        Self {
            variant_id,
            task_types,
            expected_performance: expected_performance.clamp(0.0, 1.0),
            max_regression: max_regression.clamp(0.0, 1.0),
        }
    }

    /// 变体是否适用于指定任务类型(空 task_types = 通用兜底)
    pub fn matches_task_type(&self, task_type: &str) -> bool {
        self.task_types.is_empty() || self.task_types.iter().any(|t| t == task_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_variant_id_display_and_hash_key() {
        let id = VariantId::new("spec-a", 3);
        assert_eq!(id.to_string(), "spec-a@v3");
        assert_eq!(id, VariantId::new("spec-a", 3));
        assert_ne!(id, VariantId::new("spec-a", 4));
    }

    #[test]
    fn test_contract_clamps_performance_bounds() {
        let contract = VariantContract::new(VariantId::new("s", 1), vec![], 1.5, -0.2);
        assert_eq!(contract.expected_performance, 1.0);
        assert_eq!(contract.max_regression, 0.0);
    }

    #[test]
    fn test_contract_task_type_matching() {
        let specific =
            VariantContract::new(VariantId::new("s", 1), vec!["code_fix".into()], 0.8, 0.1);
        assert!(specific.matches_task_type("code_fix"));
        assert!(!specific.matches_task_type("doc_gen"));

        // 空 task_types = 通用兜底,匹配一切
        let universal = VariantContract::new(VariantId::new("s", 1), vec![], 0.8, 0.1);
        assert!(universal.matches_task_type("anything"));
    }

    #[test]
    fn test_serde_roundtrip() {
        let contract =
            VariantContract::new(VariantId::new("s", 2), vec!["refactor".into()], 0.75, 0.05);
        let json = serde_json::to_string(&contract).expect("序列化失败");
        let back: VariantContract = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(contract, back);
    }
}
