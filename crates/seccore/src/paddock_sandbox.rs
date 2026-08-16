//! Paddock-Sandbox 解耦 — Dressage what-to-do / where-it-runs 职责分离（设计文档 §9.1）
//!
//! 对应架构层: **L4 Security**（seccore 子模块）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §9.1
//! 对应论文: 微软 Dressage（Paddock-Sandbox 解耦）
//! 对应 ADR: ADR-049 决策 1（内嵌 seccore）
//!
//! # 核心职责
//!
//! 职责分离（铁律10）：
//! - **Paddock（what-to-do）**: rollout 生命周期管理（初始化/执行步骤/收尾），
//!   Agent 逻辑不需要知道 sandbox 内部实现
//! - **SandboxProvider / SandboxRuntime（where-it-runs）**: 执行环境抽象，
//!   sandbox 不需要理解 Agent 思考方式
//!
//! # 设计约束（铁律）
//!
//! - **铁律10**: Paddock 不依赖 Sandbox 内部实现——通过 [`SandboxRuntime`] trait
//!   抽象解耦，`ProcessSandboxRuntime` 适配 seccore [`Sandbox`]，可替换为
//!   LocalBubblewrap / RemoteE2B / Custom
//! - **简化说明**: 规范原型的 pause/resume/token_evidence 分段归 L1 `segment_per`，
//!   证据收集归 L1 `TokenLedger`；本模块仅产出执行结果（StepResult/RolloutOutcome）
//! - **参考先例**: pvl-layer `auto_builder.rs` 的 `SandboxExec` trait 同款抽象模式

use async_trait::async_trait;

use crate::error::SecCoreError;
use crate::sandbox::Sandbox;
use crate::types::Command;

// ============================================================
// SandboxRuntime trait（铁律10 抽象边界）
// ============================================================

/// 沙箱执行输出 — SandboxRuntime 执行结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxExecutionOutput {
    /// 进程退出码（0 = 成功）
    pub exit_code: i32,
    /// 标准输出
    pub stdout: String,
    /// 标准错误
    pub stderr: String,
}

/// 沙箱运行时抽象 — where-it-runs（铁律10：Paddock 仅依赖此 trait）
///
/// 实现方可提供本地进程隔离（ProcessSandboxRuntime）、bubblewrap、
/// 远程 E2B、自定义沙箱。Paddock 不感知具体实现。
#[async_trait]
pub trait SandboxRuntime: Send {
    /// 在沙箱中执行命令，返回执行输出
    async fn execute(&mut self, command: Command) -> Result<SandboxExecutionOutput, SecCoreError>;

    /// 清理沙箱资源（rollout 收尾时调用）
    fn cleanup(&mut self) -> Result<(), SecCoreError>;
}

/// 进程沙箱运行时 — 适配 seccore [`Sandbox`] 的默认实现
///
/// 复用 Sandbox 的四层防御（静态分析 + 环境过滤 + 进程隔离 + Merkle 审计）。
pub struct ProcessSandboxRuntime {
    sandbox: Sandbox,
}

impl ProcessSandboxRuntime {
    /// 创建进程沙箱运行时（默认安全策略）
    pub fn new() -> Self {
        Self {
            sandbox: Sandbox::with_default_policy(),
        }
    }

    /// 从已有 Sandbox 构造（自定义策略）
    pub fn from_sandbox(sandbox: Sandbox) -> Self {
        Self { sandbox }
    }
}

impl Default for ProcessSandboxRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SandboxRuntime for ProcessSandboxRuntime {
    async fn execute(&mut self, command: Command) -> Result<SandboxExecutionOutput, SecCoreError> {
        let result = self.sandbox.audit_and_execute(command).await?;
        Ok(SandboxExecutionOutput {
            exit_code: result.exit_code,
            stdout: result.stdout,
            stderr: result.stderr,
        })
    }

    fn cleanup(&mut self) -> Result<(), SecCoreError> {
        // 进程沙箱每次执行即结束，无持久资源需清理
        Ok(())
    }
}

// ============================================================
// SandboxType / SandboxProvider（where-it-runs）
// ============================================================

/// 沙箱类型 — 执行环境选择
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxType {
    /// 本地 bubblewrap（Linux 轻量隔离）
    LocalBubblewrap,
    /// 远程 E2B（云端沙箱）
    RemoteE2B,
    /// 自定义沙箱
    Custom,
}

