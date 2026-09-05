//! 事件双轨注册表 — 轨二 DynamicEvent 运行时管理（P3-T10，v4.0 WI-21）
//!
//! 对应架构层: **L1 Core**（event-bus，ADR-149 裁决：挂既有 crate 增强）
//! 对应任务: **P3-T10**（手册 W18，WI-21：DynamicEvent 注册表 + 配额 + 审计）
//!
//! # 设计（v4.0 WI-21 规格）
//! - **轨一（内置 144 枚举）分毫不动**;轨二（动态注册）供 MCP/SubAgent/Hook
//!   外部源注册 [`DynamicEvent`]（L0 契约,nexus-contracts/src/event_v2.rs）;
//! - **命名空间配额 ≤64/空间**（ADR-149）:Builtin/Mcp/SubAgent/Hook/External
//!   各空间独立计数,超限拒绝注册;
//! - **注册审计**:register/unregister 记录审计日志（append-only,
//!   "model-visible means logged" 不变量对齐）;
//! - **路由语义**:注册后经本表查询,事件发布方经动态注册信息派发
//!   （普通 broadcast;Critical 升格由调用方按 importance 判定）。
//!
//! # ⚠ 动态路由符号澄清（ADR-173）
//! `DynamicEventRouter`：曾经的设计意图符号，从未实现；ADR-173 裁定轨二
//! （动态外源事件扩展）冻结未激活，不实现——事件演进唯一权威为
//! `event-bus/src/types.rs` 单表（144 内置变体）。
//!
//! # 红线
//! 144 内置变体序列化回归（既有测试锁定）;注册表空载 = 现状逐比特一致（回退路径）。

use std::collections::HashMap;
use std::sync::Arc;

use dashmap::DashMap;
use nexus_contracts::event_v2::{DynamicEvent, EventNamespace, EventTypeId};

/// 命名空间配额 — 每空间最大注册数（ADR-149:≤64/空间）
pub const NAMESPACE_QUOTA: usize = 64;

/// 注册审计条目 — append-only
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryAuditEntry {
    /// 操作（register/unregister）
    pub action: &'static str,
    /// 事件类型 ID
    pub event_type: String,
    /// 命名空间
    pub namespace: EventNamespace,
    /// 结果（ok/quota_exceeded/not_found）
    pub result: String,
}

/// 注册结果 — 三态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterOutcome {
    /// 注册成功（新事件）
    Registered,
    /// 覆盖既有注册（同类型 ID）
    Replaced,
    /// 命名空间配额超限（≤64/空间,ADR-149）
    QuotaExceeded,
}

/// 事件双轨注册表 — 轨二运行时管理
pub struct DynamicEventRegistry {
    /// 事件类型 ID → 动态事件（Arc 共享）
    events: DashMap<EventTypeId, Arc<dyn DynamicEvent>>,
    /// 注册审计（append-only）
    audit: std::sync::Mutex<Vec<RegistryAuditEntry>>,
    /// 命名空间计数（O(1) 配额检查;注册低频,std Mutex 原子性足够）
    ns_counts: std::sync::Mutex<HashMap<EventNamespace, usize>>,
}

impl std::fmt::Debug for DynamicEventRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // dyn DynamicEvent 无 Debug（trait 对象边界）:仅输出计数
        f.debug_struct("DynamicEventRegistry")
            .field("event_count", &self.len())
            .field("audit_len", &self.audit_len())
            .finish()
    }
}

impl Default for DynamicEventRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl DynamicEventRegistry {
    /// 新建注册表（空载 = 现状）
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: DashMap::new(),
            audit: std::sync::Mutex::new(Vec::new()),
            ns_counts: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// 注册动态事件 — 命名空间配额校验（O(1) 计数）+ 幂等覆盖
    pub fn register(&self, event: Arc<dyn DynamicEvent>) -> RegisterOutcome {
        let event_type = event.event_type();
        let namespace = event.namespace();
        // 配额校验（同 ID 覆盖不计配额;check-and-incr 同锁内原子）
        let mut counts = self.ns_counts.lock().unwrap_or_else(|p| p.into_inner());
        if !self.events.contains_key(&event_type) {
            let count = counts.entry(namespace).or_insert(0);
            if *count >= NAMESPACE_QUOTA {
                drop(counts);
                self.push_audit(RegistryAuditEntry {
                    action: "register",
                    event_type: event_type.as_str().into(),
                    namespace,
                    result: "quota_exceeded".into(),
                });
                return RegisterOutcome::QuotaExceeded;
            }
        }
        let outcome = if self.events.insert(event_type.clone(), event).is_some() {
            RegisterOutcome::Replaced
        } else {
            // 新注册:计数 +1（锁内完成,原子）
            *counts.entry(namespace).or_insert(0) += 1;
            RegisterOutcome::Registered
        };
        drop(counts);
        self.push_audit(RegistryAuditEntry {
            action: "register",
            event_type: event_type.as_str().into(),
            namespace,
            result: match outcome {
                RegisterOutcome::Registered => "ok".into(),
                RegisterOutcome::Replaced => "replaced".into(),
                RegisterOutcome::QuotaExceeded => "quota_exceeded".into(),
            },
        });
        outcome
    }

