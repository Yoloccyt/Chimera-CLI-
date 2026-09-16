//! 压缩结果 SCC 卸载(ADR-160 孤岛偿还 M10:hcw-window → scc-cache 生产边)
//!
//! 对应架构层:L2 Memory → L3 Storage(向下依赖,§2.2 依赖铁律合法)
//!
//! # 模块职责(WHY 本模块存在)
//! HCW 压缩上下文窗口后,压缩结果是 PVL 层 Producer/Verifier 双通道
//! 的共享输入。SCC(推测上下文缓存)的设计消费场景正是"Draft/Verify
//! 共享 `Arc<ContextEntry>`"(scc-cache lib.rs 自述)。本模块把压缩快照
//! 以调用方提供的 content-key 卸载进 SCC,Producer/Verifier 经
//! `shared()` 拿到同一 `Arc<ContextEntry>`(零拷贝共享,Arc 强引用保护
//! 下不被 LRU 驱逐)。
//!
//! # 装配姿势
//! 构造参数注入(ADR-161 决策 3:禁 feature 门控)。**未装配时 HCW 压缩
//! 路径行为与此前完全一致**(零行为变化):本模块不参与
//! `compress`/`select_window` 既有调用链,由上层编排器显式持有并调用。
//!
//! # 依赖方向
//! hcw-window → scc-cache:L2 → L3 向下依赖,合法(§2.2);跨层事件流
//! (CacheHit/CacheMiss)走 EventBus,不构成向上依赖边。

use std::sync::Arc;

use event_bus::EventBus;
use scc_cache::{ContextEntry, ContextId, SccCache, SccConfig};

/// 压缩窗口卸载器 — HCW 压缩结果的 SCC 侧装配封装
///
/// 持有 `SccCache` 实例,提供压缩快照的卸载(`offload`)与共享读回
/// (`shared`)两个原语。键契约:调用方以上游 content-key(如窗口内容
/// 哈希/会话段 ID)为键,Producer/Verifier 两侧使用同一键即可共享。
pub struct CompressedWindowOffload {
    cache: SccCache,
}

impl CompressedWindowOffload {
    /// 以默认配置创建卸载器(独立 EventBus;生产推荐 `with_cache` 共享总线)
    pub fn new(config: SccConfig, event_bus: EventBus) -> Self {
        Self {
            cache: SccCache::new(config, event_bus),
        }
    }

    /// 以既有缓存实例创建卸载器(生产推荐:PVL 通道共享同一 `SccCache`,
    /// Producer 卸载、Verifier 读回命中同一 Arc)
    pub fn with_cache(cache: SccCache) -> Self {
        Self { cache }
    }

    /// 卸载压缩快照:以 content-key 写入 SCC(已存在则覆盖,幂等)
    ///
    /// 真实调用 SCC `insert`;后续 `shared(key)` 命中即返回同一
    /// `Arc<ContextEntry>`(strong_count > 1 时 LRU 不驱逐该条目)。
    pub fn offload(&self, content_key: &str, compressed_text: impl Into<String>) {
        self.cache
            .insert(ContextEntry::new(content_key, compressed_text.into()));
    }

    /// 共享读回:Producer/Verifier 双通道经同一 content-key 取共享条目
    ///
    /// 真实调用 SCC `get_or_prefetch`;未命中返回 None(调用方回退本地路径)。
    pub fn shared(&self, content_key: &str) -> Option<Arc<ContextEntry>> {
        self.cache.get_or_prefetch(&ContextId::new(content_key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offload_then_shared_returns_same_arc() {
        let bus = EventBus::new();
        let offload = CompressedWindowOffload::new(SccConfig::default(), bus);
        offload.offload("win-1", "compressed text a");

        let first = offload.shared("win-1").expect("卸载后应命中共享条目");
        let second = offload.shared("win-1").expect("二次读回应命中同一 Arc");
        assert!(
            Arc::ptr_eq(&first, &second),
            "同一 content-key 的两次读回应共享同一 Arc(PVL 双通道零拷贝前提)"
        );
        assert_eq!(&*first.content, "compressed text a");
    }

    #[test]
    fn shared_miss_returns_none() {
        let bus = EventBus::new();
        let offload = CompressedWindowOffload::new(SccConfig::default(), bus);
        assert!(
            offload.shared("never-offloaded").is_none(),
            "未卸载的 key 必须回退 None(调用方本地路径)"
        );
    }

    #[test]
    fn reoffload_same_key_is_idempotent() {
        let bus = EventBus::new();
        let offload = CompressedWindowOffload::new(SccConfig::default(), bus);
        offload.offload("win-2", "v1");
        offload.offload("win-2", "v2");
        let entry = offload.shared("win-2").expect("重复卸载后仍应命中");
        assert_eq!(&*entry.content, "v2", "重复卸载覆盖旧值,语义幂等");
    }

    #[test]
    fn different_keys_are_independent() {
        let bus = EventBus::new();
        let offload = CompressedWindowOffload::new(SccConfig::default(), bus);
        offload.offload("win-a", "text-a");
        offload.offload("win-b", "text-b");
        assert_eq!(&*offload.shared("win-a").unwrap().content, "text-a");
        assert_eq!(&*offload.shared("win-b").unwrap().content, "text-b");
    }
}
