//! Chimera CLI 配置类型定义 — 14 个顶层 section 的纯数据结构
//!
//! 对应架构层:L1 Core(类型定义);加载逻辑位于 L10 chimera-cli
//!
//! ## 设计决策
//! - 子配置全部派生 `Default`,避免在 `ChimeraConfig::default` 中重复初始化
//! - `providers` 的 `capabilities` 用 `Vec<String>` 而非枚举,保持向前兼容(新能力不需改代码)
//! - `mcp.servers` 用统一 struct + `Option` 字段,兼容 stdio/http/db 三种传输
//!
//! ## 迁移说明(Phase IV F1)
//! - 本模块从 `chimera-cli/src/config.rs` 迁移纯数据类型到 L1 Core 层
//! - 加载逻辑(figment 合并 / omega.yaml 模板)保留在 chimera-cli,此处不含 figment 依赖
//! - chimera-cli 通过 `pub use nexus_core::config::*;` re-export 保持向后兼容
//! - `NexusConfig::default()` 的 `env!("CARGO_PKG_VERSION")` 取 nexus-core 版本,
//!   与 workspace.package.version(1.0.0-omega)一致,迁移无 break

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

// === 顶层配置结构 ===

/// Chimera CLI 顶层配置(对应 omega.yaml 根结构)
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(default)]
pub struct ChimeraConfig {
    /// Nexus 元信息
    pub nexus: NexusConfig,
    /// Quest 长期任务配置
    pub quest: QuestConfig,
    /// 思考切换治理(TTG)
    pub thinking_toggle: ThinkingToggleConfig,
    /// Repo Wiki 知识库
    pub repo_wiki: RepoWikiConfig,
    /// 模型路由器
    pub model_router: ModelRouterConfig,
    /// 全维稀疏架构(OSA)
    pub osa: OsaConfig,
    /// KV 块语义路由器(KVBSR)
    pub kvbsr: KvbsrConfig,
    /// 生产者-验证者循环(PVL)
    pub pvl: PvlConfig,
    /// 多步预测执行(MTPE)
    pub mtpe: MtpeConfig,
    /// 聚集执行协议(GQEP)
    pub gqep: GqepConfig,
    /// 安全核心(SecCore)
    pub seccore: SeccoreConfig,
    /// MCP 网格
    pub mcp: McpConfig,
    /// 在线进化(GSOE)
    pub evolution: EvolutionConfig,
    /// 监控(Prometheus/Grafana)
    pub monitoring: MonitoringConfig,
}

/// Nexus 元信息
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct NexusConfig {
    /// 配置版本号(与 workspace.package.version 对齐)
    pub version: String,
}

impl Default for NexusConfig {
    fn default() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Quest 长期任务配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct QuestConfig {
    /// 是否自动分解 Quest 为子任务
    pub auto_decompose: bool,
    /// 单个 Quest 最大任务数(防止无限分解)
    pub max_tasks_per_quest: u32,
    /// 默认截止时间(小时)
    pub default_deadline_hours: u32,
    /// 检查点间隔(操作次数)
    pub checkpoint_interval_ops: u32,
    /// 检查点间隔(分钟)
    pub checkpoint_interval_minutes: u32,
}

impl Default for QuestConfig {
    fn default() -> Self {
        Self {
            auto_decompose: true,
            max_tasks_per_quest: 20,
            default_deadline_hours: 168,
            checkpoint_interval_ops: 100,
            checkpoint_interval_minutes: 10,
        }
    }
}

/// 思考切换治理(TTG)配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct ThinkingToggleConfig {
    /// 默认思考模式:NonThinking / Lite / Deep / Max / Auto
    pub default_mode: String,
    /// Auto 模式下的自动切换阈值
    pub auto_thresholds: AutoThresholdsConfig,
}

impl Default for ThinkingToggleConfig {
    fn default() -> Self {
        Self {
            default_mode: "Auto".to_string(),
            auto_thresholds: AutoThresholdsConfig::default(),
        }
    }
}

/// Auto 模式阈值(复杂度 + 风险双维度)
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct AutoThresholdsConfig {
    /// 非思考模式阈值
    pub non_thinking: ThresholdEntry,
    /// 轻量思考阈值
    pub lite: ThresholdEntry,
    /// 深度思考阈值
    pub deep: ThresholdEntry,
    /// 最大思考阈值
    pub max: ThresholdEntry,
}

