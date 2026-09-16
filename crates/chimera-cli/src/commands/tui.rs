//! `chimera tui` — TUI 交互界面
//!
//! 调用 `chimera-tui` crate 启动 ratatui 终端界面。
//! 生产环境通过 EventBus 订阅实时数据，替代默认的 StubDataSource。
//!
//! # 装配说明(M9 装配收敛,同 M4 六臂先例)
//! bus 来自 dispatch 级共享装配 `AppContext`([`crate::composition::build`],C12
//! 唯一装配点),tui 臂不再自建总线——Quest 编排/经验闭环/握手/Action 编排等
//! 全部事件经共享 bus 对进程内订阅者可见(C3 Critical 旁路由 build() 保证)。
//! Quest 引擎例外地保留臂内构造(带检查点管理器,见 [`assemble_quest_stack`] WHY)。
//!
//! # v3-engine M2 切换(ADR-061)
//! 自研渲染路径默认启用,通过 `--no-v3-engine` flag 或 `CHIMERA_NO_V3_ENGINE=1`
//! 环境变量可回退到 ratatui 路径。回退机制保留 2 个版本周期(v2.11.0-omega 移除)。
//!
//! # 超窗/RAG 链路生产接线(P1,ADR-072)
//! `execute_with_ctx` 组合根创建 `OverWindowBridge`(挂 TUI 会话总线)并注入
//! `OverWindowHandle`(桥 + 会话语料提供者)给 Action 编排器;`overwindow.run`
//! 经 `TuiActionRequested` 协议真实执行两级检索,触发时发布
//! `OverWindowFallbackTriggered` → EventSubscriber → DataPipeline → latest_events
//! → OverWindow 面板结构化展示(零管道侵入)。

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::action_orchestrator::OverWindowHandle;
use crate::composition::AppContext;
use crate::config::ChimeraConfig;
use crate::overwindow_bridge::OverWindowBridge;

/// 从环境变量构造 Quest 编排器配置(H-a:打字机节奏的用户开关)。
///
/// # 环境变量
/// - `CHIMERA_TUI_CHUNK_DELAY_MS`:每**字符**的流式延迟毫秒数;缺省 20ms,
///   设 `0` 关闭打字机节奏(即时上屏,适合长回复/自动化场景)。
///
/// # 返回
/// [`OrchestratorConfig`](crate::orchestrator::OrchestratorConfig):
/// `chunk_delay` 取自环境(非法/缺失回退 20ms),`chunk_batch_chars` 取默认批大小。
///
/// WHY 在组合根读环境而非 `Default::default()` 内:与 `commands/run.rs`
/// (`CHIMERA_RUN_CHUNK_DELAY_MS`)、`commands/chat.rs` 的既有先例一致 ——
/// 配置默认值保持纯函数语义(测试与 bench 可确定复现),环境覆盖只在进程入口发生。
fn orchestrator_config_from_env() -> crate::orchestrator::OrchestratorConfig {
    let delay_ms = std::env::var("CHIMERA_TUI_CHUNK_DELAY_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(20);
    crate::orchestrator::OrchestratorConfig {
        chunk_delay: Duration::from_millis(delay_ms),
        ..Default::default()
    }
}

/// FC-05 适配器(2026-09-06 评估):把 L2 `MlcEngine` 卡片系统统计视图映射为
/// TUI ExperienceCardViz 面板数据。
///
/// WHY 依赖倒置:trait `ExperienceCardStatsProvider` 定义在 chimera-tui(L10),
/// 实现驻留组合根(chimera-cli)——L10 不直接依赖 L2 内部结构,适配器仅做
/// 纯字段映射;`Debug` 手工实现(MlcEngine 未实现 Debug,仅打印 Arc 地址)。
#[derive(Clone)]
struct MlcCardStatsProvider(std::sync::Arc<mlc_engine::MlcEngine>);

impl std::fmt::Debug for MlcCardStatsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MlcCardStatsProvider({:p})",
            std::sync::Arc::as_ptr(&self.0)
        )
    }
}

