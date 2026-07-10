//! Wiki 监控指标模块 — Prometheus 指标暴露
//!
//! 对应架构层:L5 Knowledge
//!
//! 暴露 Prometheus 指标供外部采集(M1 触发条件监控):
//! - `wiki_entries_total`:Gauge,当前 Wiki 条目总数
//!
//! # M1 触发条件
//! M1 向量索引升级的触发条件为:Wiki entries > 1000 且 KNN p95 > 10ms。
//! 本模块暴露 `wiki_entries_total` gauge,为该触发条件提供数据支撑。
//! 运维可通过 Prometheus 查询 `wiki_entries_total > 1000` 监控是否接近阈值。
//!
//! # 类型选择说明
//! `Gauge` 使用默认泛型参数 `Gauge<i64, AtomicI64>`(prometheus-client 0.22 默认),
//! 而非 `f64`。WHY:条目数是整数计数场景,`i64` 足够表达且语义更精确;
//! 默认类型避免显式指定泛型参数,代码更简洁。Prometheus 文本格式对 i64 gauge
//! 编码正确,运维查询 `wiki_entries_total > 1000` 语义无歧义。
//!
//! # 使用方式
//! `WikiStore` 内部持有 `Arc<WikiMetrics>`,insert/delete 成功后自动调用
//! `refresh_metrics` 刷新 gauge。外部通过 `store.metrics().entries_total.get()`
//! 读取当前值,或通过 Prometheus Registry 采集(需上层注册)。

use prometheus_client::metrics::gauge::Gauge;

/// M1 触发条件中的条目数阈值(entries > 1000 时触发向量索引升级评估)
pub const M1_TRIGGER_THRESHOLD: u32 = 1000;

/// 预警阈值:达到触发阈值的 80% 时发出 WARN 日志
///
/// WHY 80%:预留 20% 缓冲期(800 → 1000 = 200 条目增长空间),
/// 让运维团队在触发前有时间规划升级,避免突发触发措手不及。
pub const WARN_THRESHOLD: u32 = 800;

/// Wiki 监控指标集合
///
/// WHY `Arc<WikiMetrics>` 共享:`WikiStore` 实现 `Clone`(共享写线程与读连接池),
/// 多个 clone 实例必须看到一致的指标视图,因此用 `Arc` 共享同一份 `WikiMetrics`。
#[derive(Debug, Default)]
pub struct WikiMetrics {
    /// Wiki 条目总数(Gauge:可增可减,非单调递增)
    ///
    /// WHY Gauge 而非 Counter:Wiki entries 可因 delete 减少,
    /// Counter 只能单调递增,不适用于本场景。Gauge 支持任意 set/inc/dec。
    pub entries_total: Gauge,
}

impl WikiMetrics {
    /// 创建空指标集合(所有指标初始化为默认值)
    ///
    /// 注意:`Gauge::default()` 的 `get()` 返回 `0`(AtomicI64 默认值),
    /// 无需额外初始化即可表示"空 store"状态。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置条目总数,并在接近 M1 触发阈值时发出 WARN 预警
    ///
    /// WHY 预警阈值 800:触发阈值 1000 的 80%,预留评估缓冲期,
    /// 让运维团队在触发前有时间规划升级。
    ///
    /// WHY 每次 set 都检查阈值(而非仅在跨越时):持续提醒运维
    /// entries 已接近危险区,避免单次提醒被日志洪流淹没。
    pub fn set_entries(&self, count: u32) {
        self.entries_total.set(count as i64);
        if count >= WARN_THRESHOLD {
            tracing::warn!(
                count,
                warn_threshold = WARN_THRESHOLD,
                trigger_threshold = M1_TRIGGER_THRESHOLD,
                "Wiki entries approaching M1 vector index upgrade trigger"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_returns_default() {
        let metrics = WikiMetrics::new();
        // 默认 Gauge 值为 0(AtomicI64::default() = 0)
        assert_eq!(metrics.entries_total.get(), 0);
    }

    #[test]
    fn test_set_entries_below_threshold() {
        let metrics = WikiMetrics::new();
        metrics.set_entries(100);
        assert_eq!(metrics.entries_total.get(), 100);
    }

    #[test]
    fn test_set_entries_at_threshold() {
        let metrics = WikiMetrics::new();
        // 恰好 800:达到预警阈值(边界值,>= 800 触发)
        metrics.set_entries(WARN_THRESHOLD);
        assert_eq!(metrics.entries_total.get(), WARN_THRESHOLD as i64);
    }

    #[test]
    fn test_set_entries_decreases() {
        // 验证 Gauge 可减(与 Counter 的关键区别)
        let metrics = WikiMetrics::new();
        metrics.set_entries(500);
        assert_eq!(metrics.entries_total.get(), 500);
        metrics.set_entries(100);
        assert_eq!(metrics.entries_total.get(), 100);
        metrics.set_entries(0);
        assert_eq!(metrics.entries_total.get(), 0);
    }
}
