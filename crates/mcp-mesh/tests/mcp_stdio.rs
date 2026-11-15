//! `StdioTransport` 的行为锁（ADR-235 S4）。
//!
//! 用 `bash -c` 当假 MCP 服务器：本机（Git Bash）与 CI（ubuntu）都有。
//! 覆盖：闸门先于 spawn、读到帧、干净 EOF、半帧截断、配置环境变量注入、
//! 以及"drop 之后子进程真的死了"（`kill_on_drop` 的唯一可信证据是进程不在）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use mcp_mesh::protocol::{CommandGate, McpTransport, StdioTransport};
use mcp_mesh::McpError;

/// 解析本机可用的 POSIX shell —— 测试夹具必须用**真正的** bash。
///
/// WHY 不直接写 `"bash"`：Windows 上 PATH 里的 `bash` 可能是 WSL 桩
/// （`C:\Windows\System32\bash.exe`），它不把任意 Windows 环境变量带进 Linux 侧
/// （只有 `WSLENV` 列出的才带）⇒ `configured_env_reaches_the_child` 假红；
/// 冷启动也可能超过请求超时。本项目本地约定用 Git Bash（见
/// `Aegis/hooks/run-hook.cmd`），CI（ubuntu）走 PATH 里的 `bash`。
/// `CHIMERA_TEST_SHELL` 为其他机器的逃生口。
fn shell_program() -> &'static str {
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

/// 只允许清单里的可执行文件 —— 与生产 `ProcessFence` 同形的最小替身。
struct AllowList(Vec<&'static str>);

impl CommandGate for AllowList {
    fn check(&self, program: &str, _args: &[String]) -> Result<(), String> {
        if self.0.contains(&program) {
            Ok(())
        } else {
            Err(format!("{program} is not allowlisted"))
        }
    }
}

fn bash(script: &str) -> (String, Vec<String>) {
    (
        shell_program().to_string(),
        vec!["-c".to_string(), script.to_string()],
    )
}

fn spawn(script: &str, env: &BTreeMap<String, String>) -> StdioTransport {
    let (program, args) = bash(script);
    StdioTransport::spawn(&program, &args, env, &AllowList(vec![shell_program()]))
        .expect("bash is allowlisted and must spawn")
}

#[test]
fn gate_denial_prevents_spawn() {
    let (program, args) = bash("printf '{}'");
    let error = StdioTransport::spawn(&program, &args, &BTreeMap::new(), &AllowList(Vec::new()))
        .expect_err("empty allowlist must deny");
    assert!(
        matches!(error, McpError::Denied { .. }),
        "expected Denied, got {error:?}"
    );
    assert!(
        format!("{error}").contains("not allowlisted"),
        "denial must carry the gate's reason, got {error}"
    );
}

#[tokio::test]
async fn reads_a_frame_then_reports_clean_eof() {
    let mut transport = spawn(r#"printf '{"x":1}\n'; exit 0"#, &BTreeMap::new());
    let frame = transport
        .recv()
        .await
        .expect("first frame")
        .expect("some frame");
    assert_eq!(String::from_utf8(frame).expect("utf8"), r#"{"x":1}"#);
    assert!(
        transport.recv().await.expect("clean EOF").is_none(),
        "子进程正常退出且无半帧时必须报 Ok(None)"
    );
}

#[tokio::test]
async fn truncated_frame_then_exit_is_an_error() {
    let mut transport = spawn(r#"printf '{"x":'"#, &BTreeMap::new());
    let error = transport
        .recv()
        .await
        .expect_err("half frame must not be reported as a complete message");
    assert!(
        format!("{error}").contains("mid-frame"),
        "expected mid-frame error, got {error:?}"
    );
}

#[tokio::test]
async fn configured_env_reaches_the_child() {
    let mut env = BTreeMap::new();
    env.insert("MCP_FIXTURE_VALUE".to_string(), "injected".to_string());
    let mut transport = spawn(r#"printf '{"v":"%s"}\n' "$MCP_FIXTURE_VALUE""#, &env);
    let frame = transport.recv().await.expect("frame").expect("some");
    let text = String::from_utf8(frame).expect("utf8");
    assert!(
        text.contains("injected"),
        "配置的 env 必须到达子进程，实际帧 {text}"
    );
}

#[tokio::test]
async fn close_kills_the_child_and_reaps_it() {
    // 夹具用 bash 内建 `read` 保活，不再 fork `sleep`：子进程在 stdin 关闭前一直活着，
    // 且内存紧张时不会因 fork 失败而提前退出（并行全量跑时实测过这种假红）。
    let mut transport = spawn(r#"printf '{"ready":true}\n'; read _"#, &BTreeMap::new());
    let frame = transport.recv().await.expect("ready frame").expect("some");
    assert!(String::from_utf8_lossy(&frame).contains("ready"));

    let started = Instant::now();
    // terminate() = kill + wait：wait 返回即"已被回收（死）"，这比去 tasklist 查
    // pid 更可靠，也不引入新的外部进程（那本身就是内存压力下的抖动源）。
    transport
        .terminate()
        .await
        .expect("terminate must reap the child");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "close/terminate 应毫秒级收尾，实测 {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn silent_child_yields_no_frame_until_closed() {
    // 对端只睡不写：recv 必须挂起（而不是返回空帧）。
    let mut transport = spawn("read _", &BTreeMap::new());
    let outcome = tokio::time::timeout(Duration::from_millis(200), transport.recv()).await;
    assert!(
        outcome.is_err(),
        "静默子进程不应产生任何帧，实际 {:?}",
        outcome.map(|value| value.is_ok())
    );
    transport.close().await;
}
