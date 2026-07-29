//! gVisor runsc 运行时检测与子进程启动
//!
//! 对应架构层:L4 Security
//! 对应 ADR:ADR-001(gVisor 运行时选择)
//!
//! gVisor 是用户空间内核,通过拦截系统调用实现内核级隔离。
//! runsc 是 gVisor 的 OCI 兼容运行时二进制。
//!
//! 跨平台策略:
//! - **Linux 生产环境**:通过 runsc 在 gVisor 沙箱中执行命令,提供内核级隔离。
//! - **Windows/macOS**:`is_available()` 始终返回 false,回退到进程级隔离
//!   (`sandbox::execute_in_sandbox` 当前的 tokio::process::Command 降级实现)。
//!
//! # 安全措施
//! - 沙箱名使用 UUID 防冲突: `{prefix}-{uuid}`
//! - `--network=none` 禁用网络访问
//! - `kill_on_drop` 确保超时时子进程被终止
//! - 环境变量清零后仅注入白名单变量

use std::process::Stdio;

use tokio::process::Command as TokioCommand;
use tracing::debug;

use crate::error::SecCoreError;
use crate::types::CommandSpec;

/// gVisor 运行时 — 封装 runsc 路径检测与子进程启动
///
/// 仅在 Linux 平台可用;Windows/macOS 上 `is_available()` 始终返回 false。
///
/// # 使用示例
/// ```no_run
/// use seccore::gvisor::GvisorRuntime;
///
/// let runtime = GvisorRuntime::detect("/usr/local/bin/runsc");
/// if let Some(rt) = runtime {
///     if rt.is_available() {
///         println!("gVisor 运行时可用: {}", rt.runsc_path());
///     }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GvisorRuntime {
    /// runsc 二进制路径
    runsc_path: String,
    /// 平台标识(用于日志与平台检测)
    platform: String,
}

impl GvisorRuntime {
    /// 检测 runsc 二进制是否可用
    ///
    /// 检查 `runsc_path` 路径是否存在且可执行(通过 `Path::exists()` 检测)。
    /// 返回 `Some(Self)` 当路径存在,否则 `None`。
    ///
    /// # 参数
    /// - `runsc_path`: runsc 二进制路径(如 `/usr/local/bin/runsc`)
    ///
    /// # 注意
    /// 此方法仅检测文件存在性,不验证执行权限或 gVisor 版本。
    /// 实际可用性由 `is_available()` 做平台+文件双重检测。
    pub fn detect(runsc_path: &str) -> Option<Self> {
        let path = std::path::Path::new(runsc_path);
        if !path.exists() {
            debug!(runsc_path = %runsc_path, "runsc 二进制不存在");
            return None;
        }
        Some(Self {
            runsc_path: runsc_path.to_string(),
            platform: std::env::consts::OS.to_string(),
        })
    }

    /// 检查 gVisor 运行时是否在当前平台上可用
    ///
    /// 双重检测:
    /// 1. 平台检查:非 Linux 返回 `false`
    /// 2. 二进制检查:runsc 文件是否存在
    pub fn is_available(&self) -> bool {
        // 仅 Linux 平台支持 gVisor
        if self.platform != "linux" {
            return false;
        }
        std::path::Path::new(&self.runsc_path).exists()
    }

    /// 获取 runsc 二进制路径(用于日志/诊断)
    pub fn runsc_path(&self) -> &str {
        &self.runsc_path
    }

    /// 通过 runsc 在 gVisor 沙箱中执行命令
    ///
    /// # 参数
    /// - `spec`: 已通过策略校验的命令规格
    ///
    /// # 安全措施
    /// - 沙箱名使用 UUID 防冲突: `chimera-{uuid}`
    /// - `--network=none` 禁用网络访问
    /// - `kill_on_drop` 确保超时时子进程被终止
    /// - 环境变量清零后仅注入白名单变量
    ///
    /// # 注意
    /// 由于 runsc 需要 OCI bundle,实际测试中可能无法直接使用。
    /// 当前实现采用简化的 `runsc run` 模式,后续可增强为完整 OCI bundle 生成。
    /// 超时控制由调用方通过 `tokio::time::timeout` 处理(不在此方法内)。
    pub async fn spawn(&self, spec: &CommandSpec) -> Result<std::process::Output, SecCoreError> {
        let sandbox_id = format!("chimera-{}", uuid::Uuid::new_v4());

        debug!(
            runsc_path = %self.runsc_path,
            sandbox_id = %sandbox_id,
            program = %spec.program,
            "通过 runsc 在 gVisor 沙箱中启动子进程"
        );

        let mut cmd = TokioCommand::new(&self.runsc_path);
        cmd.arg("--rootless=true") // 非 root 模式
            .arg("--network=none") // 禁用网络
            .arg("--ignore-cgroups=true") // 非 root 时忽略 cgroup
            .arg("run")
            .arg("--bundle=/tmp") // 最小 OCI bundle(不需要完整 rootfs)
            .arg(&sandbox_id)
            .arg(&spec.program);

        // 添加命令参数
        for arg in &spec.allowed_args {
            cmd.arg(arg);
        }

        // 环境变量:清零后仅注入白名单变量(零信任原则)
        cmd.env_clear();
        for (k, v) in &spec.env_whitelist {
            cmd.env(k, v);
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        let output = cmd
            .output()
            .await
            .map_err(|e| SecCoreError::SandboxError(format!("gVisor runsc 执行失败: {}", e)))?;

        debug!(
            sandbox_id = %sandbox_id,
            exit_code = ?output.status.code(),
            "gVisor 沙箱子进程已退出"
        );

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gvisor_runtime_detect_nonexistent_path() {
        // 检测不存在的路径应返回 None
        let runtime = GvisorRuntime::detect("/nonexistent/runsc");
        assert!(runtime.is_none());
    }

    #[test]
    fn test_gvisor_runtime_is_available_non_linux() {
        // 非 Linux 平台上 is_available() 应返回 false
        let runtime = GvisorRuntime {
            runsc_path: "/usr/local/bin/runsc".into(),
            platform: "windows".into(),
        };
        assert!(!runtime.is_available());
    }

    #[test]
    fn test_gvisor_runtime_is_available_linux_no_binary() {
        // Linux 平台上但 runsc 不存在,应返回 false
        let runtime = GvisorRuntime {
            runsc_path: "/nonexistent/runsc".into(),
            platform: "linux".into(),
        };
        assert!(!runtime.is_available());
    }

    #[test]
    fn test_gvisor_config_default() {
        let config = crate::types::GvisorConfig::default();
        assert_eq!(config.runsc_path, "/usr/local/bin/runsc");
        assert_eq!(config.sandbox_name_prefix, "chimera");
        assert!(config.network_disabled);
        assert_eq!(config.platform, "linux");
    }

    #[test]
    fn test_gvisor_runtime_runsc_path_getter() {
        let runtime = GvisorRuntime {
            runsc_path: "/custom/path/runsc".into(),
            platform: "linux".into(),
        };
        assert_eq!(runtime.runsc_path(), "/custom/path/runsc");
    }
}
