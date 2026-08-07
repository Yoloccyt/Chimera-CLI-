//! spec 加载器 — affinity.d/*.toml 厂商描述符装载与校验(原则 P8 元数据外置)
//!
//! 厂商端点、模型清单、定价、能力描述符全部存于 TOML 配置:厂商调价/改名/
//! 发新模型只改 TOML 不发版,代码零厂商字符串。
//!
//! # 文件格式
//! ```toml
//! schema_version = 1
//!
//! [[models]]
//! provider = "zhipu"
//! model = "glm-5.2"
//! dialects = ["open_ai_chat", "anthropic_messages"]
//! # ... ModelAffinitySpec 全字段(serde 直映射)
//! ```
//!
//! # 部署 Profile(ADR-072 决策 ⑧)
//! 自部署通道的服务端能力经 `profiles/deployment/*.toml` 下发:
//! - `[server_params]`: 服务端部署参数(PagedAttention/KV 量化等),
//!   **零解析零执行**——仅 schema 校验(存在性与类型),不进入客户端逻辑
//! - `[client_relevant]`: 客户端可消费覆盖项(context_window / prompt_caching)
//! - spec 卡片顶层 `deployment_profile = "<id>"` 引用 Profile,加载时应用
//!
//! # 校验策略(系统边界,快速失败)
//! TOML 是系统边界输入(用户可编辑),在此做全部校验:schema 版本、
//! 端点非空、方言非空、废弃模型名拒绝(DeprecatedModelNames 怪癖)。
//! 闭集枚举(ProviderId/QuirkRule)拼错在 serde 反序列化即失败。

use std::collections::HashMap;
use std::path::Path;

use nexus_contracts::affinity::{CacheSupport, ModelAffinitySpec, QuirkRule};
use serde::Deserialize;

use crate::error::AffinityError;

/// 当前支持的 spec 文件 schema 版本
///
/// WHY 显式版本号: 未来字段演进时凭版本号做迁移/拒绝决策,
/// 避免"旧网关静默误读新格式"的隐性故障。
pub const SPEC_SCHEMA_VERSION: u32 = 1;

/// spec 文件顶层结构(一个 TOML 文件 = 一个厂商的多张模型卡)
#[derive(Debug, Deserialize)]
struct SpecFile {
    /// schema 版本(必填,不匹配即拒绝)
    schema_version: u32,
    /// 部署 Profile 引用(可选;引用 profiles/deployment/<id>.toml,
    /// ADR-072 决策 ⑧;该厂商全部模型卡共享此 Profile)
    #[serde(default)]
    deployment_profile: Option<String>,
    /// 该厂商的模型描述符列表
    #[serde(default)]
    models: Vec<ModelAffinitySpec>,
}

/// 部署 Profile 元信息 — 标识与通道归属(仅校验,不参与客户端逻辑)
#[derive(Debug, Clone, Deserialize)]
pub struct ProfileMeta {
    /// Profile 唯一标识(spec 卡片 deployment_profile 引用的 id)
    pub id: String,
    /// 通道归属(对应 affinity.d 卡片 provider 名;仅文档语义)
    #[serde(default)]
    pub channel: String,
    /// 推理引擎(vllm/sglang 等;仅文档语义)
    #[serde(default)]
    pub engine: String,
}

/// 客户端可消费覆盖项 — 服务端能力在客户端侧的等价表达
///
/// 仅此段进入客户端逻辑;`server_params` 段永不解析。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ClientRelevant {
    /// 覆盖 capabilities.context_window(服务端 max_model_len 的可用窗口)
    pub context_window_override: Option<u32>,
    /// 覆盖 capabilities.prompt_caching("implicit"/"explicit_control"/"none")
    pub cache_support_override: Option<String>,
}