/// 沙箱提供者 — 持有沙箱类型与运行时实现
pub struct SandboxProvider {
    /// 沙箱类型标识
    pub sandbox_type: SandboxType,
    /// 运行时实现（铁律10 抽象，Box<dyn> 允许多态替换）
    runtime: Box<dyn SandboxRuntime>,
}

impl SandboxProvider {
    /// 创建沙箱提供者
    pub fn new(sandbox_type: SandboxType, runtime: Box<dyn SandboxRuntime>) -> Self {
        Self {
            sandbox_type,
            runtime,
        }
    }

    /// 默认进程沙箱提供者（本地进程隔离）
    pub fn process() -> Self {
        Self::new(
            SandboxType::LocalBubblewrap,
            Box::new(ProcessSandboxRuntime::new()),
        )
    }

    /// 可变访问运行时（供 Paddock 调用）
    pub(crate) fn runtime_mut(&mut self) -> &mut dyn SandboxRuntime {
        self.runtime.as_mut()
    }
}

// ============================================================
// Paddock（what-to-do）
// ============================================================

/// Rollout 上下文 — 单次 rollout 的会话与任务信息
#[derive(Debug, Clone)]
pub struct RolloutContext {
    /// 会话 ID（rollout 唯一标识）
    pub session_id: String,
    /// 任务描述
    pub task_description: String,
}

/// 单步执行结果
#[derive(Debug, Clone)]
pub struct StepResult {
    /// 所属会话 ID
    pub session_id: String,
    /// Agent 输出（stdout）
    pub agent_output: String,
    /// 验证是否成功（exit_code == 0）
    pub verification_success: bool,
    /// 完整执行输出
    pub execution_output: SandboxExecutionOutput,
}

/// Rollout 收尾结果
#[derive(Debug, Clone)]
pub struct RolloutOutcome {
    /// 会话 ID
    pub session_id: String,
    /// 全部步骤结果
    pub step_results: Vec<StepResult>,
    /// 整体是否成功（所有步骤都成功）
    pub overall_success: bool,
}

/// Paddock — rollout 生命周期管理（what-to-do）
///
/// 铁律10: 仅依赖 [`SandboxRuntime`] trait，不感知具体沙箱实现。
pub struct Paddock {
    sandbox_provider: SandboxProvider,
}

impl Paddock {
    /// 创建 Paddock（注入沙箱提供者）
    pub fn new(sandbox_provider: SandboxProvider) -> Self {
        Self { sandbox_provider }
    }

    /// 初始化 rollout — 生成会话 ID 与任务上下文
    pub fn initialize_rollout(&self, task_description: &str) -> RolloutContext {
        RolloutContext {
            session_id: uuid::Uuid::new_v4().to_string(),
            task_description: task_description.to_string(),
        }
    }

    /// 执行单步 — 委托沙箱运行时执行命令（铁律10）
    ///
    /// 验证成功判定: exit_code == 0。
    pub async fn execute_step(
        &mut self,
        ctx: &RolloutContext,
        command: Command,
    ) -> Result<StepResult, SecCoreError> {
        let output = self.sandbox_provider.runtime_mut().execute(command).await?;
        let verification_success = output.exit_code == 0;
        Ok(StepResult {
            session_id: ctx.session_id.clone(),
            agent_output: output.stdout.clone(),
            verification_success,
            execution_output: output,
        })
    }

    /// 收尾 rollout — 清理沙箱 + 聚合步骤结果
    pub fn finalize_rollout(
        &mut self,
        ctx: &RolloutContext,
        steps: Vec<StepResult>,
    ) -> Result<RolloutOutcome, SecCoreError> {
        self.sandbox_provider.runtime_mut().cleanup()?;
        let overall_success = steps.iter().all(|s| s.verification_success);
        Ok(RolloutOutcome {
            session_id: ctx.session_id.clone(),
            step_results: steps,
            overall_success,
        })
    }

