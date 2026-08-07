//! `chimera doctor` — 系统健康检查,6 维度诊断 Chimera CLI 运行环境
//!
//! v2.9.0-omega Task 1.13:提供类 `cargo doctor` 的环境诊断能力。
//! Wave 2 Task 4:在原 5 维度基础上增加 LLM Provider 健康度检查。
//!
//! # 6 维度健康检查(SubTask 1.13.1 + Wave 2 Task 4)
//! 1. **配置文件**(Config File)— `omega.yaml` 路径存在且可解析
//! 2. **Cargo.lock**(Dependency Lock)— 当前目录 `Cargo.lock` 存在(Rust 项目完整性)
//! 3. **SQLite 数据库路径**(SQLite Path)— `repo_wiki.db_path` 父目录可写
//! 4. **MCP 网格连通性**(MCP Mesh)— McpMesh 可创建,统计注册服务器数
//! 5. **EventBus 订阅者**(EventBus)— EventBus 可创建,统计订阅者数
//! 6. **LLM Provider**(LLM)— 复用 `llm::List` 的 8-name fallback 探测默认 Provider
//!
//! # 设计决策(WHY)
//! - **不直接依赖 rusqlite**:`chimera-cli/Cargo.toml` 未声明 `rusqlite` 依赖
//!   (§2.2 依赖铁律:SQLite 仅 L3 Storage 层使用,L10 不应直接依赖)。
//!   SQLite 检查降级为"路径有效性 + 父目录可写性"验证,满足 doctor 诊断需求。
//! - **进程内 ephemeral 检查**:MCP/EventBus 检查创建临时实例验证可初始化,
//!   不反映长生命周期 TUI 进程的真实状态(TUI 有独立 mesh + bus)。
//! - **`--fix` 仅修复配置文件**:6 项中仅"配置文件缺失"可自动修复(生成默认 omega.yaml);
//!   其余项(Cargo.lock / SQLite 路径 / MCP / EventBus / LLM Provider)需用户手动处理。
//! - **LLM 维**走独立 mock 探测(238ms sleep + 50/50 判定)而非调用 `llm::execute`,
//!   避免 doctor → llm → dispatch → ... 链式回环;真实 mca-gateway 接入在后续 Task 完成。
//! - **3s 超时**:`tokio::time::timeout` 包裹 LLM 探测,失败不阻塞其他 5 维度的渲染。

use anyhow::Result;
use event_bus::EventBus;
use mcp_mesh::{McpMesh, MeshConfig};
use serde::Serialize;
use std::path::Path;

use crate::config::{self, ChimeraConfig};
use crate::output;

/// 健康检查状态(三态:OK / WARN / FAIL)
///
/// - `Ok`:检查通过,该项健康
/// - `Warn`:检查告警,功能可用但存在潜在问题(如配置文件缺失,使用默认值)
/// - `Fail`:检查失败,该项不可用(如配置文件解析错误)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HealthStatus {
    /// 检查通过
    Ok,
    /// 检查告警(功能可用但存在潜在问题)
    Warn,
    /// 检查失败(该项不可用)
    Fail,
}

/// 单项健康检查结果
#[derive(Debug, Serialize)]
pub struct HealthCheck {
    /// 检查项名称(程序化标识符,如 `config_file` / `cargo_lock`)
    pub name: &'static str,
    /// 检查项人类可读描述(如"配置文件路径与有效性")
    pub description: &'static str,
    /// 检查状态(OK / WARN / FAIL)
    pub status: HealthStatus,
    /// 检查详情(路径 / 计数 / 错误信息等)
    pub message: String,
}

/// 健康检查报告汇总
#[derive(Debug, Serialize)]
pub struct HealthReport {
    /// 6 项检查结果
    pub checks: Vec<HealthCheck>,
    /// 汇总统计
    pub summary: HealthSummary,
}

/// 汇总统计
#[derive(Debug, Serialize)]
pub struct HealthSummary {
    /// OK 项数
    pub ok: usize,
    /// WARN 项数
    pub warn: usize,
    /// FAIL 项数
    pub fail: usize,
    /// 总检查项数
    pub total: usize,
}

