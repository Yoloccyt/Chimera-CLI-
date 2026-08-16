//! P1-3(计划 Task 4):CheckpointPreserver 保留历史最佳测试(RSIBench,文档 §10.3.3)
//!
//! 覆盖:
//! - 首次 checkpoint 直接保留为最佳
//! - 更高分替换 / 更低分保留旧最佳
//! - 停止策略:attempts > 10 且有最佳 → Stop(RSIBench 78.26% 发现)
//! - 多 task_type 隔离

use gsoe_evolution::checkpoint_preserver::{
    Checkpoint, CheckpointPreserver, PreserveDecision, StopDecision,
};

/// 首次 checkpoint 直接保留为最佳
#[test]
fn test_first_checkpoint_keep_as_best() {
    let mut preserver = CheckpointPreserver::new();
    let cp = Checkpoint::new("bugfix", 0.8, "首轮搜索");

    let decision = preserver.evaluate(&cp);
    assert_eq!(decision, PreserveDecision::KeepAsBest);
    assert_eq!(
        preserver.best_checkpoints().get("bugfix").unwrap().score,
        0.8
    );
}

/// 更高分 checkpoint 替换最佳
#[test]
fn test_higher_score_replaces_best() {
    let mut preserver = CheckpointPreserver::new();
    preserver.evaluate(&Checkpoint::new("bugfix", 0.8, "v1"));
    let decision = preserver.evaluate(&Checkpoint::new("bugfix", 0.9, "v2"));

    assert_eq!(decision, PreserveDecision::ReplaceBest);
    assert_eq!(
        preserver.best_checkpoints().get("bugfix").unwrap().score,
        0.9
    );
}

/// 更低分 checkpoint 保留旧最佳(RSIBench 核心场景)
#[test]
fn test_lower_score_keeps_old_best() {
    let mut preserver = CheckpointPreserver::new();
    preserver.evaluate(&Checkpoint::new("bugfix", 0.9, "历史峰值"));
    let decision = preserver.evaluate(&Checkpoint::new("bugfix", 0.7, "后续搜索"));

    assert_eq!(decision, PreserveDecision::KeepOldBest);
    assert_eq!(
        preserver.best_checkpoints().get("bugfix").unwrap().score,
        0.9
    );
}

/// 分数相等不替换(严格大于才替换,避免无意义抖动)
#[test]
fn test_equal_score_keeps_old_best() {
    let mut preserver = CheckpointPreserver::new();
    preserver.evaluate(&Checkpoint::new("bugfix", 0.8, "v1"));
    let decision = preserver.evaluate(&Checkpoint::new("bugfix", 0.8, "v1-copy"));

    assert_eq!(decision, PreserveDecision::KeepOldBest);
    assert_eq!(
        preserver.best_checkpoints().get("bugfix").unwrap().metadata,
        "v1"
    );
}

/// 无最佳时永不停止
#[test]
fn test_should_stop_never_without_best() {
    let preserver = CheckpointPreserver::new();
    let decision = preserver.should_stop("bugfix", 100);
    assert_eq!(decision, StopDecision::Continue);
}

/// 有最佳但尝试次数 ≤ 10 时继续
#[test]
fn test_should_stop_continue_within_attempts() {
    let mut preserver = CheckpointPreserver::new();
    preserver.evaluate(&Checkpoint::new("bugfix", 0.9, "v1"));
    let decision = preserver.should_stop("bugfix", 10);
    assert_eq!(decision, StopDecision::Continue);
}

/// attempts > 10 且有最佳 → Stop(返回最佳 checkpoint)
#[test]
fn test_should_stop_after_max_attempts() {
    let mut preserver = CheckpointPreserver::new();
    preserver.evaluate(&Checkpoint::new("bugfix", 0.9, "历史峰值"));

    let decision = preserver.should_stop("bugfix", 11);
    match decision {
        StopDecision::Stop { reason, selected } => {
            assert!(reason.contains("attempts"));
            assert_eq!(selected.score, 0.9);
            assert_eq!(selected.metadata, "历史峰值");
        }
        StopDecision::Continue => panic!("attempts > 10 且有最佳应停止"),
    }
}

/// 多 task_type 隔离:不同任务类型的最佳互不影响
#[test]
fn test_task_type_isolation() {
    let mut preserver = CheckpointPreserver::new();
    preserver.evaluate(&Checkpoint::new("bugfix", 0.9, "v1"));
    preserver.evaluate(&Checkpoint::new("refactor", 0.5, "v1"));

    // bugfix 有最佳,refactor 的最佳为 0.5
    assert_eq!(
        preserver.best_checkpoints().get("bugfix").unwrap().score,
        0.9
    );
    assert_eq!(
        preserver.best_checkpoints().get("refactor").unwrap().score,
        0.5
    );
    // bugfix 达到停止条件,refactor 未注册的其他类型仍继续
    assert!(matches!(
        preserver.should_stop("bugfix", 11),
        StopDecision::Stop { .. }
    ));
    assert_eq!(preserver.should_stop("unknown", 11), StopDecision::Continue);
}
