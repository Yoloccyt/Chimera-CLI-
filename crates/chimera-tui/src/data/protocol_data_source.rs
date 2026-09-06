//! 协议数据源 — TUI 协议模式数据装配（WI-01 dogfooding 最终接线）
//!
//! # 职责
//! 实现 [`TuiDataSource`]，使 TUI 主循环在**协议模式**下从 AppOp/AppEvent
//! 协议面获取数据：Quest 生命周期（ThreadStart/TurnSubmit/完成）经协议面
//! 驱动，Quest 面板从协议事件流重建状态——TUI 数据层与核心经协议交互，
//! 不直联核心 crate（核心-表面分离验证）。
//!
//! # 与直联路径的关系（A1 双跑窗口）
//! - 直联路径（DataPipeline + EventSubscriber）保留——其他面板（Budget/
//!   Memory/Security 等）在双跑窗口内继续走直联或显示默认空数据
//! - 协议模式聚焦 Quest 生命周期全链路（核心-表面分离的最小完整闭环）
//!
//! # 线程安全
//! `snapshot()` 为同步读取（Arc 共享快照）；协议事件由外部（主循环）
//! 注入 [`ProtocolDataSource::apply_events`]，无后台任务（调用方驱动）。

use std::sync::Arc;

use nexus_contracts::app::AppEvent;

use super::protocol_client::TuiProtocolClient;
use super::snapshot::{DataSnapshot, DataSourceConfig, TuiDataSource};
use super::sync::QuestSync;
use crate::error::TuiError;

/// 协议数据源 — 协议模式的数据装配实现
#[derive(Debug)]
pub struct ProtocolDataSource {
    /// 协议客户端（AppOp 发送 + AppEvent 接收）
    client: TuiProtocolClient,
    /// Quest 同步器（协议面事件 → Quest 状态）
    quest_sync: QuestSync,
    /// 快照修订号（每轮事件应用 +1，供面板脏检查）
    revision: u64,
    /// 数据源配置
    config: DataSourceConfig,
}

impl ProtocolDataSource {
    /// 创建协议数据源
    pub fn new(config: DataSourceConfig) -> Self {
        Self {
            client: TuiProtocolClient::new(),
            quest_sync: QuestSync::default(),
            revision: 0,
            config,
        }
    }

    /// 启动协议会话（AppOp::ThreadStart → QuestSync 新增会话级 Quest）
    pub async fn start_session(&mut self, goal_id: &str, run_id: &str) -> Result<(), TuiError> {
        let events = self
            .client
            .start_thread(goal_id, run_id)
            .await
            .map_err(TuiError::DataSource)?;
        self.apply_events(&events);
        Ok(())
    }

    /// 提交回合（AppOp::TurnSubmit → 协议事件流）
    pub async fn submit_turn(&mut self, input: &str) -> Result<(), TuiError> {
        let events = self
            .client
            .submit_turn(input)
            .await
            .map_err(TuiError::DataSource)?;
        self.apply_events(&events);
        Ok(())
    }

    /// 应用协议事件流（外部/客户端注入 → QuestSync）
    ///
    /// 每应用一个事件 revision +1（面板脏检查依据）。
    pub fn apply_events(&mut self, events: &[AppEvent]) {
        self.client.apply_to(events, &mut self.quest_sync);
        if !events.is_empty() {
            self.revision += events.len() as u64;
        }
    }

    /// 当前会话 ID（协议面审计）
    pub fn thread_id(&self) -> Option<&nexus_contracts::app::ThreadId> {
        self.client.thread_id()
    }

    /// 协议事件消费计数
    pub fn consumed_events(&self) -> u64 {
        self.client.consumed_events()
    }

    /// 从协议面事件构造快照（Quest 部分真实，其余面板默认空——双跑窗口过渡态）
    fn build_snapshot(&self) -> DataSnapshot {
        DataSnapshot {
            quest_list: self.quest_sync.quests(),
            revision: self.revision,
            ..DataSnapshot::default()
        }
    }
}

impl TuiDataSource for ProtocolDataSource {
    fn snapshot(&self) -> Result<Arc<DataSnapshot>, TuiError> {
        Ok(Arc::new(self.build_snapshot()))
    }

