//! 工具审批回路（第三波 B 批，ADR-232 D6 / 追加节）
//!
//! 对应架构层: L10 Interface（chimera-tui）
//!
//! # 这个模块解决什么
//! `chimera-cli` 的 LLM 工具循环在执行每个工具调用**之前**要问一句"可以吗"
//! （接缝是 `crate::llm::approval::ToolApproval::approve`）。CLI 侧有 stdin 可以问
//! （`CliApproval`），TUI 侧不能——TUI 拥有终端控制权，后台任务去读 stdin 会和主循环
//! 抢输入。于是 TUI 需要一条**任务 → UI 问人 → 决策回任务**的进程内回路：
//!
//! ```text
//! tool_loop (任务侧)                 TuiApp (UI 侧)
//!   approve(inv, Write)                poll_approvals() 每帧 try_recv
//!     ├─ tx.send((req, reply)) ──────▶   ├─ push PopupKind::Approval
//!     └─ reply.await  (不阻塞 UI)     ◀── └─ Yes/No/Esc/Close → send(bool)
//! ```
//!
//! # 为什么载荷在这里，而不是 L0 `nexus-contracts`
//! 依赖方向是 `chimera-cli → chimera-tui`（`crates/chimera-cli/Cargo.toml:35`），反向
//! 为零。因此**载荷可以定义在 tui 侧**，而 tokio 的 channel 句柄只出现在 cli 侧。
//! 载荷本身必须是**纯标量**：它要跨过 UI 的弹窗栈（`PopupKind` 参与序列化），而
//! `ToolInvocation` 属 L7、且不该被 L10 的 UI 类型命名。把载荷下沉到 L0 会踩 G-99
//! 纯度门并要求同批翻 L0 台账，规模与本能力不成比。
//!
//! # 与 `approval_mode` 的关系（勿混）
//! [`crate::approval_mode`] 是**会话档**三态（Normal/Plan/Auto，ADR-074 的 Shift+Tab）；
//! 本模块是**单次调用**的请求-应答。前者表达"这个会话准备怎么干"，后者承载"这一步
//! 到底让不让干"。二者正交，命名上刻意分开（mode = 档位，gate = 闸门）。
//!
//! # 与 `nexus_app_server::ApprovalArbiter` 的关系（勿混）
//! 那是宿主协议面的**多方投票**裁决器（`VoteOutcome`），与本模块的**单人 Yes/No 闸门**
//! 不是同一件事，也不共享类型。另注：CODE_WIKI 的 nexus-app-server 行把 `ApprovalGate`
//! 记作该裁决器的旧设计称谓（已改名 `ApprovalArbiter`）——本模块的 `ApprovalGate` 是
//! 在役的新类型，两者同名不同物，顺名检索前先看在役定义落在 `crates/chimera-tui`。
//!
//! # fail-closed
//! 任何非"显式同意"的路径都必须返回拒：弹窗 Esc/关闭、`send` 失败、`recv` 得
//! `Disconnected`、gate 未附加（headless/测试/CLI 档）。与本仓既有先例同律
//! （`chimera-cli/src/llm/approval.rs` 的 stdin 错误按拒处理）。

use tokio::sync::{mpsc, oneshot};

/// 一次写工具审批请求的可序列化载荷
///
/// 三个字段都是标量字符串：`call_id` 是决策的唯一键（ADR-232 拒绝项 R3 要求
/// per-invocation 粒度，见下），`tool`/`args_preview` 只用于给人看清楚要批准什么。
///
/// WHY 不带 `SideEffectDecl`/`ToolInvocation`：那些是 L7 类型，跨进 L10 的 UI 载荷
/// 等于让弹窗栈依赖执行层；审批所需的全部信息在 cli 侧已判定完毕，UI 只负责问人。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    /// 工具调用的唯一键 —— 决策只对这个 id 生效
    pub call_id: String,
    /// 工具名（如 `fs.write`）
    pub tool: String,
    /// 参数摘要（已在 cli 侧截断，避免把整段 payload 打进终端）
    pub args_preview: String,
}