impl Default for AutoThresholdsConfig {
    fn default() -> Self {
        Self {
            non_thinking: ThresholdEntry {
                complexity: 0.1,
                risk: "Low".to_string(),
            },
            lite: ThresholdEntry {
                complexity: 0.4,
                risk: "Medium".to_string(),
            },
            deep: ThresholdEntry {
                complexity: 0.7,
                risk: "High".to_string(),
            },
            max: ThresholdEntry {
                complexity: 0.9,
                risk: "Critical".to_string(),
            },
        }
    }
}

/// 单个阈值条目
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(default)]
pub struct ThresholdEntry {
    /// 复杂度阈值(0.0-1.0)
    pub complexity: f64,
    /// 风险等级:Low / Medium / High / Critical
    pub risk: String,
}

/// Repo Wiki 知识库配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct RepoWikiConfig {
    /// 是否自动生成 Wiki
    pub auto_generate: bool,
    /// Wiki 数据库路径
    pub db_path: String,
    /// 嵌入向量维度
    pub embedding_dim: u32,
    /// 提交时自动更新
    pub auto_update_on_commit: bool,
}

impl Default for RepoWikiConfig {
    fn default() -> Self {
        Self {
            auto_generate: true,
            db_path: "~/.aether/wiki.db".to_string(),
            embedding_dim: 256,
            auto_update_on_commit: true,
        }
    }
}

/// 模型路由器配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct ModelRouterConfig {
    /// 路由策略:CostOptimized / SpeedOptimized / QualityOptimized / Auto / Failover
    pub strategy: String,
    /// 预算控制
    pub budget: BudgetConfig,
    /// 模型提供商列表
    pub providers: Vec<ProviderConfig>,
}

impl Default for ModelRouterConfig {
    fn default() -> Self {
        Self {
            strategy: "Auto".to_string(),
            budget: BudgetConfig::default(),
            providers: default_providers(),
        }
    }
}

/// 预算配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct BudgetConfig {
    /// 每日预算(美元)
    pub daily_usd: f64,
    /// 每月预算(美元)
    pub monthly_usd: f64,
    /// 告警阈值(0.0-1.0,占预算比例)
    pub alert_threshold: f64,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            daily_usd: 50.0,
            monthly_usd: 1000.0,
            alert_threshold: 0.8,
        }
    }
}

/// 单个模型提供商配置
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(default)]
pub struct ProviderConfig {
    /// 提供商 ID(唯一标识)
    pub id: String,
    /// 显示名称
    pub name: String,
    /// API 端点
    pub endpoint: String,
    /// 上下文窗口大小(tokens)
    pub context_window: u32,
    /// 能力列表(用 String 保持向前兼容)
    pub capabilities: Vec<String>,
    /// 层级:premium / efficient / lite
    pub tier: String,
    /// 每 1k 输入 token 成本(美元)
    pub input_cost_per_1k: f64,
    /// 每 1k 输出 token 成本(美元)
    pub output_cost_per_1k: f64,
}

