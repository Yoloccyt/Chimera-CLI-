//! TUI 配置类型 — 主题与布局
//!
//! 对应架构层:L10 Interface
//! 对应创新点:无(用户交互入口)
//!
//! # 设计决策(WHY)
//! - `theme` 默认 Dark:终端应用常用深色主题,与多数终端配色兼容
//! - `main_panel_ratio` 默认 0.7:主面板占 70%,侧边栏占 30%,保证主内容可读性
//! - `log_panel_height` 默认 8:日志面板 8 行,足够显示最近日志不占用过多空间
//!
//! # 模块组织
//! - `tui_bible` 子模块:基于 Figment 多源合并的"设计手册"配置加载器
//!   (Task 3.2,v1.8-omega),通过 `TuiBible::load()` 提供 4 源合并:
//!   默认 < `~/.chimera/tui_bible.yaml` < env `CHIMERA_BIBLE_*` < CLI 参数

pub mod tui_bible;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::TuiError;
use crate::types::SortMode;

// ============================================================
// 主题枚举
// ============================================================

/// TUI 主题 — 颜色方案
///
/// WHY enum:主题是离散选择,非连续值,enum 语义清晰。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Theme {
    /// 深色主题(默认)
    Dark,
    /// 浅色主题
    Light,
    /// 高对比度主题(色盲 + 高亮环境用户)
    ///
    /// WHY HighContrast:为色盲用户与强光环境提供最大对比度,
    /// 纯黑背景 + 纯白前景 + 高饱和度强调色,牺牲美观换取可读性。
    HighContrast,
}

impl Theme {
    /// 返回主题的人类可读名称
    pub fn as_str(&self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
            Theme::HighContrast => "high_contrast",
        }
    }

    /// 循环切换到下一个主题(Dark → Light → HighContrast → Dark)
    ///
    /// WHY 循环顺序:Dark(默认)→ Light(白天/明亮环境)→ HighContrast(色盲/强光)→ Dark
    pub fn next(&self) -> Self {
        match self {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::HighContrast,
            Theme::HighContrast => Theme::Dark,
        }
    }

    /// 返回该主题的默认颜色方案
    pub fn colors(&self) -> ThemeColors {
        match self {
            Theme::Dark => ThemeColors {
                foreground: ColorKind::White,
                background: ColorKind::Black,
                accent: ColorKind::Cyan,
                warning: ColorKind::Yellow,
                error: ColorKind::Red,
                success: ColorKind::Green,
            },
            Theme::Light => ThemeColors {
                foreground: ColorKind::Black,
                background: ColorKind::White,
                accent: ColorKind::Blue,
                warning: ColorKind::BrightYellow,
                error: ColorKind::BrightRed,
                success: ColorKind::BrightGreen,
            },
            Theme::HighContrast => ThemeColors {
                // WHY 纯黑白 + 高饱和度强调色:色盲用户 + 强光环境最大对比度
                foreground: ColorKind::White,
                background: ColorKind::Black,
                accent: ColorKind::BrightYellow,
                warning: ColorKind::BrightYellow,
                error: ColorKind::BrightRed,
                success: ColorKind::BrightGreen,
            },
        }
    }
}

impl std::fmt::Display for Theme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================
// 主题颜色方案(P6.1)
// ============================================================

/// 颜色种类(不依赖 ratatui,保持配置层纯净)
///
/// WHY 不直接用 ratatui::style::Color:config.rs 是配置层,
/// 不应依赖 UI 框架。app.rs 在使用时转换为 ratatui::style::Color。
///
/// WHY 派生 Serialize/Deserialize:`ColorScheme` 字段类型为 `Option<ColorKind>`,
/// `ColorScheme` 派生了 serde,因此 `ColorKind` 必须同步派生,否则
/// `#[derive(Deserialize)]` 缺少 trait bound 编译失败(E0277)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorKind {
    /// 黑色
    Black,
    /// 白色
    White,
    /// 红色
    Red,
    /// 绿色
    Green,
    /// 黄色
    Yellow,
    /// 蓝色
    Blue,
    /// 青色
    Cyan,
    /// 品红
    Magenta,
    /// 浅灰
    LightGray,
    /// 深灰
    DarkGray,
    /// 亮红(高饱和度)
    BrightRed,
    /// 亮绿(高饱和度)
    BrightGreen,
    /// 亮黄(高饱和度)
    BrightYellow,
    /// 亮蓝(高饱和度)
    BrightBlue,
    /// 亮青(高饱和度)
    BrightCyan,
    /// 亮品红(高饱和度)
    BrightMagenta,
}

/// 主题颜色方案 — 各主题的离散颜色预设
///
/// WHY 独立结构体:主题是离散预设(Dark/Light/HighContrast),
/// 颜色方案是细粒度覆盖(P6.3 ColorScheme)。ThemeColors 提供主题级
/// 预设,P6.3 的 ColorScheme 在此基础上允许用户细粒度覆盖。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeColors {
    /// 前景色(文字颜色)
    pub foreground: ColorKind,
    /// 背景色
    pub background: ColorKind,
    /// 强调色(标题/选中)
    pub accent: ColorKind,
    /// 警告色
    pub warning: ColorKind,
    /// 错误色
    pub error: ColorKind,
    /// 成功色
    pub success: ColorKind,
}

