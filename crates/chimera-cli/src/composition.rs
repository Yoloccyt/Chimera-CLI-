//! 集中组合根 — AppContext 装配（C12 + C1，评审波次 1；M4 P1+P2 扩展）
//!
//! # 背景（F-A2-4 / F-A2-5，来源 `DEEP_RESEARCH_NEXUS_OMEGA_architecture_review.md`）
//!
//! 此前装配分散于各命令 handler（依赖图靠 grep 才能还原），MCA 网关
//! "构造即丢弃"（main.rs 构造后无下游持有），serve/acp 跑在
//! `AppServer::new` 默认的 `InMemoryBackend` 假核心上（协议面对外
//! 承诺与实际能力脱节）。本模块**目标成为唯一装配点**：
//!
//! ```text
//! config → build() → AppContext { bus, engine, server_config }
//!        → build_app_server() → AppServer（真实 QuestBackend，C3 旁路由 build 保证）
//! ```
//!
//! 装配收敛实况（M4 P1+P2 + M9 + wave-1，2026-10 批次）：chat/run/exec/quest/
//! parliament/agent/tui 等 FullContext 命令已迁离自建 `EventBus::new()`，经
//! dispatch 共享本模块装配的 AppContext（serve/acp 走 `build_app_server`；
//! tui 的带检查点引擎经 `assemble_quest_stack` 基于共享 bus 装配）。
//! 生产代码命令级自建总线装配点已清零；doctor 的 `check_event_bus` 消费
//! 组合根共享 bus 并统计订阅者数（健康探针，非命令装配，有意保留）。

use crate::config::{ChimeraConfig, ToolPolicyAction};
use anyhow::{Context, Result};
use event_bus::{logging::BusLogger, EventBus};
use nexus_app_server::{AppServer, AppServerConfig, QuestBackend};
use quest_engine::QuestEngine;
use std::sync::Arc;

// R2 解冻阶段③ 前置：进化编排器（T3.1,feature-gated）
#[cfg(feature = "r2_unfreeze")]
use decay_engine::ShadowModeCircuitBreaker;
#[cfg(feature = "r2_unfreeze")]
use gsoe_evolution::{GsoeConfig, GsoeEvolutionEngine};

/// 进化编排器 — 封装 evolve_with_formal_verification 主路径（R2 解冻阶段③ 前置）
///
/// WHY 独立结构体：将 GSOE 引擎 + 熔断器封装为单一抽象，
/// 供 CLI/TUI 调用方通过 `step()` 方法执行完整进化循环（含 L4 门）。
#[cfg(feature = "r2_unfreeze")]
pub struct EvolutionOrchestrator {
    /// GSOE 进化引擎（L5 Knowledge）
    engine: GsoeEvolutionEngine,
    /// 影子模式熔断器（L4 Security, fail-closed 门）
    breaker: ShadowModeCircuitBreaker,
    /// 事件总线（用于发布 Critical 事件）
    event_bus: EventBus,
}

#[cfg(feature = "r2_unfreeze")]
impl EvolutionOrchestrator {
    /// 构造进化编排器
    pub fn new(
        engine: GsoeEvolutionEngine,
        breaker: ShadowModeCircuitBreaker,
        event_bus: EventBus,
    ) -> Self {
        Self {
            engine,
            breaker,
            event_bus,
        }
    }

    /// 单次进化迭代（含 L4 形式化验证门）
    ///
    /// # 流程
    /// 1. 获取 7 个形式化属性的验证结果（来自 EmptyFormalProvider → 全 Skipped）
    /// 2. 调用 engine.evolve_with_formal_verification 执行进化 + 门禁裁决
    /// 3. 许可则应用新策略，否决则回退并发布 Critical 事件
    ///
    /// # 返回
    /// - `Ok(())`: 进化成功
    /// - `Err(_)`: 进化被否决（门禁失败或熔断器跳闸）
    ///
    /// ⚠️ TODO: This method is currently a placeholder awaiting formal API definitions.
    /// See Phase 2 for implementation of EmptyFormalProvider.collect_results() and
    /// GsoeEvolutionEngine::default().
    pub async fn step(&mut self) -> Result<(), gsoe_evolution::error::GsoeError> {
        // Placeholder implementation - returns Ok to allow compilation during Phase 1
        // Full implementation will be added in Phase 2 when formal APIs are defined
        tracing::warn!("EvolutionOrchestrator::step() is a placeholder (Phase 1)");
        Ok(())
    }
}

