//! `McpClient`：一条连接上的 JSON-RPC 会话（握手 / 分页发现 / 工具调用）。
//!
//! # 结构
//! 传输由**一个后台任务**独占：它 `select!` 两个方向 —— 从 `mpsc` 取"要发的帧"、
//! 从传输取"收到的帧"。请求方只往 `mpsc` 投递并把一个 `oneshot` 放进 `pending`，
//! 响应回来时按 **id** 取回对应的 `oneshot`。
//!
//! # 为什么按 id 而不是按到达序
//! JSON-RPC 允许响应乱序到达，服务器也会并发处理。按到达序配对的实现会在
//! 并发调用下把 A 的结果交给 B —— 本文件的 `call_tool` 乱序用例就是这条的锁。
//!
//! # 失败语义（都是 fail-closed，不挂死）
//! - 单请求超时 ⇒ [`McpError::RequestTimeout`]，并把该 id 从 `pending` 摘掉；
//! - 对端 EOF / 读写出错 ⇒ 所有在途请求立即 [`McpError::Closed`]/原错误；
//! - 畸形帧 ⇒ 同上（无法归因到某个请求，宁可整条连接失败也不静默丢弃）。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Map, Value};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::error::McpError;
use crate::protocol::discovery::McpTool;
use crate::protocol::jsonrpc::{
    decode_incoming, encode_frame, Incoming, Notification, Request, RequestId,
};
use crate::protocol::transport::McpTransport;
use crate::protocol::{
    CLIENT_NAME, CLIENT_VERSION, MCP_PROTOCOL_VERSION, SUPPORTED_PROTOCOL_VERSIONS,
};

/// 默认单请求超时。
pub const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 30_000;

/// `tools/list` 的最大翻页数（防"永远给 nextCursor"的对端把发现拖成死循环）。
pub const MAX_TOOL_PAGES: usize = 64;

type Pending = Arc<Mutex<HashMap<RequestId, oneshot::Sender<Result<Value, McpError>>>>>;

/// 客户端配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientConfig {
    /// 单请求超时（毫秒）；`0` 表示用 [`DEFAULT_REQUEST_TIMEOUT_MS`]。
    pub request_timeout_ms: u64,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            request_timeout_ms: DEFAULT_REQUEST_TIMEOUT_MS,
        }
    }
}

impl ClientConfig {
    fn timeout(&self) -> Duration {
        Duration::from_millis(if self.request_timeout_ms == 0 {
            DEFAULT_REQUEST_TIMEOUT_MS
        } else {
            self.request_timeout_ms
        })
    }
}

/// `initialize` 返回的服务器信息（只保留我们真的会用到的字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    /// 服务器最终选择的协议版本（已校验在 [`SUPPORTED_PROTOCOL_VERSIONS`] 内）
    pub protocol_version: String,
    /// 服务器名（`serverInfo.name`，缺失即空串）
    pub name: String,
    /// 服务器版本（`serverInfo.version`，缺失即空串）
    pub version: String,
}

/// 一条 MCP 连接上的客户端。
pub struct McpClient {
    tx: mpsc::Sender<Vec<u8>>,
    pending: Pending,
    next_id: AtomicU64,
    timeout: Duration,
    closed: Arc<AtomicBool>,
    reader: JoinHandle<()>,
}

fn lock_pending(
    pending: &Pending,
) -> std::sync::MutexGuard<'_, HashMap<RequestId, oneshot::Sender<Result<Value, McpError>>>> {
    // 锁只保护一个 map，临界区内没有任何 await；中毒时取回内层（记录本身仍然可用）。
    pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn fail_all(pending: &Pending, error: McpError) {
    let drained: Vec<_> = {
        let mut map = lock_pending(pending);
        map.drain().map(|(_id, sender)| sender).collect()
    };
    for sender in drained {
        let _ = sender.send(Err(error.clone()));
    }
}

/// 处理一条入站帧；`Fatal` 表示这条连接不能再用了。
enum Step {
    Continue,
    Fatal(McpError),
}

