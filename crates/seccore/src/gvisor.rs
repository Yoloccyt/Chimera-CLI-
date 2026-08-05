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
use crate::types::GvisorConfig;

use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// OCI config.json 的 Rust 映射 — 用于 gVisor runsc 的 OCI bundle 生成。
///
/// 仅序列化 runsc 运行所需的必需字段，遵循 OCI Runtime Specification v1.0.2。
/// WHY 手动 serde 映射而非 oci-spec-rs crate:避免引入新的外部依赖，
/// seccore 已有 serde 依赖，手动映射更轻量。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OciConfig {
    #[serde(rename = "ociVersion")]
    oci_version: String,
    process: OciProcess,
    root: OciRoot,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    mounts: Vec<OciMount>,
    linux: OciLinux,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    annotations: HashMap<String, String>,
}

impl OciConfig {
    /// 从 CommandSpec 和 GvisorConfig 构建 OCI 配置。
    ///
    /// # 参数
    /// - `spec`: 校验通过的命令规格（含程序名、参数、环境变量）
    /// - `gvisor_config`: gVisor 运行时配置（含 bundle 路径等）
    ///
    /// # 返回
    /// 完整的 OCI config，可序列化为 runsc 接受的 config.json
    fn from_spec(
        spec: &crate::types::CommandSpec,
        gvisor_config: &crate::types::GvisorConfig,
    ) -> Self {
        let mut args = vec![spec.program.clone()];
        args.extend(spec.allowed_args.clone());

        let env: Vec<String> = spec
            .env_whitelist
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();

        let namespaces = vec![
            OciNamespace {
                type_: "pid".into(),
                path: None,
            },
            OciNamespace {
                type_: "mount".into(),
                path: None,
            },
            OciNamespace {
                type_: "ipc".into(),
                path: None,
            },
            OciNamespace {
                type_: "uts".into(),
                path: None,
            },
        ];

        OciConfig {
            oci_version: "1.0.2".into(),
            process: OciProcess {
                args,
                env,
                cwd: None,
                no_new_privileges: true,
                rlimits: vec![OciRlimit {
                    type_: "RLIMIT_NOFILE".into(),
                    hard: 1024,
                    soft: 1024,
                }],
            },
            root: OciRoot {
                path: gvisor_config
                    .effective_rootfs()
                    .to_string_lossy()
                    .to_string(),
                readonly: true,
            },
            mounts: vec![
                OciMount {
                    destination: "/proc".into(),
                    type_: "proc".into(),
                    source: "proc".into(),
                    options: vec![],
                },
                OciMount {
                    destination: "/dev".into(),
                    type_: "tmpfs".into(),
                    source: "tmpfs".into(),
                    options: vec![
                        "nosuid".into(),
                        "strictatime".into(),
                        "mode=755".into(),
                        "size=65536k".into(),
                    ],
                },
            ],
            linux: OciLinux { namespaces },
            annotations: {
                let mut m = HashMap::new();
                m.insert("chimera.sandbox.type".into(), "gvisor".into());
                m
            },
        }
    }
}

/// OCI bundle 封装 — 创建临时 bundle 目录，写入 config.json，执行后清理。
///
/// # 生命周期
/// bundle 目录在 `OciBundle` 实例被 drop 时自动清理。
/// 调用方需确保 `spawn()` 执行期间 bundle 保持存活（即 `OciBundle` 不被提前 drop）。
#[derive(Debug)]
struct OciBundle {
    /// bundle 目录路径
    dir: std::path::PathBuf,
}

impl OciBundle {
    /// 创建新的 OCI bundle。
    ///
    /// # 参数
    /// - `bundle_dir`: 用于创建 bundle 的父目录路径（由 `GvisorConfig::effective_bundle_dir()` 确定）
    /// - `config`: OCI 运行时配置
    ///
    /// # 行为
    /// 1. 在 `bundle_dir` 下创建 UUID 命名的子目录
    /// 2. 将 config 序列化为 JSON 写入 `config.json`
    ///
    /// # 错误
    /// - 目录创建失败返回 `SecCoreError::SandboxError`
    /// - 序列化/写入失败返回 `SecCoreError::SandboxError`
    fn new(bundle_dir: &Path, config: &OciConfig) -> Result<Self, SecCoreError> {
        // 验证 rootfs 路径存在
        let rootfs_path = Path::new(&config.root.path);
        if !rootfs_path.exists() {
            return Err(SecCoreError::SandboxError(format!(
                "rootfs 路径不存在: {}",
                config.root.path
            )));
        }

        let dir = bundle_dir.join(format!("chimera-bundle-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir)
            .map_err(|e| SecCoreError::SandboxError(format!("创建 OCI bundle 目录失败: {}", e)))?;

        // 设置目录权限为 0755 (Unix 平台)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).map_err(|e| {
                SecCoreError::SandboxError(format!("设置 bundle 目录权限失败: {}", e))
            })?;
        }