/// 一次请求配一个一次性回执
pub type ApprovalTicket = (ApprovalRequest, oneshot::Sender<bool>);

/// 任务侧持有的审批闸门
///
/// WHY unbounded 而非 bounded：等待审批的是**已经在跑的 LLM 任务**，反压没有意义
/// （任务本来就得停下来等人），而 bounded 满了 `send` 会失败 ⇒ 只能读作"拒"，
/// 那等于用负载高低来决定放行与否——同一操作有时问人有时不问，不可复现。
/// 队列深度由真实并发度天然界定：单次循环内串行（`tool_loop` 的 for 逐条问），
/// 跨循环可并发（编排器每事件一 spawn），同刻 UI 只展示一张弹窗，后到者排队。
pub type ApprovalGate = mpsc::UnboundedSender<ApprovalTicket>;

/// 决策值只影响它自己的 `call_id`（R3 的机器表达）
///
/// 一次 `approve()` 消费一个 ticket：`oneshot::Sender::send(self)` 是一次性的，
/// 发完即失效，所以"批一次、后面全过"在这个形状里写不出来。若日后有人加批处理，
/// 「两个写调用同批、只批第一条」的用例与待批表键集断言会红。
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
mod tests {
    use super::*;

    fn req(id: &str) -> ApprovalRequest {
        ApprovalRequest {
            call_id: id.to_string(),
            tool: "fs.write".to_string(),
            args_preview: "{\"path\":\"a\"}".to_string(),
        }
    }

    /// 载荷是纯值类型：可 Clone、可按 call_id 比较（弹窗表用 call_id 作键的前提）
    #[test]
    fn approval_request_is_clonable_and_comparable() {
        let a = req("call-1");
        let b = a.clone();
        assert_eq!(a, b, "克隆必须等值，否则弹窗与待批表会比不出同一条");
        assert_ne!(
            a,
            req("call-2"),
            "不同 call_id 必须不等价（R3：决策不可互相顶替）"
        );
    }

    /// 闸门发送、接收端取到 ticket，且回执通道真的能把布尔值送回来
    ///
    /// WHY 在 B1 就测这条：B1 声称"零行为变化"，那通道本身必须是可用的——否则
    /// B3 换档时才发现通道是坏的，等于把骨架的空转当成已验证。
    #[tokio::test]
    async fn gate_delivers_ticket_and_reply_channel_carries_verdict() {
        let (tx, mut rx) = mpsc::unbounded_channel::<ApprovalTicket>();
        let (reply_tx, reply_rx) = oneshot::channel::<bool>();

        tx.send((req("call-9"), reply_tx))
            .expect("unbounded send 在无接收端时才会失败");

        let (ticket, ui_reply) = rx.recv().await.expect("应取到一条请求");
        assert_eq!(ticket.call_id, "call-9");
        assert_eq!(ticket.tool, "fs.write");

        ui_reply
            .send(true)
            .expect("任务侧仍持有接收端，send 不应失败");
        assert!(reply_rx.await.expect("回执不应断开"));
    }

    /// 接收端被丢弃 ⇒ 任务侧 `recv` 报错 ⇒ 上层据此判拒（fail-closed 的下半段）
    #[tokio::test]
    async fn dropped_receiver_is_visible_to_task_side() {
        let (tx, rx) = mpsc::unbounded_channel::<ApprovalTicket>();
        drop(rx);
        let (reply_tx, reply_rx) = oneshot::channel::<bool>();
        assert!(
            tx.send((req("call-x"), reply_tx)).is_err(),
            "闸门应报告通道已关"
        );
        assert!(
            reply_rx.await.is_err(),
            "发送端未送决策就 drop ⇒ 接收端应读到 Disconnected，据此走拒"
        );
    }
}