/// 集中装配产物 — 依赖注入的一揽子载体（C12）
///
/// `bus` 为 `EventBus`（Clone = Arc 引用计数，廉价共享）；`engine` 为 owned，
/// 由 [`build_app_server`] 移入 `QuestBackend`（真实状态源归协议宿主持有）。
/// `gea` / `gqep` 为 M12（ADR-185 D1/D3）接线句柄：Arc 共享实例，
/// 分别注入 mas orchestrator（委托前门控激活）与 doctor（探针并行 gather）。
/// R2 解冻阶段③ 前置：`evolution_orchestrator` 封装进化引擎 + 熔断器 + 后悔率采集。
pub struct AppContext {
    /// 事件总线（全部组件共享的通信通道）
    pub bus: EventBus,
    /// L9 编排引擎（真实状态源，QuestCreated 等事件经 bus 广播）
    pub engine: QuestEngine,
    /// 协议宿主配置（由 ChimeraConfig 派生；字段级映射待协议配置面扩展）
    pub server_config: AppServerConfig,
    /// GEA 门控激活器（M12 / ADR-185 D1；E01-E08 编制已注册，共享给 mas）
    pub gea: Arc<gea_activator::GeaActivator>,
    /// GQEP 聚集执行器（M12 / ADR-185 D3；doctor 探针并行 gather 复用）
    pub gqep: Arc<gqep_executor::GqepExecutor>,
    /// 多代理调度控制面（P1 候选③ / ADR-211 Accepted 2026-10-12；
    /// `agent spawn` 前 claim 准入——claim/lease/quota，单任务即有真实语义）。
    /// `with_rate_limit` 护栏随批装配（ephemeral 进程内生效；跨进程留待常驻宿主）。
    pub sched: Arc<mas_sched::SimplePeerScheduler>,
    /// 推测上下文缓存（M13 / ADR-161 路径①；SccCache Clone=Arc 共享,
    /// doctor scc_cache 探针经此真实消费 stats()）
    pub scc: scc_cache::SccCache,
    /// 工具运行时 — 强制沙箱四环派发链（policy → sandbox → route → transport）
    ///
    /// WHY 在组合根而非各命令：四环链的沙箱层是**全局安全前置**，
    /// 在命令层装配必然遗漏某条路径 —— 这正是 ADR-238 落地后仍存在的
    /// 「四环链只被单元测试覆盖、生产路径无调用点」缺口（ADR-240 P-01）。
    ///
    /// 传输环 delegate 在此**唯一装配**为 [`crate::exec_plan::BuiltinToolExecutor`]
    /// （ADR-244 D1）。此前 `exec` 路径局部再装配一份，导致同进程内两种传输语义
    /// （同名工具两种归因）；收敛后 `exec` 复用本字段，策略规则与默认动作单源。
    pub tool_runtime: Arc<gqep_executor::tool_runtime::ToolRuntime>,
    /// SubAgent Quest 执行引擎（ADR-257 D1/D2：与 [`Self::engine`] 浅共享同一
    /// Quest 注册表；tokio 异步锁允许跨 await 持有——join_next 收 `&mut`
    /// （JoinSet 所有权语义），同一实例 spawn/join 串行化代价已登记 ADR-257）。
    /// D3：第一波仅装配 + 句柄暴露，驱动入口由命令面后续波次挂接。
    pub subagent_engine: Arc<tokio::sync::Mutex<nexus_app_server::SubAgentQuestEngine>>,
    /// hook.* 双轨注册表（ADR-258：P3 注册面闭合——14 个 hook.* 生命周期事件
    /// 经 HookEventBridge::register_all 幂等注册;轨二不占 154 变体数。
    /// r2_island_repayment feature 门控——nexus-hook 为 GATED 轨道 optional 依赖,
    /// 默认构建零符号/零边,可达性棘轮 38 不变。触发实例经 trigger_from_audit
    /// + register 挂载,消费方 get/serialize 拉取）。
    #[cfg(feature = "r2_island_repayment")]
    pub hook_registry: std::sync::Arc<event_bus::DynamicEventRegistry>,
    /// WI-D1：跨厂商辩论激活输入。`None`=未启用（默认，主干恒绿）；
    /// `Some(names)`=组合根从 `config.model_router` 解析出的去重 provider 通道名，
    /// 供 `chimera parliament` 命令经 `CrossVendorDebate::from_provider_names` 注入。
    pub cross_vendor_providers: Option<Vec<String>>,
    /// R2 解冻阶段③ 前置：进化编排器（封装 evolve_with_formal_verification 主路径）
    #[cfg(feature = "r2_unfreeze")]
    pub evolution_orchestrator: Option<EvolutionOrchestrator>,
    /// 非 r2_unfreeze 构建下的字段占位(保持元组尺寸一致,无运行时足迹)
    ///
    /// WHY `fn() -> ()` 而非 `*const u8`：裸指针使 AppContext 变成 `!Send + !Sync`，
    /// 任何未来 `tokio::spawn(dispatch)` 都会编译失败。函数指针是 Send+Sync 的零尺寸占位。
    #[cfg(not(feature = "r2_unfreeze"))]
    pub _evolution_orchestrator_placeholder: std::marker::PhantomData<fn() -> ()>,
}

// 编译期断言：AppContext 必须可跨任务共享（防止未来 spawn 才暴露 !Send/!Sync）。
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AppContext>();
};

/// 工具运行时策略 — 组合根从声明式配置源派生的 execpolicy 规则集与默认动作
///
/// # 规则源（F-01 / ADR-246）
///
/// 此前本函数硬编码 `(Vec::new(), Allow)`，策略环在生产路径形同虚设；ADR-246
/// 落地 `ChimeraConfig.tool_policy` 声明式段（nexus-core 纯类型，本函数负责
/// L1 类型 → L4 `seccore::execpolicy` 的映射——依赖方向 L10→L1→L4 合法）。
/// 装配期一次读取，**无热加载**：运行期切换策略属安全能力开关（ADR-242 §3.5）。
///
/// # WHY 默认动作缺省是 Allow 而非零信任 Deny
///
/// `seccore::execpolicy::ExecPolicy::new()` 的零信任默认是 `Deny`（未声明即拒绝），
/// 适用于「声明式白名单」语义。但 CLI 的工具执行路径在 ADR-240 接线前**本来
/// 就没有策略层**，把默认动作改成 `Deny` 会**削减现有功能**：所有工具调用从
/// 「可执行」变成「一律拒绝」。因此缺省姿态是 `Allow`（配置缺省同值）；需要
/// 零信任白名单的部署方在配置里显式声明 `default_action: deny`——**Deny 只有
/// 用户显式声明才生效**（ADR-240 D2 先例）。
///
/// **沙箱层独立于本函数真实生效**：注入形态、提权、沙箱逃逸、数据泄露、篡改
/// 六类拦截模式对 `tool_name + args_json` 完整生效（ADR-238 D4 安全论证）。
///
/// # 非法规则的处置（边界诚实）
///
/// - `action` 非法值：serde 边界硬失败（`load()` 退 1，错误可定位到非法值）；
/// - `pattern` 无有效 program（空/全空白）：跳过该条并 `tracing::warn!`
///   （fail-soft per-rule——一条笔误不拖垮整份策略，也不静默吞掉其余规则）。
///
/// # 回滚
/// 传输环已在组合根唯一装配为 `Some(BuiltinToolExecutor)`（ADR-244 D1），
/// 四环 policy→sandbox→route→transport 真实生效；回退本函数到 `(vec![], Allow)`
/// 即回到 ADR-246 前语义，不涉及任何安全层降级。
#[must_use]
pub fn tool_runtime_policy(
    config: &ChimeraConfig,
) -> (
    Vec<seccore::execpolicy::ExecPolicyRule>,
    seccore::execpolicy::PolicyAction,
) {
    let section = &config.tool_policy;
    let default_action = match section.default_action {
        ToolPolicyAction::Allow => seccore::execpolicy::PolicyAction::Allow,
        ToolPolicyAction::Ask => seccore::execpolicy::PolicyAction::Ask,
        ToolPolicyAction::Deny => seccore::execpolicy::PolicyAction::Deny,
    };
    let mut rules = Vec::with_capacity(section.rules.len());
    for def in &section.rules {
        let Some((program, args_glob)) = def.pattern_parts() else {
            tracing::warn!(
                pattern = %def.pattern,
                "tool_policy 规则 pattern 无有效 program 名，该条忽略（其余规则不受影响）"
            );
            continue;
        };
        let action = match def.action {
            ToolPolicyAction::Allow => seccore::execpolicy::PolicyAction::Allow,
            ToolPolicyAction::Ask => seccore::execpolicy::PolicyAction::Ask,
            ToolPolicyAction::Deny => seccore::execpolicy::PolicyAction::Deny,
        };
        rules.push(seccore::execpolicy::ExecPolicyRule {
            pattern: seccore::execpolicy::RulePattern::new(program, args_glob),
            action,
        });
    }
    (rules, default_action)
}

