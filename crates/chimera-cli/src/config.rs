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
//! 2. 配置文件(默认 `~/.chimera/omega.yaml`,可由 `--config` 覆盖)
//! 3. 环境变量(前缀 `CHIMERA_`,嵌套用 `__` 分隔)
//! 4. CLI 参数(目前仅 `--config` 影响加载路径)
//!
//! ## 配置样例
//! - 简化样例见 `examples/config.sample.yaml` / `examples/config.sample.toml`
//! - 完整模板(含全部 14 个顶层 section)由 `chimera config init` 生成

// 类型定义 re-export:nexus-core 定义,L10 通过 re-export 保持向后兼容。
// trait impl(Serialize/Deserialize)是全局的,re-export 后自动随类型传播。
pub use nexus_core::config::*;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use figment::{
    providers::{Env, Format, Serialized, Yaml},
    Figment,
};

// === 配置加载逻辑(保留在 L10 chimera-cli) ===

/// 默认配置文件路径:`~/.chimera/omega.yaml`
///
/// 跨平台 home 目录展开:
/// - Unix: `$HOME/.chimera/omega.yaml`
/// - Windows: `%USERPROFILE%\.chimera\omega.yaml`
pub fn default_config_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".chimera").join("omega.yaml")
}

/// 返回内置默认配置(等价于 `ChimeraConfig::default()`)
pub fn default_config() -> ChimeraConfig {
    ChimeraConfig::default()
}

/// P2-3: 校验并规范化配置文件路径(防路径穿越越权读取)
///
/// 安全检查链:
/// 1. 展开 `~` 为用户 home 目录(便于用户输入)
/// 2. **拒绝含 `..` 组件的路径**(路径穿越防护核心,在规范化前检查)
/// 3. 规范化 `.` 组件(消除冗余的当前目录引用)
/// 4. 若文件存在,canonicalize 解析符号链接(审计追踪)
///
/// # 安全模型
///
/// 采用 **OWASP 路径穿越防护标准做法**:配置文件路径中根本不应出现 `..` 组件。
/// 这比"规范化后检查残留 `..`"更安全,因为后者可能因 `..` 被成功解析
/// (如 `~/../etc/passwd` → `C:\Users\etc\passwd`)而绕过检查。
///
/// - **显式绝对路径**(如 `--config /opt/myapp/config.yaml`):允许,用户明确指定
/// - **Tilde 展开后穿越**(如 `~/../etc/passwd`):拒绝,`..` 穿越到 home 之外
/// - **相对路径穿越**(如 `../../etc/passwd`):拒绝,`..` 穿越到 cwd 之外
/// - **路径内合法 `..`**(如 `foo/bar/..`):拒绝(用户应直接写 `foo`,YAGNI)
/// - **符号链接**:若文件存在,canonicalize 解析后记录日志(审计追踪)
///
/// # 时间复杂度
///
/// O(n),n = 路径组件数。canonicalize 在文件存在时触发一次 syscall。
///
/// # 错误
///
/// - 路径含 `..` 组件(穿越尝试,无论是否可解析)
/// - canonicalize 失败(文件存在但无法解析符号链接,如权限不足)
pub fn validate_config_path(path: &Path) -> Result<PathBuf> {
    // 步骤 1: 展开 tilde(~ → home 目录)
    let expanded = expand_tilde(path);

    // 步骤 2: 拒绝含 `..` 组件的路径(路径穿越防护核心)
    // WHY 在规范化前检查:规范化会"成功解析" `..`,导致 `~/../etc/passwd`
    // 变成 `<home_parent>/etc/passwd`,反而绕过检查。直接拒绝 `..` 组件
    // 是 OWASP 推荐的标准做法,消除解析绕过风险。
    if expanded
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        anyhow::bail!(
            "配置路径包含非法的 '..' 穿越组件: {} (原始: {}). \
             请使用绝对路径或当前目录下的相对路径,禁止通过 '..' 访问上级目录",
            expanded.display(),
            path.display()
        );
    }

    // 步骤 3: 规范化 `.` 组件(消除冗余的当前目录引用,如 `./config.yaml` → `config.yaml`)
    let normalized = normalize_path(&expanded);

    // 步骤 4: 文件存在时 canonicalize(解析符号链接,审计追踪)
    if normalized.exists() {
        let canonical = normalized
            .canonicalize()
            .with_context(|| format!("路径规范化(canonicalize)失败: {}", normalized.display()))?;
        tracing::debug!(
            original = %path.display(),
            canonical = %canonical.display(),
            "配置路径已规范化(含符号链接解析)"
        );
        Ok(canonical)
    } else {
        tracing::debug!(
            original = %path.display(),
            normalized = %normalized.display(),
            "配置文件不存在,使用规范化路径(无符号链接解析)"
        );
        Ok(normalized)
    }
}