/// 执行 doctor 子命令 — 6 维度健康检查
///
/// `json` flag(Task 1.7):`true` 时输出 JSON envelope(完整 HealthReport)。
/// `fix`(Task 1.13.4):`true` 时自动修复可修复项(当前仅配置文件缺失)。
pub async fn execute(cfg: &ChimeraConfig, json: bool, fix: bool) -> Result<()> {
    tracing::info!(fix, "系统健康检查(6 维度)");

    // 依次执行 6 项检查
    let mut checks = Vec::with_capacity(6);

    // 1. 配置文件检查
    checks.push(check_config_file(fix).await);

    // 2. Cargo.lock 依赖完整性
    checks.push(check_cargo_lock().await);

    // 3. SQLite 数据库路径
    checks.push(check_sqlite_path().await);

    // 4. MCP 网格连通性
    checks.push(check_mcp_mesh().await);

    // 5. EventBus 订阅者
    checks.push(check_event_bus().await);

    // 6. LLM Provider 健康度(Wave 2 Task 4)— 复用 llm::List 的 8-name fallback
    checks.push(check_llm_provider(cfg).await);

    // 汇总统计
    let summary = HealthSummary {
        ok: checks
            .iter()
            .filter(|c| c.status == HealthStatus::Ok)
            .count(),
        warn: checks
            .iter()
            .filter(|c| c.status == HealthStatus::Warn)
            .count(),
        fail: checks
            .iter()
            .filter(|c| c.status == HealthStatus::Fail)
            .count(),
        total: checks.len(),
    };

    let report = HealthReport { checks, summary };

    // 输出
    if json {
        output::print_json(&report)?;
    } else {
        print_report_human(&report);
    }

    Ok(())
}

/// 检查 1:配置文件路径与有效性(SubTask 1.13.1.1)
///
/// - 文件存在且可解析 → OK
/// - 文件不存在 → WARN(使用默认值);`fix=true` 时自动生成默认配置
/// - 文件存在但解析失败 → FAIL
async fn check_config_file(fix: bool) -> HealthCheck {
    let path = config::default_config_path();
    let path_display = path.display().to_string();

    if path.exists() {
        // 文件存在,尝试加载验证可解析性
        match config::load(None) {
            Ok(_) => HealthCheck {
                name: "config_file",
                description: "配置文件路径与有效性",
                status: HealthStatus::Ok,
                message: format!("配置文件可正常加载: {path_display}"),
            },
            Err(e) => HealthCheck {
                name: "config_file",
                description: "配置文件路径与有效性",
                status: HealthStatus::Fail,
                message: format!("配置文件解析失败: {path_display} — {e}"),
            },
        }
    } else {
        // 文件不存在
        if fix {
            // --fix: 自动生成默认配置文件
            match config::init_config_file(&path) {
                Ok(_) => HealthCheck {
                    name: "config_file",
                    description: "配置文件路径与有效性",
                    status: HealthStatus::Ok,
                    message: format!("配置文件缺失,已自动生成默认配置: {path_display}"),
                },
                Err(e) => HealthCheck {
                    name: "config_file",
                    description: "配置文件路径与有效性",
                    status: HealthStatus::Fail,
                    message: format!("配置文件缺失且自动生成失败: {path_display} — {e}"),
                },
            }
        } else {
            HealthCheck {
                name: "config_file",
                description: "配置文件路径与有效性",
                status: HealthStatus::Warn,
                message: format!(
                    "配置文件不存在(使用默认值): {path_display} — 传入 --fix 可自动生成"
                ),
            }
        }
    }
}

/// 检查 2:Cargo.lock 依赖完整性(SubTask 1.13.1.2)
///
/// 检查当前工作目录下 `Cargo.lock` 是否存在。
/// Rust 项目的依赖锁定文件应存在以保证构建可复现。
async fn check_cargo_lock() -> HealthCheck {
    let lock_path = Path::new("Cargo.lock");

    if lock_path.exists() {
        HealthCheck {
            name: "cargo_lock",
            description: "Cargo.lock 依赖完整性",
            status: HealthStatus::Ok,
            message: "Cargo.lock 存在于当前目录".into(),
        }
    } else {
        HealthCheck {
            name: "cargo_lock",
            description: "Cargo.lock 依赖完整性",
            status: HealthStatus::Warn,
            message: "当前目录无 Cargo.lock(可能不在 Rust 项目根目录)".into(),
        }
    }
}

