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
use std::sync::OnceLock;
use std::time::SystemTime;

use anyhow::{Context, Result};
use figment::{
    providers::{Env, Format, Serialized, Yaml},
    Figment,
};

// === P2-3: 配置热重载 — 文件监听与增量更新 ===

/// 配置文件状态 — 用于检测文件变化
#[derive(Debug, Clone)]
struct ConfigFileState {
    /// 文件路径
    path: PathBuf,
    /// 最后修改时间
    last_modified: SystemTime,
    /// 文件内容哈希(简单检测内容变化)
    content_hash: u64,
}

impl ConfigFileState {
    /// 从路径创建状态快照
    fn from_path(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        let last_modified = metadata.modified().ok()?;
        let content = std::fs::read(path).ok()?;
        let content_hash = fnv_hash(&content);
        Some(Self {
            path: path.to_path_buf(),
            last_modified,
            content_hash,
        })
    }

    /// 检查文件是否发生变化
    fn has_changed(&self) -> bool {
        let current = Self::from_path(&self.path);
        match current {
            Some(c) => c.last_modified != self.last_modified || c.content_hash != self.content_hash,
            None => true,
        }
    }
}

/// 简单的 FNV-1a 哈希(用于检测文件内容变化)
fn fnv_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// P2-3: 热重载配置管理器 — 支持文件监听与增量更新
///
/// 基于 `LazyConfig` 扩展,添加文件变化检测与自动重载能力。
/// 使用轮询方式(而非 OS 文件监听)检测变化,跨平台兼容且无需额外依赖。
///
/// # 使用示例
/// ```no_run
/// use chimera_cli::config::HotReloadConfig;
///
/// # async fn run() {
/// let mut hot = HotReloadConfig::new(None).unwrap();
/// // 每 5 秒检查一次文件变化
/// hot.start_watch(std::time::Duration::from_secs(5)).await;
/// # }
/// ```
pub struct HotReloadConfig {
    /// 内部 LazyConfig(持有 Figment provider)
    inner: LazyConfig,
    /// 配置文件路径
    config_path: PathBuf,
    /// 文件状态(用于检测变化)
    file_state: Option<ConfigFileState>,
    /// 重载回调列表(配置变化时触发)
    reload_callbacks: Vec<Box<dyn Fn(&ChimeraConfig) + Send + Sync>>,
}

impl HotReloadConfig {
    /// 创建热重载配置管理器
    ///
    /// `config_path` 为 `None` 时使用 [`default_config_path`]。
    pub fn new(config_path: Option<PathBuf>) -> Result<Self> {
        let path = config_path.unwrap_or_else(default_config_path);
        let inner = LazyConfig::new(Some(path.clone()))?;
        let file_state = ConfigFileState::from_path(&path);

        Ok(Self {
            inner,
            config_path: path,
            file_state,
            reload_callbacks: Vec::new(),
        })
    }

    /// 添加重载回调
    ///
    /// 配置发生变化并成功重载后,调用此回调通知上层。
    pub fn on_reload<F>(&mut self, callback: F)
    where
        F: Fn(&ChimeraConfig) + Send + Sync + 'static,
    {
        self.reload_callbacks.push(Box::new(callback));
    }

    /// 手动检查文件变化并触发重载
    ///
    /// 返回 `true` 表示文件已变化且重载成功。
    pub fn check_and_reload(&mut self) -> Result<bool> {
        let changed = match &self.file_state {
            Some(state) => state.has_changed(),
            None => true,
        };

        if !changed {
            return Ok(false);
        }

        // 重建 Figment provider(重新读取文件)
        let new_inner = LazyConfig::new(Some(self.config_path.clone()))?;

        // 触发全量解析验证配置有效性
        let _ = new_inner.to_chimera_config()?;

        // 更新内部状态
        self.inner = new_inner;
        self.file_state = ConfigFileState::from_path(&self.config_path);

        // 触发回调
        if let Ok(config) = self.inner.to_chimera_config() {
            for cb in &self.reload_callbacks {
                cb(&config);
            }
        }

        Ok(true)
    }