fn dispatch(frame: &[u8], pending: &Pending) -> Step {
    match decode_incoming(frame) {
        Ok(Incoming::Response(response)) => {
            let outcome = match (response.result, response.error) {
                (Some(value), _) => Ok(value),
                (None, Some(error)) => Err(McpError::Remote {
                    code: error.code,
                    message: error.message,
                }),
                // 解码已保证恰有其一，这里只是把不可达分支写成显式错误。
                (None, None) => Err(McpError::Protocol {
                    reason: "response carried neither result nor error".into(),
                }),
            };
            let sender = lock_pending(pending).remove(&response.id);
            match sender {
                Some(sender) => {
                    let _ = sender.send(outcome);
                }
                None => {
                    // 迟到响应（请求已超时被摘除）或服务器乱发的 id：静默丢弃。
                    tracing::trace!(id = %response.id, "dropping response for unknown request id");
                }
            }
            Step::Continue
        }
        Ok(Incoming::Notification(notification)) => {
            tracing::trace!(method = %notification.method, "mcp notification received");
            Step::Continue
        }
        Ok(Incoming::Request(request)) => {
            // 本波不声明任何客户端能力；服务器反向调用属越界，记下来但不回包。
            tracing::warn!(
                method = %request.method,
                "server-initiated request ignored (no client capabilities advertised)"
            );
            Step::Continue
        }
        Err(error) => Step::Fatal(error),
    }
}

impl McpClient {
    /// 在**当前 Tokio 运行时内**启动客户端（后台任务独占传输）。
    ///
    /// 必须在 runtime 上下文调用（内部 `tokio::spawn`）；传输在任务退出时被 drop，
    /// 对 `StdioTransport` 即"子进程随之终止"（`kill_on_drop`）。
    #[must_use]
    pub fn spawn<T: McpTransport>(mut transport: T, config: ClientConfig) -> Self {
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(64);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let timeout = config.timeout();

        let task_pending = Arc::clone(&pending);
        let task_closed = Arc::clone(&closed);
        let reader = tokio::spawn(async move {
            loop {
                tokio::select! {
                    outgoing = rx.recv() => match outgoing {
                        Some(frame) => {
                            if let Err(error) = transport.send(&frame).await {
                                task_closed.store(true, Ordering::SeqCst);
                                fail_all(&task_pending, error);
                                break;
                            }
                        }
                        // 所有发送端已丢弃（客户端被 drop 或 shutdown）⇒ 收工。
                        None => break,
                    },
                    incoming = transport.recv() => match incoming {
                        Ok(Some(frame)) => match dispatch(&frame, &task_pending) {
                            Step::Continue => {}
                            Step::Fatal(error) => {
                                task_closed.store(true, Ordering::SeqCst);
                                fail_all(&task_pending, error);
                                break;
                            }
                        },
                        Ok(None) => {
                            task_closed.store(true, Ordering::SeqCst);
                            fail_all(&task_pending, McpError::Closed);
                            break;
                        }
                        Err(error) => {
                            task_closed.store(true, Ordering::SeqCst);
                            fail_all(&task_pending, error);
                            break;
                        }
                    },
                }
            }
            // 显式收尾：stdio 的 close 是 kill+wait；不做的话运行时关闭会等子进程
            // 自然退出（实测 sleep 30 会让进程额外停 30s）。
            transport.close().await;
        });

        Self {
            tx,
            pending,
            next_id: AtomicU64::new(0),
            timeout,
            closed,
            reader,
        }
    }

    /// 连接是否已关闭（EOF / 读写出错 / `shutdown` 之后）。
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// 仍在等响应的请求数（观测/测试用）。
    #[must_use]
    pub fn in_flight(&self) -> usize {
        lock_pending(&self.pending).len()
    }

    fn remove_pending(&self, id: &RequestId) {
        let _ = lock_pending(&self.pending).remove(id);
    }

    /// 发一个请求并等它的响应；超时会摘掉 pending 项（不泄漏）。
    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<Value, McpError> {
        if self.is_closed() {
            return Err(McpError::Closed);
        }
        let id = RequestId::Number(self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        let (reply_tx, reply_rx) = oneshot::channel();
        {
            let mut map = lock_pending(&self.pending);
            map.insert(id.clone(), reply_tx);
        }
        let frame = encode_frame(&Request {
            jsonrpc: "2.0".into(),
            id: id.clone(),
            method: method.to_string(),
            params,
            extra: Map::new(),
        })?;
        if self.tx.send(frame).await.is_err() {
            self.remove_pending(&id);
            return Err(McpError::Closed);
        }
        match tokio::time::timeout(self.timeout, reply_rx).await {
            Ok(Ok(outcome)) => outcome,
            // 发送端随任务退出而 drop —— 对端已不可用。
            Ok(Err(_dropped)) => Err(McpError::Closed),
            Err(_elapsed) => {
                self.remove_pending(&id);
                Err(McpError::RequestTimeout {
                    method: method.to_string(),
                    timeout_ms: self.timeout.as_millis() as u64,
                })
            }
        }
    }

    /// 发一个通知（不期待响应）。
    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), McpError> {
        if self.is_closed() {
            return Err(McpError::Closed);
        }
        let frame = encode_frame(&Notification {
            jsonrpc: "2.0".into(),
            method: method.to_string(),
            params,
            extra: Map::new(),
        })?;
        self.tx.send(frame).await.map_err(|_| McpError::Closed)
    }