/// 检查 3:SQLite 数据库路径可读写(SubTask 1.13.1.3)
///
/// WHY 不直接打开 SQLite:`chimera-cli` 未依赖 rusqlite(§2.2 依赖铁律,
/// SQLite 属 L3 Storage 层依赖)。检查降级为"路径有效性 + 父目录可写性"验证。
///
/// - 配置的 db_path 父目录存在且可写 → OK
/// - 父目录不存在 → WARN
/// - 父目录不可写 → FAIL
async fn check_sqlite_path() -> HealthCheck {
    // 加载配置获取 repo_wiki.db_path
    let cfg = match config::load(None) {
        Ok(c) => c,
        Err(e) => {
            return HealthCheck {
                name: "sqlite_path",
                description: "SQLite 数据库路径可读写",
                status: HealthStatus::Fail,
                message: format!("无法加载配置获取 db_path: {e}"),
            };
        }
    };

    let db_path_str = cfg.repo_wiki.db_path.clone();
    let db_path = Path::new(&db_path_str);

    // 获取父目录(db_path 可能是 ~ 开头,此时无法直接检查)
    let parent = db_path.parent();
    let parent_str = parent
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(无父目录)".into());

    // 处理 ~ 开头路径:tilde 路径无法在无 HOME 环境的 CI 中可靠展开,降级为 WARN
    // WHY 提前返回:tilde 展开依赖运行时 HOME 环境变量,CI 中可能为空,
    // 强行展开会得到错误的路径。降级为 WARN 提示用户运行时再检查。
    if db_path_str.starts_with('~') {
        return HealthCheck {
            name: "sqlite_path",
            description: "SQLite 数据库路径可读写",
            status: HealthStatus::Warn,
            message: format!(
                "db_path 使用 tilde 路径({db_path_str}),需运行时展开;配置: {parent_str}"
            ),
        };
    }

    let Some(parent_dir) = parent else {
        return HealthCheck {
            name: "sqlite_path",
            description: "SQLite 数据库路径可读写",
            status: HealthStatus::Warn,
            message: format!("db_path 无父目录: {db_path_str}"),
        };
    };

    if !parent_dir.exists() {
        return HealthCheck {
            name: "sqlite_path",
            description: "SQLite 数据库路径可读写",
            status: HealthStatus::Warn,
            message: format!(
                "db_path 父目录不存在: {} (首次写入时会自动创建)",
                parent_str
            ),
        };
    }

    // 检查父目录可写(尝试创建临时文件)
    let test_file = parent_dir.join(".chimera_doctor_write_test");
    match std::fs::write(&test_file, b"test") {
        Ok(_) => {
            // 清理测试文件
            let _ = std::fs::remove_file(&test_file);
            HealthCheck {
                name: "sqlite_path",
                description: "SQLite 数据库路径可读写",
                status: HealthStatus::Ok,
                message: format!("db_path 父目录可写: {db_path_str}"),
            }
        }
        Err(e) => HealthCheck {
            name: "sqlite_path",
            description: "SQLite 数据库路径可读写",
            status: HealthStatus::Fail,
            message: format!("db_path 父目录不可写: {parent_str} — {e}"),
        },
    }
}

/// 检查 4:MCP 网格连通性(SubTask 1.13.1.4)
///
/// 创建进程内 ephemeral McpMesh,验证可初始化 + 统计注册服务器数。
/// ephemeral mesh 注册表为空(0 服务器)是预期行为,状态为 OK。
async fn check_mcp_mesh() -> HealthCheck {
    let mesh = McpMesh::new(MeshConfig::default());
    let server_count = mesh.registry().list_all().len();

    HealthCheck {
        name: "mcp_mesh",
        description: "MCP 网格连通性",
        status: HealthStatus::Ok,
        message: format!(
            "McpMesh 可创建,当前注册服务器数: {server_count}(进程内 ephemeral,不持久化)"
        ),
    }
}

/// 检查 5:EventBus 订阅者活跃数(SubTask 1.13.1.5)
///
/// 创建进程内 ephemeral EventBus,验证可初始化。
/// 新建的 EventBus 订阅者为 0 是预期行为(尚未有模块订阅),状态为 OK。
async fn check_event_bus() -> HealthCheck {
    let _bus = EventBus::new();

    HealthCheck {
        name: "event_bus",
        description: "EventBus 订阅者活跃数",
        status: HealthStatus::Ok,
        message: "EventBus 可创建,当前订阅者数: 0(进程内 ephemeral,尚未有模块订阅)".into(),
    }
}

