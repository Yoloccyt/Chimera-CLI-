//! TuiBible 设计手册配置加载器(Task 3.2,v1.8-omega)
//!
//! 对应架构层:L10 Interface
//! 对应 spec: `enterprise-tui-monitoring-task-viz/spec.md` §五"配置与持久化"
//!
//! # 设计哲学(WHY)
//! - **设计手册权威源**:`TuiBible` 是用户在 `~/.chimera/tui_bible.yaml` 中
//!   表达的"我希望 TUI 长这样"的配置契约,与 `TuiConfig`(运行时参数,需要
//!   `validate()` 严格校验)正交分离:
//!   - `TuiConfig`:tick/帧率/事件保留数等运行时调优,validate 严苛
//!   - `TuiBible`:主题/键位/阈值/布局模板,validate 宽松(用户自由表达)
//! - **Figment 4 源合并**:与 `chimera-cli` 的 `ChimeraConfig::load` 保持一致模式
//!   (CLAUDE.md §4):默认 < 配置文件 < 环境变量 < CLI 参数。
//! - **环境变量前缀 `CHIMERA_BIBLE_`**:与项目 `CHIMERA_*` 命名空间区分,
//!   避免与 `ChimeraConfig` 的环境变量冲突;嵌套字段用 `__` 分隔(与
//!   `ChimeraConfig::load` 保持一致,如 `CHIMERA_BIBLE_LAYOUT__MAIN_PANEL_RATIO`)。
//!
//! # 配置示例(参见 `examples/config/tui_bible.sample.yaml`)
//! ```yaml
//! theme: Light
//! color_scheme:
//!   accent: BrightBlue
//!   warning: BrightYellow
//! key_bindings:
//!   quit:
//!     action: quit
//!     key: "q"
//!     description: "退出 TUI"
//! thresholds:
//!   cpu_warning: 0.75
//!   cpu_critical: 0.95
//! layout:
//!   mode: TriplePane
//!   main_panel_ratio: 0.6
//!   log_panel_height: 10
//!   sidebar_width: 30
//! ```
//!
//! # 失败语义
//! - **配置文件不存在** → 静默回退到默认值(`TuiBible::default()`),不报错
//!   (与 `TuiConfig::load_from_file` 一致,符合 Figment 合并语义)
//! - **配置文件 YAML 损坏** → 返回 `Err(TuiError::ConfigError)`,不静默
//! - **环境变量值非法**(如 `THEME=Banana`)→ 返回 `Err`,环境变量覆盖阶段报错

use std::collections::HashMap;

use figment::{
    providers::{Env, Format, Serialized, Yaml},
    Figment,
};
use serde::{Deserialize, Serialize};

use crate::config::{ColorScheme, Theme};
use crate::error::TuiError;
use crate::types::LayoutMode;

// ============================================================
// KeyBinding — 键位绑定描述
// ============================================================

/// 键位绑定 — 一个语义动作到具体按键的映射
///
/// WHY `action` + `key` 两个字段:
/// - `action` 描述"做什么"(语义名,固定枚举),如 `quit` / `next_panel`
/// - `key` 描述"按哪个键"(字符串,与终端输入解耦)
/// - 两者解耦后,`action` 可在命令面板(CommandPalette)显示给用户,
///   `key` 可在帮助面板(Help)显示按键,UI 关注点分离。
///
/// WHY `description: Option<String>`:
/// - 帮助面板按 description 渲染"按 Q 退出"等说明
/// - 缺省(None)时 Help 面板只显示按键,避免冗余
/// - `#[serde(default)]` 让 YAML 中省略 description 时为 None,降级配置负担
///
/// WHY 派生 `Default` 不可行:`action` 与 `key` 字段无合理"空"语义,
/// "空 KeyBinding" 表达无意义。若需要占位,用 `KeyBinding::placeholder()`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBinding {
    /// 动作语义名(如 `"quit"` / `"next_panel"`),用于命令面板与跨面板复用
    pub action: String,
    /// 按键字符串(如 `"q"` / `"j"` / `"Ctrl+C"`),与终端输入一致
    pub key: String,
    /// 可选:动作描述(Help 面板与命令面板 hover 提示)
    ///
    /// WHY `#[serde(default)]`:YAML 缺省时为 None,降级配置负担
    /// (用户只关心按键,不写描述也能用)。
    #[serde(default)]
    pub description: Option<String>,
}