/// 默认提供商列表(对齐 §10.2 模板的 5 个模型)
pub fn default_providers() -> Vec<ProviderConfig> {
    vec![
        ProviderConfig {
            id: "claude-opus".to_string(),
            name: "Claude Opus 4.8".to_string(),
            endpoint: "https://api.anthropic.com".to_string(),
            context_window: 200_000,
            capabilities: vec![
                "CodeGeneration".into(),
                "ArchitectureDesign".into(),
                "SecurityAudit".into(),
                "Reasoning".into(),
            ],
            tier: "premium".into(),
            input_cost_per_1k: 15.0,
            output_cost_per_1k: 75.0,
        },
        ProviderConfig {
            id: "gpt-4o".to_string(),
            name: "GPT-4o".to_string(),
            endpoint: "https://api.openai.com".to_string(),
            context_window: 128_000,
            capabilities: vec![
                "CodeGeneration".into(),
                "CodeReview".into(),
                "ToolUse".into(),
            ],
            tier: "efficient".into(),
            input_cost_per_1k: 2.5,
            output_cost_per_1k: 10.0,
        },
        ProviderConfig {
            id: "qwen-coder".to_string(),
            name: "Qwen Coder".to_string(),
            endpoint: "https://dashscope.aliyuncs.com".to_string(),
            context_window: 128_000,
            capabilities: vec![
                "CodeGeneration".into(),
                "LongContext".into(),
                "Multilingual".into(),
            ],
            tier: "lite".into(),
            input_cost_per_1k: 0.5,
            output_cost_per_1k: 2.0,
        },
        ProviderConfig {
            id: "minimax-m3".to_string(),
            name: "Minimax M3".to_string(),
            endpoint: "https://api.minimax.chat".to_string(),
            context_window: 1_000_000,
            capabilities: vec![
                "CodeGeneration".into(),
                "LongContext".into(),
                "Multimodal".into(),
            ],
            tier: "efficient".into(),
            input_cost_per_1k: 0.3,
            // 注:§10.2 模板原文为 output_cost_per_k,此处修正为 output_cost_per_1k 以保持字段一致
            output_cost_per_1k: 1.2,
        },
        ProviderConfig {
            id: "glm-5.2".to_string(),
            name: "GLM 5.2".to_string(),
            endpoint: "https://api.zhipu.ai".to_string(),
            context_window: 1_000_000,
            capabilities: vec![
                "CodeGeneration".into(),
                "LongContext".into(),
                "Reasoning".into(),
            ],
            tier: "premium".into(),
            input_cost_per_1k: 1.0,
            output_cost_per_1k: 4.0,
        },
    ]
}

/// 全维稀疏架构(OSA)配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct OsaConfig {
    /// 稀疏化维度:routing / context / memory / audit / budget
    pub dimensions: Vec<String>,
    /// 基础稀疏度(0.0-1.0,越高越稀疏)
    pub sparsity_base: f64,
    /// 是否根据复杂度动态调整
    pub complexity_adjustment: bool,
}

impl Default for OsaConfig {
    fn default() -> Self {
        Self {
            dimensions: vec![
                "routing".into(),
                "context".into(),
                "memory".into(),
                "audit".into(),
                "budget".into(),
            ],
            sparsity_base: 0.8,
            complexity_adjustment: true,
        }
    }
}

/// KV 块语义路由器(KVBSR)配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct KvbsrConfig {
    /// 最大块数
    pub max_blocks: u32,
    /// 每块工具数
    pub tools_per_block: u32,
    /// 自动重平衡阈值
    pub auto_rebalance_threshold: u32,
    /// 最小一致性阈值
    pub coherence_min: f64,
}

impl Default for KvbsrConfig {
    fn default() -> Self {
        Self {
            max_blocks: 20,
            tools_per_block: 15,
            auto_rebalance_threshold: 100,
            coherence_min: 0.7,
        }
    }
}

/// 生产者-验证者循环(PVL)配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct PvlConfig {
    /// 生产者超时(毫秒)
    pub producer_timeout_ms: u64,
    /// 验证者超时(毫秒)
    pub verifier_timeout_ms: u64,
    /// 反馈通道容量
    pub feedback_channel_size: u32,
    /// 最大重试次数
    pub max_retry: u32,
}

impl Default for PvlConfig {
    fn default() -> Self {
        Self {
            producer_timeout_ms: 5000,
            verifier_timeout_ms: 3000,
            feedback_channel_size: 100,
            max_retry: 3,
        }
    }
}

/// 多步预测执行(MTPE)配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct MtpeConfig {
    /// 默认预测深度
    pub default_prediction_depth: u32,
    /// 最大预测深度
    pub max_prediction_depth: u32,
    /// 是否启用自适应深度
    pub adapt_depth_enabled: bool,
    /// 是否批量验证
    pub batch_verify: bool,
}

impl Default for MtpeConfig {
    fn default() -> Self {
        Self {
            default_prediction_depth: 3,
            max_prediction_depth: 10,
            adapt_depth_enabled: true,
            batch_verify: true,
        }
    }
}

/// 聚集执行协议(GQEP)配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct GqepConfig {
    /// 批量大小
    pub batch_size: u32,
    /// 资源类型:FileSystem / Network / Git / Docker / Database
    pub resource_types: Vec<String>,
    /// 连接池大小
    pub connection_pool_size: u32,
}

