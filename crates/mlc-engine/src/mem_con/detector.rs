//! GhostMemoryDetector — 滑动窗口幽灵记忆检测器
//!
//! 对应架构层:L2 Memory
//! 对应任务:P2-8 MemCon 自适应控制器
//!
//! # 核心职责
//! 使用滑动窗口跟踪最近 N 次召回操作的结果,检测幽灵记忆(Ghost Memory)
//! 模式 — 即静态稀疏掩码导致的过时事实与当前事实共召回的问题。
//!
//! # 幽灵记忆检测原理
//! 幽灵记忆的特征是:同一标识符(ID)的条目在召回时,其内容与当前上下文
//! 不一致(过时事实)。`GhostMemoryDetector` 不直接检测内容一致性(这需要
//! 调用方告知),而是通过调用方传入的 `is_ghost` 标志统计幽灵率。
//!
//! 调用方(如 MlcEngine 的 recall 路径)在每次召回后,通过 `record_recall`
//! 方法传入是否为幽灵记忆,`GhostMemoryDetector` 维护滑动窗口并计算
//! 幽灵率。
//!
//! # 设计决策(WHY)
//! - **VecDeque 滑动窗口**:固定容量 VecDeque,插入 O(1),窗口满时自动移除
//!   最旧记录。比 Vec 更高效(无需手动移除旧元素),比环形缓冲区更简单。
//! - **幽灵率 = 窗口内幽灵计数 / 窗口内总召回数**:如果窗口未满,分母为
//!   当前实际数而非窗口大小,避免初始阶段幽灵率虚高或虚低。
//! - **无锁设计**:`GhostMemoryDetector` 不是 `Send + Sync` 的,由 `MemConController`
//!   在 `MlcEngine` 的 async 上下文中序列化访问。`MlcEngine` 的 recall 路径
//!   是 `&self` 方法,但 `MemConController` 使用 `Mutex` 内部可变性。

use std::collections::VecDeque;

use super::config::MemConConfig;

/// 幽灵记忆检测器 — 滑动窗口统计召回结果的幽灵率
///
/// 维护一个固定大小的滑动窗口,记录最近 N 次召回操作是否为幽灵记忆。
/// 提供当前幽灵率查询,供 `MemConController` 决策策略调整。
#[derive(Debug, Clone)]
pub struct GhostMemoryDetector {
    /// 滑动窗口(固定容量,VecDeque 实现)
    window: VecDeque<bool>,
    /// 窗口内幽灵计数(缓存,避免每次遍历求和)
    ghost_count: usize,
    /// 配置引用
    config: MemConConfig,
}

impl GhostMemoryDetector {
    /// 创建新的幽灵记忆检测器
    ///
    /// 使用指定的配置初始化滑动窗口(空窗口)。
    pub fn new(config: MemConConfig) -> Self {
        Self {
            window: VecDeque::with_capacity(config.window_size),
            ghost_count: 0,
            config,
        }
    }

    /// 记录一次召回结果
    ///
    /// `is_ghost`: 本次召回是否为幽灵记忆。
    /// 如果窗口已满,自动移除最旧的记录。
    pub fn record_recall(&mut self, is_ghost: bool) {
        // 窗口已满,移除最旧记录
        if self.window.len() == self.config.window_size {
            if let Some(oldest) = self.window.pop_front() {
                if oldest {
                    self.ghost_count -= 1;
                }
            }
        }

        // 添加新记录
        self.window.push_back(is_ghost);
        if is_ghost {
            self.ghost_count += 1;
        }
    }

    /// 获取当前窗口幽灵率
    ///
    /// 返回 [0.0, 1.0] 范围内的幽灵率。
    /// 如果窗口为空(尚无召回记录),返回 0.0。
    pub fn ghost_rate(&self) -> f32 {
        let total = self.window.len();
        if total == 0 {
            return 0.0;
        }
        self.ghost_count as f32 / total as f32
    }

    /// 获取当前幽灵率是否超过配置阈值
    pub fn is_ghost_threshold_exceeded(&self) -> bool {
        self.ghost_rate() > self.config.ghost_threshold
    }

    /// 获取当前幽灵率是否超过熔断阈值
    pub fn is_circuit_breaker_threshold_exceeded(&self) -> bool {
        self.ghost_rate() > self.config.circuit_breaker_ghost_rate
    }

    /// 获取窗口内幽灵计数
    pub fn ghost_count(&self) -> u32 {
        self.ghost_count as u32
    }

    /// 获取窗口总大小(当前记录数)
    pub fn window_size(&self) -> u32 {
        self.window.len() as u32
    }

    /// 重置检测器(清空窗口)
    pub fn reset(&mut self) {
        self.window.clear();
        self.ghost_count = 0;
    }

