//! 路径工具 — 跨平台路径处理
//!
//! 对应架构层:L1 Core(供 L2-L10 所有上层 crate 共享)
//!
//! # 设计决策(WHY)
//! - **集中定义**:mlc-engine(config.rs)与 cmt-tiering(config.rs)两处重复实现
//!   `expand_tilde` 函数(展开 `~` 为 home 目录),每处约 12 行。提取到 L1 Core
//!   消除重复,确保行为一致
//! - **不引入 `dirs` crate**:仅依赖标准库环境变量,符合"最小依赖"原则。
//!   Unix 用 `$HOME`,Windows 用 `$USERPROFILE`
//!
//! # 使用示例
//! ```
//! use nexus_core::path_util::expand_tilde;
//! use std::path::PathBuf;
//!
//! // 绝对路径不展开
//! let expanded = expand_tilde(&PathBuf::from("/absolute/path.db"));
//! assert_eq!(expanded, PathBuf::from("/absolute/path.db"));
//! ```

use std::path::{Path, PathBuf};

/// 展开路径中的 `~` 为用户 home 目录
///
/// WHY:Windows 上 `~` 不是原生概念,需要手动展开。Unix 上 shell 会展开 `~`,
/// 但程序接收的路径可能未经过 shell 展开(如配置文件中的路径)。
///
/// # 展开规则
/// - `~` → home 目录
/// - `~/path` 或 `~\path` → home 目录 + `/path` 或 `\path`
/// - 其他路径 → 原样返回
///
/// # 环境变量
/// 优先使用 `HOME`(Unix 惯例),回退到 `USERPROFILE`(Windows 惯例)。
/// 若两者均未设置,返回原路径(调用方处理错误)。
///
/// # 示例
/// ```
/// use nexus_core::path_util::expand_tilde;
/// use std::path::PathBuf;
///
/// // 无 ~ 前缀的路径原样返回
/// let path = PathBuf::from("/absolute/path.db");
/// assert_eq!(expand_tilde(&path), path);
/// ```
pub fn expand_tilde(path: &Path) -> PathBuf {
    // 优先 HOME(Unix),回退 USERPROFILE(Windows)
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok();
    expand_tilde_with_home(path, home.as_deref())
}

/// 内部辅助:用显式传入的 home 目录展开 `~`
///
/// WHY 提取此函数:测试需要稳定可复现。若直接读取环境变量,
/// 并行测试之间会因 `set_var`/`remove_var` 产生竞态——一个测试删除 `HOME`
/// 会导致另一个并行测试读到 `None`,从而 `~` 不被展开。将 home 注入为参数后,
/// 测试不再触碰环境变量,彻底消除竞态。
fn expand_tilde_with_home(path: &Path, home: Option<&str>) -> PathBuf {
    let s = path.to_string_lossy();
    if !s.starts_with('~') {
        return path.to_path_buf();
    }

    match home {
        Some(h) => {
            let expanded = if s == "~" {
                h.to_string()
            } else if s.starts_with("~/") || s.starts_with("~\\") {
                format!("{}{}", h, &s[1..])
            } else {
                s.into_owned()
            };
            PathBuf::from(expanded)
        }
        None => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_tilde_no_tilde() {
        let path = PathBuf::from("/absolute/path.db");
        let expanded = expand_tilde(&path);
        assert_eq!(expanded, path);
    }

    #[test]
    fn test_expand_tilde_with_home() {
        let path = PathBuf::from("~/memory.db");
        let expanded = expand_tilde_with_home(&path, Some("/test/home"));
        assert_eq!(expanded, PathBuf::from("/test/home/memory.db"));
    }

    #[test]
    fn test_expand_tilde_only_tilde() {
        let path = PathBuf::from("~");
        let expanded = expand_tilde_with_home(&path, Some("/test/home"));
        assert_eq!(expanded, PathBuf::from("/test/home"));
    }

    #[test]
    fn test_expand_tilde_backslash_separator() {
        let path = PathBuf::from("~\\memory.db");
        let expanded = expand_tilde_with_home(&path, Some("/test/home"));
        assert_eq!(expanded, PathBuf::from("/test/home\\memory.db"));
    }

    #[test]
    fn test_expand_tilde_no_home_env() {
        let path = PathBuf::from("~/memory.db");
        // 无 HOME/USERPROFILE 时返回原路径
        let expanded = expand_tilde_with_home(&path, None);
        assert_eq!(expanded, path);
    }
}