impl Default for GqepConfig {
    fn default() -> Self {
        Self {
            batch_size: 10,
            resource_types: vec![
                "FileSystem".into(),
                "Network".into(),
                "Git".into(),
                "Docker".into(),
                "Database".into(),
            ],
            connection_pool_size: 5,
        }
    }
}

/// 安全核心(SecCore)配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct SeccoreConfig {
    /// 沙箱类型:gvisor / none
    pub sandbox: String,
    /// 是否启用 seccomp
    pub seccomp: bool,
    /// 命令插值策略:forbidden / allowed
    pub command_interpolation: String,
    /// 红队配置
    pub red_team: RedTeamConfig,
    /// 能力衰减配置
    pub capability_decay: CapabilityDecayConfig,
}

impl Default for SeccoreConfig {
    fn default() -> Self {
        Self {
            sandbox: "gvisor".to_string(),
            seccomp: true,
            command_interpolation: "forbidden".to_string(),
            red_team: RedTeamConfig::default(),
            capability_decay: CapabilityDecayConfig::default(),
        }
    }
}

/// 红队(AHIRT)配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct RedTeamConfig {
    /// 是否启用红队
    pub enabled: bool,
    /// 审计频率(0.0-1.0,每次操作被审计的概率)
    pub audit_frequency: f64,
    /// 主动探测间隔(小时)
    pub active_probe_interval_hours: u32,
}

impl Default for RedTeamConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            audit_frequency: 0.1,
            active_probe_interval_hours: 24,
        }
    }
}

/// 能力衰减配置(对应 DecayEngine)
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct CapabilityDecayConfig {
    /// 初始能力值
    pub initial: f64,
    /// 高风险衰减率
    pub high_risk_decay: f64,
    /// 中风险衰减率
    pub medium_risk_decay: f64,
    /// 低风险衰减率
    pub low_risk_decay: f64,
    /// 恢复率
    pub recovery_rate: f64,
    /// 恢复间隔(分钟)
    pub recovery_interval_minutes: u32,
}

impl Default for CapabilityDecayConfig {
    fn default() -> Self {
        Self {
            initial: 1.0,
            high_risk_decay: 0.2,
            medium_risk_decay: 0.1,
            low_risk_decay: 0.02,
            recovery_rate: 0.05,
            recovery_interval_minutes: 10,
        }
    }
}

/// MCP 网格配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct McpConfig {
    /// Mesh 网格配置
    pub mesh: McpMeshConfig,
    /// MCP 服务器列表
    pub servers: Vec<McpServerConfig>,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            mesh: McpMeshConfig::default(),
            servers: default_mcp_servers(),
        }
    }
}

/// MCP Mesh 配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct McpMeshConfig {
    /// 传输协议:stdio / http
    pub transports: Vec<String>,
    /// 是否启用量子纠缠(QEEP)
    pub entanglement: bool,
}

impl Default for McpMeshConfig {
    fn default() -> Self {
        Self {
            transports: vec!["stdio".into(), "http".into()],
            entanglement: true,
        }
    }
}

/// 单个 MCP 服务器配置(统一 struct,兼容 stdio/http/db 三种传输)
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(default)]
pub struct McpServerConfig {
    /// 服务器 ID
    pub id: String,
    /// stdio 模式:启动命令
    pub command: Option<String>,
    /// stdio 模式:命令参数
    pub args: Option<Vec<String>>,
    /// http/db 模式:URL
    pub url: Option<String>,
    /// 认证方式:oauth / password / none
    pub auth: Option<String>,
}

/// 默认 MCP 服务器列表(对齐 §10.2 模板)
pub fn default_mcp_servers() -> Vec<McpServerConfig> {
    vec![
        McpServerConfig {
            id: "filesystem".to_string(),
            command: Some("npx".to_string()),
            args: Some(vec![
                "-y".into(),
                "@modelcontextprotocol/server-filesystem".into(),
            ]),
            url: None,
            auth: None,
        },
        McpServerConfig {
            id: "github".to_string(),
            command: None,
            args: None,
            url: Some("https://api.github.com/mcp".to_string()),
            auth: Some("oauth".to_string()),
        },
        McpServerConfig {
            id: "postgres".to_string(),
            command: None,
            args: None,
            url: Some("postgresql://localhost:5432/mcp".to_string()),
            auth: Some("password".to_string()),
        },
    ]
}

