//! `aether config <action>` — 配置管理子命令
//!
//! 支持:
//! - `init`:生成默认 omega.yaml
//! - `list`:列出当前生效配置项
//! - `show`:显示完整配置(JSON)
//! - `path`:显示配置文件路径

use std::path::Path;

use anyhow::{Context, Result};

use crate::cli::ConfigAction;
use crate::config::{self, ChimeraConfig};

/// 执行 config 子命令
///
/// 注:参数命名为 `cfg` 而非 `config`,避免与 `use crate::config` 引入的模块别名遮蔽。
pub async fn execute(action: &ConfigAction, cfg: &ChimeraConfig) -> Result<()> {
    tracing::info!(?action, "配置管理操作");
    match action {
        ConfigAction::Init => {
            let path = config::default_config_path();
            init_config(&path)?;
        }
        ConfigAction::List => {
            list_config(cfg);
        }
        ConfigAction::Show => {
            show_config(cfg)?;
        }
        ConfigAction::Path => {
            let path = config::default_config_path();
            println!("{}", path.display());
        }
    }
    Ok(())
}

/// 生成默认配置文件
fn init_config(path: &Path) -> Result<()> {
    config::init_config_file(path)
        .with_context(|| format!("生成配置文件失败:{}", path.display()))?;
    println!("[config init] 已生成默认配置:{}", path.display());
    println!("[config init] 编辑该文件以自定义 NEXUS-OMEGA 行为");
    Ok(())
}

/// 列出当前生效的关键配置项(键值对形式)
fn list_config(cfg: &ChimeraConfig) {
    println!("[config list] 当前生效配置:");
    println!("  nexus.version = {}", cfg.nexus.version);
    println!("  quest.auto_decompose = {}", cfg.quest.auto_decompose);
    println!(
        "  quest.max_tasks_per_quest = {}",
        cfg.quest.max_tasks_per_quest
    );
    println!(
        "  thinking_toggle.default_mode = {}",
        cfg.thinking_toggle.default_mode
    );
    println!("  model_router.strategy = {}", cfg.model_router.strategy);
    println!(
        "  model_router.budget.daily_usd = {}",
        cfg.model_router.budget.daily_usd
    );
    println!("  osa.sparsity_base = {}", cfg.osa.sparsity_base);
    println!("  seccore.sandbox = {}", cfg.seccore.sandbox);
    println!(
        "  seccore.command_interpolation = {}",
        cfg.seccore.command_interpolation
    );
    println!("  evolution.enabled = {}", cfg.evolution.enabled);
    println!(
        "  monitoring.prometheus.enabled = {}",
        cfg.monitoring.prometheus.enabled
    );
}

/// 以 JSON 格式显示完整配置(便于脚本消费)
fn show_config(cfg: &ChimeraConfig) -> Result<()> {
    let json = serde_json::to_string_pretty(cfg).context("序列化配置为 JSON 失败")?;
    println!("{}", json);
    Ok(())
}
