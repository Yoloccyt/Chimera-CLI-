//! TUI 协议客户端 — WI-01 dogfooding 适配层
//!
//! # 职责
//! TUI 数据层经 **AppOp/AppEvent 协议面** 与核心交互（内嵌 app-server）：
//! - 操作发送: `start_thread` / `submit_turn` 编码为 AppOp，经 AppServer 处理
//! - 事件消费: AppServer 产出 AppEvent 流，经 `apply_to` 落到 QuestSync
//!   （协议面数据保真: Item.payload 承载序列化 Quest）
//!
//! # 与直联路径的关系（A1 双跑窗口）
//! 直联路径（EventBus 订阅）保留，本客户端为协议面平行路径——TUI 可
//! 随时切换事件源（`--protocol` 模式或双跑对账），验证核心-表面分离
//! 后 TUI 无需链接核心 crate（nexus-core/quest-engine）。
//!
//! # 线程安全
//! `AppServer` 内部 DashMap 会话表并发安全；`TuiProtocolClient` 为
//! 单线程 TUI 事件循环服务（Send + Sync 满足 tokio 约束）。

use nexus_app_server::{AppServer, AppServerConfig};
use nexus_contracts::app::{AppEvent, AppOp, ThreadId, UserInput};

use super::sync::QuestSync;

/// TUI 协议客户端 — 内嵌 app-server 的协议面交互层
#[derive(Debug)]
pub struct TuiProtocolClient {
    /// 内嵌 app-server（会话状态机 + CoreBackend seam）
    server: AppServer,
    /// 当前会话 ID（ThreadStart 后建立）
    thread_id: Option<ThreadId>,
    /// 已消费事件计数（审计/对账）
    consumed_events: u64,
}

impl TuiProtocolClient {
    /// 创建协议客户端（默认 InMemoryBackend——生产接入 quest-engine
    /// 时经 `AppServer::with_backend` 注入 CoreBackend 桥接实现）
    pub fn new() -> Self {
        Self {
            server: AppServer::new(AppServerConfig::default()),
            thread_id: None,
            consumed_events: 0,
        }
    }

    /// 启动会话（AppOp::ThreadStart → AppEvent::ThreadStarted）
    pub async fn start_thread(
        &mut self,
        goal_id: &str,
        run_id: &str,
    ) -> Result<Vec<AppEvent>, String> {
        let events = self
            .server
            .handle_op(&AppOp::ThreadStart(
                nexus_contracts::app::ThreadStartParams::new(goal_id, run_id),
            ))
            .await
            .map_err(|e| e.to_string())?;
        // 记录会话 ID（协议约定: thread_id = goal_id::run_id）
        self.thread_id = Some(ThreadId::new(format!("{goal_id}::{run_id}")));
        Ok(events)
    }

    /// 提交回合输入（AppOp::TurnSubmit → ItemChanged/TurnCompleted 流）
    pub async fn submit_turn(&mut self, input: &str) -> Result<Vec<AppEvent>, String> {
        let tid = self
            .thread_id
            .clone()
            .ok_or_else(|| "会话未启动（先调用 start_thread）".to_string())?;
        self.server
            .handle_op(&AppOp::TurnSubmit {
                thread_id: tid,
                input: UserInput::new(input),
            })
            .await
            .map_err(|e| e.to_string())
    }

    /// 将 AppEvent 流应用到 TUI 数据层（QuestSync）
    ///
    /// 事件逐个转发到 `QuestSync::apply_app_event`；返回更新的 Quest 列表
    /// （事件不涉 Quest 状态时返回 None）。
    pub fn apply_to(
        &mut self,
        events: &[AppEvent],
        sync: &mut QuestSync,
    ) -> Option<Vec<nexus_core::Quest>> {
        let mut result = None;
        for ev in events {
            self.consumed_events += 1;
            if let Some(quests) = sync.apply_app_event(ev) {
                result = Some(quests);
            }
        }
        result
    }

    /// 已消费事件计数（协议面审计）
    pub fn consumed_events(&self) -> u64 {
        self.consumed_events
    }

    /// 当前会话 ID
    pub fn thread_id(&self) -> Option<&ThreadId> {
        self.thread_id.as_ref()
    }
}