// ============================================================
// 颜色方案覆盖(P6.3)
// ============================================================

/// 颜色方案覆盖 — 用户对主题颜色的细粒度定制
///
/// WHY ColorScheme:`Theme` 是离散预设(Dark/Light/HighContrast),每个主题
/// 有一套完整的 `ThemeColors`。但用户可能只想微调某个颜色(如把 accent 改成
/// 亮蓝),而不想整个换主题。`ColorScheme` 提供这种细粒度覆盖能力:每个字段
/// 是 `Option<ColorKind>`,None 表示"用主题预设",Some 表示"用户覆盖"。
///
/// WHY `#[derive(Default)]`:所有字段为 `Option<T>`,`Option::default()` 返回
/// `None`,因此 derive 自动生成"全 None"的默认值,与"不覆盖任何颜色"语义一致。
/// 无需手写 `impl Default`。
///
/// # 配置文件示例
/// ```yaml
/// tui:
///   theme: Dark
///   colors:
///     accent: BrightBlue
///     warning: BrightYellow
/// ```
/// 上述配置只覆盖 accent 和 warning,其余颜色沿用 Dark 主题预设。
/// 颜色名用 PascalCase(与 `ColorKind` 变体名一致,如 `BrightBlue`/`Cyan`)。
///
/// # 解析流程
/// `ColorScheme::resolve(theme)` 合并主题预设 + 用户覆盖:
/// 1. 取 `theme.colors()` 作为基础
/// 2. 逐字段用 `ColorScheme` 的 Some 值覆盖 None 值
/// 3. 返回最终 `ThemeColors` 供渲染层使用
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorScheme {
    /// 前景色覆盖(None = 用主题预设)
    pub foreground: Option<ColorKind>,
    /// 背景色覆盖(None = 用主题预设)
    pub background: Option<ColorKind>,
    /// 强调色覆盖(None = 用主题预设)
    pub accent: Option<ColorKind>,
    /// 警告色覆盖(None = 用主题预设)
    pub warning: Option<ColorKind>,
    /// 错误色覆盖(None = 用主题预设)
    pub error: Option<ColorKind>,
    /// 成功色覆盖(None = 用主题预设)
    pub success: Option<ColorKind>,
}

impl ColorScheme {
    /// 返回指定主题的默认 ColorScheme(所有字段为 None,表示完全用主题预设)
    ///
    /// WHY 接收 theme 参数但内部不使用:`ColorScheme` 的默认值是"不覆盖任何
    /// 颜色",与主题无关。但保持 `default_for_theme(theme)` 签名是为了:
    /// 1. API 语义清晰:明确表示"这是某主题的默认覆盖方案"
    /// 2. 未来扩展:某些主题可能有特殊的默认覆盖(如 HighContrast 默认
    ///    覆盖 accent 为 BrightYellow 以增强对比度)
    pub fn default_for_theme(_theme: Theme) -> Self {
        Self::default()
    }

    /// 合并主题预设 + 用户覆盖,返回最终渲染用的 ThemeColors
    ///
    /// 解析顺序:用户覆盖(Some)优先于主题预设(Theme::colors)。
    /// 即使用户设置了 `theme: dark` + `colors.accent: bright_blue`,
    /// 最终 accent 采用 bright_blue,其余沿用 Dark 主题预设。
    pub fn resolve(&self, theme: Theme) -> ThemeColors {
        let base = theme.colors();
        ThemeColors {
            foreground: self.foreground.unwrap_or(base.foreground),
            background: self.background.unwrap_or(base.background),
            accent: self.accent.unwrap_or(base.accent),
            warning: self.warning.unwrap_or(base.warning),
            error: self.error.unwrap_or(base.error),
            success: self.success.unwrap_or(base.success),
        }
    }
}

// ============================================================
// TUI 配置
// ============================================================

