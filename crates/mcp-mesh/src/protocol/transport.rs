//! 传输接缝：一帧 = 一条 JSON（不含分隔符），分隔符由实现负责。
//!
//! 三种实现：
//! - [`DuplexTransport`]：进程内双向管道，仅测试/夹具；
//! - [`StdioTransport`]：`spawn` 子进程 + 换行分帧（生产唯一实现，**默认不启用**，
//!   见下）；
//! - 将来的 HTTP/Streamable HTTP（本波不做，ADR-235 D2）。
//!
//! # 出站闸门（为什么是注入的 trait 而不是直接依赖 seccore）
//! 生产上出站命令必须过 SecCore `ProcessFence`（`agents.md §6.1` 裸奔红线）。
//! 但 `mcp-mesh` 与 `chimera-cli` 同为 L10，反向依赖会成环；而把整个 L4 `seccore`
//! 拖进本 crate 又只为一次 `check`。故这里定义 [`CommandGate`]，由组合根注入
//! `ProcessFence` 的适配器 —— **接缝在、防线在、依赖不增**。
//! 夹具里用"只允许 bash"的门，证明"被拒时不 spawn"。

use std::collections::BTreeMap;
use std::future::Future;
use std::process::Stdio;

use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, DuplexStream, ReadHalf,
    WriteHalf,
};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::error::McpError;

/// 单帧上限。`tools/list` 响应可能很大，但不能无界：无换行的对端会让缓冲区一直
/// 增长，必须在**读入过程中**就有一道闸门（超限即判错并断开），而不是读完再查。
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// 出站命令闸门。生产实现由组合根用 SecCore `ProcessFence` 适配。
pub trait CommandGate: Send + Sync {
    /// 允许则 `Ok(())`；拒绝则返回原因（会变成 [`McpError::Denied`]）。
    fn check(&self, program: &str, args: &[String]) -> Result<(), String>;
}

/// 传输接缝。实现方负责分帧（stdio/duplex 用换行；将来的 HTTP 用 EOF/SSE 事件）。
pub trait McpTransport: Send + 'static {
    /// 发送一帧（实现负责追加分隔符并 flush）。
    fn send(&mut self, frame: &[u8]) -> impl Future<Output = Result<(), McpError>> + Send;

    /// 接收一帧；`Ok(None)` 表示对端**正常关闭**（EOF 且无半帧）。
    ///
    /// 实现不得把 EOF 伪装成空帧，也不得把"半帧后 EOF"当成正常关闭 ——
    /// 两者都会让上层把截断当成完整消息。
    fn recv(&mut self) -> impl Future<Output = Result<Option<Vec<u8>>, McpError>> + Send;

    /// 关闭传输并释放对端资源。默认空实现；stdio 实现必须 kill + wait。
    ///
    /// WHY 单独一条而不是只靠 `kill_on_drop`：tokio 的进程回收是 blocking task，
    /// **运行时关闭会等子进程自然退出** —— 实测 `bash -c "sleep 30"` 被 drop 后
    /// 测试进程仍要等满 30s（换成 `sleep 3` 就等 3s）。显式 kill+wait 才能让
    /// `McpClient::shutdown` 与测试毫秒级收尾；`kill_on_drop` 保留作最后防线
    /// （panic 路径没人会调 close）。
    fn close(&mut self) -> impl Future<Output = ()> + Send {
        async {}
    }
}

fn transport(err: impl std::fmt::Display) -> McpError {
    McpError::Transport {
        reason: err.to_string(),
    }
}

/// 写一帧（换行分隔 + flush）；超限在写出前就拒。
async fn write_frame<W>(writer: &mut W, frame: &[u8], max_frame: usize) -> Result<(), McpError>
where
    W: AsyncWrite + Unpin,
{
    if frame.len() > max_frame {
        return Err(transport(format!(
            "outbound frame {} bytes exceeds limit {max_frame}",
            frame.len()
        )));
    }
    writer.write_all(frame).await.map_err(transport)?;
    writer.write_all(b"\n").await.map_err(transport)?;
    writer.flush().await.map_err(transport)?;
    Ok(())
}

/// 读一帧（换行分隔）；`Ok(None)` 仅在"干净的 EOF（无半帧）"时返回。
async fn read_frame<R>(reader: &mut R, max_frame: usize) -> Result<Option<Vec<u8>>, McpError>
where
    R: AsyncBufRead + Unpin,
{
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf().await.map_err(transport)?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err(transport(format!(
                    "connection closed mid-frame after {} bytes",
                    frame.len()
                )))
            };
        }
        if let Some(pos) = available.iter().position(|byte| *byte == b'\n') {
            if frame.len() + pos > max_frame {
                return Err(transport(format!(
                    "inbound frame exceeds limit {max_frame}"
                )));
            }
            frame.extend_from_slice(&available[..pos]);
            reader.consume(pos + 1);
            break;
        }
        let taken = available.len();
        if frame.len() + taken > max_frame {
            return Err(transport(format!(
                "inbound frame exceeds limit {max_frame}"
            )));
        }
        frame.extend_from_slice(available);
        reader.consume(taken);
    }
    // 容许 CRLF（某些实现会带 \r）；只剥一个尾随 \r。
    if frame.last() == Some(&b'\r') {
        frame.pop();
    }
    Ok(Some(frame))
}