    /// 启动后台轮询监听(异步)
    ///
    /// 在 tokio runtime 中定期(按 `interval`)检查文件变化,
    /// 变化时自动重载配置并触发回调。
    ///
    /// 返回 `JoinHandle` 供调用者管理任务生命周期。
    pub fn start_watch(&mut self, interval: std::time::Duration) -> tokio::task::JoinHandle<()> {
        let mut self_clone = self.clone_state();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                if let Err(e) = self_clone.check_and_reload() {
                    tracing::warn!(error = %e, "配置热重载失败");
                }
            }
        })
    }

    /// 获取当前完整配置
    pub fn config(&self) -> Result<ChimeraConfig> {
        self.inner.to_chimera_config()
    }

    /// 委托给 LazyConfig 的 section getter
    pub fn nexus(&self) -> Result<&NexusConfig> {
        self.inner.nexus()
    }

    /// 委托给 LazyConfig 的 section getter
    pub fn quest(&self) -> Result<&QuestConfig> {
        self.inner.quest()
    }

    /// 委托给 LazyConfig 的 section getter
    pub fn monitoring(&self) -> Result<&MonitoringConfig> {
        self.inner.monitoring()
    }

    // 克隆状态用于后台任务
    fn clone_state(&self) -> Self {
        Self {
            inner: LazyConfig::new(Some(self.config_path.clone()))
                .unwrap_or_else(|_| LazyConfig::new(None).expect("default config always valid")),
            config_path: self.config_path.clone(),
            file_state: self.file_state.clone(),
            reload_callbacks: Vec::new(), // 回调不克隆
        }
    }
}

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

// === LazyConfig:14 section 按需懒加载(Task 4 / E1) ===
//
// WHY 懒加载:`load()` 通过 `figment.extract::<ChimeraConfig>()` 一次性反序列化
// 全部 14 个顶层 section。实际 CLI 运行往往只用其中一部分(如 `aether quest list`
// 不需要 evolution/monitoring),启动期解析未使用 section 是纯浪费。
// `LazyConfig` 仅在首次访问对应 getter 时,通过 `Figment::extract_inner` 按路径
// 反序列化该 section,未访问 section 零解析开销。

/// 单个 section 的 fallible 懒加载缓存。
///
/// 封装 [`OnceLock`] + "首次解析、后续缓存"模式,使 14 个 getter 各自缩为一行,
/// 避免样板重复。
///
/// 线程安全:基于 [`std::sync::OnceLock`](Rust 1.70+ 稳定),无 `unsafe`,
/// 契合 crate 级 `#![forbid(unsafe_code)]`。多线程并发访问同一 getter 时,
/// 至多一个线程执行解析,其余线程阻塞等待并共享结果。
struct LazySection<T> {
    /// 缓存解析结果(含错误)。
    ///
    /// WHY 缓存 `Err`:配置文件格式错误不会因重试自愈,缓存错误既避免
    /// 重复解析坏 section,也保证"懒加载只算一次"的语义一致。
    cell: OnceLock<Result<T, String>>,
}

impl<T> LazySection<T> {
    const fn new() -> Self {
        Self {
            cell: OnceLock::new(),
        }
    }

    /// 首次访问调用 `init` 解析并缓存;后续直接返回缓存。
    ///
    /// 返回值生命周期与 `&self` 绑定(由生命周期省略规则自动推导),
    /// 保证缓存引用跨多次调用有效。
    fn get_or_try_init<F>(&self, init: F) -> Result<&T>
    where
        F: FnOnce() -> std::result::Result<T, String>,
    {
        match self.cell.get_or_init(init) {
            Ok(value) => Ok(value),
            // WHY 重建为 owned anyhow::Error:get_or_init 借出 &String,
            // 但调用方链式 `?` 需要 owned `anyhow::Error`; anyhow::Error
            // 非 Clone,故用消息重建。backtrace 信息在配置加载场景非必需。
            Err(msg) => Err(anyhow::anyhow!("配置 section 解析失败: {msg}")),
        }
    }
}

