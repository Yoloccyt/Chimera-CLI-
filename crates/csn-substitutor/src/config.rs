//! CSN 配置定义
//!
//! 控制语义向量维度、注册表容量与默认降级层级。
//! 配置项默认值经过权衡,适合大多数 L10 Interface 层能力替代场景。

use serde::{Deserialize, Serialize};

/// CSN 配置 — 控制能力注册表与降级链行为
///
/// 所有字段均有合理默认值,可通过 `Default::default()` 快速创建。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CsnConfig {
    /// 语义向量维度
    ///
    /// 默认 50,与 CSN 设计文档对齐(100 能力 × 50 维 in-memory)。
    /// WHY 50:平衡语义表达力与内存占用,50 维足以区分典型工具能力;
    /// 每个能力约 200 字节,100 能力约 20KB,可全部驻留 L1 缓存。
    pub vector_dimension: usize,

    /// 注册表容量上限
    ///
    /// 默认 100,对应 CSN 设计目标的"100 能力"。
    /// WHY 100:典型 CLI 工具场景下,活跃能力约 50-100 个,
    /// 100 容量可覆盖大多数工作集;超出时调用方应先驱逐冷能力。
    pub registry_capacity: usize,

    /// 默认降级层级列表(≥ 3 级,每级为候选 ID 列表)
    ///
    /// ## v2.9.0-omega 重设计(ADR-062)
    /// 类型从 `Vec<String>` 改为 `Vec<Vec<String>>`:
    /// - 外层 Vec 长度 = 降级深度(≥ 3 级,架构红线)
    /// - 内层 Vec 为该级的显式候选 ID 列表:
    ///   - **空 Vec** → 触发 Top-(N+1) 模式(level 0 返回 Top-1,level 1 返回 Top-2 的第 2 个,...)
    ///   - **非空 Vec** → 按显式 ID 列表查找该级的替代候选
    ///
    /// ## 默认值 `[[], [], []]`
    /// 3 级降级,每级用 Top-(N+1) 模式,即:
    /// - level 0:返回 Top-1 候选(primary substitute)
    /// - level 1:返回 Top-2 的第 2 个候选(secondary substitute)
    /// - level 2:返回 Top-3 的第 3 个候选(tertiary substitute)
    ///
    /// ## 向后兼容
    /// 旧格式 `["primary", "secondary", "tertiary"]`(字符串数组)自动转换
    /// 为 `[[], [], []]`。旧字符串仅是层级名称,不影响候选选择;新设计中
    /// 空 Vec 触发 Top-K 模式,与旧"层级名称"语义等价(均不指定显式候选)。
    /// 兼容期:2 个版本周期(v2.9 - v2.10),v2.11 起移除。
    #[serde(
        default = "default_degradation_levels",
        deserialize_with = "deserialize_degradation_levels"
    )]
    pub default_degradation_levels: Vec<Vec<String>>,

    /// 默认 Top-K 值
    ///
    /// 默认 5,从注册表中选出相似度最高的 5 个候选。
    /// WHY 5:与 FaaE/GEA 的 Top-K=8 对齐(略低,因 CSN 仅在失败时触发),
    /// 过大增加选择开销,过小丢失可选替代。
    pub top_k: usize,

    /// 相似度阈值 — 低于此值的候选将被过滤
    ///
    /// 默认 0.5。`find_substitutes` 会过滤掉相似度 < threshold 的候选,
    /// 避免选择语义不相关的替代(WHY:阈值过低会导致"伪替代",如正交能力被选为替代)。
    ///
    /// ## 取值范围 [0.0, 1.0]
    /// - 0.0:禁用过滤(所有候选都保留,包括负相似度)
    /// - 0.5:默认值,过滤掉相似度 < 0.5 的候选
    /// - 0.9:严格过滤,只保留高度相似的候选
    ///
    /// WHY f32:与 `similarity_score: f32` 类型一致,避免 §4.4 #6 f32→f64 精度膨胀。
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f32,
}

/// 默认降级层级:`[[], [], []]`(3 级,每级 Top-(N+1) 模式)
fn default_degradation_levels() -> Vec<Vec<String>> {
    vec![Vec::new(), Vec::new(), Vec::new()]
}

/// 默认相似度阈值:0.5
fn default_similarity_threshold() -> f32 {
    0.5
}

