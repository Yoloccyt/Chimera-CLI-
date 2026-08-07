//! Action 编排器 — 消费 TUI 三入口统一派发的 `TuiActionRequested`,路由到 L9 QuestEngine
//!
//! 对应架构层:L10 Interface
//!
//! # 核心职责
//! - 订阅共享 `EventBus`,消费命令面板 / 斜杠 / 面板派发的 `TuiActionRequested`
//! - 按 `action_id` 域前缀路由:编排域(quest.*)驱动 `QuestEngine` 真实执行
//! - 发布 `TuiActionCompleted` / `TuiActionFailed`,经同一 bus 回到 TUI 反馈消费
//!
//! # 设计决策(WHY)
//! - **仅处理编排域(quest.*)**:UI 本地态(view/system/monitor/viz/panel/config/export)
//!   由 chimera-tui 的 `dispatch_action` 本地处理,不绕道 cli(§2.2 依赖铁律,避免
//!   无谓 L1↔cli 往返)。误达的 UI-local action 回 `TuiActionFailed` 明确提示。
//! - **task.* 诚实失败**:核验确认引擎无 per-task 执行模型(仅 Quest 级 pause/resume/
//!   cancel/priority),task.* 真实化需 per-task 调度子系统 → 回 `TuiActionFailed`"尚未实现",
//!   不静默、不伪造(长期主义;见 Phase 3 ADR)。quest.jump 已改由 TUI 本地处理,不经 cli。
//! - **只对 `TuiActionRequested` 反应**:订阅 broadcast 会收到自身回发的
//!   Completed/Failed,`matches!` 守卫仅对 Requested spawn,避免自触发死循环。
//! - **handle/spawn 拆分**:`handle_action_event` 纯 async 可直接投喂事件断言发布序列
//!   (参照 `orchestrator` / `quest-engine::control`)。

use std::sync::Arc;

use crate::overwindow_bridge::OverWindowBridge;
use event_bus::{EventBus, EventBusError, EventMetadata, NexusEvent};
use nexus_core::{MultimodalInput, UserIntent};
use quest_engine::QuestEngine;
use serde_json::Value;
use tokio::task::JoinHandle;

/// 事件来源标识(发布回发事件时写入 `EventMetadata.source`)
const SOURCE: &str = "chimera-cli";
/// 请求者标识(写入 QuestEngine 控制方法的 `requested_by`,用于审计)
const REQUESTED_BY: &str = "tui-action";
/// TUI 会话超窗兜底的有效窗口(token)——HCW 四档(4K/32K/128K/1M)取 128K 档:
/// TUI 会话语料(Chat 历史 + Quest 标题)通常远小于此,仅语料估算超窗才触发
/// 真实兜底检索(语义正确:窗口内零开销返回,不调 provider 不发事件)。
const DEFAULT_EFFECTIVE_WINDOW_TOKENS: u64 = 131_072;

/// 超窗检索句柄 — OverWindowBridge + 会话语料提供者(commands/tui.rs 组装)
///
/// WHY 组合:桥与语料分离——桥可复用(事件发布/块表),语料提供者按会话实时
/// 派生(Chat 消息 + Quest 标题),避免把 pipeline 依赖打进编排器(编排层只依赖
/// bridge 闭包接口,依赖铁律友好;P1,ADR-072)。
#[derive(Clone)]
pub struct OverWindowHandle {
    bridge: Arc<OverWindowBridge>,
    corpus_provider: Arc<dyn Fn() -> String + Send + Sync>,
}

impl OverWindowHandle {
    /// 创建超窗检索句柄
    pub fn new(
        bridge: Arc<OverWindowBridge>,
        corpus_provider: Arc<dyn Fn() -> String + Send + Sync>,
    ) -> Self {
        Self {
            bridge,
            corpus_provider,
        }
    }
}

/// 处理单个事件:仅 `TuiActionRequested` 触发域路由,其余忽略。
///
/// WHY 纯 async(不 spawn):测试可直接 `await` 并断言发布的 Completed/Failed。
/// 路由结果 `Ok(摘要)` → `TuiActionCompleted`;`Err(描述)` → `TuiActionFailed`。
pub async fn handle_action_event(
    bus: &EventBus,
    engine: &QuestEngine,
    overwindow: Option<&OverWindowHandle>,
    event: &NexusEvent,
) {
    let NexusEvent::TuiActionRequested {
        action_id, payload, ..
    } = event
    else {
        return;
    };

    match route_action(engine, overwindow, action_id, payload).await {
        Ok(result) => {
            let _ = bus
                .publish(NexusEvent::TuiActionCompleted {
                    metadata: EventMetadata::new(SOURCE),
                    action_id: action_id.clone(),
                    result,
                })
                .await;
        }
        Err(error) => {
            let _ = bus
                .publish(NexusEvent::TuiActionFailed {
                    metadata: EventMetadata::new(SOURCE),
                    action_id: action_id.clone(),
                    error,
                })
                .await;
        }
    }
}