impl KeyBinding {
    /// 返回该键位的可读摘要(用于状态栏/帮助面板)
    pub fn summary(&self) -> String {
        match &self.description {
            Some(desc) => format!("{} → {} ({})", self.action, self.key, desc),
            None => format!("{} → {}", self.action, self.key),
        }
    }
}

// ============================================================
// LayoutTemplate — 布局模板(对应 NEXUS_OMEGA_TUI_DESIGN_BIBLE §4)
// ============================================================

/// 布局模板 — 主区域/侧边栏/日志面板的几何参数
///
/// WHY 独立结构:布局不只是 `LayoutMode` 一个枚举,还包含 `main_panel_ratio`、
/// `log_panel_height`、`sidebar_width` 等几何参数。把它们聚合成 `LayoutTemplate`
/// 让 `TuiBible.layout` 字段语义自洽(否则需要展平为 4 个独立字段,
/// 与 `key_bindings` / `thresholds` 的"复合值"风格不一致)。
///
/// WHY `sidebar_width` 是 u16 而非 f32:侧边栏宽度以**字符数**计算(ratatui
/// 的 `Layout::split` 用 `Constraint::Length(u16)`),与 `log_panel_height`
/// 单位一致;`main_panel_ratio` 才是相对比例(0.0-1.0)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutTemplate {
    /// 布局模式(单/双/三面板)
    pub mode: LayoutMode,
    /// 主面板占比 [0.0, 1.0](与 TuiConfig.main_panel_ratio 一致)
    pub main_panel_ratio: f32,
    /// 日志面板高度(行数)
    pub log_panel_height: u16,
    /// 侧边栏宽度(字符数,DualPane/TriplePane 生效)
    pub sidebar_width: u16,
}

impl Default for LayoutTemplate {
    fn default() -> Self {
        // 与 TuiConfig 默认值保持一致,避免两套配置不一致
        Self {
            mode: LayoutMode::default(), // = DualPane
            main_panel_ratio: 0.7,
            log_panel_height: 8,
            sidebar_width: 24,
        }
    }
}

// ============================================================
// TuiBible — 设计手册配置根
// ============================================================

/// 设计手册配置 — 用户对 TUI 外观与交互的"圣经"式配置
///
/// WHY 字段命名:
/// - `theme`:沿用 `TuiConfig::theme` 字段名,两套配置可双向桥接
/// - `color_scheme`:沿用 `TuiConfig::colors` 字段的语义(细粒度覆盖),
///   命名略改以明示它是 ColorScheme(不是 ThemeColors)
/// - `key_bindings`:HashMap<action_name, KeyBinding>,key 是语义动作名
/// - `thresholds`:HashMap<metric_name, f32>,key 是指标语义名
/// - `layout`:LayoutTemplate 而非平铺字段,聚合几何参数
///
/// WHY HashMap 而非 enum/threshold struct:
/// - 用户配置自由度最大化:任意 action 名/任意指标名,无需改 schema
/// - 前向兼容:v1.9+ 新增 action/指标,旧配置文件零修改即可加载
/// - 与 Figment Env provider 解析规则保持一致(简单 key=value)
///
/// WHY `#[serde(default)]`:
/// - 配置文件可省略任意字段,缺省字段回退到 `TuiBible::default()`(内置默认)
/// - 与 `TuiConfig` 行为一致(`#[serde(default)]` 在 TuiConfig 上已验证)
/// - Figment 4 源合并的最后兜底:defaults → file → env → CLI,
///   任意源缺字段都回退到 defaults
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TuiBible {
    /// 主题(颜色方案,Dark/Light/HighContrast)
    pub theme: Theme,
    /// 颜色方案覆盖(细粒度覆盖,None 用主题预设)
    pub color_scheme: ColorScheme,
    /// 键位绑定映射表:key = 动作名(如 "quit"),value = KeyBinding
    pub key_bindings: HashMap<String, KeyBinding>,
    /// 阈值映射表:key = 指标名(如 "cpu_warning"),value = 阈值 [0.0, 1.0]
    pub thresholds: HashMap<String, f32>,
    /// 布局模板(布局模式 + 几何参数)
    pub layout: LayoutTemplate,
}

