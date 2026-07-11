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

use std::time::Duration;

#[cfg(not(windows))]
use sha2::{Digest, Sha256};
#[cfg(not(windows))]
use std::process::Stdio;
#[cfg(not(windows))]
use std::time::Instant;
#[cfg(not(windows))]
use tokio::process::Command as TokioCommand;
use tracing::{info, warn};

use crate::audit::{AuditChain, AuditRecordStatus};
use crate::error::SecCoreError;
use crate::policy::{validate_command, validate_env, CommandPolicy, EnvPolicy};
use crate::spectral_attention::SpectralAttentionAnalyzer;
use crate::types::{Command, CommandSpec, ExecutionResult};
use crate::windows_sandbox::WindowsSandboxExecutor;

/// 零信任沙箱 — 封装策略、环境策略、审计链与 Spectral Attention 分析器,提供统一的执行入口。
///
/// 所有外部命令必须经 `Sandbox::audit_and_execute` 执行,
/// 确保经过四层防御:静态分析 → 环境过滤 → 沙箱执行 → 审计记录 → 频谱分析。
pub struct Sandbox {
    /// 命令策略(白名单 + 拦截模式)
    pub policy: CommandPolicy,
    /// 环境变量策略(白名单 + 敏感模式)
    pub env_policy: EnvPolicy,
    /// 审计链(SHA-256 Merkle 链)
    pub audit_chain: AuditChain,
    /// Spectral Attention 安全审计分析器
    ///
    /// WHY: 在审计链基础上增加图注意力分析,检测命令执行序列中的异常模式
    /// (周期性攻击、异常密集连接、安全关键头异常)。分析结果仅记录告警,
    /// 不阻塞命令执行(事后分析层)。
    pub spectral_analyzer: SpectralAttentionAnalyzer,
    /// 沙箱执行超时 — 防止恶意命令(如 `sleep infinity`)永久阻塞,导致 DoS (F-002)。
    ///
    /// WHY: 无超时限制时,恶意命令可永久阻塞子进程,耗尽调度资源造成 DoS。
    /// 默认 30 秒,可通过 `with_timeout` 按场景调整(如长命令设为 5 分钟)。
    pub timeout: Duration,
}

impl Sandbox {
    /// 创建沙箱,携带指定的命令策略与环境变量策略。
    ///
    /// 默认超时 30 秒(防止恶意命令永久阻塞),可用 `with_timeout` 调整。
    /// 默认启用 Spectral Attention 分析器。
    pub fn new(policy: CommandPolicy, env_policy: EnvPolicy) -> Self {
        Self {
            policy,
            env_policy,
            audit_chain: AuditChain::new(),
            spectral_analyzer: SpectralAttentionAnalyzer::with_default_config(),
            timeout: Duration::from_secs(30),
        }
    }

    /// 创建使用默认安全策略的沙箱。
    pub fn with_default_policy() -> Self {
        Self::new(CommandPolicy::default_secure(), EnvPolicy::default_secure())
    }

    /// 链式设置沙箱执行超时(F-002)。
    ///
    /// WHY: 不同场景命令耗时差异大,需可配置超时。短命令设小超时快速失败,
    /// 长命令(如构建)设大超时避免误杀。默认 30 秒。
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// 链式设置 Spectral Attention 配置。
    ///
    /// 用于调整异常检测阈值、注意力权重、容量限制等参数。
    pub fn with_spectral_config(
        mut self,
        config: crate::spectral_attention::SpectralConfig,
    ) -> Self {
        self.spectral_analyzer = SpectralAttentionAnalyzer::new(config);
        self
    }

    /// 审计并执行命令 — 零信任四层防御 + Spectral Attention 增强的统一入口。
    ///
    /// 执行流程(N5 修复:pre-execution audit 模式 + Spectral Attention 事后分析):
    /// 1. `validate_command`:静态分析,拦截注入/越权/逃逸/泄露/篡改/滥用
    /// 2. `validate_env`:环境变量过滤,拦截 SECRET/KEY/TOKEN 泄露
    /// 3. `audit_chain.append_intent`:**执行前**记录 Intent 审计块(关闭 N5 漏洞)
    /// 4. `execute_in_sandbox`:进程隔离执行(Windows 降级 / Linux gVisor)
    /// 5. `audit_chain.update_status`:执行后更新为 Executed/Failed
    /// 6. `spectral_analyzer.add_execution_record` + `analyze`: Spectral Attention 分析,
    ///    检测异常模式并记录告警(不阻塞返回)
    ///
    /// WHY(N5 修复): 原实现步骤3在步骤4之后(后置 append),若执行成功但 append
    /// 失败则无审计痕迹。改为 pre-execution 模式:执行前先写 Intent,即使后续
    /// 崩溃也有意图痕迹;执行失败也更新为 Failed,保持审计链完整。
    ///
    /// WHY(Spectral Attention): 审计链记录单次命令的哈希,但无法检测跨命令的
    /// 异常模式(如周期性攻击、异常依赖链)。Spectral Attention 将执行序列建模为图,
    /// 通过频谱分析检测这些模式,作为审计链的增强层。分析失败不影响命令执行结果。
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