/// 在线进化(GSOE)配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct EvolutionConfig {
    /// 是否启用进化
    pub enabled: bool,
    /// 变异池路径
    pub mutation_pool_path: String,
    /// 适应度函数表达式
    pub fitness_function: String,
    /// A/B 测试配置
    pub ab_test: AbTestConfig,
    /// 在线学习配置
    pub online_learning: OnlineLearningConfig,
}

impl Default for EvolutionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mutation_pool_path: "~/.aether/evolution/mutations/".to_string(),
            fitness_function:
                "(success_rate * 0.4) + (speed * 0.3) + (token_efficiency * 0.2) + (safety * 0.1)"
                    .to_string(),
            ab_test: AbTestConfig::default(),
            online_learning: OnlineLearningConfig::default(),
        }
    }
}

/// A/B 测试配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct AbTestConfig {
    /// 是否启用 A/B 测试
    pub enabled: bool,
    /// 最小样本数(统计显著性)
    pub min_samples: u32,
    /// 显著性阈值
    pub significance_threshold: f64,
}

impl Default for AbTestConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_samples: 30,
            significance_threshold: 1.5,
        }
    }
}

/// 在线学习配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct OnlineLearningConfig {
    /// 是否启用在线学习
    pub enabled: bool,
    /// 更新频率(每 N 次任务更新一次)
    pub update_frequency: u32,
    /// 学习率
    pub learning_rate: f64,
}

impl Default for OnlineLearningConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            update_frequency: 10,
            learning_rate: 0.01,
        }
    }
}

/// 监控配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct MonitoringConfig {
    /// Prometheus 配置
    pub prometheus: PrometheusConfig,
    /// Grafana 配置
    pub grafana: GrafanaConfig,
    /// 告警规则
    pub alerts: Vec<AlertConfig>,
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            prometheus: PrometheusConfig::default(),
            grafana: GrafanaConfig::default(),
            alerts: default_alerts(),
        }
    }
}

/// Prometheus 配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct PrometheusConfig {
    /// 是否启用
    pub enabled: bool,
    /// 端口
    pub port: u16,
}

impl Default for PrometheusConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 9090,
        }
    }
}

/// Grafana 配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct GrafanaConfig {
    /// 是否启用
    pub enabled: bool,
    /// Dashboard 路径
    pub dashboard_path: String,
}

impl Default for GrafanaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            dashboard_path: "./monitoring/grafana-dashboard.json".to_string(),
        }
    }
}

/// 告警规则配置
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(default)]
pub struct AlertConfig {
    /// 告警名称
    pub name: String,
    /// PromQL 表达式
    pub expr: String,
    /// 持续时间
    pub r#for: String,
}

