//! `chimera llm <action>` — LLM Provider 管理子命令(Task 2 of spec)
//!
//! 封装 `mca-gateway` 与 `model-router`,提供 6 个子动作:
//! - `list`:列出已配置 Provider
//! - `show <name>`:显示 Provider 详情
//! - `set-default <name>`:设置默认 Provider(触发 permission prompt)
//! - `test [name]`:连通性探测
//! - `channels`:列出 model-router 4 路由渠道
//! - `strategy [name]`:显示/设置 model-router 策略(触发 permission prompt)
//!
//! # 骨架声明(Task 2 范围)
//!
//! 本 Task **不**实装真实 `mca-gateway` / `model-router` API,
//! 仅搭骨架:硬编码 Provider 列表、Channels 表格、Test 模拟 1s 延迟。
//! 真实接入计划见后续 Task(预计通过 `ChimeraConfig::set_llm_default` 等接口持久化)。
//!
//! # 与现有配置的关系
//!
//! `ChimeraConfig` 当前无 `llm` 顶层 section(预计后续 Task 引入),
//! 本 Task 复用 `model_router.providers` / `model_router.strategy` 作为数据源;
//! 若 `model_router.providers` 为空,降级到 8 个内置 default 名
//! (deepseek / zhipu / minimax / volcano / moonshot / stepfun / alicloud / custom)。
//!
//! # v2.9.0-omega 全局 flag 支持
//!
//! - `--json` (Task 1.7):`true` 时各子动作输出 envelope JSON
//! - `PermissionCtx` (Task 1.11):`set-default` / `strategy Some(_)` 触发 prompt
//!   (除非 `--yes` / `--no-permission`)

#![forbid(unsafe_code)]

use std::time::Instant;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::cli::LlmAction;
use crate::config::ChimeraConfig;
use crate::error::ChimeraCliError;
use crate::output;
use crate::permission::{self, PermissionCtx};

/// 8 个内置 default Provider 名(对应 `mca-gateway` 8 个 affinity profile)
///
/// WHY 8 个:对齐 mca-gateway 设计目标 — 8 个 affinity profile 覆盖
/// 国内主流 LLM 服务(深度求索/智谱/MiniMax/字节/阶跃/阿里云等)。
const FALLBACK_PROVIDER_NAMES: &[&str] = &[
    "deepseek", "zhipu", "minimax", "volcano", "moonshot", "stepfun", "alicloud", "custom",
];

/// 单个 Provider 的展示行
#[derive(Debug, Clone, Serialize)]
struct ProviderRow {
    name: String,
    protocol: String,
    endpoint: String,
    tier: String,
    is_default: bool,
}

/// 执行 `llm` 子命令
///
/// `json` flag(Task 1.7)控制输出格式:envelope JSON vs 人类可读。
/// `perm`(Task 1.11)仅 `set-default` / `strategy Some(_)` 消费。
pub async fn execute(
    action: &LlmAction,
    cfg: &ChimeraConfig,
    json: bool,
    perm: &PermissionCtx,
) -> Result<()> {
    tracing::info!(?action, json, "LLM Provider 管理操作");
    match action {
        LlmAction::List => list_providers(cfg, json),
        LlmAction::Show { name } => show_provider(cfg, name, json),
        LlmAction::SetDefault { name } => set_default_provider(name, perm, json).await,
        LlmAction::Test { name } => test_provider(cfg, name.as_deref(), json).await,
        LlmAction::Channels => channels(json),
        LlmAction::Strategy { strategy: strat } => {
            strategy(cfg, strat.as_deref(), perm, json).await
        }
    }
}

/// 计算当前可用的 Provider 列表(优先 cfg 配置,空时回退到 8 默认名)
///
/// WHY 此处不直接读 `cfg.llm.providers`:ChimeraConfig 当前无 `llm` 顶层 section,
/// 复用 `model_router.providers` 作为数据源;若用户清空 model_router.providers,
/// 降级到内置 8 默认名(覆盖 mca-gateway 8 个 affinity profile)。
fn available_providers(cfg: &ChimeraConfig) -> Vec<ProviderRow> {
    let mut rows: Vec<ProviderRow> = Vec::new();
    let configured = &cfg.model_router.providers;
    if !configured.is_empty() {
        for (i, p) in configured.iter().enumerate() {
            // 优先使用 id 作为 name(id 唯一),回退到 name 字段
            let name = if !p.id.is_empty() {
                p.id.clone()
            } else {
                p.name.clone()
            };
            rows.push(ProviderRow {
                name,
                protocol: derive_protocol(&p.endpoint),
                endpoint: p.endpoint.clone(),
                tier: p.tier.clone(),
                is_default: i == 0,
            });
        }
    } else {
        for (i, name) in FALLBACK_PROVIDER_NAMES.iter().enumerate() {
            rows.push(ProviderRow {
                name: (*name).to_string(),
                protocol: "https".to_string(),
                endpoint: format!("https://{name}.example.com"),
                tier: "default".to_string(),
                is_default: i == 0,
            });
        }
    }
    rows
}

