//! `chimera help` — EXAMPLES 一级入口(Task 5 of spec)
//!
//! 简化策略:硬编码 5 个顶级 EXAMPLES + 6 个常用子命令的示例映射,
//! 避免 clap 反射式 help 的复杂度。新增子命令只需在 `SUBCOMMAND_EXAMPLES`
//! 表中追加一行即可。
//!
//! 测试模型:核心渲染逻辑下沉到 `render_examples<W: Write>(cmd, &mut W)`,
//! 公开 `execute` 仅负责把 stdout 接入渲染器,因此测试可以直接对 Vec<u8>
//! 写入并断言内容,无需捕获 stdout。

#![forbid(unsafe_code)]

use std::io::Write;

use anyhow::Result;

use crate::cli::Cli;

/// 顶级 5 个 EXAMPLES(硬编码,与 `Cli::after_long_help` 保持一致)
const TOP_EXAMPLES: &str = "\
EXAMPLES:
  chimera run \"实现一个 hello world 函数\"            # 运行单次任务
  chimera --json quest list                         # JSON 格式列出 Quest
  chimera --yes agent cancel <agent-id>             # 取消 Agent(跳过确认)
  chimera doctor                                    # 6 维度健康检查
  chimera completions bash > /etc/bash_completion.d/chimera  # 生成补全脚本";

/// 子命令 EXAMPLES 映射(扩展:任何新增子命令只需在此添加一行)
const SUBCOMMAND_EXAMPLES: &[(&str, &str)] = &[
    (
        "run",
        "EXAMPLES:\n  chimera run \"实现 hello\"        # 单次任务\n  chimera --json run \"重构\" # JSON 输出",
    ),
    (
        "chat",
        "EXAMPLES:\n  chimera chat              # 启动 REPL\n  chimera --no-permission chat   # CI 模式",
    ),
    (
        "tui",
        "EXAMPLES:\n  chimera tui               # 默认启用 v3-engine\n  chimera tui --no-v3-engine # 回退 ratatui",
    ),
    (
        "quest",
        "EXAMPLES:\n  chimera quest list        # 列出\n  chimera --json quest show <id>  # 详情\n  chimera --yes quest cancel <id>  # 取消",
    ),
    (
        "llm",
        "EXAMPLES:\n  chimera llm list          # 列出 Provider\n  chimera llm test deepseek  # 探测\n  chimera llm set-default deepseek  # 设默认",
    ),
    (
        "doctor",
        "EXAMPLES:\n  chimera doctor            # 6 维检查\n  chimera --json doctor     # JSON",
    ),
];

/// 渲染 EXAMPLES 到任意 writer
///
/// 可测性下沉:`execute` 把 stdout 灌进来,测试把 `Vec<u8>` 灌进来,
/// 渲染逻辑保持纯函数,无 IO side effect。
///
/// # 参数
/// - `command`:`None` 输出顶级 5 EXAMPLES;`Some(name)` 查子命令映射
/// - `out`:接收输出的 writer(println! 风格追加 `\n`)
fn render_examples<W: Write>(command: Option<&str>, out: &mut W) -> std::io::Result<()> {
    match command {
        None => writeln!(out, "{TOP_EXAMPLES}")?,
        Some(name) => {
            if let Some((_, examples)) = SUBCOMMAND_EXAMPLES.iter().find(|(n, _)| *n == name) {
                writeln!(out, "{examples}")?;
            } else {
                // [E001] UserError:未知命令,引导用户跑 `chimera help` 看可用命令
                writeln!(
                    out,
                    "[E001] UserError: 未知命令 '{name}',运行 `chimera help` 查看可用命令"
                )?;
            }
        }
    }
    Ok(())
}

/// `chimera help [command]` 入口
///
/// `--no-examples` flag 在 `Cli` 上不存在(本 Task 不引入),先用简化路径。
/// `cli` 参数当前未消费,显式 `let _ = cli;` 抑制未使用警告,保留签名以备
/// 未来 `--json` / `--no-examples` 等扩展(Task 5.x 后续迭代)。
pub async fn execute(command: Option<&str>, cli: &Cli) -> Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    render_examples(command, &mut out)
        .map_err(|e| anyhow::anyhow!("写入 EXAMPLES 到 stdout 失败:{e}"))?;
    let _ = cli;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    /// 辅助:解析空 `chimera help` 调用所需的最小 `Cli`(只用于签名占位,
    /// `execute` 内部不消费其字段)
    fn stub_cli() -> Cli {
        Cli::parse_from(["chimera", "help"])
    }

    #[tokio::test]
    async fn no_arg_outputs_top_examples_containing_chimera_run() {
        let cli = stub_cli();
        execute(None, &cli).await.expect("help 无参执行成功");

        // 二次渲染到 Vec<u8> 用于断言(已渲染到 stdout 的内容同样来源 TOP_EXAMPLES)
        let mut buf: Vec<u8> = Vec::new();
        render_examples(None, &mut buf).expect("渲染顶级示例到 buffer 成功");
        let s = String::from_utf8(buf).expect("输出是合法 UTF-8");

        assert!(
            s.contains("EXAMPLES:"),
            "无参输出应含 EXAMPLES: 头:got={s:?}"
        );
        assert!(
            s.contains("chimera run"),
            "无参输出应含 chimera run 顶级示例:got={s:?}"
        );
    }

    #[tokio::test]
    async fn help_quest_outputs_containing_quest_list() {
        let cli = stub_cli();
        execute(Some("quest"), &cli)
            .await
            .expect("help quest 执行成功");

        let mut buf: Vec<u8> = Vec::new();
        render_examples(Some("quest"), &mut buf).expect("渲染 quest 示例成功");
        let s = String::from_utf8(buf).expect("输出是合法 UTF-8");

        assert!(
            s.contains("EXAMPLES:"),
            "quest 输出应含 EXAMPLES: 头:got={s:?}"
        );
        assert!(
            s.contains("quest list"),
            "quest 输出应含 quest list 示例:got={s:?}"
        );
    }

    #[tokio::test]
    async fn help_llm_outputs_containing_llm_list() {
        let cli = stub_cli();
        execute(Some("llm"), &cli).await.expect("help llm 执行成功");

        let mut buf: Vec<u8> = Vec::new();
        render_examples(Some("llm"), &mut buf).expect("渲染 llm 示例成功");
        let s = String::from_utf8(buf).expect("输出是合法 UTF-8");

        assert!(
            s.contains("llm list"),
            "llm 输出应含 llm list 示例:got={s:?}"
        );
    }

    #[tokio::test]
    async fn help_unknown_command_outputs_containing_e001() {
        let cli = stub_cli();
        execute(Some("nonexistent"), &cli)
            .await
            .expect("help nonexistent 执行成功(仅输出错误信息)");

        let mut buf: Vec<u8> = Vec::new();
        render_examples(Some("nonexistent"), &mut buf).expect("渲染未知命令错误成功");
        let s = String::from_utf8(buf).expect("输出是合法 UTF-8");

        assert!(
            s.contains("[E001]"),
            "未知命令输出应含 [E001] 错误码:got={s:?}"
        );
        assert!(
            s.contains("nonexistent"),
            "未知命令输出应回显错误命令名:got={s:?}"
        );
    }
}
