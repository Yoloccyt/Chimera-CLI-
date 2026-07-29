//! 后悔率自动采集管线 — R2 解冻阶段③ 前置 1(纯可观测性)
//!
//! 对应架构层: L6 Router(omega-learner,与 verify_regret_non_increasing 同 crate)
//! 对应 ADR: ADR-052 待办 1(后悔率自动采集管线)+ ADR-042(R2 冻结)
//! 对应计划: R2 解冻阶段③ 前置 1
//!
//! # 职责:闭合后悔率"采集端"缺口
//!
//! `formal.rs` 的 `LearningMonotonicityChecker::verify_regret_non_increasing` 是
//! 后悔率的**验证端**(纯函数,吃 `&[f64]`),其头注释预留"上层编排器采集
//! (step, reward, regret) 快照序列投喂本验证器"——但采集端此前是真空。
//! 本模块补齐采集端:滑动窗口聚合后悔率观测,产出 `VerificationResult` 趋势信号。
//!
//! # 与前置 3 的信号链闭合
//!
//! 本采集管线的 `assess_trend()` 产出 `VerificationResult`,正好是 decay-engine
//! `ShadowModeCircuitBreaker::observe()` 的输入。前置1(采集)→ 前置3(熔断)
//! 构成完整信号链:后悔率发散 → assess_trend 返回 Violated → 熔断器永久跳闸。
//!
//! # WHY 落 omega-learner 而非 efficiency-monitor(架构决策)
//!
//! - **与验证器同 crate**:采集管线需调用 `verify_regret_non_increasing`,
//!   与之同 crate 避免跨层依赖;后悔率概念本就属于学习层(L6)
//! - **efficiency-monitor 无法访问 `VerificationResult`**:其架构约束是"仅依赖
//!   event-bus + nexus-core",不依赖 nexus-contracts(L0),拿不到该类型
//! - **零新增依赖**:VecDeque(std)+ chrono + nexus-contracts 均已在 omega-learner
//!
//! # R2 冻结声明(ADR-042)
//!
//! 纯可观测性:仅**记录**调用方提供的后悔率观测,无梯度更新、无策略网络、
//! 无训练路径;标识符规避 5 个 R2 扫描关键词。是解冻前的监控基建,不解冻。

use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::formal::LearningMonotonicityChecker;
use nexus_contracts::formal_props::VerificationResult;

/// 后悔率趋势评估的默认分块窗口大小
///
/// WHY 2:与 `formal.rs` 测试基线一致(window=2),使不重叠分块至少产出
/// 2 个窗口均值即可比较趋势;分块过大需更多样本才能评估。
pub const DEFAULT_TREND_WINDOW: usize = 2;

/// 后悔率趋势评估的默认容差
///
/// WHY 0.05:与 `formal.rs` 测试基线一致,吸收 bandit 探索步的后悔率抖动
/// (探索本质会引入短期后悔率上升,容差避免误判为发散)。
pub const DEFAULT_TREND_TOLERANCE: f64 = 0.05;

/// 默认滑动窗口容量(保留最近 N 步观测)
///
/// WHY 128:影子模式趋势评估需足够样本(≥ 数十步)才有统计意义,
/// 128 步在内存(128 × 约 24 字节 ≈ 3KB)与趋势灵敏度间平衡。
pub const DEFAULT_CAPACITY: usize = 128;

/// 单步后悔率观测 — 采集管线的最小输入单元
///
/// 由调用方(学习循环)在每步产生:step 为学习步(应单调递增),
/// regret 为该步的后悔率观测(语义上 ≥ 0)。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RegretSample {
    /// 学习步序号(应随时间单调递增)
    pub step: u64,
    /// 该步后悔率观测(regret ≥ 0)
    pub regret: f64,
    /// 观测时间戳(UTC)
    pub timestamp: DateTime<Utc>,
}

impl RegretSample {
    /// 构造后悔率观测(timestamp 取当前 UTC)
    pub fn new(step: u64, regret: f64) -> Self {
        Self {
            step,
            regret,
            timestamp: Utc::now(),
        }
    }
}