/// 懒加载配置容器:持有 Figment provider,14 个 section 按需首次访问时解析。
///
/// 与 [`load`] 的区别:`load` 立即全量 `extract::<ChimeraConfig>()`;
/// [`LazyConfig::new`] 只构建 provider 链不 extract,各 getter 首次调用时
/// 通过 `Figment::extract_inner` 按 key 路径反序列化对应 section 并缓存。
///
/// 向后兼容:`LazyConfig` 是新增 API,既有 [`load`] / [`default_config`] /
/// [`ChimeraConfig`] 签名与行为均不变。
pub struct LazyConfig {
    /// 合并后的 Figment provider(defaults > file > env),供懒加载 extract。
    /// WHY 保留 provider 引用而非 extract 后丢弃:14 个 getter 需在各自首次
    /// 访问时从同一 provider 按路径取子树,必须长期持有 Figment。
    figment: Figment,
    nexus: LazySection<NexusConfig>,
    quest: LazySection<QuestConfig>,
    thinking_toggle: LazySection<ThinkingToggleConfig>,
    repo_wiki: LazySection<RepoWikiConfig>,
    model_router: LazySection<ModelRouterConfig>,
    osa: LazySection<OsaConfig>,
    kvbsr: LazySection<KvbsrConfig>,
    pvl: LazySection<PvlConfig>,
    mtpe: LazySection<MtpeConfig>,
    gqep: LazySection<GqepConfig>,
    seccore: LazySection<SeccoreConfig>,
    mcp: LazySection<McpConfig>,
    evolution: LazySection<EvolutionConfig>,
    monitoring: LazySection<MonitoringConfig>,
}

impl LazyConfig {
    /// 从配置路径构建懒加载容器。
    ///
    /// `config_path` 为 `None` 时使用 [`default_config_path`]。
    /// 配置文件不存在时不报错(与 [`load`] 一致),仅使用默认值 + 环境变量。
    ///
    /// WHY 只构建 provider 不 extract:14 section 的反序列化推迟到首次访问,
    /// 消除启动期未使用 section 的解析开销。
    pub fn new(config_path: Option<PathBuf>) -> Result<Self> {
        let path = config_path.unwrap_or_else(default_config_path);
        let figment = Figment::from(Serialized::defaults(ChimeraConfig::default()))
            .merge(Yaml::file(&path))
            .merge(Env::prefixed("AETHER_").split("__"));
        Ok(Self {
            figment,
            nexus: LazySection::new(),
            quest: LazySection::new(),
            thinking_toggle: LazySection::new(),
            repo_wiki: LazySection::new(),
            model_router: LazySection::new(),
            osa: LazySection::new(),
            kvbsr: LazySection::new(),
            pvl: LazySection::new(),
            mtpe: LazySection::new(),
            gqep: LazySection::new(),
            seccore: LazySection::new(),
            mcp: LazySection::new(),
            evolution: LazySection::new(),
            monitoring: LazySection::new(),
        })
    }

    /// Nexus 元信息(首次访问时按 `nexus` 路径解析并缓存)。
    pub fn nexus(&self) -> Result<&NexusConfig> {
        self.nexus
            .get_or_try_init(|| extract_section(&self.figment, "nexus"))
    }

    /// Quest 长期任务配置。
    pub fn quest(&self) -> Result<&QuestConfig> {
        self.quest
            .get_or_try_init(|| extract_section(&self.figment, "quest"))
    }

    /// 思考切换治理(TTG)配置。
    pub fn thinking_toggle(&self) -> Result<&ThinkingToggleConfig> {
        self.thinking_toggle
            .get_or_try_init(|| extract_section(&self.figment, "thinking_toggle"))
    }

    /// Repo Wiki 知识库配置。
    pub fn repo_wiki(&self) -> Result<&RepoWikiConfig> {
        self.repo_wiki
            .get_or_try_init(|| extract_section(&self.figment, "repo_wiki"))
    }

    /// 模型路由器配置。
    pub fn model_router(&self) -> Result<&ModelRouterConfig> {
        self.model_router
            .get_or_try_init(|| extract_section(&self.figment, "model_router"))
    }

    /// 全维稀疏架构(OSA)配置。
    pub fn osa(&self) -> Result<&OsaConfig> {
        self.osa
            .get_or_try_init(|| extract_section(&self.figment, "osa"))
    }

    /// KV 块语义路由器(KVBSR)配置。
    pub fn kvbsr(&self) -> Result<&KvbsrConfig> {
        self.kvbsr
            .get_or_try_init(|| extract_section(&self.figment, "kvbsr"))
    }

    /// 生产者-验证者循环(PVL)配置。
    pub fn pvl(&self) -> Result<&PvlConfig> {
        self.pvl
            .get_or_try_init(|| extract_section(&self.figment, "pvl"))
    }

