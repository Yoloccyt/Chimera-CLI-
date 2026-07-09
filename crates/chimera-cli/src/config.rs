//! Figment 多源配置加载 — 对齐 §10.2 omega.yaml 模板
//!
//! ## 架构说明(Phase IV F1 迁移后)
//! - **配置类型定义**(27 个 struct + Default impl + 默认值函数)已迁移至 `nexus-core/src/config.rs`
//! - 本模块通过 `pub use nexus_core::config::*;` re-export 全部类型,保持向后兼容
//! - **加载逻辑**(figment 合并 / omega.yaml 模板 / 文件初始化)保留在本模块
//! - 这样 L10 chimera-cli 依赖 L1 nexus-core(向下依赖,符合 §2.2 铁律)
//!
//! ## 配置优先级(后者覆盖前者)
//! 1. 内置默认值(`ChimeraConfig::default`)
//! 2. 配置文件(默认 `~/.aether/omega.yaml`,可由 `--config` 覆盖)
//! 3. 环境变量(前缀 `AETHER_`,嵌套用 `__` 分隔)
//! 4. CLI 参数(目前仅 `--config` 影响加载路径)
//!
//! ## 配置样例
//! - 简化样例见 `examples/config.sample.yaml` / `examples/config.sample.toml`
//! - 完整模板(含全部 14 个顶层 section)由 `aether config init` 生成

// 类型定义 re-export:nexus-core 定义,L10 通过 re-export 保持向后兼容。
// trait impl(Serialize/Deserialize)是全局的,re-export 后自动随类型传播。
pub use nexus_core::config::*;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use figment::{
    providers::{Env, Format, Serialized, Yaml},
    Figment,
};

// === 配置加载逻辑(保留在 L10 chimera-cli) ===

/// 默认配置文件路径:`~/.aether/omega.yaml`
///
/// 跨平台 home 目录展开:
/// - Unix: `$HOME/.aether/omega.yaml`
/// - Windows: `%USERPROFILE%\.aether\omega.yaml`
pub fn default_config_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".aether").join("omega.yaml")
}

/// 返回内置默认配置(等价于 `ChimeraConfig::default()`)
pub fn default_config() -> ChimeraConfig {
    ChimeraConfig::default()
}

/// 从多源加载配置(优先级:CLI > env > file > defaults)
///
/// `config_path` 为 `None` 时使用 [`default_config_path`]。
/// 配置文件不存在时不报错,仅使用默认值 + 环境变量。
pub fn load(config_path: Option<PathBuf>) -> Result<ChimeraConfig> {
    let path = config_path.unwrap_or_else(default_config_path);

    // 优先级链:defaults -> file -> env(后者覆盖前者)
    // 注:CLI 参数目前仅影响 config_path,未直接进入 Figment;
    //     后续可扩展 CLI override provider 以支持 --strategy 等参数。
    let figment = Figment::from(Serialized::defaults(ChimeraConfig::default()))
        .merge(Yaml::file(&path))
        .merge(Env::prefixed("AETHER_").split("__"));

    figment
        .extract::<ChimeraConfig>()
        .with_context(|| format!("加载配置失败:{}", path.display()))
}

/// 生成默认 omega.yaml 到指定路径
///
/// 生成的文件与 §10.2 模板完全一致(含注释),便于用户编辑。
/// 如果父目录不存在会自动创建。
pub fn init_config_file(path: &Path) -> Result<()> {
    // 确保父目录存在(如 ~/.aether/)
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建配置目录失败:{}", parent.display()))?;
        }
    }

    let content = omega_yaml_template();
    std::fs::write(path, content)
        .with_context(|| format!("写入配置文件失败:{}", path.display()))?;
    Ok(())
}

/// 返回 omega.yaml 模板字符串(对齐 §10.2,含注释)
///
/// 独立为函数以便单测验证模板非空,且保持 `init_config_file` 简洁。
fn omega_yaml_template() -> &'static str {
    // 注:模板内容与 AETHER_NEXUS_OMEGA_ULTIMATE.md §10.2 完全对齐
    // minimax-m3 的 output_cost_per_k 已修正为 output_cost_per_1k 以保持字段一致
    r#"# ~/.aether/omega.yaml
nexus:
  version: "1.0.0-omega"

quest:
  auto_decompose: true
  max_tasks_per_quest: 20
  default_deadline_hours: 168
  checkpoint_interval_ops: 100
  checkpoint_interval_minutes: 10

thinking_toggle:
  default_mode: "Auto"  # NonThinking / Lite / Deep / Max / Auto
  auto_thresholds:
    non_thinking: { complexity: 0.1, risk: "Low" }
    lite: { complexity: 0.4, risk: "Medium" }
    deep: { complexity: 0.7, risk: "High" }
    max: { complexity: 0.9, risk: "Critical" }

