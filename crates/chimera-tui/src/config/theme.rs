//! 主题与颜色类型 — TUI 视觉外观配置
//!
//! 包含 [`Theme`] 枚举、[`ColorKind`] 颜色种类、[`ThemeColors`] 主题预设
//! 以及 [`ColorScheme`] 用户细粒度颜色覆盖。
//!
//! 对应架构层:L10 Interface

use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_theme_next_cycle() {
        assert_eq!(Theme::Dark.next(), Theme::Light);
        assert_eq!(Theme::Light.next(), Theme::HighContrast);
        assert_eq!(Theme::HighContrast.next(), Theme::Dark);
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
        assert_eq!(json, "\"HighContrast\"");
    }

    #[test]
    fn test_theme_colors_foreground_background() {
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

    #[test]
    fn test_color_scheme_default_all_none() {
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
        let cs = ColorScheme {
            accent: Some(ColorKind::BrightBlue),
            ..Default::default()
        };
        let resolved = cs.resolve(Theme::Dark);
        assert_eq!(resolved.accent, ColorKind::BrightBlue);
        assert_eq!(resolved.foreground, ColorKind::White);
        assert_eq!(resolved.background, ColorKind::Black);
        assert_eq!(resolved.warning, ColorKind::Yellow);
        assert_eq!(resolved.error, ColorKind::Red);
        assert_eq!(resolved.success, ColorKind::Green);
    }

    #[test]
    fn test_color_scheme_resolve_full_override() {
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
}