    fn config(&self) -> &DataSourceConfig {
        &self.config
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> DataSourceConfig {
        DataSourceConfig {
            max_event_history: 100,
            max_quest_list_size: 50,
            budget_metrics_ttl_ms: 5000,
            // 其余字段用默认值（DataPipeline 同构构造）
            ..DataSourceConfig::default()
        }
    }

    #[tokio::test]
    async fn protocol_data_source_full_quest_flow() {
        // WI-01 dogfooding 验收: TUI 协议模式完整 Quest 生命周期
        // ThreadStart → TurnSubmit → snapshot 含协议面 Quest 状态
        let mut ds = ProtocolDataSource::new(test_config());

        // 1. 启动会话 → Quest 面板数据来自协议面
        ds.start_session("goal-p0", "run-1")
            .await
            .expect("启动成功");
        let snap = ds.snapshot().expect("快照可读");
        assert_eq!(snap.quest_list.len(), 1, "协议面 Quest 应进入快照");
        assert_eq!(snap.quest_list[0].quest_id, "goal-p0");
        assert!(ds.thread_id().is_some());
        assert_eq!(ds.consumed_events(), 1, "ThreadStarted 已消费");

        // 2. 提交回合 → 协议事件流消费（InMemoryBackend Item 非 quest 类）
        ds.submit_turn("你好，协议模式").await.expect("提交成功");
        let snap = ds.snapshot().expect("快照可读");
        assert_eq!(snap.quest_list.len(), 1, "Quest 列表保持");
        assert!(ds.consumed_events() >= 4, "协议事件流已消费");
        assert!(snap.revision > 0, "修订号已推进（面板脏检查依据）");
    }

    #[tokio::test]
    async fn protocol_data_source_quest_payload_update() {
        // 协议面数据保真: Item payload 承载完整 Quest → 快照还原
        let mut ds = ProtocolDataSource::new(test_config());
        ds.start_session("goal-p1", "run-1")
            .await
            .expect("启动成功");

        // 核心侧序列化 Quest 进 payload（协议面全量数据）
        let quest = nexus_core::Quest {
            quest_id: "goal-p1".into(),
            title: "协议模式完整数据".into(),
            ..nexus_core::Quest::default()
        };
        let payload = serde_json::to_string(&quest).expect("序列化成功");
        let item = nexus_contracts::app::Item::new(
            nexus_contracts::app::ItemId::new("i-q-1"),
            nexus_contracts::app::ThreadId::new("goal-p1::run-1"),
            nexus_contracts::app::TurnId::new("turn-1"),
            "quest",
            nexus_contracts::app::ItemStatus::Completed,
            &payload,
        );
        ds.apply_events(&[AppEvent::ItemChanged { item }]);

        let snap = ds.snapshot().expect("快照可读");
        assert_eq!(
            snap.quest_list[0].title, "协议模式完整数据",
            "payload 全量保真"
        );
    }

    #[tokio::test]
    async fn protocol_data_source_completion_removes_quest() {
        // 完成语义: quest_completed → 活动列表移除 → 快照更新
        let mut ds = ProtocolDataSource::new(test_config());
        ds.start_session("goal-p2", "run-1")
            .await
            .expect("启动成功");

        let item = nexus_contracts::app::Item::new(
            nexus_contracts::app::ItemId::new("i-done-1"),
            nexus_contracts::app::ThreadId::new("goal-p2::run-1"),
            nexus_contracts::app::TurnId::new("turn-1"),
            "quest_completed",
            nexus_contracts::app::ItemStatus::Completed,
            r#"{"quest_id":"goal-p2"}"#,
        );
        ds.apply_events(&[AppEvent::ItemChanged { item }]);

        let snap = ds.snapshot().expect("快照可读");
        assert!(snap.quest_list.is_empty(), "完成后活动列表应为空");
    }

    #[test]
    fn protocol_data_source_empty_snapshot_renders() {
        // TuiDataSource 契约: 无事件时返回默认空快照（面板始终可渲染）
        let ds = ProtocolDataSource::new(test_config());
        let snap = ds.snapshot().expect("空快照可读");
        assert!(snap.quest_list.is_empty());
        assert_eq!(snap.revision, 0);
    }
}
