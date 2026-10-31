//! 工具审批策略（Phase 7，ADR-232）
//!
//! 对应架构层: L10 Interface（chimera-cli）
//!
//! # 为什么不把审批塞进 `ToolRuntime`
//! L7 的 `ToolRuntime` 只认 `ToolInvocation::approved` 这一**已决策事实**
//! （外加可选 `ExecPolicy` 硬拒）。"要不要问人"属于交互面（CLI 有 stdin、TUI 有弹窗、
//! 常驻宿主有协议），因此判定发生在 L10，判完才把 `approved` 交给 L7。
//!
//! # 三档语义
//! - [`DenyWrites`]：只读自动放行、写一律拒 —— 非交互与默认档（fail-closed）；
//! - [`CliApproval`]：只读自动放行、写走 `permission::confirm`（`--yes` / `--no-permission`
//!   时由 ctx 自行短路）；
//! - [`ApproveAll`]：全放行 —— 仅测试与受信本地自动化使用，调用方须显式选择。
//!
//! 命名与方案 §5.7 的偏差：方案另列 `AutoApproveReadOnly`，但其"只读自动 + 写不批"
//! 与 `DenyWrites` 完全同义；保留两个同义类型会让读者以为行为有差别，故只留一个。

use async_trait::async_trait;
use gqep_executor::tool_plane::ToolInvocation;
use nexus_contracts::tool_plan::SideEffectDecl;

use crate::permission::{self, PermissionCtx};

/// 工具审批契约
#[async_trait]
pub trait ToolApproval: Send + Sync {
    /// 返回 `false` = **不执行**该调用；循环会回填 `ask_required` 错误结果给模型，
    /// 而不是静默丢弃 —— 模型必须知道自己被拒，否则会用同样参数重试到轮数上限。
    async fn approve(&self, inv: &ToolInvocation, side_effect: SideEffectDecl) -> bool;
}

/// 只读自动放行、写拒绝（默认档）
#[derive(Debug, Default, Clone, Copy)]
pub struct DenyWrites;

#[async_trait]
impl ToolApproval for DenyWrites {
    async fn approve(&self, _inv: &ToolInvocation, side_effect: SideEffectDecl) -> bool {
        !matches!(side_effect, SideEffectDecl::Write)
    }
}

/// 全部放行（测试 / 受信本地自动化）
#[derive(Debug, Default, Clone, Copy)]
pub struct ApproveAll;

#[async_trait]
impl ToolApproval for ApproveAll {
    async fn approve(&self, _inv: &ToolInvocation, _side_effect: SideEffectDecl) -> bool {
        true
    }
}

/// CLI 审批：写工具逐条 `permission::confirm`
#[derive(Debug, Clone)]
pub struct CliApproval {
    ctx: PermissionCtx,
}

impl CliApproval {
    /// 以当前 permission 上下文构造（`--yes` / `--no-permission` 由 ctx 自行短路）
    #[must_use]
    pub fn new(ctx: PermissionCtx) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolApproval for CliApproval {
    async fn approve(&self, inv: &ToolInvocation, side_effect: SideEffectDecl) -> bool {
        if !matches!(side_effect, SideEffectDecl::Write) {
            return true;
        }
        let details = format!("tool={}, args={}", inv.tool, trunc_args(&inv.args_json));
        // `confirm` 的错误（stdin 不可用等）按"未获批准"处理：宁可少做，不可多做
        match permission::confirm(&self.ctx, "执行 LLM 请求的写工具", &details).await {
            Ok(approved) => approved,
            Err(error) => {
                tracing::warn!(tool = %inv.tool, error = %error, "写工具审批不可用,按拒绝处理");
                false
            }
        }
    }
}

/// 审批提示里的参数摘要截断（避免把整段 payload 打进终端）
fn trunc_args(args: &str) -> String {
    const MAX_CHARS: usize = 160;
    let chars: Vec<char> = args.chars().collect();
    if chars.len() <= MAX_CHARS {
        return args.to_string();
    }
    let mut head: String = chars[..MAX_CHARS].iter().collect();
    head.push_str("…(truncated)");
    head
}