impl Default for TuiProtocolClient {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn protocol_client_full_flow_updates_quest_sync() {
        // WI-01 dogfooding 验收: TUI 经协议面完成 ThreadStart → TurnSubmit，
        // 事件流落到 QuestSync（数据层与核心经协议交互，不直联核心 crate）
        let mut client = TuiProtocolClient::new();
        let mut sync = QuestSync::default();

        // 1. ThreadStart → QuestSync 新增会话级 Quest
        let events = client
            .start_thread("goal-protocol", "run-1")
            .await
            .expect("启动会话成功");
        let quests = client
            .apply_to(&events, &mut sync)
            .expect("Quest 列表应更新");
        assert_eq!(quests.len(), 1, "ThreadStarted 后应有 1 个 Quest");
        assert_eq!(quests[0].quest_id, "goal-protocol");
        assert_eq!(client.consumed_events(), 1);
        assert!(client.thread_id().is_some());

        // 2. TurnSubmit → ItemChanged 流（InMemoryBackend 产出 message/tool_call）
        let events = client
            .submit_turn("你好，协议客户端")
            .await
            .expect("提交回合成功");
        // InMemoryBackend 的 Item 不含 kind="quest"，apply_to 不更新 Quest 列表
        let quests = client.apply_to(&events, &mut sync);
        assert_eq!(client.consumed_events(), 4, "3 推送帧 + 累计");
        assert_eq!(quests, None, "非 quest Item 不触发列表更新");
        assert_eq!(sync.quests().len(), 1, "Quest 列表保持 1 个");
    }

    #[tokio::test]
    async fn protocol_client_turn_before_thread_rejected() {
        // 防御: 未启动会话即提交回合 → 拒绝（协议面错误语义）
        let mut client = TuiProtocolClient::new();
        let err = client.submit_turn("x").await.expect_err("必须拒绝");
        assert!(err.contains("会话未启动"), "错误消息应指引先 start_thread");
    }

    #[tokio::test]
    async fn protocol_client_quest_payload_roundtrip() {
        // 协议面数据保真: Quest 序列化进 Item payload → TUI 反序列化还原
        let mut client = TuiProtocolClient::new();
        let mut sync = QuestSync::default();

        let events = client
            .start_thread("goal-payload", "run-1")
            .await
            .expect("启动会话成功");
        client.apply_to(&events, &mut sync);

        // 构造携带完整 Quest 数据的 Item（核心侧序列化 → 协议面 → TUI 还原）
        let quest = nexus_core::Quest {
            quest_id: "goal-payload".into(),
            title: "完整数据保真".into(),
            ..nexus_core::Quest::default()
        };
        let payload = serde_json::to_string(&quest).expect("Quest 序列化成功");
        let item = nexus_contracts::app::Item::new(
            nexus_contracts::app::ItemId::new("i-quest-1"),
            nexus_contracts::app::ThreadId::new("goal-payload::run-1"),
            nexus_contracts::app::TurnId::new("turn-1"),
            "quest",
            nexus_contracts::app::ItemStatus::Completed,
            &payload,
        );
        let updated = sync
            .apply_app_event(&AppEvent::ItemChanged { item })
            .expect("payload 应更新 Quest");
        assert_eq!(updated[0].quest_id, "goal-payload");
        assert_eq!(updated[0].title, "完整数据保真", "payload 全量保真还原");
    }

    #[tokio::test]
    async fn protocol_client_quest_completion_removes_entry() {
        // 完成语义: kind="quest_completed" → 从活动列表移除
        let mut client = TuiProtocolClient::new();
        let mut sync = QuestSync::default();

        let events = client
            .start_thread("goal-done", "run-1")
            .await
            .expect("启动会话成功");
        client.apply_to(&events, &mut sync);
        assert_eq!(sync.quests().len(), 1);

        let item = nexus_contracts::app::Item::new(
            nexus_contracts::app::ItemId::new("i-done-1"),
            nexus_contracts::app::ThreadId::new("goal-done::run-1"),
            nexus_contracts::app::TurnId::new("turn-1"),
            "quest_completed",
            nexus_contracts::app::ItemStatus::Completed,
            r#"{"quest_id":"goal-done"}"#,
        );
        let updated = sync
            .apply_app_event(&AppEvent::ItemChanged { item })
            .expect("完成事件应更新列表");
        assert!(updated.is_empty(), "完成后活动列表应为空");
    }
}
