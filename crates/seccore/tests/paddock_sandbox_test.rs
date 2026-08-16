//! Paddock-Sandbox 解耦集成测试 — 顶层 API + rollout 生命周期 + 铁律10（v3.4.0 §9.1）
//!
//! 覆盖: 顶层 API 可达性 / rollout 生命周期端到端 / 铁律10 解耦验证 /
//! ProcessSandboxRuntime 构造 / SandboxType 三类型

#![forbid(unsafe_code)]

use async_trait::async_trait;
use seccore::paddock_sandbox::SandboxExecutionOutput;
use seccore::SecCoreError;
use seccore::{
    Command, CommandPolicy, EnvPolicy, Paddock, ProcessSandboxRuntime, Sandbox, SandboxProvider,
    SandboxRuntime, SandboxType,
};

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    // ProcessSandboxRuntime 与 SandboxProvider::process 可通过顶层访问
    let _runtime = ProcessSandboxRuntime::new();
    let provider = SandboxProvider::process();
    assert_eq!(provider.sandbox_type, SandboxType::LocalBubblewrap);
    let paddock = Paddock::new(provider);
    assert_eq!(paddock.sandbox_type(), SandboxType::LocalBubblewrap);
}

// ----------------------------------------------------------
// 铁律10 解耦验证 — Mock 运行时
// ----------------------------------------------------------

/// 测试用 Mock 运行时（可配置退出码，不真实执行）
struct MockRuntime {
    exit_code: i32,
    execute_count: u32,
}

impl MockRuntime {
    fn new(exit_code: i32) -> Self {
        Self {
            exit_code,
            execute_count: 0,
        }
    }
}

#[async_trait]
impl SandboxRuntime for MockRuntime {
    async fn execute(&mut self, _cmd: Command) -> Result<SandboxExecutionOutput, SecCoreError> {
        self.execute_count += 1;
        Ok(SandboxExecutionOutput {
            exit_code: self.exit_code,
            stdout: format!("mock-output-{}", self.execute_count),
            stderr: String::new(),
        })
    }

    fn cleanup(&mut self) -> Result<(), SecCoreError> {
        Ok(())
    }
}

fn mock_paddock(exit_code: i32) -> Paddock {
    let provider = SandboxProvider::new(SandboxType::Custom, Box::new(MockRuntime::new(exit_code)));
    Paddock::new(provider)
}

#[test]
fn iron_law_10_paddock_accepts_arbitrary_runtime() {
    // 铁律10: Paddock 通过 SandboxRuntime trait 接受任意运行时（Custom mock），
    // 编译期保证不依赖 Sandbox 具体实现
    let paddock = mock_paddock(0);
    assert_eq!(paddock.sandbox_type(), SandboxType::Custom);
}

// ----------------------------------------------------------
// rollout 生命周期端到端
// ----------------------------------------------------------

#[tokio::test]
async fn rollout_full_lifecycle_success() {
    let mut paddock = mock_paddock(0);
    // 1. 初始化
    let ctx = paddock.initialize_rollout("build the project");
    assert!(!ctx.session_id.is_empty());
    assert_eq!(ctx.task_description, "build the project");
    // 2. 执行多步
    let mut steps = Vec::new();
    for _ in 0..3 {
        let step = paddock
            .execute_step(&ctx, Command::new("echo").arg("build"))
            .await
            .expect("执行成功");
        assert!(step.verification_success);
        steps.push(step);
    }
    // 3. 收尾
    let outcome = paddock.finalize_rollout(&ctx, steps).expect("收尾成功");
    assert!(outcome.overall_success);
    assert_eq!(outcome.step_results.len(), 3);
    assert_eq!(outcome.session_id, ctx.session_id);
}

#[tokio::test]
async fn rollout_partial_failure_propagates() {
    let mut paddock = mock_paddock(1); // 全部步骤失败
    let ctx = paddock.initialize_rollout("failing task");
    let mut steps = Vec::new();
    for _ in 0..2 {
        let step = paddock
            .execute_step(&ctx, Command::new("false"))
            .await
            .expect("执行完成");
        assert!(!step.verification_success, "exit 1 应验证失败");
        steps.push(step);
    }
    let outcome = paddock.finalize_rollout(&ctx, steps).expect("收尾成功");
    assert!(!outcome.overall_success, "有失败步骤 → overall_failure");
}

#[tokio::test]
async fn unique_session_ids_across_rollouts() {
    let paddock = mock_paddock(0);
    let ctx1 = paddock.initialize_rollout("task A");
    let ctx2 = paddock.initialize_rollout("task B");
    let ctx3 = paddock.initialize_rollout("task C");
    // 会话 ID 唯一（UUID v4）
    assert_ne!(ctx1.session_id, ctx2.session_id);
    assert_ne!(ctx2.session_id, ctx3.session_id);
    assert_ne!(ctx1.session_id, ctx3.session_id);
}

// ----------------------------------------------------------
// ProcessSandboxRuntime 构造与 Sandbox 适配
// ----------------------------------------------------------

#[test]
fn process_runtime_from_custom_sandbox() {
    // ProcessSandboxRuntime 可从自定义 Sandbox 构造（策略注入）
    let sandbox = Sandbox::new(CommandPolicy::default_secure(), EnvPolicy::default_secure());
    let _runtime = ProcessSandboxRuntime::from_sandbox(sandbox);
}

#[test]
fn sandbox_type_three_variants_distinct() {
    assert_ne!(SandboxType::LocalBubblewrap, SandboxType::RemoteE2B);
    assert_ne!(SandboxType::RemoteE2B, SandboxType::Custom);
    assert_ne!(SandboxType::LocalBubblewrap, SandboxType::Custom);
}
