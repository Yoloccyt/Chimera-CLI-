//! 衰减计算器 — 基于访问频率与时间衰减的优先级计算
//!
//! 对应架构层:L3 Storage
//!
//! # 设计决策(WHY)
//! - **指数衰减公式**:`priority = access_count × exp(-Δt / τ)`
//!   - `access_count`:访问次数(反映历史热度)
//!   - `Δt = now - last_accessed_at`:距上次访问的时间(秒)
//!   - `τ`:衰减时间常数(默认 86400 秒 = 24 小时)
//!   - `exp(-Δt / τ)`:指数衰减因子,Δt 越大衰减越快
//! - **降级阈值 0.1**:`priority < 0.1` 时触发降级迁移
//!   - 例:τ=24h,Δt=72h 时,exp(-3) ≈ 0.0498,若 access_count=1 则 priority ≈ 0.0498 < 0.1
//!   - 例:τ=24h,Δt=24h 时,exp(-1) ≈ 0.3679,若 access_count=1 则 priority ≈ 0.3679 > 0.1
//! - **τ 可配置**:通过 `CmtConfig.decay_tau_seconds` 配置,
//!   τ 越大衰减越慢(适合长期保留场景),τ 越小衰减越快(适合频繁更新场景)
//!
//! # 性能基准
//! - 衰减计算 < 1μs(纯数学运算,无 I/O)
//! - 测试验证:τ=24h,Δt=72h 时 priority < 0.1 触发降级

use chrono::{DateTime, Utc};
use tracing::trace;

use crate::error::CmtError;
use crate::types::CapabilityEntry;

/// 降级阈值:priority < 0.1 时触发降级迁移
pub const DEMOTION_THRESHOLD: f32 = 0.1;

/// 衰减计算器 — 计算能力条目的衰减优先级
///
/// 基于指数衰减公式 `priority = access_count × exp(-Δt / τ)`,
/// `priority < 0.1` 时触发降级迁移。
pub struct DecayCalculator {
    /// 衰减时间常数 τ(秒)
    tau_seconds: u64,
}

impl DecayCalculator {
    /// 创建衰减计算器,指定时间常数 τ(秒)
    pub fn new(tau_seconds: u64) -> Result<Self, CmtError> {
        if tau_seconds == 0 {
            return Err(CmtError::InvalidConfig("decay_tau_seconds 不能为 0".into()));
        }
        Ok(Self { tau_seconds })
    }

    /// 从配置创建衰减计算器
    pub fn from_config(config: &crate::config::CmtConfig) -> Result<Self, CmtError> {
        Self::new(config.decay_tau_seconds)
    }

    /// 返回时间常数 τ(秒)
    pub fn tau_seconds(&self) -> u64 {
        self.tau_seconds
    }

    /// 计算能力条目的衰减优先级
    ///
    /// 公式:`priority = access_count × exp(-Δt / τ)`
    ///
    /// # 参数
    /// - `entry`:能力条目(使用 `access_count` 与 `last_accessed_at`)
    /// - `now`:当前时间(用于计算 Δt)
    ///
    /// # 返回
    /// 衰减后的优先级 [0.0, +∞)
    /// - access_count = 0 时返回 0.0(从未访问的条目优先级最低)
    /// - Δt = 0 时返回 access_count(刚访问的条目优先级 = 访问次数)
    /// - Δt 越大,优先级越低(指数衰减)
    pub fn compute_priority(&self, entry: &CapabilityEntry, now: DateTime<Utc>) -> f32 {
        // 从未访问的条目优先级为 0
        if entry.access_count == 0 {
            return 0.0;
        }

        // 计算 Δt(秒):距上次访问的时间
        let delta = now.signed_duration_since(entry.last_accessed_at);
        let delta_seconds = delta.num_seconds().max(0) as f64;

        // 计算衰减因子:exp(-Δt / τ)
        let tau = self.tau_seconds as f64;
        let decay_factor = (-delta_seconds / tau).exp();

        // priority = access_count × exp(-Δt / τ)
        let priority = entry.access_count as f32 * decay_factor as f32;

        trace!(
            cap_id = %entry.id,
            access_count = entry.access_count,
            delta_seconds = delta_seconds,
            tau_seconds = self.tau_seconds,
            decay_factor = decay_factor,
            priority = priority,
            "衰减优先级已计算"
        );

        priority
    }

    /// 判断能力条目是否应触发降级迁移
    ///
    /// 返回 true 表示 `priority < 0.1`,应降级到下层。
    pub fn should_demote(&self, entry: &CapabilityEntry, now: DateTime<Utc>) -> bool {
        let priority = self.compute_priority(entry, now);
        priority < DEMOTION_THRESHOLD
    }