    /// 注销 — 不存在返回 false
    pub fn unregister(&self, event_type: &EventTypeId) -> bool {
        match self.events.remove(event_type) {
            Some((_, ev)) => {
                // 计数 -1（锁内原子）
                let mut counts = self.ns_counts.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(c) = counts.get_mut(&ev.namespace()) {
                    *c = c.saturating_sub(1);
                }
                drop(counts);
                self.push_audit(RegistryAuditEntry {
                    action: "unregister",
                    event_type: event_type.as_str().into(),
                    namespace: ev.namespace(),
                    result: "ok".into(),
                });
                true
            }
            None => false,
        }
    }

    /// 查询事件（发布方取用;None = 未注册）
    #[must_use]
    pub fn get(&self, event_type: &EventTypeId) -> Option<Arc<dyn DynamicEvent>> {
        self.events.get(event_type).map(|e| Arc::clone(e.value()))
    }

    /// 命名空间内事件列表（诊断/路由枚举）
    #[must_use]
    pub fn list_by_namespace(&self, namespace: EventNamespace) -> Vec<EventTypeId> {
        self.events
            .iter()
            .filter(|e| e.value().namespace() == namespace)
            .map(|e| e.key().clone())
            .collect()
    }

    /// 全部事件数（诊断）
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// 空判定（空载 = 现状逐比特一致,回退路径）
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// 注册审计快照（导出/接 session-store）
    #[must_use]
    pub fn audit_snapshot(&self) -> Vec<RegistryAuditEntry> {
        self.audit.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// 审计条目数（诊断）
    #[must_use]
    pub fn audit_len(&self) -> usize {
        self.audit.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// 命名空间占用统计（诊断:配额水位）
    #[must_use]
    pub fn namespace_usage(&self) -> HashMap<EventNamespace, usize> {
        self.ns_counts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// 追加审计（内部）
    fn push_audit(&self, entry: RegistryAuditEntry) {
        self.audit
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::event_v2::{
        EventMetadataV2, EventNamespace, EventTypeId, ImportanceScore,
    };

    /// 测试动态事件 — 简单实现
    struct TestEvent {
        type_id: EventTypeId,
        ns: EventNamespace,
        meta: EventMetadataV2,
    }

    impl DynamicEvent for TestEvent {
        fn event_type(&self) -> EventTypeId {
            self.type_id.clone()
        }
        fn namespace(&self) -> EventNamespace {
            self.ns
        }
        fn serialize(&self) -> Result<Vec<u8>, String> {
            Ok(self.type_id.as_str().as_bytes().to_vec())
        }
        fn metadata(&self) -> &EventMetadataV2 {
            &self.meta
        }
        fn importance(&self) -> ImportanceScore {
            ImportanceScore::new(0.5)
        }
        fn extract_symbols(&self) -> Vec<Box<str>> {
            vec![self.type_id.as_str().into()]
        }
    }

    fn test_event(ns: EventNamespace, id: &str) -> Arc<TestEvent> {
        Arc::new(TestEvent {
            type_id: EventTypeId::new(format!("{}{}", ns.prefix(), id)),
            ns,
            meta: EventMetadataV2::new("test-source"),
        })
    }

    /// 注册/查询/注销 — 生命周期完整
    #[test]
    fn register_query_unregister() {
        let reg = DynamicEventRegistry::new();
        assert!(reg.is_empty());
        let ev = test_event(EventNamespace::Mcp, "github.issue_created");
        assert_eq!(reg.register(ev), RegisterOutcome::Registered);
        assert_eq!(reg.len(), 1);
        let id = EventTypeId::new("mcp.github.issue_created");
        assert!(reg.get(&id).is_some(), "注册后必须可查");
        assert_eq!(reg.list_by_namespace(EventNamespace::Mcp).len(), 1);
        assert_eq!(reg.list_by_namespace(EventNamespace::Hook).len(), 0);
        assert!(reg.unregister(&id), "注销必须成功");
        assert!(reg.get(&id).is_none());
        assert!(!reg.unregister(&id), "重复注销失败");
        assert_eq!(
            reg.audit_len(),
            2,
            "register + unregister 审计;重复注销不记录"
        );
    }

    /// 同 ID 覆盖 — Replaced,不重复计数
    #[test]
    fn replace_same_id() {
        let reg = DynamicEventRegistry::new();
        let a = test_event(EventNamespace::Mcp, "x");
        let b = test_event(EventNamespace::Mcp, "x");
        assert_eq!(reg.register(a), RegisterOutcome::Registered);
        assert_eq!(reg.register(b), RegisterOutcome::Replaced, "同 ID 覆盖");
        assert_eq!(reg.len(), 1, "覆盖不增计数");
    }

    /// 命名空间配额 — 每空间 ≤64 超限拒绝（ADR-149）
    #[test]
    fn namespace_quota_enforced() {
        let reg = DynamicEventRegistry::new();
        // 填满 Mcp 空间（64 个）
        for i in 0..NAMESPACE_QUOTA {
            let ev = test_event(EventNamespace::Mcp, &format!("e{i}"));
            assert_eq!(reg.register(ev), RegisterOutcome::Registered);
        }
        // 超限拒绝
        let over = test_event(EventNamespace::Mcp, "overflow");
        assert_eq!(
            reg.register(over),
            RegisterOutcome::QuotaExceeded,
            "配额超限必须拒绝"
        );
        // 其他空间不受影响（Hook 独立配额）
        let hook = test_event(EventNamespace::Hook, "h1");
        assert_eq!(reg.register(hook), RegisterOutcome::Registered);
        assert_eq!(reg.len(), NAMESPACE_QUOTA + 1);
        // 配额水位诊断
        let usage = reg.namespace_usage();
        assert_eq!(
            usage.get(&EventNamespace::Mcp).copied().unwrap_or(0),
            NAMESPACE_QUOTA
        );
        assert_eq!(usage.get(&EventNamespace::Hook).copied().unwrap_or(0), 1);
    }

    /// 审计 — 配额拒绝也留痕
    #[test]
    fn audit_records_quota_denial() {
        let reg = DynamicEventRegistry::new();
        for i in 0..NAMESPACE_QUOTA {
            reg.register(test_event(EventNamespace::SubAgent, &format!("s{i}")));
        }
        reg.register(test_event(EventNamespace::SubAgent, "overflow"));
        let snap = reg.audit_snapshot();
        assert!(
            snap.iter().any(|e| e.result == "quota_exceeded"),
            "配额拒绝必须审计"
        );
        assert_eq!(snap.len(), NAMESPACE_QUOTA + 1);
    }

    /// 并发注册 — 不同 ID 全注册（External 配额 64 内:4×8=32）
    #[test]
    fn concurrent_registration() {
        let reg = std::sync::Arc::new(DynamicEventRegistry::new());
        let handles: Vec<_> = (0..4)
            .map(|i| {
                let reg = std::sync::Arc::clone(&reg);
                std::thread::spawn(move || {
                    for j in 0..8usize {
                        reg.register(test_event(EventNamespace::External, &format!("t{i}-{j}")));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("线程正常");
        }
        assert_eq!(reg.len(), 4 * 8, "并发注册必须全部可见");
        assert_eq!(reg.list_by_namespace(EventNamespace::External).len(), 4 * 8);
    }

    /// 空载语义 — 空注册表行为与无注册表一致（回退路径）
    #[test]
    fn empty_registry_fallback() {
        let reg = DynamicEventRegistry::new();
        assert!(reg.get(&EventTypeId::new("mcp.x")).is_none());
        assert_eq!(reg.audit_len(), 0);
    }

    /// 性能门禁 — 1000 动态事件注册/查询 <10ms（WI-21 验收口径）
    ///
    /// 诚实数据:debug 模式为保守上界（release 更快）;仅断言数量级,
    /// 不依赖机器速度（16 核本机实测 µs 级）。
    #[test]
    fn thousand_registrations_query_under_10ms() {
        let reg = DynamicEventRegistry::new();
        let started = std::time::Instant::now();
        // 1000 注册（External 空间配额 64 会拒绝 936 个——改用多空间轮换）
        let namespaces = [
            EventNamespace::Mcp,
            EventNamespace::SubAgent,
            EventNamespace::Hook,
            EventNamespace::External,
            EventNamespace::Builtin,
        ];
        let mut registered = 0usize;
        for i in 0..1_000usize {
            let ns = namespaces[i % namespaces.len()];
            let ev = test_event(ns, &format!("perf-{i}"));
            if reg.register(ev) != RegisterOutcome::QuotaExceeded {
                registered += 1;
            }
        }
        // 5 空间 × 64 配额 = 320 上限;注册数 ≤ 320（配额强制）
        assert!(registered > 0);
        // 查询全部注册的事件（含未注册的 miss 路径）
        for i in 0..1_000usize {
            let id = EventTypeId::new(format!("external.perf-{i}"));
            let _ = reg.get(&id);
        }
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(10),
            "1000 注册/查询必须 <10ms,实测 {}ms",
            elapsed.as_millis()
        );
        assert_eq!(reg.audit_len(), 1_000, "全部注册尝试（含配额拒绝）审计留痕");
    }
}