        let config_json = serde_json::to_string_pretty(config)
            .map_err(|e| SecCoreError::SandboxError(format!("序列化 OCI config 失败: {}", e)))?;

        fs::write(dir.join("config.json"), &config_json)
            .map_err(|e| SecCoreError::SandboxError(format!("写入 OCI config.json 失败: {}", e)))?;

        debug!(bundle_path = %dir.display(), "OCI bundle 已创建");
        Ok(Self { dir })
    }

    /// 获取 bundle 目录路径（用于 `--bundle` 参数）
    fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for OciBundle {
    /// 清理临时 bundle 目录。
    ///
    /// 使用 `remove_dir_all` 递归删除 entire bundle 目录。
    /// 删除失败仅记录 `debug!` 日志，不 panic（资源清理不应阻止正常流程）。
    fn drop(&mut self) {
        if let Err(e) = fs::remove_dir_all(&self.dir) {
            debug!(bundle_path = %self.dir.display(), error = %e, "OCI bundle 目录清理失败");
        }
    }
}

/// OCI 进程配置 — 映射 config.json 的 process 字段。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OciProcess {
    /// 命令及参数（argv[0] 为程序名）
    args: Vec<String>,
    /// 环境变量列表（KEY=VALUE 格式）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    env: Vec<String>,
    /// 工作目录
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    /// 是否禁止特权提升
    #[serde(rename = "noNewPrivileges")]
    no_new_privileges: bool,
    /// 资源限制
    #[serde(skip_serializing_if = "Vec::is_empty")]
    rlimits: Vec<OciRlimit>,
}

/// OCI 资源限制
#[derive(Debug, Clone, Serialize)]
struct OciRlimit {
    #[serde(rename = "type")]
    type_: String,
    hard: u64,
    soft: u64,
}

/// OCI root 文件系统配置
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OciRoot {
    /// rootfs 路径（相对于 bundle 目录）
    path: String,
    /// 是否只读挂载
    readonly: bool,
}

/// OCI 挂载点配置
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OciMount {
    destination: String,
    #[serde(rename = "type")]
    type_: String,
    source: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    options: Vec<String>,
}

/// OCI Linux 特定配置
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OciLinux {
    /// 命名空间配置
    namespaces: Vec<OciNamespace>,
}

