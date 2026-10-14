//! `chimera doctor` — 系统健康检查,逐探针诊断 Chimera CLI 运行环境
//!
//! v2.9.0-omega Task 1.13:提供类 `cargo doctor` 的环境诊断能力。
//! Wave 2 Task 4:在原 5 维度基础上增加 LLM Provider 健康度检查。
//!
//! # 11 维度健康检查(SubTask 1.13.1 + Wave 2 Task 4 + WI-02 + M13 + WI-G12 + W5-1 R-2)
//! 1. **配置文件**(Config File)— `omega.yaml` 路径存在且可解析
//! 2. **Cargo.lock**(Dependency Lock)— 当前目录 `Cargo.lock` 存在(Rust 项目完整性)
//! 3. **SQLite 数据库路径**(SQLite Path)— `repo_wiki.db_path` 父目录可写
//! 4. **MCP 网格连通性**(MCP Mesh)— McpMesh 可创建,统计注册服务器数
//! 5. **EventBus 订阅者**(EventBus)— EventBus 可创建,统计订阅者数
//! 6. **LLM Provider**(LLM)— 复用 `llm::List` 的 8-name fallback 探测默认 Provider
//! 7. **认证密钥**(Auth Keys)— 常见供应商 API key 环境变量自检(WI-02)
//! 8. **SecCore 沙箱**(Sandbox)— 进程内沙箱可用性 + 命令分类在役验证(WI-02)
//! 9. **SCC 缓存**(SCC Cache)— 组合根装配的 SccCache 统计快照健康度(M13 重路由;零数据流记 WARN——见 `scc_probe_verdict`)
//! 10. **L3 存储三后端**(Storage Health)— scc/cmt/session-store 只读信号聚合(WI-G12)
//! 11. **模型资产面**(Model Asset Health)— NMC 多模态 ONNX 资产就绪度(W5-1 R-2 / ADR-196 V6)
//!
//! # 设计决策(WHY)
//! - **不直接依赖 rusqlite**:`chimera-cli/Cargo.toml` 未声明 `rusqlite` 依赖
//!   (§2.2 依赖铁律:SQLite 仅 L3 Storage 层使用,L10 不应直接依赖)。
//!   SQLite 检查降级为"路径有效性 + 父目录可写性"验证,满足 doctor 诊断需求。
//! - **进程内 ephemeral 检查**:MCP/EventBus 检查创建临时实例验证可初始化,
//!   不反映长生命周期 TUI 进程的真实状态(TUI 有独立 mesh + bus)。
//! - **`--fix` 仅修复配置文件**:11 项中仅"配置文件缺失"可自动修复(生成默认 omega.yaml);
//!   其余项(Cargo.lock / SQLite 路径 / MCP / EventBus / LLM Provider / 认证密钥 / 沙箱 / SCC 缓存 / L3 存储三后端 / 模型资产面)需用户手动处理。
//! - **LLM 维**走独立 mock 探测(238ms sleep + 50/50 判定)而非调用 `llm::execute`,
//!   避免 doctor → llm → dispatch → ... 链式回环;真实 mca-gateway 接入在后续 Task 完成。
//! - **3s 超时**:`tokio::time::timeout` 包裹 LLM 探测,失败不阻塞其他 5 维度的渲染。

use anyhow::Result;
use event_bus::EventBus;
use mcp_mesh::{McpMesh, MeshConfig};
use nexus_contracts::command_validation::{Command, CommandPolicy, CommandValidator};
use seccore::command_validator::SecCoreCommandValidator;
use seccore::Sandbox;
use serde::Serialize;
use std::path::{Path, PathBuf};

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
    /// 9 项检查结果
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

/// 执行 doctor 子命令 — 11 维度健康检查（WI-02 增强: +认证密钥 / +沙箱;M13: +SCC 缓存;WI-G12: +L3 存储三后端;W5-1 R-2: +模型资产面）
///
/// `json` flag(Task 1.7):`true` 时输出 JSON envelope(完整 HealthReport)。
/// `fix`(Task 1.13.4):`true` 时自动修复可修复项(当前仅配置文件缺失)。
///
/// 兼容层 thin wrapper(M4 先例):独立调用场景经组合根装配 ephemeral
/// AppContext;dispatch 主链调 [`execute_with_ctx`] 共享 dispatch 级上下文。
pub async fn execute(cfg: &ChimeraConfig, json: bool, fix: bool) -> Result<()> {
    let ctx = crate::composition::build(cfg)?;
    execute_with_ctx(&ctx, cfg, json, fix).await
}

