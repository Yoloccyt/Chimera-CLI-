//! `chimera tui` — TUI 交互界面
//!
//! 调用 `chimera-tui` crate 启动 ratatui 终端界面。
//! 生产环境通过 EventBus 订阅实时数据，替代默认的 StubDataSource。

use std::sync::Arc;

use anyhow::{Context, Result};

use crate::config::ChimeraConfig;

/// 执行 tui 命令
pub async fn execute(_config: &ChimeraConfig) -> Result<()> {
    tracing::info!("启动 TUI 交互界面");

    // M0: 为当前 TUI 会话创建本地事件总线；真正的全系统 EventBus 共享将在后续里程碑接入。
    // EventSubscriber::new 内部先同步 subscribe，再 spawn 后台转发任务，
    // 遵循 subscribe-before-spawn 规则(§4.4 反模式 #3)。
    let bus = event_bus::EventBus::new();
    let subscriber = chimera_tui::EventSubscriber::new(bus.clone());

    // 构建数据管道：将事件聚合为 TUI 可消费的统一快照。
    let pipeline = Arc::new(chimera_tui::DataPipeline::new(
        subscriber,
        chimera_tui::DataSourceConfig::default(),
    ));

    // 加载 TUI 专用持久化配置(~/.chimera/tui.yaml)
    // WHY 在 TuiApp 构造前加载: tui.yaml 持久化 TUI 专用字段
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

    // 启动 TUI 事件循环(阻塞直到用户退出)
    // WHY 先保存结果再 shutdown:即使 run() 返回 Err,也必须清理 DataPipeline
    // 后台任务,避免 orphan task(§4.4 反模式 #7)。
    let run_result = app.run().context("TUI 运行失败");

    // 中止上游控制订阅者;EventBus 仍由 pipeline 等持有,不会提前关闭。
    control_handle.abort();

    // 中止并清理数据管道后台任务。
    pipeline.shutdown().await;

    tracing::info!("TUI 已退出");
    run_result
}