impl chimera_tui::panels::ExperienceCardStatsProvider for MlcCardStatsProvider {
    fn global_stats(&self) -> chimera_tui::panels::ExperienceCardVizStats {
        let v = self.0.card_system_view();
        chimera_tui::panels::ExperienceCardVizStats {
            total_cards: v.total_cards,
            evaluated: v.evaluated,
            unique_errors: v.unique_errors,
            method_distribution: v.method_distribution,
            best_score: v.best_score,
            average_score: v.average_score,
        }
    }
}

/// tui 臂 Quest 栈装配（M9 装配收敛）— 从共享 AppContext 派生会话引擎 + 控制订阅者
///
/// # 参数
/// - `ctx`：dispatch 级共享装配上下文，**bus 唯一来源**（C12 唯一装配点）
/// - `checkpoint_dir`：检查点根目录（B1：与 tui.yaml 同根 `~/.chimera/checkpoints`）
///
/// # 返回
/// `(engine, control_handle)`：
/// - `engine`：带检查点管理器的 QuestEngine（Arc 共享，编排器/经验闭环/Action 编排复用）
/// - `control_handle`：quest-engine 控制事件订阅者后台句柄（调用方负责 abort，
///   遵循 §4.4 反模式 #7 关键路径句柄管理）
///
/// WHY 不用 `ctx.engine`：组合根装配的是裸 `QuestEngine::new`（run.rs:100 同因
/// 注释——CLI 单次运行无检查点需求，且 `~/.chimera` 目录可能不存在导致
/// CheckpointManager 初始化失败风险）；而 tui 臂 B1 依赖 `save_checkpoint`。
/// 故基于 **共享 bus** 臂内构造 `with_checkpoints` 引擎：事件全经 dispatch 共享
/// 总线（装配收敛目标达成），检查点能力对外零变化。
fn assemble_quest_stack(
    ctx: &AppContext,
    checkpoint_dir: std::path::PathBuf,
) -> (Arc<quest_engine::QuestEngine>, tokio::task::JoinHandle<()>) {
    let engine = Arc::new(quest_engine::QuestEngine::with_checkpoints(
        ctx.bus.clone(),
        quest_engine::QuestConfig::default(),
        checkpoint_dir,
    ));
    let control_handle =
        quest_engine::spawn_control_subscriber(Arc::clone(&engine), ctx.bus.clone());
    (engine, control_handle)
}

/// 执行 tui 命令 — 兼容层 thin wrapper（M9 装配收敛，同 M4 六臂先例）
///
/// 独立调用场景（库调用方）经组合根装配 ephemeral AppContext；
/// dispatch 主链直接调 [`execute_with_ctx`] 共享 dispatch 级 AppContext
/// （C12 唯一装配点）。
pub async fn execute(config: &ChimeraConfig, no_v3_engine: bool, protocol: bool) -> Result<()> {
    let ctx = crate::composition::build(config)?;
    execute_with_ctx(&ctx, config, no_v3_engine, protocol).await
}

