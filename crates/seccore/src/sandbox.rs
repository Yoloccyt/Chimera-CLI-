//! 沙箱执行 — 零信任沙箱的核心执行层
//!
//! 对应尸检教训:
//! - Claude Code 命令直接在用户 shell 执行,无隔离
//! - 审计日志在执行后才记录,可被绕过
//!
//! 跨平台策略(WHY):
//! - **Linux 生产环境**:应通过 gVisor(runsc)运行时 + seccomp 过滤器启动子进程,
//!   实现内核级隔离与系统调用过滤。gVisor 拦截系统调用,seccomp 限制调用集合。
//! - **Windows/macOS 降级**:无 gVisor/seccomp 等效物,降级为"进程隔离 + 白名单"
//!   模拟层。用 `tokio::process::Command` 直接执行,依赖策略层的静态分析拦截
//!   危险命令。这是**降级方案**,安全性弱于 Linux 生产环境。
//! - **ADR-001**:沙箱运行时选择 gVisor,Linux 优先。
//!
//! 四层防御(对应架构红线):
//! 1. 静态分析(validate_command):拦截注入/越权/逃逸/泄露/篡改
//! 2. 环境过滤(validate_env):拦截 SECRET/KEY/TOKEN 泄露
//! 3. 沙箱执行(execute_in_sandbox):进程隔离(Windows 降级)/gVisor(Linux)
//! 4. 审计记录(audit_chain.append):SHA-256 Merkle 链,不可篡改

use std::process::Stdio;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tokio::process::Command as TokioCommand;
use tracing::info;

use crate::audit::AuditChain;
use crate::error::SecCoreError;
use crate::policy::{validate_command, validate_env, CommandPolicy, EnvPolicy};
use crate::types::{Command, CommandSpec, ExecutionResult};

/// 零信任沙箱 — 封装策略、环境策略与审计链,提供统一的执行入口。
///
/// 所有外部命令必须经 `Sandbox::audit_and_execute` 执行,
/// 确保经过四层防御:静态分析 → 环境过滤 → 沙箱执行 → 审计记录。
pub struct Sandbox {
    /// 命令策略(白名单 + 拦截模式)
    pub policy: CommandPolicy,
    /// 环境变量策略(白名单 + 敏感模式)
    pub env_policy: EnvPolicy,
    /// 审计链(SHA-256 Merkle 链)
    pub audit_chain: AuditChain,
}

impl Sandbox {
    /// 创建沙箱,携带指定的命令策略与环境变量策略。
    pub fn new(policy: CommandPolicy, env_policy: EnvPolicy) -> Self {
        Self {
            policy,
            env_policy,
            audit_chain: AuditChain::new(),
        }
    }

    /// 创建使用默认安全策略的沙箱。
    pub fn with_default_policy() -> Self {
        Self::new(CommandPolicy::default_secure(), EnvPolicy::default_secure())
    }

    /// 审计并执行命令 — 零信任四层防御的统一入口。
    ///
    /// 执行流程:
    /// 1. `validate_command`:静态分析,拦截注入/越权/逃逸/泄露/篡改/滥用
    /// 2. `validate_env`:环境变量过滤,拦截 SECRET/KEY/TOKEN 泄露
    /// 3. `execute_in_sandbox`:进程隔离执行(Windows 降级 / Linux gVisor)
    /// 4. `audit_chain.append`:SHA-256 Merkle 链记录,不可篡改
    ///
    /// # 参数
    /// - `command`:原始命令(不可信,需经策略校验)
    ///
    /// # 返回
    /// - `Ok(ExecutionResult)`:执行成功,携带退出码、输出、审计哈希
    /// - `Err(SecCoreError)`:任一防御层拦截或执行失败
    pub async fn audit_and_execute(
        &mut self,
        command: Command,
    ) -> Result<ExecutionResult, SecCoreError> {
        // 步骤1:静态分析 — 拦截注入/越权/逃逸/泄露/篡改/滥用
        let mut spec = validate_command(&command, &self.policy)?;

        // 步骤2:环境变量过滤 — 拦截 SECRET/KEY/TOKEN 泄露
        let filtered_env = validate_env(&command.env, &self.env_policy)?;
        spec.env_whitelist = filtered_env;

        info!(
            program = %spec.program,
            risk_level = ?spec.risk_level,
            "命令通过策略校验,进入沙箱执行"
        );

        // 步骤3:沙箱执行 — 进程隔离(Windows 降级 / Linux gVisor)
        let result = self.execute_in_sandbox(&spec).await?;

        // 步骤4:审计记录 — SHA-256 Merkle 链
        self.audit_chain.append(&spec, &result)?;

        info!(
            exit_code = result.exit_code,
            audit_hash = %result.audit_hash,
            "命令执行完成,审计记录已追加"
        );

        Ok(result)
    }