/// 按 `action_id` 域前缀路由到 `QuestEngine`,返回 `Ok(结果摘要)` 或 `Err(错误描述)`。
///
/// WHY 返回 `Result<String, String>`:处理逻辑与事件发布解耦,`handle_action_event`
/// 统一映射为 Completed/Failed;`String` 错误为面向用户的可读描述(非 thiserror——
/// 编排层聚合 payload 解析 / 引擎错误 / 未实现三类失败源)。
async fn route_action(
    engine: &QuestEngine,
    overwindow: Option<&OverWindowHandle>,
    action_id: &str,
    payload: &str,
) -> Result<String, String> {
    match action_id {
        // agent.chat / quest.start:需 query 构造 UserIntent 交 create_quest 真实分解。
        // 命令面板派发通常无 query(应经 Insert/Chat 输入),此时明确失败而非空跑。
        "agent.chat" | "quest.start" => {
            let query = payload_str(payload, "query")
                .filter(|q| !q.is_empty())
                .ok_or_else(|| format!("{action_id} 需在 Chat 输入内容(i 进入 Insert 模式)"))?;
            let intent = UserIntent {
                intent_id: format!("action-{action_id}"),
                raw_text: query.clone(),
                multimodal_inputs: vec![MultimodalInput::Text(query)],
                risk_level: 0,
            };
            let quest = engine
                .create_quest(intent)
                .await
                .map_err(|e| e.to_string())?;
            Ok(format!(
                "已创建 Quest「{}」({} 个任务)",
                quest.title,
                quest.tasks.len()
            ))
        }
        "quest.pause" => {
            let qid = resolve_quest_id(engine, payload)?;
            engine
                .pause_quest(&qid, REQUESTED_BY)
                .await
                .map_err(|e| e.to_string())?;
            Ok(format!("已暂停 Quest {qid}"))
        }
        "quest.resume" => {
            let qid = resolve_quest_id(engine, payload)?;
            engine
                .resume_quest(&qid, REQUESTED_BY)
                .await
                .map_err(|e| e.to_string())?;
            Ok(format!("已恢复 Quest {qid}"))
        }
        "quest.cancel" => {
            let qid = resolve_quest_id(engine, payload)?;
            engine
                .cancel_quest(&qid, REQUESTED_BY)
                .await
                .map_err(|e| e.to_string())?;
            Ok(format!("已取消 Quest {qid}"))
        }
        // 超窗兜底检索(P1,ADR-072):真实执行 kvbsr→repo-wiki→hcw 两级检索链。
        // 需 query(命令栏 `:overwindow run <词>`);palette 无参派发时明确失败。
        "overwindow.run" => {
            let handle = overwindow.ok_or_else(|| {
                "overwindow.run 未启用(未注入 OverWindowBridge,见 ADR-072)".to_string()
            })?;
            let query = payload_str(payload, "query")
                .filter(|q| !q.is_empty())
                .ok_or_else(|| {
                    format!("{action_id} 需提供 query 参数(如 :overwindow run 检索词)")
                })?;
            let corpus = (handle.corpus_provider)();
            if corpus.trim().is_empty() {
                return Err("会话上下文为空,无可检索语料(先在 Chat 面板输入内容)".to_string());
            }
            // 语料 token 估算与 OverWindowBridge::chunk_corpus 一致(字符数 / 4)
            let corpus_tokens = (corpus.chars().count() / 4) as u64;
            // set_corpus 每次重建块表(O(语料)):会话级语料规模可接受,且复用
            // overwindow_bridge 的锁外构建 + 原子 swap 模式(写锁仅覆盖 Arc 赋值)
            handle.bridge.set_corpus(&corpus);
            let outcome = handle
                .bridge
                .run(&query, corpus_tokens, DEFAULT_EFFECTIVE_WINDOW_TOKENS)
                .await
                .map_err(|e| e.to_string())?;
            if outcome.triggered {
                Ok(format!(
                    "超窗兜底触发:语料 {corpus_tokens} token > 窗口 {DEFAULT_EFFECTIVE_WINDOW_TOKENS} token,候选 {} 条",
                    outcome.candidate_count
                ))
            } else {
                Ok(format!(
                    "语料 {corpus_tokens} token 未超窗(≤ {DEFAULT_EFFECTIVE_WINDOW_TOKENS} token),未触发兜底"
                ))
            }
        }
        // task.*:引擎无 per-task 执行模型(无"正在执行的 task"可暂停/取消/调优先级),
        // 真实化需 per-task 调度子系统(见 Phase 3 ADR),当前诚实未实现(不静默、不伪造)。
        // quest.jump 已改由 TUI 本地处理(切事件流),不再经 cli。
        "task.create" | "task.pause" | "task.resume" | "task.cancel" | "task.set_priority" => Err(
            format!("{action_id} 尚未实现:需 per-task 调度子系统(见 Phase 3 ADR)"),
        ),
        // UI 本地态动作若误达 cli(正常应由 TUI 本地 dispatch_action 处理)
        _ => Err(format!("{action_id} 应由 TUI 本地处理,不应派发至编排层")),
    }
}

