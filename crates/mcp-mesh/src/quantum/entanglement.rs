//! 纠缠链接 — 服务器间状态同步策略
//!
//! 对应架构层:L10 Interface
//!
//! ## 量子纠缠语义
//! 两个服务器建立"纠缠链接"后,一端状态变更需按 `SyncStrategy` 同步至另一端:
//! - `Eager`:立即同步(强一致,延迟高)— `sync_state_change` 内直接 `bus.publish().await`
//! - `Lazy`:周期同步(最终一致,延迟低)— 推入 `lazy_buffer`,由 `flush_lazy_buffer()` 批量发布
//! - `BestEffort`:尽力同步(失败不重试,适合低优先级状态)— `tokio::spawn` fire-and-forget
//!
//! ## Task 0.7 v2.9.0-omega SubTask 0.7.9
//!
//! 原实现仅为 in-process 注册表,无真实同步逻辑。SubTask 0.7.9 引入 EventBus 集成,
//! 使状态变更能通过事件流传播至订阅者(同进程的对端服务器 / TUI 监控面板 / 审计日志)。
//!
//! ## fire-and-forget 评估(§4.4 反模式 #7)
//!
//! `BestEffort` 策略使用 `tokio::spawn` fire-and-forget,符合反模式 #7 的适用条件:
//! - **幂等**:状态同步事件重复发布不破坏一致性(订阅者取最新状态)
//! - **非关键路径**:状态同步失败不影响 2PC 事务正确性,仅影响监控实时性
//! - **不影响数据一致性**:状态可由下次心跳重建

use std::sync::{Arc, Mutex};

use chrono::Utc;
use dashmap::DashMap;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::error::McpError;

/// 同步策略 — 控制纠缠链接两端的状态同步时机
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SyncStrategy {
    /// 立即同步:状态变更后立刻推送至对端(强一致,延迟高)
    Eager,
    /// 周期同步:按固定周期批量同步(最终一致,延迟低)
    Lazy,
    /// 尽力同步:推送失败不重试(适合低优先级状态)
    BestEffort,
}

impl SyncStrategy {
    /// 策略名称(用于日志与序列化)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Eager => "Eager",
            Self::Lazy => "Lazy",
            Self::BestEffort => "BestEffort",
        }
    }
}

impl std::fmt::Display for SyncStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 纠缠链接 — 描述两个服务器间的状态同步关系
///
/// `linked_servers` 的两个元素顺序无关(对称链接),但构造时会规范化为
/// 字典序较小的在前,便于 `EntanglementManager` 去重。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntanglementLink {
    /// 链接的两端服务器 ID(字典序规范化:(min, max))
    pub linked_servers: (String, String),
    /// 同步策略
    pub sync_strategy: SyncStrategy,
}

impl EntanglementLink {
    /// 创建纠缠链接 — 自动规范化服务器顺序(字典序小的在前)
    ///
    /// # 错误
    /// - `InvalidEntanglement`:两端服务器 ID 相同
    pub fn new(
        a: impl Into<String>,
        b: impl Into<String>,
        sync_strategy: SyncStrategy,
    ) -> Result<Self, McpError> {
        let a = a.into();
        let b = b.into();
        if a == b {
            return Err(McpError::InvalidEntanglement {
                reason: format!("两端服务器不能相同: {a}"),
            });
        }
        // 规范化:字典序小的在前,确保 (a,b) 与 (b,a) 视为同一条链接
        let linked_servers = if a <= b { (a, b) } else { (b, a) };
        Ok(Self {
            linked_servers,
            sync_strategy,
        })
    }

    /// 判断指定服务器是否为链接的一端
    pub fn involves(&self, server_id: &str) -> bool {
        self.linked_servers.0 == server_id || self.linked_servers.1 == server_id
    }

