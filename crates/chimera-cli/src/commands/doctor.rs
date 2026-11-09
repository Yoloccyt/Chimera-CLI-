//! `chimera doctor` — 系统健康检查,逐探针诊断 Chimera CLI 运行环境
//!
//! v2.9.0-omega Task 1.13:提供类 `cargo doctor` 的环境诊断能力。
//! Wave 2 Task 4:在原 5 维度基础上增加 LLM Provider 健康度检查。
//!
//! # 11 维度健康检查(SubTask 1.13.1 + Wave 2 Task 4 + WI-02 + M13 + WI-G12 + W5-1 R-2)
//! 1. **配置文件**(Config File)— `omega.yaml` 路径存在(可解析性由 `main` 加载保证)
//! 2. **Cargo.lock**(Dependency Lock)— 当前目录 `Cargo.lock` 存在(Rust 项目完整性)
//! 3. **SQLite 数据库路径**(SQLite Path)— `repo_wiki.db_path` 父目录可写
//! 4. **MCP 网格连通性**(MCP Mesh)— McpMesh 可创建,统计注册服务器数
//! 5. **EventBus 订阅者**(EventBus)— 共享 EventBus 可用,统计订阅者数
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
//! - **LLM 维只做配置解析校验，不探测连通性**（ADR-272）：旧实现 sleep 238ms +
//!   50/50 随机判定并把成功文案写成固定 `ping` 延迟，等于把 mock 伪装成真实 RTT。
//!   真实 mca-gateway 探测属独立波次，且不应直调 `llm::execute`（回环风险）。
//! - **探针 IO 纪律**：路径存在性一律 `tokio::fs::try_exists`，写盘经 `spawn_blocking`；
//!   探针并发执行，同步 IO 会占用 worker（ADR-241 P-08 / ADR-272）。

use anyhow::Result;
use event_bus::EventBus;
use nexus_contracts::command_validation::{Command, CommandPolicy, CommandValidator};
use seccore::command_validator::SecCoreCommandValidator;
use seccore::Sandbox;
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::config::{self, ChimeraConfig};
use crate::output;

mod report;

use report::print_report_human;
pub use report::{HealthCheck, HealthReport, HealthStatus, HealthSummary};