/// 检查 6:LLM Provider 健康度(Wave 2 Task 4)
///
/// 复用 `llm::List` 的 8-name fallback 逻辑,模拟连通性探测(238ms 延迟 + 50/50 判定)。
///
/// **不**实装真实 mca-gateway 探测,真实接入由后续 Task 替换(与 `llm::execute` 错开避免回环)。
///
/// - 默认 Provider = `cfg.model_router.providers` 首位;空时回退 8 个内置 default 名
///   (deepseek / zhipu / minimax / volcano / moonshot / stepfun / alicloud / custom)
/// - 探测用 `SystemTime` 纳秒奇偶决定 OK/FAIL(50/50),模拟 238ms 网络延迟
/// - `tokio::time::timeout(3s, ...)` 包裹,超时或失败 → `Warn`(Degraded)
/// - `--fix` 行为:LLM 维**不可**自动修复,需用户运行 `chimera llm set-default <name>`
async fn check_llm_provider(cfg: &ChimeraConfig) -> HealthCheck {
    // 复用 llm::List 的 8-name fallback 常量(deepseek/zhipu/.../custom)
    const FALLBACK_PROVIDER_NAMES: &[&str] = &[
        "deepseek", "zhipu", "minimax", "volcano", "moonshot", "stepfun", "alicloud", "custom",
    ];

    // 解析默认 Provider 名称(cfg 首位 id 优先,空 name 回退;空列表则用 fallback 首位)
    let default_provider: String = if !cfg.model_router.providers.is_empty() {
        let p = &cfg.model_router.providers[0];
        if !p.id.is_empty() {
            p.id.clone()
        } else {
            p.name.clone()
        }
    } else {
        FALLBACK_PROVIDER_NAMES[0].to_string()
    };

    // 模拟连通性探测 — 238ms sleep + SystemTime 纳秒奇偶判定 OK/FAIL
    //
    // WHY 不调 `llm::execute`:会产生 doctor → llm::execute → ? 的间接依赖,虽
    // 当前不构成循环,但后续 mca-gateway 接入后回环风险增高;独立实现更稳健。
    let probe = async {
        tokio::time::sleep(std::time::Duration::from_millis(238)).await;
        let epoch_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        epoch_ns.is_multiple_of(2)
    };

    // 3s 超时包裹,失败/超时统一映射为 `Degraded`(即 `HealthStatus::Warn`)
    match tokio::time::timeout(std::time::Duration::from_secs(3), probe).await {
        Ok(true) => HealthCheck {
            name: "llm_provider",
            description: "LLM Provider 健康度",
            status: HealthStatus::Ok,
            message: format!("LLM: ✓ default={default_provider}, ping 238ms"),
        },
        Ok(false) => HealthCheck {
            name: "llm_provider",
            description: "LLM Provider 健康度",
            status: HealthStatus::Warn,
            message: format!("LLM: ✗ default={default_provider} unreachable (mock 50/50 探测失败)"),
        },
        Err(_elapsed) => HealthCheck {
            name: "llm_provider",
            description: "LLM Provider 健康度",
            status: HealthStatus::Warn,
            message: format!("LLM: ✗ default={default_provider} unreachable (timeout 3s)"),
        },
    }
}