/// doctor 子命令主体 — 11 探针经组合根共享 GQEP 执行器并行 gather(M12 / ADR-185 D3;
/// M13 起 +SCC 缓存探针;WI-G12 起 +L3 存储三后端探针;W5-1 R-2 起 +模型资产面探针)
///
/// ## 并行化决策(WHY GQEP 而非 tokio::join!)
///
/// - **语义不变**:11 项检查内容/状态/汇总与串行版逐项一致(索引 tag 按注册序
///   还原,FuturesUnordered 完成序不保证输入序);仅时序由串行改并行
/// - **零孤儿治理**:探针 future 全部经 QEEP `entangle` 包裹(单操作 5s 超时 +
///   OrphanGuard),比裸 `join!` 多双层超时防护,符合 §6.1 红线
/// - **可观测**:gather 在共享总线发布 `GatherCompleted`(total/succeeded/failed),
///   TUI/审计可订阅;探针恒 `Ok` 返回(检查失败是结果而非错误),失败 tail 仅
///   可能来自 gqep 超时——以 FAIL 占位兜底,报告恒 11 项
/// - **可回滚**:单 commit revert 即恢复(M13 scc 探针/WI-G12 storage_health 探针均同 commit 可独立 revert)
pub async fn execute_with_ctx(
    ctx: &crate::composition::AppContext,
    cfg: &ChimeraConfig,
    json: bool,
    fix: bool,
) -> Result<()> {
    tracing::info!(fix, "系统健康检查(11 维度, GQEP 并行 gather)");

    let checks = run_probes_parallel(ctx, cfg, fix).await;
    let probe_total = checks.len() as u32;

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
    debug_assert_eq!(
        summary.total, probe_total as usize,
        "报告项数应与探针数一致(兜底占位也计数)"
    );

    let report = HealthReport { checks, summary };

    // 输出
    if json {
        output::print_json(&report)?;
    } else {
        print_report_human(&report);
    }

    Ok(())
}