        // 步骤3(N5 修复):pre-execution audit — 执行前记录 Intent
        // WHY: append_intent 失败时 `?` 短路,阻止命令执行,确保无意图无执行
        let record_id = self.audit_chain.append_intent(&spec)?;

        // 步骤4:沙箱执行 — 进程隔离(Windows 降级 / Linux gVisor)
        let exec_result = self.execute_in_sandbox(&spec).await;

        // 步骤5(N5 修复):post-execution update — 根据执行结果更新审计状态
        // WHY: 无论成功失败都要更新审计链,防止 Intent 记录永久悬挂
        let final_result = match exec_result {
            Ok(result) => {
                // 执行成功:更新为 Executed,填充 result_hash
                if let Err(e) = self.audit_chain.update_status(
                    record_id,
                    AuditRecordStatus::Executed,
                    Some(&result),
                ) {
                    // WHY: update_status 失败不影响已执行的命令结果,但记录错误供审计
                    // 审计链更新失败是严重异常(理论上不会发生),仅记日志不阻塞返回
                    tracing::error!(
                        record_id = record_id,
                        error = %e,
                        "审计链 update_status(Executed) 失败,执行结果仍返回但审计可能不完整"
                    );
                }

                info!(
                    exit_code = result.exit_code,
                    audit_hash = %result.audit_hash,
                    "命令执行完成,审计记录已更新为 Executed"
                );

                Ok(result)
            }
            Err(e) => {
                // 执行失败:更新为 Failed,保持审计链完整(记录失败意图)
                // WHY: 用 let _ = 忽略 update_status 的二次错误,优先返回原始执行错误
                //      审计更新失败仅记日志,不掩盖原始执行失败原因
                if let Err(audit_err) =
                    self.audit_chain
                        .update_status(record_id, AuditRecordStatus::Failed, None)
                {
                    tracing::error!(
                        record_id = record_id,
                        error = %audit_err,
                        "审计链 update_status(Failed) 失败,执行错误仍返回但审计可能不完整"
                    );
                }

                info!(error = %e, "命令执行失败,审计记录已更新为 Failed");

                Err(e)
            }
        };

