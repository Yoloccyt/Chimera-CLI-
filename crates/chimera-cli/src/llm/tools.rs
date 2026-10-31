//! `ToolDescriptor` → `ToolDecl` 映射（Phase 7，ADR-232）
//!
//! 对应架构层: L10 Interface（chimera-cli）
//!
//! WHY 单独成模块：模型侧只该看到"工具名 + 说明 + 入参 schema"三件事，
//! L7 的执行属性（timeout / max_concurrency / provider）不得泄漏进 prompt——
//! 它们既占 token 又可能被模型当成可协商参数。

use gqep_executor::tool_plane::ToolDescriptor;
use nexus_contracts::affinity::ToolDecl;

/// `input_schema` 缺省时的兜底 schema
///
/// WHY `additionalProperties:false`：缺 schema 时宁可让模型只给空对象，
/// 也不要它自由发挥字段名（provider 侧参数校验会拒，形成隐式重试成本）。
pub const FALLBACK_OBJECT_SCHEMA: &str = r#"{"type":"object","additionalProperties":false}"#;

/// 把工具描述符映射为模型可见的工具声明
#[must_use]
pub fn tool_decls(descriptors: &[ToolDescriptor]) -> Vec<ToolDecl> {
    descriptors
        .iter()
        .map(|descriptor| ToolDecl {
            name: descriptor.name.as_str().into(),
            description: descriptor.description.as_str().into(),
            parameters_schema: descriptor
                .input_schema
                .clone()
                .unwrap_or_else(|| FALLBACK_OBJECT_SCHEMA.to_string())
                .into(),
        })
        .collect()
}

/// 从内容块序列里取出全部文本（最终回复拼接用）
#[must_use]
pub fn text_of(blocks: &[nexus_contracts::affinity::ContentBlock]) -> String {
    use nexus_contracts::affinity::ContentBlock;
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_ref()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}