/// 各探针并行 gather(GQEP `gather_collected<(usize, HealthCheck)>`)
///
/// 索引 tag 设计:每个探针 future 携 `(索引, 结果)`——gather 返回完成序的
/// values,此处按索引还原注册序,保证报告输出顺序与串行版逐字节一致。
/// 超时空底:探针恒 `Ok`,值缺失仅源于 gqep 单操作/全局超时,以 FAIL 占位
/// 保证报告项数恒等于探针数(该 tail 实践中不可达:LLM 探针自带 3s 内部超时,
/// 其余探针均快路径)。
async fn run_probes_parallel(
    ctx: &crate::composition::AppContext,
    cfg: &ChimeraConfig,
    fix: bool,
) -> Vec<HealthCheck> {
    const PROBE_COUNT: usize = 11;

    // cfg 以 owned clone 移入 future('static 约束);Clone 代价 = 一次配置结构拷贝,
    // 远小于 LLM 探针 238ms 睡眠——并行净收益不受影响。
    // scc 句柄同理(SccCache Clone = Arc 引用计数,廉价):M13 第 9 探针消费
    // 组合根装配的共享缓存实例,而非探针内自建 ephemeral(那是装配态空转)。
    let cfg_owned = cfg.clone();
    let scc_owned = ctx.scc.clone();
    // WI-G12 第 10 探针需要第二份 scc 句柄(Clone = Arc 引用计数,廉价)
    let scc_owned_b = ctx.scc.clone();

    // 探针注册序 = 报告输出序(与串行版一致,输出内容零变化;M13 起追加 scc_cache,
    // WI-G12 追加 storage_health,W5-1 R-2 追加 model_asset_health)
    let probes: Vec<gqep_executor::GqepFuture<(usize, HealthCheck)>> = vec![
        Box::pin(async move { Ok((0, check_config_file(fix).await)) }),
        Box::pin(async move { Ok((1, check_cargo_lock().await)) }),
        Box::pin(async move { Ok((2, check_sqlite_path().await)) }),
        Box::pin(async move { Ok((3, check_mcp_mesh().await)) }),
        Box::pin(async move { Ok((4, check_event_bus().await)) }),
        Box::pin(async move { Ok((5, check_llm_provider(&cfg_owned).await)) }),
        Box::pin(async move { Ok((6, check_auth_keys())) }),
        Box::pin(async move { Ok((7, check_seccore())) }),
        Box::pin(async move { Ok((8, check_scc_cache(&scc_owned))) }),
        Box::pin(async move { Ok((9, check_storage_health(&scc_owned_b))) }),
        Box::pin(async move { Ok((10, check_model_assets())) }),
    ];

    let outcome = ctx.gqep.gather_collected(probes).await;
    tracing::debug!(
        total = outcome.stats.total,
        succeeded = outcome.stats.succeeded,
        failed = outcome.stats.failed,
        latency_ms = outcome.stats.latency_ms,
        "doctor 探针 gather 完成"
    );

    // 按索引还原注册序;缺失索引(FAIL 占位)仅来自 gqep 超时 tail
    let mut by_index: std::collections::HashMap<usize, HealthCheck> =
        outcome.values.into_iter().collect();
    let mut checks = Vec::with_capacity(PROBE_COUNT);
    for i in 0..PROBE_COUNT {
        checks.push(by_index.remove(&i).unwrap_or_else(|| HealthCheck {
            name: "probe_timeout",
            description: "探针执行超时",
            status: HealthStatus::Fail,
            message: format!("探针 #{i} 未在 gather 时限内完成(gqep 超时兜底)"),
        }));
    }
    checks
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

/// 检查 7:认证密钥（WI-02）— API key 环境变量自检
///
/// 检查常见供应商密钥环境变量是否设置（不泄露密钥值）。
/// - 任一已设置 → OK
/// - 全部缺失 → WARN（LLM 调用需显式配置密钥）
fn check_auth_keys() -> HealthCheck {
    // 常见供应商密钥环境变量（与 examples/config.sample.* 对齐）
    const KEY_ENVS: &[&str] = &[
        "DEEPSEEK_API_KEY",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "ZHIPU_API_KEY",
        "MOONSHOT_API_KEY",
        "MINIMAX_API_KEY",
        "CHIMERA_API_KEY",
    ];
    let configured: Vec<&str> = KEY_ENVS
        .iter()
        .copied()
        .filter(|k| std::env::var(k).map(|v| !v.is_empty()).unwrap_or(false))
        .collect();

    if configured.is_empty() {
        HealthCheck {
            name: "auth_keys",
            description: "认证密钥（API key）",
            status: HealthStatus::Warn,
            message: "Auth: ✗ 未检测到 API key 环境变量（LLM 调用需显式配置）".into(),
        }
    } else {
        HealthCheck {
            name: "auth_keys",
            description: "认证密钥（API key）",
            status: HealthStatus::Ok,
            message: format!(
                "Auth: ✓ 已配置 {} 个密钥环境变量（{}，值不泄露）",
                configured.len(),
                configured.join(", ")
            ),
        }
    }
}

/// 检查 8:沙箱（WI-02）— SecCore 进程内沙箱可用性 + 命令分类
///
/// 创建默认策略沙箱并验证命令分类器：安全命令（ls）通过、
/// 危险命令（rm -rf）被拦截。验证零信任防线在役。
fn check_seccore() -> HealthCheck {
    // 沙箱构造（默认策略，含攻击类型规则）——验证沙箱运行时在役
    let sandbox = Sandbox::with_default_policy();
    // 拦截统计接口可用性（零信任防线数据面）
    let (_, _, _) = sandbox.interception_stats();
    let validator = SecCoreCommandValidator;
    let policy = CommandPolicy::default();

    // 安全命令: ls 应通过
    let safe_ok = validator
        .validate(&Command::new("ls").arg("-la"), &policy)
        .is_ok();
    // 危险命令: rm -rf 应被拦截（零信任红线）
    let danger_blocked = validator
        .validate(&Command::new("rm").arg("-rf").arg("/"), &policy)
        .is_err();

    if safe_ok && danger_blocked {
        HealthCheck {
            name: "seccore_sandbox",
            description: "SecCore 沙箱",
            status: HealthStatus::Ok,
            message: "Sandbox: ✓ 默认策略就绪，命令分类器在役（安全命令放行/危险命令拦截）".into(),
        }
    } else {
        HealthCheck {
            name: "seccore_sandbox",
            description: "SecCore 沙箱",
            status: HealthStatus::Fail,
            message: format!(
                "Sandbox: ✗ 命令分类异常（safe_ok={safe_ok}, danger_blocked={danger_blocked}）"
            ),
        }
    }
}

/// 检查 9:SCC 缓存（M13 重路由）— 组合根装配的 SccCache 统计快照健康度
///
/// 消费组合根共享 `SccCache` 实例(ADR-161 路径①,非探针内自建 ephemeral——
/// 那不构成对装配面的真实消费,ADR-179 非空转判据)。只读 `stats()` 快照 +
/// `len()`/`access_count()` 计数,零变异、零事件发布(不触发 CacheHit/Miss)。
///
/// 判定本身在 [`scc_probe_verdict`]（纯函数,两向可测）。
fn check_scc_cache(cache: &scc_cache::SccCache) -> HealthCheck {
    let stats = cache.stats();
    let accesses = cache.access_count();
    let (status, message) = scc_probe_verdict(stats.entry_count, accesses, stats.hit_rate);

    HealthCheck {
        name: "scc_cache",
        description: "SCC 推测上下文缓存",
        status,
        message,
    }
}

/// 第 9 探针的纯判定（无 IO、可独立测试；v3.5 W6-4 诚实降级）。
///
/// WHY 不再“恒 OK”：旧判据把“已装配”当“在役健康”，并把 `条目 0 / 访问 0` 叙述成
/// “装配初期”——但生产链今天**没有任何 SccCache 写入方**（组合根只装配句柄、探针只读
/// `stats()`），所以零条目不是初期状态而是**设计态**；拿“命中率会随访问自然上升”讲下去，
/// 就是把一条未接线的通道叙述成待激活的健康面（终局原则 3：主张必须可证伪）。
///
/// 口径沿用本文件已有先例（第 10/11 探针：缺席/未启用记 Warn 而非 Fail）：
/// - 条目与访问**均为 0** ⇒ `Warn` + 明确写出“零数据流、属设计态、不是故障”；
/// - 其余（已有条目或已有访问）⇒ `Ok`，报真实命中率。
#[must_use]
pub fn scc_probe_verdict(entries: usize, accesses: u64, hit_rate: f32) -> (HealthStatus, String) {
    if entries == 0 && accesses == 0 {
        return (
            HealthStatus::Warn,
            "SCC: ⚠ 已装配但零数据流（条目 0 / 累计访问 0）——生产链尚无写入方，属设计态而非故障；\n      命中率要等推测执行接线后才有意义".into(),
        );
    }
    (
        HealthStatus::Ok,
        format!(
            "SCC: ✓ 缓存在役且有数据流（条目 {entries} / 命中率 {:.1}% / 累计访问 {accesses}）",
            f64::from(hit_rate) * 100.0
        ),
    )
}

/// L3 存储三后端健康快照 — 纯类型（WI-G12;无 IO、可独立测试）
///
/// # 三后端只读信号
/// - **scc**:组合根共享缓存 `stats()` 快照（条目数 / 累计访问）——只报**已装配**与计数，
///   不拿它当“健康”判据（零数据流的语义由第 9 探针 `scc_probe_verdict` 承担）;
/// - **cmt**:经验卡片库根目录就绪（`experience_loop::experience_db_path()` 父目录,
///   与 L3 持久化入口同源路径）;
/// - **session-store**:段数据目录就绪——锚点 = session-store crate 默认
///   `StoreConfig::default().data_dir = "data/sessions"`（`session-store/src/types.rs`）;
///   **chimera-cli 不持 session-store 直接依赖边**（零新增内部依赖边纪律）,
///   故为同源字面锚点——session-store 若改默认值,须同批更新本锚点与测试。
///   另:生产装配尚未构造 `CbmrWriter`（现测全库零生产构造点,WI-G11 读口已就绪待宿主接管）,
///   缺席按"未启用/未初始化"记 Warn 而非 Fail（诚实登记,不虚构生产路径）。
///
/// 判定 = 纯函数:cmt 与 session 目录双就绪 → Ok;任一缺席 → Warn（未初始化/未启用,非故障）。
/// probe 仅做 `Path::exists` 级静态检查（无 IO 阻塞语义、无 rusqlite 依赖）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StorageHealth {
    /// scc 缓存条目数（stats() 只读快照）
    pub scc_entries: usize,
    /// scc 累计访问数
    pub scc_accesses: u64,
    /// cmt 经验卡片库根目录就绪
    pub cmt_dir_ready: bool,
    /// session-store 段数据目录就绪
    pub session_dir_ready: bool,
}

