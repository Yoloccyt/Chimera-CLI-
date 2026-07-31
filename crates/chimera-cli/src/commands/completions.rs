//! `chimera completions <shell>` — 生成 shell 补全脚本(Task 1.14)
//!
//! v2.9.0-omega Task 1.14:真实接入 `clap_complete::generate`,为 5 种 shell 生成补全脚本。
//!
//! # 支持的 Shell(SubTask 1.14.3)
//! - `bash` — Bash 补全脚本(`/etc/bash_completion.d/chimera`)
//! - `zsh` — Zsh 补全脚本(`~/.zsh/completions/_chimera`)
//! - `fish` — Fish 补全脚本(`~/.config/fish/completions/chimera.fish`)
//! - `powershell` — PowerShell 补全脚本(导入到 `$PROFILE`)
//! - `elvish` — Elvish 补全脚本(`~/.config/elvish/lib/chimera.elv`)
//!
//! # 用法
//! ```bash
//! # 生成 bash 补全并安装到系统目录(需 root)
//! chimera completions bash | sudo tee /etc/bash_completion.d/chimera > /dev/null
//!
//! # 生成 zsh 补全到用户目录
//! chimera completions zsh > ~/.zsh/completions/_chimera
//!
//! # 生成 PowerShell 补全并导入
//! chimera completions powershell | Out-String | Invoke-Expression
//! ```
//!
//! # 设计决策(WHY)
//! - **直接写入 stdout**:`clap_complete::generate` 的第 4 参数接受 `&mut dyn Write`,
//!   传入 `&mut io::stdout()` 让脚本内容直接流到 stdout,便于管道重定向。
//! - **不消费 json/perm flag**:补全脚本是纯文本输出,无破坏性操作,
//!   不需要 JSON envelope 或 permission prompt。
//! - **bin name = "chimera"**:与 `cli.rs:46` `#[command(name = "chimera")]` 一致,
//!   保证生成的补全脚本匹配实际命令名(`chimera run` / `chimera quest list` 等)。

#![forbid(unsafe_code)]

use std::io;

use anyhow::Result;
use clap::CommandFactory;
use clap_complete::Shell;

use crate::cli::Cli;

/// 执行 completions 子命令 — 生成 shell 补全脚本(SubTask 1.14.2)
///
/// 调用 `clap_complete::generate(shell, &mut Cli::command(), "chimera", &mut io::stdout())`
/// 将指定 shell 的补全脚本写到 stdout。
///
/// # 错误
/// - stdout 写入失败(如管道关闭)返回 `IoError`(退出码 3)
///
/// # 示例
/// ```text
/// $ chimera completions bash
/// _chimera() {
///     local i cur prev opts cmd
///     COMPREPLY=()
///     ...
/// }
/// complete -F _chimera chimera
/// ```
pub async fn execute(shell: Shell) -> Result<()> {
    tracing::info!(?shell, "生成 shell 补全脚本");

    // 从 Cli derive 出 Command(包含全部子命令元信息)
    // WHY CommandFactory::command:clap_complete 需要 `mut Command` 引用,
    // `Cli::command()` 由 `#[derive(Parser)]` 自动实现,返回完整命令树
    let mut cmd = Cli::command();

    // 生成补全脚本到 stdout
    // 参数说明:
    // - shell:目标 shell 类型(Shell::Bash / Zsh / Fish / PowerShell / Elvish)
    // - &mut cmd:命令元信息(子命令 / 参数 / 帮助文本)
    // - "chimera":bin name(决定补全脚本中的函数名前缀,如 `_chimera`)
    // - &mut io::stdout():输出目标(直接流到 stdout,便于管道重定向)
    clap_complete::generate(shell, &mut cmd, "chimera", &mut io::stdout());

    Ok(())
}
