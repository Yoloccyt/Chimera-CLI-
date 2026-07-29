//! Process-Score — 九维度过程评分(polish-v2.7 P3-5)
//!
//! 对应架构层:L7 Execution(pvl-layer 子模块)
//! 对应 ADR:ADR-049 决策 1(process-score 落点 pvl-layer)
//! 对应设计源:`chimera_ultimate_polish_v2.7.md` §11 / §8.2(快手 KAT Process-Score)
//!
//! # 核心思想(快手 KAT)
//!
//! 结果通过 ≠ 过程可信:硬编码返回 true 的测试也能"通过"。九维度过程评分
//! 从执行证据(时长/覆盖率/验证/重试纪律等)量化单次操作的过程质量,
//! 低分样本被过滤,不进入经验回放池污染训练数据(Phase 4 消费)。
//!
//! # 设计决策(WHY)
//!
//! - **纯函数评分**:`ProcessScorer::score` 无状态无副作用,输入观测输出评分,
//!   可在任意上下文(验证后/回放池入池前)调用,天然可测
//! - **真实执行检查**(方案 §8.2 `check_real_execution`):
//!   `execution_time > 10ms && coverage > 0` 是"测试真实跑过"的最低证据

use serde::{Deserialize, Serialize};

/// 真实执行的最短时长证据(毫秒,方案 §8.2)
///
/// WHY 10ms:真实的测试执行(进程启动 + 断言)不可能低于 10ms;
/// 亚 10ms "通过"高度疑似硬编码返回或空跑。
const REAL_EXECUTION_MIN_MS: u64 = 10;

/// 低质样本过滤阈值 — 总分低于此值的样本不应进入经验回放池
///
/// WHY 0.5:九维平均分低于一半意味着过程证据多数缺失,
/// 该样本的"成功/失败"标签不可信,入池会污染 R1 离线训练数据。
const LOW_QUALITY_THRESHOLD: f32 = 0.5;

/// 合理执行时长上限(毫秒)— 效率维度的满分边界
///
/// WHY 60s:单操作超过 1 分钟属长尾,效率分按比例衰减(不判零,
/// 长任务仍可能合理,只是效率证据变弱)。
const EFFICIENCY_CEILING_MS: u64 = 60_000;

/// 单次操作的过程观测 — 评分的输入证据集
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessObservation {
    /// 执行时长(毫秒)
    pub execution_time_ms: u64,
    /// 代码覆盖率 [0.0, 1.0](不可得时为 0,将影响真实执行维度)
    pub coverage: f32,
    /// Verifier 验证是否通过
    pub verification_passed: bool,
    /// Producer 自评置信度 [0.0, 1.0]
    pub confidence: f32,
    /// 重试次数(0 = 一次成功)
    pub retry_count: u32,
    /// 产出内容长度(字节;0 = 空产出)
    pub output_len: usize,
    /// 是否零孤儿调用(QEEP 保证,来自 gqep OrphanDetector)
    pub orphan_free: bool,
    /// 沙箱是否零违规(来自 seccore 审计)
    pub sandbox_clean: bool,
    /// 验证反馈是否被 Producer 应用(feedback 闭环证据)
    pub feedback_applied: bool,
}

/// 九维度过程评分结果
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessScore {
    /// 维度 1:真实执行(时长 >10ms 且覆盖率 >0,二值)
    pub real_execution: f32,
    /// 维度 2:覆盖率(直接取 coverage)
    pub coverage: f32,
    /// 维度 3:验证通过(二值)
    pub verification: f32,
    /// 维度 4:生产置信度(直接取 confidence)
    pub confidence: f32,
    /// 维度 5:执行效率(时长越短越高,60s 处衰减至 0)
    pub efficiency: f32,
    /// 维度 6:重试纪律(0 次重试满分,每次重试 -0.25)
    pub retry_discipline: f32,
    /// 维度 7:产出实质性(非空产出,二值)
    pub output_substance: f32,
    /// 维度 8:零孤儿调用(二值)
    pub orphan_free: f32,
    /// 维度 9:沙箱清洁(二值)
    pub sandbox_clean: f32,
    /// 九维平均总分 [0.0, 1.0]
    pub total: f32,
}

impl ProcessScore {
    /// 是否为低质样本(总分 < 0.5,应被过滤不入回放池)
    pub fn is_low_quality(&self) -> bool {
        self.total < LOW_QUALITY_THRESHOLD
    }
}

