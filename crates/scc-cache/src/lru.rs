//! LRU 驱逐策略 — 基于逻辑时钟与 Arc 引用保护的 LRU 受害者选择
//!
//! 对应架构层:L3 Storage
//!
//! # 设计决策(WHY)
//! - **逻辑时钟而非墙钟时间**:Windows 系统时钟精度约 15ms,短时间内多次操作
//!   可能产生相同 `last_accessed_at`,导致 LRU 顺序不确定。逻辑时钟(AtomicU64)
//!   严格单调递增,消除墙钟精度依赖(继承 cmt-tiering SubTask 20.2 经验)
//! - **Arc::strong_count 引用保护**:Producer/Verifier 可能持有 `Arc<ContextEntry>`,
//!   strong_count > 1 表示条目正在使用,驱逐会导致其内容丢失。跳过这些条目
//!   保证运行中的操作不受驱逐影响(spec.md 关键设计决策 4)
//! - **O(n) 扫描**:256 条目规模下 O(n) 扫描约 1μs,性能可接受。
//!   不引入 LinkedHashMap 等额外数据结构,保持简洁

use std::sync::Arc;

use dashmap::DashMap;

use crate::types::{ContextEntry, ContextId};

/// 从缓存中选择 LRU 驱逐受害者
///
/// 扫描 `access_order`(逻辑时钟值),找到时钟值最小(最久未访问)的条目,
/// 且该条目的 `Arc::strong_count == 1`(未被外部引用)。
///
/// # 参数
/// - `access_order`:逻辑时钟表(ContextId → 时钟值,越小越久未访问)
/// - `entries`:缓存条目表(ContextId → `Arc<ContextEntry>`)
///
/// # 返回
/// 被选中的受害者 ContextId;若所有条目都在使用中或缓存为空,返回 None
///
/// # 并发安全
/// 此函数只读扫描两个 DashMap,不持有写锁。调用方(`SccCache::insert`)
/// 在 `insert_lock` 临界区内调用此函数,保证"选择 → 驱逐"的原子性。
/// `Arc::strong_count` 检查存在 TOCTOU 竞态(检查后计数可能变化),
/// 但对缓存场景可接受:最坏情况驱逐一个刚被引用的条目(极低概率),
/// 且被引用方仍持有有效 Arc(只是缓存不再持有)
pub fn select_victim(
    access_order: &DashMap<ContextId, u64>,
    entries: &DashMap<ContextId, Arc<ContextEntry>>,
) -> Option<ContextId> {
    let mut victim: Option<(u64, ContextId)> = None;

    for ref_multi in access_order.iter() {
        let id = ref_multi.key();

        // 检查 Arc 引用计数:strong_count > 1 表示被外部引用,跳过
        // WHY:entries 表持有一份 Arc,get_or_prefetch 返回的 Arc 被
        // Producer/Verifier 持有。strong_count == 1 表示仅缓存持有,可安全驱逐
        let strong_count = entries
            .get(id)
            .map(|entry_arc| Arc::strong_count(&entry_arc))
            .unwrap_or(0);

        if strong_count > 1 {
            continue;
        }
        // strong_count == 0 表示 access_order 与 entries 不一致(条目已被移除),跳过
        if strong_count == 0 {
            continue;
        }

        let clock_val = *ref_multi.value();
        match &victim {
            None => victim = Some((clock_val, id.clone())),
            Some((min, _)) if clock_val < *min => {
                victim = Some((clock_val, id.clone()));
            }
            _ => {}
        }
    }

    victim.map(|(_, id)| id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ContextEntry;

    fn make_entry(id: &str) -> Arc<ContextEntry> {
        Arc::new(ContextEntry::new(id, "content"))
    }

    #[test]
    fn test_select_victim_empty() {
        let access_order = DashMap::new();
        let entries = DashMap::new();
        assert!(select_victim(&access_order, &entries).is_none());
    }

    #[test]
    fn test_select_victim_oldest() {
        let access_order = DashMap::new();
        let entries = DashMap::new();

        // 插入 3 个条目,逻辑时钟值分别为 0, 1, 2
        let id_a = ContextId::new("ctx-a");
        let id_b = ContextId::new("ctx-b");
        let id_c = ContextId::new("ctx-c");

        entries.insert(id_a.clone(), make_entry("ctx-a"));
        entries.insert(id_b.clone(), make_entry("ctx-b"));
        entries.insert(id_c.clone(), make_entry("ctx-c"));

        access_order.insert(id_a.clone(), 0);
        access_order.insert(id_b.clone(), 1);
        access_order.insert(id_c.clone(), 2);

        // 最久未访问的是 ctx-a(时钟值 0)
        let victim = select_victim(&access_order, &entries);
        assert_eq!(victim.as_ref().map(|id| id.as_str()), Some("ctx-a"));
    }

    #[test]
    fn test_select_victim_skip_arc_referenced() {
        let access_order = DashMap::new();
        let entries = DashMap::new();

        let id_a = ContextId::new("ctx-a");
        let id_b = ContextId::new("ctx-b");

        let entry_a = make_entry("ctx-a");
        let entry_b = make_entry("ctx-b");

        // 模拟外部引用:clone entry_a 使 strong_count = 2
        let _external_ref = Arc::clone(&entry_a);

        entries.insert(id_a.clone(), entry_a);
        entries.insert(id_b.clone(), entry_b);

        // ctx-a 时钟值更小(更久未访问),但被外部引用
        access_order.insert(id_a.clone(), 0);
        access_order.insert(id_b.clone(), 1);

        // 应跳过 ctx-a,选择 ctx-b
        let victim = select_victim(&access_order, &entries);
        assert_eq!(victim.as_ref().map(|id| id.as_str()), Some("ctx-b"));
    }

    #[test]
    fn test_select_victim_all_referenced_returns_none() {
        let access_order = DashMap::new();
        let entries = DashMap::new();

        let id_a = ContextId::new("ctx-a");
        let entry_a = make_entry("ctx-a");
        let _external_ref = Arc::clone(&entry_a);

        entries.insert(id_a.clone(), entry_a);
        access_order.insert(id_a.clone(), 0);

        // 所有条目都被外部引用,无法驱逐
        let victim = select_victim(&access_order, &entries);
        assert!(victim.is_none());
    }
}
