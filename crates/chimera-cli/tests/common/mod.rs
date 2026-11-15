//! 集成测试共享夹具（`tests/common/mod.rs` 不会被 cargo 当作独立测试目标）。
//!
//! # 为什么需要解析 POSIX shell 而不是直接写 `"bash"`
//! Windows 上 PATH 里的 `bash` 可能是 WSL 桩（`C:\Windows\System32\bash.exe`）：
//! - 它不把任意 Windows 环境变量带进 Linux 侧（只有 `WSLENV` 列出的才会），
//!   使"配置 env 必须到达子进程"的接线测试假红；
//! - 冷启动可能超过 MCP 请求超时（实测 5s 超时 ⇒ 假服务器永远不回包）。
//!
//! 本项目本地开发约定使用 Git Bash（见 `Aegis/hooks/run-hook.cmd`），CI（ubuntu）
//! 直接使用 PATH 里的 `bash`。`CHIMERA_TEST_SHELL` 为其他机器的逃生口。

#![allow(dead_code)] // 各测试二进制按需引用本模块的子集

/// 解析本机可用的 POSIX shell（测试夹具专用）
pub fn shell_program() -> &'static str {
    static SHELL: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    SHELL.get_or_init(|| {
        if let Ok(custom) = std::env::var("CHIMERA_TEST_SHELL") {
            if !custom.trim().is_empty() {
                return Box::leak(custom.into_boxed_str());
            }
        }
        if cfg!(windows) {
            for candidate in [
                r"C:\Program Files\Git\bin\bash.exe",
                r"C:\Program Files (x86)\Git\bin\bash.exe",
            ] {
                if std::path::Path::new(candidate).exists() {
                    return candidate;
                }
            }
        }
        "bash"
    })
}

/// 该程序名是否是 POSIX shell（`bash` / `bash.exe` / 其绝对路径）
///
/// 夹具闸门用它替代 `program == "bash"`：解析后的 shell 可能是绝对路径，
/// 但仍应被"只放行 shell"的最小闸门接受。
pub fn is_posix_shell(program: &str) -> bool {
    std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.eq_ignore_ascii_case("bash") || name.eq_ignore_ascii_case("bash.exe")
        })
}
