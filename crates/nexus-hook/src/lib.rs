//! nexus-hook — 生命周期 Hook 系统（P3-T3，v4.0 WI-24）
//!
//! 对应架构层: **L9 Quest**（ADR-146 裁决：D-P4 层归属定案——挂靠 Quest 生命周期）
//! 对应任务: **P3-T3**（手册 W16，WI-24：13+ LifecycleEvent + TOML 挂载 + 沙箱）
//!
//! # 职责（v4.0 WI-24 规格）
//! 用户可编程生命周期挂载点:
//! - 13+ [`LifecycleEvent`]（PreToolUse/PostToolUse/PreQuestTurn/PostQuestTurn 等）;
//! - TOML 配置挂载 shell 命令 + 环境变量注入（$TOOL_NAME/$SESSION_ID/$GOAL_ID）;
//! - **非零退出码可中断**:PreToolUse 类 hook 返回非零 → 拒否该次工具调用;
//! - **安全门**:hook 命令执行前经 seccore [`ProcessFence`] 沙箱校验
//!   （逃逸拒绝:写 /etc、越界网络）+ 项目信任提示（TrustLevel）+ 超时熔断;
//! - **全量审计**:每条 hook 触发记录进 [`HookAudit`]（可接 session-store,预留）。
//!
//! # 设计约束
//! - hook 命令为**同步 shell 命令**（TOML 配置）,执行走 `tokio::process` + 超时
//!   （单 hook 超时默认 5s,超时熔断不阻主流程——恶意/故障 hook 防护）;
//! - 禁 feature 标志:trust 级别与挂载经配置/构造参数表达;
//! - `#![forbid(unsafe_code)]` 由 crate 顶层保证。

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod audit;
pub mod config;
/// P4-T3: hook.* 双轨注册与触发事件桥（WI-21 联动）
pub mod event_bridge;
pub mod executor;
pub mod lifecycle;

pub use audit::{AuditSink, HookAudit, HookAuditEntry, NoopAuditSink};
pub use config::{HookConfig, HookSpec, TrustLevel};
pub use executor::{HookError, HookExecutor, HookResult};
pub use lifecycle::LifecycleEvent;

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::{HookAudit, HookConfig, HookExecutor, HookResult, LifecycleEvent, TrustLevel};
}