        // 步骤6:Spectral Attention 分析 — 事后增强审计(不阻塞返回)
        // WHY: 分析失败仅记录日志,不影响命令执行结果。分析器从 spec 和 result 构建图,
        //      检测跨命令的异常模式。只有在 final_result 是 Ok 时才传入 result,
        //      Failed 时传入一个占位结果。
        let result_for_analysis = match &final_result {
            Ok(result) => result.clone(),
            Err(_) => ExecutionResult {
                exit_code: -1,
                stdout: String::new(),
                stderr: String::new(),
                duration: Duration::from_secs(0),
                audit_hash: "0".repeat(64),
            },
        };
        self.spectral_analyzer
            .add_execution_record(&spec, &result_for_analysis);
        match self.spectral_analyzer.analyze() {
            Ok(analysis) => {
                if !analysis.critical_head_alerts.is_empty() {
                    for alert in &analysis.critical_head_alerts {
                        warn!(
                            head_type = %alert.head_type,
                            alert_level = ?alert.alert_level,
                            attention_score = alert.attention_score,
                            description = %alert.description,
                            "Spectral Attention 安全关键头告警"
                        );
                    }
                }
                if analysis.anomaly_level != crate::spectral_attention::AnomalyLevel::Normal {
                    warn!(
                        anomaly_level = ?analysis.anomaly_level,
                        anomaly_score = analysis.anomaly_score,
                        periodicity_score = analysis.periodicity_score,
                        "Spectral Attention 检测到异常执行模式"
                    );
                }
                if analysis.anomaly_level == crate::spectral_attention::AnomalyLevel::Critical {
                    info!(
                        anomaly_score = analysis.anomaly_score,
                        "Spectral Attention 判定为 Critical 异常,建议人工复核"
                    );
                }
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "Spectral Attention 分析失败,不影响命令执行结果"
                );
            }
        }

        final_result
    }

    /// 在沙箱中执行校验通过的命令规格。
    ///
    /// 跨平台策略:
    /// - **Windows**: 使用 `WindowsSandboxExecutor` 实现三层降级:
    ///   1. Windows Sandbox API (WSB 配置文件 + WindowsSandbox.exe)
    ///   2. Job Object 限制 (`start /b /low` 低优先级 + 单核亲和)
    ///   3. 标准进程隔离(最终降级)
    /// - **Linux 生产环境**:此处应通过 gVisor runsc 运行时启动子进程,
    ///   并应用 seccomp 过滤器限制系统调用集合。当前实现为降级版本。
    /// - **macOS**:用 `tokio::process::Command` 直接执行,
    ///   依赖策略层的静态分析拦截危险命令。这是降级方案,安全性弱于 Linux。
    ///
    /// # 安全提示
    /// 此函数只接受 `CommandSpec`(已通过策略校验),不接受原始 `Command`。
    /// 调用方必须先调用 `validate_command`。
    async fn execute_in_sandbox(
        &self,
        spec: &CommandSpec,
    ) -> Result<ExecutionResult, SecCoreError> {
        // Windows 平台:使用 WindowsSandboxExecutor 实现三层降级隔离
        #[cfg(windows)]
        {
            let executor = WindowsSandboxExecutor::new(self.timeout).with_job_object(true);
            return executor.execute(spec).await;
        }

        // 非 Windows 平台:使用原有进程隔离(后续可接入 gVisor)
        #[cfg(not(windows))]
        {
            self.execute_standard(spec).await
        }
    }

    /// 标准进程隔离(跨平台降级,非Windows平台使用)
    #[cfg(not(windows))]
    async fn execute_standard(&self, spec: &CommandSpec) -> Result<ExecutionResult, SecCoreError> {
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

        // WHY: kill_on_drop 确保超时 future 被 drop 时子进程被 SIGKILL 强制终止
        // 防止超时后子进程继续运行成为孤儿进程,持续占用资源 (F-002)
        cmd.kill_on_drop(true);

        // WHY: 超时保护 — 防止恶意命令(如 sleep infinity、死循环)永久阻塞,导致 DoS (F-002)
        // tokio::time::timeout 包裹 cmd.output():超时后 future 被 drop,
        // 触发 kill_on_drop 强制终止子进程。cmd.output() 内部并行读取管道与等待,
        // 避免大输出填满管道缓冲区导致死锁(恶意命令可能故意产生大量输出)。
        let output = match tokio::time::timeout(self.timeout, cmd.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                return Err(SecCoreError::SandboxError(format!("进程执行失败: {e}")));
            }
            Err(_) => {
                // 超时:kill_on_drop(true) 已在 future drop 时强制终止子进程
                return Err(SecCoreError::SandboxTimeout {
                    timeout: self.timeout,
                    program: spec.program.clone(),
                });
            }
        };

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
#[cfg(not(windows))]
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

    // === Spectral Attention 集成测试 ===

    #[test]
    fn test_sandbox_spectral_analyzer_default() {
        let sandbox = Sandbox::with_default_policy();
        assert!(sandbox.spectral_analyzer.graph().nodes().is_empty());
        assert_eq!(sandbox.spectral_analyzer.config().anomaly_threshold, 0.7);
    }

    #[test]
    fn test_sandbox_with_spectral_config() {
        let config = crate::spectral_attention::SpectralConfig {
            anomaly_threshold: 0.9,
            max_nodes: 500,
            ..Default::default()
        };
        let sandbox = Sandbox::with_default_policy().with_spectral_config(config);
        assert_eq!(sandbox.spectral_analyzer.config().anomaly_threshold, 0.9);
        assert_eq!(sandbox.spectral_analyzer.config().max_nodes, 500);
    }

    #[tokio::test]
    async fn test_sandbox_spectral_graph_after_blocked() {
        // 被拦截的命令不应进入 Spectral Attention 图
        let mut sandbox = Sandbox::with_default_policy();
        let cmd = Command::new("echo").arg("$(whoami)");
        let _ = sandbox.audit_and_execute(cmd).await;
        assert!(
            sandbox.spectral_analyzer.graph().nodes().is_empty(),
            "被拦截命令不应进入 Spectral Attention 图"
        );
    }
}
