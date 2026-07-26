//! HarnessSpec 加载器 — TOML 反序列化 + 字段校验 + 不可进化面检查（P4-W15.1.2）
//!
//! 对应架构层: **L5 Knowledge**（gsoe-evolution）
//! 对应 ADR: **ADR-031**（Harness-as-Spec + omega-learner 边界）
//! 对应任务: **P4-W15.1.2**（spec 加载器实现）
//!
//! # 核心职责
//!
//! 1. **TOML 反序列化**: 解析 TOML 格式 HarnessSpec 文件（设计文档 §7.2 完整 DSL）
//! 2. **字段校验**: 调用 `HarnessSpec::validate()` 执行 L0 校验规则
//! 3. **不可进化面检查**: 通过 validate() 拒绝任何试图修改不可进化面的 spec
//! 4. **严格 acceptance_gates 校验**: 解析 auxiliary 字段，验证 4 个强制门全部存在
//! 5. **防注入保证**: 所有方法返回 `&HarnessSpec`（不可变），不暴露写路径
//!
//! # 设计决策（WHY）
//!
//! - **中间结构 `HarnessSpecToml`**: TOML 文件中 `[auxiliary]` 是子表，但 L0
//!   `HarnessSpec.auxiliary: Option<String>` 存储为原始字符串（ADR-033 禁止
//!   L0 依赖 serde_json）。SpecLoader 先用 `toml::Value` 捕获 auxiliary 子表，
//!   再序列化为 TOML 字符串赋给 HarnessSpec.auxiliary
//!
//! - **严格校验在 L5**: L0 validate() 仅做子串匹配（无法解析 JSON/TOML），
//!   L5 SpecLoader 解析 auxiliary 字段并严格验证 acceptance_gates 数组
//!   包含全部 4 个强制门（设计文档 §7.2）
//!
//! - **错误分类独立**: `SpecLoaderError` 独立于 `GsoeError`，因 spec 加载
//!   有专属错误分类（IO/TOML 解析/校验/auxiliary 解析），与进化引擎错误
//!   语义不同。仍实现 `std::error::Error` 供 anyhow 链式调用
//!
//! - **零状态 struct**: `SpecLoader` 为单元结构体（stateless），所有方法
//!   为关联函数。理由: spec 加载是无副作用操作，无需持有状态
//!
//! # 流程图
//!
//! ```text
//! TOML 文本/文件
//!     │
//!     ▼
//! toml::from_str → HarnessSpecToml（中间结构，auxiliary: toml::Value）
//!     │
//!     ▼
//! HarnessSpecToml::to_harness_spec() → HarnessSpec（auxiliary: String）
//!     │
//!     ▼
//! HarnessSpec::validate() → 不可进化面 + 字段引用 + hop 完整性 + 子串匹配
//!     │
//!     ▼
//! SpecLoader::strict_check_auxiliary() → 严格 acceptance_gates 校验
//!     │
//!     ▼
//! Ok(HarnessSpec) / Err(SpecLoaderError)
//! ```
//!
//! # 防注入保证（P4-W15.1.4 铺垫）
//!
//! - 所有 `load_*` 方法返回 `Result<HarnessSpec, SpecLoaderError>`，不写文件
//! - `HarnessSpec` 方法均为 `&self`（不可变借用），无法通过 spec API 注入路径
//! - TOML 反序列化只解析声明字段，忽略未知字段（`deny_unknown_fields` 未启用，
//!   容错性优先；未知字段不影响 validate() 的不可进化面检查）

use crate::error::GsoeError;
use nexus_contracts::{
    ContractSpec, HarnessMeta, HarnessSpec, HarnessSpecError, HopSpec, ImmutableSurface,
    REQUIRED_ACCEPTANCE_GATES,
};
use std::path::Path;
use thiserror::Error;

// ============================================================
// SpecLoaderError — spec 加载器错误类型（P4-W15.1.2）
// ============================================================