/// 默认告警规则(对齐 §10.2 模板)
pub fn default_alerts() -> Vec<AlertConfig> {
    vec![
        AlertConfig {
            name: "CapabilityDepleted".to_string(),
            expr: "aether_capability_current < 0.1".to_string(),
            r#for: "1m".to_string(),
        },
        AlertConfig {
            name: "HighOrphanRate".to_string(),
            expr: "rate(aether_orphan_calls_total[5m]) > 0".to_string(),
            r#for: "1m".to_string(),
        },
        AlertConfig {
            name: "BudgetAlert".to_string(),
            expr: "aether_daily_cost / aether_daily_budget > 0.8".to_string(),
            r#for: "5m".to_string(),
        },
        AlertConfig {
            name: "RedTeamVulnerability".to_string(),
            expr: "aether_red_team_vulnerabilities > 0".to_string(),
            r#for: "1m".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证全部 14 个顶层 section + 13 个嵌套子类型可从 nexus_core::config 导入,
    /// 且 Default impl 可用。
    #[test]
    fn test_config_types_in_nexus_core() {
        use crate::config::{
            AbTestConfig, AlertConfig, AutoThresholdsConfig, BudgetConfig, CapabilityDecayConfig,
            ChimeraConfig, EvolutionConfig, GqepConfig, GrafanaConfig, KvbsrConfig, McpConfig,
            McpMeshConfig, McpServerConfig, ModelRouterConfig, MonitoringConfig, MtpeConfig,
            NexusConfig, OnlineLearningConfig, OsaConfig, PrometheusConfig, ProviderConfig,
            PvlConfig, QuestConfig, RedTeamConfig, RepoWikiConfig, SeccoreConfig,
            ThinkingToggleConfig, ThresholdEntry,
        };

        // 编译期验证:上述 27 个类型均存在且可导入
        // 运行期验证:构造默认配置,间接验证所有 section 的 Default impl 可用
        let cfg = ChimeraConfig::default();

        // 14 个顶层 section 全部可访问且非空/非零
        assert!(!cfg.nexus.version.is_empty(), "nexus.version 非空");
        assert!(cfg.quest.auto_decompose, "quest.auto_decompose = true");
        assert!(
            !cfg.thinking_toggle.default_mode.is_empty(),
            "thinking_toggle.default_mode 非空"
        );
        assert!(
            cfg.repo_wiki.auto_generate,
            "repo_wiki.auto_generate = true"
        );
        assert!(
            !cfg.model_router.providers.is_empty(),
            "model_router.providers 非空"
        );
        assert!(!cfg.osa.dimensions.is_empty(), "osa.dimensions 非空");
        assert!(cfg.kvbsr.max_blocks > 0, "kvbsr.max_blocks > 0");
        assert!(cfg.pvl.max_retry > 0, "pvl.max_retry > 0");
        assert!(
            cfg.mtpe.default_prediction_depth > 0,
            "mtpe.default_prediction_depth > 0"
        );
        assert!(cfg.gqep.batch_size > 0, "gqep.batch_size > 0");
        assert!(!cfg.seccore.sandbox.is_empty(), "seccore.sandbox 非空");
        assert!(!cfg.mcp.servers.is_empty(), "mcp.servers 非空");
        assert!(cfg.evolution.enabled, "evolution.enabled = true");
        assert!(
            cfg.monitoring.prometheus.enabled,
            "monitoring.prometheus.enabled = true"
        );

        // 抑制未使用导入警告(类型仅在 use 语句中引用,编译期验证已足够)
        let _ = (
            AbTestConfig::default(),
            AlertConfig::default(),
            AutoThresholdsConfig::default(),
            BudgetConfig::default(),
            CapabilityDecayConfig::default(),
            EvolutionConfig::default(),
            GrafanaConfig::default(),
            GqepConfig::default(),
            KvbsrConfig::default(),
            McpConfig::default(),
            McpMeshConfig::default(),
            McpServerConfig::default(),
            ModelRouterConfig::default(),
            MonitoringConfig::default(),
            MtpeConfig::default(),
            NexusConfig::default(),
            OnlineLearningConfig::default(),
            OsaConfig::default(),
            PrometheusConfig::default(),
            ProviderConfig::default(),
            PvlConfig::default(),
            QuestConfig::default(),
            RedTeamConfig::default(),
            RepoWikiConfig::default(),
            SeccoreConfig::default(),
            ThinkingToggleConfig::default(),
            ThresholdEntry::default(),
        );
    }

    /// 验证默认配置非空(从 chimera-cli 迁移的原有测试)
    #[test]
    fn test_default_config_non_empty() {
        let cfg = ChimeraConfig::default();
        assert!(!cfg.nexus.version.is_empty());
        assert!(!cfg.quest.auto_decompose.to_string().is_empty());
        assert!(!cfg.model_router.providers.is_empty());
    }

    /// 验证 ChimeraConfig::default() 的 serde 序列化/反序列化往返一致。
    ///
    /// 采用 JSON 字符串比对策略:序列化 → 反序列化 → 再序列化,
    /// 两次 JSON 字符串应完全一致。这无需为 struct 派生 PartialEq,
    /// 即可验证往返无损。
    #[test]
    fn test_default_config_roundtrip() {
        let cfg = ChimeraConfig::default();

        let json1 = serde_json::to_string(&cfg).expect("序列化 ChimeraConfig 失败");
        let cfg2: ChimeraConfig =
            serde_json::from_str(&json1).expect("反序列化 ChimeraConfig 失败");
        let json2 = serde_json::to_string(&cfg2).expect("二次序列化 ChimeraConfig 失败");

        assert_eq!(
            json1, json2,
            "serde 往返不一致:默认配置序列化后应可无损还原"
        );
    }
}
