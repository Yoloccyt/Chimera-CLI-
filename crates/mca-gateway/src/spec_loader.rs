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
//! # 校验策略(系统边界,快速失败)
//! TOML 是系统边界输入(用户可编辑),在此做全部校验:schema 版本、
//! 端点非空、方言非空、废弃模型名拒绝(DeprecatedModelNames 怪癖)。
//! 闭集枚举(ProviderId/QuirkRule)拼错在 serde 反序列化即失败。

use std::path::Path;

use nexus_contracts::affinity::{ModelAffinitySpec, QuirkRule};
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
    /// 该厂商的模型描述符列表
    #[serde(default)]
    models: Vec<ModelAffinitySpec>,
}

/// 解析单个 TOML 文本为描述符列表(纯函数,便于测试与 fixture 复用)
pub fn parse_spec_toml(content: &str) -> Result<Vec<ModelAffinitySpec>, AffinityError> {
    let file: SpecFile = toml::from_str(content).map_err(|e| AffinityError::Unknown {
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
    Ok(file.models)
}

/// 加载 affinity.d 目录下全部 *.toml(文件名字典序,保证注册顺序确定)
pub fn load_spec_dir(dir: &Path) -> Result<Vec<ModelAffinitySpec>, AffinityError> {
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
        specs.extend(parse_spec_toml(&content)?);
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
}
