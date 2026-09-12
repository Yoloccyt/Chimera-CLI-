//! 事件桥 — hook.* 双轨注册与触发事件实例工厂（P4-T3③，WI-21/WI-24 联动）
//!
//! 对应架构层: **L9 Quest**（nexus-hook）
//! 对应任务: **P4-T3**（W20 集成周:Phase 3 遗留接线③）
//!
//! # 设计（双轨制，WI-21）
//! - **轨二注册**:14 个 [`LifecycleEvent`] 经 [`HookEventBridge::register_all`]
//!   注册进 event-bus [`DynamicEventRegistry`]（`hook.*` 命名空间,配额 64 内）;
//! - **触发实例**:hook 执行完成后经 [`HookTriggerEvent::new`] 构造触发事件实例
//!   （实现 [`DynamicEvent`]）,消费方经 registry `get` 拉取或直接持有实例;
//! - **144 内置枚举分毫不动**——hook 事件全程走轨二。
//!
//! # 依赖
//! event-bus 的 DynamicEventRegistry 与 nexus-contracts 的 DynamicEvent 契约
//! （nexus-hook 已依赖二者,零新增依赖）。

use nexus_contracts::event_v2::{
    DynamicEvent, EventMetadataV2, EventNamespace, EventTypeId, ImportanceScore,
};
// L1 动态注册表（L0 契约不承载存储,ADR-033 豁免圈 L1 实现）
use event_bus::DynamicEventRegistry;

use crate::audit::HookAuditEntry;
use crate::lifecycle::LifecycleEvent;

/// hook.* 触发事件实例 — 实现 DynamicEvent（轨二）
#[derive(Debug, Clone)]
pub struct HookTriggerEvent {
    type_id: EventTypeId,
    meta: EventMetadataV2,
    payload_json: String,
    importance: ImportanceScore,
}

impl HookTriggerEvent {
    /// 从审计条目构造触发事件（序列化 = JSON 载荷）
    #[must_use]
    pub fn from_audit(session_id: &str, entry: &HookAuditEntry) -> Self {
        let type_id = EventTypeId::new(format!("hook.{}", entry.event.toml_section()));
        let payload_json = serde_json::json!({
            "event": entry.event.toml_section(),
            "command": entry.command,
            "exit_code": entry.exit_code,
            "duration_ms": entry.duration_ms,
            "interrupted": entry.interrupted,
            "sandbox_denied": entry.sandbox_denied,
        })
        .to_string();
        Self {
            type_id,
            meta: EventMetadataV2::new(session_id),
            payload_json,
            // 沙箱拒绝/中断为高重要性（残留/压缩决策输入）
            importance: ImportanceScore::new(if entry.sandbox_denied || entry.interrupted {
                0.9
            } else {
                0.4
            }),
        }
    }
}

impl DynamicEvent for HookTriggerEvent {
    fn event_type(&self) -> EventTypeId {
        self.type_id.clone()
    }
    fn namespace(&self) -> EventNamespace {
        EventNamespace::Hook
    }
    fn serialize(&self) -> Result<Vec<u8>, String> {
        // payload_json 已是 JSON 文本,直接转字节（禁止二次 serde 序列化,否则变成字符串字面量）
        Ok(self.payload_json.clone().into_bytes())
    }
    fn metadata(&self) -> &EventMetadataV2 {
        &self.meta
    }
    fn importance(&self) -> ImportanceScore {
        self.importance
    }
    fn extract_symbols(&self) -> Vec<Box<str>> {
        vec![self.type_id.as_str().into()]
    }
}

/// Hook 事件桥 — 双轨注册 + 触发实例工厂
#[derive(Debug, Default)]
pub struct HookEventBridge;

impl HookEventBridge {
    /// 注册全部 14 个 hook 生命周期事件类型到双轨注册表
    ///
    /// 幂等（同 ID 覆盖为 Replaced）;`hook.*` 命名空间 14 ≤ 配额 64。
    pub fn register_all(registry: &DynamicEventRegistry) {
        for event in LifecycleEvent::ALL {
            let trigger = HookTriggerEvent {
                type_id: EventTypeId::new(format!("hook.{}", event.toml_section())),
                meta: EventMetadataV2::new("hook-bridge"),
                payload_json: "{}".to_string(),
                importance: ImportanceScore::new(0.4),
            };
            // 同 ID 重复注册为 Replaced（幂等）
            let _ = registry.register(std::sync::Arc::new(trigger));
        }
    }