/// tui 命令主体 — 复用 dispatch 级共享装配 AppContext（M9 装配收敛）
///
/// `config`：已加载的合并配置。当前消费 `enable_strategy_cap`（经验闭环
/// 策略封顶守卫开关，§16.1 装配参数）；装配面（bus）一律来自 `ctx`。
///
/// `no_v3_engine`：来自 CLI `--no-v3-engine` flag，true 时设置
/// `CHIMERA_NO_V3_ENGINE=1` 环境变量，使 `TuiApp::render` 走 ratatui 回退路径。
///
/// `protocol`：WI-01 协议模式开关（Quest 生命周期经 AppOp/AppEvent 协议面驱动）。
///
/// # 迁移要点（M9）
/// - `bus` 一律来自 `ctx`（dispatch 级 C12 装配），tui 臂不再自建总线；
///   协议/非协议两分支及下游全部组件（EventSubscriber/编排器/经验闭环/
///   握手/Action 编排/超窗桥）共享同一 bus，事件对进程内订阅者全量可见。
/// - Quest 引擎保留臂内构造（[`assemble_quest_stack`]），检查点能力零变化。
/// - 对外行为/输出/退出码与迁移前一致。
pub async fn execute_with_ctx(
    ctx: &AppContext,
    config: &ChimeraConfig,
    no_v3_engine: bool,
    protocol: bool,
) -> Result<()> {
    // v3-engine M2(ADR-061):CLI flag 优先,设置 env var 让 TuiApp 在渲染时
    // 通过 `v3_engine_disabled_by_env()` 检测到回退意图。WHY env var 而非直接
    // 传参:TuiApp 已封装好双路径分发,env var 是最小侵入式回退通道,且支持
    // 不修改 CLI 时通过环境变量回退(CI/运维场景友好)。
    if no_v3_engine {
        std::env::set_var("CHIMERA_NO_V3_ENGINE", "1");
        tracing::info!("v3-engine 已通过 --no-v3-engine flag 禁用,回退到 ratatui 路径");
    } else {
        tracing::info!("启动 TUI 交互界面(v3-engine 默认启用)");
    }

    // M9: bus 取自 dispatch 级共享装配 AppContext(C12),tui 臂不再自建总线;
    // Quest 编排器/经验闭环/Action 编排等均订阅此同一共享总线,
    // TUI ↔ 各后台组件的事件回环对进程内全部订阅者可见。
    let bus = ctx.bus.clone();
    // EventSubscriber::new 内部先同步 subscribe，再 spawn 后台转发任务，
    // 遵循 subscribe-before-spawn 规则(§4.4 反模式 #3)。
    let subscriber = chimera_tui::EventSubscriber::new(bus.clone());

    // 加载 TUI 专用持久化配置(~/.chimera/tui.yaml)
    // WHY 必须在 DataPipeline 构造前加载: `DataSourceConfig::from_tui_config`
    // 需读取 tui_config.tick_interval_ms(P1 tick 配置修复,ADR-072;原实现用
    // DataSourceConfig::default() 导致该配置生产断线)。
    // (theme/colors/main_panel_ratio/tick_interval_ms),覆盖默认值;
    // 文件不存在时 load_from_file 静默返回默认配置(首次启动场景)。
    let tui_config = {
        let tui_path = chimera_tui::TuiConfig::default_path();
        match chimera_tui::TuiConfig::load_from_file(&tui_path) {
            Ok(persisted) => {
                tracing::debug!(
                    path = %tui_path.display(),
                    "Loaded persisted TuiConfig"
                );
                persisted
            }
            Err(e) => {
                tracing::warn!(
                    path = %tui_path.display(),
                    error = %e,
                    "Failed to load TuiConfig, using defaults"
                );
                chimera_tui::TuiConfig::default()
            }
        }
    };

    // Concord T1.5(P4① 接线):TuiBible 四源 Figment 合并(默认→
    // ~/.chimera/tui_bible.yaml→CHIMERA_BIBLE_* 环境变量→CLI),在 TuiConfig
    // 持久化加载之后、DataPipeline/TuiApp 构造之前应用,使下游全部消费
    // 合并后的配置。WHY 损坏文件回退默认而非阻断启动:TuiBible 是体验增强
    // 配置,不应成为启动单点故障;回退经 warn 日志可观测(错误处理准则)。
    let tui_config = {
        let mut cfg = tui_config;
        match chimera_tui::TuiBible::load() {
            Ok(bible) => {
                cfg.apply_bible_overrides(&bible);
                tracing::info!(
                    theme = ?bible.theme,
                    key_bindings = bible.key_bindings.len(),
                    "TuiBible 四源合并完成并已应用到 TuiConfig"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "TuiBible 加载失败(文件损坏或环境变量非法),回退默认配置"
                );
            }
        }
        cfg
    };
    // B2:策展配置在 tui_config 被 move 进 TuiApp 前提取(编排器 compact 消费)
    let curation_cfg = tui_config.curation.clone();

    // WI-01 协议模式: TUI 数据层经 AppOp/AppEvent 协议面与核心交互
    // (核心-表面分离 dogfooding——Quest 生命周期走协议面,其他面板默认空;
    // A1 双跑窗口过渡态,直联路径(DataPipeline)保留)
    if protocol {
        tracing::info!("TUI 协议模式: Quest 生命周期经 AppOp/AppEvent 协议面驱动");
        let mut protocol_ds = chimera_tui::ProtocolDataSource::new(
            chimera_tui::DataSourceConfig::from_tui_config(&tui_config),
        );
        protocol_ds
            .start_session("tui-protocol", "run-1")
            .await
            .context("协议会话启动失败")?;
        let mut app = chimera_tui::TuiApp::with_data_source(tui_config, Box::new(protocol_ds))
            .context("TUI 初始化失败")?;
        app = chimera_tui::TuiApp::with_event_bus(app, bus.clone());

        // Quest 编排器保留(EventBus 双向控制: TUI ↔ 编排器回环)
        let engine = Arc::new(quest_engine::QuestEngine::new(bus.clone()));
        let control_handle =
            quest_engine::spawn_control_subscriber(Arc::clone(&engine), bus.clone());
        let quest_handle = crate::orchestrator::spawn_quest_orchestrator(
            bus.clone(),
            Arc::clone(&engine),
            orchestrator_config_from_env(),
        );

        let run_result = app.run().context("TUI 协议模式运行失败");
        control_handle.abort();
        quest_handle.abort();
        tracing::info!("TUI 协议模式退出");
        return run_result;
    }

    // 构建数据管道：将事件聚合为 TUI 可消费的统一快照。
    // tick 间隔来自持久化 TuiConfig(修复 F-4:SetTickInterval 持久化后
    // 下次启动经 from_tui_config 生效;运行时改值见 event_loop 提示语义)。
    // Concord T1.6(P4②):接入指标历史持久化层——打开失败(磁盘/权限)时
    // 降级为无持久化管道并 warn,不阻断启动(错误处理准则)。
    let pipeline = match chimera_tui::MetricsHistory::open_default().await {
        Ok(history) => {
            tracing::debug!(
                path = %history.db_path().display(),
                "MetricsHistory 已接线到 DataPipeline(慢同步 1s + 回填 30s)"
            );
            Arc::new(chimera_tui::DataPipeline::new_with_history(
                subscriber,
                chimera_tui::DataSourceConfig::from_tui_config(&tui_config),
                Arc::new(history),
            ))
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "MetricsHistory 打开失败,降级为无持久化管道(趋势图无跨重启回填)"
            );
            Arc::new(chimera_tui::DataPipeline::new(
                subscriber,
                chimera_tui::DataSourceConfig::from_tui_config(&tui_config),
            ))
        }
    };

    // 创建 TUI 应用，使用实时数据管道而非空桩。
    let mut app =
        chimera_tui::TuiApp::with_data_source(tui_config, Box::new(Arc::clone(&pipeline)))
            .context("TUI 初始化失败")?;

    // M4:将 EventBus 注入 TUI,使控制面板可发布请求事件。
    // 保留 bus 所有权,后续仍需要克隆给上游控制订阅者。
    app = chimera_tui::TuiApp::with_event_bus(app, bus.clone());

    // M4 review fix:启动 quest-engine 控制事件订阅者,
    // 消费 TUI 发布的 QuestPauseRequested/QuestResumeRequested,
    // 形成 TUI → EventBus → 上游处理 → 状态反馈的端到端路径。
    // B1(2026-09-06 复评):启用检查点管理器 —— `/quest checkpoint` 编排命令
    // 依赖 engine.save_checkpoint;目录与 tui.yaml 同根(~/.chimera/checkpoints)。
    // M9:引擎/订阅者改经 assemble_quest_stack 从共享 ctx 装配(见该函数 WHY)。
    let checkpoint_dir = chimera_tui::TuiConfig::default_path()
        .parent()
        .map(|p| p.join("checkpoints"))
        .unwrap_or_else(|| std::path::PathBuf::from(".chimera/checkpoints"));
    let (engine, control_handle) = assemble_quest_stack(ctx, checkpoint_dir);

    // Quest 分解管线:启动 Quest 编排器,消费 TUI 发布的 TuiChatSubmitted,经真实 L9
    // QuestEngine 分解为任务 DAG 并批聚合流式回发(H-a)。复用上方 engine(与控制订阅者
    // 共享),create_quest 内部广播的 QuestCreated 经同一 bus 同步点亮 Quest 面板。
    // 节奏与批大小经 orchestrator_config_from_env() 构造(打字机延迟可经
    // CHIMERA_TUI_CHUNK_DELAY_MS 关闭)。
    let quest_handle = crate::orchestrator::spawn_quest_orchestrator(
        bus.clone(),
        Arc::clone(&engine),
        orchestrator_config_from_env(),
    );

    // §16.1 经验卡片闭环装配(Phase 10 审计修复 Wave 1):组合根接线
    // ExperienceCardBus 主链 — L3 SQLite 双流持久化(含 Critical 高分卡) +
    // L2 MlcEngine 卡片消费 + L6 算子反馈回流 + RuntimeAuditor 五维报告
    // 周期发布(打通 SelfAssessmentPanel) + 协调度量订阅器。
    // WHY 失败不阻断启动:闭环装配是增强链路,降级后 TUI 核心交互仍可用;
    // 失败经 warn 日志可观测(与 TuiBible 回退同款错误处理准则)。
    // WHY 下划线前缀持有:绑定存活至函数结束,保持后台任务与 Arc 句柄生命周期。
    let experience_loop_handles = match crate::experience_loop::spawn_experience_loop(
        bus.clone(),
        Arc::clone(&engine),
        config.enable_strategy_cap,
    )
    .await
    {
        Ok(handles) => Some(handles),
        Err(e) => {
            tracing::warn!(error = %e, "经验卡片闭环装配失败,降级运行(闭环不可用)");
            None
        }
    };

    // FC-05(2026-09-06 评估):经验卡片可视化面板接线 —— 组合根注入 L2
    // MlcEngine 卡片系统统计(运行期真实数据,不再恒显 "Awaiting")。
    // 闭环装配失败(handles = None)时面板维持默认未接线状态(诚实提示),
    // 与经验闭环降级策略一致,不阻断启动。
    if let Some(handles) = &experience_loop_handles {
        app.install_experience_card_provider(Arc::new(MlcCardStatsProvider(Arc::clone(
            &handles.mlc,
        ))));
    }

    // §16.1 L9 组件装配(Phase 10 审计修复 Wave 2):
    // 1. Ambient Mode 后台常驻订阅器(资源看门狗/记忆整理/检查点调度,
    //    BudgetExceeded/ResourceRecovered/CheckpointSaved 双通道;NoopTidyHook
    //    默认——真实记忆整理由 mlc-engine 接线方注入,依赖倒置先例)。
    let ambient_handle = quest_engine::spawn_ambient_subscriber(
        bus.clone(),
        Arc::clone(&engine),
        quest_engine::AmbientModeConfig::default(),
        Arc::new(quest_engine::NoopTidyHook),
    );
    // 2. Quest 生命周期组件桥:QuestCreated/Progress/Completed 事件驱动
    //    LongTaskMap + SearchTreeManager + LongTermCreditAssigner 真实运行。
    let _quest_loop =
        crate::quest_loop::spawn_quest_lifecycle_bridge(bus.clone(), Arc::clone(&engine));
    // WHY 持有:ambient_handle/quest_loop 绑定存活至函数结束(后台任务生命周期)。
    let _ambient_handle = ambient_handle;

    // Concord W10 T10.2(ADR-082):启动协议握手应答器 — 响应 TUI 启动时
    // 发布的 TuiHello,协商兼容级别并回 TuiHelloAck(SEC-4 一次性);
    // 必须在 TUI run() 前 spawn(subscribe-before-spawn,不错过启动瞬间握手)。
    let handshake_handle = crate::handshake::spawn_handshake_responder(bus.clone());

    // P1(ADR-072):构造超窗兜底桥并注入 Action 编排器。
    // 桥挂 TUI 会话总线——触发时发布 OverWindowFallbackTriggered,经 subscriber
    // → pipeline 进入 latest_events,由 OverWindow 面板结构化展示(闭环断点 F-3 修复)。
    // WHY 会话级桥而非全局共享总线:保持 TUI 会话隔离(总线共享见 ADR-072 结论,
    // 避免大爆炸式改造);桥的 provider 闭包由 overwindow_bridge 内部组装。
    let overwindow_bridge =
        Arc::new(OverWindowBridge::new(bus.clone()).context("OverWindowBridge 初始化失败")?);
    // 会话语料提供者 = Chat 消息 + Quest 标题(pipeline 快照派生;空语料时
    // overwindow.run 由编排器明确失败,不空跑)。
    let pipeline_for_corpus = Arc::clone(&pipeline);
    let overwindow = OverWindowHandle::new(
        Arc::clone(&overwindow_bridge),
        Arc::new(move || {
            chimera_tui::TuiDataSource::snapshot(&*pipeline_for_corpus)
                .ok()
                .map(|snapshot| {
                    let mut corpus = String::new();
                    for msg in &snapshot.chat_messages {
                        corpus.push_str(&msg.content);
                        corpus.push('\n');
                    }
                    for quest in &snapshot.quest_list {
                        corpus.push_str(&quest.title);
                        corpus.push('\n');
                    }
                    corpus
                })
                .unwrap_or_default()
        }),
    );

    // FC-2(ADR-081):/compact 策展的结构化会话消息提供者 —— 与超窗语料闭包
    // 同源(DataPipeline 快照),但保留 ChatMessage 角色(Pinned 保护段判定依赖),
    // 供编排器 compact 分支执行 RuleCurationPolicy 并回写历史。
    let pipeline_for_compact = Arc::clone(&pipeline);
    let chat_provider: crate::action_orchestrator::ChatMessagesProvider = Arc::new(move || {
        chimera_tui::TuiDataSource::snapshot(&*pipeline_for_compact)
            .map(|snapshot| snapshot.chat_messages.clone())
            .unwrap_or_default()
    });

    // P0 交互链:启动 Action 编排器,消费命令面板/斜杠/面板派发的 TuiActionRequested,
    // 按 action_id 域前缀路由:quest.* 驱动同一 engine 真实执行,回发 TuiActionCompleted/Failed。
    // UI 本地态动作由 TUI 本地 dispatch_action 处理,不到达此处(误达则回 Failed)。
    let action_handle = crate::action_orchestrator::spawn_action_orchestrator(
        bus.clone(),
        Arc::clone(&engine),
        Some(overwindow),
        Some(chat_provider),
        // B2:/compact 策展配置随 TuiConfig 四源合并后透传编排器
        curation_cfg,
    );

    // 启动 TUI 事件循环(阻塞直到用户退出)
    // WHY 先保存结果再 shutdown:即使 run() 返回 Err,也必须清理 DataPipeline
    // 后台任务,避免 orphan task(§4.4 反模式 #7)。
    let run_result = app.run().context("TUI 运行失败");

    // 中止上游控制订阅者;EventBus 仍由 pipeline 等持有,不会提前关闭。
    control_handle.abort();
    // 中止 Quest 编排器后台任务(避免 orphan task,§4.4 #7)。
    quest_handle.abort();
    // 中止握手应答器后台任务(Concord W10,避免 orphan task)。
    handshake_handle.abort();
    // 中止 Action 编排器后台任务(避免 orphan task,§4.4 #7)。
    action_handle.abort();

    // 中止并清理数据管道后台任务。
    pipeline.shutdown().await;

    tracing::info!("TUI 已退出");
    run_result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composition;
    use event_bus::NexusEvent;
    use nexus_core::{MultimodalInput, UserIntent};
    use uuid::Uuid;

    /// 构造最小 UserIntent(与 run.rs 测试同形,仅驱动引擎发布事件)
    fn test_intent(text: &str) -> UserIntent {
        UserIntent {
            intent_id: format!("intent-{}", Uuid::now_v7()),
            raw_text: text.to_string(),
            multimodal_inputs: vec![MultimodalInput::Text(text.to_string())],
            risk_level: 0,
        }
    }

    /// M9 装配收敛:tui 臂 Quest 栈事件经 dispatch 共享 ctx.bus 对外部订阅者可见
    /// (M4 先例 run_events_visible_on_shared_context_bus 的 tui 臂版本)。
    /// 迁移前 tui 臂自建私有总线,QuestCreated 等事件对进程内订阅者零可见;
    /// 迁移后 engine 经 [`assemble_quest_stack`] 挂在共享 bus 上,事件流全量可见。
    /// 此测试先行失败(红):assemble_quest_stack 尚不存在。
    #[tokio::test]
    async fn tui_events_visible_on_shared_context_bus() {
        let ctx = composition::build(&ChimeraConfig::default()).expect("装配应成功");
        // §4.4 反模式 3:先 subscribe 再驱动 tui 臂装配,否则事件静默丢失
        let mut rx = ctx.bus.subscribe();
        let tmp = tempfile::tempdir().expect("临时检查点目录应可创建");
        let (engine, control_handle) = assemble_quest_stack(&ctx, tmp.path().to_path_buf());
        engine
            .create_quest(test_intent("评审增量验证:分解并总结"))
            .await
            .expect("Quest 分解应成功");
        let mut saw_quest_created = false;
        // 超时轮询:事件已进 broadcast 缓冲,等待接收即可
        for _ in 0..40 {
            match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
                Ok(Ok(NexusEvent::QuestCreated { .. })) => {
                    saw_quest_created = true;
                    break;
                }
                Ok(Ok(_)) => continue, // 其他事件跳过
                _ => break,
            }
        }
        control_handle.abort();
        assert!(
            saw_quest_created,
            "QuestCreated 应在组合根共享 bus 上可见(tui 臂不再自建私有 bus,M9)"
        );
    }

    /// M9 零变化回归:B1(2026-09-06 复评)要求 tui 臂引擎保留检查点能力——
    /// `/quest checkpoint` 编排命令依赖 `engine.save_checkpoint`。
    /// 若误用组合根裸 engine(`QuestEngine::new`,无 CheckpointManager),
    /// save_checkpoint 将以 "checkpoints disabled" 失败;本测试锁死迁移前后
    /// 均为 with_checkpoints 构造的行为等价性。同时断言 CheckpointSaved(Critical 级)
    /// 同经共享 bus 可见,验证检查点事件流也不落私有总线。
    /// 此测试先行失败(红):assemble_quest_stack 尚不存在。
    #[tokio::test]
    async fn tui_quest_stack_preserves_checkpoint_capability() {
        let ctx = composition::build(&ChimeraConfig::default()).expect("装配应成功");
        let mut rx = ctx.bus.subscribe();
        let tmp = tempfile::tempdir().expect("临时检查点目录应可创建");
        let (engine, control_handle) = assemble_quest_stack(&ctx, tmp.path().to_path_buf());
        let quest = engine
            .create_quest(test_intent("检查点能力零变化回归"))
            .await
            .expect("Quest 分解应成功");
        let checkpoint = engine
            .save_checkpoint(&quest.quest_id)
            .await
            .expect("检查点保存应成功(B1 能力须在迁移后保留)");
        let mut saw_checkpoint_saved = false;
        for _ in 0..40 {
            match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
                Ok(Ok(NexusEvent::CheckpointSaved { .. })) => {
                    saw_checkpoint_saved = true;
                    break;
                }
                Ok(Ok(_)) => continue,
                _ => break,
            }
        }
        control_handle.abort();
        assert!(!checkpoint.checkpoint_id.is_empty());
        assert!(
            saw_checkpoint_saved,
            "CheckpointSaved 应在组合根共享 bus 上可见(检查点事件流不落私有总线)"
        );
    }

    /// M9 源码守卫:tui.rs **生产代码段**(#[cfg(test)] 模块之前、剥除行注释后)
    /// 不得再出现自建总线装配点 —— 将"收敛后生产代码自建总线清零"的验收口径
    /// 固化为可持续回归的测试(扫描器先例:chimera-tui event_coverage_drift_test)。
    /// WHY 剥注释:注释里的历史提及不算装配点;WHY 截断 #[cfg(test)]:
    /// 测试代码自身会引用被禁字面量做断言,须排除在扫描面之外。
    /// WHY concat! 拼接被禁字面量:使对本文件的朴素 grep 零命中,
    /// 与"生产代码自建总线清零"的验收口径完全一致。
    #[test]
    fn tui_production_code_has_no_private_event_bus() {
        let forbidden = concat!("EventBus", "::new()");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/tui.rs");
        let text = std::fs::read_to_string(&path).expect("tui.rs 应可读");
        let production = text
            .split("#[cfg(test)]")
            .next()
            .expect("应存在 #[cfg(test)] 测试模块分隔");
        let code_only: String = production
            .lines()
            .map(|line| match line.find("//") {
                Some(i) => &line[..i],
                None => line,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code_only.contains(forbidden),
            "tui 臂生产代码不得自建 EventBus(M9 装配收敛:bus 一律来自 dispatch 共享 AppContext)"
        );
    }
}
