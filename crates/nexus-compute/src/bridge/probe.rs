//! 观测面方法簇 — 池/饱和/阈值/计数器的只读探针与测试注入缝。
//!
//! 从 `bridge` 单文件按方法簇拆出:本文件只承载可观测面,
//! 不改任何判定逻辑（方法与字段访问逐字平移）。

use std::sync::atomic::Ordering;
#[cfg(test)]
use std::sync::Arc;

use super::ComputeBridge;
use crate::dispatch::TaskKind;
use crate::hts::ThresholdSource;
use crate::telemetry::{self, KindCounters, SaturationMode};
#[cfg(test)]
use crate::utilization::ActiveCounter;
use crate::utilization::RayonProbe;

impl ComputeBridge {
    /// WI-34 rayon 探针 — 池活跃任务数（瞬时,Relaxed 读）
    #[must_use]
    pub fn pool_active_tasks(&self) -> usize {
        self.active.active()
    }

    /// WI-34 rayon 探针 — 池线程数（构造时快照）
    #[must_use]
    pub fn pool_threads(&self) -> usize {
        self.thread_count
    }

    /// A1/ADR-197:累计因饱和而降级为 Inline 的卸载次数（**仅 Degrade 策略下会增长**）
    #[must_use]
    pub fn saturation_degradations(&self) -> u64 {
        self.counters.degradations()
    }

    /// 第三轮 T2.2:累计降级为 Inline 的**项数**（项粒度，与 `saturation_degradations`
    /// 的批粒度口径不同；一批降级 n 项则本计数 +n、`saturation_degradations` +1）
    #[must_use]
    pub fn saturation_degraded_items(&self) -> u64 {
        self.counters.degraded_items_total()
    }

    /// A1/ADR-197:累计超阈观测次数（三种策略均增长，上报器去抖基线）
    #[must_use]
    pub fn saturation_observations(&self) -> u64 {
        self.counters.saturations()
    }

    /// A1/ADR-197:占用最高的任务类型名（事件归因字段）
    #[must_use]
    pub fn saturation_hottest_kind(&self) -> String {
        self.counters.hottest_kind()
    }

    /// A1/ADR-197:按类型计数快照（供指标导出与测试断言）
    #[must_use]
    pub fn kind_counters(&self) -> &KindCounters {
        &self.counters
    }

    /// B1/ADR-198:当前生效阈值（校准候选计算与测试断言用）
    #[must_use]
    pub fn threshold_of(&self, kind: TaskKind) -> usize {
        self.hts.load().get(kind).threshold
    }

    /// B1/ADR-198:当前生效 chunk（写表时沿用现值,本批不主张 chunk 语义）
    #[must_use]
    pub fn chunk_of(&self, kind: TaskKind) -> usize {
        self.hts.load().get(kind).chunk
    }

    /// B1/ADR-198:阈值来源描述串（可审计面:预填/校准/兜底三态必可区分）
    ///
    /// WHY 返回字符串而非 `ThresholdSource`:本方法只服务可观测面与测试断言,
    /// 将枚举逐变异展开为串可避免在公API 里泄露 `hts` 内部类型。
    #[must_use]
    pub fn threshold_source_of(&self, kind: TaskKind) -> String {
        match self.hts.load().get(kind).source {
            ThresholdSource::OfflineMeasured { measured_at } => {
                format!("OfflineMeasured{{measured_at:{measured_at}}}")
            }
            ThresholdSource::SequentialTest {
                promoted_at,
                samples,
            } => {
                format!("SequentialTest{{promoted_at:{promoted_at},samples:{samples}}}")
            }
            ThresholdSource::ConservativeDefault => "ConservativeDefault".to_string(),
        }
    }

    /// 当前占用是否已达饱和阈值（公开只读判定,供上层与测试使用）
    #[must_use]
    pub fn is_saturated(&self) -> bool {
        telemetry::saturation_ratio_bp(self.active.active(), self.thread_count)
            >= self.saturation_threshold_bp()
    }

    /// 生效的饱和阈值千分比（实例覆盖优先,否则 env/默认）
    #[must_use]
    pub fn saturation_threshold_bp(&self) -> u32 {
        let override_bp = self.sat_bp_override.load(Ordering::Relaxed);
        if override_bp == 0 {
            telemetry::threshold_bp()
        } else {
            override_bp
        }
    }

    /// 生效的饱和处置策略（实例覆盖优先,否则 env/默认 Observe）
    #[must_use]
    pub fn saturation_mode(&self) -> SaturationMode {
        match self.mode_override.load(Ordering::Relaxed) {
            0 => SaturationMode::Observe,
            1 => SaturationMode::Degrade,
            2 => SaturationMode::Strict,
            // 哨兵 u8::MAX = 未覆盖 → 回落 env/默认
            _ => telemetry::saturation_mode(),
        }
    }

    /// 测试可见的饱和注入缝:强制阈值与处置策略（仅同模块测试使用）
    #[cfg(test)]
    pub(super) fn force_saturation(&self, threshold_bp: u32, mode: SaturationMode) {
        self.sat_bp_override.store(threshold_bp, Ordering::Relaxed);
        let encoded = match mode {
            SaturationMode::Observe => 0,
            SaturationMode::Degrade => 1,
            SaturationMode::Strict => 2,
        };
        self.mode_override.store(encoded, Ordering::Relaxed);
    }

    /// 测试可见的活跃计数句柄（仅同模块测试用于人工占池）
    #[cfg(test)]
    pub(super) fn active_handle(&self) -> Arc<ActiveCounter> {
        Arc::clone(&self.active)
    }

    /// WI-34 rayon 探针 — 池活跃率快照（供 utilization::RayonProbe 消费）
    #[must_use]
    pub fn rayon_probe(&self) -> RayonProbe {
        RayonProbe {
            pool_threads: self.thread_count,
            active_tasks: self.active.active(),
        }
    }
}