/// 装配 AppContext（唯一组合根入口，C12）
///
/// # 参数
/// - `config`：四源合并后的 CLI 配置。当前用于装配观测（版本号日志）；
///   `model_router`/`quest` section 到 `AppServerConfig` 的字段级映射
///   待协议配置面扩展（`AppServerConfig` 现仅 2 字段，见 c1 报告设计决策）。
///
/// # 标准装配步骤（M4-P2）
/// 1. 构造 EventBus + QuestEngine（bus/engine 共享同一 Arc 内核）；
/// 2. 注册 Critical 旁路订阅者（C3 契约全局生效——不再仅 serve/acp 协议宿主，
///    全部经组合根的命令进程内 Critical 事件均有真实消费者兜底）。
/// 3. 装配工具运行时四环链（ADR-240）：policy → sandbox → route → transport。
///
/// # 错误
/// 当前构造链不可失败（EventBus/QuestEngine 均无失败构造）；
/// 保留 `Result` 语义以便未来装配引入不可恢复预检时调用方 warn 降级
/// （对齐 main.rs init_mca_gateway 的优雅降级惯例）。
///
/// # 运行时要求
/// 内部经 [`spawn_critical_subscriber`] 调 `tokio::spawn`，**必须在 tokio runtime
/// 上下文调用**（无 runtime 时 `tokio::spawn` panic）。当前全部调用方
/// （dispatch async 主链、serve/acp 的 `async fn`、各命令 wrapper、
/// `#[tokio::test]`）满足此前置；同步上下文（如 proptest 闭包）须先建
/// runtime 再 `block_on` 驱动（见 tests/composition_root_e2e.rs 范式）。
pub fn build(config: &ChimeraConfig) -> Result<AppContext> {
    // 库调用方默认保 Auto：保持与既有 `EventBus::new()`+leaf opt-in 完全一致的语义
    // （9 crate 10 处 try_enable_sharding 调用点零行为变更）。CLI 路径经
    // `commands/mod.rs` 按 `spec::plan` 选定的策略调 `build_with_sharding`。
    build_with_sharding(config, crate::spec::ShardingPolicy::Auto)
}

