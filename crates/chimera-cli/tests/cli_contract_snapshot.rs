//! CLI 行为契约快照 — Phase 0 行为护栏（T0.1）
//!
//! WHY 独立 target：后续重构涉及命令装配时机、退出码与 JSON envelope，
//! 必须先用一份"黑盒子进程级"断言锁住当前行为，才能安全地逐批改动。
//!
//! 设计约束：
//! - 只断言稳定可复现的部分：退出码、stdout 首行前缀、stdout/stderr 关键字；
//! - 归一化版本号与默认路径，避免每次发版/换机器误红；
//! - 通过 `HOME`/`USERPROFILE` 指向临时目录隔离真实用户配置；
//! - `RUST_LOG=error` 抑制 info 日志，只保留错误路径输出；
//! - 清空外部 `CHIMERA_*` 环境变量，避免开发者本地配置污染契约。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// 一条契约用例。
struct Case {
    name: &'static str,
    args: &'static [&'static str],
    expect_exit: i32,
    /// stdout 首行（归一化后）必须以此前缀开头；None 表示不检查首行。
    stdout_first_prefix: Option<&'static str>,
    /// stdout 必须包含的稳定关键字。
    stdout_contains: &'static [&'static str],
    /// stderr 必须包含的稳定关键字。
    stderr_contains: &'static [&'static str],
    /// 是否要求 stderr 为空（仅用于成功且无提示噪声的命令）。
    expect_empty_stderr: bool,
}

const CASES: &[Case] = &[
    Case {
        name: "version",
        args: &["--version"],
        expect_exit: 0,
        stdout_first_prefix: Some("chimera <VERSION>"),
        stdout_contains: &[],
        stderr_contains: &[],
        expect_empty_stderr: true,
    },
    Case {
        name: "help_all",
        args: &["--no-banner", "--no-color", "--help"],
        expect_exit: 0,
        stdout_first_prefix: Some("NEXUS-OMEGA AI Coding Agent"),
        stdout_contains: &["Usage:", "Commands:"],
        stderr_contains: &[],
        expect_empty_stderr: true,
    },
    Case {
        name: "help_cmd",
        args: &["--no-banner", "--no-color", "help"],
        expect_exit: 0,
        stdout_first_prefix: Some("EXAMPLES:"),
        stdout_contains: &["chimera run", "chimera doctor"],
        stderr_contains: &[],
        expect_empty_stderr: true,
    },
    Case {
        name: "help_quest",
        args: &["--no-banner", "--no-color", "help", "quest"],
        expect_exit: 0,
        stdout_first_prefix: Some("EXAMPLES:"),
        stdout_contains: &["chimera quest list"],
        stderr_contains: &[],
        expect_empty_stderr: true,
    },
    Case {
        name: "help_unknown",
        args: &["--no-banner", "--no-color", "help", "nonexistent"],
        expect_exit: 0,
        stdout_first_prefix: Some("[E001]"),
        stdout_contains: &["nonexistent"],
        stderr_contains: &[],
        expect_empty_stderr: true,
    },
    Case {
        name: "config_path",
        args: &["--no-banner", "--no-color", "config", "path"],
        expect_exit: 0,
        stdout_first_prefix: Some("<PATH>"),
        stdout_contains: &["omega.yaml"],
        stderr_contains: &[],
        expect_empty_stderr: true,
    },
    Case {
        name: "completions_bash",
        args: &["--no-banner", "--no-color", "completions", "bash"],
        expect_exit: 0,
        stdout_first_prefix: Some("_chimera"),
        stdout_contains: &[],
        stderr_contains: &[],
        expect_empty_stderr: true,
    },
    Case {
        name: "mcp_serve_not_implemented",
        args: &["--no-banner", "--no-color", "mcp", "serve"],
        expect_exit: 2,
        stdout_first_prefix: None,
        stdout_contains: &[],
        stderr_contains: &["NotImplemented"],
        expect_empty_stderr: false,
    },
    Case {
        name: "quest_show_missing_engine_error",
        args: &[
            "--no-banner",
            "--no-color",
            "quest",
            "show",
            "__no_such_quest__",
        ],
        expect_exit: 3,
        stdout_first_prefix: None,
        stdout_contains: &[],
        stderr_contains: &["EngineError", "Quest 不存在"],
        expect_empty_stderr: false,
    },
    Case {
        name: "config_path_traversal_rejected",
        args: &[
            "--no-banner",
            "--no-color",
            "--config",
            "../escape.yaml",
            "config",
            "show",
        ],
        expect_exit: 1,
        stdout_first_prefix: None,
        stdout_contains: &[],
        stderr_contains: &["ConfigError", ".."],
        expect_empty_stderr: false,
    },
    Case {
        name: "legacy_exit_code_collapses_to_one",
        args: &[
            "--no-banner",
            "--no-color",
            "--legacy-exit-code",
            "mcp",
            "serve",
        ],
        expect_exit: 1,
        stdout_first_prefix: None,
        stdout_contains: &[],
        stderr_contains: &["NotImplemented"],
        expect_empty_stderr: false,
    },
    Case {
        name: "json_cost",
        args: &["--no-banner", "--no-color", "--json", "cost"],
        expect_exit: 0,
        stdout_first_prefix: Some("{"),
        stdout_contains: &["\"status\": \"ok\""],
        stderr_contains: &[],
        expect_empty_stderr: false,
    },
    Case {
        name: "json_config_show",
        args: &["--no-banner", "--no-color", "--json", "config", "show"],
        expect_exit: 0,
        stdout_first_prefix: Some("{"),
        stdout_contains: &["\"status\": \"ok\"", "\"nexus\""],
        stderr_contains: &[],
        expect_empty_stderr: false,
    },
    Case {
        name: "json_quest_list",
        args: &["--no-banner", "--no-color", "--json", "quest", "list"],
        expect_exit: 0,
        stdout_first_prefix: Some("{"),
        stdout_contains: &["\"status\": \"ok\""],
        stderr_contains: &[],
        expect_empty_stderr: false,
    },
    Case {
        name: "json_mcp_list",
        args: &["--no-banner", "--no-color", "--json", "mcp", "list"],
        expect_exit: 0,
        stdout_first_prefix: Some("{"),
        stdout_contains: &["\"status\": \"ok\""],
        stderr_contains: &[],
        expect_empty_stderr: false,
    },
    Case {
        name: "json_llm_list",
        args: &["--no-banner", "--no-color", "--json", "llm", "list"],
        expect_exit: 0,
        stdout_first_prefix: Some("{"),
        stdout_contains: &["\"status\": \"ok\""],
        stderr_contains: &[],
        expect_empty_stderr: false,
    },
    Case {
        name: "json_error_envelope",
        args: &["--no-banner", "--no-color", "--json", "mcp", "serve"],
        expect_exit: 2,
        // WHY stderr：main.rs 的错误 envelope 走 stderr（stdout 只承载成功数据）
        stdout_first_prefix: None,
        stdout_contains: &[],
        stderr_contains: &[
            "NotImplemented",
            "\"status\": \"error\"",
            "\"exit_code\": 2",
        ],
        expect_empty_stderr: false,
    },
];

