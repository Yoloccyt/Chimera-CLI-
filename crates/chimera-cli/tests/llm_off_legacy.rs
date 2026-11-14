//! LLM 循环默认关闭时的旧行为契约（Phase 7 / T7.7）
//!
//! 这些用例的唯一目的：**证明没开开关时什么都没变**。全部走子进程，各自带 env，
//! 不与库内测试争抢进程级环境变量。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom
#![forbid(unsafe_code)]

use std::process::{Command, Output};

fn cli(envs: &[(&str, &str)], stdin: Option<&str>, args: &[&str]) -> Output {
    use std::io::Write;
    use std::process::Stdio;

    let mut command = Command::new(env!("CARGO_BIN_EXE_chimera"));
    command.args(args).env("RUST_LOG", "error");
    // 基线确定性：LLM 相关开关一律先清空
    command.env_remove("CHIMERA_LLM_LOOP");
    command.env_remove("CHIMERA_LLM_PROVIDER");
    command.env_remove("CHIMERA_TOOL_PLANE");
    for (key, value) in envs {
        command.env(key, value);
    }
    command.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    // WHY 显式 piped：`Command::spawn` 默认继承父进程 stdio —— 不显式接管的话，
    // 子进程输出会直接打到测试控制台，而 `output.stdout` 是空的（假红）。
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn chimera");
    if let Some(text) = stdin {
        child
            .stdin
            .as_mut()
            .expect("stdin piped")
            .write_all(text.as_bytes())
            .expect("write stdin");
    }
    child.wait_with_output().expect("collect output")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// 不传 `--llm`：旧 `run` 路径逐位不变（Quest 分解 + 打字机 + `[done]`）
#[test]
fn run_without_llm_keeps_legacy() {
    let output = cli(&[], None, &["--no-banner", "--no-color", "run", "hello"]);
    let stdout = text(&output.stdout);
    assert!(
        output.status.success(),
        "旧路径必须照常成功, stderr={}",
        text(&output.stderr)
    );
    assert!(stdout.contains("[done]"), "旧输出契约丢失: {stdout}");
    assert!(
        !stdout.contains("mock-echo"),
        "没开 --llm 却进了 LLM 循环: {stdout}"
    );
}

/// `--llm` + 离线回声档：必须看到循环产物（旧路径不可能产生该前缀）
#[test]
fn run_llm_flag_uses_loop() {
    let output = cli(
        &[("CHIMERA_LLM_PROVIDER", "mock")],
        None,
        &["--no-banner", "--no-color", "run", "--llm", "hello"],
    );
    let stdout = text(&output.stdout);
    assert!(
        output.status.success(),
        "循环必须跑通, stderr={}",
        text(&output.stderr)
    );
    assert!(
        stdout.contains("mock-echo: hello"),
        "--llm 未走循环或结果未上屏: {stdout}"
    );
    assert!(stdout.contains("[done]"), "流式收尾标记不得丢: {stdout}");
}

/// `--llm --json`：typed 载荷带停止原因与计数
#[test]
fn run_llm_json_reports_loop_counts() {
    let output = cli(
        &[("CHIMERA_LLM_PROVIDER", "mock")],
        None,
        &[
            "--no-banner",
            "--no-color",
            "--json",
            "run",
            "--llm",
            "count me",
        ],
    );
    assert!(output.status.success(), "stderr={}", text(&output.stderr));
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout 应为 JSON envelope");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["stop"], "final", "data={}", json["data"]);
    assert_eq!(json["data"]["turns"], 1, "回声档单轮即收敛");
    assert_eq!(json["data"]["tool_calls"], 0, "回声档不产工具调用");
    assert!(json["data"]["text"]
        .as_str()
        .unwrap_or_default()
        .contains("count me"));
}

/// 未配 provider 且未编译 `mca`：退出码 1 + 给出可执行提示（不静默回落 mock）
#[test]
fn run_llm_without_provider_errors_with_hint() {
    let output = cli(
        &[],
        None,
        &["--no-banner", "--no-color", "run", "--llm", "x"],
    );
    assert_eq!(output.status.code(), Some(1), "配置类错误应为退出码 1");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("mca") || stderr.contains("CHIMERA_LLM_PROVIDER"),
        "必须点名两条可用路径, stderr={stderr}"
    );
}

/// chat 的环境档同样默认关闭：`CHIMERA_LLM_LOOP` 未设时走旧 Quest 分解路径
#[test]
fn chat_llm_off_keeps_quest_path() {
    let output = cli(
        &[],
        Some("你好\n/exit\n"),
        &["--no-banner", "--no-color", "chat"],
    );
    let stdout = text(&output.stdout);
    assert!(
        !stdout.contains("mock-echo"),
        "chat 默认不得进 LLM 循环: {stdout}"
    );
}

/// chat 开环境档 ⇒ 走循环（与 run 共用同一灰度判据）
#[test]
fn chat_llm_env_enables_loop() {
    let output = cli(
        &[
            ("CHIMERA_LLM_LOOP", "apply"),
            ("CHIMERA_LLM_PROVIDER", "mock"),
        ],
        Some("你好\n/exit\n"),
        &["--no-banner", "--no-color", "chat"],
    );
    let stdout = text(&output.stdout);
    assert!(
        stdout.contains("mock-echo: 你好"),
        "环境档未让 chat 进循环: {stdout}"
    );
}