/// 向后兼容反序列化器 — 支持旧字符串数组与新二维数组两种格式
///
/// ## 转换规则
/// - 旧格式 `["primary", "secondary", "tertiary"]` → `[[], [], []]`
///   (每级空 Vec,触发 Top-(N+1) 模式;旧字符串仅是层级名称,不影响候选选择)
/// - 新格式 `[[], [], []]` 或 `[["cap-a"], ["cap-b"], []]` → 原样保留
///
/// ## 兼容期
/// 2 个版本周期(v2.9 - v2.10),v2.11 起移除旧格式支持。
fn deserialize_degradation_levels<'de, D>(deserializer: D) -> Result<Vec<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;

    /// unagged enum:先尝试新格式(二维数组),失败则尝试旧格式(字符串数组)
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum LevelsFormat {
        /// 新格式:二维数组 `[[], [], []]` 或 `[["cap-a"], ["cap-b"]]`
        New(Vec<Vec<String>>),
        /// 旧格式:字符串数组 `["primary", "secondary", "tertiary"]`
        Legacy(Vec<String>),
    }

    match LevelsFormat::deserialize(deserializer)? {
        LevelsFormat::New(levels) => Ok(levels),
        // 旧格式 → 新格式:每级用空 Vec 表示 Top-(N+1) 模式
        // WHY 空 Vec:旧字符串仅是层级名称(如 "primary"),不指定显式候选 ID;
        // 新设计中空 Vec 触发 Top-K 自动选择,与旧"层级名称"语义等价
        LevelsFormat::Legacy(strings) => Ok(strings.iter().map(|_| Vec::new()).collect()),
    }
}

impl Default for CsnConfig {
    fn default() -> Self {
        Self {
            vector_dimension: 50,
            registry_capacity: 100,
            default_degradation_levels: default_degradation_levels(),
            top_k: 5,
            similarity_threshold: default_similarity_threshold(),
        }
    }
}