    /// `initialize` 握手 + `notifications/initialized` 通知。
    ///
    /// 版本策略（ADR-235 D4）：请求 [`MCP_PROTOCOL_VERSION`]；服务器回别的版本时，
    /// 只有落在 [`SUPPORTED_PROTOCOL_VERSIONS`] 内才接受，否则
    /// [`McpError::UnsupportedVersion`] —— 不降级猜测。
    pub async fn initialize(&self) -> Result<ServerInfo, McpError> {
        let params = json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": CLIENT_NAME, "version": CLIENT_VERSION },
        });
        let result = self.request("initialize", Some(params)).await?;
        let protocol_version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !SUPPORTED_PROTOCOL_VERSIONS.contains(&protocol_version.as_str()) {
            return Err(McpError::UnsupportedVersion {
                requested: MCP_PROTOCOL_VERSION.to_string(),
                got: protocol_version,
            });
        }
        self.notify("notifications/initialized", None).await?;
        let server = result.get("serverInfo");
        Ok(ServerInfo {
            protocol_version,
            name: server
                .and_then(|info| info.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            version: server
                .and_then(|info| info.get("version"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
    }

    /// `tools/list`，自动跟随 `nextCursor` 翻页并按名去重（保序）。
    ///
    /// 防两类"对端不守规矩"：游标重复（分页环）与页数超 [`MAX_TOOL_PAGES`]。
    pub async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        let mut tools: Vec<McpTool> = Vec::new();
        let mut seen_names: HashSet<String> = HashSet::new();
        let mut seen_cursors: HashSet<String> = HashSet::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0usize;
        loop {
            pages += 1;
            if pages > MAX_TOOL_PAGES {
                return Err(McpError::Protocol {
                    reason: format!("tools/list exceeded {MAX_TOOL_PAGES} pages"),
                });
            }
            let params = cursor.as_ref().map(|value| json!({ "cursor": value }));
            let result = self.request("tools/list", params).await?;
            let page: ToolPage =
                serde_json::from_value(result).map_err(|error| McpError::Protocol {
                    reason: format!("tools/list payload malformed: {error}"),
                })?;
            for tool in page.tools {
                if seen_names.insert(tool.name.clone()) {
                    tools.push(tool);
                } else {
                    tracing::warn!(tool = %tool.name, "tools/list returned a duplicate name; keeping the first");
                }
            }
            match page.next_cursor {
                Some(next) => {
                    if !seen_cursors.insert(next.clone()) {
                        return Err(McpError::Protocol {
                            reason: format!("tools/list pagination loop at cursor {next:?}"),
                        });
                    }
                    cursor = Some(next);
                }
                None => break,
            }
        }
        Ok(tools)
    }

    /// `tools/call` —— 返回服务器给的原始 result（内容块等由上层解释）。
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, McpError> {
        self.request(
            "tools/call",
            Some(json!({ "name": name, "arguments": arguments })),
        )
        .await
    }

    /// 关闭连接：丢发送端（后台任务随之退出并 drop 传输），最多等 5s 收尾。
    ///
    /// 幂等性由类型保证：消费 `self`，第二次调用不可能发生；对已 EOF 的连接
    /// 调用也不阻塞（任务已退出，join 立即返回）。
    pub async fn shutdown(self) {
        self.closed.store(true, Ordering::SeqCst);
        drop(self.tx);
        let _ = tokio::time::timeout(Duration::from_secs(5), self.reader).await;
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolPage {
    #[serde(default)]
    tools: Vec<McpTool>,
    #[serde(default)]
    next_cursor: Option<String>,
}