    /// 获取配置引用
    pub fn config(&self) -> &MemConConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证默认配置下,空窗口幽灵率为 0.0
    #[test]
    fn test_empty_window_ghost_rate_zero() {
        let detector = GhostMemoryDetector::new(MemConConfig::default());
        assert_eq!(detector.ghost_rate(), 0.0);
        assert!(!detector.is_ghost_threshold_exceeded());
    }

    /// 验证全部非幽灵召回时,幽灵率为 0.0
    #[test]
    fn test_all_non_ghost_records() {
        let mut detector = GhostMemoryDetector::new(MemConConfig::default());
        for _ in 0..50 {
            detector.record_recall(false);
        }
        assert_eq!(detector.ghost_rate(), 0.0);
        assert!(!detector.is_ghost_threshold_exceeded());
    }

    /// 验证全部幽灵召回时,幽灵率为 1.0
    #[test]
    fn test_all_ghost_records() {
        let mut detector = GhostMemoryDetector::new(MemConConfig::default());
        for _ in 0..50 {
            detector.record_recall(true);
        }
        assert_eq!(detector.ghost_rate(), 1.0);
        assert!(detector.is_ghost_threshold_exceeded());
        assert!(detector.is_circuit_breaker_threshold_exceeded());
    }

    /// 验证幽灵率在阈值附近(略低于阈值)
    #[test]
    fn test_ghost_rate_below_threshold() {
        let config = MemConConfig {
            ghost_threshold: 0.5,
            ..Default::default()
        };
        let mut detector = GhostMemoryDetector::new(config);

        // 10 次召回,4 次幽灵 = 40% < 50%
        for _ in 0..6 {
            detector.record_recall(false);
        }
        for _ in 0..4 {
            detector.record_recall(true);
        }
        assert!((detector.ghost_rate() - 0.4).abs() < f32::EPSILON);
        assert!(!detector.is_ghost_threshold_exceeded());
    }

    /// 验证幽灵率在阈值附近(略高于阈值)
    #[test]
    fn test_ghost_rate_above_threshold() {
        let config = MemConConfig {
            ghost_threshold: 0.5,
            ..Default::default()
        };
        let mut detector = GhostMemoryDetector::new(config);

        // 10 次召回,6 次幽灵 = 60% > 50%
        for _ in 0..4 {
            detector.record_recall(false);
        }
        for _ in 0..6 {
            detector.record_recall(true);
        }
        assert!((detector.ghost_rate() - 0.6).abs() < f32::EPSILON);
        assert!(detector.is_ghost_threshold_exceeded());
    }

    /// 验证滑动窗口:窗口满后,最旧记录被移除
    #[test]
    fn test_sliding_window_eviction() {
        let config = MemConConfig {
            window_size: 10,
            ..Default::default()
        };
        let mut detector = GhostMemoryDetector::new(config);

        // 填充 10 次非幽灵
        for _ in 0..10 {
            detector.record_recall(false);
        }
        assert_eq!(detector.ghost_rate(), 0.0);

        // 再添加 10 次幽灵(窗口满,最旧的 10 次非幽灵被移除)
        for _ in 0..10 {
            detector.record_recall(true);
        }
        assert_eq!(detector.ghost_rate(), 1.0);
        assert_eq!(detector.window_size(), 10);
    }

    /// 验证重置后检测器回到初始状态
    #[test]
    fn test_reset_detector() {
        let mut detector = GhostMemoryDetector::new(MemConConfig::default());
        detector.record_recall(true);
        detector.record_recall(true);
        assert!(detector.ghost_count() > 0);

        detector.reset();
        assert_eq!(detector.ghost_rate(), 0.0);
        assert_eq!(detector.ghost_count(), 0);
        assert_eq!(detector.window_size(), 0);
    }

    /// 验证幽灵计数与窗口大小一致
    #[test]
    fn test_ghost_count_consistency() {
        let mut detector = GhostMemoryDetector::new(MemConConfig::default());
        for i in 0..20 {
            detector.record_recall(i % 3 == 0); // 约 1/3 是幽灵
        }
        let expected_ghost_count = (0..20).filter(|i| i % 3 == 0).count() as u32;
        assert_eq!(detector.ghost_count(), expected_ghost_count);
        assert_eq!(detector.window_size(), 20);
    }

    /// 验证配置验证的最小窗口值
    #[test]
    fn test_window_size_zero_invalid() {
        let config = MemConConfig {
            window_size: 0,
            ..Default::default()
        };
        let errors = config.validate();
        assert!(!errors.is_empty());
        assert!(errors.iter().any(|e| e.contains("window_size 必须大于 0")));
    }

    /// 验证配置验证的最大窗口值
    #[test]
    fn test_window_size_above_max_invalid() {
        let config = MemConConfig {
            window_size: 10_001,
            ..Default::default()
        };
        let errors = config.validate();
        assert!(!errors.is_empty());
        assert!(errors
            .iter()
            .any(|e| e.contains("window_size 不能超过 10000")));
    }
}
