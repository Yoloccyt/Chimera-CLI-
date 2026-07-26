//! Quest 分解编排器 — 消费 TUI 对话提交,经真实 L9 Quest 引擎分解并流式回发
//!
//! 对应架构层:L10 Interface
//!
//! # 核心职责
//! - 订阅共享 `EventBus`,消费 TUI 发布的 `TuiChatSubmitted`
//! - 将 query 构造为 `UserIntent`,交由 `QuestEngine::create_quest` **真实分解**为任务 DAG
//! - 逐字符流式回发分解结果:`TuiChatStatusChanged(Thinking)` → `TuiChatResponseChunk`×N
//!   → `TuiChatCompleted` → `TuiChatStatusChanged(Idle)`
//! - 回发事件经同一 `EventBus` 回到 TUI 的订阅者 → `DataPipeline` → `ChatSync`,
//!   点亮 Chat 面板的端到端流式对话;`create_quest` 内部广播的 `QuestCreated`
//!   同步点亮 Quest 面板(一次提交同时驱动两个面板)
//!
//! # 设计决策(WHY)
//! - **真实 L9 管线(替换 M3c 回声)**:query → `UserIntent` → `QuestEngine` 分解,
//!   回复内容为真实任务 DAG 摘要;系统无 LLM,Quest 分解是当前可达的最真实处理。
//! - **复用 tui.rs 的 `Arc<QuestEngine>`**:与控制订阅者共享同一引擎实例,避免状态分裂。
//! - **只对 `TuiChatSubmitted` 反应**:编排器订阅 broadcast,会收到自身/引擎发布的
//!   chunk/status/QuestCreated 等事件;处理器仅对 Submit 反应,避免自触发死循环。
//! - **每个 Submit spawn 独立任务**:recv 循环收到 Submit 后立即 spawn 分解+流式任务
//!   并继续 recv,避免长流式期间阻塞 recv 导致自身高频 chunk 挤爆广播缓冲(Lagged)。
//! - **handle/spawn 拆分**:`handle_chat_event` 为纯 async 处理单事件,测试可直接投喂
//!   事件断言发布序列(参照 `quest-engine::control`)。

use std::sync::Arc;
use std::time::Duration;

use event_bus::{ChatStatus, EventBus, EventBusError, EventMetadata, NexusEvent};
use nexus_core::{MultimodalInput, Quest, UserIntent};
use quest_engine::{QuestEngine, QuestError};
use tokio::task::JoinHandle;

/// 事件来源标识(发布回发事件时写入 `EventMetadata.source`)
const SOURCE: &str = "chimera-cli";

/// Quest 编排器配置
pub struct OrchestratorConfig {
    /// 每个 chunk 之间的流式延迟。
    ///
    /// WHY 可配置:默认 20ms 制造逐字符浮现的流式观感(配合 TUI 250ms tick,
    /// 每 tick 约增长 12 字符);测试与 bench 设 `Duration::ZERO` 以最快吞吐运行。
    pub chunk_delay: Duration,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            chunk_delay: Duration::from_millis(20),
        }
    }
}

/// 将 Quest 分解结果格式化为回复文本(纯函数)。
///
/// WHY 纯函数:回复文本生成与事件发布解耦,便于单元测试与 bench 直接验证。
/// 回复是 Agent 输出内容(非 UI chrome),故不经 i18n,保持简单字符串。
pub fn build_quest_reply(quest: &Quest) -> String {
    let mut reply = format!(
        "已理解需求「{}」,分解为 {} 个任务(思考模式:{:?}):",
        quest.title,
        quest.tasks.len(),
        quest.thinking_mode
    );
    for (i, task) in quest.tasks.iter().enumerate() {
        reply.push_str(&format!("\n{}. {}", i + 1, task.description));
    }
    reply
}

/// 将分解失败格式化为回复文本(纯函数)。
///
/// WHY 显式错误回复:query 异常(如空文本、环依赖)导致 `create_quest` 失败时,
/// 仍向 Chat 面板回发可读错误并正常收尾(状态回 Idle),不静默吞掉。
pub fn build_error_reply(err: &QuestError) -> String {
    format!("需求分解失败:{err}")
}

