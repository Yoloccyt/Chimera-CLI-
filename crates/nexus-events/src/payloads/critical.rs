//! Critical 通道指标载荷
//!
//! WHY 独立成模块：本组类型只服务于 `Critical 通道指标载荷` 一个领域，
//! 与其它域的载荷无耦合（同文件平铺时改任一领域都要翻找全部类型）。
//!
//! 对外路径不变：全部类型经 `payloads/mod.rs` 的 facade 转出，
//! `event_bus::payloads::X` 逐字不变（ADR-314 W6 / BC-03 域下沉）。
//!
//! 类型：`CriticalEventDropped`（Critical 有界旁路满载丢弃计数快照）。

use serde::{Deserialize, Serialize};

/// Critical 通道丢弃事件指标载荷 — P1-W2.1(D3 改造)
///
/// 当 Critical 通道(有界 mpsc::Sender<4096>)因消费者跟不上而满载时,
/// `publish_critical` 内部 `try_send` 失败会丢弃事件并递增计数。此结构体
/// 作为指标载荷,供 efficiency-monitor 拉取并发布 `EfficiencyAlertTriggered`
/// 告警,同时供 TUI 显示丢弃计数(spec.md L188:丢弃事件计入
/// CriticalEventDropped 指标 + TUI 告警)。
///
/// # 设计约束
/// - 轻量级(u64 单字段),实现 Copy 以便廉价传递
/// - 不持有 Mutex(计数由 EventBus 内部 Arc<AtomicU64> 维护,
///   此结构体仅为快照,避免 §4.4 红线 1 "持锁跨 await")
/// - 实现 Serialize/Deserialize 以支持跨进程指标传递(MCP Mesh)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CriticalEventDropped {
    /// 累计丢弃的 Critical 事件数(单调递增,不重置)
    dropped_count: u64,
}

impl CriticalEventDropped {
    /// 创建指标载荷快照
    ///
    /// `count` 通常来自 `EventBus::critical_dropped_count()` 的当前快照值。
    pub fn new(dropped_count: u64) -> Self {
        Self { dropped_count }
    }

    /// 获取累计丢弃的 Critical 事件数
    pub fn dropped_count(&self) -> u64 {
        self.dropped_count
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn dropped_count_roundtrip_at_u64_boundaries() {
        for count in [0_u64, 1, u64::MAX] {
            let value = CriticalEventDropped::new(count);
            assert_eq!(value.dropped_count(), count);
            let json = serde_json::to_string(&value).unwrap();
            let back: CriticalEventDropped = serde_json::from_str(&json).unwrap();
            assert_eq!(back, value);
            assert_eq!(back.dropped_count(), count);
        }
    }
}
