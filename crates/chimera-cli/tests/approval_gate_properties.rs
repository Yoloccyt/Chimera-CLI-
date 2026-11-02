//! 写工具审批的决策不串号属性（第三波 B 批，ADR-232 拒绝项 R3）
//!
//! 对应架构层: L10 Interface（chimera-cli）
//!
//! # 为什么用属性测试而不是只写例子
//! R3 的主张是**全称**的："任何一次允许都不为后续任何调用背书"。例子只能覆盖想到的
//! 形状；这里把 call_id 集合、每条的答案值、以及"哪些条目故意没人答"全部交给 proptest
//! 生成，用来穷举"两条答案相反"、"同 id 被问两次"、"部分未答"这类容易漏的组合。
//!
//! # 三条性质
//! 1. **不串号**：每个 `call_id` 的返回值恰等于人给它的答案，与其他条目的答案无关；
//! 2. **未答恒拒**：回执被 drop（弹窗被摘/app 退出）的那些一律 `false`；
//! 3. **一问一答不重不漏**：n 次 `approve` ⇒ 闸门里恰有 n 条请求，键集与发起集相等。

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code

use std::collections::{BTreeMap, BTreeSet};

use chimera_cli::llm::approval::{ToolApproval, TuiApproval};
use chimera_tui::approval_gate::{ApprovalGate, ApprovalTicket};
use gqep_executor::tool_plane::{ToolInvocation, ToolOrigin, ToolPriority};
use nexus_contracts::tool_plan::SideEffectDecl;
use proptest::prelude::*;
use tokio::sync::mpsc;

/// 造一次写工具调用（只有 `call_id` 参与审批判定，其余字段填合法值即可）
fn write_call(call_id: &str) -> ToolInvocation {
    ToolInvocation {
        call_id: call_id.to_string(),
        tool: "fs.write".to_string(),
        args_json: "{}".to_string(),
        timeout_ms: None,
        priority: ToolPriority::Normal,
        origin: ToolOrigin::Llm,
        quest_id: None,
        task_id: None,
        approved: false,
    }
}

/// 跑一轮完整审批：每条写调用各自问人，`answers` 给出答案，`silence` 里的故意不回
///
/// 返回 `call_id -> 审批结果`。
fn run_round(
    answers: &BTreeMap<String, bool>,
    silence: &BTreeSet<String>,
) -> BTreeMap<String, bool> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime 应可建");

    runtime.block_on(async {
        let (gate, mut rx): (ApprovalGate, mpsc::UnboundedReceiver<ApprovalTicket>) =
            mpsc::unbounded_channel();
        let approval = std::sync::Arc::new(TuiApproval::new(gate));

        // 先全部发起：unbounded 通道 + 每条一个 oneshot，任务之间不会互锁
        let mut ids: Vec<String> = answers.keys().cloned().collect();
        ids.extend(silence.iter().cloned());
        let mut joins = Vec::with_capacity(ids.len());
        for id in ids.clone() {
            let asker = std::sync::Arc::clone(&approval);
            let invocation = write_call(&id);
            joins.push((
                id,
                tokio::spawn(
                    async move { asker.approve(&invocation, SideEffectDecl::Write).await },
                ),
            ));
        }

        // 逐条作答。键取自**请求本身**而非发送顺序：把 id 与答案的对应关系交给
        // 通道传递，正是本测试要验的那件事，不能靠顺序作弊。
        let total = answers.len() + silence.len();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        while seen.len() < total {
            let Some((request, reply)) = rx.recv().await else {
                break;
            };
            assert!(
                seen.insert(request.call_id.clone()),
                "同一 call_id 被问了两次: {}",
                request.call_id
            );
            match answers.get(&request.call_id) {
                // 任务侧可能已被取消，send 失败即无人接收 —— 不影响本轮断言
                Some(value) => {
                    let _ = reply.send(*value);
                }
                None => drop(reply),
            }
        }
        assert_eq!(
            seen,
            ids.iter().cloned().collect(),
            "被问到的键集应等于发起键集"
        );

        let mut got = BTreeMap::new();
        for (id, join) in joins {
            got.insert(id, join.await.expect("审批任务不应 panic"));
        }
        got
    })
}

proptest! {
    /// 性质 1 + 2：决策值只绑定到自己的 `call_id`，未答者恒拒
    #[test]
    fn verdicts_bind_to_call_id_not_to_tool_name(
        answers in prop::collection::btree_map("[a-z]{1,5}", any::<bool>(), 1..8usize),
        silence in prop::collection::btree_set("[0-9]{1,3}", 0..4usize),
    ) {
        let got = run_round(&answers, &silence);
        for (call_id, want) in answers {
            // WHY 位置参数：`prop_assert_*!` 展开宏时 format_args 不能捕获外层变量
            prop_assert_eq!(
                got.get(&call_id),
                Some(&want),
                "call_id {} 的决策被串到了别的条目",
                call_id
            );
        }
        for call_id in silence {
            prop_assert_eq!(
                got.get(&call_id),
                Some(&false),
                "无人应答时 {} 必须按拒收口（fail-closed）",
                call_id
            );
        }
    }
}

/// 定点用例：闸门在问之前就关（UI 从未启动 / 已退出）
#[test]
fn closed_gate_denies_write_but_still_allows_read() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (gate, rx) = mpsc::unbounded_channel::<ApprovalTicket>();
        drop(rx);
        let approval = TuiApproval::new(gate);
        assert!(
            !approval
                .approve(&write_call("closed-1"), SideEffectDecl::Write)
                .await,
            "没有可问人的 UI 时不得放行写调用"
        );
        assert!(
            approval
                .approve(&write_call("closed-2"), SideEffectDecl::ReadOnly)
                .await,
            "拒的理由是「问不到人」，与闸门状态无关的只读调用不应被牵连"
        );
    });
}

/// 定点用例：回执被 drop（弹窗被别的路径摘掉 / app 退出）⇒ 拒
///
/// 这条与 proptest 的 `silence` 集同轴，保留它的理由是**错误信息**：属性测试失败时
/// 报的是随机种子，而本例直接指出"未答即拒"这条具名契约仍然成立。
#[test]
fn dropped_reply_is_denied() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (gate, mut rx) = mpsc::unbounded_channel::<ApprovalTicket>();
        let approval = TuiApproval::new(gate);
        let task = tokio::spawn(async move {
            approval
                .approve(&write_call("drop-a"), SideEffectDecl::Write)
                .await
        });
        let (request, reply) = rx.recv().await.unwrap();
        assert_eq!(request.call_id, "drop-a");
        drop(reply); // UI 侧未作答即放弃
        assert!(!task.await.unwrap(), "未获显式同意的写调用必须按拒收口");
    });
}