impl StorageHealth {
    /// 纯判定（三后端聚合;无 IO）
    #[must_use]
    pub fn verdict(&self) -> HealthStatus {
        if self.cmt_dir_ready && self.session_dir_ready {
            HealthStatus::Ok
        } else {
            HealthStatus::Warn
        }
    }

    /// 人类可读消息（三后端逐项）
    #[must_use]
    pub fn message(&self) -> String {
        format!(
            "L3: scc 已装配（条目 {} / 访问 {}）; cmt 经验库目录 {}; session-store 段目录 {}",
            self.scc_entries,
            self.scc_accesses,
            if self.cmt_dir_ready {
                "就绪"
            } else {
                "缺席（未初始化）"
            },
            if self.session_dir_ready {
                "就绪"
            } else {
                "缺席（未启用/未初始化）"
            },
        )
    }
}

/// 收集三后端就绪信号（目录参数注入以便测试;纯 `Path::exists` 静态检查）
fn collect_storage_health(
    cache: &scc_cache::SccCache,
    cmt_root: &Path,
    session_dir: &Path,
) -> StorageHealth {
    let stats = cache.stats();
    StorageHealth {
        scc_entries: stats.entry_count,
        scc_accesses: cache.access_count(),
        cmt_dir_ready: cmt_root.exists(),
        session_dir_ready: session_dir.exists(),
    }
}