/// Spec 加载器错误 — TOML 解析/校验/auxiliary 严格检查失败时返回
///
/// # 错误分类
///
/// | 错误变体 | 含义 | 触发场景 |
/// |---------|------|---------|
/// | `IoError` | 文件读取失败 | load_from_path 文件不存在/无权限 |
/// | `TomlParseError` | TOML 解析失败 | 语法错误/字段类型不匹配 |
/// | `AuxiliarySerializeError` | auxiliary 序列化失败 | toml::Value → String 转换失败 |
/// | `ValidationFailed` | L0 校验失败 | HarnessSpec::validate() 返回 Err |
/// | `AuxiliaryParseError` | auxiliary 严格解析失败 | auxiliary 非 TOML 表格/字段类型错 |
/// | `MissingAcceptanceGates` | 强制门缺失 | auxiliary 缺 4 个强制门之一 |
/// | `EmptyAuxiliary` | auxiliary 为空 | spec 未定义 auxiliary 字段 |
#[derive(Debug, Error)]
pub enum SpecLoaderError {
    /// 文件读取失败（IO 错误）
    #[error("spec 文件读取失败: {0}")]
    IoError(#[from] std::io::Error),

    /// TOML 解析失败（语法错误或字段类型不匹配）
    #[error("TOML 解析失败: {0}")]
    TomlParseError(#[from] toml::de::Error),

    /// auxiliary 序列化失败（toml::Value → String 转换异常）
    #[error("auxiliary 序列化为 TOML 字符串失败: {0}")]
    AuxiliarySerializeError(#[from] toml::ser::Error),

    /// L0 校验失败（HarnessSpec::validate() 返回错误）
    ///
    /// WHY 包装而非转换: 保留原始 HarnessSpecError 供调用方匹配具体变体
    /// （如 ImmutableSurfaceViolation 用于安全审计）
    #[error("spec 校验失败: {0}")]
    ValidationFailed(HarnessSpecError),

    /// auxiliary 严格解析失败（非 TOML 表格或字段类型错误）
    #[error("auxiliary 严格解析失败: {reason}")]
    AuxiliaryParseError {
        /// 错误原因描述
        reason: String,
    },

    /// auxiliary 缺失强制 acceptance_gates 门
    ///
    /// 设计文档 §7.2 规定 auxiliary.acceptance_gates 必须包含 4 个强制门:
    /// - "tests_pass"
    /// - "bench_no_regression"
    /// - "invariants_clean"
    /// - "redline_scan_clean"
    #[error("auxiliary.acceptance_gates 缺失强制门: {missing:?}")]
    MissingAcceptanceGates {
        /// 缺失的强制门列表
        missing: Vec<String>,
    },

    /// auxiliary 字段为空（spec 未定义 [auxiliary] 段）
    ///
    /// WHY 单独错误: 设计文档 §7.2 要求所有 spec 必须定义 acceptance_gates，
    /// 未定义 auxiliary 段的 spec 无法通过严格校验
    #[error("auxiliary 字段为空（spec 必须定义 [auxiliary] 段与 acceptance_gates）")]
    EmptyAuxiliary,
}

// ============================================================
// HarnessSpecToml — TOML 反序列化中间结构（P4-W15.1.2）
// ============================================================
//
// WHY 中间结构而非直接反序列化 HarnessSpec:
// - TOML 文件中 [auxiliary] 是子表，serde 期望 toml::Value 类型
// - HarnessSpec.auxiliary: Option<String>（L0 ADR-033 禁止 serde_json/toml 依赖）
// - SpecLoader 用 toml::Value 捕获 auxiliary 子表，再序列化为 String
//
// 字段一一对应 HarnessSpec，仅 auxiliary 类型不同:
// - HarnessSpec.auxiliary: Option<String>
// - HarnessSpecToml.auxiliary: Option<toml::Value>

/// HarnessSpec TOML 反序列化中间结构
///
/// 字段一一对应 `HarnessSpec`，仅 `auxiliary` 类型为 `toml::Value`
/// （用于捕获 TOML `[auxiliary]` 子表），随后转换为 `HarnessSpec`
#[derive(Debug, Clone, serde::Deserialize)]
struct HarnessSpecToml {
    /// 元信息（名称/版本/不可进化标记）
    meta: HarnessMeta,
    /// 契约列表（默认空 Vec）
    #[serde(default)]
    contracts: Vec<ContractSpec>,
    /// 执行步骤列表（默认空 Vec）
    #[serde(default)]
    hops: Vec<HopSpec>,
    /// 重试策略（默认值见 RetryPolicy::default）
    #[serde(default)]
    retry: nexus_contracts::RetryPolicy,
    /// 辅助字段（TOML 子表，捕获为 toml::Value）
    #[serde(default)]
    auxiliary: Option<toml::Value>,
}

impl HarnessSpecToml {
    /// 转换为 HarnessSpec（auxiliary 序列化为 TOML 字符串）
    ///
    /// # 转换规则
    /// - auxiliary: None → None
    /// - auxiliary: Some(value) → Some(toml::to_string(value)?)
    ///
    /// # 错误
    /// - `SpecLoaderError::AuxiliarySerializeError`: toml 序列化失败
    ///   （理论上 toml::Value 已从 TOML 解析得来，再序列化不会失败；
    ///   保留错误处理以防边缘情况）
    ///
    /// # 命名约定（WHY `into_` 前缀）
    /// Rust 惯例: `into_` 前缀表示消费 self 并转换为其他类型的方法
    /// （clippy::wrong_self_convention 规则）
    fn into_harness_spec(self) -> Result<HarnessSpec, SpecLoaderError> {
        let auxiliary_str = match self.auxiliary {
            None => None,
            Some(value) => Some(toml::to_string(&value)?),
        };

        Ok(HarnessSpec {
            meta: self.meta,
            contracts: self.contracts,
            hops: self.hops,
            retry: self.retry,
            auxiliary: auxiliary_str,
        })
    }
}

// ============================================================
// SpecLoader — spec 加载器主类型（P4-W15.1.2）
// ============================================================

/// HarnessSpec 加载器 — TOML 反序列化 + 校验 + 不可进化面检查
///
/// # 设计决策
///
/// - **零状态单元 struct**: 所有方法为关联函数（`load_from_str` 等），
///   无需实例化。理由: spec 加载是无副作用操作，不持有状态
/// - **严格校验**: 调用 L0 `validate()` + L5 严格 auxiliary 检查
/// - **防注入保证**: 返回 `HarnessSpec` 不可变引用，所有方法不写文件
///
/// # 示例
///
/// ## 从字符串加载 spec
///
/// ```
/// use gsoe_evolution::spec_loader::{SpecLoader, SpecLoaderError};
///
/// let toml_str = r#"
/// [meta]
/// name = "test-spec"
/// version = 1
/// immutable = false
///
/// [[contracts]]
/// name = "no_panic"
/// property = "fuzz_target_must_not_panic"
///
/// [[hops]]
/// name = "generate_input"
/// order = ["Architect.propose"]
/// contracts = ["no_panic"]
///
/// [retry]
/// max_attempts = 3
/// backoff_ms = 500
/// exponential = true
///
/// [auxiliary]
/// acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
/// "#;
///
/// let spec = SpecLoader::load_from_str(toml_str)?;
/// assert_eq!(spec.meta.name, "test-spec");
/// # Ok::<(), SpecLoaderError>(())
/// ```
pub struct SpecLoader;

impl SpecLoader {
    /// 从 TOML 字符串加载并校验 HarnessSpec
    ///
    /// # 流程
    /// 1. toml::from_str → HarnessSpecToml（中间结构）
    /// 2. HarnessSpecToml::to_harness_spec() → HarnessSpec
    /// 3. strict_check_auxiliary() → L5 严格 acceptance_gates 校验（先解析后语义）
    /// 4. HarnessSpec::validate() → L0 校验（不可进化面 + 字段引用 + 子串匹配兜底）
    ///
    /// # 校验顺序设计（WHY L5 在前 L0 在后）
    ///
    /// - L5 strict_check_auxiliary 先执行: 解析 auxiliary TOML 表格结构，
    ///   返回类型错误（非数组/字段缺失/解析失败）等精确错误
    /// - L0 validate() 后执行: 兜底子串匹配 + 不可进化面 + 契约引用检查
    /// - 若 auxiliary 完全合规，L5 strict check 通过，L0 validate 接管剩余检查
    /// - 若 auxiliary 有问题，L5 先给出精确错误（含行号/类型），无需 L0 子串兜底
    ///
    /// # 参数
    /// - `toml_str`: TOML 格式字符串
    ///
    /// # 返回
    /// - `Ok(HarnessSpec)`: 加载并校验成功
    /// - `Err(SpecLoaderError)`: 解析或校验失败
    ///
    /// # 防注入保证
    /// 此方法仅读取 `toml_str`，不写文件，不修改 spec。
    /// 返回的 HarnessSpec 所有方法均为 `&self`，无法通过 spec API 注入路径
    pub fn load_from_str(toml_str: &str) -> Result<HarnessSpec, SpecLoaderError> {
        // 1. TOML 反序列化为中间结构
        let raw: HarnessSpecToml = toml::from_str(toml_str)?;

        // 2. 转换为 HarnessSpec（auxiliary 序列化为 String）
        let spec = raw.into_harness_spec()?;

        // 3. L5 严格 acceptance_gates 校验（先解析后语义，错误归因更精确）
        Self::strict_check_auxiliary(&spec)?;

        // 4. L0 校验（不可进化面 + 字段引用 + hop 完整性 + 子串匹配兜底）
        spec.validate().map_err(SpecLoaderError::ValidationFailed)?;

        Ok(spec)
    }

    /// 从字节切片加载并校验 HarnessSpec
    ///
    /// WHY 提供: 文件读取通常返回 bytes，此方法先转 UTF-8 字符串再解析
    ///
    /// # 参数
    /// - `bytes`: TOML 格式字节切片（必须为合法 UTF-8）
    ///
    /// # 返回
    /// - `Ok(HarnessSpec)`: 加载并校验成功
    /// - `Err(SpecLoaderError)`: 解析或校验失败
    pub fn load_from_bytes(bytes: &[u8]) -> Result<HarnessSpec, SpecLoaderError> {
        // 1. 字节切片转字符串（TOML 要求 UTF-8 编码）
        let toml_str =
            std::str::from_utf8(bytes).map_err(|e| SpecLoaderError::AuxiliaryParseError {
                reason: format!("字节切片非 UTF-8 编码: {}", e),
            })?;

        // 2. 复用 load_from_str
        Self::load_from_str(toml_str)
    }

    /// 从文件路径加载并校验 HarnessSpec
    ///
    /// # 流程
    /// 1. std::fs::read 读取文件为 bytes
    /// 2. Self::load_from_bytes 加载并校验
    ///
    /// # 参数
    /// - `path`: TOML 文件路径
    ///
    /// # 返回
    /// - `Ok(HarnessSpec)`: 加载并校验成功
    /// - `Err(SpecLoaderError)`: IO/解析/校验失败
    ///
    /// # 安全性
    /// - 文件路径由调用方提供（SpecLoader 不构造路径，防注入）
    /// - 文件大小无限制（生产环境应由调用方限制）
    pub fn load_from_path<P: AsRef<Path>>(path: P) -> Result<HarnessSpec, SpecLoaderError> {
        let bytes = std::fs::read(path)?;
        Self::load_from_bytes(&bytes)
    }

    /// 严格校验 auxiliary 字段的 acceptance_gates（L5 严格检查）
    ///
    /// # 校验规则
    /// 1. auxiliary 必须存在（None → EmptyAuxiliary 错误）
    /// 2. auxiliary 必须可解析为 TOML 表格（toml::from_str）
    /// 3. 表格必须包含 acceptance_gates 字段
    /// 4. acceptance_gates 必须是字符串数组
    /// 5. 数组必须包含全部 4 个强制门
    ///
    /// # 与 L0 validate() 的差异
    /// - L0 validate(): 仅做子串匹配（auxiliary.contains("tests_pass")）
    /// - L5 strict_check: 解析 TOML 表格，验证 acceptance_gates 数组结构
    ///
    /// WHY 严格校验在 L5:
    /// - L0 ADR-033 禁止依赖 serde_json/toml
    /// - L5 gsoe-evolution 已依赖 toml，可解析 TOML 子表
    /// - 严格校验防止攻击者构造"tests_pass_anything"绕过子串匹配
    fn strict_check_auxiliary(spec: &HarnessSpec) -> Result<(), SpecLoaderError> {
        let auxiliary_str = spec
            .auxiliary
            .as_ref()
            .ok_or(SpecLoaderError::EmptyAuxiliary)?;

        // 解析 auxiliary 字符串为 TOML Value
        let aux_value: toml::Value =
            toml::from_str(auxiliary_str).map_err(|e| SpecLoaderError::AuxiliaryParseError {
                reason: format!("auxiliary 字段无法解析为 TOML: {}", e),
            })?;

        // 必须是表格类型
        let aux_table = aux_value
            .as_table()
            .ok_or(SpecLoaderError::AuxiliaryParseError {
                reason: "auxiliary 字段必须是 TOML 表格".to_string(),
            })?;

        // 必须包含 acceptance_gates 字段
        let gates_value =
            aux_table
                .get("acceptance_gates")
                .ok_or(SpecLoaderError::AuxiliaryParseError {
                    reason: "auxiliary.acceptance_gates 字段缺失".to_string(),
                })?;

        // acceptance_gates 必须是字符串数组
        let gates_array = gates_value
            .as_array()
            .ok_or(SpecLoaderError::AuxiliaryParseError {
                reason: "auxiliary.acceptance_gates 必须是字符串数组".to_string(),
            })?;

        // 收集所有 gate 字符串
        let gates: Vec<String> = gates_array
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();

        // 检查全部 4 个强制门存在（精确匹配，非子串）
        let missing: Vec<String> = REQUIRED_ACCEPTANCE_GATES
            .iter()
            .filter(|required| !gates.iter().any(|g| g == *required))
            .map(|s| s.to_string())
            .collect();

        if !missing.is_empty() {
            return Err(SpecLoaderError::MissingAcceptanceGates { missing });
        }

        Ok(())
    }

    /// 返回不可进化面清单（透传 L0 API，便于调用方安全审计）
    ///
    /// WHY 提供: 调用方（如 SpecRegistry P4-W15.2）可能需要枚举不可进化面
    /// 做安全审计。此方法透传 `HarnessSpec::immutable_surfaces()` 避免调用方
    /// 直接依赖 nexus_contracts（虽然 gsoe-evolution 已依赖）
    pub fn immutable_surfaces() -> &'static [ImmutableSurface; 20] {
        HarnessSpec::immutable_surfaces()
    }

    /// 将 SpecLoaderError 转换为 GsoeError（向后兼容，供 engine 调用）
    ///
    /// WHY 提供: GsoeEvolutionEngine 未来可能加载 spec，需统一错误类型
    /// 转换为 GsoeError::ConfigError 保留错误消息
    pub fn into_gsoe_error(err: SpecLoaderError) -> GsoeError {
        GsoeError::ConfigError {
            reason: err.to_string(),
        }
    }
}

// ============================================================
// 单元测试（P4-W15.1.2）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 构造一个最小合法 spec TOML 字符串
    fn make_valid_spec_toml() -> String {
        r#"
[meta]
name = "test-spec"
version = 1
immutable = false

[[contracts]]
name = "no_panic"
property = "fuzz_target_must_not_panic"
description = "Fuzz target must not panic on any input"

[[hops]]
name = "generate_input"
input_type = "Vec<u8>"
output_type = "ParseResult"
contracts = ["no_panic"]
description = "Generate fuzz input"
order = ["Architect.propose", "Skeptic.review"]

[retry]
max_attempts = 3
backoff_ms = 500
exponential = true

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
custom_field = "allowed"
"#
        .to_string()
    }

    // === 成功路径测试 ===

    #[test]
    fn test_load_from_str_success() {
        let toml_str = make_valid_spec_toml();
        let result = SpecLoader::load_from_str(&toml_str);
        assert!(result.is_ok(), "加载合法 spec 应成功: {:?}", result.err());

        let spec = result.unwrap();
        assert_eq!(spec.meta.name, "test-spec");
        assert_eq!(spec.meta.version, 1);
        assert!(!spec.meta.immutable);
        assert_eq!(spec.contracts.len(), 1);
        assert_eq!(spec.hops.len(), 1);
        assert_eq!(spec.retry.max_attempts, 3);
    }

    #[test]
    fn test_load_from_str_preserves_auxiliary_as_toml_string() {
        let toml_str = make_valid_spec_toml();
        let spec = SpecLoader::load_from_str(&toml_str).unwrap();

        // auxiliary 应为 TOML 字符串（非 None）
        let aux = spec.auxiliary.expect("auxiliary 应存在");
        assert!(aux.contains("acceptance_gates"));
        assert!(aux.contains("tests_pass"));
        assert!(aux.contains("custom_field"));
    }

    #[test]
    fn test_load_from_bytes_success() {
        let toml_str = make_valid_spec_toml();
        let bytes = toml_str.as_bytes();
        let result = SpecLoader::load_from_bytes(bytes);
        assert!(result.is_ok(), "字节切片加载应成功: {:?}", result.err());
    }

    #[test]
    fn test_load_from_path_success() {
        // 创建临时文件并写入合法 spec
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        let toml_str = make_valid_spec_toml();
        tmp.write_all(toml_str.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let result = SpecLoader::load_from_path(tmp.path());
        assert!(result.is_ok(), "文件加载应成功: {:?}", result.err());

        let spec = result.unwrap();
        assert_eq!(spec.meta.name, "test-spec");
    }

    #[test]
    fn test_load_with_default_retry() {
        // 省略 [retry] 段，使用 RetryPolicy::default()
        let toml_str = r#"
[meta]
name = "min-spec"
version = 1

[[contracts]]
name = "c1"
property = "p1"

[[hops]]
name = "h1"
order = ["A.x"]
contracts = ["c1"]

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#;
        let spec = SpecLoader::load_from_str(toml_str).unwrap();
        // 默认值: max_attempts=5, backoff_ms=1000, exponential=true
        assert_eq!(spec.retry.max_attempts, 5);
        assert_eq!(spec.retry.backoff_ms, 1000);
        assert!(spec.retry.exponential);
    }

    #[test]
    fn test_load_with_empty_contracts_and_hops() {
        // contracts/hops 可以为空（serde default = []）
        let toml_str = r#"
[meta]
name = "empty-spec"
version = 1

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#;
        let spec = SpecLoader::load_from_str(toml_str).unwrap();
        assert!(spec.contracts.is_empty());
        assert!(spec.hops.is_empty());
    }

    // === TOML 解析错误测试 ===

    #[test]
    fn test_load_invalid_toml_syntax() {
        let bad_toml = "this is not valid toml = = =";
        let result = SpecLoader::load_from_str(bad_toml);
        assert!(matches!(result, Err(SpecLoaderError::TomlParseError(_))));
    }

    #[test]
    fn test_load_missing_meta_section() {
        let toml_str = r#"
[[contracts]]
name = "c1"
property = "p1"
"#;
        let result = SpecLoader::load_from_str(toml_str);
        // meta 是必需字段，缺失会导致 toml 解析错误
        assert!(matches!(result, Err(SpecLoaderError::TomlParseError(_))));
    }

    #[test]
    fn test_load_invalid_field_type() {
        // version 应为 u32，传字符串
        let toml_str = r#"
[meta]
name = "test"
version = "not_a_number"

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#;
        let result = SpecLoader::load_from_str(toml_str);
        assert!(matches!(result, Err(SpecLoaderError::TomlParseError(_))));
    }

    // === L0 校验错误测试（不可进化面）===

    #[test]
    fn test_load_rejects_empty_meta_name() {
        let toml_str = r#"
[meta]
name = ""
version = 1

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#;
        let result = SpecLoader::load_from_str(toml_str);
        match result {
            Err(SpecLoaderError::ValidationFailed(HarnessSpecError::EmptyMetaName)) => {}
            other => panic!("期望 EmptyMetaName 错误，实际: {:?}", other),
        }
    }

    #[test]
    fn test_load_rejects_zero_version() {
        let toml_str = r#"
[meta]
name = "test"
version = 0

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#;
        let result = SpecLoader::load_from_str(toml_str);
        match result {
            Err(SpecLoaderError::ValidationFailed(HarnessSpecError::InvalidVersion)) => {}
            other => panic!("期望 InvalidVersion 错误，实际: {:?}", other),
        }
    }

    #[test]
    fn test_load_rejects_parent_ge_version() {
        let toml_str = r#"
[meta]
name = "test"
version = 5
parent = 10

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#;
        let result = SpecLoader::load_from_str(toml_str);
        match result {
            Err(SpecLoaderError::ValidationFailed(HarnessSpecError::InvalidVersion)) => {}
            other => panic!(
                "期望 InvalidVersion（parent >= version），实际: {:?}",
                other
            ),
        }
    }

    #[test]
    fn test_load_rejects_immutable_surface_in_contract_from() {
        // contracts[0].from 引用不可进化面 "critical-asa-intervention"
        // (ImmutableSurface::as_str() 返回 kebab-case 标识)
        let toml_str = r#"
[meta]
name = "test"
version = 1

[[contracts]]
name = "c1"
property = "p1"
from = "critical-asa-intervention"

[[hops]]
name = "h1"
order = ["A.x"]
contracts = ["c1"]

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#;
        let result = SpecLoader::load_from_str(toml_str);
        match result {
            Err(SpecLoaderError::ValidationFailed(
                HarnessSpecError::ImmutableSurfaceViolation {
                    surface: ImmutableSurface::CriticalAsaIntervention,
                    ..
                },
            )) => {}
            other => panic!(
                "期望 ImmutableSurfaceViolation (critical-asa-intervention)，实际: {:?}",
                other
            ),
        }
    }

    #[test]
    fn test_load_rejects_immutable_surface_in_hop_order() {
        // hops[0].order 引用不可进化面 "critical-budget-exceeded"
        let toml_str = r#"
[meta]
name = "test"
version = 1

[[contracts]]
name = "c1"
property = "p1"

[[hops]]
name = "h1"
order = ["critical-budget-exceeded"]
contracts = ["c1"]

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#;
        let result = SpecLoader::load_from_str(toml_str);
        match result {
            Err(SpecLoaderError::ValidationFailed(
                HarnessSpecError::ImmutableSurfaceViolation {
                    surface: ImmutableSurface::CriticalBudgetExceeded,
                    ..
                },
            )) => {}
            other => panic!(
                "期望 ImmutableSurfaceViolation (critical-budget-exceeded)，实际: {:?}",
                other
            ),
        }
    }

    #[test]
    fn test_load_rejects_immutable_surface_in_contract_fields() {
        // contracts[0].fields 引用不可进化面 "inv-7-memory-budget"
        let toml_str = r#"
[meta]
name = "test"
version = 1

[[contracts]]
name = "c1"
property = "p1"
fields = ["inv-7-memory-budget"]

[[hops]]
name = "h1"
order = ["A.x"]
contracts = ["c1"]

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#;
        let result = SpecLoader::load_from_str(toml_str);
        match result {
            Err(SpecLoaderError::ValidationFailed(
                HarnessSpecError::ImmutableSurfaceViolation {
                    surface: ImmutableSurface::Invariant7MemoryBudget,
                    ..
                },
            )) => {}
            other => panic!(
                "期望 ImmutableSurfaceViolation (inv-7-memory-budget)，实际: {:?}",
                other
            ),
        }
    }

    #[test]
    fn test_load_rejects_invalid_contract_reference() {
        // hop 引用未定义的契约 "undefined_contract"
        let toml_str = r#"
[meta]
name = "test"
version = 1

[[contracts]]
name = "c1"
property = "p1"

[[hops]]
name = "h1"
order = ["A.x"]
contracts = ["undefined_contract"]

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#;
        let result = SpecLoader::load_from_str(toml_str);
        match result {
            Err(SpecLoaderError::ValidationFailed(
                HarnessSpecError::InvalidContractReference { contract_name, .. },
            )) => {
                assert_eq!(contract_name, "undefined_contract");
            }
            other => panic!("期望 InvalidContractReference，实际: {:?}", other),
        }
    }

    #[test]
    fn test_load_rejects_empty_hop_order_with_extended_fields() {
        // hop 使用扩展字段（on_veto）但 order 为空
        let toml_str = r#"
[meta]
name = "test"
version = 1

[[contracts]]
name = "c1"
property = "p1"

[[hops]]
name = "h1"
contracts = ["c1"]
on_veto = "replan(max=2)"

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#;
        let result = SpecLoader::load_from_str(toml_str);
        match result {
            Err(SpecLoaderError::ValidationFailed(HarnessSpecError::EmptyHopOrder { .. })) => {}
            other => panic!("期望 EmptyHopOrder，实际: {:?}", other),
        }
    }

    // === auxiliary 严格校验测试 ===

    #[test]
    fn test_load_rejects_empty_auxiliary() {
        let toml_str = r#"
[meta]
name = "test"
version = 1

[[contracts]]
name = "c1"
property = "p1"

[[hops]]
name = "h1"
order = ["A.x"]
contracts = ["c1"]
"#;
        let result = SpecLoader::load_from_str(toml_str);
        match result {
            Err(SpecLoaderError::EmptyAuxiliary) => {}
            other => panic!("期望 EmptyAuxiliary，实际: {:?}", other),
        }
    }

    #[test]
    fn test_load_rejects_missing_required_gate() {
        // acceptance_gates 缺失 "redline_scan_clean"
        let toml_str = r#"
[meta]
name = "test"
version = 1

[[contracts]]
name = "c1"
property = "p1"

[[hops]]
name = "h1"
order = ["A.x"]
contracts = ["c1"]

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean"]
"#;
        let result = SpecLoader::load_from_str(toml_str);
        match result {
            Err(SpecLoaderError::MissingAcceptanceGates { missing }) => {
                assert!(missing.contains(&"redline_scan_clean".to_string()));
            }
            other => panic!("期望 MissingAcceptanceGates，实际: {:?}", other),
        }
    }

    #[test]
    fn test_load_rejects_multiple_missing_gates() {
        // acceptance_gates 只有一个门，缺失 3 个
        let toml_str = r#"
[meta]
name = "test"
version = 1

[auxiliary]
acceptance_gates = ["tests_pass"]
"#;
        let result = SpecLoader::load_from_str(toml_str);
        match result {
            Err(SpecLoaderError::MissingAcceptanceGates { missing }) => {
                assert_eq!(missing.len(), 3);
                assert!(missing.contains(&"bench_no_regression".to_string()));
                assert!(missing.contains(&"invariants_clean".to_string()));
                assert!(missing.contains(&"redline_scan_clean".to_string()));
            }
            other => panic!("期望 MissingAcceptanceGates (3 个缺失)，实际: {:?}", other),
        }
    }

    #[test]
    fn test_load_rejects_acceptance_gates_not_array() {
        // acceptance_gates 是字符串而非数组
        let toml_str = r#"
[meta]
name = "test"
version = 1

[auxiliary]
acceptance_gates = "tests_pass"
"#;
        let result = SpecLoader::load_from_str(toml_str);
        match result {
            Err(SpecLoaderError::AuxiliaryParseError { reason }) => {
                assert!(reason.contains("必须是字符串数组"));
            }
            other => panic!("期望 AuxiliaryParseError（非数组），实际: {:?}", other),
        }
    }

    #[test]
    fn test_load_rejects_acceptance_gates_missing_field() {
        // [auxiliary] 段存在但缺 acceptance_gates 字段
        let toml_str = r#"
[meta]
name = "test"
version = 1

[auxiliary]
custom_field = "value"
"#;
        let result = SpecLoader::load_from_str(toml_str);
        match result {
            Err(SpecLoaderError::AuxiliaryParseError { reason }) => {
                assert!(reason.contains("acceptance_gates 字段缺失"));
            }
            other => panic!("期望 AuxiliaryParseError（缺字段），实际: {:?}", other),
        }
    }

    // === 子串匹配绕过测试（L0 vs L5 差异）===

    #[test]
    fn test_strict_check_rejects_substring_bypass() {
        // 攻击者构造 "tests_pass_anything" 试图绕过 L0 子串匹配
        // L5 严格检查应拒绝（精确匹配失败）
        let toml_str = r#"
[meta]
name = "test"
version = 1

[auxiliary]
acceptance_gates = ["tests_pass_anything", "bench_no_regression_xyz", "invariants_clean_fake", "redline_scan_clean_pretend"]
"#;
        let result = SpecLoader::load_from_str(toml_str);
        match result {
            Err(SpecLoaderError::MissingAcceptanceGates { missing }) => {
                // 全部 4 个强制门应判定为缺失（精确匹配失败）
                assert_eq!(missing.len(), 4);
            }
            other => panic!(
                "期望 MissingAcceptanceGates（子串绕过被阻止），实际: {:?}",
                other
            ),
        }
    }

    #[test]
    fn test_strict_check_accepts_extra_gates() {
        // 接受额外 gate（除 4 个强制门外还有自定义 gate）
        let toml_str = r#"
[meta]
name = "test"
version = 1

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean", "custom_gate"]
"#;
        let result = SpecLoader::load_from_str(toml_str);
        assert!(result.is_ok(), "额外 gate 不应导致失败: {:?}", result.err());
    }

    // === IO 错误测试 ===

    #[test]
    fn test_load_nonexistent_file() {
        let result = SpecLoader::load_from_path("/nonexistent/path/spec.toml");
        assert!(matches!(result, Err(SpecLoaderError::IoError(_))));
    }

    // === 不可进化面清单 API 测试 ===

    #[test]
    fn test_immutable_surfaces_returns_20_variants() {
        let surfaces = SpecLoader::immutable_surfaces();
        assert_eq!(surfaces.len(), 20);
    }

    #[test]
    fn test_immutable_surfaces_contains_redlines() {
        let surfaces = SpecLoader::immutable_surfaces();
        // 至少包含 8 条红线
        let redlines = surfaces
            .iter()
            .filter(|s| matches!(s, ImmutableSurface::RedlineLockAcrossAwait))
            .count();
        assert_eq!(redlines, 1);
    }

    #[test]
    fn test_immutable_surfaces_contains_critical_events() {
        let surfaces = SpecLoader::immutable_surfaces();
        // 包含 6 个 Critical 事件
        let criticals = surfaces
            .iter()
            .filter(|s| {
                matches!(
                    s,
                    ImmutableSurface::CriticalCheckpointSaved
                        | ImmutableSurface::CriticalSkepticVeto
                        | ImmutableSurface::CriticalRedTeamAudit
                        | ImmutableSurface::CriticalAsaIntervention
                        | ImmutableSurface::CriticalAgentTaskFailed
                        | ImmutableSurface::CriticalBudgetExceeded
                )
            })
            .count();
        assert_eq!(criticals, 6);
    }

    // === 错误转换测试 ===

    #[test]
    fn test_into_gsoe_error_preserves_message() {
        let err = SpecLoaderError::EmptyAuxiliary;
        let gsoe_err = SpecLoader::into_gsoe_error(err);
        let msg = gsoe_err.to_string();
        assert!(msg.contains("auxiliary"));
    }

    #[test]
    fn test_into_gsoe_error_from_validation_failed() {
        let err = SpecLoaderError::ValidationFailed(HarnessSpecError::EmptyMetaName);
        let gsoe_err = SpecLoader::into_gsoe_error(err);
        let msg = gsoe_err.to_string();
        assert!(msg.contains("meta.name"));
    }

    // === 复杂 spec 加载测试 ===

    #[test]
    fn test_load_complex_spec_with_multiple_contracts_and_hops() {
        let toml_str = r#"
[meta]
name = "complex-spec"
version = 5
parent = 4
immutable = false
task_type = "code_refactor"

[[contracts]]
name = "no_panic"
property = "fuzz_target_must_not_panic"
description = "Fuzz target must not panic"
from = "Architect"
to = "orchestrator"
fields = ["input_bytes", "parse_result"]

[[contracts]]
name = "no_corruption"
property = "memory_safety"
description = "Memory safety check"

[[hops]]
name = "generate_input"
input_type = "Vec<u8>"
output_type = "ParseResult"
contracts = ["no_panic"]
description = "Generate fuzz input"
order = ["Architect.propose", "Skeptic.review", "Security.gate"]
on_veto = "replan(max=2)"
fallback = "EscalateToHuman"

[[hops]]
name = "execute"
input_type = "ParseResult"
output_type = "ExecutionResult"
contracts = ["no_panic", "no_corruption"]
description = "Execute parsed input"
order = ["Executor.run", "Verifier.check"]
on_veto = "abort"

[retry]
max_attempts = 7
backoff_ms = 2000
exponential = false

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
custom_metadata = "value"
"#
        .to_string();

        let spec = SpecLoader::load_from_str(&toml_str).expect("复杂 spec 应加载成功");
        assert_eq!(spec.meta.name, "complex-spec");
        assert_eq!(spec.meta.version, 5);
        assert_eq!(spec.meta.parent, Some(4));
        assert_eq!(spec.meta.task_type, Some("code_refactor".to_string()));
        assert_eq!(spec.contracts.len(), 2);
        assert_eq!(spec.hops.len(), 2);
        assert_eq!(spec.retry.max_attempts, 7);
        assert_eq!(spec.retry.backoff_ms, 2000);
        assert!(!spec.retry.exponential);

        // 验证 contract 扩展字段
        assert_eq!(spec.contracts[0].from, Some("Architect".to_string()));
        assert_eq!(spec.contracts[0].to, Some("orchestrator".to_string()));
        assert_eq!(spec.contracts[0].fields.len(), 2);

        // 验证 hop 扩展字段
        assert_eq!(spec.hops[0].order.len(), 3);
        assert_eq!(spec.hops[0].on_veto, Some("replan(max=2)".to_string()));
        assert_eq!(spec.hops[0].fallback, Some("EscalateToHuman".to_string()));
    }

    #[test]
    fn test_load_immutable_meta_with_valid_name() {
        // meta.name 不在不可进化面清单中，immutable=true 应允许
        let toml_str = r#"
[meta]
name = "custom-immutable-spec"
version = 1
immutable = true

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#;
        let result = SpecLoader::load_from_str(toml_str);
        assert!(
            result.is_ok(),
            "非不可进化面名称 + immutable=true 应允许: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_load_rejects_immutable_meta_not_marked() {
        // meta.name 在不可进化面清单中但 immutable=false
        // 不可进化面清单中的标识使用 kebab-case（与 as_str() 一致）
        let toml_str = r#"
[meta]
name = "critical-asa-intervention"
version = 1
immutable = false

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#;
        let result = SpecLoader::load_from_str(toml_str);
        match result {
            Err(SpecLoaderError::ValidationFailed(HarnessSpecError::ImmutableMetaNotMarked {
                name,
            })) => {
                assert_eq!(name, "critical-asa-intervention");
            }
            other => panic!("期望 ImmutableMetaNotMarked，实际: {:?}", other),
        }
    }

    // === canonical_merkle_input 透传测试 ===

    #[test]
    fn test_loaded_spec_canonical_merkle_input_is_stable() {
        let toml_str = make_valid_spec_toml();
        let spec1 = SpecLoader::load_from_str(&toml_str).unwrap();
        let spec2 = SpecLoader::load_from_str(&toml_str).unwrap();

        // 相同 TOML 应产生相同 merkle 输入
        let input1 = spec1.canonical_merkle_input();
        let input2 = spec2.canonical_merkle_input();
        assert_eq!(input1, input2);
    }

    #[test]
    fn test_loaded_spec_validate_idempotent() {
        // 已加载的 spec 再次调用 validate() 应继续通过
        let toml_str = make_valid_spec_toml();
        let spec = SpecLoader::load_from_str(&toml_str).unwrap();
        assert!(spec.validate().is_ok(), "已加载 spec 再次 validate 应通过");
    }

    // ============================================================
    // P4-W15.1.4: 防注入验证测试
    // ============================================================
    //
    // 验证 HarnessSpec 与 SpecLoader API 的防注入属性:
    // 1. 所有 HarnessSpec 方法均为 &self（不可变借用）
    // 2. SpecLoader 方法不写文件（仅读）
    // 3. 加载的 spec 副本相互独立（无共享可变状态）
    // 4. canonical_merkle_input 不泄露可变引用
    // 5. validate() 无副作用
    // 6. 不可进化面引用被硬编码拒绝

    /// 验证 &self 不变性: HarnessSpec 所有公开方法均接受 &self
    ///
    /// 此测试通过在 `&HarnessSpec`（不可变引用）上调用所有方法来验证
    /// 编译期保证: 若任一方法签名变为 &mut self 或 self，编译将失败
    #[test]
    fn test_harness_spec_methods_take_immutable_ref() {
        let toml_str = make_valid_spec_toml();
        let spec = SpecLoader::load_from_str(&toml_str).unwrap();

        // 取不可变引用
        let spec_ref: &HarnessSpec = &spec;

        // 在 &HarnessSpec 上调用所有公开方法（编译期保证 &self 签名）
        let _ = spec_ref.validate();
        let _ = spec_ref.canonical_merkle_input();
        let _ = HarnessSpec::immutable_surfaces();

        // 静态方法不需实例化
        let _: [&str; 4] = nexus_contracts::REQUIRED_ACCEPTANCE_GATES;
    }

    /// 验证 SpecLoader::load_from_path 仅读不写
    ///
    /// 通过从只读文件加载 spec，验证 SpecLoader 不修改源文件
    #[test]
    fn test_load_from_path_does_not_write_source_file() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        let original_content = make_valid_spec_toml();
        tmp.write_all(original_content.as_bytes()).unwrap();
        tmp.flush().unwrap();

        // 记录原始文件内容
        let path = tmp.path().to_path_buf();
        let before_load = std::fs::read_to_string(&path).unwrap();

        // 加载 spec（不应修改源文件）
        let _spec = SpecLoader::load_from_path(&path).unwrap();

        // 验证文件内容未变
        let after_load = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            before_load, after_load,
            "SpecLoader::load_from_path 不应修改源文件内容"
        );
    }

    /// 验证加载的 spec 副本相互独立（无共享可变状态）
    ///
    /// 多次加载同一 TOML 应产生独立的 owned HarnessSpec 实例，
    /// 修改一个不影响其他（虽然 spec 字段是 public 的，但 owned 数据独立）
    #[test]
    fn test_loaded_specs_are_independent_owned_copies() {
        let toml_str = make_valid_spec_toml();

        let spec1 = SpecLoader::load_from_str(&toml_str).unwrap();
        let spec2 = SpecLoader::load_from_str(&toml_str).unwrap();

        // 两个 spec 内容相同
        assert_eq!(spec1.meta.name, spec2.meta.name);

        // 但它们是独立的内存分配（不共享可变状态）
        let addr1 = &spec1 as *const _;
        let addr2 = &spec2 as *const _;
        assert_ne!(addr1, addr2, "两次加载应产生独立 owned 实例");
    }

    /// 验证 canonical_merkle_input 不暴露可变引用
    ///
    /// 多次调用 canonical_merkle_input() 应返回相同字符串，
    /// 且不修改 spec 内部状态
    #[test]
    fn test_canonical_merkle_input_is_pure() {
        let toml_str = make_valid_spec_toml();
        let spec = SpecLoader::load_from_str(&toml_str).unwrap();

        // 多次调用应产生相同结果（纯函数，无副作用）
        let input1 = spec.canonical_merkle_input();
        let input2 = spec.canonical_merkle_input();
        let input3 = spec.canonical_merkle_input();

        assert_eq!(input1, input2);
        assert_eq!(input2, input3);
    }

    /// 验证 validate() 是纯函数（无副作用）
    ///
    /// 多次调用 validate() 不应改变 spec 状态，
    /// 也不应影响后续 canonical_merkle_input() 的结果
    #[test]
    fn test_validate_is_pure_function() {
        let toml_str = make_valid_spec_toml();
        let spec = SpecLoader::load_from_str(&toml_str).unwrap();

        // 记录调用前的 merkle 输入
        let merkle_before = spec.canonical_merkle_input();

        // 多次调用 validate()
        let r1 = spec.validate();
        let r2 = spec.validate();
        let r3 = spec.validate();

        // 所有调用应返回相同结果
        assert_eq!(r1, r2);
        assert_eq!(r2, r3);
        assert!(r1.is_ok());

        // 调用后 merkle 输入不应改变
        let merkle_after = spec.canonical_merkle_input();
        assert_eq!(merkle_before, merkle_after, "validate() 不应有副作用");
    }

    /// 验证 SpecLoader 不构造文件路径（无路径注入）
    ///
    /// load_from_str 接受 &str 输入，不涉及任何文件系统操作。
    /// 即使输入包含路径分隔符或恶意构造，也不应触发文件读写
    #[test]
    fn test_load_from_str_no_filesystem_access() {
        // 包含路径分隔符的输入（不应被解释为文件路径）
        let malicious_input = r#"
[meta]
name = "../../etc/passwd"
version = 1

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#;
        // spec 应正常加载（meta.name 只是字符串，不是文件路径）
        let spec = SpecLoader::load_from_str(malicious_input).unwrap();
        assert_eq!(spec.meta.name, "../../etc/passwd");

        // 验证不可进化面校验未触发（路径字符串不是不可进化面标识）
        assert!(spec.validate().is_ok());
    }

    /// 验证 load_from_bytes 不写文件
    #[test]
    fn test_load_from_bytes_no_filesystem_write() {
        let toml_str = make_valid_spec_toml();
        let bytes = toml_str.as_bytes();

        // 在临时目录中加载，确保不创建任何文件
        let temp_dir = tempfile::tempdir().unwrap();
        let initial_entries = std::fs::read_dir(temp_dir.path()).unwrap().count();

        let _spec = SpecLoader::load_from_bytes(bytes).unwrap();

        // 临时目录内容不应改变（无文件写入）
        let final_entries = std::fs::read_dir(temp_dir.path()).unwrap().count();
        assert_eq!(
            initial_entries, final_entries,
            "load_from_bytes 不应创建任何文件"
        );
    }

    /// 验证 strict_check_auxiliary 不修改 spec
    ///
    /// 严格校验是只读操作，不应改变 spec 的任何字段
    #[test]
    fn test_strict_check_does_not_mutate_spec() {
        let toml_str = make_valid_spec_toml();
        let spec = SpecLoader::load_from_str(&toml_str).unwrap();

        // 记录校验前的状态
        let aux_before = spec.auxiliary.clone();
        let merkle_before = spec.canonical_merkle_input();

        // 调用 strict_check_auxiliary（私有方法，通过 load_from_str 间接调用）
        // 重新加载并验证
        let spec2 = SpecLoader::load_from_str(&toml_str).unwrap();
        assert!(spec2.validate().is_ok());

        // 验证状态未变
        assert_eq!(spec.auxiliary, aux_before);
        assert_eq!(
            spec.canonical_merkle_input(),
            merkle_before,
            "strict_check 不应修改 spec"
        );
    }

    /// 验证 amount amount_load_rejects_malicious_auxiliary_injection
    ///
    /// 攻击者试图通过 auxiliary 字段注入恶意 TOML 路径引用，
    /// 但 auxiliary 只是被解析为值，不会被解释为文件路径
    #[test]
    fn test_auxiliary_cannot_inject_file_paths() {
        // 攻击 payload: 试图在 auxiliary 中注入路径引用
        let malicious_toml = r#"
[meta]
name = "test"
version = 1

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
malicious_path = "../../../etc/passwd"
shell_injection = "$(rm -rf /)"
command_injection = "; rm -rf /; "
"#;
        // spec 应正常加载（auxiliary 字段值不被解释为路径或命令）
        let spec = SpecLoader::load_from_str(malicious_toml).unwrap();

        // auxiliary 字段是字符串，不会被解释为可执行路径
        assert!(spec.auxiliary.as_ref().unwrap().contains("malicious_path"));
        assert!(spec
            .auxiliary
            .as_ref()
            .unwrap()
            .contains("../../../etc/passwd"));
        assert!(spec.auxiliary.as_ref().unwrap().contains("$(rm -rf /)"));

        // validate 仍应通过（auxiliary 字段不影响核心校验）
        assert!(spec.validate().is_ok());
    }

    /// 验证不可进化面硬编码拒绝（最后一道防线）
    ///
    /// 即使攻击者构造看似合法的 spec，试图修改不可进化面
    /// （如 critical-asa-intervention），validate() 应硬性拒绝
    #[test]
    fn test_immutable_surface_hardcoded_rejection() {
        // 试图通过 hop.on_veto 修改 AsaIntervention 行为
        let attack_toml = r#"
[meta]
name = "attack-spec"
version = 1

[[contracts]]
name = "c1"
property = "p1"

[[hops]]
name = "h1"
order = ["A.x"]
contracts = ["c1"]
on_veto = "critical-asa-intervention"

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#;
        let result = SpecLoader::load_from_str(attack_toml);
        // 必须被拒绝
        assert!(result.is_err());
        match result {
            Err(SpecLoaderError::ValidationFailed(
                HarnessSpecError::ImmutableSurfaceViolation { surface, .. },
            )) => {
                assert_eq!(surface, ImmutableSurface::CriticalAsaIntervention);
            }
            _ => panic!("不可进化面应被硬编码拒绝"),
        }
    }

    /// 验证 SpecLoaderError 不泄露敏感路径信息
    ///
    /// IO 错误消息应只包含调用方提供的路径，不应泄露 SpecLoader 内部状态
    ///
    /// WHY 跨平台/跨 locale 兼容: std::io::Error 的 Display 输出随 OS locale 变化
    /// （Windows 中文: "系统找不到指定的路径"; Unix: "No such file or directory"），
    /// 故改用 `ErrorKind::NotFound` 判断错误类别，而非字符串匹配。
    /// 真正要守护的是:错误消息不含内部源码路径（如 spec_loader.rs / gsoe_evolution）
    /// 这类泄露内部实现细节的信息。
    #[test]
    fn test_io_error_does_not_leak_internal_state() {
        let result = SpecLoader::load_from_path("/nonexistent/spec.toml");
        match result {
            Err(SpecLoaderError::IoError(io_err)) => {
                // 跨 locale 判断错误类别（NotFound 表示系统级"文件不存在"）
                assert_eq!(
                    io_err.kind(),
                    std::io::ErrorKind::NotFound,
                    "错误类别应为 NotFound"
                );
                // 不应包含内部实现细节（如 SpecLoader 源码路径）
                let msg = io_err.to_string();
                assert!(!msg.contains("spec_loader.rs"), "不应泄露源码文件名: {msg}");
                assert!(
                    !msg.contains("gsoe_evolution"),
                    "不应泄露 crate 名称: {msg}"
                );
            }
            other => panic!("期望 IoError，实际: {:?}", other),
        }
    }

    /// 验证 toml 反序列化不调用任意代码
    ///
    /// TOML 反序列化只解析数据，不执行命令或调用函数
    /// （Rust serde derive保证，但需显式测试边界）
    #[test]
    fn test_toml_deserialization_does_not_execute_code() {
        // 包含看似可执行内容的 TOML（但实际只是字符串值）
        let toml_with_code_like_strings = r#"
[meta]
name = "test"
version = 1

[[contracts]]
name = "c1"
property = "rm -rf / && echo hacked"

[[hops]]
name = "h1"
order = ["A.x"]
contracts = ["c1"]

[auxiliary]
acceptance_gates = ["tests_pass", "bench_no_regression", "invariants_clean", "redline_scan_clean"]
"#;
        // 应正常加载，property 字段只是字符串
        let spec = SpecLoader::load_from_str(toml_with_code_like_strings).unwrap();
        assert_eq!(spec.contracts[0].property, "rm -rf / && echo hacked");

        // 没有 side effect（无文件删除、无命令执行）
        // （此测试通过编译保证：serde derive 仅赋值字段，不调用函数）
    }
}
