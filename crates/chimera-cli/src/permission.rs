//! Permission prompt 机制 — 破坏性操作的交互式确认
//!
//! 对应任务:Task 1.11
//!
//! # 设计
//!
//! 破坏性命令(`quest cancel` / `agent cancel` / `mcp call` / `chimera run`
//! 外部命令执行)在执行前必须调用 `confirm` 获取用户确认,
//! 除非显式跳过(见 `PermissionCtx`)//!
//! # 全局 flag(Task 1.11.2 / 1.11.3)
//!
//! - `--yes`:跳过所有 prompt,自动确认(适合熟练用户快速操作)
//! - `--no-permission`:自动允许所有操作,不弹 prompt(适合 CI/自动化场景)
//!
//! 两者语义区别:`--yes` 假设用户已知晓操作影响并主动确认;
//! `--no-permission` 假设运行环境无交互能力(无 TTY),需要 fail-open。
//! 当前实现中两者效果相同(均跳过 prompt 返回 true),保留语义区分
//! 便于未来扩展(如 `--no-permission` 可触发更详细的审计日志)。
//!
//! # Ctrl+C 处理(Task 1.11.5)
//!
//! 用户在 prompt 等待期间按 Ctrl+C 时,`confirm` 通过 `tokio::select!`
//! 在 `tokio::signal::ctrl_c()` 与 stdin 读取之间竞争,若 Ctrl+C 先触发
//! 则返回 `ChimeraCliError::UserCancelled`,退出码 4
//! (对应 ADR-060 ExitCode 矩阵的 `user_cancelled`)。

#![forbid(unsafe_code)]

use std::io::{self, BufRead, IsTerminal, Write};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::cli::Cli;
use crate::error::ChimeraCliError;

/// 权限三态决策（T2-1）
///
/// 非交互环境无法替用户回答「是否允许」时，`Unknown` 必须由调用方
/// fail-closed 降级为 `Deny`，而不是默认放行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    /// 显式允许（`--yes` / `--no-permission`）
    Allow,
    /// 显式拒绝或 fail-closed 降级
    Deny,
    /// 非交互环境下无法判断，需交互确认
    Unknown,
}

/// 授权结果 — 三态决策 + 是否发生 fail-closed 降级
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionAuthorization {
    /// 最终决策
    pub decision: PermissionDecision,
    /// 由 `Unknown` 降级为 `Deny` 时为 true
    pub downgraded: bool,
}

impl PermissionAuthorization {
    /// 是否放行
    #[must_use]
    pub const fn is_allowed(&self) -> bool {
        matches!(self.decision, PermissionDecision::Allow)
    }
}

/// fail-closed 降级计数器（全局静态，供测试与后续指标面板读取）
static FAIL_CLOSED_DOWNGRADES: AtomicU64 = AtomicU64::new(0);

/// 读取 fail-closed 降级累计次数
#[must_use]
pub fn fail_closed_downgrades() -> u64 {
    FAIL_CLOSED_DOWNGRADES.load(Ordering::Relaxed)
}

/// 权限门 — 三态裁决 + 可注入的交互能力
#[derive(Debug, Clone, Copy)]
pub struct PermissionGate {
    /// 当前环境是否具备 TTY 交互能力
    interactive: bool,
}

impl PermissionGate {
    /// 以显式交互能力构造（测试用可注入路径）
    #[must_use]
    pub const fn new(interactive: bool) -> Self {
        Self { interactive }
    }

    /// 以进程 stdin 是否连接终端推断交互能力
    #[must_use]
    pub fn from_stdin() -> Self {
        Self::new(io::stdin().is_terminal())
    }

    /// 当前门是否处于交互环境
    #[must_use]
    pub const fn is_interactive(&self) -> bool {
        self.interactive
    }

    /// 三态评估（纯函数，不含降级与计数）
    ///
    /// ADR-276 D1：本函数**当前只产 Allow / Unknown 两态** —— `Deny` 只能由
    /// [`Self::authorize`] 的非交互 fail-closed 降级产生。这不是缺陷，是分层：
    /// `evaluate` 只回答「用户是否已显式授权」，规则式拒绝属策略层（ExecPolicy）。
    /// 三态枚举与 `authorize` 的 Deny 臂**不得删除**：删除会把「未知」与「拒绝」
    /// 压成两态，未来任何规则式拒绝源接入时无处安放（且 `PermissionDecision`
    /// 是公开契约）。结构锁：`tests/zero_traffic_adjudication_locks.rs`。
    #[must_use]
    pub fn evaluate(&self, ctx: &PermissionCtx) -> PermissionDecision {
        if ctx.should_skip_prompt() {
            PermissionDecision::Allow
        } else {
            PermissionDecision::Unknown
        }
    }