/// 装配 AppContext，并按宿主类型注入事件分片策略（性能重构 D1）。
///
/// 与 [`build`] 的唯一差异：`ShardingPolicy::Off` 时在 `EventBus::new()` 之后、
/// 任何 leaf 组件（`GqepExecutor::new` / `SccCache::new`）构造之前调用
/// `bus.without_sharding()`，屏蔽后续 `try_enable_sharding` 的 64 worker spawn。
/// `Critical` mpsc 旁路、单流 broadcast、漏发率语义均不变（仅关并行扇出）。
pub fn build_with_sharding(
    config: &ChimeraConfig,
    sharding: crate::spec::ShardingPolicy,
) -> Result<AppContext> {
    let mut bus = EventBus::new();
    // D1：one-shot 命令在此关闭分片 opt-in，必须早于任何触发 try_enable_sharding
    // 的组件构造（gqep/scc）；显式 enable_sharding(n) 不受此开关影响（逃生门/测试）。
    if sharding == crate::spec::ShardingPolicy::Off {
        bus = bus.without_sharding();
    }
    // O-1 (ADR-191 D1): 挂载 L1 BusLogger，激活全套 Prometheus 计数器。
    // 此前 `EventBus::new()` 置 logger=None（bus.rs）⇒ logging.rs 指标生产死码。
    // BusLogger 自带 per-instance Registry（非全局默认），多次构造不冲突；
    // set_logger 为同步内联赋值，不影响后续 clone/&bus。指标导出(/metrics 或文件
    // dump)为 D1 后续增量，本步先接线使计数器真实累加。
    //
    // W3(ADR-248 C-A): 装配时序为「构造 → 注册批量归因计数器 → set_logger」——
    // 注册写的是 logger 内部 Registry，必须先于 Arc 收编（bus.rs:1113-1116 收值）；
    // sink 句柄在下方 ToolRuntime 装配处经 with_batch_sink 注入。
    let mut bus_logger = BusLogger::new("chimera-cli");
    let tool_batch_sink = Arc::new(crate::metrics_sink::TelemetryBatchSink::register(
        &mut bus_logger,
    ));
    bus.set_logger(bus_logger);
    // E-1 (ADR-191 D4): 可选 Critical WAL 持久化。默认关=保留 LogCriticalSink;
    // 设 CHIMERA_CRITICAL_WAL_PATH 则把“无 mpsc 订阅者”的 Critical 事件 JSONL 落盘,
    // 崩溃后 FileWalCriticalSink::replay 可审计(at-least-once)。走 env 而非新增
    // ChimeraConfig 段:避免动多段 Figment 与 config_sample 门(正式 config 化留后续;
    // ADR-246 的 tool_policy 段是首个走正式 config 化路径的安全邻域段)。
    if let Ok(wal_path) = std::env::var("CHIMERA_CRITICAL_WAL_PATH") {
        match event_bus::FileWalCriticalSink::open(&wal_path) {
            Ok(sink) => {
                bus = bus.with_critical_fallback(Arc::new(sink));
            }
            Err(e) => tracing::warn!(
                path = %wal_path, error = %e,
                "Critical WAL 打开失败,回退 LogCriticalSink"
            ),
        }
    }
    // WP-E（R7）：依赖推断模式经 env 三态开关接入（CHIMERA_QUEST_DEPENDENCY_MODE），
    // 默认未设 = Linear = 历史行为（分解器线性链 + run --execute 单批派发，逐字节等价）。
    // WHY 走 env 而非 ChimeraConfig：后者受 G-69 十五字段对拍门锁定，实验性开关
    // 不值得为此扩 config schema——与 CHIMERA_SCHED_CLAIM（composition.rs:332）同惯例。
    let mut quest_config = quest_engine::QuestConfig::default();
    if let Ok(mode) = std::env::var("CHIMERA_QUEST_DEPENDENCY_MODE") {
        if mode.trim().eq_ignore_ascii_case("heuristic-dag") {
            quest_config.dependency_mode = quest_engine::decompose::DependencyMode::HeuristicDag;
            tracing::info!("Quest 依赖推断已切换 HeuristicDag（波次并行执行）");
        }
    }
    let engine = QuestEngine::with_config(bus.clone(), quest_config);
    // ADR-257 D1 案 β：QuestEngine 浅 Clone 共享给 SubAgentQuestEngine——
    // 同一引擎状态的两个视图（Quest 注册表/事件总线共享，非独立副本）。
    // D2：AppContext 持 Arc<tokio::sync::Mutex<…>>（join_next 收 &mut，
    // JoinSet 所有权语义 ⇒ 同一实例 spawn/join 串行化，代价已登记 ADR-257）。
    // D3：第一波仅装配 + 句柄暴露——「Quest 分解自动委托」机制不存在
    // （deepdive §7.2 实证），驱动入口由命令面后续波次挂接，不虚构委托。
    let subagent_engine = Arc::new(tokio::sync::Mutex::new(
        nexus_app_server::SubAgentQuestEngine::new(engine.clone()).with_event_bus(bus.clone()),
    ));
    // M12(ADR-185 D1/D3):gea + gqep 组合根装配——与 bus 同生命周期,
    // Arc 共享实例注入 mas orchestrator / doctor(非"构造即丢弃":
    // agent 命令经 with_gea 消费 gea,doctor 经 gather_collected 消费 gqep)。
    // gea 构造可失败(config 校验),失败即装配失败——mas_gea_config 为
    // crate 内常量恒合法,此处 ? 仅保留未来配置面扩展的诚实错误路径。
    // ADR-275（W9）：分段计时（零行为变化）。
    //
    // 下面四段（gea / gqep / sched / scc）彼此无数据依赖，只共享 `bus.clone()`——
    // 是装配期并行的天然候选。但「可并行」不等于「值得并行」：并行要付线程
    // spawn 的固定成本（≈293µs/线程，见 coordinator.rs 先例）。先用数据裁决，
    // 再决定是否引入 `join!`/`thread::scope`；结论见 ADR-275 D2（否决并行）。
    let phase_t = std::time::Instant::now();
    let gea =
        gea_activator::GeaActivator::new(chimera_mas::gea_bridge::mas_gea_config(), bus.clone())
            .context("组合根装配 GeaActivator 失败(config 校验)")?;
    // E01-E08 静态编制 → gea 动态画像(幂等,启动期一次性)
    chimera_mas::gea_bridge::register_mas_experts(&gea);
    let gea_ms = phase_t.elapsed().as_secs_f64() * 1000.0;

    let phase_t = std::time::Instant::now();
    let gqep = gqep_executor::GqepExecutor::new(gqep_executor::GqepConfig::default(), bus.clone());
    let gqep_ms = phase_t.elapsed().as_secs_f64() * 1000.0;
    // P1 候选③（ADR-211 Accepted 2026-10-12）:mas-sched claim 准入调度器。
    // WHY with_rate_limit: broker 护栏（防轮询风暴）随批携带；ephemeral 进程内
    // 单次 claim 不触发阈值，跨进程防护留待常驻宿主（serve/tui）复用同一实例。
    // 运行期开关为 env 三态 CHIMERA_SCHED_CLAIM（off 默认），非 feature。
    let phase_t = std::time::Instant::now();
    let sched = mas_sched::SimplePeerScheduler::with_rate_limit(60, 60_000);
    let sched_ms = phase_t.elapsed().as_secs_f64() * 1000.0;

    let phase_t = std::time::Instant::now();
    // M13(ADR-161 路径①):scc-cache 组合根装配——默认配置(容量 256,零 IO 轻构造),
    // 与 bus 同生命周期;doctor scc_cache 探针每次运行经 ctx.scc 真实消费
    // stats()(非"构造即丢弃");CacheHit/Miss 事件仅由真实缓存访问触发,
    // 装配本身零事件流量(零行为变化)。
    let scc = scc_cache::SccCache::new(scc_cache::SccConfig::default(), bus.clone());
    let scc_ms = phase_t.elapsed().as_secs_f64() * 1000.0;

    let phase_t = std::time::Instant::now();
    // ADR-240 W2-T1 + ADR-244 D1：装配工具运行时四环链（policy → sandbox → route → transport）。
    //
    // WHY 在组合根：沙箱层是全局安全前置，命令层装配必然漏路径。
    // WHY 只在此装配：传输环 delegate 若在命令层再装配一份，同进程会出现两种
    // 传输语义（同名工具两种归因），且策略规则双源无编译期一致性保证。
    // 内置执行器仅暴露只读工具（路径受限、fs.read ≤256 KiB），未知工具恒 Err。
    let (tool_rules, tool_default_action) = tool_runtime_policy(config);
    // ADR-256 D2：Builtin 与 Mcp 双 delegate 前缀分派组合——`mcp.<server>.<tool>`
    // 进四环（policy/sandbox 前置完整），能力/路由表仅转发 Builtin（mcp 分支
    // None/空集为诚实声明，`mcp.*` 恒走 fallback）。唯一装配点不变（ADR-244 D1）。
    let tool_delegate: Arc<dyn gqep_executor::ToolExecutor> =
        Arc::new(crate::mcp_tool_executor::CompositeToolExecutor::new(
            crate::exec_plan::BuiltinToolExecutor,
            crate::mcp_tool_executor::McpToolExecutor::from_config(config),
        ));
    let tool_runtime = gqep_executor::tool_runtime::ToolRuntime::with_optional_delegate(
        Some(tool_delegate),
        tool_rules,
        tool_default_action,
    )
    .map_err(|e| anyhow::anyhow!("工具运行时四环链装配失败: {e}"))?
    // W3(ADR-248):批量归因出口——execute_batch 的 BatchAttribution 经
    // TelemetryBatchSink 递增 BusLogger 计数器（None=改造前行为，L1 等价）。
    .with_batch_sink(tool_batch_sink);
    let tool_runtime_ms = phase_t.elapsed().as_secs_f64() * 1000.0;
    tracing::debug!(
        gea_ms,
        gqep_ms,
        sched_ms,
        scc_ms,
        tool_runtime_ms,
        "composition phase timings"
    );
    // ADR-258（P3/U-03）：hook.* 双轨注册面闭合——DynamicEventRegistry 生产
    // 构造点落组合根（深审增补项 D1：EventBus 不持有该 registry，挂 L1 需动
    // 结构已否决）；register_all 幂等注册 14 个 hook.* 生命周期事件（轨二,
    // 不占 154 变体数）。r2_island_repayment feature 门控（nexus-hook 为
    // GATED 轨道 optional 依赖——默认构建零符号/零边，可达性棘轮 38 不变）。
    // 同步构造 + 同步注册，无 spawn ⇒ subscribe-before-spawn 纪律天然成立。
    #[cfg(feature = "r2_island_repayment")]
    let hook_registry = {
        let registry = std::sync::Arc::new(event_bus::DynamicEventRegistry::new());
        nexus_hook::event_bridge::HookEventBridge::register_all(&registry);
        registry
    };
    // M4-P2: C3 Critical 旁路注册上提为 build() 标准装配步骤（subscribe 在
    // engine 构造后、AppContext 移出前同步完成，§4.4 反模式 3 纪律）。
    spawn_critical_subscriber(&bus);

    // R2 解冻阶段③ 前置：进化编排器装配（T3.1,feature-gated）
    #[cfg(feature = "r2_unfreeze")]
    let evolution_orchestrator = Some(EvolutionOrchestrator::new(
        // WHY 显式 `GsoeConfig::default()` 而非 `GsoeEvolutionEngine::default()`:
        // 引擎未实现 Default(仅 new(GsoeConfig)),此前写法使 r2_unfreeze 分支从未编译通过。
        GsoeEvolutionEngine::new(GsoeConfig::default()),
        ShadowModeCircuitBreaker::new(),
        bus.clone(),
    ));
    #[cfg(not(feature = "r2_unfreeze"))]
    let _evolution_orchestrator: Option<()> = None;
    tracing::debug!(
        version = %config.nexus.version,
        "AppContext assembled at composition root (C12, critical bypass wired, gea+gqep+scc wired)"
    );
    // 性能重构 P2-3：细分启动段——把「本命令实际是否分片」显式化，使 one-shot Off
    // 与长驻 Auto 的差异可归因（`assemble_ms` 只覆盖整段，不足以证明分片税已去掉）。
    // 必须在 `bus` 移入 AppContext 之前读 `sharding_enabled()`。
    tracing::info!(
        sharding_policy = ?sharding,
        sharding_enabled = bus.sharding_enabled(),
        "startup phase: shard"
    );
    // WI-D1：跨厂商辩论激活输入（opt-in，默认 None）。
    // 启用条件：config.model_router.cross_vendor_debate == true 且去重后 provider >= 2；
    // 否则保持 None（Parliament 走默认，行为与本字段引入前一致）。
    // 仅在此存名字列表（provider id 字符串），把 ProviderId 亲和映射留在 parliament(L8)。
    let cross_vendor_providers = if config.model_router.cross_vendor_debate {
        let mut names: Vec<String> = Vec::new();
        for p in &config.model_router.providers {
            if !names.iter().any(|n| n == &p.id) {
                names.push(p.id.clone());
            }
        }
        if names.len() >= 2 {
            Some(names)
        } else {
            tracing::warn!(
                distinct = names.len(),
                "cross_vendor_debate 已启用但去重 provider 不足 2，本次不激活"
            );
            None
        }
    } else {
        None
    };
    Ok(AppContext {
        bus,
        engine,
        server_config: AppServerConfig::default(),
        gea: Arc::new(gea),
        gqep: Arc::new(gqep),
        sched: Arc::new(sched),
        scc,
        subagent_engine,
        #[cfg(feature = "r2_island_repayment")]
        hook_registry,
        tool_runtime: Arc::new(tool_runtime),
        cross_vendor_providers,
        #[cfg(feature = "r2_unfreeze")]
        evolution_orchestrator,
        #[cfg(not(feature = "r2_unfreeze"))]
        _evolution_orchestrator_placeholder: std::marker::PhantomData,
    })
}

