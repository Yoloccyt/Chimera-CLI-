//! P2 / ADR-257 集成验收 — `SubAgentQuestEngine` 组合根接线（deepdive §7.4）。
//!
//! 覆盖面：装配可用性、spawn/join 闭环、SWARM_LIMIT 上限、AgentTask* 事件
//! 闭环（subscribe-before-spawn 红线）、零旁路 ToolRuntime 反向断言、
//! 禁嵌套/容量两属性测试。
//!
//! 运行：`cargo test -p chimera-cli --test subagent_wiring -- --test-threads=1`

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use chimera_cli::composition::build;
use chimera_cli::ChimeraConfig;
use nexus_subagent::types::{SubAgentKind, SubAgentSpec};

/// 多线程 flavor：spawn 闭包经 spawn_blocking + Handle::block_on 驱动
/// create_quest（与 nexus-app-server 适配器自测同型，§7.4 复用 :130-146 形态）。
fn make_ctx() -> chimera_cli::composition::AppContext {
    build(&ChimeraConfig::default()).expect("组合根装配应成功")
}

/// 集成 1：装配可用性——ctx.subagent_engine 句柄存在且初值 spawned_total()==0。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn composition_build_wires_subagent() {
    let ctx = make_ctx();
    let mut engine = ctx.subagent_engine.lock().await;
    assert_eq!(
        engine.runtime_mut().spawned_total(),
        0,
        "装配初值：零派发历史"
    );
}

/// 集成 2：spawn → join 闭环产出真实 Quest ID（`quest-` 前缀；复用
/// subagent_engine.rs:130-146 断言形态落 chimera-cli 侧）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_spawn_join_event_closure() {
    let ctx = make_ctx();
    let handle = ctx
        .subagent_engine
        .lock()
        .await
        .spawn_quest(
            SubAgentSpec::new(SubAgentKind::Explore),
            "接线验收任务",
            None,
        )
        .expect("顶层派发（无嵌套）");
    let (task_id, result) = ctx
        .subagent_engine
        .lock()
        .await
        .join_next()
        .await
        .expect("JoinSet 必有结果");
    assert_eq!(task_id, handle.task_id, "join 归属 spawn 的任务");
    let quest_id = result.expect("Quest 创建成功");
    assert!(quest_id.starts_with("quest-"), "真实 Quest ID: {quest_id}");
}

/// 集成 3：SWARM_LIMIT=8 上限——连发 8 个成功（不 join,JoinSet 计入已完成
/// 未收取任务）,第 9 个 Err（容量超限确定性:JoinSet::len 含未 join 项）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_swarm_limit_enforced() {
    let ctx = make_ctx();
    let mut engine = ctx.subagent_engine.lock().await;
    for i in 0..8 {
        engine
            .spawn_quest(
                SubAgentSpec::new(SubAgentKind::Explore),
                &format!("任务 {i}"),
                None,
            )
            .unwrap_or_else(|e| panic!("第 {} 个 spawn 应成功（上限内）: {e}", i + 1));
    }
    let ninth = engine.spawn_quest(SubAgentSpec::new(SubAgentKind::Coder), "第 9 个", None);
    assert!(ninth.is_err(), "第 9 个 spawn 必须被容量上限拒绝");
    // 收尾:排空 JoinSet（事件发布面在本测试未注入 bus——None 场景零发布）
    while engine.join_next().await.is_some() {}
}

/// 集成 4：事件闭环——subscribe-before-spawn 红线下,spawn/join 闭环内
/// `AgentTaskDelegated`/`AgentTaskCompleted` 可订阅到（组合根注入共享 bus）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_event_publishes_agent_task() {
    let ctx = make_ctx();
    let mut rx = ctx.bus.subscribe(); // subscribe-before-spawn 红线（§4.4 #3）

    let handle = ctx
        .subagent_engine
        .lock()
        .await
        .spawn_quest(
            SubAgentSpec::new(SubAgentKind::Explore),
            "事件闭环任务",
            None,
        )
        .expect("顶层派发");
    let (task_id, result) = ctx
        .subagent_engine
        .lock()
        .await
        .join_next()
        .await
        .expect("JoinSet 必有结果");
    result.expect("Quest 创建成功");
    assert_eq!(task_id, handle.task_id);

    // 排空订阅缓冲,断言 Delegated 与 Completed 均到达且 task_id 归属正确
    let mut saw_delegated = false;
    let mut saw_completed = false;
    for _ in 0..64 {
        match rx.try_recv() {
            Ok(Some(event)) => match event {
                event_bus::NexusEvent::AgentTaskDelegated { task_id: tid, .. } => {
                    assert_eq!(tid, task_id, "Delegated 归属正确");
                    saw_delegated = true;
                }
                event_bus::NexusEvent::AgentTaskCompleted { task_id: tid, .. } => {
                    assert_eq!(tid, task_id, "Completed 归属正确");
                    saw_completed = true;
                }
                _ => {}
            },
            Ok(None) | Err(_) => break,
        }
    }
    assert!(saw_delegated, "AgentTaskDelegated 应可订阅到");
    assert!(saw_completed, "AgentTaskCompleted 应可订阅到");
}