    /// 沙箱类型只读访问（可观测性）
    pub fn sandbox_type(&self) -> SandboxType {
        self.sandbox_provider.sandbox_type
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用 mock 沙箱运行时（记录执行的命令数，不真实执行）
    struct MockRuntime {
        executed: u32,
        cleaned: bool,
        exit_code: i32,
    }

    impl MockRuntime {
        fn new(exit_code: i32) -> Self {
            Self {
                executed: 0,
                cleaned: false,
                exit_code,
            }
        }
    }

    #[async_trait]
    impl SandboxRuntime for MockRuntime {
        async fn execute(
            &mut self,
            _command: Command,
        ) -> Result<SandboxExecutionOutput, SecCoreError> {
            self.executed += 1;
            Ok(SandboxExecutionOutput {
                exit_code: self.exit_code,
                stdout: format!("output-{}", self.executed),
                stderr: String::new(),
            })
        }

        fn cleanup(&mut self) -> Result<(), SecCoreError> {
            self.cleaned = true;
            Ok(())
        }
    }

    fn mock_paddock(exit_code: i32) -> Paddock {
        let provider =
            SandboxProvider::new(SandboxType::Custom, Box::new(MockRuntime::new(exit_code)));
        Paddock::new(provider)
    }

    #[test]
    fn initialize_rollout_generates_unique_session() {
        let paddock = mock_paddock(0);
        let ctx1 = paddock.initialize_rollout("task A");
        let ctx2 = paddock.initialize_rollout("task B");
        assert_ne!(ctx1.session_id, ctx2.session_id, "会话 ID 应唯一");
        assert_eq!(ctx1.task_description, "task A");
    }

    #[tokio::test]
    async fn execute_step_success_exit_zero() {
        let mut paddock = mock_paddock(0);
        let ctx = paddock.initialize_rollout("task");
        let cmd = Command::new("echo").arg("hello");
        let result = paddock.execute_step(&ctx, cmd).await.expect("执行成功");
        assert!(result.verification_success, "exit_code 0 应验证成功");
        assert_eq!(result.session_id, ctx.session_id);
        assert_eq!(result.agent_output, "output-1");
    }

    #[tokio::test]
    async fn execute_step_failure_nonzero_exit() {
        let mut paddock = mock_paddock(1);
        let ctx = paddock.initialize_rollout("task");
        let cmd = Command::new("false");
        let result = paddock.execute_step(&ctx, cmd).await.expect("执行完成");
        assert!(!result.verification_success, "exit_code 1 应验证失败");
    }

    #[tokio::test]
    async fn rollout_lifecycle_initialize_execute_finalize() {
        let mut paddock = mock_paddock(0);
        // 初始化
        let ctx = paddock.initialize_rollout("build project");
        // 执行多步
        let mut steps = Vec::new();
        for _ in 0..3 {
            let step = paddock
                .execute_step(&ctx, Command::new("echo").arg("step"))
                .await
                .expect("执行成功");
            steps.push(step);
        }
        // 收尾
        let outcome = paddock.finalize_rollout(&ctx, steps).expect("收尾成功");
        assert_eq!(outcome.step_results.len(), 3);
        assert!(outcome.overall_success, "全部成功应 overall_success");
        assert_eq!(outcome.session_id, ctx.session_id);
    }

    #[tokio::test]
    async fn rollout_overall_failure_if_any_step_fails() {
        let mut paddock = mock_paddock(1); // 全部失败
        let ctx = paddock.initialize_rollout("task");
        let mut steps = Vec::new();
        for _ in 0..2 {
            let step = paddock
                .execute_step(&ctx, Command::new("false"))
                .await
                .expect("执行完成");
            steps.push(step);
        }
        let outcome = paddock.finalize_rollout(&ctx, steps).expect("收尾成功");
        assert!(!outcome.overall_success, "有失败步骤应 overall_failure");
    }

    #[test]
    fn iron_law_10_paddock_depends_only_on_trait() {
        // 铁律10 验证: Paddock 可注入任意 SandboxRuntime 实现（Custom mock），
        // 不依赖 Sandbox 具体实现——编译期保证（MockRuntime 非 Sandbox）
        let paddock = mock_paddock(0);
        assert_eq!(paddock.sandbox_type(), SandboxType::Custom);
    }

    #[test]
    fn process_provider_default_type() {
        let provider = SandboxProvider::process();
        assert_eq!(provider.sandbox_type, SandboxType::LocalBubblewrap);
    }

    #[test]
    fn sandbox_type_variants() {
        // 三种沙箱类型可达
        let types = [
            SandboxType::LocalBubblewrap,
            SandboxType::RemoteE2B,
            SandboxType::Custom,
        ];
        assert_eq!(types.len(), 3);
    }
}