/// 解析目标 quest_id:优先 `payload.quest_id`;缺失时回退唯一活跃 Quest。
///
/// WHY 不猜测多 Quest:存在多个 Quest 且未指定时返回 Err 明确提示,避免误操作
/// 到非预期 Quest(破坏性动作如 cancel 尤其需要精确目标)。
fn resolve_quest_id(engine: &QuestEngine, payload: &str) -> Result<String, String> {
    if let Some(qid) = payload_str(payload, "quest_id") {
        if !qid.is_empty() {
            return Ok(qid);
        }
    }
    let quests = engine.list_quests();
    match quests.len() {
        0 => Err("当前无活跃 Quest 可操作".to_string()),
        1 => Ok(quests[0].quest_id.clone()),
        _ => Err("存在多个 Quest,请在 payload.quest_id 指定目标".to_string()),
    }
}

/// 从 JSON `payload` 提取字符串字段(解析失败 / 字段缺失 / 非字符串 → None)。
fn payload_str(payload: &str, key: &str) -> Option<String> {
    serde_json::from_str::<Value>(payload)
        .ok()?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

/// 启动后台 Action 编排器,返回可 abort 的任务句柄。
///
/// # 生命周期(§4.4 反模式 #3 / #7)
/// - **subscribe-before-spawn**:`bus.subscribe()` 在 `tokio::spawn()` 前同步调用。
/// - 每个 `TuiActionRequested` spawn 独立任务(共享 `Arc<QuestEngine>`),recv 循环
///   立即继续,避免长处理阻塞 recv。
/// - recv 错误:`SlowConsumerDropped`(Lagged)记录后继续;`ChannelClosed` 等退出。
/// - 调用方负责在退出时 `abort()` 句柄,避免 orphan task。
pub fn spawn_action_orchestrator(
    bus: EventBus,
    engine: Arc<QuestEngine>,
    overwindow: Option<OverWindowHandle>,
) -> JoinHandle<()> {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    // 仅为动作请求建任务;忽略自身回发的 Completed/Failed 及其他事件。
                    if matches!(event, NexusEvent::TuiActionRequested { .. }) {
                        let bus = bus.clone();
                        let engine = Arc::clone(&engine);
                        let overwindow = overwindow.clone();
                        tokio::spawn(async move {
                            handle_action_event(&bus, &engine, overwindow.as_ref(), &event).await;
                        });
                    }
                }
                Err(EventBusError::SlowConsumerDropped { lag, .. }) => {
                    tracing::warn!(lag, "Action 编排器接收滞后,丢弃部分事件后继续");
                }
                Err(e) => {
                    tracing::info!(error = %e, "Action 编排器订阅结束,退出");
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::ActionSource;

    /// 构造 TuiActionRequested 事件(来源 Palette)
    fn action(action_id: &str, payload: &str) -> NexusEvent {
        NexusEvent::TuiActionRequested {
            metadata: EventMetadata::new("chimera-tui"),
            action_id: action_id.into(),
            payload: payload.into(),
            source: ActionSource::Palette,
        }
    }

    /// 构造含单个 Quest 的引擎(返回引擎与 quest_id)
    async fn engine_with_one_quest(bus: &EventBus) -> (QuestEngine, String) {
        let engine = QuestEngine::new(bus.clone());
        let intent = UserIntent {
            intent_id: "i-test".into(),
            raw_text: "分析需求。设计方案。".into(),
            multimodal_inputs: vec![MultimodalInput::Text("x".into())],
            risk_level: 0,
        };
        let quest = engine.create_quest(intent).await.unwrap();
        (engine, quest.quest_id)
    }

    /// 从订阅缓冲中查找是否发布了指定判定的事件(有界 drain,容忍穿插事件)
    fn drain_find(rx: &mut event_bus::EventReceiver, pred: impl Fn(&NexusEvent) -> bool) -> bool {
        for _ in 0..32 {
            match rx.try_recv() {
                Ok(Some(ev)) => {
                    if pred(&ev) {
                        return true;
                    }
                }
                Ok(None) => return false,
                Err(_) => return false,
            }
        }
        false
    }

    #[tokio::test]
    async fn quest_pause_drives_engine_and_completes() {
        let bus = EventBus::new();
        let (engine, qid) = engine_with_one_quest(&bus).await;
        let mut rx = bus.subscribe();
        // 单一 Quest → 无需 payload.quest_id,回退解析命中
        handle_action_event(&bus, &engine, None, &action("quest.pause", "{}")).await;
        assert!(
            engine.is_paused(&qid),
            "quest.pause 应驱动 QuestEngine::pause_quest"
        );
        assert!(
            drain_find(&mut rx, |ev| matches!(
                ev,
                NexusEvent::TuiActionCompleted { action_id, .. } if action_id == "quest.pause"
            )),
            "quest.pause 成功应发布 TuiActionCompleted"
        );
    }

    #[tokio::test]
    async fn task_action_fails_gracefully_not_silent() {
        let bus = EventBus::new();
        let engine = QuestEngine::new(bus.clone());
        let mut rx = bus.subscribe();
        handle_action_event(&bus, &engine, None, &action("task.pause", "{}")).await;
        assert!(
            drain_find(&mut rx, |ev| matches!(
                ev,
                NexusEvent::TuiActionFailed { action_id, .. } if action_id == "task.pause"
            )),
            "task.* 未实现应发 TuiActionFailed 而非静默丢失"
        );
    }

    #[tokio::test]
    async fn ui_local_action_rejected_at_cli() {
        let bus = EventBus::new();
        let engine = QuestEngine::new(bus.clone());
        let mut rx = bus.subscribe();
        handle_action_event(&bus, &engine, None, &action("view.switch_layout", "{}")).await;
        assert!(
            drain_find(&mut rx, |ev| matches!(
                ev,
                NexusEvent::TuiActionFailed { .. }
            )),
            "UI 本地态动作误达 cli 应被拒绝(TuiActionFailed)"
        );
    }

    #[tokio::test]
    async fn quest_pause_no_active_quest_fails() {
        let bus = EventBus::new();
        let engine = QuestEngine::new(bus.clone()); // 无 Quest
        let mut rx = bus.subscribe();
        handle_action_event(&bus, &engine, None, &action("quest.pause", "{}")).await;
        assert!(
            drain_find(&mut rx, |ev| matches!(
                ev,
                NexusEvent::TuiActionFailed { .. }
            )),
            "无活跃 Quest 时 quest.pause 应失败(不猜测目标)"
        );
    }

    #[tokio::test]
    async fn ignores_non_action_event() {
        let bus = EventBus::new();
        let engine = QuestEngine::new(bus.clone());
        let mut rx = bus.subscribe();
        let unrelated = NexusEvent::CacheHit {
            metadata: EventMetadata::new("test"),
            cache_key: "k".into(),
        };
        handle_action_event(&bus, &engine, None, &unrelated).await;
        assert!(
            matches!(rx.try_recv(), Ok(None)),
            "非 TuiActionRequested 事件不应触发任何发布"
        );
    }

    #[tokio::test]
    async fn spawn_orchestrator_consumes_requested() {
        let bus = EventBus::new();
        let (engine, _qid) = engine_with_one_quest(&bus).await;
        let mut rx = bus.subscribe();
        let handle = spawn_action_orchestrator(bus.clone(), Arc::new(engine), None);
        bus.publish(action("quest.cancel", "{}")).await.unwrap();

        let mut saw_completed = false;
        for _ in 0..64 {
            match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await {
                Ok(Ok(NexusEvent::TuiActionCompleted { action_id, .. }))
                    if action_id == "quest.cancel" =>
                {
                    saw_completed = true;
                    break;
                }
                Ok(Ok(_)) => {}
                _ => break,
            }
        }
        handle.abort();
        assert!(
            saw_completed,
            "编排器应消费 quest.cancel 并回发 TuiActionCompleted"
        );
    }

    /// 构造超窗检索句柄:语料提供者返回超窗语料(70K×8 字符 ≈ 560K 字符 ≈ 140K token > 128K 窗口)
    fn big_corpus_handle(bus: &EventBus) -> OverWindowHandle {
        OverWindowHandle::new(
            Arc::new(crate::overwindow_bridge::OverWindowBridge::new(bus.clone()).unwrap()),
            Arc::new(|| "语义检索测试语料".repeat(70_000)),
        )
    }

    #[tokio::test]
    async fn overwindow_run_triggers_and_completes() {
        let bus = EventBus::new();
        let engine = QuestEngine::new(bus.clone());
        let mut rx = bus.subscribe();
        let handle = big_corpus_handle(&bus);
        handle_action_event(
            &bus,
            &engine,
            Some(&handle),
            &action("overwindow.run", r#"{"query":"语义检索"}"#),
        )
        .await;
        // 单趟收集两个断言:bridge.run 先发布 Triggered、编排器后发布 Completed,
        // 若用两次 drain_find,第一次查找会把另一事件消费掉(破坏性 drain)。
        let mut saw_completed = false;
        let mut saw_triggered = false;
        for _ in 0..64 {
            match rx.try_recv() {
                Ok(Some(ev)) => {
                    if matches!(
                        &ev,
                        NexusEvent::TuiActionCompleted { action_id, .. }
                            if action_id == "overwindow.run"
                    ) {
                        saw_completed = true;
                    }
                    if matches!(&ev, NexusEvent::OverWindowFallbackTriggered { .. }) {
                        saw_triggered = true;
                    }
                    if saw_completed && saw_triggered {
                        break;
                    }
                }
                Ok(None) | Err(_) => break,
            }
        }
        assert!(
            saw_completed,
            "overwindow.run 超窗应发布 TuiActionCompleted"
        );
        assert!(
            saw_triggered,
            "超窗触发应发布 OverWindowFallbackTriggered(TUI 面板数据源)"
        );
    }

    #[tokio::test]
    async fn overwindow_run_within_window_completes_without_trigger() {
        let bus = EventBus::new();
        let engine = QuestEngine::new(bus.clone());
        let mut rx = bus.subscribe();
        let handle = OverWindowHandle::new(
            Arc::new(crate::overwindow_bridge::OverWindowBridge::new(bus.clone()).unwrap()),
            Arc::new(|| "短语料".to_string()), // ≈1 token ≤ 窗口 → 不触发
        );
        handle_action_event(
            &bus,
            &engine,
            Some(&handle),
            &action("overwindow.run", r#"{"query":"x"}"#),
        )
        .await;
        let mut saw_completed = false;
        let mut saw_triggered = false;
        for _ in 0..64 {
            match rx.try_recv() {
                Ok(Some(ev)) => {
                    if matches!(&ev, NexusEvent::TuiActionCompleted { .. }) {
                        saw_completed = true;
                    }
                    if matches!(&ev, NexusEvent::OverWindowFallbackTriggered { .. }) {
                        saw_triggered = true;
                    }
                }
                Ok(None) | Err(_) => break,
            }
        }
        assert!(saw_completed, "窗口内应发布 TuiActionCompleted(未触发说明)");
        assert!(!saw_triggered, "窗口内不得发布触发事件(零开销语义)");
    }

    #[tokio::test]
    async fn overwindow_run_missing_query_fails() {
        let bus = EventBus::new();
        let engine = QuestEngine::new(bus.clone());
        let mut rx = bus.subscribe();
        let handle = big_corpus_handle(&bus);
        handle_action_event(
            &bus,
            &engine,
            Some(&handle),
            &action("overwindow.run", "{}"),
        )
        .await;
        assert!(
            drain_find(&mut rx, |ev| matches!(
                ev,
                NexusEvent::TuiActionFailed { action_id, .. } if action_id == "overwindow.run"
            )),
            "缺 query 应发布 TuiActionFailed(不空跑)"
        );
    }

    #[tokio::test]
    async fn overwindow_run_without_handle_fails() {
        let bus = EventBus::new();
        let engine = QuestEngine::new(bus.clone());
        let mut rx = bus.subscribe();
        handle_action_event(
            &bus,
            &engine,
            None,
            &action("overwindow.run", r#"{"query":"x"}"#),
        )
        .await;
        assert!(
            drain_find(&mut rx, |ev| matches!(
                ev,
                NexusEvent::TuiActionFailed { .. }
            )),
            "未注入桥时应发布 TuiActionFailed"
        );
    }
}
