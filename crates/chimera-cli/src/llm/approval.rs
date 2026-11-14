//! 工具审批策略（Phase 7，ADR-232）
//!
//! 对应架构层: L10 Interface（chimera-cli）
//!
//! # 为什么不把审批塞进 `ToolRuntime`
//! L7 的 `ToolRuntime` 只认 `ToolInvocation::approved` 这一**已决策事实**
//! （外加可选 `ExecPolicy` 硬拒）。"要不要问人"属于交互面（CLI 有 stdin、TUI 有弹窗、
//! 常驻宿主有协议），因此判定发生在 L10，判完才把 `approved` 交给 L7。
//!
//! # 四档语义
//! - [`DenyWrites`]：只读自动放行、写一律拒 —— 非交互与默认档（fail-closed）；
//! - [`CliApproval`]：只读自动放行、写走 `permission::confirm`（`--yes` / `--no-permission`
//!   时由 ctx 自行短路）；
//! - [`TuiApproval`]：只读自动放行、写逐条经 TUI 弹窗问人 —— TUI 持有终端控制权，
//!   后台任务不能读 stdin，故与 `CliApproval` 分档；闸门未接线时**拒**而非退回问 stdin；
//! - [`ApproveAll`]：全放行 —— 仅测试与受信本地自动化使用，调用方须显式选择。
//!
//! 命名与方案 §5.7 的偏差：方案另列 `AutoApproveReadOnly`，但其"只读自动 + 写不批"
//! 与 `DenyWrites` 完全同义；保留两个同义类型会让读者以为行为有差别，故只留一个。

use async_trait::async_trait;
use chimera_tui::approval_gate::{ApprovalGate, ApprovalRequest};
use gqep_executor::tool_plane::ToolInvocation;
use nexus_contracts::tool_plan::SideEffectDecl;
use tokio::sync::oneshot;

use crate::permission::{self, PermissionCtx};

/// 工具审批契约
#[async_trait]
pub trait ToolApproval: Send + Sync {
    /// 返回 `false` = **不执行**该调用；循环会回填 `ask_required` 错误结果给模型，
    /// 而不是静默丢弃 —— 模型必须知道自己被拒，否则会用同样参数重试到轮数上限。
    async fn approve(&self, inv: &ToolInvocation, side_effect: SideEffectDecl) -> bool;
}

/// 只读自动放行、写拒绝（默认档）
#[derive(Debug, Default, Clone, Copy)]
pub struct DenyWrites;

#[async_trait]
impl ToolApproval for DenyWrites {
    async fn approve(&self, _inv: &ToolInvocation, side_effect: SideEffectDecl) -> bool {
        !matches!(side_effect, SideEffectDecl::Write)
    }
}

/// 全部放行（测试 / 受信本地自动化）
#[derive(Debug, Default, Clone, Copy)]
pub struct ApproveAll;

#[async_trait]
impl ToolApproval for ApproveAll {
    async fn approve(&self, _inv: &ToolInvocation, _side_effect: SideEffectDecl) -> bool {
        true
    }
}

/// CLI 审批：写工具逐条 `permission::confirm`
#[derive(Debug, Clone)]
pub struct CliApproval {
    ctx: PermissionCtx,
}