/// 从 endpoint URL 推导协议标识(用于 List 表格 protocol 列)
///
/// 简单字符串匹配,后续 Task 接入 mca-gateway 后可从 `ProviderConfig` 字段直接读取。
fn derive_protocol(endpoint: &str) -> String {
    let lower = endpoint.to_lowercase();
    if lower.contains("anthropic") {
        "anthropic".to_string()
    } else if lower.contains("openai") {
        "openai".to_string()
    } else if lower.contains("zhipu") {
        "zhipu".to_string()
    } else if lower.contains("dashscope") {
        "dashscope".to_string()
    } else if lower.contains("minimax") {
        "minimax".to_string()
    } else {
        "https".to_string()
    }
}

/// 渲染 Provider 列表为人类可读字符串(便于单测断言内容)
///
/// 格式:`name | protocol | default`(3 列,与 spec 对齐)。
fn render_provider_rows(rows: &[ProviderRow]) -> String {
    let mut out = String::new();
    out.push_str("NAME      PROTOCOL   DEFAULT\n");
    out.push_str("----------------------------\n");
    for r in rows {
        out.push_str(&format!(
            "{:<9}{:<11}{}\n",
            r.name,
            r.protocol,
            if r.is_default { "*" } else { "" },
        ));
    }
    out
}

/// `llm list` — 列出所有已配置 Provider
fn list_providers(cfg: &ChimeraConfig, json: bool) -> Result<()> {
    let rows = available_providers(cfg);
    if json {
        output::print_json(&rows).context("序列化 llm list 为 JSON 失败")?;
    } else {
        output::print_info("已配置 LLM Provider:");
        println!("{}", render_provider_rows(&rows));
    }
    Ok(())
}

/// `llm show <name>` — 显示 Provider 详情(endpoint / 协议 / 配额)
fn show_provider(cfg: &ChimeraConfig, name: &str, json: bool) -> Result<()> {
    let rows = available_providers(cfg);
    let row = rows
        .iter()
        .find(|r| r.name == name)
        .ok_or_else(|| ChimeraCliError::ConfigError(format!("未找到 Provider: {name}")))?;
    if json {
        output::print_json(row).context("序列化 llm show 为 JSON 失败")?;
    } else {
        output::print_info(&format!("Provider 详情: {}", name));
        println!("  endpoint: {}", row.endpoint);
        println!("  protocol: {}", row.protocol);
        println!("  tier:     {}", row.tier);
        println!("  default:  {}", if row.is_default { "yes" } else { "no" });
    }
    Ok(())
}

/// `llm set-default <name>` — 设置默认 Provider(骨架,仅打印 mock)
///
/// WHY 仅 mock:Task 2 范围内不实装 `ChimeraConfig::set_llm_default` 持久化,
/// 真实 API 接入在后续 Task 完成。
async fn set_default_provider(name: &str, perm: &PermissionCtx, json: bool) -> Result<()> {
    // 破坏性操作:改变全局默认行为,必须经 permission prompt
    let confirmed =
        permission::confirm(perm, "设置默认 LLM Provider", &format!("Provider: {name}")).await?;
    if !confirmed {
        return Err(
            ChimeraCliError::PermissionDenied(format!("用户拒绝设置默认 Provider {name}")).into(),
        );
    }
    if json {
        let payload = serde_json::json!({
            "action": "set-default",
            "provider": name,
            "persisted": false,
            "note": "[mock] ChimeraConfig::set_llm_default 尚未实装,后续 Task 接入",
        });
        output::print_json(&payload).context("序列化 llm set-default 为 JSON 失败")?;
    } else {
        println!("[mock] default provider set to: {name}");
        println!("(后续 Task 接入 ChimeraConfig::set_llm_default 持久化)");
    }
    Ok(())
}

/// `llm test [name]` — 探测 Provider 连通性(1s 模拟延迟 + 50/50 伪随机结果)
///
/// WHY 50/50 用 `std::time` 推算而非引入 `rand`:Task 2 约束零依赖新增。
/// `SystemTime` 的纳秒部分奇偶作为伪随机种子,统计上约 50% 成功率。
async fn test_provider(cfg: &ChimeraConfig, name: Option<&str>, json: bool) -> Result<()> {
    let rows = available_providers(cfg);
    // `name=None` 时默认使用列表中第一个 provider(即 `is_default=true` 那个)
    let target: String = match name {
        Some(n) => n.to_string(),
        None => rows
            .first()
            .map(|r| r.name.clone())
            .unwrap_or_else(|| FALLBACK_PROVIDER_NAMES[0].to_string()),
    };

    let start = Instant::now();
    // 模拟 1s 网络延迟(Task 2 骨架,后续 Task 替换为真实 HTTP 探测)
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let latency_ms = start.elapsed().as_millis() as u64;

    // 用当前 epoch 纳秒奇偶决定成功/失败(50/50)
    let epoch_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let success = epoch_ns.is_multiple_of(2);

    if json {
        let payload = if success {
            serde_json::json!({
                "provider": target,
                "ok": true,
                "latency_ms": latency_ms,
            })
        } else {
            serde_json::json!({
                "provider": target,
                "ok": false,
                "latency_ms": latency_ms,
                "error": "连通性探测失败(mock)",
            })
        };
        output::print_json(&payload).context("序列化 llm test 为 JSON 失败")?;
        if !success {
            return Err(
                ChimeraCliError::EngineError(format!("Provider {target} 连通性探测失败")).into(),
            );
        }
    } else if success {
        output::print_success(&format!("OK {latency_ms}ms ({target})"));
    } else {
        output::print_error(&format!("FAIL {latency_ms}ms ({target}) — 连通性探测失败"));
        return Err(
            ChimeraCliError::EngineError(format!("Provider {target} 连通性探测失败")).into(),
        );
    }
    Ok(())
}