/// 逐字符将回复规划为 chunk 序列(纯函数)。
///
/// WHY 独立 pub 函数:既是流式分块的单一事实源,也作为 bench 度量
/// "高频 chunk 生产"的被测目标(每字符一次 `String` 分配的代价基线)。
pub fn plan_chunks(reply: &str) -> Vec<String> {
    reply.chars().map(|c| c.to_string()).collect()
}

/// 执行一轮 Quest 分解 + 流式发布:Thinking → 真实分解 → 逐字符 chunk → Completed → Idle。
///
/// `chunk_delay` 为零时不 sleep(测试/bench 快速路径)。session_id 全程透传,
/// 供 TUI 侧多会话关联。publish 失败(如暂无订阅者)以 `let _ =` 容忍——回发为
/// 非关键事件,丢失不影响系统一致性,且高频 chunk 不宜 spam 日志。
async fn stream_quest(
    bus: &EventBus,
    engine: &QuestEngine,
    cfg: &OrchestratorConfig,
    session_id: &str,
    query: &str,
) {
    let sid = session_id.to_string();

    // 1. 进入思考态(分解期间面板显示 Thinking 指示器)
    let _ = bus
        .publish(NexusEvent::TuiChatStatusChanged {
            metadata: EventMetadata::new(SOURCE),
            session_id: sid.clone(),
            status: ChatStatus::Thinking,
        })
        .await;

    // 2. 真实 L9 分解:query → UserIntent → Quest(create_quest 内部广播 QuestCreated,
    //    经同一 bus 点亮 Quest 面板)。失败则回发可读错误文本。
    let intent = UserIntent {
        intent_id: format!("intent-{sid}"),
        raw_text: query.to_string(),
        multimodal_inputs: vec![MultimodalInput::Text(query.to_string())],
        risk_level: 0,
    };
    let reply = match engine.create_quest(intent).await {
        Ok(quest) => build_quest_reply(&quest),
        Err(e) => build_error_reply(&e),
    };

    // 3. 逐字符流式回发分解结果
    for (i, delta) in plan_chunks(&reply).into_iter().enumerate() {
        let _ = bus
            .publish(NexusEvent::TuiChatResponseChunk {
                metadata: EventMetadata::new(SOURCE),
                session_id: sid.clone(),
                delta,
                cursor_hint: i as u32,
            })
            .await;
        if !cfg.chunk_delay.is_zero() {
            tokio::time::sleep(cfg.chunk_delay).await;
        }
    }

    // 4. 完成 + 回到 Idle
    let _ = bus
        .publish(NexusEvent::TuiChatCompleted {
            metadata: EventMetadata::new(SOURCE),
            session_id: sid.clone(),
            tool_use: None,
        })
        .await;
    let _ = bus
        .publish(NexusEvent::TuiChatStatusChanged {
            metadata: EventMetadata::new(SOURCE),
            session_id: sid,
            status: ChatStatus::Idle,
        })
        .await;
}

/// 处理单个事件:仅 `TuiChatSubmitted` 触发 Quest 分解流式,其余事件忽略。
///
/// WHY 纯 async(不 spawn):测试可直接 `await` 此函数并断言发布序列,
/// 无需启动/中止后台任务。后台循环则对每个 Submit spawn 本函数(见
/// [`spawn_quest_orchestrator`]),保持 recv 循环可响应。
pub async fn handle_chat_event(
    bus: &EventBus,
    engine: &QuestEngine,
    cfg: &OrchestratorConfig,
    event: &NexusEvent,
) {
    if let NexusEvent::TuiChatSubmitted {
        session_id, query, ..
    } = event
    {
        stream_quest(bus, engine, cfg, session_id, query).await;
    }
}

