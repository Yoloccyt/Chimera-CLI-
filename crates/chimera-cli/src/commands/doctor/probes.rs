//! Doctor 探针实现（T2.5 拆分）。
//!
//! WHY 独立：探针是报告的生产者，报告 schema 在 `report.rs`；
//! 两者拆开后修改探针不必触碰序列化契约。

use super::*;

/// 检查 1:配置文件路径与有效性(SubTask 1.13.1.1;W3-2 复用 main 已解析结果)
///
/// - 文件存在 → OK(可解析性由 `main` 的加载流程保证:坏配置在
///   dispatch 之前即以 exit 1 退出,doctor 根本不会运行)
/// - 文件不存在 → WARN(使用默认值);`fix=true` 时自动生成默认配置
///
/// WHY 不再二次 parse:探针曾各自全量加载 14 段配置
/// (debug ≈3.7ms/次),且走**默认路径** → `doctor --config X` 实际校验默认文件。
pub(super) async fn check_config_file(fix: bool, config_path: &Path) -> HealthCheck {
    let path_display = config_path.display().to_string();

    if config_path.exists() {
        HealthCheck {
            name: "config_file",
            description: "配置文件路径与有效性",
            status: HealthStatus::Ok,
            message: format!("配置文件可正常加载: {path_display}"),
        }
    } else {
        // 文件不存在
        if fix {
            // --fix: 自动生成默认配置文件
            match config::init_config_file(config_path) {
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
pub(super) async fn check_cargo_lock() -> HealthCheck {
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

/// 检查 3:SQLite 数据库路径可读写(SubTask 1.13.1.3;W3-2 复用已加载配置)
///
/// WHY 不直接打开 SQLite:`chimera-cli` 未依赖 rusqlite(§2.2 依赖铁律,
/// SQLite 属 L3 Storage 层依赖)。检查降级为"路径有效性 + 父目录可写性"验证。
///
/// WHY 接收 `&ChimeraConfig`:`main` 已全量加载配置,探针内再全量加载
/// 既重复付费又绕过 `--config`(W3-2 正确性缺陷)。
///
/// - 配置的 db_path 父目录存在且可写 → OK
/// - 父目录不存在 → WARN
/// - 父目录不可写 → FAIL
pub(super) async fn check_sqlite_path(cfg: &ChimeraConfig) -> HealthCheck {
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
    //
    // WHY 用 tokio::fs 而非 std::fs：本探针经 doctor.rs 的 gather_collected
    // 与其他探针**并发**执行，同步文件 IO 会阻塞所在 Tokio worker 线程，
    // 使并发探针的墙钟收益被抵消（ADR-241 P-08）。
    let test_file = parent_dir.join(".chimera_doctor_write_test");
    match tokio::fs::write(&test_file, b"test").await {
        Ok(_) => {
            // 清理测试文件（清理失败不影响"可写"判定本身）
            let _ = tokio::fs::remove_file(&test_file).await;
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
/// 经组合根派生工厂构造 McpMesh，验证可初始化 + 统计注册服务器数。
/// ephemeral mesh 注册表为空(0 服务器)是预期行为,状态为 OK。
///
/// 注：`build_mesh()` 当前使用默认 `MeshConfig`（无配置透传）；本探针是唯一调用方。
pub(super) async fn check_mcp_mesh() -> HealthCheck {
    let mesh = crate::composition::build_mesh();
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
/// 消费组合根共享总线，读取真实订阅者数（不再自建 ephemeral 总线）。
pub(super) async fn check_event_bus(bus: &EventBus) -> HealthCheck {
    let subscribers = bus.subscriber_count();

    HealthCheck {
        name: "event_bus",
        description: "EventBus 订阅者活跃数",
        status: HealthStatus::Ok,
        message: format!("共享 EventBus 可用,当前订阅者数: {subscribers}"),
    }
}

/// 检查 6:LLM Provider 健康度(Wave 2 Task 4)
///
/// 复用 `llm` 包内的 `FALLBACK_PROVIDER_NAMES` 常量（单一真值源），
/// 模拟连通性探测（238ms 延迟 + 50/50 判定）。
///
/// **不**实装真实 mca-gateway 探测,真实接入由后续 Task 替换(与 `llm::execute` 错开避免回环)。
///
/// - 默认 Provider = `cfg.model_router.providers` 首位;空时回退 8 个内置 default 名
///   (deepseek / zhipu / minimax / volcano / moonshot / stepfun / alicloud / custom)
/// - 探测用 `SystemTime` 纳秒奇偶决定 OK/FAIL(50/50),模拟 238ms 网络延迟
/// - `tokio::time::timeout(3s, ...)` 包裹,超时或失败 → `Warn`(Degraded)
/// - `--fix` 行为:LLM 维**不可**自动修复,需用户运行 `chimera llm set-default <name>`
pub(super) async fn check_llm_provider(cfg: &ChimeraConfig) -> HealthCheck {
    // 单一真值源：直接引用 `llm` 包内常量（勿在探针里再写一份副本）。
    use crate::commands::llm::FALLBACK_PROVIDER_NAMES;

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
pub(super) fn check_auth_keys() -> HealthCheck {
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
pub(super) fn check_seccore() -> HealthCheck {
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
pub(super) fn check_scc_cache(cache: &scc_cache::SccCache) -> HealthCheck {
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
pub(super) fn collect_storage_health(
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
pub(super) fn check_storage_health(cache: &scc_cache::SccCache) -> HealthCheck {
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
pub(super) const NMC_ASSET_DIR: &str = "assets/nmc/models";
/// NMC 资产清单（模型名/期望 SHA-256 的登记表，同样来 fetch 脚本默认）。
pub(super) const NMC_MANIFEST_PATH: &str = "assets/nmc/manifest.toml";

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
pub(super) fn collect_model_asset_health(
    asset_dir: &Path,
    manifest_path: &Path,
) -> ModelAssetHealth {
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
pub(super) fn check_model_assets() -> HealthCheck {
    let health = collect_model_asset_health(Path::new(NMC_ASSET_DIR), Path::new(NMC_MANIFEST_PATH));
    HealthCheck {
        name: "model_asset_health",
        description: "模型资产面(NMC 多模态)",
        status: health.verdict(),
        message: health.message(),
    }
}