/// TUI 配置 — 主题与布局参数
///
/// 所有字段均有合理默认值,可通过 `Default::default()` 快速构造。
/// 构造 `TuiApp` 时会调用 `validate()` 校验配置合法性。
///
/// WHY `#[serde(default)]`:配置文件(`~/.aether/omega.yaml`)只需提供
/// 用户想覆盖的字段,其余字段回退到 `TuiConfig::default()` 的预设值。
/// 这与 Figment 四源合并(CLAUDE.md §4)一致 — 内置默认 → 配置文件 → 环境变量 → CLI。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TuiConfig {
    /// 主题(颜色方案)
    pub theme: Theme,
    /// 颜色方案覆盖(P6.3)— 用户对主题颜色的细粒度定制
    ///
    /// WHY 默认全 None:`ColorScheme::default()` 不覆盖任何颜色,
    /// 完全沿用 `theme` 预设。用户在配置文件 `tui.colors` 节设置
    /// 某字段才会生效。渲染层通过 `colors.resolve(theme)` 获取最终颜色。
    pub colors: ColorScheme,
    /// 主面板占比(范围 0.0-1.0,表示主面板占水平方向的比例)
    pub main_panel_ratio: f32,
    /// 日志面板高度(行数)
    pub log_panel_height: u16,
    /// 是否启用鼠标支持
    pub enable_mouse: bool,
    /// 刷新率(帧/秒)
    pub frame_rate: u16,
    /// tick 间隔(毫秒),控制 DataPipeline 快照频率(P4.3 性能优化)
    ///
    /// WHY 250ms 默认:平衡响应性与 CPU 开销,4 Hz 更新足够面板展示
    /// 实时指标;过低(如 50ms)会导致 event-bus 频繁加锁,
    /// 过高(如 1000ms)会让操作员感觉面板"卡顿"。
    pub tick_interval_ms: u16,
    /// 快照间隔(秒),P7 历史回放用(P7 接口占位,v1.8+ 实现)
    ///
    /// WHY 30s 默认:历史回放粒度,过细会占用大量内存,过粗无法回看细节。
    pub snapshot_interval_s: u16,
    /// 事件流最大保留条数(P2.2 EventStream 面板需要万级)
    ///
    /// WHY 256 默认:与现有 `DataSourceConfig::max_event_history` 默认值
    /// 保持一致;P2.2 EventStream 实现万级虚拟滚动时,可上调至 10000+。
    pub max_event_history: usize,
    /// 快照最大保留数(P7 接口占位,v1.8+ 实现)
    ///
    /// WHY 100 默认:30s × 100 = 50 分钟历史回放窗口,覆盖典型调试场景。
    pub max_snapshots: usize,
    // === v1.8-omega: 监控/任务/系统信息扩展字段(Task 1.4) ===
    /// 是否启用实时趋势图(默认 false — 不破坏既有 resource_monitor 面板断言)
    ///
    /// WHY 默认 false:spec §MODIFIED Requirements 迁移路径明确指出"默认关闭,
    /// 需用户显式开启以避免破坏既有 `resource_monitor_panel_test.rs` 断言"。
    /// 用户在配置文件中设置 `enable_trend_charts: true` 后,ResourceMonitorPanel
    /// 才渲染 sparkline 趋势图 + 阈值告警颜色。
    pub enable_trend_charts: bool,
    /// 指标采样间隔(毫秒,默认 1000ms = 1Hz)
    ///
    /// 控制 ResourceMonitorPanel 趋势图的采样频率,1Hz 与 5 分钟窗口 300 点对齐。
    /// WHY 1000ms:平衡实时性与存储开销;过低(<500ms)导致 CPU/IO 压力,
    /// 过高(>5000ms)丢失细节。validate() 限制 [100, 60000]。
    pub metrics_sample_interval_ms: u64,
    /// 指标历史保留天数(默认 7 天)
    ///
    /// 控制 metrics_history.sqlite 的数据保留期,过期数据由后台清理任务删除。
    /// WHY 7 天:一周历史覆盖典型运维诊断周期(周末复盘 + 工作日回溯)。
    pub metrics_history_retention_days: u32,
    /// 任务管理面板默认排序模式(默认 Priority)
    ///
    /// 决定 TaskManagerPanel 启动时的 Quest 列表排序方式,
    /// 用户可在面板内通过快捷键循环切换(SortMode::next())。
    pub task_manager_default_sort: SortMode,
    /// 系统信息刷新间隔(毫秒,默认 5000ms = 5s)
    ///
    /// 控制 SysinfoPanel 进程信息(PID/RSS/线程数/文件句柄数)的刷新频率,
    /// 主机信息(OS/CPU/内存)仅在面板首次打开时采集一次。
    /// WHY 5000ms:5s 刷新足够展示进程变化趋势,避免 sysinfo 调用过于频繁
    /// 导致 CPU 占用(spec §Scenario "系统信息面板启动加载")。
    pub sysinfo_refresh_interval_ms: u64,
    /// 是否启用视图状态持久化（默认 true）
    ///
    /// WHY 默认 true:退出时保存布局模式/过滤器等用户偏好,
    /// 下次启动自动恢复,减少用户重复操作。用户可通过配置文件关闭。
    pub persist_state: bool,
    /// 状态文件路径（默认 ~/.chimera/tui_state.yaml）
    pub state_file_path: PathBuf,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            // WHY Dark:终端应用常用深色主题,与多数终端配色兼容
            theme: Theme::Dark,
            // P6.3:默认不覆盖任何颜色,完全使用 Dark 主题预设
            colors: ColorScheme::default(),
            // WHY 0.7:主面板 70%,侧边栏 30%,保证主内容可读性
            main_panel_ratio: 0.7,
            // WHY 8:日志面板 8 行,足够显示最近日志不占用过多空间
            log_panel_height: 8,
            enable_mouse: true,
            // WHY 60:60 FPS,流畅渲染且不过度消耗 CPU
            frame_rate: 60,
            // P2.4 默认值见字段文档
            tick_interval_ms: 250,
            snapshot_interval_s: 30,
            max_event_history: 256,
            max_snapshots: 100,
            // === v1.8-omega 扩展字段默认值(Task 1.4) ===
            // 与 spec §Requirement / §MODIFIED Requirements 对齐
            enable_trend_charts: false,
            metrics_sample_interval_ms: 1000,
            metrics_history_retention_days: 7,
            task_manager_default_sort: SortMode::default(), // = Priority
            sysinfo_refresh_interval_ms: 5000,
            persist_state: true,
            state_file_path: Self::default_state_path(),
        }
    }
}