/// `llm channels` — 列出 model-router 4 路由渠道(硬编码骨架)
///
/// 后续 Task 接入 `model_router.channels()` 真实数据。
fn channels(json: bool) -> Result<()> {
    let data: &[(&str, &[&str])] = &[
        ("Quality", &["zhipu", "alicloud"]),
        ("Balanced", &["deepseek"]),
        ("Cost", &["volcano"]),
        ("Speed", &["moonshot"]),
    ];
    if json {
        let payload: Vec<serde_json::Value> = data
            .iter()
            .map(|(name, members)| {
                serde_json::json!({
                    "name": name,
                    "providers": members,
                })
            })
            .collect();
        output::print_json(&payload).context("序列化 llm channels 为 JSON 失败")?;
    } else {
        output::print_info("model-router 4 路由渠道:");
        for (name, members) in data {
            println!("{name:<10}[{members:?}]");
        }
    }
    Ok(())
}

/// `llm strategy [name]` — 显示/设置 model-router 策略
///
/// `strategy=None` 时打印当前策略;`Some(s)` 时切换(permission prompt)。
async fn strategy(
    cfg: &ChimeraConfig,
    new_strategy: Option<&str>,
    perm: &PermissionCtx,
    json: bool,
) -> Result<()> {
    let current = cfg.model_router.strategy.as_str();
    if let Some(target) = new_strategy {
        // 切换策略:影响全局路由行为,必须经 permission prompt
        let confirmed = permission::confirm(
            perm,
            "切换 model-router 策略",
            &format!("new strategy: {target} (current: {current})"),
        )
        .await?;
        if !confirmed {
            return Err(
                ChimeraCliError::PermissionDenied(format!("用户拒绝切换策略 {target}")).into(),
            );
        }
        if json {
            let payload = serde_json::json!({
                "action": "set-strategy",
                "strategy": target,
                "previous": current,
                "persisted": false,
                "note": "[mock] 策略切换尚未实装,后续 Task 接入",
            });
            output::print_json(&payload).context("序列化 llm strategy 为 JSON 失败")?;
        } else {
            println!("[mock] strategy switched to: {target} (previous: {current})");
        }
    } else if json {
        let payload = serde_json::json!({ "strategy": current });
        output::print_json(&payload).context("序列化 llm strategy 为 JSON 失败")?;
    } else {
        output::print_info(&format!("当前 model-router 策略: {current}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证 `llm list` 在 `model_router.providers` 为空时,回退到 8 个内置
    /// default 名(deepseek / zhipu / ...),且渲染输出字符串包含 "deepseek"。
    #[tokio::test]
    async fn test_list_contains_deepseek_when_providers_empty() {
        // 构造 model_router.providers 为空的 cfg,触发 8 默认名回退
        let mut cfg = ChimeraConfig::default();
        cfg.model_router.providers.clear();

        let rows = available_providers(&cfg);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.contains(&"deepseek"),
            "应包含 deepseek,实际: {names:?}"
        );
        assert_eq!(rows.len(), 8, "应回退到 8 个内置 default 名");
        assert!(rows[0].is_default, "列表首位应标记为 default");

        // 验证渲染字符串包含 deepseek
        let rendered = render_provider_rows(&rows);
        assert!(
            rendered.contains("deepseek"),
            "渲染表格应包含 deepseek,实际输出:\n{rendered}"
        );
    }

    /// 验证 `available_providers` 在配置非空时,优先使用 cfg 中的 provider,
    /// 不触发 8 默认名回退(此时默认 config 含 5 个 provider:claude-opus 等)。
    #[tokio::test]
    async fn test_list_uses_configured_providers_when_nonempty() {
        let cfg = ChimeraConfig::default();
        // 默认 ChimeraConfig::model_router.providers 应非空(5 个 ProviderConfig)
        let rows = available_providers(&cfg);
        assert!(!rows.is_empty(), "默认 cfg 应有 provider");
        // 默认 cfg 的 5 个 provider id 包含 claude-opus(不应含 deepseek,因 deepseek
        // 只在 fallback 列表中)
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert!(
            !names.contains(&"deepseek"),
            "默认 cfg 不应回退到 fallback 列表: {names:?}"
        );
    }
}
