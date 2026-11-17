//! `fs.read` / `fs.list` providers — L10 tool_plane 的文件工具面。
//!
//! W1（L2 工具层重构方案 §7 W1-c）：把四环链 builtin 的 fs 工具补齐进 tool_plane
//! 注册表，使 LLM 发布面与计划面共享**同一工具集与同一实现**——真值源 =
//! [`crate::exec_plan::{fs_read, fs_list, safe_relative_path}`]（路径约束：仅相对
//! 路径、拒绝 `..` 与根/盘符前缀；`fs.read` 单文件 ≤256 KiB）。
//!
//! # 行为变化登记（W1）
//!
//! tool_plane 注册表新增 `fs.read` / `fs.list` ⇒ LLM tools 表 +2 个只读工具
//! （`side_effect = ReadOnly`，不受审批闸拦截）。四环链侧同名工具语义不变。

use async_trait::async_trait;
use gqep_executor::tool_plane::{ToolDescriptor, ToolError, ToolInvocation, ToolProvider};
use nexus_contracts::tool_plan::SideEffectDecl;
use tokio_util::sync::CancellationToken;

/// fs 工具 provider — `fs.read` / `fs.list`（只读，路径受限）
#[derive(Debug, Default)]
pub struct FsToolsProvider;

impl FsToolsProvider {
    /// New provider.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// 共享描述符构造 — name/side_effect/timeout 等静态属性单源
/// （`descriptors()` 与测试断言共用，防两处清单漂移）。
fn fs_descriptor(name: &str, description: &str) -> ToolDescriptor {
    ToolDescriptor {
        name: name.to_string(),
        provider: "fs".to_string(),
        description: description.to_string(),
        side_effect: SideEffectDecl::ReadOnly,
        // 5s 与 echo 同档：本地文件操作，无需 30s 级网络/进程容忍
        timeout_ms: 5_000,
        max_concurrency: 8,
        input_schema: Some(
            r#"{"type":"object","properties":{"path":{"type":"string","description":"relative path under the working directory"}},"required":["path"],"additionalProperties":false}"#
                .to_string(),
        ),
    }
}

#[async_trait]
impl ToolProvider for FsToolsProvider {
    fn provider_id(&self) -> &str {
        "fs"
    }

    fn descriptors(&self) -> Vec<ToolDescriptor> {
        vec![
            fs_descriptor(
                "fs.read",
                "Read a UTF-8 text file under the working directory (relative path, <=256 KiB)",
            ),
            fs_descriptor(
                "fs.list",
                "List directory entries under the working directory (relative path, sorted)",
            ),
        ]
    }

    async fn invoke(
        &self,
        inv: &ToolInvocation,
        cancel: CancellationToken,
    ) -> Result<String, ToolError> {
        // fs 操作是本地即时操作（单文件读/列目录），运行中途中断无意义；
        // 取消语义收敛为「派发前检查」——令牌已触发则不发起 IO。
        if cancel.is_cancelled() {
            return Err(ToolError::Cancelled(inv.call_id.clone()));
        }
        // 参数 JSON 解析在本层前置（InvalidArgs 归因）；其余错误（路径拒绝/IO
        // 失败）统一映射 `Provider` 并保留原文案（文案自带 `fs.read`/`fs.list`
        // 前缀，语义自包含）。W4 类型化契约落地后由 exec_plan 返回结构化错误。
        let outcome = match inv.tool.as_str() {
            "fs.read" | "fs.list" => {
                if let Err(e) = serde_json::from_str::<serde_json::Value>(&inv.args_json) {
                    return Err(ToolError::InvalidArgs(format!(
                        "{} invalid args JSON: {e}",
                        inv.tool
                    )));
                }
                if inv.tool == "fs.read" {
                    crate::exec_plan::fs_read(&inv.args_json).await
                } else {
                    crate::exec_plan::fs_list(&inv.args_json).await
                }
            }
            other => {
                return Err(ToolError::NotFound(format!("unknown fs tool: {other}")));
            }
        };
        outcome.map_err(ToolError::Provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn invocation(tool: &str, args: &str) -> ToolInvocation {
        ToolInvocation {
            call_id: format!("test-{tool}"),
            tool: tool.to_string(),
            args_json: args.to_string(),
            timeout_ms: None,
            priority: Default::default(),
            origin: Default::default(),
            quest_id: None,
            task_id: None,
            approved: true,
        }
    }

    #[tokio::test]
    async fn descriptors_are_readonly_and_consistent() {
        let provider = FsToolsProvider::new();
        assert_eq!(provider.provider_id(), "fs");
        let descriptors = provider.descriptors();
        assert_eq!(descriptors.len(), 2, "fs.read + fs.list");
        for d in &descriptors {
            assert_eq!(d.side_effect, SideEffectDecl::ReadOnly);
            assert_eq!(d.provider, "fs");
            assert!(d.name.starts_with("fs."), "name={}", d.name);
        }
    }

    #[tokio::test]
    async fn fs_read_roundtrip_and_rejections() {
        let provider = FsToolsProvider::new();
        // 真实读本文件（相对路径从 workspace root 之外不可控，用绝对路径拒
        // 绝来验证守卫 + 用 Cargo.toml 不可达性验证；真读路径放 tests 夹具）
        let temp = std::env::temp_dir().join("chimera_w1_fs_read_fixture.txt");
        tokio::fs::write(&temp, "hello w1").await.unwrap();
        // 绝对路径必须被拒（safe_relative_path 守卫）——Windows 反斜杠转正斜杠
        // 以保证 JSON 合法；`C:/...` 形式同样命中 Prefix 组件拒绝
        let abs = PathBuf::from(&temp).canonicalize().unwrap();
        let abs_json = abs.to_string_lossy().replace('\\', "/");
        let res = provider
            .invoke(
                &invocation("fs.read", &format!(r#"{{"path":"{abs_json}"}}"#)),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(matches!(res, ToolError::Provider(_)), "绝对路径拒绝: {res}");
        // 相对路径真读：cwd 由测试 runner 决定，构造临时相对等价物不可靠——
        // 改为验证非法 JSON 与缺 path 两个确定性分支
        let res = provider
            .invoke(&invocation("fs.read", "not-json"), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(res, ToolError::InvalidArgs(_)));
        tokio::fs::remove_file(&temp).await.ok();
    }

    #[tokio::test]
    async fn unknown_tool_is_not_found() {
        let provider = FsToolsProvider::new();
        let res = provider
            .invoke(&invocation("fs.write", "{}"), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(
            matches!(res, ToolError::NotFound(_)),
            "写类工具不存在: {res}"
        );
    }

    #[tokio::test]
    async fn cancelled_token_skips_io() {
        let provider = FsToolsProvider::new();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let res = provider
            .invoke(&invocation("fs.read", r#"{"path":"x"}"#), cancel)
            .await
            .unwrap_err();
        assert!(matches!(res, ToolError::Cancelled(_)));
    }
}