/// 部署 Profile — 服务端不可控域的能力表达(ADR-072 决策 ⑧)
///
/// `server_params` 用 `serde_json::Value` 接住:存在性/类型校验通过即忽略,
/// **零解析零执行**——服务端参数不属于客户端领域,进入客户端逻辑即违反
/// "服务端不可控域零代码"边界。
#[derive(Debug, Clone, Deserialize)]
pub struct DeploymentProfile {
    /// Profile 元信息
    pub profile: ProfileMeta,
    /// 服务端部署参数(仅 schema 校验,不解析不执行)
    pub server_params: serde_json::Value,
    /// 客户端可消费覆盖项
    #[serde(default)]
    pub client_relevant: ClientRelevant,
}

/// 解析单个部署 Profile TOML 文本(纯函数)
pub fn parse_profile_toml(content: &str) -> Result<DeploymentProfile, AffinityError> {
    toml::from_str(content).map_err(|e| AffinityError::Unknown {
        raw: format!("deployment profile TOML parse failed: {e}"),
    })
}

/// 应用 Profile 覆盖 — 仅 client_relevant 段进入 spec(ADR-072 决策 ⑧)
///
/// - `context_window_override` → capabilities.context_window
/// - `cache_support_override` → capabilities.prompt_caching
///   (未知字符串回落 None,保守:不臆造厂商缓存能力)
///
/// # 边界
/// server_params 段在此函数中**不可见**(解析时已丢弃语义),
/// 从类型层面保证服务端参数零客户端消费。
pub fn apply_profile_override(spec: &mut ModelAffinitySpec, profile: &DeploymentProfile) {
    if let Some(window) = profile.client_relevant.context_window_override {
        spec.capabilities.context_window = window;
    }
    if let Some(cs) = &profile.client_relevant.cache_support_override {
        spec.capabilities.prompt_caching = match cs.as_str() {
            "implicit" => CacheSupport::Implicit,
            "explicit_control" => CacheSupport::ExplicitControl,
            _ => CacheSupport::None,
        };
    }
}

/// 加载部署 Profile 目录(profiles/deployment/*.toml)为 id → Profile 表
///
/// 单文件失败即整体失败(快速失败):Profile 是能力覆盖的事实源,
/// 部分加载会静默改变通道能力认知。
pub fn load_profile_dir(dir: &Path) -> Result<HashMap<String, DeploymentProfile>, AffinityError> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| AffinityError::Unknown {
            raw: format!("read profile dir {}: {e}", dir.display()),
        })?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    paths.sort();

    let mut profiles = HashMap::new();
    for path in paths {
        let content = std::fs::read_to_string(&path).map_err(|e| AffinityError::Unknown {
            raw: format!("read profile file {}: {e}", path.display()),
        })?;
        let profile = parse_profile_toml(&content)?;
        // 重复 id 拒绝(快速失败):引用歧义比缺失更危险
        if profiles
            .insert(profile.profile.id.clone(), profile)
            .is_some()
        {
            return Err(AffinityError::Unknown {
                raw: format!("duplicate deployment profile id in {}", path.display()),
            });
        }
    }
    Ok(profiles)
}

/// 解析单个 TOML 文本为描述符列表(纯函数,便于测试与 fixture 复用)
pub fn parse_spec_toml(content: &str) -> Result<Vec<ModelAffinitySpec>, AffinityError> {
    parse_spec_toml_with_profiles(content, &HashMap::new())
}

/// 解析单个 TOML 文本并应用部署 Profile 覆盖(ADR-072 决策 ⑧)
///
/// spec 卡片顶层 `deployment_profile = "<id>"` 引用 Profile:
/// - 引用存在 → `apply_profile_override` 应用 client_relevant 覆盖
/// - 引用缺失 → 快速失败(引用歧义:静默忽略会改变通道能力认知)
pub fn parse_spec_toml_with_profiles(
    content: &str,
    profiles: &HashMap<String, DeploymentProfile>,
) -> Result<Vec<ModelAffinitySpec>, AffinityError> {
    let mut file: SpecFile = toml::from_str(content).map_err(|e| AffinityError::Unknown {
        raw: format!("spec TOML parse failed: {e}"),
    })?;
    if file.schema_version != SPEC_SCHEMA_VERSION {
        return Err(AffinityError::Unknown {
            raw: format!(
                "spec schema_version {} unsupported (expected {SPEC_SCHEMA_VERSION})",
                file.schema_version
            ),
        });
    }
    for spec in &file.models {
        validate_spec(spec)?;
    }
    // 部署 Profile 引用应用(全模型卡共享同一 Profile)
    if let Some(profile_id) = &file.deployment_profile {
        let profile = profiles
            .get(profile_id)
            .ok_or_else(|| AffinityError::Unknown {
                raw: format!("deployment profile '{profile_id}' referenced but not found"),
            })?;
        for spec in &mut file.models {
            apply_profile_override(spec, profile);
        }
    }
    Ok(file.models)
}

