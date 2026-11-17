//! End-to-end contracts for `chimera tool` (Phase 3).
//!
//! These tests drive the real binary, so they also lock the CLI envelope and
//! exit-code matrix. No network, no external services.

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_chimera")
}

fn run(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(bin());
    command.args(args).env("RUST_LOG", "error");
    command.env_remove("CHIMERA_TOOL_PLANE");
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("run chimera binary")
}

fn parse(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON: {error}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn plan_fixture() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tool_plan_diamond.json")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn tool_list_shows_echo_and_process() {
    let output = run(
        &["--no-banner", "--no-color", "--json", "tool", "list"],
        &[],
    );
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse(&output);
    assert_eq!(json["status"], "ok");
    // W1 翻转：fs.read/fs.list 接入 ⇒ 2 → 4（echo/proc/fs.read/fs.list）
    assert_eq!(json["data"]["count"], 4);
    let names: Vec<&str> = json["data"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(names.contains(&"echo.echo"));
    assert!(names.contains(&"proc.exec"));
}

#[test]
fn tool_list_works_when_feature_is_off() {
    let output = run(
        &["--no-banner", "--no-color", "--json", "tool", "list"],
        &[],
    );
    assert!(output.status.success());
}

#[test]
fn tool_call_off_errors_with_hint() {
    let output = run(
        &[
            "--no-banner",
            "--no-color",
            "--json",
            "tool",
            "call",
            "echo.echo",
        ],
        &[],
    );
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("CHIMERA_TOOL_PLANE"),
        "hint must mention the env switch, stderr={stderr}"
    );
}

#[test]
fn tool_call_echo_returns_payload() {
    let output = run(
        &[
            "--no-banner",
            "--no-color",
            "--json",
            "tool",
            "call",
            "echo.echo",
            "--args",
            r#"{"x":1}"#,
        ],
        &[("CHIMERA_TOOL_PLANE", "apply")],
    );
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse(&output);
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["status"], "ok");
    assert_eq!(json["data"]["payload"], r#"{"x":1}"#);
}

#[test]
fn tool_call_denied_exits_5() {
    let output = run(
        &[
            "--no-banner",
            "--no-color",
            "--yes",
            "--json",
            "tool",
            "call",
            "proc.exec",
            "--args",
            r#"{"program":"rm","args":["-rf","/"]}"#,
        ],
        &[("CHIMERA_TOOL_PLANE", "apply")],
    );
    assert_eq!(
        output.status.code(),
        Some(5),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("PermissionDenied"), "stderr={stderr}");
    assert!(stderr.contains("denied"), "stderr={stderr}");
}

#[test]
fn tool_call_timeout_exits_6() {
    let output = run(
        &[
            "--no-banner",
            "--no-color",
            "--json",
            "tool",
            "call",
            "echo.echo",
            "--args",
            r#"{"sleep_ms":500}"#,
            "--timeout-ms",
            "50",
        ],
        &[("CHIMERA_TOOL_PLANE", "apply")],
    );
    assert_eq!(
        output.status.code(),
        Some(6),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Timeout"), "stderr={stderr}");
}

#[test]
fn tool_plan_diamond_returns_summary() {
    let fixture = plan_fixture();
    let output = run(
        &[
            "--no-banner",
            "--no-color",
            "--json",
            "tool",
            "plan",
            "--file",
            &fixture,
        ],
        &[("CHIMERA_TOOL_PLANE", "apply")],
    );
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse(&output);
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["tool_calls"], 4);
    assert_eq!(json["data"]["nodes_executed"], 4);
    assert!(
        json["data"]["summary"]
            .as_str()
            .unwrap_or_default()
            .contains("sink"),
        "summary={}",
        json["data"]["summary"]
    );
}

