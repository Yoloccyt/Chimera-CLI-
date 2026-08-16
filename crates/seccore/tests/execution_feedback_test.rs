//! 六类状态反馈集成器集成测试 — 顶层 API + 分类优先级 + L0 协同（v3.4.0 §9.3）
//!
//! 覆盖: 顶层 API 可达性 / 六类状态全可达 / 分类优先级端到端 /
//! 与 L0 ExecutionStatus 纯函数协同（is_retryable/generates_meaningful_card）

#![forbid(unsafe_code)]

use nexus_contracts::experience_card::ExecutionStatus;
use seccore::ExecutionFeedbackIntegrator;

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    let status = ExecutionFeedbackIntegrator::classify(true, true, true, Some(0.9), false, None);
    assert_eq!(status, ExecutionStatus::Success);
}

// ----------------------------------------------------------
// 六类状态全可达（铁律8 全覆盖）
// ----------------------------------------------------------

#[test]
fn all_six_statuses_end_to_end() {
    let integrator_outputs = [
        ExecutionFeedbackIntegrator::classify(true, true, true, Some(1.0), true, None),
        ExecutionFeedbackIntegrator::classify(false, true, true, None, false, Some("err")),
        ExecutionFeedbackIntegrator::classify(true, true, false, Some(0.5), false, None),
        ExecutionFeedbackIntegrator::classify(true, false, true, Some(0.5), false, None),
        ExecutionFeedbackIntegrator::classify(true, true, true, None, false, None),
        ExecutionFeedbackIntegrator::classify(true, true, true, Some(0.9), false, None),
    ];
    let expected = [
        ExecutionStatus::Timeout,
        ExecutionStatus::Error,
        ExecutionStatus::NoSubmit,
        ExecutionStatus::MissingCode,
        ExecutionStatus::ScoreFailed,
        ExecutionStatus::Success,
    ];
    for (actual, exp) in integrator_outputs.iter().zip(expected.iter()) {
        assert_eq!(actual, exp);
    }
}

// ----------------------------------------------------------
// 分类优先级端到端
// ----------------------------------------------------------

#[test]
fn priority_timeout_beats_all() {
    // Timeout 优先于 Error/NoSubmit/MissingCode/Success
    for error_output in [Some("err"), None] {
        for success in [true, false] {
            let status = ExecutionFeedbackIntegrator::classify(
                success,
                true,
                true,
                Some(1.0),
                true,
                error_output,
            );
            assert_eq!(status, ExecutionStatus::Timeout);
        }
    }
}

#[test]
fn priority_error_beats_no_submit() {
    // !success + error_output → Error（优先于 NoSubmit/MissingCode）
    let status = ExecutionFeedbackIntegrator::classify(
        false,
        false,
        false,
        None,
        false,
        Some("compilation error"),
    );
    assert_eq!(status, ExecutionStatus::Error);
}

// ----------------------------------------------------------
// 与 L0 ExecutionStatus 纯函数协同
// ----------------------------------------------------------

#[test]
fn classified_status_integrates_with_l0_pure_functions() {
    // 分类结果可直接调用 L0 ExecutionStatus 的纯函数（铁律8 全链路）
    let error = ExecutionFeedbackIntegrator::classify(false, true, true, None, false, Some("err"));
    assert!(error.is_retryable(), "Error 应可重试");
    assert!(error.generates_meaningful_card(), "Error 应生成有意义卡片");

    let timeout = ExecutionFeedbackIntegrator::classify(true, true, true, Some(1.0), true, None);
    assert!(timeout.is_retryable(), "Timeout 应可重试");
    assert!(
        timeout.generates_meaningful_card(),
        "Timeout 应生成有意义卡片"
    );

    let success = ExecutionFeedbackIntegrator::classify(true, true, true, Some(0.9), false, None);
    assert!(!success.is_retryable(), "Success 不可重试");
    assert!(
        success.generates_meaningful_card(),
        "Success 生成有意义卡片"
    );

    let missing_code =
        ExecutionFeedbackIntegrator::classify(true, false, true, Some(0.5), false, None);
    assert!(
        !missing_code.generates_meaningful_card(),
        "MissingCode 不生成有意义卡片"
    );
}

#[test]
fn classify_score_boundary_zero_and_one() {
    // score 边界值 0.0 与 1.0 都算已评分（Some）→ Success
    let zero = ExecutionFeedbackIntegrator::classify(true, true, true, Some(0.0), false, None);
    assert_eq!(zero, ExecutionStatus::Success);
    let one = ExecutionFeedbackIntegrator::classify(true, true, true, Some(1.0), false, None);
    assert_eq!(one, ExecutionStatus::Success);
}