/// 加载 affinity.d 目录下全部 *.toml(文件名字典序,保证注册顺序确定)
///
/// 不加载部署 Profile(等价 `load_spec_dir_with_profiles(dir, None)`)。
pub fn load_spec_dir(dir: &Path) -> Result<Vec<ModelAffinitySpec>, AffinityError> {
    load_spec_dir_with_profiles(dir, None)
}

/// 加载 affinity.d 目录并应用部署 Profile(ADR-072 决策 ⑧)
///
/// `profile_dir` 为 Some 时先加载 `profiles/deployment/*.toml` 表,
/// 再解析 spec 卡片并应用 `deployment_profile` 引用覆盖。
pub fn load_spec_dir_with_profiles(
    dir: &Path,
    profile_dir: Option<&Path>,
) -> Result<Vec<ModelAffinitySpec>, AffinityError> {
    let profiles = match profile_dir {
        Some(pd) => load_profile_dir(pd)?,
        None => HashMap::new(),
    };
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| AffinityError::Unknown {
            raw: format!("read spec dir {}: {e}", dir.display()),
        })?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    paths.sort();

    let mut specs = Vec::new();
    for path in paths {
        let content = std::fs::read_to_string(&path).map_err(|e| AffinityError::Unknown {
            raw: format!("read spec file {}: {e}", path.display()),
        })?;
        // 单文件失败即整体失败(快速失败):spec 是路由决策的事实源,
        // 半加载状态比启动失败更危险(部分通道静默缺失难以排查)
        specs.extend(parse_spec_toml_with_profiles(&content, &profiles)?);
    }
    Ok(specs)
}