/// 执行 doctor 子命令 — 11 维度健康检查（WI-02 增强: +认证密钥 / +沙箱;M13: +SCC 缓存;WI-G12: +L3 存储三后端;W5-1 R-2: +模型资产面）
///
/// `json` flag(Task 1.7):`true` 时输出 JSON envelope(完整 HealthReport)。
/// `fix`(Task 1.13.4):`true` 时自动修复可修复项(当前仅配置文件缺失)。
/// `config_path`(W3-2):`--config` 显式路径;探针 0/2 不再各自 `config::load(None)`。
///
/// 兼容层 thin wrapper(M4 先例):独立调用场景经组合根装配 ephemeral
/// AppContext;dispatch 主链调 [`execute_with_ctx`] 共享 dispatch 级上下文。
pub async fn execute(
    cfg: &ChimeraConfig,
    json: bool,
    fix: bool,
    config_path: Option<PathBuf>,
) -> Result<()> {
    let ctx = crate::composition::build(cfg)?;
    execute_with_ctx(&ctx, cfg, json, fix, config_path).await
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
    config_path: Option<PathBuf>,
) -> Result<()> {
    tracing::info!(fix, "系统健康检查(11 维度, GQEP 并行 gather)");

    let checks = run_probes_parallel(ctx, cfg, fix, config_path).await;
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
/// 保证报告项数恒等于探针数(该 tail 实践中不可达:全部探针为毫秒级快路径,
/// 无长驻等待——LLM 探针的 238ms mock 睡眠已按 ADR-272 删除)。
///
/// W3-2:探针 0/2 复用 `main` 已加载的 `cfg` 与解析后的配置路径——
/// 删除探针内两次全量配置加载(debug ≈7.5ms),并修复
/// `doctor --config X` 实际校验默认路径的缺陷。路径解析失败仅使探针 0 报 Fail
/// (不 panic);`main` 侧的坏配置已在 dispatch 前以 exit 1 拦截。
async fn run_probes_parallel(
    ctx: &crate::composition::AppContext,
    cfg: &ChimeraConfig,
    fix: bool,
    config_path: Option<PathBuf>,
) -> Vec<HealthCheck> {
    const PROBE_COUNT: usize = 11;

    // cfg 以 owned clone 移入 future('static 约束);Clone 代价 = 一次配置结构拷贝。
    // 注:ADR-272 删除了 LLM 探针的 238ms mock 睡眠后，该拷贝在总耗时中的占比上升，
    // 但仍是常量级；探针并发收益的实测口径见 ADR-272 §6。
    // scc 句柄同理(SccCache Clone = Arc 引用计数,廉价):M13 第 9 探针消费
    // 组合根装配的共享缓存实例,而非探针内自建 ephemeral(那是装配态空转)。
    let cfg_owned = cfg.clone();
    // W3-2 探针 2（sqlite_path）直接消费已加载配置,不再自建全量加载。
    let cfg_for_sqlite = cfg.clone();
    let bus_owned = ctx.bus.clone();
    let scc_owned = ctx.scc.clone();
    // WI-G12 第 10 探针需要第二份 scc 句柄(Clone = Arc 引用计数,廉价)
    let scc_owned_b = ctx.scc.clone();
    // W3-2 探针 0（config_file）用解析后的路径(显式 --config 优先),
    // 并在 future 内消化 Err → Fail(与既有 FAIL 形态一致)。
    let resolved_config_path = config::resolve_config_path(config_path);

    // 探针注册序 = 报告输出序(与串行版一致,输出内容零变化;M13 起追加 scc_cache,
    // WI-G12 追加 storage_health,W5-1 R-2 追加 model_asset_health)
    let probes: Vec<gqep_executor::GqepFuture<(usize, HealthCheck)>> = vec![
        Box::pin(async move {
            let check = match resolved_config_path {
                Ok(path) => check_config_file(fix, &path).await,
                Err(e) => HealthCheck {
                    name: "config_file",
                    description: "配置文件路径与有效性",
                    status: HealthStatus::Fail,
                    message: format!("配置文件路径解析失败: {e}"),
                },
            };
            Ok((0, check))
        }),
        Box::pin(async move { Ok((1, check_cargo_lock().await)) }),
        Box::pin(async move { Ok((2, check_sqlite_path(&cfg_for_sqlite).await)) }),
        Box::pin(async move { Ok((3, check_mcp_mesh().await)) }),
        Box::pin(async move { Ok((4, check_event_bus(&bus_owned).await)) }),
        Box::pin(async move { Ok((5, check_llm_provider(&cfg_owned).await)) }),
        Box::pin(async move { Ok((6, check_auth_keys())) }),
        Box::pin(async move { Ok((7, check_seccore())) }),
        Box::pin(async move { Ok((8, check_scc_cache(&scc_owned))) }),
        Box::pin(async move { Ok((9, check_storage_health(&scc_owned_b).await)) }),
        Box::pin(async move { Ok((10, check_model_assets().await)) }),
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

mod probes;
use probes::*;

/// SQLite 路径探针的公开入口（供集成测试验收异步 IO 语义）
///
/// WHY 需要这层薄包装：`check_sqlite_path` 是 `pub(super)`（模块内可见），
/// 集成测试在 crate 外部，无法直接调用。而该探针的**异步 IO 语义**
/// （不阻塞 Tokio worker）需要从 crate 外部验证 —— 它是 ADR-241 W3-T1
/// 的核心验收项。
///
/// 这是一层**零逻辑**的转发：判定逻辑仍在 `probes::check_sqlite_path`，
/// 此处只做可见性放宽，不复制任何行为。
pub async fn probes_check_sqlite_path(cfg: &ChimeraConfig) -> HealthCheck {
    check_sqlite_path(cfg).await
}

/// LLM Provider 探针的公开入口（供集成测试验收「诚实 mock 文案」）
///
/// 与 [`probes_check_sqlite_path`] 同构的零逻辑转发：判定逻辑仍在
/// `probes::check_llm_provider`，此处只放宽可见性。
pub async fn probes_check_llm_provider(cfg: &ChimeraConfig) -> HealthCheck {
    check_llm_provider(cfg).await
}

// 人类可读渲染已迁至 `commands/doctor/report.rs`（T2.5 拆分）。

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
        execute(&cfg, true, false, None)
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

        execute_with_ctx(&ctx, &ChimeraConfig::default(), true, false, None)
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

    /// W3-2:sqlite 探针必须消费传入的 `ChimeraConfig`(而非自行全量加载),
    /// 否则 `doctor --config X` 会静默校验默认路径配置(缺陷基线)。
    #[tokio::test]
    async fn test_check_sqlite_path_consumes_passed_config() {
        let unique = format!("w3_sqlite_probe_{}", std::process::id());
        let mut cfg = ChimeraConfig::default();
        cfg.repo_wiki.db_path = format!("{unique}/db.sqlite");

        let check = check_sqlite_path(&cfg).await;
        assert_eq!(check.name, "sqlite_path");
        assert!(
            check.message.contains(&unique),
            "sqlite_path 探针必须消费传入 cfg 的 db_path({unique}),实际: {}",
            check.message
        );
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

    /// 三后端信号聚合:就绪布尔注入 → 逐项沉淀。
    ///
    /// 路径探测已上移到异步探针（`tokio::fs::try_exists`，ADR-272）;
    /// 本测只锁「布尔 → 字段 → 判定/文案」这一段纯逻辑。
    #[test]
    fn storage_health_collects_three_backend_signals() {
        let cache = scc_cache::SccCache::new(scc_cache::SccConfig::new(), EventBus::new());

        let health = collect_storage_health(&cache, true, false);
        assert_eq!(health.scc_entries, 0, "装配初期 scc 空仓");
        assert!(health.cmt_dir_ready, "注入就绪 → cmt 就绪");
        assert!(!health.session_dir_ready, "注入缺席 → session 缺席");
        assert_eq!(health.verdict(), HealthStatus::Warn, "单缺席 → Warn");

        let all_ready = collect_storage_health(&cache, true, true);
        assert_eq!(all_ready.verdict(), HealthStatus::Ok, "双就绪 → Ok");
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
    #[tokio::test]
    async fn model_asset_probe_names_and_never_fail() {
        let check = check_model_assets().await;
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
        let check = check_storage_health(&ctx.scc).await;
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