impl CsnConfig {
    /// 校验配置合法性
    ///
    /// WHY:防御性编程(P4-5)—`default_degradation_levels` 为空会导致降级链无法创建,
    /// 在配置构造阶段提前拦截而非运行时静默失败。
    pub fn validate(&self) -> Result<(), String> {
        if self.default_degradation_levels.is_empty() {
            return Err("default_degradation_levels 不能为空(架构红线:≥ 3 级降级)".into());
        }
        if self.default_degradation_levels.len() < 3 {
            return Err(format!(
                "default_degradation_levels 至少需要 3 级,当前:{}",
                self.default_degradation_levels.len()
            ));
        }
        // similarity_threshold 范围校验 [0.0, 1.0]
        // WHY f32 直接比较:与字段类型一致,避免 §4.4 #6 f32→f64 精度膨胀
        if self.similarity_threshold < 0.0 || self.similarity_threshold > 1.0 {
            return Err(format!(
                "similarity_threshold 必须在 [0.0, 1.0] 范围内,当前:{}",
                self.similarity_threshold
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CsnConfig::default();
        assert_eq!(config.vector_dimension, 50);
        assert_eq!(config.registry_capacity, 100);
        assert_eq!(
            config.default_degradation_levels.len(),
            3,
            "默认 ≥ 3 级降级"
        );
        // 默认每级为空 Vec(Top-(N+1) 模式)
        assert!(
            config
                .default_degradation_levels
                .iter()
                .all(|v| v.is_empty()),
            "默认每级应为空 Vec"
        );
        assert_eq!(config.top_k, 5);
        assert!((config.similarity_threshold - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_default_degradation_levels_at_least_three() {
        let config = CsnConfig::default();
        assert!(
            config.default_degradation_levels.len() >= 3,
            "降级链深度必须 ≥ 3 级(架构红线)"
        );
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = CsnConfig {
            vector_dimension: 64,
            registry_capacity: 200,
            default_degradation_levels: vec![
                vec!["L1".into()],
                vec!["L2".into()],
                vec!["L3".into()],
                vec!["L4".into()],
            ],
            top_k: 10,
            similarity_threshold: 0.7,
        };
        let json = serde_json::to_string(&config).expect("序列化失败");
        let restored: CsnConfig = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(restored.vector_dimension, 64);
        assert_eq!(restored.registry_capacity, 200);
        assert_eq!(restored.default_degradation_levels.len(), 4);
        assert_eq!(restored.top_k, 10);
        assert!((restored.similarity_threshold - 0.7).abs() < 1e-6);
    }

    // === SubTask 0.5.2: 向后兼容解析测试 ===

    #[test]
    fn test_backward_compat_legacy_string_array() {
        // 旧格式:字符串数组 → 自动转换为空 Vec 数组
        let json = r#"{
            "vector_dimension": 50,
            "registry_capacity": 100,
            "default_degradation_levels": ["primary", "secondary", "tertiary"],
            "top_k": 5,
            "similarity_threshold": 0.5
        }"#;
        let config: CsnConfig = serde_json::from_str(json).expect("反序列化失败");
        assert_eq!(config.default_degradation_levels.len(), 3);
        // 旧字符串应被转换为空 Vec(触发 Top-(N+1) 模式)
        assert!(
            config
                .default_degradation_levels
                .iter()
                .all(|v| v.is_empty()),
            "旧格式字符串应转换为空 Vec"
        );
    }

    #[test]
    fn test_new_format_2d_array() {
        // 新格式:二维数组,每级显式指定候选 ID
        let json = r#"{
            "vector_dimension": 50,
            "registry_capacity": 100,
            "default_degradation_levels": [["cap-a"], ["cap-b", "cap-c"], []],
            "top_k": 5,
            "similarity_threshold": 0.5
        }"#;
        let config: CsnConfig = serde_json::from_str(json).expect("反序列化失败");
        assert_eq!(config.default_degradation_levels.len(), 3);
        assert_eq!(config.default_degradation_levels[0], vec!["cap-a"]);
        assert_eq!(config.default_degradation_levels[1], vec!["cap-b", "cap-c"]);
        assert!(config.default_degradation_levels[2].is_empty());
    }

    #[test]
    fn test_default_degradation_levels_omitted_uses_default() {
        // 省略 default_degradation_levels 字段时使用默认值
        let json = r#"{
            "vector_dimension": 50,
            "registry_capacity": 100,
            "top_k": 5,
            "similarity_threshold": 0.5
        }"#;
        let config: CsnConfig = serde_json::from_str(json).expect("反序列化失败");
        assert_eq!(config.default_degradation_levels.len(), 3);
        assert!(
            config
                .default_degradation_levels
                .iter()
                .all(|v| v.is_empty()),
            "省略字段应使用默认值 [vec![], vec![], vec![]]"
        );
    }

    #[test]
    fn test_similarity_threshold_omitted_uses_default() {
        let json = r#"{
            "vector_dimension": 50,
            "registry_capacity": 100,
            "top_k": 5
        }"#;
        let config: CsnConfig = serde_json::from_str(json).expect("反序列化失败");
        assert!(
            (config.similarity_threshold - 0.5).abs() < 1e-6,
            "省略 similarity_threshold 应使用默认值 0.5"
        );
    }

    #[test]
    fn test_config_clone() {
        let config = CsnConfig::default();
        let cloned = config.clone();
        assert_eq!(config.vector_dimension, cloned.vector_dimension);
        assert_eq!(config.registry_capacity, cloned.registry_capacity);
        assert_eq!(
            config.default_degradation_levels,
            cloned.default_degradation_levels
        );
        assert!((config.similarity_threshold - cloned.similarity_threshold).abs() < 1e-6);
    }

    #[test]
    fn test_validate_default_config_ok() {
        let config = CsnConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_degradation_levels_err() {
        let config = CsnConfig {
            default_degradation_levels: vec![],
            ..CsnConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_insufficient_degradation_levels_err() {
        let config = CsnConfig {
            default_degradation_levels: vec![vec!["L1".into()], vec!["L2".into()]],
            ..CsnConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_similarity_threshold_out_of_range() {
        let config = CsnConfig {
            similarity_threshold: 1.5,
            ..CsnConfig::default()
        };
        assert!(config.validate().is_err());

        let config = CsnConfig {
            similarity_threshold: -0.1,
            ..CsnConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_similarity_threshold_boundaries_ok() {
        // 边界值 0.0 和 1.0 应通过
        let config = CsnConfig {
            similarity_threshold: 0.0,
            ..CsnConfig::default()
        };
        assert!(config.validate().is_ok());

        let config = CsnConfig {
            similarity_threshold: 1.0,
            ..CsnConfig::default()
        };
        assert!(config.validate().is_ok());
    }
}