/// 会话持久化数据目录：`~/.chimera/sessions/`（C-63）
///
/// WHY 与 [`crate::experience_loop::experience_db_path`] 同族（同目录、同 home 展开）：
/// 两个 SQLite 面必须落在同一处，否则备份/清理脚本要记两套路径。
///
/// # Q-2：环境变量覆盖（C-63 附带发现）
/// `CHIMERA_SESSION_DIR` 优先，空值等同未设。WHY 需要它：组合根接上双写后，
/// **测试与生产会共用同一个目录**（跑 `composition_root_e2e` 就真写进了用户主目录）。
/// 有覆盖后，集成测试/CI 可指到 tempdir，不必给别人家的 `~/.chimera/sessions/`
/// 塞垃圾；本地也能用一行 env 做隔离复现。
///
/// 默认路径抽成纯函数 `session_dir_from`，使“有/无覆盖”两条分支都能被单测
/// 直接驱动——改 env 的测试在并行下是竞态源，不能靠它测。
pub fn session_store_dir() -> std::path::PathBuf {
    session_dir_from(std::env::var_os("CHIMERA_SESSION_DIR"))
}

/// 纯函数：给定覆盖值算出会话目录（`None` / 空串 ⇒ 走默认 `~/.chimera/sessions`）。
fn session_dir_from(override_dir: Option<std::ffi::OsString>) -> std::path::PathBuf {
    if let Some(dir) = override_dir.filter(|d| !d.is_empty()) {
        return std::path::PathBuf::from(dir);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home)
        .join(".chimera")
        .join("sessions")
}