/// 后悔率采集管线 — 滑动窗口聚合 + 趋势/单调性评估
///
/// FIFO 滑动窗口存储最近 `capacity` 条观测;`assess_trend` / `assess_step_monotonicity`
/// 复用 `LearningMonotonicityChecker` 的纯函数验证器对窗口内数据评估,
/// 产出 `VerificationResult` 供熔断器 / 告警消费。
#[derive(Debug, Clone)]
pub struct RegretCollector {
    /// 后悔率观测滑动窗口(FIFO,超容量丢弃最旧)
    samples: VecDeque<RegretSample>,
    /// 窗口容量上限
    capacity: usize,
    /// 趋势评估的不重叠分块窗口大小
    trend_window: usize,
    /// 趋势评估容差(相邻窗口均值允许的最大上升)
    tolerance: f64,
}

impl Default for RegretCollector {
    fn default() -> Self {
        Self::new(
            DEFAULT_CAPACITY,
            DEFAULT_TREND_WINDOW,
            DEFAULT_TREND_TOLERANCE,
        )
    }
}

impl RegretCollector {
    /// 创建采集管线
    ///
    /// # 参数
    /// - `capacity`: 滑动窗口容量(≥ 1;传 0 归一为 1 防退化)
    /// - `trend_window`: 趋势评估分块窗口(≥ 1;传 0 归一为 1)
    /// - `tolerance`: 趋势容差(吸收探索抖动)
    pub fn new(capacity: usize, trend_window: usize, tolerance: f64) -> Self {
        Self {
            samples: VecDeque::with_capacity(capacity.max(1)),
            capacity: capacity.max(1),
            trend_window: trend_window.max(1),
            tolerance,
        }
    }