    /// 获取对端服务器 ID(若指定服务器在链接中)
    pub fn partner_of(&self, server_id: &str) -> Option<&str> {
        if self.linked_servers.0 == server_id {
            Some(&self.linked_servers.1)
        } else if self.linked_servers.1 == server_id {
            Some(&self.linked_servers.0)
        } else {
            None
        }
    }
}

/// 待同步的状态变更记录(Task 0.7 v2.9.0-omega SubTask 0.7.9)
///
/// `Lazy` 策略下,`sync_state_change` 将变更推入 `lazy_buffer`,
/// 由 `flush_lazy_buffer()` 周期批量发布至 EventBus。
///
/// # 字段
/// - `source_server`:状态变更的源服务器 ID
/// - `partner_server`:需同步至的对端服务器 ID
/// - `state_payload`:状态变更内容(如 "capability_added:tool-x" / "status:degraded")
#[derive(Debug, Clone)]
pub struct PendingSync {
    /// 状态变更的源服务器 ID
    pub source_server: String,
    /// 需同步至的对端服务器 ID
    pub partner_server: String,
    /// 状态变更内容(语义由调用方定义)
    pub state_payload: String,
}

/// 纠缠链接管理器 — 基于 DashMap 的并发安全注册表 + EventBus 状态同步
///
/// 链接以 `(min_id, max_id)` 元组为 key,确保同一对服务器的链接只注册一次。
/// `register` 时若链接已存在,更新 sync_strategy 并返回旧值。
///
/// Task 0.7 v2.9.0-omega SubTask 0.7.9 新增:
/// - `event_bus`:可选 EventBus 引用,启用后 `sync_state_change` 可发布状态变更事件
/// - `lazy_buffer`:`Lazy` 策略的待同步队列,由 `flush_lazy_buffer()` 批量消费
pub struct EntanglementManager {
    /// 纠缠链接注册表
    links: DashMap<(String, String), SyncStrategy>,
    /// 可选 EventBus — 启用后 `sync_state_change` 按策略发布事件
    ///
    /// WHY Option:无 EventBus 时(如纯单元测试)`sync_state_change` 仍可记录
    /// 链接关系,只是不发布事件,保持向后兼容。
    event_bus: Option<EventBus>,
    /// `Lazy` 策略的待同步队列 — `Arc<Mutex<Vec>>` 因 `flush_lazy_buffer` 需异步消费
    ///
    /// WHY `std::sync::Mutex` 而非 `tokio::sync::Mutex`:缓冲区操作为快速 push/take,
    /// 不跨 await 持有(§4.4 反模式 #1:锁内取快照→释放→await)。
    lazy_buffer: Arc<Mutex<Vec<PendingSync>>>,
}