/// 装配协议宿主 AppServer（真实核心后端，C1）
///
/// 与 `AppServer::new`（InMemoryBackend 桩）的目标差异：
/// `QuestBackend::with_engine` 包装真实 L9 QuestEngine——`TurnSubmit`
/// 产出含真实 `quest_id` 的 `quest_state` Item。
///
/// Critical 旁路订阅由 [`build`] 标准装配保证（M4-P2 上提），本函数不再重
/// 复注册（避免双消费者）；engine/bus 移入 `QuestBackend` 后旁路订阅者仍存活
/// （EventBus Clone = Arc 共享内核，move 不丢订阅）。
///
/// # CBMR 会话双写（C-63 / ADR-224 D1 / ADR-225 D4）
/// 本函数是 **组合根**：在此构造 `CbmrWriter` 并交给
/// `AppServer::with_session_store`，使 `turn_submit` 落盘（段文件 + SQLite 索引）
/// 从 DORMANT 转为 PRODUCTION-WIRED。构造走 [`CbmrWriter::open`]（async）而不是
/// `new()`：打开/建库/PRAGMA/DDL 都是阻塞调用，必须进阻塞池（M51/C-62）。
///
/// # N-4 降级姿态（必须先定义才能接线的那个前置）
/// **存储不可用时服务照常启动**：`CbmrWriter::open` 失败只 `warn!` 并退回
/// [`AppServer::with_backend`]（双写关闭），不让会话持久化变成服务启动的
/// 单点失败。WHY：本 crate 的旧文档已自陈「现仅 Err 无降级」——若把 Err 向
/// 上传，磁盘不可写就会把整个协议宿主打死，而持久化的业务语义是
/// **best-effort 可观测**（replay 能力降级，但 turn 不失败）。
pub async fn build_app_server(ctx: AppContext) -> AppServer {
    let backend = Box::new(QuestBackend::with_engine(ctx.engine, ctx.bus));
    let store_dir = session_store_dir();
    match session_store::CbmrWriter::open(session_store::StoreConfig::with_dir(&store_dir)).await {
        Ok(store) => AppServer::with_session_store(ctx.server_config, backend, store),
        Err(e) => {
            tracing::warn!(
                dir = %store_dir.display(),
                error = %e,
                "会话持久化不可用，已降级为不双写启动（N-4 best-effort）"
            );
            AppServer::with_backend(ctx.server_config, backend)
        }
    }
}

/// 轻量 QuestEngine 的唯一合法派生构造点（TUI 协议模式使用）。
///
/// WHY 放在组合根：`QuestEngine::new` 属于装配动作；命令层直接调用会被
/// `check_composition_root.py` 记为绕过。
pub fn build_engine(bus: &EventBus) -> Arc<QuestEngine> {
    Arc::new(QuestEngine::new(bus.clone()))
}

/// TUI 检查点引擎的唯一合法派生构造点（供组合根门禁归零）。
///
/// WHY 放在组合根：`QuestEngine::with_checkpoints` 属于装配动作；
/// 命令层直接调用会被 `check_composition_root.py` 记为绕过。
pub fn build_checkpoint_engine(
    bus: &EventBus,
    checkpoint_dir: std::path::PathBuf,
) -> Arc<QuestEngine> {
    Arc::new(QuestEngine::with_checkpoints(
        bus.clone(),
        quest_engine::QuestConfig::default(),
        checkpoint_dir,
    ))
}

/// MCP 网格的唯一合法派生构造点（当前唯一调用方：doctor 的 MCP 探针）。
///
/// 注：`chimera mcp` 命令不走本构造点（旧注释「mcp 命令与 doctor 探针共用」是错的）；
/// 此处使用 `MeshConfig::default()`，未来需要配置透传时在此扩展。
pub fn build_mesh() -> mcp_mesh::McpMesh {
    mcp_mesh::McpMesh::new(mcp_mesh::MeshConfig::default())
}

/// 注册 Critical 旁路订阅者并 spawn 后台日志消费者（C1 配套，C3 接线；M4-P2 由 build() 标准调用）
///
/// WHY 组合根显式注册旁路消费者（B-a 语义更新）：`EventBus` 自 2026-09-12 起
/// 内建保底 sink（`LogCriticalSink` 结构化 error!，见 event-bus `critical_sink`
/// 模块）——**无订阅者时事件不再无痕**，但保底仅是最低落点。本函数提供
/// 进程内真实消费者，使 `has_critical_subscribers()==true`（组合根自检目标态），
/// 并作为未来 TUI 面板/告警管道消费的挂载点。消费策略：逐条 error! 结构化
/// 日志（event_type 字段），供宿主侧运维检索。进程生命周期内 detached
/// （长驻宿主无 orphan 语义）。
/// WHY 先同步 subscribe 再 spawn：§4.4 反模式 3（subscribe-then-spawn 纪律）。
fn spawn_critical_subscriber(bus: &EventBus) {
    let mut rx = bus.subscribe_critical_events();
    tokio::spawn(async move {
        // recv() 返回 Option<NexusEvent>：None = 通道关闭（所有 Sender 已 drop）
        while let Some(ev) = rx.recv().await {
            tracing::error!(
                event_type = ev.type_name(),
                "Critical event received via mpsc bypass"
            );
        }
    });
}

/// MCA 组合根装配（`#[cfg(feature = "mca")]` 门控，C12 单一构造点）
///
/// main.rs 启动诊断与未来 AppContext 扩展共用本构造函数，
/// 消除"双份构造代码"漂移面。M4 装配态语义保持不变
/// （空注册表、不接线、spec_count==0 自检；句柄存续待 M1+ 挂载 transport）。
/// 构造失败由调用方 warn 降级（绝不 panic，优雅降级惯例）。
#[cfg(feature = "mca")]
pub fn build_mca_gateway() -> Result<mca_gateway::McaGateway> {
    use mca_gateway::{McaGateway, McaGatewayConfig};

    // assembly-only: 构造空注册表网关（容量提示走安全默认，对齐 McaGatewayConfig::default）
    let gateway = McaGateway::new(McaGatewayConfig::default());
    // 装配态自检：空注册表、未做任何主动接线，证明"仅装配"。
    // 用 debug_assert_eq! 而非 assert_eq!：本函数返回 Result 且调用方（main.rs
    // init_mca_gateway）以 `if let Err` 接失败做 warn 降级（绝不 panic 惯例）——
    // 但 `if let Err` 接不住 panic，故 release 下必须无 panic（debug_assert 被编译掉）；
    // debug/test 下仍触发，保住 ADR-177 "装配态自检"的牙齿（composition 测试即在此跑）。
    debug_assert_eq!(
        gateway.spec_count(),
        0,
        "M4 assembly must not register any spec"
    );
    Ok(gateway)
}