    /// 多步预测执行(MTPE)配置。
    pub fn mtpe(&self) -> Result<&MtpeConfig> {
        self.mtpe
            .get_or_try_init(|| extract_section(&self.figment, "mtpe"))
    }

    /// 聚集执行协议(GQEP)配置。
    pub fn gqep(&self) -> Result<&GqepConfig> {
        self.gqep
            .get_or_try_init(|| extract_section(&self.figment, "gqep"))
    }

    /// 安全核心(SecCore)配置。
    pub fn seccore(&self) -> Result<&SeccoreConfig> {
        self.seccore
            .get_or_try_init(|| extract_section(&self.figment, "seccore"))
    }

    /// MCP 网格配置。
    pub fn mcp(&self) -> Result<&McpConfig> {
        self.mcp
            .get_or_try_init(|| extract_section(&self.figment, "mcp"))
    }

    /// 在线进化(GSOE)配置。
    pub fn evolution(&self) -> Result<&EvolutionConfig> {
        self.evolution
            .get_or_try_init(|| extract_section(&self.figment, "evolution"))
    }

    /// 监控(Prometheus/Grafana)配置。
    pub fn monitoring(&self) -> Result<&MonitoringConfig> {
        self.monitoring
            .get_or_try_init(|| extract_section(&self.figment, "monitoring"))
    }

    /// 聚合全部 14 section 为完整 [`ChimeraConfig`]。
    ///
    /// WHY 会触发所有未访问 section 的解析:仅用于需要完整配置的场景;
    /// 若只需部分 section,优先用对应 getter 避免全量解析。
    pub fn to_chimera_config(&self) -> Result<ChimeraConfig> {
        Ok(ChimeraConfig {
            nexus: self.nexus()?.clone(),
            quest: self.quest()?.clone(),
            thinking_toggle: self.thinking_toggle()?.clone(),
            repo_wiki: self.repo_wiki()?.clone(),
            model_router: self.model_router()?.clone(),
            osa: self.osa()?.clone(),
            kvbsr: self.kvbsr()?.clone(),
            pvl: self.pvl()?.clone(),
            mtpe: self.mtpe()?.clone(),
            gqep: self.gqep()?.clone(),
            seccore: self.seccore()?.clone(),
            mcp: self.mcp()?.clone(),
            evolution: self.evolution()?.clone(),
            monitoring: self.monitoring()?.clone(),
        })
    }
}

/// 按 key 路径从 Figment 提取单个 section(私有辅助)。
///
/// WHY 独立函数:14 个 getter 的解析逻辑完全相同
/// (`figment.extract_inner::<T>(path).map_err(to_string)`),
/// 提取为函数消除重复,且便于未来统一错误格式。
fn extract_section<T>(figment: &Figment, path: &str) -> std::result::Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    figment
        .extract_inner::<T>(path)
        .map_err(|e| format!("section `{path}`: {e}"))
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

    // ============================================================
    // P2-3: 热重载测试
    // ============================================================

    #[test]
    fn test_hot_reload_config_new() {
        let hot = HotReloadConfig::new(None);
        assert!(hot.is_ok());
    }

    #[test]
    fn test_hot_reload_config_path() {
        use tempfile::NamedTempFile;
        let file = NamedTempFile::with_suffix(".yaml").unwrap();
        std::fs::write(file.path(), "nexus:\n  version: \"1.0.0\"\n").unwrap();

        let mut hot = HotReloadConfig::new(Some(file.path().to_path_buf())).unwrap();
        assert!(hot.config().is_ok());
    }

    #[test]
    fn test_config_file_state_detects_change() {
        use tempfile::NamedTempFile;
        let file = NamedTempFile::with_suffix(".yaml").unwrap();
        std::fs::write(file.path(), "nexus:\n  version: \"1.0.0\"\n").unwrap();

        let state = ConfigFileState::from_path(file.path()).unwrap();
        assert!(!state.has_changed()); // 未修改

        // 修改文件
        std::fs::write(file.path(), "nexus:\n  version: \"2.0.0\"\n").unwrap();
        assert!(state.has_changed());
    }

    #[test]
    fn test_fnv_hash() {
        let h1 = fnv_hash(b"hello");
        let h2 = fnv_hash(b"hello");
        let h3 = fnv_hash(b"world");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }
}