    /// 构造触发事件实例（从审计条目）
    #[must_use]
    pub fn trigger_from_audit(session_id: &str, entry: &HookAuditEntry) -> HookTriggerEvent {
        HookTriggerEvent::from_audit(session_id, entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{make_entry, HookAuditEntry};
    use std::sync::Arc;
    use std::time::Duration;

    fn entry(cmd: &str, interrupted: bool, denied: bool) -> HookAuditEntry {
        make_entry(
            LifecycleEvent::PreToolUse,
            cmd,
            Some(0),
            Duration::from_millis(5),
            interrupted,
            denied,
        )
    }

    /// 注册全部 — 14 个 hook.* 事件进入命名空间（WI-21 双轨）
    #[test]
    fn register_all_fourteen() {
        let registry = DynamicEventRegistry::new();
        HookEventBridge::register_all(&registry);
        let hooks = registry.list_by_namespace(EventNamespace::Hook);
        assert_eq!(hooks.len(), 14, "14 个生命周期事件全部注册");
        assert!(registry
            .get(&EventTypeId::new("hook.pre_tool_use"))
            .is_some());
        assert!(registry.get(&EventTypeId::new("hook.stop")).is_some());
    }

    /// 重复注册幂等 — Replaced 不增计数
    #[test]
    fn register_all_idempotent() {
        let registry = DynamicEventRegistry::new();
        HookEventBridge::register_all(&registry);
        HookEventBridge::register_all(&registry);
        assert_eq!(registry.list_by_namespace(EventNamespace::Hook).len(), 14);
    }

    /// 触发实例 — 从审计条目构造,序列化含载荷字段
    #[test]
    fn trigger_from_audit_serializable() {
        let e = HookEventBridge::trigger_from_audit("sess-1", &entry("git stash", false, false));
        assert_eq!(e.event_type().as_str(), "hook.pre_tool_use");
        let bytes = e.serialize().expect("序列化成功");
        let text = String::from_utf8(bytes).expect("JSON 合法");
        assert!(text.contains("git stash"));
        assert!(text.contains("pre_tool_use"));
        // 中断事件重要性提升
        let e2 = HookEventBridge::trigger_from_audit("s", &entry("x", true, false));
        assert!(e2.importance().value() > 0.5, "中断事件重要性 ≥0.9");
    }

    /// Arc<dyn DynamicEvent> 经注册表查询 — get 返回可序列化实例
    #[test]
    fn registry_get_returns_trigger() {
        let registry = DynamicEventRegistry::new();
        HookEventBridge::register_all(&registry);
        let e = HookEventBridge::trigger_from_audit("s1", &entry("git stash", false, false));
        registry.register(Arc::new(e.clone()));
        let fetched = registry.get(&e.event_type()).expect("注册后可查");
        let bytes = fetched.serialize().expect("序列化成功");
        assert!(!bytes.is_empty());
    }

    /// 全路径 — 审计条目 → 触发实例 → 注册表（接线语义）
    #[test]
    fn full_bridge_path() {
        let registry = DynamicEventRegistry::new();
        HookEventBridge::register_all(&registry);
        // 模拟 hook 执行产生的两条审计
        let e1 = HookEventBridge::trigger_from_audit("s1", &entry("git stash", false, false));
        let e2 = HookEventBridge::trigger_from_audit("s1", &entry("cmd /c pwn", false, true));
        registry.register(Arc::new(e1));
        registry.register(Arc::new(e2));
        // pre_tool_use 同 ID 覆盖（后写胜出）
        let fetched = registry
            .get(&EventTypeId::new("hook.pre_tool_use"))
            .expect("可查");
        let text = String::from_utf8(fetched.serialize().unwrap()).expect("JSON");
        assert!(text.contains("pwn"), "后写实例胜出: {text}");
        assert!(text.contains("sandbox_denied\":true"));
    }
}