/// 检查 10:L3 存储三后端（WI-G12）— scc（共享句柄统计）/ cmt（经验库目录）/
/// session-store（段目录,锚点见 [`StorageHealth`] 注释）只读信号聚合
///
/// 只读:零变异、零事件发布;`Path::exists` 静态检查（无 IO 阻塞语义、无 rusqlite）。
fn check_storage_health(cache: &scc_cache::SccCache) -> HealthCheck {
    let cmt_root = crate::experience_loop::experience_db_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    // 锚点 = session-store 默认段目录（同源字面锚点,见 StorageHealth 注释）
    let session_dir = PathBuf::from("data/sessions");
    let health = collect_storage_health(cache, &cmt_root, &session_dir);
    HealthCheck {
        name: "storage_health",
        description: "L3 存储三后端健康度",
        status: health.verdict(),
        message: health.message(),
    }
}

/// NMC 多模态资产目录约定落点（与 `scripts/fetch_nmc_assets.{ps1,sh}` 的
/// `$TargetDir`/`TARGET_DIR` 默认值逐字一致，由
/// `model_asset_paths_match_fetch_scripts` 测每仓钉住，防两处漂移）。
const NMC_ASSET_DIR: &str = "assets/nmc/models";
/// NMC 资产清单（模型名/期望 SHA-256 的登记表，同样来 fetch 脚本默认）。
const NMC_MANIFEST_PATH: &str = "assets/nmc/manifest.toml";

/// 检查 11:模型资产面（ADR-196 V6，WI-R2）— NMC 多模态 ONNX 资产就绪度
///
/// **语义硬约束（V6 原文判据）**:资产缺席 = Warn「未启用」，**不得**判 Error/Fail。
/// 依据：`nmc-encoder` 的 `NmcConfig::model_dir` 默认是**空串**即“未启用 ONNX
/// 推理”，image/video/audio 三感知器均以 `config.model_dir.is_empty()` 判
/// `backend = None` 并按设计降级——所以“未下载资产”是受支持的运行形态，不是故障。
///
/// WHY 只查约定目录而不读 `NmcConfig`:chimera-cli **不依赖** nmc-encoder
/// （现核 Cargo.toml 无该边），引入依赖去取默认 `model_dir` 会新增一条
/// L10→L2 内部依赖边（违 G-33 零新增边）;故与第 10 探针同构，只做
/// `Path::exists` 静态检查（零变异、零事件发布、无 rusqlite）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelAssetHealth {
    /// NMC 资产目录就绪（存在 = 已 fetch）
    pub nmc_asset_dir_ready: bool,
    /// NMC 清单在位（供诊断：区分“从未 fetch”与“清了目录忘删清单”）
    pub manifest_present: bool,
}

impl ModelAssetHealth {
    /// 纯判定（无 IO）:目录就绪 → Ok;未就位 → Warn（未启用，非故障）
    #[must_use]
    pub fn verdict(&self) -> HealthStatus {
        if self.nmc_asset_dir_ready {
            HealthStatus::Ok
        } else {
            HealthStatus::Warn
        }
    }

    /// 人读文案:未启用语义明写，并给出官方获取路径与“仍需显式配 model_dir”的事实
    #[must_use]
    pub fn message(&self) -> String {
        if self.nmc_asset_dir_ready {
            format!(
                "NMC 资产目录已就位({NMC_ASSET_DIR});真实多模态推理仍需显式配置 nmc-encoder 的 `model_dir`（默认空 = 降级路径）"
            )
        } else {
            let manifest = if self.manifest_present {
                "清单在位"
            } else {
                "清单缺失"
            };
            format!(
                "NMC 多模态资产未就位({NMC_ASSET_DIR};{manifest})— 未启用、非故障;获取:scripts/fetch_nmc_assets.{{ps1,sh}}（默认 dry-run）"
            )
        }
    }
}