    /// 授权 — 在 `evaluate` 基础上应用 fail-closed 降级与计数
    #[must_use]
    pub fn authorize(&self, ctx: &PermissionCtx) -> PermissionAuthorization {
        match self.evaluate(ctx) {
            PermissionDecision::Allow => PermissionAuthorization {
                decision: PermissionDecision::Allow,
                downgraded: false,
            },
            // ADR-276 D1：生产不可达但**结构性保留**（见 `evaluate` 文档）。
            // 删除本臂会让三态坍缩为两态；保留它使 `evaluate` 将来接入规则源
            // 时无需改动 `authorize` 的判决形状。
            PermissionDecision::Deny => PermissionAuthorization {
                decision: PermissionDecision::Deny,
                downgraded: false,
            },
            PermissionDecision::Unknown if !self.interactive => {
                FAIL_CLOSED_DOWNGRADES.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    reason = "unknown_permission_fail_closed",
                    interactive = self.interactive,
                    "permission decision unknown in non-interactive context; fail-closed to Deny"
                );
                PermissionAuthorization {
                    decision: PermissionDecision::Deny,
                    downgraded: true,
                }
            }
            PermissionDecision::Unknown => PermissionAuthorization {
                decision: PermissionDecision::Unknown,
                downgraded: false,
            },
        }
    }
}

/// Permission 上下文 — 封装 `--yes` 与 `--no-permission` 全局 flag
///
/// 由 `Cli` 构造,传递到需要 permission prompt 的命令处理函数。
/// 避免在函数签名中传递两个独立 bool 参数。
#[derive(Clone, Copy, Debug, Default)]
pub struct PermissionCtx {
    /// `--yes` flag:跳过所有 prompt,自动确认
    pub yes: bool,
    /// `--no-permission` flag:自动允许所有操作(CI 友好,fail-open)
    pub no_permission: bool,
}

impl PermissionCtx {
    /// 从 `Cli` 结构体构造 `PermissionCtx`
    pub fn from_cli(cli: &Cli) -> Self {
        Self {
            yes: cli.yes,
            no_permission: cli.no_permission,
        }
    }

    /// 判断是否应跳过 prompt(返回 true 自动确认)
    ///
    /// `--yes` 或 `--no-permission` 任一启用即跳过
    pub fn should_skip_prompt(&self) -> bool {
        self.yes || self.no_permission
    }
}