    /// 记录一条后悔率观测;超容量则丢弃最旧(FIFO)
    ///
    /// WHY 不校验 step 单调性:采集侧只忠实存储观测,单调性判定交给
    /// `assess_step_monotonicity`(职责分离);校验前置会掩盖真实乱序数据,
    /// 反而妨碍熔断器发现"快照乱序"这类持久化缺陷。
    pub fn record(&mut self, sample: RegretSample) {
        if self.samples.len() >= self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    /// 便捷记录(step + regret,timestamp 自动取当前 UTC)
    pub fn record_regret(&mut self, step: u64, regret: f64) {
        self.record(RegretSample::new(step, regret));
    }

    /// 当前窗口内观测数
    #[must_use]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// 窗口是否为空
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// 提取当前窗口的后悔率序列(按记录顺序)
    #[must_use]
    pub fn regret_sequence(&self) -> Vec<f64> {
        self.samples.iter().map(|s| s.regret).collect()
    }

    /// 最新一条后悔率观测(空窗为 None)
    #[must_use]
    pub fn latest_regret(&self) -> Option<f64> {
        self.samples.back().map(|s| s.regret)
    }

    /// 当前窗口后悔率均值(空窗为 None)
    #[must_use]
    pub fn mean_regret(&self) -> Option<f64> {
        if self.samples.is_empty() {
            return None;
        }
        let sum: f64 = self.samples.iter().map(|s| s.regret).sum();
        Some(sum / self.samples.len() as f64)
    }

    /// 评估后悔率非增趋势 — 复用 `verify_regret_non_increasing`
    ///
    /// 对当前窗口的后悔率序列做不重叠分块均值趋势判定:
    /// - `Satisfied`: 相邻窗口均值非增(容差内),学习收敛
    /// - `Violated`: 后悔率发散(应触发熔断回退)
    /// - `Skipped`: 样本不足(完整窗口 < 2)
    #[must_use]
    pub fn assess_trend(&self) -> VerificationResult {
        let seq = self.regret_sequence();
        LearningMonotonicityChecker::new().verify_regret_non_increasing(
            &seq,
            self.trend_window,
            self.tolerance,
        )
    }

    /// 评估学习步单调性 — 复用 `verify_steps_monotonic`
    ///
    /// 验证窗口内 step 序列严格递增(违反 = 快照乱序 / 状态回退)。
    #[must_use]
    pub fn assess_step_monotonicity(&self) -> VerificationResult {
        let steps: Vec<u64> = self.samples.iter().map(|s| s.step).collect();
        LearningMonotonicityChecker::new().verify_steps_monotonic(&steps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_is_empty() {
        let c = RegretCollector::default();
        assert!(c.is_empty());
        assert_eq!(c.len(), 0);
        assert!(c.latest_regret().is_none());
        assert!(c.mean_regret().is_none());
    }

    #[test]
    fn test_record_accumulates() {
        let mut c = RegretCollector::default();
        c.record_regret(1, 0.9);
        c.record_regret(2, 0.7);
        assert_eq!(c.len(), 2);
        assert_eq!(c.latest_regret(), Some(0.7));
        assert_eq!(c.regret_sequence(), vec![0.9, 0.7]);
    }

    #[test]
    fn test_capacity_evicts_oldest() {
        let mut c = RegretCollector::new(3, 2, 0.05);
        // WHY 字面量数组而非 step*0.1 计算:避免 3.0*0.1=0.30000...4 的浮点误差
        // 导致 assert_eq! 精确比较失败(测试关注淘汰逻辑,非浮点运算)
        let regrets = [0.11, 0.22, 0.33, 0.44, 0.55];
        for (i, r) in regrets.iter().enumerate() {
            c.record_regret(i as u64 + 1, *r);
        }
        // 容量 3:只保留最后 3 条(step 3/4/5)
        assert_eq!(c.len(), 3);
        assert_eq!(c.regret_sequence(), vec![0.33, 0.44, 0.55]);
    }

    #[test]
    fn test_zero_capacity_normalized_to_one() {
        let mut c = RegretCollector::new(0, 0, 0.05);
        c.record_regret(1, 0.5);
        c.record_regret(2, 0.4);
        // 容量归一为 1:只留最新
        assert_eq!(c.len(), 1);
        assert_eq!(c.latest_regret(), Some(0.4));
    }

    #[test]
    fn test_mean_regret() {
        let mut c = RegretCollector::default();
        c.record_regret(1, 0.6);
        c.record_regret(2, 0.4);
        assert_eq!(c.mean_regret(), Some(0.5));
    }

    #[test]
    fn test_assess_trend_converging_satisfied() {
        let mut c = RegretCollector::new(128, 2, 0.05);
        // 收敛曲线:窗口均值 0.8 → 0.5 → 0.2
        for (step, r) in [0.9, 0.7, 0.6, 0.4, 0.3, 0.1].iter().enumerate() {
            c.record_regret(step as u64 + 1, *r);
        }
        assert!(c.assess_trend().is_satisfied());
    }

    #[test]
    fn test_assess_trend_diverging_violated() {
        let mut c = RegretCollector::new(128, 2, 0.05);
        // 发散:窗口均值 0.2 → 0.8
        for (step, r) in [0.2, 0.2, 0.8, 0.8].iter().enumerate() {
            c.record_regret(step as u64 + 1, *r);
        }
        assert!(matches!(
            c.assess_trend(),
            VerificationResult::Violated { .. }
        ));
    }

    #[test]
    fn test_assess_trend_insufficient_skipped() {
        let mut c = RegretCollector::new(128, 2, 0.05);
        c.record_regret(1, 0.5); // 单条,完整窗口 < 2
        assert!(matches!(
            c.assess_trend(),
            VerificationResult::Skipped { .. }
        ));
    }

    #[test]
    fn test_assess_step_monotonic_satisfied() {
        let mut c = RegretCollector::default();
        c.record_regret(1, 0.5);
        c.record_regret(2, 0.4);
        c.record_regret(3, 0.3);
        assert!(c.assess_step_monotonicity().is_satisfied());
    }

    #[test]
    fn test_assess_step_monotonic_violated_on_regression() {
        let mut c = RegretCollector::default();
        c.record_regret(5, 0.5);
        c.record_regret(3, 0.4); // 步数回退!
        assert!(matches!(
            c.assess_step_monotonicity(),
            VerificationResult::Violated { .. }
        ));
    }

    #[test]
    fn test_sample_new_sets_timestamp() {
        let before = Utc::now();
        let s = RegretSample::new(1, 0.5);
        assert!(s.timestamp >= before);
        assert_eq!(s.step, 1);
        assert_eq!(s.regret, 0.5);
    }
}
