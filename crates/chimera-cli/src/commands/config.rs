//! `chimera config <action>` — 配置管理子命令
//!
//! 支持:
//! - `init`:生成默认 omega.yaml
//! - `list`:列出当前生效配置项
//! - `show`:显示完整配置(JSON)
//! - `path`:显示配置文件路径
//!
//! v2.9.0-omega Task 1.7:支持 `--json` flag 输出结构化 JSON(envelope schema)

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::cli::ConfigAction;
use crate::config::{self, ChimeraConfig};
use crate::output;

/// 执行 config 子命令
///
/// 注:参数命名为 `cfg` 而非 `config`,避免与 `use crate::config` 引入的模块别名遮蔽。
///
/// `json` flag(Task 1.7)控制输出格式:
/// - `true`:输出 envelope schema JSON(`{ "status": "ok", "data": <payload> }`)
/// - `false`:输出人类可读格式
pub async fn execute(action: &ConfigAction, cfg: &ChimeraConfig, json: bool) -> Result<()> {
    tracing::info!(?action, json, "配置管理操作");
    match action {
        ConfigAction::Init => {
            let path = config::default_config_path();
            init_config(&path, json)?;
        }
        ConfigAction::List => {
            list_config(cfg, json);
        }
        ConfigAction::Show => {
            show_config(cfg, json)?;
        }
        ConfigAction::Path => {
            let path = config::default_config_path();
            if json {
                let payload = serde_json::json!({ "path": path.display().to_string() });
                output::print_json(&payload).context("序列化 config path 为 JSON 失败")?;
            } else {
                println!("{}", path.display());
            }
        }
    }
    Ok(())
}

/// `config init` 的输出载荷(Task 1.7 JSON schema)
#[derive(Debug, Serialize)]
struct InitPayload<'a> {
    path: &'a str,
    created: bool,
}

/// 生成默认配置文件
fn init_config(path: &Path, json: bool) -> Result<()> {
    config::init_config_file(path)
        .with_context(|| format!("生成配置文件失败:{}", path.display()))?;
    let path_str = path.display().to_string();
    if json {
        let payload = InitPayload {
            path: &path_str,
            created: true,
        };
        output::print_json(&payload).context("序列化 config init 为 JSON 失败")?;
    } else {
        output::print_success(&format!("已生成默认配置:{}", path_str));
        println!("编辑该文件以自定义 NEXUS-OMEGA 行为");
    }
    Ok(())
}

/// `config list` 的输出载荷(Task 1.7 JSON schema)
#[derive(Debug, Serialize)]
struct ListPayload<'a> {
    nexus_version: &'a str,
    quest_auto_decompose: bool,
    quest_max_tasks_per_quest: u32,
    thinking_toggle_default_mode: &'a str,
    model_router_strategy: &'a str,
    model_router_budget_daily_usd: f64,
    osa_sparsity_base: f64,
    seccore_sandbox: &'a str,
    seccore_command_interpolation: &'a str,
    evolution_enabled: bool,
    monitoring_prometheus_enabled: bool,
}

/// 列出当前生效的关键配置项
///
/// `json=true` 时输出结构化 JSON,否则输出键值对表格(使用 output helper 统一前缀)
fn list_config(cfg: &ChimeraConfig, json: bool) {
    if json {
        let payload = ListPayload {
            nexus_version: &cfg.nexus.version,
            quest_auto_decompose: cfg.quest.auto_decompose,
            quest_max_tasks_per_quest: cfg.quest.max_tasks_per_quest,
            thinking_toggle_default_mode: &cfg.thinking_toggle.default_mode,
            model_router_strategy: &cfg.model_router.strategy,
            model_router_budget_daily_usd: cfg.model_router.budget.daily_usd,
            osa_sparsity_base: cfg.osa.sparsity_base,
            seccore_sandbox: &cfg.seccore.sandbox,
            seccore_command_interpolation: &cfg.seccore.command_interpolation,
            evolution_enabled: cfg.evolution.enabled,
            monitoring_prometheus_enabled: cfg.monitoring.prometheus.enabled,
        };
        // print_json 内部已处理序列化错误(返回 Result),此处 unwrap 安全因
        // ListPayload 全为基本类型,序列化不可能失败
        let _ = output::print_json(&payload);
    } else {
        // 人类可读:用表格渲染键值对(Task 1.12)
        let rows = vec![
            vec!["nexus.version".into(), cfg.nexus.version.clone()],
            vec![
                "quest.auto_decompose".into(),
                cfg.quest.auto_decompose.to_string(),
            ],
            vec![
                "quest.max_tasks_per_quest".into(),
                cfg.quest.max_tasks_per_quest.to_string(),
            ],
            vec![
                "thinking_toggle.default_mode".into(),
                cfg.thinking_toggle.default_mode.clone(),
            ],
            vec![
                "model_router.strategy".into(),
                cfg.model_router.strategy.clone(),
            ],
            vec![
                "model_router.budget.daily_usd".into(),
                cfg.model_router.budget.daily_usd.to_string(),
            ],
            vec![
                "osa.sparsity_base".into(),
                cfg.osa.sparsity_base.to_string(),
            ],
            vec!["seccore.sandbox".into(), cfg.seccore.sandbox.clone()],
            vec![
                "seccore.command_interpolation".into(),
                cfg.seccore.command_interpolation.clone(),
            ],
            vec![
                "evolution.enabled".into(),
                cfg.evolution.enabled.to_string(),
            ],
            vec![
                "monitoring.prometheus.enabled".into(),
                cfg.monitoring.prometheus.enabled.to_string(),
            ],
        ];
        output::print_info("当前生效配置:");
        output::print_table(&["Key", "Value"], &rows);
    }
}

/// 以 JSON 格式显示完整配置
///
/// `json=true` 时输出 envelope schema(包装完整 ChimeraConfig),
/// `json=false` 时仍输出原始 JSON(向后兼容 v2.8.0 行为,便于脚本消费)
fn show_config(cfg: &ChimeraConfig, json: bool) -> Result<()> {
    if json {
        // envelope schema:完整配置作为 data 字段
        output::print_json(cfg).context("序列化配置为 JSON 失败")?;
    } else {
        // 人类可读模式:直接 pretty-print JSON(保留 v2.8.0 行为)
        let json = serde_json::to_string_pretty(cfg).context("序列化配置为 JSON 失败")?;
        println!("{}", json);
    }
    Ok(())
}
