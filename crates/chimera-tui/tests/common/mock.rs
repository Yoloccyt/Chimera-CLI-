//! 共享 MockDataSource 实现(集成测试替身)
//!
//! (R14 收敛)原 `incremental_render_test.rs` / `panel_state_preservation_test.rs` /
//! `parliament_virtual_scroll_test.rs` / `quest_event_jump_test.rs` 四处同形
//! `MockDataSource` 定义(字段同为 `snapshot + config`、实现同一 `TuiDataSource`
//! trait、方法逐字一致)统一收敛至此,一举消除重复。差异项(#[derive(Debug)] 缺失
//! 3/4、new() 缺失 3/4、import 风格全限定 vs 裸名)以下述为准补齐。

use std::sync::Arc;

use chimera_tui::data::{DataSnapshot, DataSourceConfig, TuiDataSource};
use chimera_tui::error::TuiError;

/// 可编程 mock 数据源 — 每次 `snapshot()` 返回当前内部快照的克隆。
#[derive(Debug)]
pub struct MockDataSource {
    snapshot: DataSnapshot,
    config: DataSourceConfig,
}

impl MockDataSource {
    /// 用给定快照构造 mock,`config` 采用默认值。
    pub fn new(snapshot: DataSnapshot) -> Self {
        Self {
            snapshot,
            config: DataSourceConfig::default(),
        }
    }
}

impl TuiDataSource for MockDataSource {
    fn snapshot(&self) -> Result<Arc<DataSnapshot>, TuiError> {
        Ok(Arc::new(self.snapshot.clone()))
    }

    fn config(&self) -> &DataSourceConfig {
        &self.config
    }
}
