//! `aether tui` — TUI 交互界面
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

    // 创建 TUI 应用，使用实时数据管道而非空桩。
    let mut app = chimera_tui::TuiApp::with_data_source(
        chimera_tui::TuiConfig::default(),
        Box::new(Arc::clone(&pipeline)),
    )
    .context("TUI 初始化失败")?;

    // 启动 TUI 事件循环(阻塞直到用户退出)
    app.run().context("TUI 运行失败")?;

    // 中止并清理数据管道后台任务。
    pipeline.shutdown().await;

    tracing::info!("TUI 已退出");
    Ok(())
}