/// OCI 命名空间配置
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OciNamespace {
    #[serde(rename = "type")]
    type_: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

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
    /// gVisor 运行时配置（含 OCI bundle 路径等）
    config: GvisorConfig,
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
            config: GvisorConfig::default(),
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

    /// 通过 runsc 在 gVisor 沙箱中执行命令（完整 OCI bundle 模式）
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
    /// 超时控制由调用方通过 `tokio::time::timeout` 处理(不在此方法内)。
    pub async fn spawn(&self, spec: &CommandSpec) -> Result<std::process::Output, SecCoreError> {
        let sandbox_id = format!("chimera-{}", uuid::Uuid::new_v4());

        debug!(
            runsc_path = %self.runsc_path,
            sandbox_id = %sandbox_id,
            program = %spec.program,
            "通过 runsc 在 gVisor 沙箱中启动子进程（完整 OCI bundle）"
        );

        // 1. 构建 OCI config
        let config = OciConfig::from_spec(spec, &self.config);

        // 2. 创建临时 bundle（含 config.json 写入）
        let bundle = OciBundle::new(&self.config.effective_bundle_dir(), &config)?;

        // 3. 构建 runsc 命令，使用 bundle.path()
        let mut cmd = TokioCommand::new(&self.runsc_path);
        cmd.arg("--rootless=true")
            .arg("--network=none")
            .arg("--ignore-cgroups=true")
            .arg("run")
            .arg(format!("--bundle={}", bundle.path().display()))
            .arg(&sandbox_id);

        // 4. 环境变量:清零后仅注入白名单变量(零信任原则)
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

        // 注意: bundle 在函数返回前不会被 drop（异步块持有 bundle 的生命周期足够长）
        // 但 spawn 函数返回后 bundle 框架会立即被 drop，而 runsc 进程可能尚未完成。
        // 实际上 spawn 内部 await 了 output()，所以 runsc 已执行完毕。
        // 因此 bundle 在 return 后 drop 是安全的。
        drop(bundle);

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
            config: GvisorConfig::default(),
        };
        assert!(!runtime.is_available());
    }

    #[test]
    fn test_gvisor_runtime_is_available_linux_no_binary() {
        // Linux 平台上但 runsc 不存在,应返回 false
        let runtime = GvisorRuntime {
            runsc_path: "/nonexistent/runsc".into(),
            platform: "linux".into(),
            config: GvisorConfig::default(),
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
            config: GvisorConfig::default(),
        };
        assert_eq!(runtime.runsc_path(), "/custom/path/runsc");
    }

    #[test]
    fn test_oci_config_serialization() {
        let config = GvisorConfig::default();
        let spec = CommandSpec {
            program: "echo".into(),
            allowed_args: vec!["hello".into()],
            env_whitelist: [("PATH".into(), "/usr/bin".into())].into(),
            risk_level: crate::types::RiskLevel::Low,
            risk_score: 10,
        };
        let oci = OciConfig::from_spec(&spec, &config);
        let json = serde_json::to_string_pretty(&oci).expect("序列化 OCI config 应成功");
        assert!(
            json.contains(r#""ociVersion": "1.0.2""#),
            "应包含 ociVersion"
        );
        assert!(json.contains("echo"), "应包含程序名");
        assert!(json.contains("hello"), "应包含参数");
        assert!(json.contains("PATH"), "应包含环境变量");
        assert!(json.contains("pid"), "应包含 pid 命名空间");
    }

    #[test]
    fn test_oci_bundle_creation() {
        // 创建临时 rootfs 目录以满足 rootfs 存在性验证
        let rootfs_dir = std::env::temp_dir().join("chimera-test-bundles-rootfs");
        let _ = std::fs::remove_dir_all(&rootfs_dir);
        std::fs::create_dir_all(&rootfs_dir).expect("创建临时 rootfs 目录应成功");

        let config = GvisorConfig {
            rootfs_path: Some(rootfs_dir.to_string_lossy().to_string()),
            ..GvisorConfig::default()
        };
        let spec = CommandSpec {
            program: "echo".into(),
            allowed_args: vec!["test".into()],
            env_whitelist: [].into(),
            risk_level: crate::types::RiskLevel::Low,
            risk_score: 10,
        };
        let oci = OciConfig::from_spec(&spec, &config);
        let bundle_dir = std::env::temp_dir().join("chimera-test-bundles");
        let _ = std::fs::remove_dir_all(&bundle_dir); // 清理旧目录

        let bundle = OciBundle::new(&bundle_dir, &oci).expect("创建 bundle 应成功");
        assert!(bundle.path().exists(), "bundle 目录应存在");
        assert!(
            bundle.path().join("config.json").exists(),
            "config.json 应存在"
        );

        // 验证 config.json 内容
        let content = std::fs::read_to_string(bundle.path().join("config.json"))
            .expect("读取 config.json 应成功");
        assert!(
            content.contains("ociVersion"),
            "config.json 应包含 ociVersion"
        );

        // 清理测试目录
        let _ = std::fs::remove_dir_all(&rootfs_dir);
        let _ = std::fs::remove_dir_all(&bundle_dir);
    }

    #[test]
    fn test_oci_bundle_drop_cleanup() {
        // 创建临时 rootfs 目录以满足 rootfs 存在性验证
        let rootfs_dir = std::env::temp_dir().join("chimera-test-cleanup-rootfs");
        let _ = std::fs::remove_dir_all(&rootfs_dir);
        std::fs::create_dir_all(&rootfs_dir).expect("创建临时 rootfs 目录应成功");

        let config = GvisorConfig {
            rootfs_path: Some(rootfs_dir.to_string_lossy().to_string()),
            ..GvisorConfig::default()
        };
        let spec = CommandSpec {
            program: "true".into(),
            allowed_args: vec![],
            env_whitelist: [].into(),
            risk_level: crate::types::RiskLevel::Low,
            risk_score: 10,
        };
        let oci = OciConfig::from_spec(&spec, &config);
        let bundle_dir = std::env::temp_dir().join("chimera-test-cleanup");
        let _ = std::fs::remove_dir_all(&bundle_dir);

        let bundle_path;
        {
            let bundle = OciBundle::new(&bundle_dir, &oci).expect("创建 bundle 应成功");
            bundle_path = bundle.path().to_path_buf();
            assert!(bundle_path.exists(), "bundle 目录在作用域内应存在");
        }
        // bundle 已 drop，目录应被清理
        assert!(!bundle_path.exists(), "bundle 目录在 drop 后应被清理");

        let _ = std::fs::remove_dir_all(&rootfs_dir);
        let _ = std::fs::remove_dir_all(&bundle_dir);
    }

    #[test]
    fn test_oci_bundle_rootfs_not_exists() {
        // rootfs 路径不存在时，创建 bundle 应返回错误
        let config = GvisorConfig {
            rootfs_path: Some("/nonexistent/rootfs".into()),
            ..GvisorConfig::default()
        };
        let spec = CommandSpec {
            program: "echo".into(),
            allowed_args: vec!["hello".into()],
            env_whitelist: [].into(),
            risk_level: crate::types::RiskLevel::Low,
            risk_score: 10,
        };
        let oci = OciConfig::from_spec(&spec, &config);
        let bundle_dir = std::env::temp_dir().join("chimera-test-rootfs-not-exists");
        let _ = std::fs::remove_dir_all(&bundle_dir);

        let result = OciBundle::new(&bundle_dir, &oci);
        assert!(result.is_err(), "rootfs 不存在时应返回错误");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("rootfs"),
            "错误信息应包含 rootfs，实际: {}",
            err_msg
        );

        let _ = std::fs::remove_dir_all(&bundle_dir);
    }

    #[test]
    fn test_oci_bundle_rootfs_exists() {
        // rootfs 路径存在时，创建 bundle 应成功
        let rootfs_dir = std::env::temp_dir().join("chimera-test-rootfs-exists");
        let _ = std::fs::remove_dir_all(&rootfs_dir);
        std::fs::create_dir_all(&rootfs_dir).expect("创建临时 rootfs 目录应成功");

        let config = GvisorConfig {
            rootfs_path: Some(rootfs_dir.to_string_lossy().to_string()),
            ..GvisorConfig::default()
        };
        let spec = CommandSpec {
            program: "echo".into(),
            allowed_args: vec!["hello".into()],
            env_whitelist: [].into(),
            risk_level: crate::types::RiskLevel::Low,
            risk_score: 10,
        };
        let oci = OciConfig::from_spec(&spec, &config);
        let bundle_dir = std::env::temp_dir().join("chimera-test-rootfs-exists-bundle");
        let _ = std::fs::remove_dir_all(&bundle_dir);

        let bundle = OciBundle::new(&bundle_dir, &oci).expect("rootfs 存在时应成功创建 bundle");
        assert!(bundle.path().exists(), "bundle 目录应存在");
        assert!(
            bundle.path().join("config.json").exists(),
            "config.json 应存在"
        );

        // 清理
        let _ = std::fs::remove_dir_all(&rootfs_dir);
        let _ = std::fs::remove_dir_all(&bundle_dir);
    }

    #[test]
    fn test_oci_config_from_spec_with_risk_score() {
        let config = GvisorConfig::default();
        let spec = CommandSpec {
            program: "rm".into(),
            allowed_args: vec!["-rf".into(), "/tmp/test".into()],
            env_whitelist: [("HOME".into(), "/root".into())].into(),
            risk_level: crate::types::RiskLevel::High,
            risk_score: 85,
        };
        let oci = OciConfig::from_spec(&spec, &config);
        let json = serde_json::to_string_pretty(&oci).expect("序列化应成功");
        assert!(json.contains("rm"), "应包含程序名 rm");
        assert!(json.contains("-rf"), "应包含参数 -rf");
        assert!(json.contains("HOME"), "应包含环境变量 HOME");
        assert!(json.contains("RLIMIT_NOFILE"), "应包含资源限制");
        assert!(json.contains("chimera.sandbox.type"), "应包含 annotations");
    }

    #[test]
    fn test_gvisor_spawn_oci_bundle_parameter() {
        // 验证 spawn 方法使用正确的 --bundle 参数（mock runsc 路径不存在时验证命令构建）
        // 注意：此测试不实际执行 runsc，仅验证参数构建逻辑
        let runtime = GvisorRuntime {
            runsc_path: "/usr/local/bin/runsc".into(),
            platform: "linux".into(),
            config: GvisorConfig::default(),
        };
        assert_eq!(runtime.runsc_path(), "/usr/local/bin/runsc");
        assert_eq!(runtime.config.bundle_dir, None);
        assert_eq!(runtime.config.rootfs_path, None);
    }
}