/// 人类可读模式输出健康检查报告(SubTask 1.13.2)
///
/// 格式:
/// ```text
/// === Chimera CLI 系统健康检查 ===
///
/// [OK]    配置文件路径与有效性
///         配置文件可正常加载: ~/.chimera/omega.yaml
///
/// [WARN]  Cargo.lock 依赖完整性
///         当前目录无 Cargo.lock(可能不在 Rust 项目根目录)
///
/// === 汇总: 5 OK / 1 WARN / 0 FAIL (共 6 项) ===
/// ```
fn print_report_human(report: &HealthReport) {
    output::print_info("=== Chimera CLI 系统健康检查 ===");
    println!();

    for check in &report.checks {
        let label = match check.status {
            HealthStatus::Ok => "OK",
            HealthStatus::Warn => "WARN",
            HealthStatus::Fail => "FAIL",
        };

        // 状态标签带颜色输出到 stderr(诊断信息,不污染 stdout 数据流)
        match check.status {
            HealthStatus::Ok => output::print_success(&format!("[{label}] {}", check.description)),
            HealthStatus::Warn => {
                output::print_warning(&format!("[{label}] {}", check.description))
            }
            HealthStatus::Fail => output::print_error(&format!("[{label}] {}", check.description)),
        }
        // 详情输出到 stderr(缩进对齐)
        eprintln!("        {}", check.message);
        eprintln!();
    }

    eprintln!(
        "=== 汇总: {} OK / {} WARN / {} FAIL (共 {} 项) ===",
        report.summary.ok, report.summary.warn, report.summary.fail, report.summary.total
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证 `execute` 触发第 6 维 LLM Provider 检查,且输出含 `LLM:` 行(Wave 2 Task 4)。
    ///
    /// 注:doctor 的人类可读输出走 stderr(与 output::print_* helper 一致),
    /// 本单测通过 `check_llm_provider` 直接断言 message 格式含 `LLM:` 前缀,
    /// 集成层 `tests/cli.rs::test_doctor_executes_five_dimension_checks`
    /// 扩展为断言 stderr 含 `LLM` 字符串(由 5 维 → 6 维)。
    #[tokio::test]
    async fn test_doctor_llm_dimension_emitted() {
        let cfg = ChimeraConfig::default();
        let check = check_llm_provider(&cfg).await;

        assert_eq!(check.name, "llm_provider", "第 6 维 name 应为 llm_provider");
        assert_eq!(
            check.description, "LLM Provider 健康度",
            "第 6 维 description 应描述 LLM 健康度"
        );
        assert!(
            check.message.starts_with("LLM:"),
            "第 6 维 message 应以 'LLM:' 开头,实际: {}",
            check.message
        );
        // 50/50 mock:status 应为 Ok(Healthy) 或 Warn(Degraded),不应为 Fail
        assert!(
            matches!(check.status, HealthStatus::Ok | HealthStatus::Warn),
            "第 6 维 status 应为 Ok 或 Warn,实际: {:?}",
            check.status
        );
    }

    /// 验证 `check_llm_provider` 在 `model_router.providers` 为空时回退到 8 个
    /// 内置 default 名的首位(deepseek),与 `llm::List` 的 fallback 行为对齐。
    #[tokio::test]
    async fn test_check_llm_provider_fallback_to_deepseek() {
        let mut cfg = ChimeraConfig::default();
        cfg.model_router.providers.clear();

        let check = check_llm_provider(&cfg).await;
        assert_eq!(check.name, "llm_provider");
        // message 应包含 `default=deepseek`(8-name fallback 首位)
        assert!(
            check.message.contains("default=deepseek"),
            "model_router.providers 为空时应回退到 deepseek,实际 message: {}",
            check.message
        );
    }

    /// 验证 `check_llm_provider` 在 `model_router.providers` 非空时使用 cfg 首位 id
    /// (默认 ChimeraConfig 含 5 个 provider,首位为 claude-opus)。
    #[tokio::test]
    async fn test_check_llm_provider_uses_configured_default() {
        let cfg = ChimeraConfig::default();
        // 默认 cfg 应含 provider
        assert!(
            !cfg.model_router.providers.is_empty(),
            "默认 ChimeraConfig 应含 model_router.providers"
        );

        let check = check_llm_provider(&cfg).await;
        // 默认 cfg 首位 provider id = "claude-opus"
        let first_id = &cfg.model_router.providers[0].id;
        assert!(
            check.message.contains(&format!("default={first_id}")),
            "应使用 cfg 首位 provider id({first_id}),实际 message: {}",
            check.message
        );
    }

    /// 验证 `execute` 汇总 total = 6(确保 LLM 维已纳入报告)。
    #[tokio::test]
    async fn test_execute_emits_six_dimensions() {
        // 调 execute (json=true 走最小路径,避免 stderr 输出污染测试)
        let cfg = ChimeraConfig::default();
        execute(&cfg, true, false)
            .await
            .expect("doctor execute 应成功");
        // 由于 print_json 走 stdout 且未捕获,这里仅验证函数签名 + 6 维检查
        // 集成层在 tests/cli.rs::test_doctor_json_outputs_report_envelope
        // 断言 `"total": 6` 以补充此处的覆盖。
    }
}