impl Default for TuiBible {
    fn default() -> Self {
        Self {
            // 与 TuiConfig 默认值保持一致
            theme: Theme::Dark,
            // 默认不覆盖任何颜色,完全用主题预设
            color_scheme: ColorScheme::default(),
            // 提供一组覆盖广泛场景的默认键位,保证即开即用
            // (后续面板可按需扩展)
            key_bindings: default_key_bindings(),
            // 默认阈值覆盖 CPU/内存/磁盘/网络的 warning 与 critical 两级
            thresholds: default_thresholds(),
            // 默认布局(DualPane + 标准几何)
            layout: LayoutTemplate::default(),
        }
    }
}

impl TuiBible {
    /// 返回默认配置文件路径:`~/.chimera/tui_bible.yaml`
    ///
    /// 与 `TuiConfig::default_path`(`tui.yaml`)区分,
    /// 保持两套配置的物理文件分离,避免配置语义混淆。
    pub fn default_path() -> std::path::PathBuf {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        std::path::PathBuf::from(home)
            .join(".chimera")
            .join("tui_bible.yaml")
    }

    /// 从 4 源加载 TuiBible:默认 < 配置文件 < 环境变量 < CLI
    ///
    /// # 优先级(后者覆盖前者)
    /// 1. `TuiBible::default()`(内置默认)
    /// 2. `~/.chimera/tui_bible.yaml`(若存在)
    /// 3. 环境变量 `CHIMERA_BIBLE_*`(嵌套字段用 `__`,如 `CHIMERA_BIBLE_THEME`)
    /// 4. CLI 参数(当前为占位,后续可扩展 Figment override provider)
    ///
    /// # 错误
    /// - 配置文件存在但 YAML 损坏 → `Err(TuiError::ConfigError)`
    /// - 环境变量值非法(如 `THEME=Banana`)→ `Err(TuiError::ConfigError)`
    /// - 配置文件不存在 → 静默回退到默认,不报错(符合 Figment 合并语义)
    pub fn load() -> Result<Self, TuiError> {
        let path = Self::default_path();
        // 优先级链:defaults -> file -> env
        // 注:CLI 参数目前仅影响 config_path,未直接进入 Figment;
        //     后续可扩展 Figment override provider 以支持 --theme 等参数。
        let figment = Figment::from(Serialized::defaults(TuiBible::default()))
            .merge(Yaml::file(&path))
            .merge(Env::prefixed("CHIMERA_BIBLE_").split("__"));

        figment
            .extract::<TuiBible>()
            .map_err(|e| TuiError::ConfigError {
                detail: format!("加载 tui_bible 失败:{}", e),
            })
    }
}

// ============================================================
// 默认值辅助函数
// ============================================================

/// 默认键位映射表
///
/// WHY 内置默认值:用户首次启动 TUI 不至于没有键位可用。
/// 覆盖:quit / next_panel / prev_panel / help / search / command_palette / refresh。
fn default_key_bindings() -> HashMap<String, KeyBinding> {
    let mut map = HashMap::new();
    let bindings = [
        ("quit", "q", "退出 TUI"),
        ("next_panel", "Tab", "切换到下一个面板"),
        ("prev_panel", "BackTab", "切换到上一个面板"),
        ("help", "?", "显示帮助面板"),
        ("search", "/", "进入搜索模式"),
        ("command_palette", ":", "打开命令面板"),
        ("refresh", "r", "刷新数据快照"),
    ];
    for (action, key, desc) in bindings {
        map.insert(
            action.to_string(),
            KeyBinding {
                action: action.to_string(),
                key: key.to_string(),
                description: Some(desc.to_string()),
            },
        );
    }
    map
}

