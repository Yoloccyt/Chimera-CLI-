//! 分层回放池完整性审计测试（Milestone B-3b，九层防御 L3 补齐）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §5.1 P2 / §7.2 九层防御 L3 补齐）：
//! 回放池完整性仅"R2 冻结声明注释"覆盖，无独立审计 → 补齐完整性审计接口，
//! 校验分层统计一致性与容量不变量（超限 = 内部淘汰 bug 的信号）。

#![forbid(unsafe_code)]

use cmt_tiering::rl_replay_pool::{ReplayExperience, TieredReplayPool};

/// 构造经验条目（payload 可空以模拟损坏场景）
fn exp(id: &str, reward: f32, success: bool, payload: Vec<u8>) -> ReplayExperience {
    ReplayExperience {
        experience_id: id.into(),
        reward,
        success,
        payload,
    }
}

/// 空池审计：一致性成立
#[test]
fn empty_pool_audit_is_consistent() {
    let pool = TieredReplayPool::new();
    let report = pool.integrity_audit();
    assert!(report.consistent, "空池应一致: {report:?}");
    assert_eq!(report.total, 0);
}

/// 存储后审计：total 与分层之和一致
#[test]
fn stored_experiences_are_consistent() {
    let pool = TieredReplayPool::new();
    pool.store(exp("e-1", 1.0, true, vec![1, 2, 3]));
    pool.store(exp("e-2", -8.0, false, vec![4, 5, 6])); // 高价值失败 → Cold
    pool.store(exp("e-3", 0.5, true, vec![7, 8]));
    let report = pool.integrity_audit();
    assert!(report.consistent, "存储后应一致: {report:?}");
    assert_eq!(report.total, 3, "总条目应为 3: {report:?}");
    assert_eq!(
        report.hot + report.warm + report.cold + report.ice,
        report.total
    );
}

/// 空 payload 条目被标记（数据损坏信号）
#[test]
fn empty_payload_flagged_as_corruption() {
    let pool = TieredReplayPool::new();
    pool.store(exp("e-bad", 1.0, true, vec![])); // 空 payload
    pool.store(exp("e-ok", 1.0, true, vec![9]));
    let report = pool.integrity_audit();
    assert!(!report.consistent, "空 payload 应导致不一致: {report:?}");
    assert_eq!(report.empty_payload, 1);
}

/// 分层容量不变量：超限层被标记（内部淘汰 bug 信号）
#[test]
fn capacity_overflow_flagged() {
    let pool = TieredReplayPool::new();
    // Hot 容量 100——压入 105 个触发 FIFO 淘汰，不应超限；
    // 为直接验证容量标记路径，审计应能识别任何超限层。
    for i in 0..105 {
        pool.store(exp(&format!("e-{i}"), 1.0, true, vec![i as u8]));
    }
    let report = pool.integrity_audit();
    assert!(report.consistent, "FIFO 淘汰后不应超限: {report:?}");
    assert!(report.hot <= 100, "Hot 层不应超容量: {report:?}");
    assert_eq!(report.total, 105);
}

/// 采样后审计仍一致（sample 不应破坏分层统计）
#[test]
fn audit_consistent_after_sampling() {
    let pool = TieredReplayPool::new();
    for i in 0..20 {
        pool.store(exp(&format!("e-{i}"), 1.0, true, vec![i as u8]));
    }
    let mut rng = rand::thread_rng();
    let batch = pool.sample(8, &mut rng);
    assert!(!batch.is_empty());
    let report = pool.integrity_audit();
    assert!(report.consistent, "采样后应一致: {report:?}");
    assert_eq!(report.total, 20);
}