/// 单张描述符的语义校验(serde 类型校验之上的业务规则)
fn validate_spec(spec: &ModelAffinitySpec) -> Result<(), AffinityError> {
    if spec.model.is_empty() {
        return Err(AffinityError::Unknown {
            raw: format!("spec for {:?} has empty model name", spec.provider),
        });
    }
    if spec.dialects.is_empty() {
        return Err(AffinityError::Capability {
            provider: spec.provider.clone(),
            capability: "dialects (at least one protocol dialect required)".into(),
        });
    }
    if spec.endpoint.base_url.is_empty() {
        return Err(AffinityError::Unknown {
            raw: format!("spec '{}' has empty endpoint.base_url", spec.route_key()),
        });
    }
    // 流式是核心能力:不支持流式的通道进入 ChannelRejected 语义,拒绝注册
    if !spec.capabilities.streaming {
        return Err(AffinityError::Capability {
            provider: spec.provider.clone(),
            capability: "streaming".into(),
        });
    }
    // 系统边界: max_output=0 使 negotiate_budget 产出 thinking > max_output 不变量违例;
    // max_output=0 在任何厂商 API 上都无法工作(max_tokens ≥ 1 是通用约束),
    // 加载期快速失败优于运行期静默产出畸形预算
    if spec.capabilities.max_output == 0 {
        return Err(AffinityError::Capability {
            provider: spec.provider.clone(),
            capability: "max_output (must be > 0)".into(),
        });
    }
    // DeprecatedModelNames 怪癖:废弃模型名禁止注册
    // (DeepSeek 旧名 deepseek-chat/reasoner 已于 2026-07-24 废弃)
    for quirk in &spec.quirks {
        if let QuirkRule::DeprecatedModelNames { names } = quirk {
            if names.iter().any(|n| n.as_ref() == spec.model.as_ref()) {
                return Err(AffinityError::Unknown {
                    raw: format!(
                        "model '{}' is deprecated by vendor, registration rejected",
                        spec.route_key()
                    ),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::affinity::{ProtocolDialect, ProviderId};

    /// 最小合法 spec TOML(测试基准)
    fn minimal_toml(model: &str) -> String {
        format!(
            r#"
schema_version = 1

[[models]]
provider = "zhipu"
model = "{model}"
dialects = ["open_ai_chat"]

[models.capabilities]
streaming = true
tool_calling = true
thinking = "on_off"
context_window = 1000000
max_output = 128000
prompt_caching = "explicit_control"
service_tiers = ["standard"]
state_preservation = "block_preservation"
modalities = ["text"]
structured_output = true

[models.pricing]
currency = "cny"
input_micro_per_mtok = 2000000
output_micro_per_mtok = 8000000
cache_hit_micro_per_mtok = 400000
peak_periods = []

[models.endpoint]
base_url = "https://open.bigmodel.cn/api/paas/v4"
api_key_env = "ZHIPU_API_KEY"
timeout_ms = 120000
connect_timeout_ms = 10000
"#
        )
    }

    #[test]
    fn parse_minimal_spec() {
        let specs = parse_spec_toml(&minimal_toml("glm-5.2")).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].provider, ProviderId::Zhipu);
        assert_eq!(specs[0].route_key(), "zhipu/glm-5.2");
        assert!(specs[0].supports_dialect(ProtocolDialect::OpenAiChat));
    }

    #[test]
    fn reject_wrong_schema_version() {
        let toml = minimal_toml("glm-5.2").replace("schema_version = 1", "schema_version = 99");
        assert!(parse_spec_toml(&toml).is_err());
    }

    #[test]
    fn reject_misspelled_enum_value() {
        // 闭集枚举快速失败:provider 拼错在反序列化即报错(P8 数据化的安全面)
        let toml =
            minimal_toml("glm-5.2").replace(r#"provider = "zhipu""#, r#"provider = "zhipuu""#);
        assert!(parse_spec_toml(&toml).is_err());
    }

    #[test]
    fn reject_non_streaming_channel() {
        // 核心能力缺失 → ChannelRejected 语义(三态降级协议第三态)
        let toml = minimal_toml("glm-5.2").replace("streaming = true", "streaming = false");
        let err = parse_spec_toml(&toml).unwrap_err();
        assert!(matches!(err, AffinityError::Capability { .. }));
    }

    #[test]
    fn reject_zero_max_output() {
        // max_output=0 破坏 negotiate_budget 不变量(thinking ≤ max_output),拒绝注册
        let toml = minimal_toml("glm-5.2").replace("max_output = 128000", "max_output = 0");
        let err = parse_spec_toml(&toml).unwrap_err();
        assert!(matches!(err, AffinityError::Capability { .. }));
        assert!(err.to_string().contains("max_output"));
    }

    #[test]
    fn reject_deprecated_model_name() {
        // DeepSeek 旧名废弃场景:DeprecatedModelNames 怪癖拒绝注册
        let mut toml = minimal_toml("deepseek-chat");
        toml.push_str(
            r#"
[[models.quirks]]
rule = "deprecated_model_names"
names = ["deepseek-chat", "deepseek-reasoner"]
"#,
        );
        let err = parse_spec_toml(&toml).unwrap_err();
        assert!(err.to_string().contains("deprecated"));
    }

    #[test]
    fn load_dir_sorted_and_merged() {
        // 目录加载:多文件合并,字典序保证注册顺序确定
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("b_second.toml"),
            minimal_toml("glm-5.2-fast"),
        )
        .unwrap();
        std::fs::write(dir.path().join("a_first.toml"), minimal_toml("glm-5.2")).unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "not toml").unwrap();
        let specs = load_spec_dir(dir.path()).unwrap();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].model.as_ref(), "glm-5.2");
        assert_eq!(specs[1].model.as_ref(), "glm-5.2-fast");
    }

    // ============================================================
    // 部署 Profile(ADR-072 决策 ⑧)
    // ============================================================

    fn sample_profile() -> &'static str {
        r#"
[profile]
id = "vllm-7b-chat"
channel = "custom"
engine = "vllm"

[server_params]
kv_cache_quant_bits = 8
paged_attention_block_size = 16
enable_prefix_caching = true
chunked_prefill_size = 8192
max_model_len = 131072

[client_relevant]
context_window_override = 131072
cache_support_override = "implicit"
"#
    }

    #[test]
    fn profile_override_applies_client_relevant_only() {
        // client_relevant 覆盖生效:context_window 与 prompt_caching 被覆盖
        let profile = parse_profile_toml(sample_profile()).unwrap();
        let mut specs = parse_spec_toml(&minimal_toml("glm-5.2")).unwrap();
        apply_profile_override(&mut specs[0], &profile);
        assert_eq!(specs[0].capabilities.context_window, 131_072);
        assert_eq!(
            specs[0].capabilities.prompt_caching,
            CacheSupport::Implicit,
            "cache_support_override 必须覆盖显式声明"
        );
        // server_params 段已解析为 Value(存在性校验),不参与任何能力
        assert!(profile.server_params.get("kv_cache_quant_bits").is_some());
    }

    #[test]
    fn profile_reference_applied_at_parse() {
        // spec 卡片顶层引用 Profile → 解析时应用覆盖(全模型卡共享)
        let mut toml = minimal_toml("local-model");
        toml.insert_str(
            toml.find("[[models]]").unwrap(),
            "deployment_profile = \"vllm-7b-chat\"\n\n",
        );
        let mut profiles = HashMap::new();
        let profile = parse_profile_toml(sample_profile()).unwrap();
        profiles.insert(profile.profile.id.clone(), profile);
        let specs = parse_spec_toml_with_profiles(&toml, &profiles).unwrap();
        assert_eq!(specs[0].capabilities.context_window, 131_072);
        assert_eq!(specs[0].capabilities.prompt_caching, CacheSupport::Implicit);
    }

    #[test]
    fn profile_reference_missing_fails_fast() {
        // 引用了不存在的 Profile → 快速失败(引用歧义比缺失更危险)
        let mut toml = minimal_toml("local-model");
        toml.insert_str(
            toml.find("[[models]]").unwrap(),
            "deployment_profile = \"ghost-profile\"\n\n",
        );
        let err = parse_spec_toml_with_profiles(&toml, &HashMap::new()).unwrap_err();
        assert!(
            err.to_string().contains("ghost-profile"),
            "缺失引用必须快速失败: {err}"
        );
    }

    #[test]
    fn unknown_cache_support_falls_back_to_none() {
        // 未知缓存覆盖字符串回落 None(保守:不臆造厂商缓存能力)
        let mut spec = ModelAffinitySpec::minimal(
            ProviderId::Custom("local".into()),
            "m",
            ProtocolDialect::OpenAiChat,
        );
        spec.capabilities.prompt_caching = CacheSupport::ExplicitControl;
        apply_profile_override(
            &mut spec,
            &DeploymentProfile {
                profile: ProfileMeta {
                    id: "x".into(),
                    channel: String::new(),
                    engine: String::new(),
                },
                server_params: serde_json::json!({}),
                client_relevant: ClientRelevant {
                    context_window_override: None,
                    cache_support_override: Some("quantum_cache".into()),
                },
            },
        );
        assert_eq!(spec.capabilities.prompt_caching, CacheSupport::None);
    }

    #[test]
    fn duplicate_profile_id_rejected() {
        // 重复 id 快速失败(引用歧义)
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.toml"), sample_profile()).unwrap();
        std::fs::write(dir.path().join("b.toml"), sample_profile()).unwrap();
        let err = load_profile_dir(dir.path()).unwrap_err();
        assert!(err.to_string().contains("duplicate"), "{err}");
    }
}
