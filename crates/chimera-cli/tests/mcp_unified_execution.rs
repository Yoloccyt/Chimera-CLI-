//! W2 端到端验收 — 计划面 MCP **真执行**（UnifiedExecutorAdapter → tool_plane →
//! StdioTransport JSON-RPC → mock server），L2 工具层重构方案 §8.2。
//!
//! 三条核心契约（**单测试合并**——SharedToolPlane 是进程级 OnceLock 单例，
//! 「首次装配决定缓存」是生产语义裁决：单进程单配置一次 build 与真实进程
//! 生命周期同构，多配置多测试会互相污染快照）：
//!
//! 1. **真执行**：计划面（四环 `dispatch`）调用 mock stdio MCP server 的
//!    readonly 工具，返回 server 真实应答（非 2PC 模拟面的编造
//!    `{success:true}`）——病灶 P2（语义伪造）根除的端到端有牙判据；
//! 2. **写类 fail-closed**：无任何 readonly 证据的 MCP 工具在批路径（legacy
//!    `execute`，无写放行位）显式失败（`policy: approval required`）——
//!    修复前 2PC 模拟面对写类从不拦截（无审批概念），显式拒绝是 W2 的安全
//!    收敛（ADR-32X 行为变化登记；恢复路径 = 声明配置或 policy 环 allow）；
//! 3. **配置声明解锁**：`declared_side_effects` 显式声明 readonly 的工具
//!    （hint 不可信但用户意图明确）⇒ 审批证据链放行——R10 WP-T1 供给面在
//!    W2 统一链上的延续（R11 W1 穿透链端到端确认）。
//!
//! 运行：`cargo test -p chimera-cli --test mcp_unified_execution -- --test-threads=1`

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::Arc;

use chimera_cli::config_types::{DeclaredSideEffectEntry, McpConfig, McpServerConfig};
use chimera_cli::tool_plane::{assemble_mcp, UnifiedExecutorAdapter};
use chimera_cli::ChimeraConfig;
use gqep_executor::tool_plane::{
    BoundedToolScheduler, ToolRegistry, ToolRuntime, ToolRuntimeConfig,
};
use gqep_executor::tool_runtime::{DispatchError, DispatchRequest, ToolRuntime as FourRingRuntime};
use gqep_executor::ToolExecutor;
use mcp_mesh::protocol::CommandGate;

mod common;

/// 全放行闸门（仅本测试：server 命令恒为本机解析 shell；fence 白名单纪律由
/// discovery_assembly 的真闸门测试覆盖——`FenceGate::default_secure` 白名单
/// 刻意不含 bash（通用 shell = 绕过面，N1 审计裁决），故本测试不经组合根
/// 硬编码闸门，改为手工组装全链（与 wiring 探针链同款模式），闸门策略本身
/// 的行为锁在 discovery_assembly）。
struct AllowShell;

impl CommandGate for AllowShell {
    fn check(&self, _program: &str, _args: &[String]) -> Result<(), String> {
        Ok(())
    }
}

/// 应答 initialize / tools/list / tools/call 的 mock server。
/// `readonly` 控制 tools/list 里工具的 `readOnlyHint`（写类证据构造）。
fn mock_server(id: &str, tool: &str, readonly: bool) -> McpServerConfig {
    let hint = if readonly {
        r#","annotations":{"readOnlyHint":true}"#
    } else {
        ""
    };
    let script = format!(
        r#"while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2025-03-26","serverInfo":{{"name":"mock-{id}","version":"1.0"}},"capabilities":{{}}}}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[{{"name":"{tool}","description":"mock","inputSchema":{{"type":"object"}}{hint}}}]}}}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"content":[{{"type":"text","text":"mock-echo-ok"}}],"isError":false}}}}\n' "$id"
      ;;
  esac
done"#
    );
    McpServerConfig {
        id: id.to_string(),
        command: Some(common::shell_program().to_string()),
        args: Some(vec!["-c".to_string(), script]),
        enabled: true,
        timeout_ms: 5_000,
        // readonly 工具的 descriptor 副作用判定依赖此开关（discovery.rs:105）
        trust_readonly_hints: readonly,
        ..Default::default()
    }
}

