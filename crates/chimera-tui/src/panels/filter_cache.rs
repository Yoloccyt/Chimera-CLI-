//! FilterCache — 事件过滤结果缓存(EventStream/Log 共享,M4 v1)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **键 = 快照 revision + 三过滤器**:revision 由 DataPipeline 每 tick 递增,
//!   覆盖事件流内容变化;过滤器单独成键,覆盖 `:find`/`:filter`/`:level`
//!   命令(不改变 revision)。
//! - **存索引而非事件引用**:latest_events 每 tick 整体替换,索引在跨帧缓存
//!   期间仍按下标访问新容器(引用会悬垂);索引为 latest_events 正序下标。
//! - **仅 production 启用**:`revision == 0`(测试桩)时 latest_events 可能被
//!   就地修改,缓存会造成陈旧结果;无关键字时过滤是廉价模式匹配,无需缓存。

use crate::types::TuiState;

/// 事件过滤结果缓存(键 = revision + keyword/topic/level,值 = 正序索引)
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct FilterCache {
    /// 快照生成号(与 `TuiState.last_snapshot_revision` 对齐)
    revision: u64,
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
    /// 缓存是否应启用(production 快照 + 存在关键字过滤)
    pub(crate) fn enabled(state: &TuiState) -> bool {
        state.last_snapshot_revision != 0 && state.filter_keyword.is_some()
    }

    /// 缓存键是否与当前状态一致(命中)
    pub(crate) fn matches(&self, state: &TuiState) -> bool {
        self.revision == state.last_snapshot_revision
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
        self.revision = state.last_snapshot_revision;
        self.keyword = state.filter_keyword.clone();
        self.topic = state.filter_topic.clone();
        self.level = state.filter_level.clone();
        self.indices = indices;
    }
}