repo_wiki:
  auto_generate: true
  db_path: "~/.aether/wiki.db"
  embedding_dim: 256
  auto_update_on_commit: true

model_router:
  strategy: "Auto"  # CostOptimized / SpeedOptimized / QualityOptimized / Auto / Failover
  budget:
    daily_usd: 50.0
    monthly_usd: 1000.0
    alert_threshold: 0.8
  providers:
    - id: "claude-opus"
      name: "Claude Opus 4.8"
      endpoint: "https://api.anthropic.com"
      context_window: 200000
      capabilities: [CodeGeneration, ArchitectureDesign, SecurityAudit, Reasoning]
      tier: "premium"
      input_cost_per_1k: 15.0
      output_cost_per_1k: 75.0
    - id: "gpt-4o"
      name: "GPT-4o"
      endpoint: "https://api.openai.com"
      context_window: 128000
      capabilities: [CodeGeneration, CodeReview, ToolUse]
      tier: "efficient"
      input_cost_per_1k: 2.5
      output_cost_per_1k: 10.0
    - id: "qwen-coder"
      name: "Qwen Coder"
      endpoint: "https://dashscope.aliyuncs.com"
      context_window: 128000
      capabilities: [CodeGeneration, LongContext, Multilingual]
      tier: "lite"
      input_cost_per_1k: 0.5
      output_cost_per_1k: 2.0
    - id: "minimax-m3"
      name: "Minimax M3"
      endpoint: "https://api.minimax.chat"
      context_window: 1000000
      capabilities: [CodeGeneration, LongContext, Multimodal]
      tier: "efficient"
      input_cost_per_1k: 0.3
      output_cost_per_1k: 1.2
    - id: "glm-5.2"
      name: "GLM 5.2"
      endpoint: "https://api.zhipu.ai"
      context_window: 1000000
      capabilities: [CodeGeneration, LongContext, Reasoning]
      tier: "premium"
      input_cost_per_1k: 1.0
      output_cost_per_1k: 4.0

osa:
  dimensions: [routing, context, memory, audit, budget]
  sparsity_base: 0.8
  complexity_adjustment: true

kvbsr:
  max_blocks: 20
  tools_per_block: 15
  auto_rebalance_threshold: 100
  coherence_min: 0.7

pvl:
  producer_timeout_ms: 5000
  verifier_timeout_ms: 3000
  feedback_channel_size: 100
  max_retry: 3

mtpe:
  default_prediction_depth: 3
  max_prediction_depth: 10
  adapt_depth_enabled: true
  batch_verify: true

gqep:
  batch_size: 10
  resource_types: [FileSystem, Network, Git, Docker, Database]
  connection_pool_size: 5

seccore:
  sandbox: gvisor
  seccomp: true
  command_interpolation: forbidden
  red_team:
    enabled: true
    audit_frequency: 0.1
    active_probe_interval_hours: 24
  capability_decay:
    initial: 1.0
    high_risk_decay: 0.2
    medium_risk_decay: 0.1
    low_risk_decay: 0.02
    recovery_rate: 0.05
    recovery_interval_minutes: 10

mcp:
  mesh:
    transports: [stdio, http]
    entanglement: true
  servers:
    - id: filesystem
      command: "npx"
      args: ["-y", "@modelcontextprotocol/server-filesystem"]
    - id: github
      url: "https://api.github.com/mcp"
      auth: oauth
    - id: postgres
      url: "postgresql://localhost:5432/mcp"
      auth: password

evolution:
  enabled: true
  mutation_pool_path: "~/.aether/evolution/mutations/"
  fitness_function: "(success_rate * 0.4) + (speed * 0.3) + (token_efficiency * 0.2) + (safety * 0.1)"
  ab_test:
    enabled: true
    min_samples: 30
    significance_threshold: 1.5
  online_learning:
    enabled: true
    update_frequency: 10  # 每 10 次任务更新
    learning_rate: 0.01

monitoring:
  prometheus:
    enabled: true
    port: 9090
  grafana:
    enabled: true
    dashboard_path: "./monitoring/grafana-dashboard.json"
  alerts:
    - name: "CapabilityDepleted"
      expr: "aether_capability_current < 0.1"
      for: "1m"
    - name: "HighOrphanRate"
      expr: "rate(aether_orphan_calls_total[5m]) > 0"
      for: "1m"
    - name: "BudgetAlert"
      expr: "aether_daily_cost / aether_daily_budget > 0.8"
      for: "5m"
    - name: "RedTeamVulnerability"
      expr: "aether_red_team_vulnerabilities > 0"
      for: "1m"
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_omega_yaml_template_non_empty() {
        let tpl = omega_yaml_template();
        assert!(tpl.contains("nexus:"));
        assert!(tpl.contains("model_router:"));
        assert!(tpl.contains("seccore:"));
    }
}
