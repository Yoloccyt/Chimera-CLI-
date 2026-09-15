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
//! 装配收敛实况（M4 P1+P2，2026-09 批次）：chat/run/exec/quest/parliament/agent
//! 六命令已迁离自建 `EventBus::new()`，经 dispatch 共享本模块装配的
//! AppContext（命令保留原签名 thin wrapper，内部转组合根 ephemeral 装配）。
//! 剩余各自装配点仅 `commands/tui.rs`（下一波次显式排除）；doctor 的
//! `check_event_bus` 是健康探针（验证总线可构造），非命令装配，有意保留。

use crate::config::ChimeraConfig;
use anyhow::Result;
use event_bus::EventBus;
use nexus_app_server::{AppServer, AppServerConfig, QuestBackend};
use quest_engine::QuestEngine;

/// 集中装配产物 — 依赖注入的一揽子载体（C12）
///
/// `bus` 为 `EventBus`（Clone = Arc 引用计数，廉价共享）；`engine` 为 owned，
/// 由 [`build_app_server`] 移入 `QuestBackend`（真实状态源归协议宿主持有）。
pub struct AppContext {
    /// 事件总线（全部组件共享的通信通道）
    pub bus: EventBus,
    /// L9 编排引擎（真实状态源，QuestCreated 等事件经 bus 广播）
    pub engine: QuestEngine,
    /// 协议宿主配置（由 ChimeraConfig 派生；字段级映射待协议配置面扩展）
    pub server_config: AppServerConfig,
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
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus.clone());
    // M4-P2: C3 Critical 旁路注册上提为 build() 标准装配步骤（subscribe 在
    // engine 构造后、AppContext 移出前同步完成，§4.4 反模式 3 纪律）。
    spawn_critical_subscriber(&bus);
    tracing::debug!(
        version = %config.nexus.version,
        "AppContext assembled at composition root (C12, critical bypass wired)"
    );
    Ok(AppContext {
        bus,
        engine,
        server_config: AppServerConfig::default(),
    })
}

/// 装配协议宿主 AppServer（真实核心后端，C1）
///
/// 与 `AppServer::new`（InMemoryBackend 桩）的目标差异：
/// `QuestBackend::with_engine` 包装真实 L9 QuestEngine——`TurnSubmit`
/// 产出含真实 `quest_id` 的 `quest_state` Item。
///
/// Critical 旁路订阅由 [`build`] 标准装配保证（M4-P2 上提），本函数不再重复
/// 注册（避免双消费者）；engine/bus 移入 `QuestBackend` 后旁路订阅者仍存活
/// （EventBus Clone = Arc 共享内核，move 不丢订阅）。
pub fn build_app_server(ctx: AppContext) -> AppServer {
    let backend = Box::new(QuestBackend::with_engine(ctx.engine, ctx.bus));
    AppServer::with_backend(ctx.server_config, backend)
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

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::app::{AppEvent, AppOp, Item, ThreadStartParams, UserInput};

    fn make_ctx() -> AppContext {
        build(&ChimeraConfig::default()).expect("装配应成功")
    }

    /// 构造已装配的 AppServer，并返回 bus 探针（Clone 共享内核状态，
    /// 用于断言 Critical 旁路订阅者存在性——C1×C3 接线证据）。
    fn make_running_server() -> (AppServer, EventBus) {
        let ctx = make_ctx();
        let probe = ctx.bus.clone();
        let server = build_app_server(ctx);
        (server, probe)
    }

    /// C1 验收：组合根装配的 AppServer 处理 TurnSubmit 产出真实引擎特征
    /// Item（`quest_state` + 真实 quest_id）——InMemory 桩无此 kind。
    #[tokio::test]
    async fn composition_server_routes_turns_to_real_engine() {
        let (server, _probe) = make_running_server();
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
        let (server, probe) = make_running_server();
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
}