/// 三契约端到端（合并理由见模块头）。server 阵列：
/// - `mock/echo`：readOnlyHint=true + trust ⇒ 契约 1 真执行；
/// - `mockd/deploy`：无 hint（按 Write）但**配置显式声明 readonly** ⇒ 契约 3 放行；
/// - `mockn/deploy`：无 hint 且**无声明** ⇒ 契约 2 fail-closed。
#[tokio::test]
async fn unified_execution_end_to_end_contracts() {
    let cfg = ChimeraConfig {
        mcp: McpConfig {
            servers: vec![
                mock_server("mock", "echo", true),
                mock_server("mockd", "deploy", false),
                mock_server("mockn", "deploy", false),
            ],
            declared_side_effects: vec![DeclaredSideEffectEntry {
                tool: "mcp.mockd.deploy".to_string(),
                effect: nexus_contracts::tool_plan::SideEffectDecl::ReadOnly,
            }],
        },
        ..ChimeraConfig::default()
    };
    // 手工组装（与组合根 build() 同构的装配序，闸门换 AllowShell）：
    // 真发现 → tool_plane 注册表/调度器/运行时 → UnifiedExecutorAdapter →
    // 四环链（policy/sandbox/route/transport 恒齐备）。
    let mcp_plane = assemble_mcp(&cfg.mcp.servers, Arc::new(AllowShell));
    assert!(
        mcp_plane
            .snapshot
            .servers
            .iter()
            .map(|s| s.tools.len())
            .sum::<usize>()
            >= 3,
        "三个 mock server 必须被发现（每个 1 工具），实得: {:?}",
        mcp_plane
            .snapshot
            .servers
            .iter()
            .map(|s| (s.id.clone(), s.tools.len()))
            .collect::<Vec<_>>()
    );
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(chimera_cli::tool_plane::McpToolProvider::new(
            Arc::clone(&mcp_plane.snapshot),
        )))
        .expect("注册表无重名");
    let plane = Arc::new(ToolRuntime::new(
        Arc::new(registry),
        None,
        Arc::new(BoundedToolScheduler::new(8, 8, 64, 8)),
        ToolRuntimeConfig {
            interactive_inflight: 8,
            background_inflight: 8,
            max_waiters: 64,
            max_provider_inflight: 8,
            default_timeout_ms: 5_000,
            max_payload_bytes: 64 * 1024,
            max_batch: 64,
        },
    ));
    let adapter = Arc::new(UnifiedExecutorAdapter::from_shared_plane(
        Arc::clone(&plane),
        &cfg,
    ));
    let four_ring = FourRingRuntime::with_optional_delegate(
        Some(adapter as Arc<dyn ToolExecutor>),
        Vec::new(),
        seccore::execpolicy::PolicyAction::Allow,
    )
    .expect("四环链装配");
    // 会话保活：drop 即终止全部 MCP 会话。WHY ManuallyDrop——McpPlane 持有
    // tokio Runtime，在 async 上下文 drop 会触发「Cannot drop a runtime in a
    // context where blocking is not allowed」。生产路径 SharedToolPlane 为
    // OnceLock 进程单例（静态存储 drop，不在此问题面）；测试进程退出由 OS
    // 回收 runtime 资源，泄漏是测试专用妥协（known-issue 登记见 ADR-32X）。
    let _keepalive = std::mem::ManuallyDrop::new(mcp_plane);

    // ── 契约 1（真执行）：点号格式（计划面历史形态）→ adapter 归一 →
    // tool_plane StdioTransport → mock server tools/call 真实应答。
    let result = four_ring
        .dispatch(&DispatchRequest {
            tool_name: "mcp.mock.echo".to_string(),
            args_json: r#"{"text":"hi"}"#.to_string(),
        })
        .await;
    match &result {
        Ok(outcome) => {
            assert!(
                outcome.content_json.contains("mock-echo-ok"),
                "载荷须为 mock server 真实应答（病灶 P2 根除的端到端证据），实际: {}",
                outcome.content_json
            );
            assert!(
                !outcome.content_json.contains("transaction_id"),
                "不得再出现 2PC 模拟回执形态"
            );
        }
        other => panic!("readonly MCP 工具应真执行成功，实际: {other:?}"),
    }

    // ── 契约 2（写类 fail-closed）：mockn/deploy 无 hint、无声明、无放行位
    // ⇒ AskRequired（原 2PC 面对写类从不拦截）。
    let result = four_ring
        .dispatch(&DispatchRequest {
            tool_name: "mcp.mockn.deploy".to_string(),
            args_json: "{}".to_string(),
        })
        .await;
    match &result {
        Err(DispatchError::Transport(msg)) => {
            assert!(
                msg.contains("policy: approval required"),
                "无证据写类工具必须 fail-closed（AskRequired 归因），实际: {msg}"
            );
        }
        Ok(outcome) => panic!(
            "写类工具在无放行位时不得执行（fail-closed），实得: {:?}",
            outcome.content_json
        ),
        other => {
            panic!("写类 fail-closed 应归因 Transport（adapter policy: 文案），实际: {other:?}")
        }
    }

    // ── 契约 3（配置声明解锁）：mockd/deploy 同为无 hint 工具，但用户显式
    // 声明 readonly ⇒ 审批证据链放行；server 不应答 tools/call ⇒ 拿到的是
    // 执行期错误而**不是** approval required（放行与执行分离的判据）。
    let result = four_ring
        .dispatch(&DispatchRequest {
            tool_name: "mcp.mockd.deploy".to_string(),
            args_json: "{}".to_string(),
        })
        .await;
    match &result {
        Ok(outcome) => {
            assert!(
                outcome.content_json.contains("mock-echo-ok"),
                "声明 readonly 后真执行（mock server 同样应答 tools/call），实际: {}",
                outcome.content_json
            );
        }
        Err(DispatchError::Transport(msg)) => {
            assert!(
                !msg.contains("policy: approval required"),
                "用户显式声明 readonly ⇒ 审批证据链放行（错误只可能来自执行期，实际: {msg}）"
            );
        }
        other => panic!("声明 readonly 后不得被审批拦截，实际: {other:?}"),
    }
}