/// 启动后台 Quest 编排器,返回可 abort 的任务句柄。
///
/// # 生命周期(§4.4 反模式 #3 / #7)
/// - **subscribe-before-spawn**:`bus.subscribe()` 在 `tokio::spawn()` 前同步调用,
///   避免启动瞬间错过事件。
/// - 每个 `TuiChatSubmitted` spawn 独立任务(共享 `Arc<QuestEngine>`),recv 循环立即
///   继续,避免长流式阻塞 recv 导致自身高频 chunk 把广播缓冲挤爆。
/// - recv 错误:`SlowConsumerDropped`(Lagged)记录后继续(丢失的多为自身 chunk /
///   引擎事件,编排器本就忽略,无害);`ChannelClosed` 等致命错误退出循环。
/// - 调用方负责在退出时 `abort()` 句柄,避免 orphan task。
pub fn spawn_quest_orchestrator(
    bus: EventBus,
    engine: Arc<QuestEngine>,
    cfg: OrchestratorConfig,
) -> JoinHandle<()> {
    let mut rx = bus.subscribe();
    let cfg = Arc::new(cfg);
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    // 仅为对话提交建任务;忽略编排器自身回发的 chunk/status/completed
                    // 及引擎广播的 QuestCreated 等,避免为高频事件无谓 spawn。
                    if matches!(event, NexusEvent::TuiChatSubmitted { .. }) {
                        let bus = bus.clone();
                        let engine = Arc::clone(&engine);
                        let cfg = Arc::clone(&cfg);
                        tokio::spawn(async move {
                            handle_chat_event(&bus, &engine, &cfg, &event).await;
                        });
                    }
                }
                // Lagged:多为自身高频 chunk / 引擎事件溢出,编排器忽略,继续接收。
                Err(EventBusError::SlowConsumerDropped { lag, .. }) => {
                    tracing::warn!(lag, "Quest 编排器接收滞后,丢弃部分事件后继续");
                }
                // ChannelClosed 等:所有 Sender 已释放,退出循环。
                Err(e) => {
                    tracing::info!(error = %e, "Quest 编排器订阅结束,退出");
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::{Task, TaskStatus, ThinkingMode};

    /// 构造 TuiChatSubmitted 事件(session_id 固定 "s1")
    fn submit(query: &str) -> NexusEvent {
        NexusEvent::TuiChatSubmitted {
            metadata: EventMetadata::new("chimera-tui"),
            session_id: "s1".into(),
            query: query.into(),
            slash_command: None,
        }
    }

    /// 构造样例 Quest(2 任务,供 build_quest_reply / bench 使用)
    fn sample_quest() -> Quest {
        Quest {
            quest_id: "q-1".into(),
            title: "分析需求".into(),
            tasks: vec![
                Task {
                    task_id: "t1".into(),
                    description: "分析需求".into(),
                    status: TaskStatus::Pending,
                    dependencies: vec![],
                },
                Task {
                    task_id: "t2".into(),
                    description: "设计方案".into(),
                    status: TaskStatus::Pending,
                    dependencies: vec!["t1".into()],
                },
            ],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
            priority: 128,
        }
    }

    #[test]
    fn build_quest_reply_lists_tasks() {
        let reply = build_quest_reply(&sample_quest());
        assert!(reply.contains("2 个任务"), "应含任务数:{reply}");
        assert!(reply.contains("分析需求"), "应含标题");
        assert!(reply.contains("设计方案"), "应含任务描述");
    }

    #[test]
    fn build_error_reply_contains_error() {
        let reply = build_error_reply(&QuestError::CyclicDependency);
        assert!(reply.contains("失败"), "错误回复应含失败提示:{reply}");
    }

    #[test]
    fn plan_chunks_is_per_char() {
        assert_eq!(plan_chunks("abc"), vec!["a", "b", "c"]);
        // 多字节字符按 char 切分(非字节),每个 char 一条
        assert_eq!(plan_chunks("你好").len(), 2);
    }

    #[tokio::test]
    async fn handle_ignores_non_submit() {
        let bus = EventBus::new();
        let engine = QuestEngine::new(bus.clone());
        let mut rx = bus.subscribe();
        let cfg = OrchestratorConfig {
            chunk_delay: Duration::ZERO,
        };

        let unrelated = NexusEvent::CacheHit {
            metadata: EventMetadata::new("test"),
            cache_key: "k-1".into(),
        };
        handle_chat_event(&bus, &engine, &cfg, &unrelated).await;

        // 非 Submit 事件不触发分解/发布 → 订阅缓冲为空
        assert!(
            matches!(rx.try_recv(), Ok(None)),
            "非 TuiChatSubmitted 事件不应触发任何发布"
        );
    }

    #[tokio::test]
    async fn handle_produces_full_sequence() {
        let bus = EventBus::new();
        let engine = QuestEngine::new(bus.clone());
        let mut rx = bus.subscribe();
        let cfg = OrchestratorConfig {
            chunk_delay: Duration::ZERO,
        };

        // 含两句 → 规则分解器真实产出多个任务;await 完成后事件已入 1024 缓冲(无溢出)
        handle_chat_event(&bus, &engine, &cfg, &submit("分析需求。设计方案。")).await;

        let mut saw_thinking = false;
        let mut saw_quest_created = false;
        let mut saw_completed = false;
        let mut saw_idle = false;
        let mut acc = String::new();
        // 有界 drain:容忍 create_quest 广播的 QuestCreated / ThinkingModeSwitched 等穿插
        for _ in 0..512 {
            match rx.try_recv() {
                Ok(Some(NexusEvent::TuiChatStatusChanged {
                    status: ChatStatus::Thinking,
                    ..
                })) => saw_thinking = true,
                Ok(Some(NexusEvent::TuiChatResponseChunk { delta, .. })) => acc.push_str(&delta),
                Ok(Some(NexusEvent::TuiChatCompleted { .. })) => saw_completed = true,
                Ok(Some(NexusEvent::TuiChatStatusChanged {
                    status: ChatStatus::Idle,
                    ..
                })) => {
                    saw_idle = true;
                    break;
                }
                Ok(Some(NexusEvent::QuestCreated { .. })) => saw_quest_created = true,
                Ok(Some(_)) => {}  // 引擎其他广播事件,忽略
                Ok(None) => break, // 缓冲已空
                Err(_) => break,
            }
        }

        assert!(saw_thinking, "应收到 Thinking 状态");
        assert!(
            saw_quest_created,
            "create_quest 应广播 QuestCreated(真实 L9 管线证据)"
        );
        assert!(acc.contains("任务"), "回复应含分解任务信息:{acc}");
        assert!(saw_completed, "应收到 Completed");
        assert!(saw_idle, "应回到 Idle 状态");
    }

    #[tokio::test]
    async fn spawn_quest_orchestrator_streams_on_submit() {
        let bus = EventBus::new();
        let engine = Arc::new(QuestEngine::new(bus.clone()));
        let mut rx = bus.subscribe();
        // subscribe(rx)先于 spawn(内部再 subscribe)与 publish,保证均收到事件
        let handle = spawn_quest_orchestrator(
            bus.clone(),
            engine,
            OrchestratorConfig {
                chunk_delay: Duration::ZERO,
            },
        );

        bus.publish(submit("分析需求。设计方案。")).await.unwrap();

        let mut acc = String::new();
        let (mut saw_thinking, mut saw_completed, mut saw_idle) = (false, false, false);
        // 有界轮询 + 每次 recv 超时兜底,避免调度异常时挂起
        for _ in 0..512 {
            match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
                Ok(Ok(NexusEvent::TuiChatStatusChanged {
                    status: ChatStatus::Thinking,
                    ..
                })) => saw_thinking = true,
                Ok(Ok(NexusEvent::TuiChatResponseChunk { delta, .. })) => acc.push_str(&delta),
                Ok(Ok(NexusEvent::TuiChatCompleted { .. })) => saw_completed = true,
                Ok(Ok(NexusEvent::TuiChatStatusChanged {
                    status: ChatStatus::Idle,
                    ..
                })) => {
                    saw_idle = true;
                    break;
                }
                Ok(Ok(_)) => {} // 自身回环的 Submit / 引擎事件忽略
                _ => break,
            }
        }
        handle.abort();

        assert!(saw_thinking, "应收到 Thinking 状态");
        assert!(acc.contains("任务"), "应流式收到分解任务信息:{acc}");
        assert!(saw_completed, "应收到 Completed");
        assert!(saw_idle, "应回到 Idle 状态");
    }
}