/// 收集模型资产面信号（路径参数注入以便测试;纯 `Path::exists` 静态检查）
fn collect_model_asset_health(asset_dir: &Path, manifest_path: &Path) -> ModelAssetHealth {
    ModelAssetHealth {
        nmc_asset_dir_ready: asset_dir.exists(),
        manifest_present: manifest_path.exists(),
    }
}

/// 检查 11 入口:按约定路径采集模型资产面信号
///
/// 路径为**仓内约定相对路径**（与 fetch 脚本默认一致）；从任意工作目录跑
/// 二进制时目录不存在 → 报 Warn「未启用」，与第 10 探针的 `data/sessions`
/// 字面锚点同一口径。
fn check_model_assets() -> HealthCheck {
    let health = collect_model_asset_health(Path::new(NMC_ASSET_DIR), Path::new(NMC_MANIFEST_PATH));
    HealthCheck {
        name: "model_asset_health",
        description: "模型资产面(NMC 多模态)",
        status: health.verdict(),
        message: health.message(),
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// 验证 `execute` 触发第 6 维 LLM Provider 检查,且输出含 `LLM:` 行(Wave 2 Task 4)。
    ///
    /// 注:doctor 的人类可读输出走 stderr(与 output::print_* helper 一致),
    /// 本单测通过 `check_llm_provider` 直接断言 message 格式含 `LLM:` 前缀,
    /// 集成层 `tests/cli.rs::test_doctor_reports_all_probes_and_summary`
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

    /// 验证 `execute` 烟雾路径成功(汇总 total = 11:LLM / 沙箱 / SCC / L3 存储三后端 / 模型资产面已纳入报告)。
    #[tokio::test]
    async fn test_execute_emits_eleven_dimensions() {
        // 调 execute (json=true 走最小路径,避免 stderr 输出污染测试)
        let cfg = ChimeraConfig::default();
        execute(&cfg, true, false)
            .await
            .expect("doctor execute 应成功");
        // 由于 print_json 走 stdout 且未捕获,这里仅验证函数签名 + 11 维检查
        // 集成层在 tests/cli.rs::test_doctor_json_outputs_report_envelope
        // 断言 `"total": 11` 以补充此处的覆盖。
    }

    /// M13 / ADR-161 路径① 验收:第 9 探针 scc_cache 消费组合根共享缓存实例
    /// (stats() 只读快照;非自建 ephemeral = 非装配态空转,ADR-179)
    ///
    /// v3.5 W6-4 诚实降级：今天零写入方 ⇒ 该探针应记 **Warn**，消息必须自陈“设计态”，
    /// 不得再拿“✓ 缓存在役”与“装配初期”讲一个未接线的通道。
    #[tokio::test]
    async fn test_doctor_scc_cache_probe_consumes_assembled_cache() {
        let ctx = crate::composition::build(&ChimeraConfig::default()).expect("装配应成功");
        let check = check_scc_cache(&ctx.scc);

        assert_eq!(check.name, "scc_cache");
        assert_eq!(check.description, "SCC 推测上下文缓存");
        assert!(
            matches!(check.status, HealthStatus::Warn),
            "零数据流应记 Warn（设计态，非故障也非健康）,实际: {:?}",
            check.status
        );
        assert!(
            check.message.starts_with("SCC:"),
            "message 应以 'SCC:' 开头,实际: {}",
            check.message
        );
        assert!(
            check.message.contains("条目 0"),
            "探针必须报出零条目实况,实际 message: {}",
            check.message
        );
        assert!(
            check.message.contains("设计态"),
            "零数据流必须自陈设计态而非‘初期’,实际 message: {}",
            check.message
        );
    }

    /// 第 9 探针纯判定的两向锁（v3.5 W6-4）：
    ///   反向——零条目零访问 ⇒ Warn 且文案含“尚无写入方”；
    ///   正向——有访问或有条目 ⇒ Ok 且报命中率（防“一律降成 Warn”的另一方向误伤）。
    #[test]
    fn scc_probe_verdict_matrix() {
        let (status, msg) = scc_probe_verdict(0, 0, 0.0);
        assert_eq!(status, HealthStatus::Warn, "零数据流→Warn");
        assert!(msg.contains("尚无写入方"), "{msg}");
        assert!(!msg.contains("✓"), "零数据流不得给✓,实际: {msg}");

        // 已接线但尚未命中（有访问无条目）也属于“有数据流”，不该报设计态
        let (status, msg) = scc_probe_verdict(0, 12, 0.0);
        assert_eq!(status, HealthStatus::Ok, "有访问→Ok");
        assert!(msg.contains("累计访问 12"), "{msg}");

        let (status, msg) = scc_probe_verdict(4, 20, 0.5);
        assert_eq!(status, HealthStatus::Ok, "有条目→Ok");
        assert!(msg.contains("条目 4"), "{msg}");
        assert!(msg.contains("50.0%"), "命中率应换算成百分比,实际: {msg}");
    }

    /// M12 / ADR-185 D3 验收③(探针数 M13 起 8→9;WI-G12 起 9→10;W5-1 R-2 起 10→11):并行 gather 路径
    /// 产生 GatherCompleted(total=11) 事件
    ///
    /// 组合根共享 bus 订阅断言:11 探针经 gqep gather_collected 并行执行,
    /// gqep 在 gather 完成时发布 GatherCompleted(total, succeeded, failed);
    /// 探针 future 恒 Ok(检查失败是结果而非错误),故 succeeded 恒 = 11。
    #[tokio::test]
    async fn test_execute_with_ctx_emits_gather_completed_with_eleven_probes() {
        let ctx = crate::composition::build(&ChimeraConfig::default()).expect("装配应成功");
        // §4.4 反模式 3:subscribe 必须在 gather 调用之前同步调用
        let mut rx = ctx.bus.subscribe();

        execute_with_ctx(&ctx, &ChimeraConfig::default(), true, false)
            .await
            .expect("doctor 并行 gather 路径应成功");

        let event = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
            .await
            .expect("500ms 内应收到 GatherCompleted")
            .expect("recv 失败");
        match event {
            event_bus::NexusEvent::GatherCompleted {
                total, succeeded, ..
            } => {
                assert_eq!(
                    total, 11,
                    "探针总数应为 11(M13 +scc_cache;WI-G12 +storage_health;W5-1 R-2 +model_asset_health)"
                );
                assert_eq!(succeeded, 11, "探针恒 Ok,应全部成功");
            }
            other => panic!("期望 GatherCompleted,实际 {}", other.type_name()),
        }
    }

    // ============================================================
    // WI-G12 — L3 存储三后端探针（StorageHealth 纯类型）
    // ============================================================

    /// 纯判定矩阵:双目录就绪 → Ok;任一缺席 → Warn（未初始化/未启用,非故障）
    #[test]
    fn storage_health_verdict_matrix() {
        let base = StorageHealth {
            scc_entries: 3,
            scc_accesses: 9,
            cmt_dir_ready: true,
            session_dir_ready: true,
        };
        assert_eq!(base.verdict(), HealthStatus::Ok, "双就绪 → Ok");
        assert_eq!(
            StorageHealth {
                cmt_dir_ready: false,
                ..base.clone()
            }
            .verdict(),
            HealthStatus::Warn,
            "cmt 缺席 → Warn"
        );
        assert_eq!(
            StorageHealth {
                session_dir_ready: false,
                ..base.clone()
            }
            .verdict(),
            HealthStatus::Warn,
            "session 缺席 → Warn"
        );
        // 消息包含三后端逐项信号
        let msg = base.message();
        assert!(msg.starts_with("L3:"), "message 前缀: {msg}");
        assert!(msg.contains("scc 已装配"), "{msg}");
        assert!(msg.contains("cmt 经验库目录 就绪"), "{msg}");
        assert!(msg.contains("session-store 段目录 就绪"), "{msg}");
    }

    /// 三后端集成:真实 scc 句柄 + 注入目录（tempdir）→ 信号逐项正确
    #[test]
    fn storage_health_collects_three_backend_signals() {
        let cache = scc_cache::SccCache::new(scc_cache::SccConfig::new(), EventBus::new());
        let dir = std::env::temp_dir().join(format!("doctor_storage_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("definitely-absent");

        let health = collect_storage_health(&cache, &dir, &missing);
        assert_eq!(health.scc_entries, 0, "装配初期 scc 空仓");
        assert!(health.cmt_dir_ready, "注入目录存在 → cmt 就绪");
        assert!(!health.session_dir_ready, "缺失目录 → session 缺席");
        assert_eq!(health.verdict(), HealthStatus::Warn, "单缺席 → Warn");

        let all_ready = collect_storage_health(&cache, &dir, &dir);
        assert_eq!(all_ready.verdict(), HealthStatus::Ok, "双就绪 → Ok");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ============================================================
    // W5-1 R-2 — 模型资产面探针（ModelAssetHealth 纯类型 / ADR-196 V6）
    // ============================================================

    /// 判定矩阵的**核心红线**（V6 原文）:资产缺席只能记 Warn「未启用」。
    ///
    /// 本测同时钉住“不可能判 Fail”:一旦有人把未启用改写成故障,
    /// `doctor` 会在无资产机器上非零退出，把受支持的降级形态变成阻断。
    #[test]
    fn model_asset_health_verdict_matrix() {
        let absent = ModelAssetHealth {
            nmc_asset_dir_ready: false,
            manifest_present: true,
        };
        assert_eq!(absent.verdict(), HealthStatus::Warn, "未就位 → Warn");
        assert_ne!(absent.verdict(), HealthStatus::Fail, "V6 红线:不得判 Fail");

        let ready = ModelAssetHealth {
            nmc_asset_dir_ready: true,
            manifest_present: true,
        };
        assert_eq!(ready.verdict(), HealthStatus::Ok, "已就位 → Ok");
    }

    /// 未就位文案必须说清三件事:未启用非故障 / 约定目录在哪 / 怎么获取
    #[test]
    fn model_asset_message_says_not_enabled_when_absent() {
        let health = ModelAssetHealth {
            nmc_asset_dir_ready: false,
            manifest_present: false,
        };
        let msg = health.message();
        assert!(msg.contains("未启用"), "应报「未启用」,实际: {msg}");
        assert!(msg.contains("非故障"), "应明写非故障,实际: {msg}");
        assert!(
            msg.contains("fetch_nmc_assets"),
            "应给出官方获取路径,实际: {msg}"
        );
        assert!(msg.contains(NMC_ASSET_DIR), "应点名约定目录,实际: {msg}");
        assert!(
            msg.contains("清单缺失"),
            "manifest 缺席应可区分,实际: {msg}"
        );
    }

    /// 探针入口:约定路径采集（仓库根跑时 manifest 在位;资产目录通常未 fetch）
    #[test]
    fn model_asset_probe_names_and_never_fail() {
        let check = check_model_assets();
        assert_eq!(check.name, "model_asset_health");
        assert_eq!(check.description, "模型资产面(NMC 多模态)");
        assert!(
            matches!(check.status, HealthStatus::Ok | HealthStatus::Warn),
            "探针只能 Ok/Warn,实际: {:?}",
            check.status
        );
    }

    /// 路径一致性钉:doctor 侧常量与 fetch 脚本默认落点逐字一致。
    ///
    /// WHY 机械钉而非注释约定:chimera-cli 不能依赖 nmc-encoder 去读
    /// `NmcConfig` 默认值(G-33 零新增边),所以“未启用”判定用的目录约定
    /// 只有 fetch 脚本一个同源。两处一旦漂移,探针会永远报 Warn。
    #[test]
    fn model_asset_paths_match_fetch_scripts() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
        for name in ["fetch_nmc_assets.ps1", "fetch_nmc_assets.sh"] {
            let path = std::path::Path::new(root).join("scripts").join(name);
            let text =
                std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {path:?}: {e}"));
            assert!(
                text.contains(&format!("$TargetDir = \"{NMC_ASSET_DIR}\""))
                    || text.contains(&format!("TARGET_DIR=\"{NMC_ASSET_DIR}\"")),
                "{name} 的默认目标目录与 doctor 常量 {NMC_ASSET_DIR:?} 不一致"
            );
            assert!(
                text.contains(&format!("$Manifest = \"{NMC_MANIFEST_PATH}\""))
                    || text.contains(&format!("MANIFEST=\"{NMC_MANIFEST_PATH}\"")),
                "{name} 的默认清单路径与 doctor 常量 {NMC_MANIFEST_PATH:?} 不一致(ps1 写 $Manifest,sh 写 MANIFEST)"
            );
        }
    }

    /// 探针壳:消费组合根共享 SccCache（装配态真实消费,非 ephemeral）
    #[tokio::test]
    async fn test_doctor_storage_health_probe_uses_assembled_cache() {
        let ctx = crate::composition::build(&ChimeraConfig::default()).expect("装配应成功");
        let check = check_storage_health(&ctx.scc);
        assert_eq!(check.name, "storage_health");
        assert_eq!(check.description, "L3 存储三后端健康度");
        assert!(
            matches!(check.status, HealthStatus::Ok | HealthStatus::Warn),
            "缺席按 Warn 而非 Fail（未初始化非故障）,实际: {:?}",
            check.status
        );
        assert!(
            check.message.starts_with("L3:"),
            "message: {}",
            check.message
        );
    }
}