    /// 在沙箱中执行校验通过的命令规格。
    ///
    /// 跨平台策略:
    /// - **Linux 生产环境**:此处应通过 gVisor runsc 运行时启动子进程,
    ///   并应用 seccomp 过滤器限制系统调用集合。当前实现为降级版本。
    /// - **Windows/macOS**:用 `tokio::process::Command` 直接执行,
    ///   依赖策略层的静态分析拦截危险命令。这是降级方案,安全性弱于 Linux。
    ///
    /// # 安全提示
    /// 此函数只接受 `CommandSpec`(已通过策略校验),不接受原始 `Command`。
    /// 调用方必须先调用 `validate_command`。
    async fn execute_in_sandbox(
        &self,
        spec: &CommandSpec,
    ) -> Result<ExecutionResult, SecCoreError> {
        let start = Instant::now();

        // 构建子进程命令
        // 注意:此处不使用 shell(无 sh -c),避免 shell 注入风险
        // 参数直接传递给 execve,不经 shell 二次解析
        let mut cmd = TokioCommand::new(&spec.program);
        cmd.args(&spec.allowed_args);

        // 仅传递白名单过滤后的环境变量(零信任:不继承父进程环境)
        // 注意:cmd.env() 是增量设置,需先 clear_env 清除继承
        cmd.env_clear();
        for (k, v) in &spec.env_whitelist {
            cmd.env(k, v);
        }

        // 捕获 stdout/stderr,避免继承父进程终端
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // 执行并等待完成
        let output = cmd
            .output()
            .await
            .map_err(|e| SecCoreError::SandboxError(format!("进程启动失败: {e}")))?;

        let duration = start.elapsed();

        // 解码输出(UTF-8 失败时用替换字符,避免 panic)
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

        // 退出码:信号终止时 code() 返回 None,用 -1 表示
        let exit_code = output.status.code().unwrap_or(-1);

        // 计算审计哈希(执行结果摘要,用于审计链)
        let audit_hash = compute_audit_hash(exit_code, &stdout, &stderr, duration);

        Ok(ExecutionResult {
            exit_code,
            stdout,
            stderr,
            duration,
            audit_hash,
        })
    }
}

/// 计算执行结果的审计哈希(SHA-256 十六进制)。
///
/// 哈希内容:exit_code || stdout || stderr || duration_nanos。
/// 此哈希存储在 `ExecutionResult.audit_hash`,用于快速比对。
/// 审计链验证时会重新计算(不信任此字段),防止篡改。
fn compute_audit_hash(exit_code: i32, stdout: &str, stderr: &str, duration: Duration) -> String {
    let mut hasher = Sha256::new();
    hasher.update(exit_code.to_le_bytes());
    hasher.update(stdout.as_bytes());
    hasher.update(stderr.as_bytes());
    hasher.update(duration.as_nanos().to_le_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sandbox_blocks_injection() {
        let mut sandbox = Sandbox::with_default_policy();
        let cmd = Command::new("echo").arg("$(whoami)");
        let result = sandbox.audit_and_execute(cmd).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_sandbox_blocks_env_leak() {
        let mut sandbox = Sandbox::with_default_policy();
        let cmd = Command::new("echo").arg("hello").env("SECRET_KEY", "leak");
        let result = sandbox.audit_and_execute(cmd).await;
        assert!(result.is_err());
    }
}