impl EntanglementManager {
    /// 创建空管理器(无 EventBus,`sync_state_change` 将不发布事件)
    pub fn new() -> Self {
        Self {
            links: DashMap::new(),
            event_bus: None,
            lazy_buffer: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 创建带 EventBus 的管理器(Task 0.7 v2.9.0-omega SubTask 0.7.9)
    ///
    /// 启用后 `sync_state_change` 按 `SyncStrategy` 发布 `McpNodeHeartbeat` 事件:
    /// - `Eager`:立即 `bus.publish().await`
    /// - `Lazy`:推入 `lazy_buffer`,由 `flush_lazy_buffer()` 批量发布
    /// - `BestEffort`:`tokio::spawn` fire-and-forget
    ///
    /// # 参数
    /// - `bus`:事件总线(将 clone 存储在管理器内)
    pub fn with_event_bus(bus: EventBus) -> Self {
        Self {
            links: DashMap::new(),
            event_bus: Some(bus),
            lazy_buffer: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 返回 EventBus 引用(若已配置)
    pub fn event_bus(&self) -> Option<&EventBus> {
        self.event_bus.as_ref()
    }

    /// 当前 lazy_buffer 中待同步条目数量(用于测试与监控)
    pub fn lazy_buffer_len(&self) -> usize {
        self.lazy_buffer.lock().map(|buf| buf.len()).unwrap_or(0)
    }

    /// 注册纠缠链接 — 若已存在则更新策略,返回旧策略(若有)
    pub fn register(&self, link: EntanglementLink) -> Result<Option<SyncStrategy>, McpError> {
        let key = link.linked_servers.clone();
        // DashMap::insert 返回旧值
        let old = self.links.insert(key, link.sync_strategy);
        Ok(old)
    }

    /// 注销纠缠链接,返回被移除的策略(若存在)
    pub fn unregister(&self, a: &str, b: &str) -> Option<SyncStrategy> {
        let key = if a <= b {
            (a.to_string(), b.to_string())
        } else {
            (b.to_string(), a.to_string())
        };
        self.links.remove(&key).map(|(_, v)| v)
    }

    /// 查询指定服务器参与的所有链接策略
    pub fn strategies_for(&self, server_id: &str) -> Vec<SyncStrategy> {
        self.links
            .iter()
            .filter(|entry| {
                let (a, b) = entry.key();
                a == server_id || b == server_id
            })
            .map(|entry| *entry.value())
            .collect()
    }

    /// 获取指定服务器对之间的同步策略(若存在)
    pub fn get(&self, a: &str, b: &str) -> Option<SyncStrategy> {
        let key = if a <= b {
            (a.to_string(), b.to_string())
        } else {
            (b.to_string(), a.to_string())
        };
        self.links.get(&key).map(|r| *r.value())
    }

    /// 当前链接数量
    pub fn len(&self) -> usize {
        self.links.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    /// 同步状态变更至所有纠缠链接的对端服务器(Task 0.7 v2.9.0-omega SubTask 0.7.9)
    ///
    /// 查找 `source_server` 参与的所有纠缠链接,按各链接的 `SyncStrategy` 同步:
    /// - `Eager`:立即通过 EventBus 发布 `McpNodeHeartbeat` 事件(强一致)
    /// - `Lazy`:推入 `lazy_buffer`,由 `flush_lazy_buffer()` 周期批量发布(最终一致)
    /// - `BestEffort`:`tokio::spawn` fire-and-forget 发布(失败仅告警)
    ///
    /// # 参数
    /// - `source_server`:状态变更的源服务器 ID
    /// - `state_payload`:状态变更内容(如 "capability_added:tool-x")
    ///
    /// # 返回
    /// - `Ok(())`:同步已按策略派发(Eager 已 await 完成,Lazy 已入队,BestEffort 已 spawn)
    /// - `Err`:仅在 Eager 策略下 EventBus 发布失败时返回(Lazy/BestEffort 不传播错误)
    ///
    /// # 无 EventBus 时的行为
    /// 若管理器未配置 EventBus(`new()` 而非 `with_event_bus()`):
    /// - `Eager`/`BestEffort`:跳过发布,记 debug 日志(不报错,保持向后兼容)
    /// - `Lazy`:仍推入 `lazy_buffer`(可由调用方后续取出处理)
    pub async fn sync_state_change(
        &self,
        source_server: &str,
        state_payload: &str,
    ) -> Result<(), McpError> {
        // 1. 收集 source_server 参与的所有链接(partner + strategy)
        // WHY 先收集再处理:避免 DashMap 读锁跨 await(§4.4 反模式 #1)
        let partners: Vec<(String, SyncStrategy)> = self
            .links
            .iter()
            .filter_map(|entry| {
                let (a, b) = entry.key();
                if a == source_server {
                    Some((b.clone(), *entry.value()))
                } else if b == source_server {
                    Some((a.clone(), *entry.value()))
                } else {
                    None
                }
            })
            .collect();

        if partners.is_empty() {
            debug!(server = %source_server, "无纠缠链接,跳过状态同步");
            return Ok(());
        }

        for (partner, strategy) in partners {
            match strategy {
                SyncStrategy::Eager => {
                    // 立即发布 — 强一致路径
                    if let Some(bus) = &self.event_bus {
                        let event = build_state_sync_event(source_server, &partner, state_payload);
                        if let Err(e) = bus.publish(event).await {
                            warn!(
                                source = %source_server,
                                partner = %partner,
                                error = %e,
                                "Eager 状态同步发布失败"
                            );
                            return Err(McpError::EventBusPublish {
                                reason: format!("Eager 同步失败: {e}"),
                            });
                        }
                        debug!(
                            source = %source_server,
                            partner = %partner,
                            "Eager 状态同步已发布"
                        );
                    } else {
                        debug!(
                            source = %source_server,
                            partner = %partner,
                            "无 EventBus,Eager 同步跳过"
                        );
                    }
                }
                SyncStrategy::Lazy => {
                    // 推入缓冲区 — 最终一致路径
                    // WHY lock 后立即 push 并释放:不跨 await 持有锁
                    let pending = PendingSync {
                        source_server: source_server.to_string(),
                        partner_server: partner.clone(),
                        state_payload: state_payload.to_string(),
                    };
                    if let Ok(mut buf) = self.lazy_buffer.lock() {
                        buf.push(pending);
                        debug!(
                            source = %source_server,
                            partner = %partner,
                            buffer_len = buf.len(),
                            "Lazy 状态变更已入缓冲区"
                        );
                    } else {
                        warn!(
                            source = %source_server,
                            partner = %partner,
                            "lazy_buffer 中毒,状态变更丢失"
                        );
                    }
                }
                SyncStrategy::BestEffort => {
                    // fire-and-forget — 适合低优先级状态(§4.4 反模式 #7)
                    if let Some(bus) = &self.event_bus {
                        let bus = bus.clone();
                        let source = source_server.to_string();
                        let payload = state_payload.to_string();
                        // WHY clone before move:spawn 闭包会 move partner,
                        // 而 spawn 后的 debug! 仍需引用 partner,故先克隆一份给闭包
                        let partner_for_spawn = partner.clone();
                        tokio::spawn(async move {
                            let event =
                                build_state_sync_event(&source, &partner_for_spawn, &payload);
                            if let Err(e) = bus.publish(event).await {
                                warn!(
                                    source = %source,
                                    partner = %partner_for_spawn,
                                    error = %e,
                                    "BestEffort 状态同步发布失败(fire-and-forget,已忽略)"
                                );
                            }
                        });
                        debug!(
                            source = %source_server,
                            partner = %partner,
                            "BestEffort 状态同步已 spawn"
                        );
                    } else {
                        debug!(
                            source = %source_server,
                            partner = %partner,
                            "无 EventBus,BestEffort 同步跳过"
                        );
                    }
                }
            }
        }

        Ok(())
    }

    /// 批量发布 `Lazy` 策略缓冲区中的待同步状态变更(Task 0.7 v2.9.0-omega SubTask 0.7.9)
    ///
    /// 周期性调用(如每 5s),将 `lazy_buffer` 中累积的状态变更批量发布至 EventBus。
    /// 发布失败的条目记告警并丢弃(符合 Lazy 最终一致语义:下次状态变更会再推入)。
    ///
    /// # 返回
    /// 成功发布的条目数(用于监控与日志)
    ///
    /// # 无 EventBus 时的行为
    /// 若管理器未配置 EventBus,清空缓冲区并返回 0(避免缓冲区无限增长)。
    pub async fn flush_lazy_buffer(&self) -> usize {
        // 1. 取出缓冲区快照并清空(锁内快速操作,不跨 await)
        // WHY std::mem::take:原子地取出全部条目并清空 Vec,避免部分发布后崩溃丢数据
        let pending_batch: Vec<PendingSync> = match self.lazy_buffer.lock() {
            Ok(mut buf) => std::mem::take(&mut *buf),
            Err(_) => {
                warn!("lazy_buffer 中毒,flush 失败");
                return 0;
            }
        };

        if pending_batch.is_empty() {
            return 0;
        }

        let bus = match &self.event_bus {
            Some(bus) => bus,
            None => {
                debug!(
                    count = pending_batch.len(),
                    "无 EventBus,丢弃 lazy_buffer 中的待同步条目"
                );
                return 0;
            }
        };

        let mut published = 0usize;
        for pending in pending_batch {
            let event = build_state_sync_event(
                &pending.source_server,
                &pending.partner_server,
                &pending.state_payload,
            );
            match bus.publish(event).await {
                Ok(()) => published += 1,
                Err(e) => {
                    warn!(
                        source = %pending.source_server,
                        partner = %pending.partner_server,
                        error = %e,
                        "Lazy 批量同步:单条发布失败,丢弃(下次状态变更会重新推入)"
                    );
                }
            }
        }

        debug!(published, "Lazy 批量同步完成");
        published
    }
}

/// 构建状态同步用的 `McpNodeHeartbeat` 事件(Task 0.7 v2.9.0-omega SubTask 0.7.9)
///
/// WHY 复用 `McpNodeHeartbeat` 而非新增事件变体:
/// - `McpNodeHeartbeat` 已有 `node_id` + `status` + `last_seen` 字段,完全满足状态同步需求
/// - 新增事件变体需修改 `severity()` / `event_name()` 等多处,且需更新 109 变体计数
/// - `status` 字段编码为 `"sync:{source}->{partner}:{payload}"` 格式,订阅者可解析
/// - `throughput` 设为 0(状态同步不携带吞吐量信息)
fn build_state_sync_event(
    source_server: &str,
    partner_server: &str,
    state_payload: &str,
) -> NexusEvent {
    NexusEvent::McpNodeHeartbeat {
        metadata: EventMetadata::new("mcp-mesh-entanglement"),
        node_id: source_server.to_string(),
        // 编码格式:sync:{source}->{partner}:{payload}
        // WHY 编码而非新字段:复用现有 status 字段,避免修改事件结构
        status: format!("sync:{source_server}->{partner_server}:{state_payload}"),
        throughput: 0,
        last_seen: Utc::now(),
    }
}

impl Default for EntanglementManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_link_new_normalizes_order() {
        let link1 = EntanglementLink::new("s-2", "s-1", SyncStrategy::Eager).expect("创建失败");
        let link2 = EntanglementLink::new("s-1", "s-2", SyncStrategy::Eager).expect("创建失败");
        // 顺序无关,规范化后应相等
        assert_eq!(link1, link2);
        assert_eq!(link1.linked_servers, ("s-1".into(), "s-2".into()));
    }

    #[test]
    fn test_link_rejects_same_servers() {
        let err = EntanglementLink::new("s-1", "s-1", SyncStrategy::Lazy).unwrap_err();
        assert!(matches!(err, McpError::InvalidEntanglement { .. }));
    }

    #[test]
    fn test_link_involves_and_partner() {
        let link = EntanglementLink::new("s-1", "s-2", SyncStrategy::Eager).expect("创建失败");
        assert!(link.involves("s-1"));
        assert!(link.involves("s-2"));
        assert!(!link.involves("s-3"));

        assert_eq!(link.partner_of("s-1"), Some("s-2"));
        assert_eq!(link.partner_of("s-2"), Some("s-1"));
        assert_eq!(link.partner_of("s-3"), None);
    }

    #[test]
    fn test_manager_register_and_get() {
        let mgr = EntanglementManager::new();
        let link = EntanglementLink::new("s-1", "s-2", SyncStrategy::Eager).expect("创建失败");
        assert!(mgr.register(link).expect("注册失败").is_none());

        assert_eq!(mgr.get("s-1", "s-2"), Some(SyncStrategy::Eager));
        assert_eq!(mgr.get("s-2", "s-1"), Some(SyncStrategy::Eager)); // 顺序无关
        assert_eq!(mgr.get("s-1", "s-3"), None);
        assert_eq!(mgr.len(), 1);
    }

    #[test]
    fn test_manager_register_updates_strategy() {
        let mgr = EntanglementManager::new();
        let link1 = EntanglementLink::new("s-1", "s-2", SyncStrategy::Eager).expect("创建失败");
        mgr.register(link1).expect("注册失败");

        let link2 = EntanglementLink::new("s-2", "s-1", SyncStrategy::Lazy).expect("创建失败");
        let old = mgr.register(link2).expect("注册失败");
        assert_eq!(old, Some(SyncStrategy::Eager));
        assert_eq!(mgr.get("s-1", "s-2"), Some(SyncStrategy::Lazy));
        assert_eq!(mgr.len(), 1); // 仍是一条链接
    }

    #[test]
    fn test_manager_unregister() {
        let mgr = EntanglementManager::new();
        let link = EntanglementLink::new("s-1", "s-2", SyncStrategy::BestEffort).expect("创建失败");
        mgr.register(link).expect("注册失败");

        let removed = mgr.unregister("s-2", "s-1"); // 顺序无关
        assert_eq!(removed, Some(SyncStrategy::BestEffort));
        assert!(mgr.is_empty());
    }

    #[test]
    fn test_manager_strategies_for() {
        let mgr = EntanglementManager::new();
        mgr.register(EntanglementLink::new("s-1", "s-2", SyncStrategy::Eager).expect("创建失败"))
            .expect("注册失败");
        mgr.register(EntanglementLink::new("s-1", "s-3", SyncStrategy::Lazy).expect("创建失败"))
            .expect("注册失败");
        mgr.register(
            EntanglementLink::new("s-2", "s-3", SyncStrategy::BestEffort).expect("创建失败"),
        )
        .expect("注册失败");

        let strategies = mgr.strategies_for("s-1");
        assert_eq!(strategies.len(), 2);
        assert!(strategies.contains(&SyncStrategy::Eager));
        assert!(strategies.contains(&SyncStrategy::Lazy));

        let s2_strategies = mgr.strategies_for("s-2");
        assert_eq!(s2_strategies.len(), 2);
    }

    #[test]
    fn test_sync_strategy_serde() {
        let s = SyncStrategy::Eager;
        let json = serde_json::to_string(&s).expect("序列化失败");
        let restored: SyncStrategy = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(s, restored);
    }

    // === Task 0.7 v2.9.0-omega SubTask 0.7.9 同步逻辑测试 ===

    #[test]
    fn test_sync_state_change_no_links_returns_ok() {
        // 无纠缠链接时 sync_state_change 应快速返回 Ok
        let mgr = EntanglementManager::new();
        let rt = tokio::runtime::Runtime::new().expect("创建 runtime 失败");
        let result = rt.block_on(mgr.sync_state_change("s-1", "status:online"));
        assert!(result.is_ok(), "无链接时应返回 Ok");
    }

    #[test]
    fn test_sync_state_change_lazy_without_event_bus_buffers() {
        // 无 EventBus 时 Lazy 策略仍应推入缓冲区
        let mgr = EntanglementManager::new();
        mgr.register(EntanglementLink::new("s-1", "s-2", SyncStrategy::Lazy).expect("创建失败"))
            .expect("注册失败");

        let rt = tokio::runtime::Runtime::new().expect("创建 runtime 失败");
        rt.block_on(mgr.sync_state_change("s-1", "capability_added:tool-x"))
            .expect("sync 失败");

        assert_eq!(mgr.lazy_buffer_len(), 1, "Lazy 应推入 1 条待同步记录");
    }

    #[test]
    fn test_sync_state_change_lazy_with_multiple_partners() {
        // s-1 与 s-2(Lazy) + s-1 与 s-3(Lazy) → sync 应推入 2 条
        let mgr = EntanglementManager::new();
        mgr.register(EntanglementLink::new("s-1", "s-2", SyncStrategy::Lazy).expect("创建失败"))
            .expect("注册失败");
        mgr.register(EntanglementLink::new("s-1", "s-3", SyncStrategy::Lazy).expect("创建失败"))
            .expect("注册失败");

        let rt = tokio::runtime::Runtime::new().expect("创建 runtime 失败");
        rt.block_on(mgr.sync_state_change("s-1", "status:degraded"))
            .expect("sync 失败");

        assert_eq!(mgr.lazy_buffer_len(), 2, "两条 Lazy 链接应推入 2 条记录");
    }

    #[test]
    fn test_flush_lazy_buffer_empty_returns_zero() {
        let mgr = EntanglementManager::new();
        let rt = tokio::runtime::Runtime::new().expect("创建 runtime 失败");
        let published = rt.block_on(mgr.flush_lazy_buffer());
        assert_eq!(published, 0, "空缓冲区 flush 应返回 0");
    }

    #[test]
    fn test_flush_lazy_buffer_without_event_bus_clears_and_returns_zero() {
        // 无 EventBus 时 flush 应清空缓冲区并返回 0(避免无限增长)
        let mgr = EntanglementManager::new();
        mgr.register(EntanglementLink::new("s-1", "s-2", SyncStrategy::Lazy).expect("创建失败"))
            .expect("注册失败");

        let rt = tokio::runtime::Runtime::new().expect("创建 runtime 失败");
        rt.block_on(mgr.sync_state_change("s-1", "payload"))
            .expect("sync 失败");
        assert_eq!(mgr.lazy_buffer_len(), 1, "sync 后应有 1 条待同步");

        let published = rt.block_on(mgr.flush_lazy_buffer());
        assert_eq!(published, 0, "无 EventBus 时应返回 0");
        assert_eq!(mgr.lazy_buffer_len(), 0, "flush 后缓冲区应清空");
    }

    #[tokio::test]
    async fn test_sync_state_change_eager_with_event_bus_publishes_event() {
        // Eager 策略 + EventBus → 应立即发布 McpNodeHeartbeat 事件
        let bus = EventBus::new();
        let mut rx = bus.subscribe(); // 关键:在 sync 之前订阅(broadcast 不缓存历史)

        let mgr = EntanglementManager::with_event_bus(bus);
        mgr.register(EntanglementLink::new("s-1", "s-2", SyncStrategy::Eager).expect("创建失败"))
            .expect("注册失败");

        mgr.sync_state_change("s-1", "status:online")
            .await
            .expect("Eager sync 不应失败");

        // 应收到 McpNodeHeartbeat 事件
        let event = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .await
            .expect("应收到事件");

        match event {
            NexusEvent::McpNodeHeartbeat {
                node_id,
                status,
                throughput,
                ..
            } => {
                assert_eq!(node_id, "s-1");
                assert!(status.contains("sync:s-1->s-2:status:online"));
                assert_eq!(throughput, 0);
            }
            other => panic!("期望 McpNodeHeartbeat,收到 {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sync_state_change_lazy_with_event_bus_flush_publishes() {
        // Lazy 策略 + EventBus → sync 入缓冲区,flush 后发布事件
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let mgr = EntanglementManager::with_event_bus(bus);
        mgr.register(EntanglementLink::new("s-1", "s-2", SyncStrategy::Lazy).expect("创建失败"))
            .expect("注册失败");

        // sync 应入缓冲区,不立即发布
        mgr.sync_state_change("s-1", "capability_added:tool-x")
            .await
            .expect("Lazy sync 不应失败");
        assert_eq!(mgr.lazy_buffer_len(), 1, "应入缓冲区");

        // flush 前不应有事件
        assert!(
            rx.try_recv().expect("不应有事件").is_none(),
            "flush 前不应发布事件"
        );

        // flush 后应发布事件
        let published = mgr.flush_lazy_buffer().await;
        assert_eq!(published, 1, "应发布 1 条");

        let event = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .await
            .expect("应收到事件");

        match event {
            NexusEvent::McpNodeHeartbeat {
                node_id, status, ..
            } => {
                assert_eq!(node_id, "s-1");
                assert!(status.contains("sync:s-1->s-2:capability_added:tool-x"));
            }
            other => panic!("期望 McpNodeHeartbeat,收到 {other:?}"),
        }

        assert_eq!(mgr.lazy_buffer_len(), 0, "flush 后缓冲区应清空");
    }

    #[tokio::test]
    async fn test_sync_state_change_best_effort_does_not_block() {
        // BestEffort 策略应 spawn fire-and-forget,不阻塞调用方
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let mgr = EntanglementManager::with_event_bus(bus);
        mgr.register(
            EntanglementLink::new("s-1", "s-2", SyncStrategy::BestEffort).expect("创建失败"),
        )
        .expect("注册失败");

        // sync 应立即返回(spawn 在后台)
        let start = std::time::Instant::now();
        mgr.sync_state_change("s-1", "status:ok")
            .await
            .expect("BestEffort sync 不应失败");
        // spawn 后立即返回,耗时应 < 100ms(允许调度开销)
        assert!(
            start.elapsed() < std::time::Duration::from_millis(100),
            "BestEffort 不应阻塞"
        );

        // 后台任务应最终发布事件(等待最多 1s)
        let event = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .await
            .expect("应收到 BestEffort 事件");

        match event {
            NexusEvent::McpNodeHeartbeat {
                node_id, status, ..
            } => {
                assert_eq!(node_id, "s-1");
                assert!(status.contains("sync:s-1->s-2:status:ok"));
            }
            other => panic!("期望 McpNodeHeartbeat,收到 {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sync_state_change_mixed_strategies() {
        // s-1 与 s-2(Eager) + s-1 与 s-3(Lazy) + s-1 与 s-4(BestEffort)
        // sync 应:Eager 立即发布 + Lazy 入缓冲区 + BestEffort spawn
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let mgr = EntanglementManager::with_event_bus(bus);
        mgr.register(EntanglementLink::new("s-1", "s-2", SyncStrategy::Eager).expect("创建失败"))
            .expect("注册失败");
        mgr.register(EntanglementLink::new("s-1", "s-3", SyncStrategy::Lazy).expect("创建失败"))
            .expect("注册失败");
        mgr.register(
            EntanglementLink::new("s-1", "s-4", SyncStrategy::BestEffort).expect("创建失败"),
        )
        .expect("注册失败");

        mgr.sync_state_change("s-1", "mixed_payload")
            .await
            .expect("混合策略 sync 不应失败");

        // Lazy 应入缓冲区
        assert_eq!(mgr.lazy_buffer_len(), 1, "Lazy 应推入 1 条");

        // Eager + BestEffort 应发布 2 个事件(顺序不确定,因 BestEffort 是 spawn)
        let mut received = 0;
        for _ in 0..2 {
            if rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .await
                .is_ok()
            {
                received += 1;
            }
        }
        assert_eq!(received, 2, "Eager + BestEffort 应发布 2 个事件");

        // flush Lazy
        let published = mgr.flush_lazy_buffer().await;
        assert_eq!(published, 1, "Lazy flush 应发布 1 条");
    }

    #[test]
    fn test_with_event_bus_stores_bus_reference() {
        let bus = EventBus::new();
        let mgr = EntanglementManager::with_event_bus(bus);
        assert!(mgr.event_bus().is_some(), "应存储 EventBus 引用");
    }

    #[test]
    fn test_new_without_event_bus_has_none() {
        let mgr = EntanglementManager::new();
        assert!(mgr.event_bus().is_none(), "new() 不应有 EventBus");
    }
}