impl CliApproval {
    /// 以当前 permission 上下文构造（`--yes` / `--no-permission` 由 ctx 自行短路）
    #[must_use]
    pub fn new(ctx: PermissionCtx) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolApproval for CliApproval {
    async fn approve(&self, inv: &ToolInvocation, side_effect: SideEffectDecl) -> bool {
        if !matches!(side_effect, SideEffectDecl::Write) {
            return true;
        }
        let details = format!("tool={}, args={}", inv.tool, trunc_args(&inv.args_json));
        // `confirm` 的错误（stdin 不可用等）按"未获批准"处理：宁可少做，不可多做
        match permission::confirm(&self.ctx, "执行 LLM 请求的写工具", &details).await {
            Ok(approved) => approved,
            Err(error) => {
                tracing::warn!(tool = %inv.tool, error = %error, "写工具审批不可用,按拒绝处理");
                false
            }
        }
    }
}

/// 审批提示里的参数摘要截断（避免把整段 payload 打进终端）
fn trunc_args(args: &str) -> String {
    const MAX_CHARS: usize = 160;
    let chars: Vec<char> = args.chars().collect();
    if chars.len() <= MAX_CHARS {
        return args.to_string();
    }
    let mut head: String = chars[..MAX_CHARS].iter().collect();
    head.push_str("…(truncated)");
    head
}

/// TUI 弹窗审批：写工具逐条经 [`ApprovalGate`] 问人（第三波 B 批，ADR-232 D6）
///
/// 与 `CliApproval` 的分工：两者都在**副作用发生之前**判定，差别只在问人的介质
/// —— CLI 读 stdin，TUI 走进程内通道把问题交给帧循环弹出的确认弹窗。TUI 持有终端
/// 控制权，后台任务去读 stdin 会与 REPL 抢输入，故必须有这条独立档位。
///
/// WHY 不需要锁：批准计划的写法是"sender 用 `std::Mutex` 保护、锁内 clone 出锁再
/// await"（§4.4 #1 的形）。实测 `tokio::mpsc::UnboundedSender::send` 取 `&self`，
/// 即发送端本身可并发共享，不需要内部可变性 —— 于是**一条锁都不引入**，持锁跨
/// await 的风险面直接归零而不是被规避。
///
/// fail-closed 的三条出口：闸门已关（UI 退出/句柄被 drop）、回执通道断（弹窗未答
/// 就消失）、以及本档未附加（`None` ⇒ 由调用方回落到 `DenyWrites`）。三条全部返回
/// 拒，与 `CliApproval` 对 stdin 错误的处理同律。
pub struct TuiApproval {
    gate: ApprovalGate,
}

impl TuiApproval {
    /// 以审批闸门构造（发送端可廉价克隆，故按值持有）
    pub fn new(gate: ApprovalGate) -> Self {
        Self { gate }
    }
}

#[async_trait]
impl ToolApproval for TuiApproval {
    async fn approve(&self, inv: &ToolInvocation, side_effect: SideEffectDecl) -> bool {
        // 只读自动放行：与 DenyWrites / CliApproval 同档，避免"问人疲劳"把真正的
        // 写调用淹没在噪声里（一次循环动辄数十次只读探测）。
        if !matches!(side_effect, SideEffectDecl::Write) {
            return true;
        }

        let (reply_tx, reply_rx) = oneshot::channel::<bool>();
        let request = ApprovalRequest {
            // 键 = call_id：一次 approve 消费一张 ticket（`send(self)` 一次性），
            // 这就是 ADR-232 拒绝项 R3"不得批量背书"的机器保证。
            call_id: inv.call_id.clone(),
            tool: inv.tool.clone(),
            args_preview: trunc_args(&inv.args_json),
        };

        if self.gate.send((request, reply_tx)).is_err() {
            tracing::warn!(
                tool = %inv.tool,
                call_id = %inv.call_id,
                approval_gate_open = false,
                "审批闸门已关闭,按拒绝处理"
            );
            return false;
        }

        match reply_rx.await {
            Ok(allowed) => allowed,
            Err(err) => {
                // UI 侧未作答即 drop（弹窗被别的路径摘掉、app 退出、任务被取消）
                tracing::warn!(
                    tool = %inv.tool,
                    call_id = %inv.call_id,
                    approval_reply = %err,
                    "审批未获答复,按拒绝处理"
                );
                false
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
mod tests {
    use super::*;
    use chimera_tui::approval_gate::{ApprovalGate, ApprovalTicket};
    use gqep_executor::tool_plane::{ToolInvocation, ToolOrigin, ToolPriority};
    use std::time::Duration;

    fn inv(call_id: &str) -> ToolInvocation {
        ToolInvocation {
            call_id: call_id.to_string(),
            tool: "fs.write".to_string(),
            args_json: format!("{{\"path\":\"{call_id}.env\"}}"),
            timeout_ms: None,
            priority: ToolPriority::Normal,
            origin: ToolOrigin::Llm,
            quest_id: None,
            task_id: None,
            approved: false,
        }
    }

    /// 建一对闸门；返回（发送端，接收端）
    fn channel() -> (
        ApprovalGate,
        tokio::sync::mpsc::UnboundedReceiver<ApprovalTicket>,
    ) {
        tokio::sync::mpsc::unbounded_channel()
    }

    /// 只读调用**不占问人预算**：直接放行，且闸门里不得出现任何请求
    ///
    /// WHY 断言通道为空：一次循环动辄几十次只读探测，若实现改成"全都问人"，
    /// 用户会被噪声淹没到开始随手按 Yes —— 那比不审批更危险。
    #[tokio::test]
    async fn readonly_is_allowed_without_touching_the_gate() {
        let (gate, mut rx) = channel();
        let approval = TuiApproval::new(gate);
        assert!(
            approval
                .approve(&inv("r-1"), SideEffectDecl::ReadOnly)
                .await
        );
        assert!(rx.try_recv().is_err(), "只读调用不该产生待答问题");
    }

    /// 写调用 + 人答 Yes ⇒ 允许
    #[tokio::test]
    async fn write_with_yes_is_allowed() {
        let (gate, mut rx) = channel();
        let approval = std::sync::Arc::new(TuiApproval::new(gate));
        let task = {
            let a = std::sync::Arc::clone(&approval);
            tokio::spawn(async move { a.approve(&inv("w-1"), SideEffectDecl::Write).await })
        };

        let (request, reply) = rx.recv().await.expect("应收到一条审批请求");
        assert_eq!(request.call_id, "w-1");
        assert_eq!(request.tool, "fs.write");
        reply.send(true).expect("任务侧仍在等");
        assert!(task.await.expect("审批任务不应 panic"));
    }

    /// 写调用 + 人答 No ⇒ 拒（与 Yes 走同一通道，只有值不同）
    #[tokio::test]
    async fn write_with_no_is_denied() {
        let (gate, mut rx) = channel();
        let approval = TuiApproval::new(gate);
        let task =
            tokio::spawn(async move { approval.approve(&inv("w-2"), SideEffectDecl::Write).await });
        let (_request, reply) = rx.recv().await.expect("应收到一条审批请求");
        reply.send(false).expect("任务侧仍在等");
        assert!(!task.await.expect("审批任务不应 panic"));
    }

    /// 未获答复（UI 侧 drop 回执）⇒ 拒。fail-closed 的主断言。
    #[tokio::test]
    async fn unanswered_write_is_denied_when_reply_channel_drops() {
        let (gate, mut rx) = channel();
        let approval = TuiApproval::new(gate);
        let task =
            tokio::spawn(async move { approval.approve(&inv("w-3"), SideEffectDecl::Write).await });
        let (_request, reply) = rx.recv().await.expect("应收到一条审批请求");
        drop(reply); // 弹窗被别的路径摘掉 / app 退出
        assert!(
            !task.await.expect("审批任务不应 panic"),
            "没有显式同意就必须按拒，不能等到超时也不能默认放行"
        );
    }

    /// 闸门已关（UI 从未启动或已退出）⇒ 拒，且**不阻塞**
    #[tokio::test]
    async fn write_is_denied_when_gate_is_closed() {
        let (gate, rx) = channel();
        drop(rx);
        let approval = TuiApproval::new(gate);
        assert!(
            !approval.approve(&inv("w-4"), SideEffectDecl::Write).await,
            "无 UI 可问时不得放行写调用"
        );
    }

    /// R3 的机器表达：一次允许**不为后续调用背书**
    ///
    /// 若有人把审批改成"记住本会话已批准 fs.write"，本用例必红：第二条永远问不到人。
    ///
    /// WHY 应答放独立任务而不是把 `approval` spawn 进任务：`approve(&self,..)` 借用了
    /// `approval`，被 move 进第一个任务就再也发不出第二条（E0382）。改为「主任务顺序
    /// await 两次、应答端在另一任务里排队收票」，同时多证一件事 —— **两条票是按调用
    /// 顺序各发一次的**，不是发一次复用。
    #[tokio::test]
    async fn one_yes_never_approves_the_next_call() {
        let (gate, mut rx) = channel();
        let approval = TuiApproval::new(gate);
        let responder = tokio::spawn(async move {
            let (req1, reply1) = rx.recv().await.expect("第一条应到达");
            assert_eq!(req1.call_id, "c-1");
            reply1.send(true).expect("第一条仍在等");
            let (req2, reply2) = rx.recv().await.expect("第二条应重新问人");
            assert_eq!(req2.call_id, "c-2", "决策键必须是 call_id 而非工具名");
            reply2.send(false).expect("第二条仍在等");
        });

        let first = approval.approve(&inv("c-1"), SideEffectDecl::Write).await;
        let second = approval.approve(&inv("c-2"), SideEffectDecl::Write).await;

        // 超时而非无限等：若实现记住了"已批过 c-1"而不再发票，recv() 会永远 Pending，
        // 那正是本用例要抓的退化 —— 必须红，不能挂。
        tokio::time::timeout(Duration::from_secs(5), responder)
            .await
            .expect("应答端应在超时内收齐两条票")
            .expect("应答端不 panic");
        assert!(first, "c-1 答 Yes ⇒ 放行");
        assert!(!second, "c-2 答 No ⇒ 拒绝，且不受 c-1 影响");
    }

    /// `args_preview` 必须是截断后的摘要，不能把整段 payload 打进终端
    ///
    /// 刻意用**远超上限**的参数：短参数下"预览足够短"恒真，那样这条测试证明的是
    /// 测试自己而不是实现。断言落在三处：变短了、带上截断标记、原值仍是长的（对照）。
    #[tokio::test]
    async fn args_preview_is_truncated_not_raw() {
        let (gate, mut rx) = channel();
        let approval = TuiApproval::new(gate);
        let mut long = inv("c-long");
        let blob = "x".repeat(500);
        long.args_json = format!("{{\"blob\":\"{blob}\"}}");
        let original_len = long.args_json.chars().count();

        let task =
            tokio::spawn(async move { approval.approve(&long, SideEffectDecl::Write).await });
        let (request, reply) = rx.recv().await.expect("应收到请求");

        assert!(
            request.args_preview.chars().count() < original_len,
            "预览必须短于原 payload（原值 {original_len} 字，实得 {}）",
            request.args_preview.chars().count()
        );
        assert!(
            request.args_preview.ends_with("…(truncated)"),
            "被截断时必须自证被截断，否则人会以为看到了完整参数"
        );
        reply.send(false).expect("仍在等");
        let _ = task.await;
    }
}