// ============================================================
// 持久化配置结构(Task 15 TDD-GREEN)
// ============================================================

/// 持久化配置结构 — 只包含需要保存到文件的字段
///
/// WHY 单独结构: TuiConfig 有 10 个字段,但只有 4 个需要持久化
/// (theme/colors/main_panel_ratio/tick_interval_ms)。运行时字段
/// (frame_rate/enable_mouse/max_event_history/max_snapshots/
/// snapshot_interval_s/log_panel_height)不应持久化,因为它们与
/// 硬件环境或性能调优相关,每次启动应使用默认值,持久化会导致
/// 跨环境配置污染。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistentConfig {
    theme: Theme,
    colors: ColorScheme,
    main_panel_ratio: f32,
    tick_interval_ms: u16,
}

impl TuiConfig {
    /// 校验配置合法性
    ///
    /// WHY:在构造 TuiApp 时调用,提前暴露配置错误。
    ///
    /// # 校验规则
    /// - `main_panel_ratio` ∈ (0.0, 1.0)(不能为 0 或 1,需留侧边栏空间)
    /// - `log_panel_height` >= 3(至少 3 行:边框 + 1 行内容)
    /// - `frame_rate` >= 1
    /// - `tick_interval_ms` ∈ [100, 1000](过短导致 CPU 占用高,过长面板卡顿)
    /// - `snapshot_interval_s` >= 1(P7 历史回放最小粒度)
    /// - `max_event_history` >= 64(EventStream 面板最小可用容量)
    /// - `max_snapshots` >= 10(P7 历史回放最小回看窗口)
    pub fn validate(&self) -> Result<(), TuiError> {
        if self.main_panel_ratio.is_nan() || !(0.0..=1.0).contains(&self.main_panel_ratio) {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "main_panel_ratio must be in [0.0, 1.0], got {}",
                    self.main_panel_ratio
                ),
            });
        }
        if self.main_panel_ratio == 0.0 || self.main_panel_ratio == 1.0 {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "main_panel_ratio must be in (0.0, 1.0) exclusive, got {} (0 or 1 leaves no room for sidebar)",
                    self.main_panel_ratio
                ),
            });
        }
        if self.log_panel_height < 3 {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "log_panel_height must be >= 3 (border + 1 line content), got {}",
                    self.log_panel_height
                ),
            });
        }
        if self.frame_rate == 0 {
            return Err(TuiError::ConfigError {
                detail: "frame_rate must be >= 1".into(),
            });
        }
        // P2.4 新增校验
        if !(100..=1000).contains(&self.tick_interval_ms) {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "tick_interval_ms must be in [100, 1000], got {} (too low: CPU overhead; too high: panel feels frozen)",
                    self.tick_interval_ms
                ),
            });
        }
        if self.snapshot_interval_s < 1 {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "snapshot_interval_s must be >= 1, got {}",
                    self.snapshot_interval_s
                ),
            });
        }
        if self.max_event_history < 64 {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "max_event_history must be >= 64 (EventStream panel minimum), got {}",
                    self.max_event_history
                ),
            });
        }
        if self.max_snapshots < 10 {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "max_snapshots must be >= 10 (P7 history replay minimum), got {}",
                    self.max_snapshots
                ),
            });
        }
        // === v1.8-omega 扩展字段校验(Task 1.4 REFACTOR) ===
        // 校验规则与字段文档保持一致,避免文档/代码漂移
        if !(100..=60_000).contains(&self.metrics_sample_interval_ms) {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "metrics_sample_interval_ms must be in [100, 60000], got {} (too low: CPU/IO pressure; too high: loses detail)",
                    self.metrics_sample_interval_ms
                ),
            });
        }
        if self.metrics_history_retention_days < 1 {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "metrics_history_retention_days must be >= 1, got {} (0 days would immediately purge all history)",
                    self.metrics_history_retention_days
                ),
            });
        }
        if self.sysinfo_refresh_interval_ms < 100 {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "sysinfo_refresh_interval_ms must be >= 100, got {} (too low: sysinfo refresh is heavy)",
                    self.sysinfo_refresh_interval_ms
                ),
            });
        }
        // SortMode 是 enum,无范围可言,无需校验(serde 反序列化已保证有效性)
        Ok(())
    }

    // ============================================================
    // TuiConfig 持久化(Task 15 TDD-GREEN)
    // ============================================================

    /// 保存配置到 YAML 文件
    ///
    /// 持久化字段: theme / colors / main_panel_ratio / tick_interval_ms
    /// 不持久化: frame_rate / enable_mouse / max_event_history /
    ///           max_snapshots / snapshot_interval_s / log_panel_height
    ///
    /// WHY 只持久化 4 个字段: 运行时字段与硬件环境或性能调优相关,
    /// 每次启动应使用默认值,持久化会导致跨环境配置污染。
    pub fn save_to_file(&self, path: &std::path::Path) -> Result<(), crate::error::TuiError> {
        let persistent = PersistentConfig {
            theme: self.theme,
            colors: self.colors.clone(),
            main_panel_ratio: self.main_panel_ratio,
            tick_interval_ms: self.tick_interval_ms,
        };

        // 确保父目录存在(如 ~/.chimera/),避免写入时因目录缺失失败
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| crate::error::TuiError::ConfigError {
                detail: format!("Failed to create config directory: {}", e),
            })?;
        }

        let yaml = serde_yaml::to_string(&persistent).map_err(|e| {
            crate::error::TuiError::ConfigError {
                detail: format!("Failed to serialize config: {}", e),
            }
        })?;

        std::fs::write(path, yaml).map_err(|e| crate::error::TuiError::ConfigError {
            detail: format!("Failed to write config file: {}", e),
        })?;

        Ok(())
    }

    /// 从 YAML 文件加载配置
    ///
    /// - 文件不存在时返回 Ok(TuiConfig::default()),不报错
    /// - 文件损坏时返回 Err(TuiError::ConfigError)
    ///
    /// WHY 文件不存在返回默认值: 首次启动时配置文件尚未创建,
    /// 应静默回退到默认配置而非报错,符合 Figment 多源合并的
    /// "内置默认 → 配置文件"优先级语义。
    pub fn load_from_file(path: &std::path::Path) -> Result<Self, crate::error::TuiError> {
        // 文件不存在时返回默认配置,不报错(首次启动场景)
        if !path.exists() {
            return Ok(TuiConfig::default());
        }

        let content =
            std::fs::read_to_string(path).map_err(|e| crate::error::TuiError::ConfigError {
                detail: format!("Failed to read config file: {}", e),
            })?;

        let persistent: PersistentConfig =
            serde_yaml::from_str(&content).map_err(|e| crate::error::TuiError::ConfigError {
                detail: format!("Failed to parse config YAML: {}", e),
            })?;

        // 用加载的持久化字段覆盖默认值,运行时字段保持默认(struct update 语法)
        let config = TuiConfig {
            theme: persistent.theme,
            colors: persistent.colors,
            main_panel_ratio: persistent.main_panel_ratio,
            tick_interval_ms: persistent.tick_interval_ms,
            ..Default::default()
        };

        Ok(config)
    }

    /// 返回默认配置文件路径
    ///
    /// - Linux/macOS: ~/.chimera/tui.yaml
    /// - Windows: %USERPROFILE%\.chimera\tui.yaml
    ///
    /// WHY 优先 HOME 回退 USERPROFILE: Unix 系统使用 HOME,
    /// Windows 使用 USERPROFILE。回退到 "." 保证极端环境下不 panic,
    /// 虽然路径可能不合理但调用方可检测。
    pub fn default_path() -> std::path::PathBuf {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());

        std::path::PathBuf::from(home)
            .join(".chimera")
            .join("tui.yaml")
    }

    /// 返回默认状态文件路径 ~/.chimera/tui_state.yaml
    ///
    /// - Linux/macOS: ~/.chimera/tui_state.yaml
    /// - Windows: %USERPROFILE%\.chimera\tui_state.yaml
    ///
    /// WHY 与 `default_path` 共享同一目录:配置文件和状态文件应放在同一
    /// 配置目录下,便于用户备份/迁移/清理。
    pub fn default_state_path() -> PathBuf {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());

        PathBuf::from(home).join(".chimera").join("tui_state.yaml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let cfg = TuiConfig::default();
        assert_eq!(cfg.theme, Theme::Dark);
        assert!((cfg.main_panel_ratio - 0.7).abs() < 1e-6);
        assert_eq!(cfg.log_panel_height, 8);
        assert!(cfg.enable_mouse);
        assert_eq!(cfg.frame_rate, 60);
        // P2.4 新增字段默认值
        assert_eq!(cfg.tick_interval_ms, 250);
        assert_eq!(cfg.snapshot_interval_s, 30);
        assert_eq!(cfg.max_event_history, 256);
        assert_eq!(cfg.max_snapshots, 100);
    }

    #[test]
    fn test_validate_ok() {
        let cfg = TuiConfig::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_validate_ratio_out_of_range() {
        let cfg = TuiConfig {
            main_panel_ratio: 1.5,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_ratio_zero() {
        let cfg = TuiConfig {
            main_panel_ratio: 0.0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_ratio_one() {
        let cfg = TuiConfig {
            main_panel_ratio: 1.0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_log_height_too_small() {
        let cfg = TuiConfig {
            log_panel_height: 2,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_zero_frame_rate() {
        let cfg = TuiConfig {
            frame_rate: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    // === P2.4 新增字段校验测试 ===

    #[test]
    fn test_validate_tick_interval_too_low() {
        let cfg = TuiConfig {
            tick_interval_ms: 50,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_tick_interval_too_high() {
        let cfg = TuiConfig {
            tick_interval_ms: 2000,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_snapshot_interval_zero() {
        let cfg = TuiConfig {
            snapshot_interval_s: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_max_event_history_too_small() {
        let cfg = TuiConfig {
            max_event_history: 32,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_max_snapshots_too_small() {
        let cfg = TuiConfig {
            max_snapshots: 5,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_theme_as_str() {
        assert_eq!(Theme::Dark.as_str(), "dark");
        assert_eq!(Theme::Light.as_str(), "light");
    }

    #[test]
    fn test_theme_display() {
        assert_eq!(Theme::Dark.to_string(), "dark");
    }

    #[test]
    fn test_theme_serde_roundtrip() {
        let theme = Theme::Light;
        let json = serde_json::to_string(&theme).unwrap();
        let restored: Theme = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, theme);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let cfg = TuiConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: TuiConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.theme, cfg.theme);
        assert!((restored.main_panel_ratio - cfg.main_panel_ratio).abs() < 1e-6);
    }

    // === P6.1 运行时主题切换测试 ===

    #[test]
    fn test_theme_next_cycle() {
        // 循环顺序:Dark → Light → HighContrast → Dark
        assert_eq!(Theme::Dark.next(), Theme::Light);
        assert_eq!(Theme::Light.next(), Theme::HighContrast);
        assert_eq!(Theme::HighContrast.next(), Theme::Dark);
        // 连续切换 3 次应回到起点
        let start = Theme::Dark;
        let after_three = start.next().next().next();
        assert_eq!(after_three, start);
    }

    #[test]
    fn test_theme_as_str_high_contrast() {
        assert_eq!(Theme::HighContrast.as_str(), "high_contrast");
    }

    #[test]
    fn test_theme_colors_dark() {
        let c = Theme::Dark.colors();
        assert_eq!(c.foreground, ColorKind::White);
        assert_eq!(c.background, ColorKind::Black);
        assert_eq!(c.accent, ColorKind::Cyan);
        assert_eq!(c.warning, ColorKind::Yellow);
        assert_eq!(c.error, ColorKind::Red);
        assert_eq!(c.success, ColorKind::Green);
    }

    #[test]
    fn test_theme_colors_light() {
        let c = Theme::Light.colors();
        assert_eq!(c.foreground, ColorKind::Black);
        assert_eq!(c.background, ColorKind::White);
        assert_eq!(c.accent, ColorKind::Blue);
        assert_eq!(c.warning, ColorKind::BrightYellow);
        assert_eq!(c.error, ColorKind::BrightRed);
        assert_eq!(c.success, ColorKind::BrightGreen);
    }

    #[test]
    fn test_theme_colors_high_contrast() {
        let c = Theme::HighContrast.colors();
        // 纯黑白 + 高饱和度强调色:色盲用户 + 强光环境最大对比度
        assert_eq!(c.foreground, ColorKind::White);
        assert_eq!(c.background, ColorKind::Black);
        assert_eq!(c.accent, ColorKind::BrightYellow);
        assert_eq!(c.warning, ColorKind::BrightYellow);
        assert_eq!(c.error, ColorKind::BrightRed);
        assert_eq!(c.success, ColorKind::BrightGreen);
    }

    #[test]
    fn test_theme_serde_high_contrast() {
        let theme = Theme::HighContrast;
        let json = serde_json::to_string(&theme).unwrap();
        let restored: Theme = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, theme);
        // 序列化应产生有效的变体名
        assert_eq!(json, "\"HighContrast\"");
    }

    #[test]
    fn test_theme_colors_foreground_background() {
        // 所有主题的前景/背景对比度合理
        // Dark: 白字黑底 / Light: 黑字白底 / HighContrast: 白字黑底
        let dark = Theme::Dark.colors();
        assert_eq!(dark.foreground, ColorKind::White);
        assert_eq!(dark.background, ColorKind::Black);
        assert_ne!(dark.foreground, dark.background);

        let light = Theme::Light.colors();
        assert_eq!(light.foreground, ColorKind::Black);
        assert_eq!(light.background, ColorKind::White);
        assert_ne!(light.foreground, light.background);

        let hc = Theme::HighContrast.colors();
        assert_eq!(hc.foreground, ColorKind::White);
        assert_eq!(hc.background, ColorKind::Black);
        assert_ne!(hc.foreground, hc.background);
    }

    // === P6.3 颜色方案配置测试 ===

    #[test]
    fn test_color_scheme_default_all_none() {
        // 默认 ColorScheme 应所有字段为 None,表示不覆盖任何主题预设
        let cs = ColorScheme::default();
        assert!(cs.foreground.is_none());
        assert!(cs.background.is_none());
        assert!(cs.accent.is_none());
        assert!(cs.warning.is_none());
        assert!(cs.error.is_none());
        assert!(cs.success.is_none());
    }

    #[test]
    fn test_color_scheme_default_for_theme() {
        // default_for_theme 对所有主题应返回全 None(不覆盖任何颜色)
        for theme in [Theme::Dark, Theme::Light, Theme::HighContrast] {
            let cs = ColorScheme::default_for_theme(theme);
            assert!(
                cs.foreground.is_none(),
                "{theme:?} foreground should be None"
            );
            assert!(cs.accent.is_none(), "{theme:?} accent should be None");
        }
    }

    #[test]
    fn test_color_scheme_resolve_no_override() {
        // 无覆盖时,resolve 应返回与主题预设完全相同的 ThemeColors
        let cs = ColorScheme::default();
        let resolved = cs.resolve(Theme::Dark);
        let expected = Theme::Dark.colors();
        assert_eq!(resolved.foreground, expected.foreground);
        assert_eq!(resolved.background, expected.background);
        assert_eq!(resolved.accent, expected.accent);
        assert_eq!(resolved.warning, expected.warning);
        assert_eq!(resolved.error, expected.error);
        assert_eq!(resolved.success, expected.success);
    }

    #[test]
    fn test_color_scheme_resolve_with_partial_override() {
        // 部分覆盖:只覆盖 accent,其余沿用主题预设
        let cs = ColorScheme {
            accent: Some(ColorKind::BrightBlue),
            ..Default::default()
        };
        let resolved = cs.resolve(Theme::Dark);
        // 覆盖的字段用用户值
        assert_eq!(resolved.accent, ColorKind::BrightBlue);
        // 未覆盖的字段沿用 Dark 主题预设
        assert_eq!(resolved.foreground, ColorKind::White);
        assert_eq!(resolved.background, ColorKind::Black);
        assert_eq!(resolved.warning, ColorKind::Yellow);
        assert_eq!(resolved.error, ColorKind::Red);
        assert_eq!(resolved.success, ColorKind::Green);
    }

    #[test]
    fn test_color_scheme_resolve_full_override() {
        // 全覆盖:所有字段都用用户值,主题预设完全被忽略
        let cs = ColorScheme {
            foreground: Some(ColorKind::Black),
            background: Some(ColorKind::White),
            accent: Some(ColorKind::Magenta),
            warning: Some(ColorKind::BrightYellow),
            error: Some(ColorKind::BrightRed),
            success: Some(ColorKind::BrightGreen),
        };
        let resolved = cs.resolve(Theme::Dark);
        assert_eq!(resolved.foreground, ColorKind::Black);
        assert_eq!(resolved.background, ColorKind::White);
        assert_eq!(resolved.accent, ColorKind::Magenta);
        assert_eq!(resolved.warning, ColorKind::BrightYellow);
        assert_eq!(resolved.error, ColorKind::BrightRed);
        assert_eq!(resolved.success, ColorKind::BrightGreen);
    }

    #[test]
    fn test_color_scheme_serde_roundtrip() {
        let cs = ColorScheme {
            accent: Some(ColorKind::BrightCyan),
            warning: Some(ColorKind::BrightYellow),
            ..Default::default()
        };
        let json = serde_json::to_string(&cs).unwrap();
        let restored: ColorScheme = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, cs);
    }

    #[test]
    fn test_config_with_colors_serde_roundtrip() {
        // TuiConfig 含 colors 字段的序列化/反序列化往返
        let cfg = TuiConfig {
            colors: ColorScheme {
                accent: Some(ColorKind::BrightBlue),
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: TuiConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.theme, cfg.theme);
        assert_eq!(restored.colors, cfg.colors);
        assert_eq!(restored.colors.accent, Some(ColorKind::BrightBlue));
    }

    #[test]
    fn test_config_json_colors_override_from_string() {
        // P6.3.1 TDD-RED 核心场景:从 JSON 反序列化 tui.colors 节覆盖默认颜色
        // 模拟配置文件:
        // {
        //   "theme": "Dark",
        //   "colors": {
        //     "accent": "BrightBlue",
        //     "warning": "BrightYellow"
        //   }
        // }
        let json = r#"{
            "theme": "Dark",
            "colors": {
                "accent": "BrightBlue",
                "warning": "BrightYellow"
            }
        }"#;
        let cfg: TuiConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.theme, Theme::Dark);
        assert_eq!(cfg.colors.accent, Some(ColorKind::BrightBlue));
        assert_eq!(cfg.colors.warning, Some(ColorKind::BrightYellow));
        assert!(cfg.colors.foreground.is_none());
        assert!(cfg.colors.background.is_none());
        // resolve 后 accent 应为用户覆盖值,foreground 应为 Dark 主题预设
        let resolved = cfg.colors.resolve(cfg.theme);
        assert_eq!(resolved.accent, ColorKind::BrightBlue);
        assert_eq!(resolved.foreground, ColorKind::White);
    }

    #[test]
    fn test_config_colors_field_default_when_absent() {
        // 配置文件未指定 colors 节时,应回退到 ColorScheme::default()(全 None)
        // 这验证 #[serde(default)] 标注的正确性
        let json = r#"{"theme": "Light"}"#;
        let cfg: TuiConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.theme, Theme::Light);
        assert_eq!(cfg.colors, ColorScheme::default());
    }

    // ============================================================
    // TUI v1.8-omega: TuiConfig 持久化测试(TDD-RED)
    // Task 15 将实现 save_to_file / load_from_file / default_path
    // ============================================================

    #[test]
    fn test_config_save_and_load_roundtrip() {
        // 保存配置到临时文件,再加载回来,验证字段一致
        let config = TuiConfig {
            theme: Theme::Light,
            main_panel_ratio: 0.6,
            tick_interval_ms: 200,
            ..TuiConfig::default()
        };

        // 使用临时目录
        let temp_dir = std::env::temp_dir();
        let config_path = temp_dir.join("chimera_tui_test_roundtrip.yaml");

        // 清理可能存在的旧文件
        let _ = std::fs::remove_file(&config_path);

        // 保存
        config
            .save_to_file(&config_path)
            .expect("save should succeed");

        // 加载
        let loaded = TuiConfig::load_from_file(&config_path).expect("load should succeed");

        // 验证持久化字段
        assert_eq!(loaded.theme, config.theme);
        assert!((loaded.main_panel_ratio - config.main_panel_ratio).abs() < 1e-5);
        assert_eq!(loaded.tick_interval_ms, config.tick_interval_ms);

        // 清理
        let _ = std::fs::remove_file(&config_path);
    }

    #[test]
    fn test_config_load_nonexistent_returns_default() {
        // 文件不存在时返回 Ok(TuiConfig::default())
        let temp_dir = std::env::temp_dir();
        let nonexistent = temp_dir.join("chimera_tui_nonexistent_12345.yaml");
        let _ = std::fs::remove_file(&nonexistent);

        let result = TuiConfig::load_from_file(&nonexistent);
        assert!(result.is_ok(), "nonexistent file should return Ok(default)");
        let loaded = result.unwrap();
        assert_eq!(loaded.theme, TuiConfig::default().theme);
    }

    #[test]
    fn test_config_load_corrupted_returns_error() {
        // 文件存在但 YAML 损坏时返回 Err
        let temp_dir = std::env::temp_dir();
        let corrupted_path = temp_dir.join("chimera_tui_corrupted.yaml");

        // 写入无效 YAML
        std::fs::write(&corrupted_path, "invalid: yaml: content: [unclosed").unwrap();

        let result = TuiConfig::load_from_file(&corrupted_path);
        assert!(result.is_err(), "corrupted YAML should return Err");

        // 清理
        let _ = std::fs::remove_file(&corrupted_path);
    }

    #[test]
    fn test_config_default_path_ends_with_tui_yaml() {
        // 默认路径应以 tui.yaml 结尾
        let path = TuiConfig::default_path();
        assert!(
            path.ends_with("tui.yaml") || path.ends_with("tui.yml"),
            "default path should end with tui.yaml, got: {:?}",
            path
        );
    }

    #[test]
    fn test_config_save_creates_file() {
        // 保存后文件应存在
        let config = TuiConfig::default();
        let temp_dir = std::env::temp_dir();
        let config_path = temp_dir.join("chimera_tui_test_create.yaml");

        let _ = std::fs::remove_file(&config_path);

        config
            .save_to_file(&config_path)
            .expect("save should succeed");

        assert!(config_path.exists(), "file should exist after save");

        // 清理
        let _ = std::fs::remove_file(&config_path);
    }

    #[test]
    fn test_config_persistence_excludes_runtime_fields() {
        // 验证不持久化字段:max_event_history / max_snapshots / snapshot_interval_s
        // 保存时修改这些字段,加载后应恢复为默认值
        let config = TuiConfig {
            max_event_history: 999,
            max_snapshots: 999,
            snapshot_interval_s: 999,
            ..TuiConfig::default()
        };

        let temp_dir = std::env::temp_dir();
        let config_path = temp_dir.join("chimera_tui_test_exclude.yaml");
        let _ = std::fs::remove_file(&config_path);

        config
            .save_to_file(&config_path)
            .expect("save should succeed");
        let loaded = TuiConfig::load_from_file(&config_path).expect("load should succeed");

        // 运行时字段应恢复为默认值,不持久化
        let default = TuiConfig::default();
        assert_eq!(loaded.max_event_history, default.max_event_history);
        assert_eq!(loaded.max_snapshots, default.max_snapshots);
        assert_eq!(loaded.snapshot_interval_s, default.snapshot_interval_s);

        // 清理
        let _ = std::fs::remove_file(&config_path);
    }
}