/// 探针接线骨架（R10 WP-T3，ADR-294 附录——observe 手动档）
///
/// WHY 独立于 `build_mca_gateway`:后者是 ADR-177「装配态自检」的
/// assembly-only 语义（debug_assert spec_count==0），探针栈允许装载
/// spec（observe 标定需要真实通道与 spec 档案）——两者语义不同构，
/// 单独装配函数互不污染。
///
/// # 观察档纪律（蓝图 §5.3）
/// `ProbeMode::Observe` 默认:探针标定只发布 `ContextCapabilityUpdated`
/// 观测事件,**不改写 resident 定容输入**;转 apply 需独立治理裁决
/// (RSI 三件套守门)。`calibrate` 的执行器闭包（真实 5 针检索打
/// 「最便宜 flash 通道+缓存命中」）由调用方在真实通道可用时注入——
/// 本骨架只保证结构就位/数据流可走/默认不烧 token。
///
/// # 减法（U-10 纪律）
/// 不接定时调度、不建 L2 订阅链（ContextCapabilityUpdated→with_governor
/// 为设计文档状态 context_capability.rs:24）——无消费场景验证不造。
#[cfg(feature = "mca")]
pub struct McaProbeStack {
    /// 网关（spec 装载后 spec_count>0;装载失败维持空表）
    pub gateway: mca_gateway::McaGateway,
    /// 有效窗口探针（observe 档;经 with_bus 挂事件发布）
    pub probe: mca_gateway::EffectiveWindowProbe,
    /// 通道健康注册表（EWMA 数据面,探针复用;DegradedPublisher 消费方）
    pub health: std::sync::Arc<mca_gateway::HealthRegistry>,
}

/// 构造探针栈（R10 WP-T3 骨架,ADR-294 附录）
///
/// # 参数
/// - `bus`:事件总线(探针标定结果经 `ContextCapabilityUpdated` 发布)
/// - `spec_dir`:spec 档案目录(None 或目录不存在 ⇒ 空注册表,行为与
///   assembly-only 一致——探针无标定对象,calibrate 返回 Disabled)
///
/// # 减法
/// 不接定时调度(observe 手动档);不接真实通道(执行器闭包由调用方注入);
/// 默认构建(无 mca feature)零符号零边(本函数与结构体均 cfg 门控)。
#[cfg(feature = "mca")]
pub fn build_mca_probe_stack(
    bus: EventBus,
    spec_dir: Option<&std::path::Path>,
) -> Result<McaProbeStack> {
    use mca_gateway::{
        EffectiveWindowProbe, HealthRegistry, McaGateway, McaGatewayConfig, ProbeMode,
    };

    let gateway = McaGateway::new(McaGatewayConfig::default());
    if let Some(dir) = spec_dir {
        if dir.is_dir() {
            match mca_gateway::load_spec_dir(dir) {
                Ok(specs) => {
                    for spec in specs {
                        gateway.register_spec(spec);
                    }
                }
                Err(e) => {
                    // 装载失败降级为空表(warn,绝不 panic——优雅降级惯例);
                    // 探针无标定对象时 calibrate 自然 Disabled,不阻断启动
                    tracing::warn!(dir = %dir.display(), error = %e, "MCA spec 目录装载失败,探针栈降级为空表");
                }
            }
        }
    }
    let health = std::sync::Arc::new(HealthRegistry::new());
    let probe = EffectiveWindowProbe::with_bus(ProbeMode::Observe, bus);
    Ok(McaProbeStack {
        gateway,
        probe,
        health,
    })
}

/// R10 WP-T3 骨架冒烟:mca feature 下探针栈构造——空目录=空表 + Observe 模式
/// + health 可用(observe 手动档,不烧 token)
#[cfg(all(test, feature = "mca"))]
mod probe_stack_tests {
    use super::*;

    #[test]
    fn build_mca_probe_stack_smoke() {
        let bus = EventBus::new();
        // None 目录:gateway 空表(探针无标定对象,calibrate 自然 Disabled)
        let stack = build_mca_probe_stack(bus, None).expect("探针栈构造应成功");
        assert_eq!(stack.gateway.spec_count(), 0, "无目录=空注册表");
        assert_eq!(stack.probe.mode(), mca_gateway::ProbeMode::Observe);
        // health 数据面可用(degraded_channels 空)
        assert!(stack.health.degraded_channels().is_empty());
    }