/// 交互式确认 prompt(同步版本)— 询问用户是否确认执行某操作
///
/// `action`:操作简述(如 "取消 Quest" / "调用 MCP 工具")
/// `details`:操作详情(如 Quest ID / 工具名 + 参数)
///
/// 返回 `Ok(true)` 表示用户确认,`Ok(false)` 表示用户拒绝(输入非 y/yes),
/// `Err(IoError)` 表示 stdin 读取失败。
///
/// # 注意
///
/// 此函数为同步版本,不处理 Ctrl+C(Ctrl+C 会触发 SIGINT 直接终止进程)。
/// 需要 Ctrl+C 优雅处理的场景应使用异步版本 [`confirm`](fn.confirm.html)。
/// 保留此函数用于:
/// 1. 不需要 Ctrl+C 处理的简单场景
/// 2. 单元测试中验证输入解析逻辑
/// 3. 作为 `confirm` 的底层实现
pub fn prompt_confirmation(action: &str, details: &str) -> Result<bool, ChimeraCliError> {
    // WHY eprint 而非 eprintln:prompt 后跟输入,需在同一行
    eprint!("{} ({})? [y/N] ", action, details);
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;

    // 解析用户输入:仅 "y" / "yes"(大小写不敏感)视为确认,其余视为拒绝
    Ok(matches!(input.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// 带 PermissionCtx 的异步确认 prompt — 自动处理 `--yes` / `--no-permission` 跳过 + Ctrl+C
///
/// 这是推荐的调用入口(命令处理函数应使用此函数):
/// 1. 若 `--yes` 或 `--no-permission` 启用,直接返回 `Ok(true)` 不读 stdin
/// 2. 否则打印 prompt 并 `tokio::select!` 在 `ctrl_c()` 与 stdin 读取之间竞争:
///    - Ctrl+C 先触发 → `Err(UserCancelled)`(退出码 4)
///    - stdin 先就绪 → 解析输入返回 `Ok(true/false)`
///
/// WHY 用 `spawn_blocking` 包装 `read_line`:stdin 读取是阻塞 IO,
/// 直接在 async 上下文调用会阻塞 runtime(`§4.4 反模式 #2`),
/// `spawn_blocking` 将其移到阻塞线程池。
pub async fn confirm(
    ctx: &PermissionCtx,
    action: &str,
    details: &str,
) -> Result<bool, ChimeraCliError> {
    confirm_with(&PermissionGate::from_stdin(), ctx, action, details).await
}

/// 可注入权限门的确认实现（T2-2）
///
/// 1. `Allow` → 直接 `Ok(true)`，不读 stdin
/// 2. `Deny` → 直接 `Ok(false)`，不读 stdin（fail-closed 降级也走这里）
/// 3. `Unknown` → 原交互 prompt + Ctrl+C select 路径
pub(crate) async fn confirm_with(
    gate: &PermissionGate,
    ctx: &PermissionCtx,
    action: &str,
    details: &str,
) -> Result<bool, ChimeraCliError> {
    let authorization = gate.authorize(ctx);
    match authorization.decision {
        PermissionDecision::Allow => {
            tracing::debug!(
                action,
                details,
                yes = ctx.yes,
                no_perm = ctx.no_permission,
                "permission prompt skipped"
            );
            return Ok(true);
        }
        PermissionDecision::Deny => {
            tracing::info!(
                action,
                details,
                downgraded = authorization.downgraded,
                "permission denied without prompting"
            );
            return Ok(false);
        }
        PermissionDecision::Unknown => {}
    }

    // 打印 prompt 到 stderr
    eprint!("{} ({})? [y/N] ", action, details);
    io::stderr().flush()?;

    // 在 Ctrl+C 与 stdin 读取之间 select,确保 Ctrl+C 优雅返回 UserCancelled
    tokio::select! {
        biased;
        // Ctrl+C 先触发:返回 UserCancelled(Task 1.11.5)
        _ = tokio::signal::ctrl_c() => {
            eprintln!();
            tracing::info!("用户在 permission prompt 期间按 Ctrl+C,操作已取消");
            Err(ChimeraCliError::UserCancelled)
        }
        // stdin 先就绪:解析用户输入
        result = tokio::task::spawn_blocking(read_stdin_line) => {
            match result {
                Ok(Ok(input)) => Ok(matches!(
                    input.trim().to_lowercase().as_str(),
                    "y" | "yes"
                )),
                Ok(Err(e)) => Err(ChimeraCliError::IoError(e)),
                // spawn_blocking panic 视为取消
                Err(_) => Err(ChimeraCliError::UserCancelled),
            }
        }
    }
}

/// 从 stdin 读取一行(spawn_blocking 中执行)
///
/// 独立函数便于 `spawn_blocking` 调用(要求 `FnOnce + Send + 'static`)。
fn read_stdin_line() -> io::Result<String> {
    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    Ok(input)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// 验证默认 `PermissionCtx` 不跳过 prompt
    #[test]
    fn test_default_ctx_does_not_skip() {
        let ctx = PermissionCtx::default();
        assert!(!ctx.should_skip_prompt(), "默认 ctx 应不跳过 prompt");
    }

    /// 验证 `--yes` flag 跳过 prompt
    #[test]
    fn test_yes_flag_skips_prompt() {
        let ctx = PermissionCtx {
            yes: true,
            no_permission: false,
        };
        assert!(ctx.should_skip_prompt(), "--yes 应跳过 prompt");
    }

    /// 验证 `--no-permission` flag 跳过 prompt
    #[test]
    fn test_no_permission_flag_skips_prompt() {
        let ctx = PermissionCtx {
            yes: false,
            no_permission: true,
        };
        assert!(ctx.should_skip_prompt(), "--no-permission 应跳过 prompt");
    }

    /// 验证 `--yes` + `--no-permission` 同时启用跳过 prompt
    #[test]
    fn test_both_flags_skip_prompt() {
        let ctx = PermissionCtx {
            yes: true,
            no_permission: true,
        };
        assert!(ctx.should_skip_prompt(), "两个 flag 同时启用应跳过 prompt");
    }

    /// 验证 `PermissionCtx::from_cli` 正确提取 flag
    #[test]
    fn test_from_cli_extracts_flags() {
        use crate::cli::Cli;
        use clap::Parser;

        // --yes 启用
        let cli = Cli::parse_from(["chimera", "--yes", "run", "test"]);
        let ctx = PermissionCtx::from_cli(&cli);
        assert!(ctx.yes, "--yes 应被提取为 true");
        assert!(!ctx.no_permission, "未传 --no-permission 应为 false");

        // --no-permission 启用
        let cli = Cli::parse_from(["chimera", "--no-permission", "run", "test"]);
        let ctx = PermissionCtx::from_cli(&cli);
        assert!(!ctx.yes, "未传 --yes 应为 false");
        assert!(ctx.no_permission, "--no-permission 应被提取为 true");

        // 两者都不传
        let cli = Cli::parse_from(["chimera", "run", "test"]);
        let ctx = PermissionCtx::from_cli(&cli);
        assert!(!ctx.yes);
        assert!(!ctx.no_permission);
    }

    /// 验证 `confirm` 在 `--yes` 启用时返回 true 且不读取 stdin
    ///
    /// WHY 这很重要:CI 环境无 stdin,若 `--yes` 不跳过会导致 hang
    #[tokio::test]
    async fn test_confirm_with_yes_returns_true_without_reading_stdin() {
        let ctx = PermissionCtx {
            yes: true,
            no_permission: false,
        };
        // 此测试不依赖 stdin(应直接返回 true),若 hang 则说明逻辑有误
        let result = confirm(&ctx, "test action", "test details").await.unwrap();
        assert!(result, "--yes 启用时 confirm 应返回 true");
    }

    /// 验证 `confirm` 在 `--no-permission` 启用时返回 true
    #[tokio::test]
    async fn test_confirm_with_no_permission_returns_true() {
        let ctx = PermissionCtx {
            yes: false,
            no_permission: true,
        };
        let result = confirm(&ctx, "test action", "test details").await.unwrap();
        assert!(result, "--no-permission 启用时 confirm 应返回 true");
    }

    /// 权限三态矩阵：Allow×{yes,no_permission} / Unknown×non-interactive / Unknown×interactive
    #[test]
    fn permission_matrix() {
        let before = fail_closed_downgrades();

        // Allow × {--yes, --no-permission}：evaluate 与 authorize 均为 Allow，不降级
        for ctx in [
            PermissionCtx {
                yes: true,
                no_permission: false,
            },
            PermissionCtx {
                yes: false,
                no_permission: true,
            },
        ] {
            let gate = PermissionGate::new(false);
            assert_eq!(gate.evaluate(&ctx), PermissionDecision::Allow);
            let auth = gate.authorize(&ctx);
            assert_eq!(auth.decision, PermissionDecision::Allow);
            assert!(!auth.downgraded, "显式 allow 不应触发 fail-closed 降级");
        }

        // Unknown × non-interactive：降级为 Deny，计数 +1
        let ctx = PermissionCtx::default();
        let gate = PermissionGate::new(false);
        assert_eq!(gate.evaluate(&ctx), PermissionDecision::Unknown);
        let auth = gate.authorize(&ctx);
        assert_eq!(auth.decision, PermissionDecision::Deny);
        assert!(auth.downgraded, "非交互 Unknown 必须 fail-closed 降级");
        let after = fail_closed_downgrades();
        assert!(
            after > before,
            "fail-closed 降级计数应递增：before={before} after={after}"
        );

        // Unknown × interactive：保持 Unknown（等待交互确认）
        let gate = PermissionGate::new(true);
        assert_eq!(gate.evaluate(&ctx), PermissionDecision::Unknown);
        let auth = gate.authorize(&ctx);
        assert_eq!(auth.decision, PermissionDecision::Unknown);
        assert!(!auth.downgraded, "交互环境不应降级");
    }

    /// 非交互且无 flag 时 confirm 返回 Ok(false)，且不读 stdin
    #[tokio::test]
    async fn test_confirm_non_interactive_without_flag_returns_false() {
        let gate = PermissionGate::new(false);
        let ctx = PermissionCtx::default();
        let result = confirm_with(&gate, &ctx, "test action", "test details")
            .await
            .unwrap();
        assert!(!result, "非交互无 flag 应 fail-closed 拒绝");
    }

    /// Allow 路径 authorize 后 confirm 语义等价 Ok(true)（不读 stdin）
    #[tokio::test]
    async fn test_confirm_allow_matrix_returns_true() {
        for ctx in [
            PermissionCtx {
                yes: true,
                no_permission: false,
            },
            PermissionCtx {
                yes: false,
                no_permission: true,
            },
        ] {
            let gate = PermissionGate::new(false);
            assert!(gate.authorize(&ctx).is_allowed());
            let result = confirm_with(&gate, &ctx, "test action", "test details")
                .await
                .unwrap();
            assert!(result, "Allow 路径 confirm 应返回 Ok(true)");
        }
    }
}
