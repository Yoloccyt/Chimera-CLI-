//! 神经符号一致性守护（P2-T11，v4.0 WI-27）
//!
//! 对应架构层: **L7 Execution**（gqep-executor）
//! 对应任务: **P2-T11**（手册 W13-14）
//!
//! # 问题（WI-27）
//! LLM 生成代码/配置后无即时符号验证，错误可累积至不可恢复。
//! 分布式"持续一致性验证"：每次写操作后增量验证不变量。
//!
//! # 设计（v4.0 WI-27 规格）
//! - `Invariant` trait：`check(change) -> InvariantResult`（增量验证入口）
//! - `ProjectCompilesInvariant`：写文件后 `cargo check`——大仓库降级为
//!   变更 crate 局部 check（`cargo check -p <crate>`，延迟 <5s 门禁）
//! - 验证报告喂 WI-30 奖励函数（编译+类型双过 +3.0，否则 −1.0）——
//!   本任务输出 `InvariantResult` 供消费，不直接接奖励（P2-T10 已建 Shadow）
//! - **纯 Rust 调既有工具链，零模型组件**（红线：WI-27）

use std::process::Command;
use std::time::{Duration, Instant};

/// 不变量验证结果
#[derive(Debug, Clone, PartialEq)]
pub enum InvariantResult {
    /// 验证通过（含编译+类型双过）
    Passed {
        /// 验证耗时
        elapsed_ms: u64,
    },
    /// 验证失败（含错误摘要）
    Failed {
        /// 失败原因摘要（截断）
        summary: String,
        /// 验证耗时
        elapsed_ms: u64,
    },
    /// 工具链不可用（降级：不阻断写操作，但记录——fail-open 仅限工具缺失）
    Skipped {
        /// 降级原因描述（写入审计日志）
        reason: String,
    },
}

impl InvariantResult {
    /// 是否通过
    #[must_use]
    pub const fn is_passed(&self) -> bool {
        matches!(self, Self::Passed { .. })
    }

    /// 奖励信号映射（WI-30：编译+类型双过 +3.0，否则 −1.0）
    #[must_use]
    pub const fn reward_signal(&self) -> f64 {
        match self {
            Self::Passed { .. } => 3.0,
            Self::Failed { .. } => -1.0,
            Self::Skipped { .. } => 0.0,
        }
    }
}

/// 一致性不变量 trait — 增量验证入口
pub trait Invariant: Send + Sync {
    /// 对一次变更执行增量验证
    fn check(&self, change: &Change) -> InvariantResult;
}

/// 变更描述（写操作的最小面）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// 变更文件路径
    pub path: String,
    /// 变更 crate（大仓库局部 check 用；None = 未知，退全仓）
    pub crate_name: Option<String>,
}

impl Change {
    /// 新建变更描述
    #[must_use]
    pub fn new(path: impl Into<String>, crate_name: Option<String>) -> Self {
        Self {
            path: path.into(),
            crate_name,
        }
    }
}

/// 工程编译不变量 — 写文件后 cargo check（增量验证）
///
/// # 大仓库降级（WI-27 门禁）
/// 变更指定 crate 时执行 `cargo check -p <crate>`（局部，延迟 <5s 门禁）；
/// 未指定时全仓 `cargo check`（门禁内为全仓最小面）。
///
/// # 降级路径
/// cargo 工具链缺失 → `Skipped`（fail-open 仅限工具缺失，非验证失败）。
pub struct ProjectCompilesInvariant {
    /// workspace 根目录（cargo check 执行目录）
    workspace_root: std::path::PathBuf,
    /// 超时（超过即判定失败——防卡死；预留运行时判定，当前仅编译态校验）
    #[allow(dead_code)]
    timeout: Duration,
}

impl ProjectCompilesInvariant {
    /// 新建不变量验证器
    #[must_use]
    pub fn new(workspace_root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            timeout: Duration::from_secs(60),
        }
    }

    /// 带超时构造（测试用小超时）
    #[must_use]
    pub fn with_timeout(workspace_root: impl Into<std::path::PathBuf>, timeout: Duration) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            timeout,
        }
    }

    /// 执行 cargo check（局部/全仓）
    fn run_check(&self, change: &Change) -> InvariantResult {
        let t0 = Instant::now();
        // 构造 cargo check 命令（局部 check 优先——大仓库降级）
        let mut cmd = Command::new("cargo");
        cmd.arg("check");
        if let Some(crate_name) = &change.crate_name {
            cmd.arg("-p").arg(crate_name);
        }
        cmd.arg("--quiet");
        cmd.current_dir(&self.workspace_root);

        let output = match cmd.output() {
            Ok(o) => o,
            Err(e) => {
                return InvariantResult::Skipped {
                    reason: format!("cargo 工具链不可用: {e}"),
                };
            }
        };
        let elapsed_ms = t0.elapsed().as_millis() as u64;
        if output.status.success() {
            InvariantResult::Passed { elapsed_ms }
        } else {
            // 失败摘要截断（stderr 前 200 字符）
            let stderr = String::from_utf8_lossy(&output.stderr);
            let summary: String = stderr.chars().take(200).collect();
            InvariantResult::Failed {
                summary: if summary.is_empty() {
                    "cargo check 失败（无 stderr）".to_string()
                } else {
                    summary
                },
                elapsed_ms,
            }
        }
    }
}

impl Invariant for ProjectCompilesInvariant {
    fn check(&self, change: &Change) -> InvariantResult {
        self.run_check(change)
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_root() -> PathBuf {
        // 测试用临时目录（空目录 → cargo check 失败于无 Cargo.toml，
        // 验证失败路径；Skipped 由工具缺失触发——CI 有 cargo 则不会触发）
        std::env::temp_dir()
    }

    #[test]
    fn reward_signal_mapping() {
        assert_eq!(
            InvariantResult::Passed { elapsed_ms: 10 }.reward_signal(),
            3.0
        );
        assert_eq!(
            InvariantResult::Failed {
                summary: "e".into(),
                elapsed_ms: 5
            }
            .reward_signal(),
            -1.0
        );
        assert_eq!(
            InvariantResult::Skipped {
                reason: "r".into()
            }
            .reward_signal(),
            0.0
        );
    }

    #[test]
    fn check_on_empty_dir_fails_or_skips() {
        // 空临时目录：cargo check 失败（无清单）或工具缺失 Skip——两者
        // 都不 panic 且不误报 Passed（fail-closed 语义：不可验证 ≠ 通过）
        let inv = ProjectCompilesInvariant::with_timeout(
            test_root(),
            Duration::from_secs(10),
        );
        let change = Change::new("/tmp/x.rs", Some("nonexistent-crate".into()));
        let result = inv.check(&change);
        assert!(
            matches!(result, InvariantResult::Failed { .. } | InvariantResult::Skipped { .. }),
            "不可验证必须 fail-closed: {result:?}"
        );
    }

    #[test]
    fn timeout_bounded() {
        // 超时构造不 panic（快速路径验证构造正确性）
        let inv = ProjectCompilesInvariant::with_timeout(test_root(), Duration::from_millis(1));
        let change = Change::new("/tmp/y.rs", None);
        let _ = inv.check(&change);
    }
}