/// 真实执行检查(方案 §8.2,快手 Process-Score 的关键检查)
///
/// 真实执行需要时间(>10ms)且有代码覆盖率(>0);
/// 二者缺一即疑似"硬编码通过"或空跑。
pub fn check_real_execution(execution_time_ms: u64, coverage: f32) -> bool {
    execution_time_ms > REAL_EXECUTION_MIN_MS && coverage > 0.0
}

/// 九维度过程评分器 — 无状态纯函数评分
#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessScorer;

impl ProcessScorer {
    /// 创建评分器
    pub fn new() -> Self {
        Self
    }

    /// 对单次过程观测评分(九维度 + 平均总分)
    pub fn score(&self, obs: &ProcessObservation) -> ProcessScore {
        let real_execution = if check_real_execution(obs.execution_time_ms, obs.coverage) {
            1.0
        } else {
            0.0
        };
        let coverage = obs.coverage.clamp(0.0, 1.0);
        let verification = if obs.verification_passed { 1.0 } else { 0.0 };
        let confidence = obs.confidence.clamp(0.0, 1.0);
        // 效率:0ms → 1.0 线性衰减至 60s → 0.0
        let efficiency =
            (1.0 - obs.execution_time_ms as f32 / EFFICIENCY_CEILING_MS as f32).clamp(0.0, 1.0);
        // 重试纪律:每次重试 -0.25(4 次以上归零)
        let retry_discipline = (1.0 - obs.retry_count as f32 * 0.25).clamp(0.0, 1.0);
        let output_substance = if obs.output_len > 0 { 1.0 } else { 0.0 };
        let orphan_free = if obs.orphan_free { 1.0 } else { 0.0 };
        let sandbox_clean = if obs.sandbox_clean { 1.0 } else { 0.0 };

        let total = (real_execution
            + coverage
            + verification
            + confidence
            + efficiency
            + retry_discipline
            + output_substance
            + orphan_free
            + sandbox_clean)
            / 9.0;

        ProcessScore {
            real_execution,
            coverage,
            verification,
            confidence,
            efficiency,
            retry_discipline,
            output_substance,
            orphan_free,
            sandbox_clean,
            total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 健康观测样本(全维度接近满分)
    fn healthy_observation() -> ProcessObservation {
        ProcessObservation {
            execution_time_ms: 500,
            coverage: 0.8,
            verification_passed: true,
            confidence: 0.9,
            retry_count: 0,
            output_len: 1024,
            orphan_free: true,
            sandbox_clean: true,
            feedback_applied: true,
        }
    }

    #[test]
    fn test_check_real_execution_boundary() {
        // 方案 §8.2:>10ms 且 coverage>0 才算真实执行
        assert!(check_real_execution(11, 0.1));
        assert!(!check_real_execution(10, 0.1)); // 恰 10ms 不算
        assert!(!check_real_execution(100, 0.0)); // 零覆盖率不算
    }

    #[test]
    fn test_healthy_observation_scores_high() {
        let score = ProcessScorer::new().score(&healthy_observation());
        assert_eq!(score.real_execution, 1.0);
        assert_eq!(score.verification, 1.0);
        assert_eq!(score.retry_discipline, 1.0);
        assert!(score.total > 0.9);
        assert!(!score.is_low_quality());
    }

    #[test]
    fn test_hardcoded_pass_detected_as_low_quality() {
        // "硬编码通过"画像:验证通过但 1ms 完成、零覆盖、空产出
        let obs = ProcessObservation {
            execution_time_ms: 1,
            coverage: 0.0,
            verification_passed: true,
            confidence: 0.9,
            retry_count: 0,
            output_len: 0,
            orphan_free: false,
            sandbox_clean: false,
            feedback_applied: false,
        };
        let score = ProcessScorer::new().score(&obs);
        assert_eq!(score.real_execution, 0.0);
        // 验证虽通过,过程证据缺失 → 低质样本被过滤
        assert!(score.is_low_quality());
    }

    #[test]
    fn test_retry_discipline_decay() {
        let mut obs = healthy_observation();
        obs.retry_count = 2;
        let score = ProcessScorer::new().score(&obs);
        assert!((score.retry_discipline - 0.5).abs() < f32::EPSILON);

        obs.retry_count = 5;
        let score = ProcessScorer::new().score(&obs);
        assert_eq!(score.retry_discipline, 0.0);
    }

    #[test]
    fn test_efficiency_decays_with_duration() {
        let mut obs = healthy_observation();
        obs.execution_time_ms = 30_000; // 一半上限
        let score = ProcessScorer::new().score(&obs);
        assert!((score.efficiency - 0.5).abs() < 0.01);

        obs.execution_time_ms = 120_000; // 超上限
        let score = ProcessScorer::new().score(&obs);
        assert_eq!(score.efficiency, 0.0);
    }
}
