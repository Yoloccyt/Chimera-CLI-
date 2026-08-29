//! `chimera tui` — TUI 交互界面
//!
//! 调用 `chimera-tui` crate 启动 ratatui 终端界面。
//! 生产环境通过 EventBus 订阅实时数据，替代默认的 StubDataSource。
//!
//! # v3-engine M2 切换(ADR-061)
//! 自研渲染路径默认启用,通过 `--no-v3-engine` flag 或 `CHIMERA_NO_V3_ENGINE=1`
//! 环境变量可回退到 ratatui 路径。回退机制保留 2 个版本周期(v2.11.0-omega 移除)。
//!
//! # 超窗/RAG 链路生产接线(P1,ADR-072)
//! `execute` 组合根创建 `OverWindowBridge`(挂 TUI 会话总线)并注入
//! `OverWindowHandle`(桥 + 会话语料提供者)给 Action 编排器;`overwindow.run`
//! 经 `TuiActionRequested` 协议真实执行两级检索,触发时发布
//! `OverWindowFallbackTriggered` → EventSubscriber → DataPipeline → latest_events
//! → OverWindow 面板结构化展示(零管道侵入)。

use std::sync::Arc;

use anyhow::{Context, Result};

use crate::action_orchestrator::OverWindowHandle;
use crate::config::ChimeraConfig;
use crate::overwindow_bridge::OverWindowBridge;

/// 执行 tui 命令
///
/// `no_v3_engine`:来自 CLI `--no-v3-engine` flag,true 时设置
/// `CHIMERA_NO_V3_ENGINE=1` 环境变量,使 `TuiApp::render` 走 ratatui 回退路径。
pub async fn execute(_config: &ChimeraConfig, no_v3_engine: bool, protocol: bool) -> Result<()> {
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

    // M0: 为当前 TUI 会话创建本地事件总线;Quest 编排器亦订阅此同一总线,
    // 形成 TUI ↔ 编排器的对话事件回环(全系统 EventBus 共享仍待后续里程碑)。
    // EventSubscriber::new 内部先同步 subscribe，再 spawn 后台转发任务，
    // 遵循 subscribe-before-spawn 规则(§4.4 反模式 #3)。
    let bus = event_bus::EventBus::new();
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
            crate::orchestrator::OrchestratorConfig::default(),
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
    // 这里使用最小化的 QuestEngine 实例(仅支持控制订阅演示)。
    let engine = Arc::new(quest_engine::QuestEngine::new(bus.clone()));
    let control_handle = quest_engine::spawn_control_subscriber(Arc::clone(&engine), bus.clone());

    // Quest 分解管线:启动 Quest 编排器,消费 TUI 发布的 TuiChatSubmitted,经真实 L9
    // QuestEngine 分解为任务 DAG 并逐字符流式回发。复用上方 engine(与控制订阅者共享),
    // create_quest 内部广播的 QuestCreated 经同一 bus 同步点亮 Quest 面板。
    let quest_handle = crate::orchestrator::spawn_quest_orchestrator(
        bus.clone(),
        Arc::clone(&engine),
        crate::orchestrator::OrchestratorConfig::default(),
    );

    // §16.1 经验卡片闭环装配(Phase 10 审计修复 Wave 1):组合根接线
    // ExperienceCardBus 主链 — L3 SQLite 双流持久化(含 Critical 高分卡) +
    // L2 MlcEngine 卡片消费 + L6 算子反馈回流 + RuntimeAuditor 五维报告
    // 周期发布(打通 SelfAssessmentPanel) + 协调度量订阅器。
    // WHY 失败不阻断启动:闭环装配是增强链路,降级后 TUI 核心交互仍可用;
    // 失败经 warn 日志可观测(与 TuiBible 回退同款错误处理准则)。
    // WHY 下划线前缀持有:绑定存活至函数结束,保持后台任务与 Arc 句柄生命周期。
    let _experience_loop =
        match crate::experience_loop::spawn_experience_loop(bus.clone(), Arc::clone(&engine)).await
        {
            Ok(handles) => Some(handles),
            Err(e) => {
                tracing::warn!(error = %e, "经验卡片闭环装配失败,降级运行(闭环不可用)");
                None
            }
        };

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

    // P0 交互链:启动 Action 编排器,消费命令面板/斜杠/面板派发的 TuiActionRequested,
    // 按 action_id 域前缀路由:quest.* 驱动同一 engine 真实执行,回发 TuiActionCompleted/Failed。
    // UI 本地态动作由 TUI 本地 dispatch_action 处理,不到达此处(误达则回 Failed)。
    let action_handle = crate::action_orchestrator::spawn_action_orchestrator(
        bus.clone(),
        Arc::clone(&engine),
        Some(overwindow),
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
