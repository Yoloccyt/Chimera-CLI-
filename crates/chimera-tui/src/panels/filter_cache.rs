//! FilterCache — 事件过滤结果缓存(EventStream/Log 共享,M4 v1)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **键 = 事件集身份 + 三过滤器**:事件集身份 = `latest_events` 的 Arc 指针地址。
//!   (PF-01,2026-09-06 评估)生产管道对事件集采用「整体替换」(pipeline.rs
//!   `latest_events = Arc::new(deque)`),内容变化必然产生新分配 → 指针身份与
//!   内容一一对应;以指针身份替代 revision 计数,「无新事件但 revision 每 tick
//!   递增」的恒定 O(n) 重扫被消除(事件不变时缓存跨 tick 命中零重建)。
//! - **保留 `revision != 0` 守卫**:测试桩(`TuiState::new()` revision==0)可能经
//!   `Arc::make_mut` 就地突变容器(指针不变、内容变),此时不能依赖指针身份,
//!   缓存禁用(仅测试路径;生产管道不做就地突变)。
//! - **存索引而非事件引用**:latest_events 每 tick 整体替换,索引在跨帧缓存
//!   期间仍按下标访问新容器(引用会悬垂);索引为 latest_events 正序下标。
//! - **仅 production 启用**:`revision == 0`(测试桩)时 latest_events 可能被
//!   就地修改,缓存会造成陈旧结果(P-B 起不再有 keyword 门槛:缓存键已含
//!   keyword/topic/level 三元组,仅 topic/level 过滤的每帧 O(n) 谓词遍历
//!   同样值得缓存,语义与 keyword 路径一致)。

use crate::types::TuiState;

/// 事件过滤结果缓存(键 = 事件集 Arc 身份 + keyword/topic/level,值 = 正序索引)
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct FilterCache {
    /// 事件集身份(`latest_events` Arc 指针地址;内容整体替换时变化)
    events_id: usize,
    /// 关键字过滤器(缓存键)
    keyword: Option<String>,
    /// 主题过滤器(缓存键)
    topic: Option<String>,
    /// 级别过滤器(缓存键)
    level: Option<String>,
    /// 过滤后事件在 `latest_events`(正序)中的索引
    indices: Vec<usize>,
}

impl FilterCache {
    /// 缓存是否应启用(production 快照)
    ///
    /// WHY 移除 keyword 门槛(P-B,评估报告 v2):缓存键已含 keyword/topic/level
    /// 三元组,仅 topic/level 过滤时同样存在每帧 O(n) 谓词遍历,应一并缓存;
    /// 仅保留 `revision != 0` 守卫(测试桩事件流可能被就地修改,缓存会陈旧)。
    pub(crate) fn enabled(state: &TuiState) -> bool {
        state.last_snapshot_revision != 0
    }

    /// 事件集身份:latest_events Arc 指针地址
    ///
    /// WHY 指针身份 = 内容身份:生产管道事件集整体替换(内容变必然换 Arc),
    /// 见模块文档 PF-01 决策;测试桩就地突变由 `enabled()` 守卫拦截。
    fn events_id(state: &TuiState) -> usize {
        std::sync::Arc::as_ptr(&state.latest_events) as usize
    }

    /// 缓存键是否与当前状态一致(命中)
    pub(crate) fn matches(&self, state: &TuiState) -> bool {
        self.events_id == Self::events_id(state)
            && self.keyword == state.filter_keyword
            && self.topic == state.filter_topic
            && self.level == state.filter_level
    }

    /// 缓存索引切片(供调用方映射回事件引用)
    pub(crate) fn indices(&self) -> &[usize] {
        &self.indices
    }

    /// 以当前状态键 + 新索引刷新缓存
    pub(crate) fn update(&mut self, state: &TuiState, indices: Vec<usize>) {
        self.events_id = Self::events_id(state);
        self.keyword = state.filter_keyword.clone();
        self.topic = state.filter_topic.clone();
        self.level = state.filter_level.clone();
        self.indices = indices;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::{EventMetadata, NexusEvent};
    use std::collections::VecDeque;
    use std::sync::Arc;

    /// 构造生产语义测试状态:revision != 0 + 事件集 Arc(内容变化 = 换 Arc)
    fn state_with(events: Arc<VecDeque<NexusEvent>>, keyword: Option<&str>) -> TuiState {
        let mut state = TuiState::new();
        state.last_snapshot_revision = 1;
        state.filter_keyword = keyword.map(str::to_string);
        state.latest_events = events;
        state
    }

    fn events_arc(keys: &[&str]) -> Arc<VecDeque<NexusEvent>> {
        Arc::new(
            keys.iter()
                .map(|k| NexusEvent::CacheHit {
                    metadata: EventMetadata::new("scc-cache"),
                    cache_key: (*k).into(),
                })
                .collect(),
        )
    }

    fn hit_key(event: &NexusEvent) -> &str {
        match event {
            NexusEvent::CacheHit { cache_key, .. } => cache_key.as_str(),
            other => panic!("expected CacheHit, got {other:?}"),
        }
    }

    /// PF-01:revision 每 tick 递增但事件集不变 → 缓存必须命中(零重建)
    #[test]
    fn keyed_on_events_identity_not_revision() {
        let events = events_arc(&["alpha-1", "beta-2", "alpha-3"]);
        let state = state_with(Arc::clone(&events), Some("alpha"));

        let mut cache = FilterCache::default();
        let indices = vec![2, 0]; // alpha-3, alpha-1(最新在前)
        cache.update(&state, indices.clone());

        // 模拟生产 tick:revision 递增(revision != 0 守卫仍满足)但事件 Arc 不变
        let mut same_events = state;
        same_events.last_snapshot_revision = 2;
        assert!(
            cache.matches(&same_events),
            "事件集身份未变时缓存应命中(revision 递增不得触发重建)"
        );
        assert_eq!(cache.indices(), indices.as_slice());
    }

    /// PF-01:事件集内容变化(新 Arc)→ 缓存失效重建
    #[test]
    fn invalidates_on_event_set_replacement() {
        let state = state_with(events_arc(&["alpha-1", "beta-2"]), Some("alpha"));
        let mut cache = FilterCache::default();
        cache.update(&state, vec![0]);
        assert!(cache.matches(&state));

        // 模拟管道整体替换事件集(新 Arc = 新身份)
        let changed = state_with(events_arc(&["alpha-9", "alpha-8"]), Some("alpha"));
        assert!(!cache.matches(&changed), "事件集整体替换后缓存应失效");
    }

    /// 过滤器变化 → 缓存失效(既有语义)
    #[test]
    fn invalidates_on_filter_change() {
        let events = events_arc(&["alpha-1", "beta-2"]);
        let state = state_with(Arc::clone(&events), Some("alpha"));
        let mut cache = FilterCache::default();
        cache.update(&state, vec![0]);
        assert!(cache.matches(&state));

        let mut filtered = state;
        filtered.filter_keyword = Some("beta".into());
        assert!(!cache.matches(&filtered), "过滤器变化应失效");
    }

    /// 事件集身份键下索引指向新容器仍正确(整体替换 + 重新过滤)
    #[test]
    fn indices_map_into_replaced_container() {
        let state = state_with(events_arc(&["alpha-1", "beta-2", "alpha-3"]), Some("alpha"));
        let mut cache = FilterCache::default();
        cache.update(&state, vec![0, 2]);
        let got: Vec<&str> = cache
            .indices()
            .iter()
            .map(|&i| hit_key(&state.latest_events[i]))
            .collect();
        assert_eq!(got, vec!["alpha-1", "alpha-3"]);
    }
}
