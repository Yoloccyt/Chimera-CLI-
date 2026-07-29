//! 配置持久化 — 保存/加载 TUI 配置到 YAML 文件
//!
//! 包含 [`PersistentConfig`] 内部结构体以及 [`TuiConfig`](super::TuiConfig)
//! 的文件 I/O 方法(`save_to_file`、`load_from_file`、`default_path`、
//! `default_state_path`)。
//!
//! 对应架构层:L10 Interface

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{ColorScheme, Theme, TuiConfig};
use crate::error::TuiError;

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
    /// 保存配置到 YAML 文件
    ///
    /// 持久化字段: theme / colors / main_panel_ratio / tick_interval_ms
    /// 不持久化: frame_rate / enable_mouse / max_event_history /
    ///           max_snapshots / snapshot_interval_s / log_panel_height
    ///
    /// WHY 只持久化 4 个字段: 运行时字段与硬件环境或性能调优相关,
    /// 每次启动应使用默认值,持久化会导致跨环境配置污染。
    pub fn save_to_file(&self, path: &std::path::Path) -> Result<(), TuiError> {
        let persistent = PersistentConfig {
            theme: self.theme,
            colors: self.colors.clone(),
            main_panel_ratio: self.main_panel_ratio,
            tick_interval_ms: self.tick_interval_ms,
        };

        // 确保父目录存在(如 ~/.chimera/),避免写入时因目录缺失失败
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| TuiError::ConfigError {
                detail: format!("Failed to create config directory: {}", e),
            })?;
        }

        let yaml = serde_yaml::to_string(&persistent).map_err(|e| TuiError::ConfigError {
            detail: format!("Failed to serialize config: {}", e),
        })?;

        std::fs::write(path, yaml).map_err(|e| TuiError::ConfigError {
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
    pub fn load_from_file(path: &std::path::Path) -> Result<Self, TuiError> {
        // 文件不存在时返回默认配置,不报错(首次启动场景)
        if !path.exists() {
            return Ok(TuiConfig::default());
        }

        let content = std::fs::read_to_string(path).map_err(|e| TuiError::ConfigError {
            detail: format!("Failed to read config file: {}", e),
        })?;

        let persistent: PersistentConfig =
            serde_yaml::from_str(&content).map_err(|e| TuiError::ConfigError {
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
    pub fn default_path() -> PathBuf {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());

        PathBuf::from(home).join(".chimera").join("tui.yaml")
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
    use super::super::{Theme, TuiConfig};

    #[test]
    fn test_config_save_and_load_roundtrip() {
        let config = TuiConfig {
            theme: Theme::Light,
            main_panel_ratio: 0.6,
            tick_interval_ms: 200,
            ..TuiConfig::default()
        };

        let temp_dir = std::env::temp_dir();
        let config_path = temp_dir.join("chimera_tui_test_roundtrip.yaml");
        let _ = std::fs::remove_file(&config_path);

        config
            .save_to_file(&config_path)
            .expect("save should succeed");

        let loaded = TuiConfig::load_from_file(&config_path).expect("load should succeed");

        assert_eq!(loaded.theme, config.theme);
        assert!((loaded.main_panel_ratio - config.main_panel_ratio).abs() < 1e-5);
        assert_eq!(loaded.tick_interval_ms, config.tick_interval_ms);

        let _ = std::fs::remove_file(&config_path);
    }

    #[test]
    fn test_config_load_nonexistent_returns_default() {
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
        let temp_dir = std::env::temp_dir();
        let corrupted_path = temp_dir.join("chimera_tui_corrupted.yaml");

        std::fs::write(&corrupted_path, "invalid: yaml: content: [unclosed").unwrap();

        let result = TuiConfig::load_from_file(&corrupted_path);
        assert!(result.is_err(), "corrupted YAML should return Err");

        let _ = std::fs::remove_file(&corrupted_path);
    }

    #[test]
    fn test_config_default_path_ends_with_tui_yaml() {
        let path = TuiConfig::default_path();
        assert!(
            path.ends_with("tui.yaml") || path.ends_with("tui.yml"),
            "default path should end with tui.yaml, got: {:?}",
            path
        );
    }

    #[test]
    fn test_config_save_creates_file() {
        let config = TuiConfig::default();
        let temp_dir = std::env::temp_dir();
        let config_path = temp_dir.join("chimera_tui_test_create.yaml");
        let _ = std::fs::remove_file(&config_path);

        config
            .save_to_file(&config_path)
            .expect("save should succeed");

        assert!(config_path.exists(), "file should exist after save");

        let _ = std::fs::remove_file(&config_path);
    }

    #[test]
    fn test_config_persistence_excludes_runtime_fields() {
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

        let default = TuiConfig::default();
        assert_eq!(loaded.max_event_history, default.max_event_history);
        assert_eq!(loaded.max_snapshots, default.max_snapshots);
        assert_eq!(loaded.snapshot_interval_s, default.snapshot_interval_s);

        let _ = std::fs::remove_file(&config_path);
    }
}