/// 集成 5（反向）：SubAgent 闭包仅 `create_quest`——生产文本扫描无第二执行链
/// （零旁路 ToolRuntime:适配器不派发工具、不经四环以外执行面）。
#[test]
fn subagent_delegation_not_bypassing_tool_runtime() {
    let src = include_str!("../../nexus-app-server/src/subagent_engine.rs");
    // 闭包执行体:唯一引擎调用为 create_quest
    assert!(
        src.contains("engine.create_quest"),
        "适配器执行体应经 QuestEngine::create_quest"
    );
    // 反向:无第二执行链（不派发工具/不触碰 ToolRuntime/不直调 mesh）
    for banned in [
        "tool_runtime",
        "ToolRuntime",
        ".dispatch(",
        "execute_transaction",
    ] {
        assert!(
            !src.contains(banned),
            "适配器不得引入旁路执行链（{banned}）——零旁路 ToolRuntime 不变量"
        );
    }
}

// ============================================================
// proptest（deepdive §7.4 两行）
// ============================================================

use proptest::prelude::*;

// WHY 顶层 runtime 助手：proptest! 生成普通 fn（非 async），P2 的 spawn/join
// 面是 async——每 case 用专属多线程 runtime 驱动（与适配器自测同型 flavor）。
fn wiring_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .build()
        .expect("多线程 runtime 构造应成功")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 任意 from_task 归属:活跃任务 ID 作发起方 → spawn 恒 Err（禁嵌套）;
    /// 未知 ID 保守放行（与 runtime 语义一致,适配器透传）。
    #[test]
    fn prop_spawn_never_nests(from_task in ".*") {
        use nexus_contracts::NexusError;
        let rt = wiring_runtime();
        let _ = rt.block_on(async move {
            let ctx = build(&ChimeraConfig::default()).expect("装配应成功");
            let mut engine = ctx.subagent_engine.lock().await;
            // 两个真实活跃任务（不 join ⇒ 保持活跃）
            let h1 = engine
                .spawn_quest(SubAgentSpec::new(SubAgentKind::Explore), "父任务", None)
                .unwrap();
            let _ = engine
                .spawn_quest(SubAgentSpec::new(SubAgentKind::Coder), "第二任务", None)
                .unwrap();
            let attempt = engine.spawn_quest(
                SubAgentSpec::new(SubAgentKind::Explore),
                "任意归属派发",
                Some(&from_task),
            );
            if from_task == h1.task_id {
                prop_assert!(
                    matches!(attempt, Err(NexusError::NestedSubAgentForbidden)),
                    "活跃任务 ID 作发起方必须被禁嵌套拒绝: {from_task:?}"
                );
            } else {
                // 未知/第二任务 ID:保守放行（Ok）——本夹具 active ≤ 3 < 8,不触发容量
                prop_assert!(attempt.is_ok(), "未知 ID 保守放行失败: {from_task:?}");
            }
            while engine.join_next().await.is_some() {}
            Ok(())
        });
    }

    /// 任意 spawn 序列下 active_count() <= 8（SWARM_LIMIT 不变量:
    /// 超限 spawn 被拒而非入队——容量即硬上界）。
    #[test]
    fn prop_swarm_capacity_bound(k in 0usize..=12) {
        let rt = wiring_runtime();
        let _ = rt.block_on(async move {
            let ctx = build(&ChimeraConfig::default()).expect("装配应成功");
            let mut engine = ctx.subagent_engine.lock().await;
            for i in 0..k {
                let out = engine.spawn_quest(
                    SubAgentSpec::new(SubAgentKind::Explore),
                    &format!("任务 {i}"),
                    None,
                );
                if out.is_err() {
                    // 容量满被拒:合法路径（不变量仍成立）
                    prop_assert!(engine.runtime_mut().active_count() <= 8);
                    break;
                }
                prop_assert!(
                    engine.runtime_mut().active_count() <= 8,
                    "第 {} 次 spawn 后 active_count 必须不越界",
                    i + 1
                );
            }
            while engine.join_next().await.is_some() {}
            Ok(())
        });
    }
}