/// 归一化：统一换行、版本号、默认配置路径。
///
/// WHY 只做最小归一化：契约断言要能发现"行为变化"，但不能因为发版或
/// 换机器（home 路径不同）而误红。
fn normalize(text: &str) -> String {
    let text = text.replace("\r\n", "\n");

    text.lines()
        .map(|line| {
            if line.starts_with("chimera ") && line.contains('.') {
                "chimera <VERSION>".to_string()
            } else if looks_like_path(line) {
                // 保留文件名：既能断言“输出的是路径”，又能断言默认配置文件名为 omega.yaml。
                let name = Path::new(line.trim())
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                if name.is_empty() {
                    "<PATH>".to_string()
                } else {
                    format!("<PATH>/{name}")
                }
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 判断一行是否"整行就是一个路径"。
///
/// 只用于 `config path` 这类纯路径输出；不处理嵌入路径的日志行，
/// 避免把正常文本误判为路径导致断言失真。
fn looks_like_path(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    // 注意：不能因含空格就排除——本仓库根路径本身含空格（"Chimera CLI"），
    // `Path::is_absolute()` 能正确处理带空格的 Windows 绝对路径。
    Path::new(trimmed).is_absolute() || trimmed.starts_with("~/") || trimmed.ends_with("omega.yaml")
}

/// 构造隔离的命令：清空外部 `CHIMERA_*`，固定 HOME/USERPROFILE 与日志级别。
fn command(home: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_chimera"));

    for (key, _) in std::env::vars() {
        if key.starts_with("CHIMERA_") {
            cmd.env_remove(key);
        }
    }

    cmd.env("HOME", home)
        .env("USERPROFILE", home)
        .env("RUST_LOG", "error")
        // 流式输出在测试中无需真实延迟
        .env("CHIMERA_RUN_CHUNK_DELAY_MS", "0")
        .args(args);

    cmd
}

#[test]
fn cli_contract_snapshot() {
    let home = TempDir::new().expect("创建隔离 HOME 目录失败");
    let mut failures = Vec::new();

    for case in CASES {
        let output = command(home.path(), case.args)
            .output()
            .unwrap_or_else(|e| panic!("[{}] 启动 chimera 失败: {e}", case.name));

        let code = output.status.code().unwrap_or(-1);
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        let stderr = normalize(&String::from_utf8_lossy(&output.stderr));
        let stdout_first = stdout.lines().next().unwrap_or("");

        if code != case.expect_exit {
            failures.push(format!(
                "[{}] exit: expected {}, got {}; stdout={:?}; stderr={:?}",
                case.name, case.expect_exit, code, stdout, stderr
            ));
        }

        if let Some(prefix) = case.stdout_first_prefix {
            if !stdout_first.starts_with(prefix) {
                failures.push(format!(
                    "[{}] stdout 首行: expected prefix {:?}, got {:?}",
                    case.name, prefix, stdout_first
                ));
            }
        }

        for needle in case.stdout_contains {
            if !stdout.contains(needle) {
                failures.push(format!(
                    "[{}] stdout 缺少 {:?}; got {:?}",
                    case.name, needle, stdout
                ));
            }
        }

        for needle in case.stderr_contains {
            if !stderr.contains(needle) {
                failures.push(format!(
                    "[{}] stderr 缺少 {:?}; got {:?}",
                    case.name, needle, stderr
                ));
            }
        }

        if case.expect_empty_stderr && !stderr.trim().is_empty() {
            failures.push(format!("[{}] stderr 应为空; got {:?}", case.name, stderr));
        }
    }

    assert!(
        failures.is_empty(),
        "CLI 契约快照失败 {} 条:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