    #[test]
    fn build_mca_probe_stack_missing_dir_degrades_gracefully() {
        let bus = EventBus::new();
        // 目录不存在:降级空表(warn),绝不 panic
        let stack =
            build_mca_probe_stack(bus, Some(std::path::Path::new("Z:/definitely/not/exist")))
                .expect("目录不存在应优雅降级而非失败");
        assert_eq!(stack.gateway.spec_count(), 0);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use nexus_contracts::app::{AppEvent, AppOp, Item, ThreadStartParams, UserInput};

    fn make_ctx() -> AppContext {
        build(&ChimeraConfig::default()).expect("装配应成功")
    }

    /// WI-D1 验收：默认（cross_vendor_debate off）→ cross_vendor_providers 为 None，
    /// 行为与字段引入前完全一致（主干恒绿）。
    /// #[tokio::test]：`build()` 内部 `spawn_critical_subscriber` 调 `tokio::spawn`，需运行时上下文。
    #[tokio::test]
    async fn composition_cross_vendor_opt_in_default_off() {
        let ctx = build(&ChimeraConfig::default()).expect("装配应成功");
        assert!(
            ctx.cross_vendor_providers.is_none(),
            "默认 off → 不应激活跨厂商"
        );
    }

    /// WI-D1 验收：启用且默认 5 provider（去重 >=2）→ Some(去重名列表)。
    #[tokio::test]
    async fn composition_cross_vendor_opt_in_on_produces_names() {
        let mut cfg = ChimeraConfig::default();
        cfg.model_router.cross_vendor_debate = true;
        let ctx = build(&cfg).expect("装配应成功");
        let names = ctx
            .cross_vendor_providers
            .expect("启用且有 >=2 provider 应 Some");
        assert!(
            names.len() >= 2,
            "应解析出去重后 >=2 provider，实际 {names:?}"
        );
    }

    /// 构造已装配的 AppServer，并返回 bus 探针（Clone 共享内核状态，
    /// 用于断言 Critical 旁路订阅者存在性——C1×C3 接线证据）。
    async fn make_running_server() -> (AppServer, EventBus) {
        let ctx = make_ctx();
        let probe = ctx.bus.clone();
        let server = build_app_server(ctx).await;
        (server, probe)
    }

    /// C1 验收：组合根装配的 AppServer 处理 TurnSubmit 产出真实引擎特征
    /// Item（`quest_state` + 真实 quest_id）——InMemory 桩无此 kind。
    #[tokio::test]
    async fn composition_server_routes_turns_to_real_engine() {
        let (server, _probe) = make_running_server().await;
        // 完整两步协议流：ThreadStart → TurnSubmit
        let events = server
            .handle_op(&AppOp::ThreadStart(ThreadStartParams::new(
                "goal-1", "run-1",
            )))
            .await
            .expect("ThreadStart 应成功");
        assert!(!events.is_empty(), "ThreadStart 应产生事件");
        let thread_id = match events.first().expect("应产生首个事件") {
            AppEvent::ThreadStarted { thread } => thread.thread_id.clone(),
            other => panic!("首个事件应为 ThreadStarted，实际 {other:?}"),
        };
        let events = server
            .handle_op(&AppOp::TurnSubmit {
                thread_id,
                input: UserInput {
                    text: "分析项目依赖".into(),
                    extras: None,
                },
            })
            .await
            .expect("TurnSubmit 应成功");
        // Item 以 ItemChanged 事件携带（协议面：事件流即产物流）
        let items: Vec<&Item> = events
            .iter()
            .filter_map(|ev| match ev {
                AppEvent::ItemChanged { item } => Some(item),
                _ => None,
            })
            .collect();
        let quest_item = items
            .iter()
            .find(|it| it.kind.as_ref() == "quest_state")
            .unwrap_or_else(|| {
                panic!(
                    "真实引擎应产出 quest_state Item（InMemory 桩无此 kind）；实际 kinds: {:?}",
                    items.iter().map(|it| &*it.kind).collect::<Vec<_>>()
                )
            });
        assert!(
            quest_item.payload.contains("quest_id"),
            "载荷应含真实 quest_id（引擎生成，非空）"
        );
    }

    /// C1×C3 验收：组合根装配后 Critical 旁路订阅者存在——
    /// mpsc 送达保障在协议宿主真实生效（has_critical_subscribers 为 true）。
    #[tokio::test]
    async fn composition_server_subscribes_critical_bypass() {
        let (server, probe) = make_running_server().await;
        let _ = server;
        assert!(
            probe.has_critical_subscribers(),
            "组合根应注册 Critical 旁路订阅者（C1×C3 接线）"
        );
    }

    /// M4-P2 验收：Critical 旁路注册上提为 build() 的标准装配步骤 ——
    /// C3 契约对**所有**经组合根的命令全局生效（不再仅 serve/acp 协议宿主）。
    /// 此测试先行失败（红）：build() 当前不注册旁路，仅 build_app_server 注册。
    #[tokio::test]
    async fn composition_build_registers_critical_bypass() {
        let ctx = make_ctx();
        assert!(
            ctx.bus.has_critical_subscribers(),
            "build() 应注册 Critical 旁路订阅者（M4-P2：C3 全局生效）"
        );
    }

    /// M12 / ADR-185 D1×D3 验收：组合根 gea/gqep 句柄真实装配非空转 ——
    /// gea 已注册 E01-E08 编制（8 专家），gqep 可服务 gather（共享 bus）。
    #[tokio::test]
    async fn composition_build_wires_gea_and_gqep() {
        let ctx = make_ctx();
        assert_eq!(
            ctx.gea.expert_count(),
            8,
            "组合根 gea 应已注册 E01-E08 静态编制（mas 桥接）"
        );
        assert!(
            ctx.gqep.config().gather_deadline_ms > 0,
            "gqep 双层超时配置应生效（全局 deadline 启用）"
        );
    }

    /// M13 / ADR-161 路径① 验收：组合根 scc 句柄真实装配非空转 ——
    /// SccCache 默认配置就绪（容量 256、空缓存、命中率 0），可服务
    /// doctor scc_cache 探针的 stats() 消费（共享 bus 生命周期）。
    #[tokio::test]
    async fn composition_build_wires_scc() {
        let ctx = make_ctx();
        assert!(ctx.scc.is_empty(), "装配初期缓存应为空");
        assert_eq!(ctx.scc.len(), 0);
        let stats = ctx.scc.stats();
        assert_eq!(stats.entry_count, 0);
        assert!((stats.hit_rate - 0.0).abs() < f32::EPSILON);
    }

    /// 性能重构 D1 验收：`ShardingPolicy::Off` 装配的 AppContext，其 bus 不应启用分片——
    /// `without_sharding()` 应在 gqep/scc 构造前屏蔽其 `try_enable_sharding`。
    #[tokio::test]
    async fn composition_build_with_sharding_off_keeps_single_stream() {
        let ctx = build_with_sharding(&ChimeraConfig::default(), crate::spec::ShardingPolicy::Off)
            .expect("Off 装配应成功");
        assert!(
            !ctx.bus.sharding_enabled(),
            "one-shot 策略下 gqep/scc 的 try_enable_sharding 应被 without_sharding 屏蔽"
        );
    }

    /// 性能重构 D1 验收：`ShardingPolicy::Auto`（= `build` 默认）应启用分片，
    /// 与既有 leaf opt-in 语义一致（长驻宿主保留扇出）。
    #[tokio::test]
    async fn composition_build_default_still_enables_sharding() {
        let ctx = build(&ChimeraConfig::default()).expect("默认(Auto) 装配应成功");
        assert!(
            ctx.bus.sharding_enabled(),
            "长驻宿主默认应启用分片（与既有 leaf opt-in 语义一致）"
        );
        // 优雅清理：避免 64 worker 残留到测试 runtime（guard 也会在 drop 时 abort）
        ctx.bus.shutdown_sharding().await;
    }

    /// C-63 附带发现 / Q-2：会话目录必须有 env 覆盖，否则测试与生产共用用户主目录。
    ///
    /// WHY 测纯函数而不是改 env：env 是进程级全局，并行测试里改它本身就制造竞态，
    /// 所以把“有/无覆盖”两条分支抽成 `session_dir_from` 直接驱动。
    #[test]
    fn session_dir_honours_env_override_and_falls_back_to_home() {
        use std::ffi::OsString;

        // 有覆盖（非空）⇒ 原样用
        let overridden = session_dir_from(Some(OsString::from("/tmp/chimera-sess-probe")));
        assert_eq!(overridden.to_string_lossy(), "/tmp/chimera-sess-probe");

        // 空串等同未设 ⇒ 回落到 ~/.chimera/sessions（不把空值当成根目录！）
        let empty = session_dir_from(Some(OsString::from("")));
        assert!(
            empty.ends_with("sessions"),
            "空覆盖值应回落默认目录，实际: {empty:?}"
        );
        assert!(
            empty.components().count() >= 2,
            "回落路径不应退化成单段相对路径，实际: {empty:?}"
        );

        // 未设 ⇒ 同样回落
        let none = session_dir_from(None);
        assert_eq!(none, empty, "未设与空串应得到同一个默认目录");
    }
}