    /// 基于 metadata 判断是否应触发降级迁移(不加载完整条目)
    ///
    /// WHY(SubTask 19.2):衰减判断仅需 `access_count` + `last_accessed_at`,
    /// 无需加载完整 CapabilityEntry(含 content)。此方法允许调用方
    /// 通过 `list_idle_metadata()` 获取轻量元数据进行批量衰减判断,
    /// 避免全量加载 content 导致的内存峰值。
    pub fn should_demote_metadata(
        &self,
        last_accessed_at: DateTime<Utc>,
        access_count: u64,
        now: DateTime<Utc>,
    ) -> bool {
        // 从未访问的条目优先级为 0,应降级
        if access_count == 0 {
            return true;
        }

        // 计算 Δt(秒):距上次访问的时间
        let delta = now.signed_duration_since(last_accessed_at);
        let delta_seconds = delta.num_seconds().max(0) as f64;

        // 计算衰减因子:exp(-Δt / τ)
        let tau = self.tau_seconds as f64;
        let decay_factor = (-delta_seconds / tau).exp();

        // priority = access_count × exp(-Δt / τ)
        let priority = access_count as f32 * decay_factor as f32;
        priority < DEMOTION_THRESHOLD
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Tier;
    use chrono::Duration;

    fn make_entry_with_access(
        id: &str,
        access_count: u64,
        last_accessed_at: DateTime<Utc>,
    ) -> CapabilityEntry {
        let mut entry = CapabilityEntry::new(id, format!("content-{id}"), Tier::Hot);
        entry.access_count = access_count;
        entry.last_accessed_at = last_accessed_at;
        entry
    }

    #[test]
    fn test_new_valid_tau() {
        let calc = DecayCalculator::new(86400).unwrap();
        assert_eq!(calc.tau_seconds(), 86400);
    }

    #[test]
    fn test_new_zero_tau_returns_error() {
        let result = DecayCalculator::new(0);
        assert!(matches!(result, Err(CmtError::InvalidConfig(_))));
    }

    #[test]
    fn test_compute_priority_zero_access_count() {
        let calc = DecayCalculator::new(86400).unwrap();
        let now = Utc::now();
        let entry = make_entry_with_access("cap-1", 0, now);

        // 从未访问的条目优先级为 0
        let priority = calc.compute_priority(&entry, now);
        assert_eq!(priority, 0.0);
    }

    #[test]
    fn test_compute_priority_zero_delta_t() {
        let calc = DecayCalculator::new(86400).unwrap();
        let now = Utc::now();
        let entry = make_entry_with_access("cap-1", 10, now);

        // Δt = 0 时,priority = access_count × exp(0) = access_count × 1
        let priority = calc.compute_priority(&entry, now);
        assert!((priority - 10.0).abs() < 1e-6);
    }

    #[test]
    fn test_compute_priority_tau_24h_delta_t_24h() {
        // τ=24h,Δt=24h 时,exp(-1) ≈ 0.3679
        let calc = DecayCalculator::new(86400).unwrap();
        let now = Utc::now();
        let entry = make_entry_with_access("cap-1", 1, now - Duration::hours(24));

        let priority = calc.compute_priority(&entry, now);
        // exp(-1) ≈ 0.36787944
        assert!((priority - 0.36787944).abs() < 1e-4);
        // priority > 0.1,不应触发降级
        assert!(!calc.should_demote(&entry, now));
    }

    #[test]
    fn test_compute_priority_tau_24h_delta_t_72h() {
        // τ=24h,Δt=72h 时,exp(-3) ≈ 0.0498
        // 任务要求:τ=24h,Δt=72h 时 priority < 0.1 触发降级
        let calc = DecayCalculator::new(86400).unwrap();
        let now = Utc::now();
        let entry = make_entry_with_access("cap-1", 1, now - Duration::hours(72));

        let priority = calc.compute_priority(&entry, now);
        // exp(-3) ≈ 0.04978707
        assert!((priority - 0.04978707).abs() < 1e-4);
        // priority < 0.1,应触发降级
        assert!(priority < DEMOTION_THRESHOLD);
        assert!(calc.should_demote(&entry, now));
    }

    #[test]
    fn test_compute_priority_high_access_count() {
        // 高访问次数的条目即使长时间未访问,优先级也可能高于阈值
        let calc = DecayCalculator::new(86400).unwrap();
        let now = Utc::now();
        // access_count = 100,Δt = 72h
        let entry = make_entry_with_access("cap-1", 100, now - Duration::hours(72));

        let priority = calc.compute_priority(&entry, now);
        // priority = 100 × exp(-3) ≈ 100 × 0.0498 ≈ 4.98
        assert!((priority - 4.98).abs() < 0.1);
        // priority > 0.1,不应触发降级
        assert!(!calc.should_demote(&entry, now));
    }

    #[test]
    fn test_should_demote_at_threshold() {
        let calc = DecayCalculator::new(86400).unwrap();
        let now = Utc::now();

        // priority 刚好等于阈值 0.1 时不触发降级(严格小于)
        // priority = access_count × exp(-Δt / τ) = 0.1
        // exp(-Δt / τ) = 0.1 / access_count
        // Δt = -τ × ln(0.1 / access_count)
        // 取 access_count = 1,Δt = -86400 × ln(0.1) ≈ 86400 × 2.3026 ≈ 198934 秒 ≈ 55.26 小时
        let delta_seconds = -86400.0 * (0.1_f64).ln();
        let delta_duration = Duration::seconds(delta_seconds as i64);
        let entry = make_entry_with_access("cap-1", 1, now - delta_duration);

        let priority = calc.compute_priority(&entry, now);
        // priority 应接近 0.1(浮点误差允许)
        assert!((priority - 0.1).abs() < 1e-4);
    }

    #[test]
    fn test_compute_priority_negative_delta_t() {
        // last_accessed_at 在未来(时钟漂移),Δt 应视为 0
        let calc = DecayCalculator::new(86400).unwrap();
        let now = Utc::now();
        let entry = make_entry_with_access("cap-1", 5, now + Duration::hours(1));

        let priority = calc.compute_priority(&entry, now);
        // Δt 视为 0,priority = access_count × 1 = 5
        assert!((priority - 5.0).abs() < 1e-6);
    }

    #[test]
    fn test_from_config() {
        let config = crate::config::CmtConfig::default();
        let calc = DecayCalculator::from_config(&config).unwrap();
        assert_eq!(calc.tau_seconds(), 86400);
    }
}