/// 进程内双向管道传输（仅测试/夹具使用）。
pub struct DuplexTransport {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    max_frame: usize,
}

impl DuplexTransport {
    /// 用默认帧上限包装一端。
    #[must_use]
    pub fn new(stream: DuplexStream) -> Self {
        Self::with_limit(stream, MAX_FRAME_BYTES)
    }

    /// 指定帧上限（小上限用于测"超限必须报错"）。
    #[must_use]
    pub fn with_limit(stream: DuplexStream, max_frame: usize) -> Self {
        let (read, write) = tokio::io::split(stream);
        Self {
            reader: BufReader::new(read),
            writer: write,
            max_frame: max_frame.max(1),
        }
    }

    /// 配一对：返回（客户端传输, 服务器侧原始流）。服务器侧由测试按行读写。
    #[must_use]
    pub fn pair(capacity: usize) -> (Self, DuplexStream) {
        let (client, server) = tokio::io::duplex(capacity);
        (Self::new(client), server)
    }
}

impl McpTransport for DuplexTransport {
    async fn send(&mut self, frame: &[u8]) -> Result<(), McpError> {
        write_frame(&mut self.writer, frame, self.max_frame).await
    }

    async fn recv(&mut self) -> Result<Option<Vec<u8>>, McpError> {
        read_frame(&mut self.reader, self.max_frame).await
    }
}

/// 从父进程继承的环境变量白名单。
///
/// WHY 不是全量继承：第三方 MCP 服务器会拿到这份环境；把 `*_API_KEY`、`AWS_*`
/// 一并传过去就是凭据外泄（`agents.md §6.1` 的"裸奔"红线）。只保留能跑起来的
/// 最小集合，其余由配置显式注入。
const INHERITED_ENV: &[&str] = &[
    "PATH",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "HOME",
    "USERPROFILE",
    "TEMP",
    "TMP",
    "LANG",
    "LC_ALL",
];

/// stdio 传输：`spawn` 子进程，stdin/stdout 走换行分隔 JSON，stderr 直通。
///
/// # 默认不启用
/// 真实 MCP 服务器通常是 `npx`/`node`/`python`，而 SecCore `ProcessFence` 的
/// 白名单不含它们 ⇒ 组合根注入的真闸门会把这类命令拒成 [`McpError::Denied`]。
/// 这是**政策未裁**（ADR-235 D2），不是本实现的缺陷：本波只交付接缝，
/// 放行哪些可执行文件由 `ProcessFence` 清单决定（另立裁决）。
#[derive(Debug)]
pub struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    max_frame: usize,
}

impl StdioTransport {
    /// 过闸门 → spawn → 接管 stdin/stdout。
    ///
    /// `kill_on_drop(true)`：传输被丢弃时子进程随之终止，避免泄漏。
    /// `env_clear()` + 白名单继承 + 配置注入：见 [`INHERITED_ENV`] 的 WHY。
    pub fn spawn(
        program: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
        gate: &dyn CommandGate,
    ) -> Result<Self, McpError> {
        gate.check(program, args)
            .map_err(|reason| McpError::Denied {
                program: program.to_string(),
                reason,
            })?;
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .env_clear();
        for key in INHERITED_ENV {
            if let Ok(value) = std::env::var(key) {
                command.env(key, value);
            }
        }
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command.spawn().map_err(transport)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| transport("child stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| transport("child stdout unavailable"))?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            max_frame: MAX_FRAME_BYTES,
        })
    }

    /// 子进程 PID（排障/测试断言"已被杀死"用）。
    #[must_use]
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// 主动终止并**回收**子进程，返回退出状态。
    ///
    /// WHY 返回值而不是 `()`：`wait()` 返回即意味着子进程已被回收（死），这是
    /// 比"去 tasklist 查 pid"更可靠、也不依赖外部进程的存活判据。
    pub async fn terminate(&mut self) -> Result<std::process::ExitStatus, McpError> {
        self.child.start_kill().map_err(transport)?;
        self.child.wait().await.map_err(transport)
    }
}

impl McpTransport for StdioTransport {
    async fn send(&mut self, frame: &[u8]) -> Result<(), McpError> {
        write_frame(&mut self.stdin, frame, self.max_frame).await
    }

    async fn recv(&mut self) -> Result<Option<Vec<u8>>, McpError> {
        read_frame(&mut self.stdout, self.max_frame).await
    }

    async fn close(&mut self) {
        // wait 会把 tokio 的进程回收任务收干净；不做的话运行时关闭仍会等子进程。
        let _ = self.terminate().await;
    }
}