/// 展开 `~` 为用户 home 目录
///
/// 支持以下形式:
/// - `~` → home 目录
/// - `~/path/to/file` → home/path/to/file
/// - `~\path\to\file` → home\path\to\file (Windows)
///
/// 不展开 `~user` 形式(需 getpwuid,跨平台复杂,YAGNI)
fn expand_tilde(path: &Path) -> PathBuf {
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return path.to_path_buf(),
    };

    if path_str == "~" {
        return home_dir();
    }

    // Unix 风格: ~/...
    if let Some(rest) = path_str.strip_prefix("~/") {
        return home_dir().join(rest);
    }

    // Windows 风格: ~\...
    if let Some(rest) = path_str.strip_prefix("~\\") {
        return home_dir().join(rest);
    }

    path.to_path_buf()
}

/// 获取用户 home 目录(与 default_config_path 一致的跨平台逻辑)
fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string())
        .into()
}

/// 规范化路径组件(解析 `.` 和 `..`)
///
/// 逐组件处理:
/// - `.` (CurDir):跳过
/// - `..` (ParentDir):弹出最后一个 Normal 组件;若无法弹出(RootDir/PrefixDir)则保留 `..`
/// - 其他(RootDir/PrefixDir/Normal):直接追加
///
/// 返回的路径可能仍含 `..` 组件(当 `..` 数量超过 Normal 组件时),
/// 调用方应检查并拒绝此类路径(路径穿越防护)。
fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => { /* 跳过 `.` */ }
            std::path::Component::ParentDir => {
                // 尝试弹出最后一个 Normal 组件
                if result
                    .components()
                    .next_back()
                    .is_some_and(|c| matches!(c, std::path::Component::Normal(_)))
                {
                    result.pop();
                } else {
                    // 无法弹出(RootDir/PrefixDir 或已有 `..`),保留 `..` 供调用方检测
                    result.push("..");
                }
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

/// 从多源加载配置(优先级:CLI > env > file > defaults)
///
/// `config_path` 为 `None` 时使用 [`default_config_path`]。
/// 配置文件不存在时不报错,仅使用默认值 + 环境变量。
///
/// # 安全
///
/// 调用 [`validate_config_path`] 对用户提供的路径进行安全校验:
/// - 展开 `~` 为 home 目录
/// - 规范化路径组件(解析 `.` 和 `..`)
/// - 拒绝含未解析 `..` 的路径穿越尝试
/// - 文件存在时 `canonicalize` 解析符号链接(审计追踪)
///
/// 校验失败立即返回错误,阻止后续 figment 加载。
pub fn load(config_path: Option<PathBuf>) -> Result<ChimeraConfig> {
    let raw_path = config_path.unwrap_or_else(default_config_path);

    // P2-3: 路径安全校验(防路径穿越越权读取)
    // 在 figment 加载前完成校验,确保恶意路径不会触及文件 IO。
    let path = validate_config_path(&raw_path)?;

    // 优先级链:defaults -> file -> env(后者覆盖前者)
    // 注:CLI 参数目前仅影响 config_path,未直接进入 Figment;
    //     后续可扩展 CLI override provider 以支持 --strategy 等参数。
    let figment = Figment::from(Serialized::defaults(ChimeraConfig::default()))
        .merge(Yaml::file(&path))
        .merge(Env::prefixed("CHIMERA_").split("__"));

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
    r#"# ~/.chimera/omega.yaml
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
  db_path: "~/.chimera/wiki.db"
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
  mutation_pool_path: "~/.chimera/evolution/mutations/"
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
      expr: "chimera_capability_current < 0.1"
      for: "1m"
    - name: "HighOrphanRate"
      expr: "rate(chimera_orphan_calls_total[5m]) > 0"
      for: "1m"
    - name: "BudgetAlert"
      expr: "chimera_daily_cost / chimera_daily_budget > 0.8"
      for: "5m"
    - name: "RedTeamVulnerability"
      expr: "chimera_red_team_vulnerabilities > 0"
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
            .merge(Env::prefixed("CHIMERA_").split("__"));
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

    // === P2-3: 路径安全校验测试(防路径穿越越权读取) ===

    #[test]
    fn test_expand_tilde_bare() {
        // `~` 单独使用 → 展开为 home 目录
        let expanded = expand_tilde(std::path::Path::new("~"));
        // home 目录应非空(至少包含 HOME 或 USERPROFILE 之一)
        assert!(expanded.components().count() >= 1);
        // 不应再含 `~` 字符
        assert!(!expanded.to_string_lossy().contains('~'));
    }

    #[test]
    fn test_expand_tilde_unix_style() {
        // `~/foo/bar` → home/foo/bar
        let expanded = expand_tilde(std::path::Path::new("~/foo/bar"));
        let s = expanded.to_string_lossy();
        assert!(!s.contains('~'));
        assert!(s.ends_with("foo/bar") || s.ends_with("foo\\bar"));
    }

    #[test]
    fn test_expand_tilde_windows_style() {
        // `~\foo\bar` → home\foo\bar(Windows 风格)
        let expanded = expand_tilde(std::path::Path::new("~\\foo\\bar"));
        let s = expanded.to_string_lossy();
        assert!(!s.contains('~'));
    }

    #[test]
    fn test_expand_tilde_no_tilde() {
        // 不以 `~` 开头 → 原样返回
        let path = std::path::Path::new("/etc/passwd");
        let expanded = expand_tilde(path);
        assert_eq!(expanded, path.to_path_buf());
    }

    #[test]
    fn test_expand_tilde_tildeuser_not_expanded() {
        // `~user` 形式不展开(YAGNI,跨平台不支持)
        let path = std::path::Path::new("~root/config");
        let expanded = expand_tilde(path);
        // 应原样返回(不展开 ~user)
        assert_eq!(expanded, path.to_path_buf());
    }

    #[test]
    fn test_normalize_path_dots_only() {
        // `.` 应被跳过
        let normalized = normalize_path(std::path::Path::new("./foo/./bar"));
        // 不应含 `.` 组件(规范化后 CurDir 应被消除)
        assert!(!normalized
            .components()
            .any(|c| c == std::path::Component::CurDir));
    }

    #[test]
    fn test_normalize_path_parent_dir_resolvable() {
        // `foo/bar/..` → `foo`
        let normalized = normalize_path(std::path::Path::new("foo/bar/.."));
        let s = normalized.to_string_lossy();
        // `..` 应被弹出,结果为 `foo`
        assert_eq!(s, "foo");
    }

    #[test]
    fn test_normalize_path_parent_dir_not_resolvable() {
        // `../..` → 保留 `../..`(无法弹出,留给调用方拒绝)
        let normalized = normalize_path(std::path::Path::new("../.."));
        // 应仍含 `..` 组件
        let has_parent = normalized
            .components()
            .any(|c| c == std::path::Component::ParentDir);
        assert!(has_parent, "未解析的 `..` 应保留以供调用方检测");
    }

    #[test]
    fn test_validate_config_path_rejects_traversal() {
        // `~/../etc/passwd` → 展开后含 `..` 穿越组件,应被拒绝
        let result = validate_config_path(std::path::Path::new("~/../etc/passwd"));
        assert!(
            result.is_err(),
            "路径穿越尝试应被拒绝,实际结果: {:?}",
            result
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains(".."),
            "错误信息应提及 `..` 穿越组件: {}",
            err_msg
        );
    }

    #[test]
    fn test_validate_config_path_rejects_relative_traversal() {
        // `../../etc/passwd` → 相对路径穿越,应被拒绝
        let result = validate_config_path(std::path::Path::new("../../etc/passwd"));
        assert!(result.is_err(), "相对路径穿越应被拒绝");
    }

    #[test]
    fn test_validate_config_path_accepts_normal_relative() {
        // `config.yaml`(当前目录下)→ 允许
        let result = validate_config_path(std::path::Path::new("config.yaml"));
        assert!(result.is_ok(), "当前目录下的相对路径应允许: {:?}", result);
    }

    #[test]
    fn test_validate_config_path_accepts_absolute() {
        // 绝对路径(可能不存在)→ 允许
        // 使用一个肯定不存在的路径,避免与真实文件冲突
        let path = if cfg!(windows) {
            std::path::Path::new("C:\\nonexistent\\config.yaml")
        } else {
            std::path::Path::new("/nonexistent/config.yaml")
        };
        let result = validate_config_path(path);
        assert!(result.is_ok(), "绝对路径应允许: {:?}", result);
    }

    #[test]
    fn test_validate_config_path_accepts_tilde() {
        // `~/.chimera/omega.yaml` → 展开后允许(假设 home 目录存在)
        let result = validate_config_path(std::path::Path::new("~/.chimera/omega.yaml"));
        assert!(result.is_ok(), "Tilde 展开后的路径应允许: {:?}", result);
        // 结果不应含 `~`
        let path = result.unwrap();
        assert!(!path.to_string_lossy().contains('~'));
    }

    #[test]
    fn test_validate_config_path_canonicalizes_existing() {
        // 创建临时文件,验证 canonicalize 解析符号链接
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join("chimera_test_validate_config.yaml");
        std::fs::write(&temp_file, "nexus:\n  version: \"test\"\n").unwrap();

        let result = validate_config_path(&temp_file);
        assert!(result.is_ok(), "存在的文件应成功校验: {:?}", result);

        // 清理临时文件
        let _ = std::fs::remove_file(&temp_file);
    }

    #[test]
    fn test_load_rejects_traversal_path() {
        // 集成测试:load 函数应拒绝路径穿越
        let result = load(Some(std::path::PathBuf::from("~/../etc/passwd")));
        assert!(result.is_err(), "load 应拒绝路径穿越尝试: {:?}", result);
    }

    #[test]
    fn test_load_accepts_nonexistent_normal_path() {
        // 集成测试:load 应允许不存在的正常路径(配置文件可选)
        let path = if cfg!(windows) {
            std::path::PathBuf::from("C:\\nonexistent\\chimera\\omega.yaml")
        } else {
            std::path::PathBuf::from("/nonexistent/chimera/omega.yaml")
        };
        let result = load(Some(path));
        // 配置文件不存在时,figment 使用默认值 + env,应成功
        assert!(
            result.is_ok(),
            "不存在的正常路径应允许加载(使用默认值): {:?}",
            result
        );
    }
}