#[test]
fn tool_dry_run_and_observe_do_not_execute() {
    for args in [
        vec![
            "--no-banner",
            "--no-color",
            "--dry-run",
            "--json",
            "tool",
            "call",
            "echo.echo",
        ],
        vec![
            "--no-banner",
            "--no-color",
            "--json",
            "tool",
            "call",
            "echo.echo",
        ],
    ] {
        let env = if args.contains(&"--dry-run") {
            vec![("CHIMERA_TOOL_PLANE", "apply")]
        } else {
            vec![("CHIMERA_TOOL_PLANE", "observe")]
        };
        let output = run(&args, &env);
        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json = parse(&output);
        assert!(
            json["data"]["would_execute"] == false,
            "dry-run/observe must not execute, got {}",
            json["data"]
        );
        assert_eq!(
            json["data"]["status"],
            if args.contains(&"--dry-run") {
                "dry_run"
            } else {
                "observe"
            }
        );
    }
}

#[test]
fn tool_plan_missing_file_exits_1() {
    let output = run(
        &[
            "--no-banner",
            "--no-color",
            "--json",
            "tool",
            "plan",
            "--file",
            "definitely-missing-plan.json",
        ],
        &[("CHIMERA_TOOL_PLANE", "apply")],
    );
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ConfigError"), "stderr={stderr}");
}

#[test]
fn help_tool_mentions_examples() {
    let output = run(&["--no-banner", "--no-color", "help", "tool"], &[]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("chimera tool list"), "stdout={stdout}");
}

#[test]
fn mcp_call_v2_returns_payload() {
    let output = run(
        &[
            "--no-banner",
            "--no-color",
            "--yes",
            "--json",
            "mcp",
            "call",
            "s1",
            "echo",
            "--endpoint",
            "203.0.113.1:8080",
        ],
        &[("CHIMERA_MCP_CALL_V2", "on")],
    );
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse(&output);
    assert_eq!(json["data"]["result"]["success"], true);
    let payload = json["data"]["result"]["payload"]
        .as_str()
        .unwrap_or_default();
    assert!(payload.contains("echo"), "payload={payload}");
    assert!(json["data"]["result"]["transaction_id"].is_null());
}

#[test]
fn mcp_call_v2_missing_endpoint_exits_1() {
    let output = run(
        &[
            "--no-banner",
            "--no-color",
            "--yes",
            "--json",
            "mcp",
            "call",
            "s1",
            "echo",
        ],
        &[("CHIMERA_MCP_CALL_V2", "on")],
    );
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("endpoint"), "stderr={stderr}");
}

#[test]
fn mcp_call_v1_unchanged() {
    let output = run(
        &[
            "--no-banner",
            "--no-color",
            "--yes",
            "--json",
            "mcp",
            "call",
            "s1",
            "echo",
        ],
        &[],
    );
    assert_eq!(
        output.status.code(),
        Some(3),
        "legacy 2PC path must still fail on unregistered server; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_plan_fixture() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tool_plan_write_proc.json")
        .to_string_lossy()
        .into_owned()
}

/// The *reason* a write node is refused is what proves approval travelled:
/// without `--yes` the provider must refuse for lack of approval (`ask_required`),
/// and with `--yes` the same node must get past that gate and be refused by the
/// process policy instead (`denied`). Before the plan path carried a real
/// invocation, the second case was unreachable — every node arrived with
/// `approved: false` no matter what the user passed.
#[test]
fn tool_plan_write_node_refuses_for_missing_approval() {
    let file = write_plan_fixture();
    let output = run(
        &[
            "--no-banner",
            "--no-color",
            "--json",
            "tool",
            "plan",
            "--file",
            &file,
        ],
        &[("CHIMERA_TOOL_PLANE", "apply")],
    );
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        stdout.contains("ask_required"),
        "expected an approval-required refusal, got stdout={stdout} stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn tool_plan_write_node_with_yes_reaches_process_policy() {
    let file = write_plan_fixture();
    let output = run(
        &[
            "--no-banner",
            "--no-color",
            "--yes",
            "--json",
            "tool",
            "plan",
            "--file",
            &file,
        ],
        &[("CHIMERA_TOOL_PLANE", "apply")],
    );
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        !stdout.contains("ask_required"),
        "--yes must clear the approval gate; stdout={stdout}"
    );
    assert!(
        stdout.contains("denied"),
        "expected the process policy to refuse `rm -rf /`, got stdout={stdout} stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}
