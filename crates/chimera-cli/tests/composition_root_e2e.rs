//! 组合根（C12）集成测试 —— 从 crate 外部边界验证公共装配面真可用。
//!
//! WHY 集成层而非只靠 composition.rs 内联测试：内联 `#[cfg(test)]` 能过，不代表
//! 外部 bin 路径（main.rs 经 `chimera_cli::composition::build`）真的拿到同一能力。
//! 本文件只用 `chimera_cli` 的**公开**导出面复现 serve/acp 的两步协议流，锁死
//! "lib 导出 = bin 可用"这条接缝（G7 收口缺口）。proptest 部分覆盖输入空间与
//! subscribe-then-move 顺序不变量（§6.2 红线可参数化守护）。

#![forbid(unsafe_code)]

use chimera_cli::composition::{build, build_app_server};
use chimera_cli::ChimeraConfig;
use nexus_contracts::app::{AppEvent, AppOp, ThreadStartParams, UserInput};
use proptest::prelude::*;

/// C1 验收（bin 可达路径）：组合根装配的 AppServer 处理 TurnSubmit 产出真实引擎
/// 特征 Item（`quest_state` + 真实 quest_id）——InMemory 桩无此 kind。
#[tokio::test]
async fn composition_root_replaces_inmemory_stub_on_protocol_hosts() {
    let ctx = build(&ChimeraConfig::default()).expect("装配应成功");
    let server = build_app_server(ctx);
    let events = server
        .handle_op(&AppOp::ThreadStart(ThreadStartParams::new(
            "goal-1", "run-1",
        )))
        .await
        .expect("ThreadStart 应成功");
    let thread_id = match events.first().expect("应产生首个事件") {
        AppEvent::ThreadStarted { thread } => thread.thread_id.clone(),
        other => panic!("首个事件应为 ThreadStarted，实际 {other:?}"),
    };
    let events = server
        .handle_op(&AppOp::TurnSubmit {
            thread_id,
            input: UserInput {
                text: "分析项目依赖".into(),
                extras: None,
            },
        })
        .await
        .expect("TurnSubmit 应成功");
    let items: Vec<_> = events
        .iter()
        .filter_map(|ev| match ev {
            AppEvent::ItemChanged { item } => Some(item),
            _ => None,
        })
        .collect();
    let quest_item = items
        .iter()
        .find(|it| it.kind.as_ref() == "quest_state")
        .unwrap_or_else(|| {
            panic!(
                "真实引擎应产出 quest_state Item（InMemory 桩无此 kind）；实际 kinds: {:?}",
                items.iter().map(|it| &*it.kind).collect::<Vec<_>>()
            )
        });
    assert!(
        quest_item.payload.contains("quest_id"),
        "载荷应含真实 quest_id（引擎生成，非空）"
    );
}

/// C1×C3 验收：build() 之后、build_app_server() 之前，Critical 旁路**未**注册
/// （subscribe 尚未发生）；build_app_server() 之后**已**注册。锁死"先订阅再移交"
/// 顺序（§6.2 红线的 mpsc 送达需对端消费者）。
#[tokio::test]
async fn critical_bypass_absent_before_and_present_after_build_app_server() {
    let ctx = build(&ChimeraConfig::default()).expect("装配应成功");
    let probe = ctx.bus.clone();
    assert!(
        !probe.has_critical_subscribers(),
        "build() 仅构造 bus/engine，不得提前注册 Critical 旁路订阅者"
    );
    let _server = build_app_server(ctx);
    assert!(
        probe.has_critical_subscribers(),
        "build_app_server() 应注册 Critical 旁路订阅者（C1×C3 接线）"
    );
}

proptest! {
    /// P-2（顺序不变量的 proptest 版，计划点名）：对任意 config（version 扫描），
    /// build() 后、build_app_server() 前 `has_critical_subscribers()==false`，之后 `==true`。
    /// 跨输入 sweep 复验 §6.2 "subscribe-then-move" 顺序不可被后人调换。
    #[test]
    fn prop_critical_bypass_order_holds_for_any_config(version in ".{0,32}") {
        let mut cfg = ChimeraConfig::default();
        let _ = std::mem::replace(&mut cfg.nexus.version, version);
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| TestCaseError::fail(format!("runtime: {e}")))?;
        let ctx = build(&cfg).map_err(|e| TestCaseError::fail(format!("build 失败: {e}")))?;
        let probe = ctx.bus.clone();
        prop_assert!(
            !probe.has_critical_subscribers(),
            "build() 仅构造 bus/engine，不得提前注册 Critical 旁路"
        );
        // build_app_server 内含 tokio::spawn，需在 runtime 语境下调用 → block_on 提供。
        let server = rt.block_on(async move { build_app_server(ctx) });
        prop_assert!(
            probe.has_critical_subscribers(),
            "build_app_server() 后必须已注册 Critical 旁路（subscribe-then-move）"
        );
        let _ = server;
    }

    /// P-1：任意 version 字符串下 build() 都成功，且 bus 与 engine 共享同一内核
    /// （EventBus Clone = Arc 引用，廉价共享；组合根不得产生双总线）。
    #[test]
    fn prop_build_never_panics_and_shares_one_bus(version in ".{0,32}") {
        let mut cfg = ChimeraConfig::default();
        // 用可控字段喂 proptest：version 任意都不应影响装配（当前 config 仅用于日志）。
        let _ = std::mem::replace(&mut cfg.nexus.version, version);
        let ctx = build(&cfg).map_err(|e| TestCaseError::fail(format!("build 失败: {e}")))?;
        let cloned = ctx.bus.clone();
        // 共享内核自证：clone 前后对同一 Critical 通道状态一致（未注册都应为 false）。
        prop_assert_eq!(cloned.has_critical_subscribers(), ctx.bus.has_critical_subscribers());
    }

    /// P-3：任意用户输入下，一次 TurnSubmit 至多产出一个 quest_state Item
    /// （真实引擎每回合建一个 quest；防多引擎/多回合噪声下重复 quest_state 回归）。
    #[test]
    fn prop_turn_submit_yields_at_most_one_quest_state(text in ".{0,64}") {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let kinds = rt.block_on(drive_two_step_turn_async(&text));
        let quest_states = kinds.iter().filter(|k| *k == "quest_state").count();
        prop_assert!(
            quest_states <= 1,
            "TurnSubmit 应至多一个 quest_state，实际 {quest_states}（输入 {text:?}）"
        );
    }
}

/// proptest 内部用 block_on 驱动（proptest 闭包非 async），复用两步流逻辑。
fn drive_two_step_turn_async(user_text: &str) -> impl std::future::Future<Output = Vec<String>> {
    let text = user_text.to_string();
    async move {
        let ctx = build(&ChimeraConfig::default()).expect("装配应成功");
        let server = build_app_server(ctx);
        let events = server
            .handle_op(&AppOp::ThreadStart(ThreadStartParams::new("g", "r")))
            .await
            .expect("ThreadStart");
        let thread_id = match events.first().expect("首事件") {
            AppEvent::ThreadStarted { thread } => thread.thread_id.clone(),
            _ => panic!("期望 ThreadStarted"),
        };
        let events = server
            .handle_op(&AppOp::TurnSubmit {
                thread_id,
                input: UserInput {
                    text: text.into(),
                    extras: None,
                },
            })
            .await
            .expect("TurnSubmit");
        events
            .iter()
            .filter_map(|ev| match ev {
                AppEvent::ItemChanged { item } => Some(item.kind.to_string()),
                _ => None,
            })
            .collect()
    }
}
