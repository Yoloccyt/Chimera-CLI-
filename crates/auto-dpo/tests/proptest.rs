//! auto-dpo 属性测试 — 偏好对构建不变量
//!
//! 对应架构层:L5 Knowledge
//! 对应 SubTask 13.4:v1.5.0-omega 发布就绪差距闭合
//!
//! # 测试覆盖的不变量
//! 1. ModelOutput::new 对 score 进行 clamp 到 [0, 1],NaN 映射为 0.0
//! 2. SampleQuality::from_score 分级:≥0.8 → High,≥0.5 → Medium,<0.5 → Low
//! 3. PreferencePair::score_gap() == chosen_score - rejected_score
//! 4. validate 拒绝 chosen_score <= rejected_score 的偏好对(无偏好信号)
//! 5. generate 选出的 chosen_score 严格 > rejected_score(偏好信号非空)
//!
//! # 设计要点
//! - 使用整数策略生成 f32 score(避免 NaN/Inf)
//! - block-named 语法(§4.1 规范)
//! - 256 cases(proptest 默认)

#![forbid(unsafe_code)]

use auto_dpo::{
    AutoDpoConfig, ModelOutput, PreferencePair, PreferencePairGenerator, SampleQuality,
};
use proptest::prelude::*;

proptest! {
    /// 不变量 1:ModelOutput::new 对 score 进行 clamp 到 [0, 1],NaN 映射为 0.0
    ///
    /// - 输入 score ∈ [0, 1]:保持不变
    /// - 输入 score > 1:clamp 到 1.0
    /// - 输入 score < 0:clamp 到 0.0
    /// - 输入 NaN:映射为 0.0(§4.4 反模式:NaN 污染会导致排序异常)
    #[test]
    fn prop_model_output_score_clamp(score_milli in 0u32..=2000) {
        let input_score = score_milli as f32 / 1000.0; // [0.0, 2.0]
        let output = ModelOutput::new("test", input_score);

        // 期望值:clamp 到 [0, 1]
        let expected = if input_score > 1.0 {
            1.0
        } else {
            input_score
        };
        prop_assert!(
            (output.score - expected).abs() < 1e-6,
            "score {} 应 clamp 到 {},实际 {}",
            input_score,
            expected,
            output.score
        );
        prop_assert!(output.score >= 0.0 && output.score <= 1.0);
    }

    /// 不变量 2:SampleQuality::from_score 分级正确
    ///
    /// - score ∈ [0.8, 1.0] → High
    /// - score ∈ [0.5, 0.8) → Medium
    /// - score ∈ [0.0, 0.5) → Low
    #[test]
    fn prop_quality_from_score_classification(score_milli in 0u32..=1000) {
        let score = score_milli as f32 / 1000.0;
        let quality = SampleQuality::from_score(score);

        if score >= 0.8 {
            prop_assert_eq!(
                quality,
                SampleQuality::High,
                "score {} >= 0.8 应为 High",
                score
            );
        } else if score >= 0.5 {
            prop_assert_eq!(
                quality,
                SampleQuality::Medium,
                "score {} ∈ [0.5, 0.8) 应为 Medium",
                score
            );
        } else {
            prop_assert_eq!(
                quality,
                SampleQuality::Low,
                "score {} < 0.5 应为 Low",
                score
            );
        }
    }

    /// 不变量 3:PreferencePair::score_gap() == chosen_score - rejected_score
    ///
    /// 验证 score_gap 的算术不变量:精确等于 chosen - rejected。
    #[test]
    fn prop_preference_pair_score_gap(
        chosen_milli in 0u32..=1000,
        rejected_milli in 0u32..=1000,
    ) {
        let chosen_score = chosen_milli as f32 / 1000.0;
        let rejected_score = rejected_milli as f32 / 1000.0;
        let pair = PreferencePair::new(
            "pair-test",
            "chosen-text",
            "rejected-text",
            chosen_score,
            rejected_score,
        );
        let expected_gap = chosen_score - rejected_score;
        prop_assert!(
            (pair.score_gap() - expected_gap).abs() < 1e-6,
            "score_gap 应为 {} - {} = {},实际 {}",
            chosen_score,
            rejected_score,
            expected_gap,
            pair.score_gap()
        );
    }

    /// 不变量 4:validate 拒绝 chosen_score <= rejected_score 的偏好对
    ///
    /// DPO 语义要求偏好信号非空:chosen 必须严格优于 rejected。
    /// 当 chosen_score <= rejected_score 时,validate 应返回错误。
    #[test]
    fn prop_validate_rejects_non_positive_gap(
        chosen_milli in 0u32..=1000,
        rejected_milli in 0u32..=1000,
    ) {
        let generator = PreferencePairGenerator::new(AutoDpoConfig::default())
            .expect("默认配置应有效");
        let chosen_score = chosen_milli as f32 / 1000.0;
        let rejected_score = rejected_milli as f32 / 1000.0;
        let pair = PreferencePair::new(
            "pair-test",
            "chosen-text",
            "rejected-text",
            chosen_score,
            rejected_score,
        );
        let result = generator.validate(&pair);

        if chosen_score <= rejected_score {
            prop_assert!(
                result.is_err(),
                "chosen_score={} <= rejected_score={} 应被拒绝",
                chosen_score,
                rejected_score
            );
        } else if (0.5..=1.0).contains(&chosen_score) {
            // chosen_score > rejected_score 且 chosen 质量可接受时,应通过
            // (chosen_score < 0.5 会触发 QualityTooLow)
            prop_assert!(
                result.is_ok(),
                "有效偏好对(chosen={} > rejected={}, chosen 质量可接受)应通过校验: {:?}",
                chosen_score,
                rejected_score,
                result
            );
        }
    }

    /// 不变量 5:generate 选出的 chosen_score 严格 > rejected_score
    ///
    /// 从 N 个候选中生成偏好对,chosen(最高分)必须严格 > rejected(最低分)。
    /// WHY 不能用 >=:DPO 训练要求偏好信号非空,相等分数无训练价值。
    #[test]
    fn prop_generate_chosen_strictly_greater_than_rejected(
        // 使用 (0..1000) 的向量,每个元素代表一个 score(毫秒精度)
        scores_milli in prop::collection::vec(0u32..=1000, 2..=10),
    ) {
        let generator = PreferencePairGenerator::new(AutoDpoConfig::default())
            .expect("默认配置应有效");
        let outputs: Vec<ModelOutput> = scores_milli
            .iter()
            .map(|&s| ModelOutput::new(format!("out-{s}"), s as f32 / 1000.0))
            .collect();

        let result = generator.generate(&outputs);

        // 只有当最高分 >= 0.5(质量阈值)且最高分 != 最低分时,generate 才成功
        let max_score = scores_milli.iter().copied().max().unwrap_or(0) as f32 / 1000.0;
        let min_score = scores_milli.iter().copied().min().unwrap_or(0) as f32 / 1000.0;

        if max_score >= 0.5 && max_score > min_score {
            // 应成功生成
            let pair = result.expect("应成功生成偏好对");
            prop_assert!(
                pair.chosen_score > pair.rejected_score,
                "chosen_score={} 必须严格 > rejected_score={}",
                pair.chosen_score,
                pair.rejected_score
            );
            prop_assert!(
                pair.score_gap() > 0.0,
                "score_gap 必须为正,实际 {}",
                pair.score_gap()
            );
        } else {
            // 应失败(max_score < 0.5 → QualityTooLow,或 max == min → GenerationFailed)
            prop_assert!(
                result.is_err(),
                "max_score={} < 0.5 或 max==min={} 应导致 generate 失败",
                max_score,
                min_score
            );
        }
    }
}

/// 辅助测试:NaN 输入映射为 0.0(非属性测试,因 NaN 无法由 proptest 策略生成)
///
/// WHY 独立测试:NaN != NaN,无法在 proptest 策略中生成,
/// 但 ModelOutput::new 显式处理 NaN,需验证此边界。
/// 此为单元测试(1 case),非 proptest 属性测试(256 cases)。
#[test]
fn prop_model_output_nan_becomes_zero() {
    let output = ModelOutput::new("nan-test", f32::NAN);
    assert!(
        (output.score - 0.0).abs() < 1e-6,
        "NaN 应映射为 0.0,实际 {}",
        output.score
    );
    assert_eq!(
        output.quality,
        SampleQuality::Low,
        "NaN → 0.0 应为 Low 质量"
    );
}