/// 默认阈值映射表(覆盖 CPU/内存/磁盘/网络四类指标的 warning/critical 两级)
///
/// WHY 阈值用 [0.0, 1.0] 比例(非绝对值):
/// - CPU 0.7/0.9(70% 警告,90% 严重)
/// - 内存 0.8/0.95
/// - 磁盘 0.85/0.95
/// - 网络 0.75/0.9(链路利用率,需后续结合带宽上限计算)
fn default_thresholds() -> HashMap<String, f32> {
    let mut map = HashMap::new();
    map.insert("cpu_warning".to_string(), 0.70);
    map.insert("cpu_critical".to_string(), 0.90);
    map.insert("memory_warning".to_string(), 0.80);
    map.insert("memory_critical".to_string(), 0.95);
    map.insert("disk_warning".to_string(), 0.85);
    map.insert("disk_critical".to_string(), 0.95);
    map.insert("network_warning".to_string(), 0.75);
    map.insert("network_critical".to_string(), 0.90);
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_bible_values() {
        // TuiBible 默认值与 TuiConfig 默认值保持一致(避免两套配置漂移)
        let bible = TuiBible::default();
        assert_eq!(bible.theme, Theme::Dark);
        assert_eq!(bible.layout.mode, LayoutMode::DualPane);
        assert!((bible.layout.main_panel_ratio - 0.7).abs() < 1e-6);
        assert_eq!(bible.layout.log_panel_height, 8);
        // 默认键位至少包含 quit
        assert!(bible.key_bindings.contains_key("quit"));
        // 默认阈值至少包含 CPU warning
        assert!(bible.thresholds.contains_key("cpu_warning"));
    }

    #[test]
    fn test_default_path_ends_with_tui_bible_yaml() {
        // 默认路径应以 tui_bible.yaml 结尾(与 TuiConfig::default_path 区分)
        let path = TuiBible::default_path();
        assert!(
            path.ends_with("tui_bible.yaml") || path.ends_with("tui_bible.yml"),
            "default path should end with tui_bible.yaml, got: {:?}",
            path
        );
    }

    #[test]
    fn test_key_binding_summary_with_description() {
        let kb = KeyBinding {
            action: "quit".into(),
            key: "q".into(),
            description: Some("退出".into()),
        };
        assert_eq!(kb.summary(), "quit → q (退出)");
    }

    #[test]
    fn test_key_binding_summary_without_description() {
        let kb = KeyBinding {
            action: "next_panel".into(),
            key: "Tab".into(),
            description: None,
        };
        assert_eq!(kb.summary(), "next_panel → Tab");
    }

    #[test]
    fn test_key_binding_serde_roundtrip() {
        let kb = KeyBinding {
            action: "test".into(),
            key: "t".into(),
            description: Some("test action".into()),
        };
        let json = serde_json::to_string(&kb).unwrap();
        let restored: KeyBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, kb);
    }

    #[test]
    fn test_layout_template_default() {
        let lt = LayoutTemplate::default();
        assert_eq!(lt.mode, LayoutMode::DualPane);
        assert!((lt.main_panel_ratio - 0.7).abs() < 1e-6);
        assert_eq!(lt.log_panel_height, 8);
        assert_eq!(lt.sidebar_width, 24);
    }

    #[test]
    fn test_tui_bible_serde_roundtrip() {
        let bible = TuiBible::default();
        let json = serde_json::to_string(&bible).unwrap();
        let restored: TuiBible = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, bible);
    }

    #[test]
    fn test_tui_bible_serde_yaml_roundtrip() {
        // YAML 格式(Figment 实际使用)的序列化往返
        let bible = TuiBible::default();
        let yaml = serde_yaml::to_string(&bible).unwrap();
        let restored: TuiBible = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(restored.theme, bible.theme);
        assert_eq!(restored.layout.mode, bible.layout.mode);
        assert_eq!(restored.key_bindings.len(), bible.key_bindings.len());
        assert_eq!(restored.thresholds.len(), bible.thresholds.len());
    }

    #[test]
    fn test_partial_yaml_override() {
        // 用户 YAML 只覆盖部分字段,其余字段沿用默认
        let yaml = r#"
theme: Light
key_bindings:
  custom_action:
    action: custom_action
    key: "x"
    description: "自定义动作"
"#;
        let mut bible: TuiBible = serde_yaml::from_str(yaml).unwrap();
        // 覆盖字段
        assert_eq!(bible.theme, Theme::Light);
        assert_eq!(bible.key_bindings.get("custom_action").unwrap().key, "x");
        // 未覆盖字段保持默认
        assert_eq!(bible.layout.mode, LayoutMode::DualPane);
        assert!(bible.thresholds.contains_key("cpu_warning"));
        // 完整 Default 填充未指定的字段
        bible.layout = LayoutTemplate::default();
        bible.thresholds = default_thresholds();
        bible.color_scheme = ColorScheme::default();
        let full = TuiBible::default();
        assert_eq!(bible.layout, full.layout);
        assert_eq!(bible.thresholds, full.thresholds);
    }
}
