//! AuditChain pre-execution audit 模式测试 — Task I-3 [N5]
//!
//! 对应漏洞:N5 AuditChain 后置记录
//! 修复目标:执行前记录 Intent → 执行 → 更新为 Executed/Failed;
//!         append 失败必须返回 Err 阻止命令执行。
//!
//! TDD 流程:本文件先写(RED),实现 audit.rs 改造后转 GREEN。

use std::collections::HashMap;
use std::time::Duration;

use seccore::{AuditChain, AuditRecordStatus, CommandSpec, ExecutionResult, RiskLevel};

/// 构造测试用命令规格(与 audit.rs 内部测试一致)。
fn make_spec() -> CommandSpec {
    CommandSpec {
        program: "echo".to_string(),
        allowed_args: vec!["hello".to_string()],
        env_whitelist: HashMap::new(),
        risk_level: RiskLevel::Low,
    }
}

/// 构造测试用执行结果。
fn make_result() -> ExecutionResult {
    ExecutionResult {
        exit_code: 0,
        stdout: "hello\n".to_string(),
        stderr: String::new(),
        duration: Duration::from_millis(10),
        audit_hash: "0".repeat(64),
    }
}

// =============================================================================
// N5 修复核心测试 1:pre-execution append 完整流程
// =============================================================================
// 验证:执行前追加 Intent 记录 → 执行后更新为 Executed → 审计链完整
// 修复前:audit.rs 只有 append(command, result),不支持分阶段记录

#[test]
fn test_audit_chain_pre_execution_append() {
    let mut chain = AuditChain::new();
    let spec = make_spec();

    // === 阶段 1:执行前记录意图(pre-execution)===
    // WHY: 在命令执行前先写入 Intent 记录,确保即使执行中崩溃也有意图痕迹。
    //      这是修复 N5 漏洞的核心:执行前 append,append 失败则阻止执行。
    let record_id = chain
        .append_intent(&spec)
        .expect("append_intent 应成功返回 RecordId");
    assert_eq!(record_id, 0, "首条记录 ID 应为 0(链索引)");
    assert_eq!(chain.len(), 1, "Intent 记录后链长应为 1");

    // 验证 Intent 状态:result_hash 为空占位,等待执行后填充
    assert_eq!(
        chain.blocks[0].status,
        AuditRecordStatus::Intent,
        "执行前状态应为 Intent"
    );
    assert!(
        chain.blocks[0].result_hash.is_empty(),
        "Intent 状态 result_hash 应为空占位"
    );

    // Intent 阶段审计链也应完整(merkle_root 计算包含 status)
    assert!(chain.verify().unwrap(), "Intent 阶段审计链应完整");

    // === 阶段 2:执行后更新为 Executed ===
    let result = make_result();
    chain
        .update_status(record_id, AuditRecordStatus::Executed, Some(&result))
        .expect("update_status 应成功");

    assert_eq!(
        chain.blocks[0].status,
        AuditRecordStatus::Executed,
        "执行后状态应更新为 Executed"
    );
    assert!(
        !chain.blocks[0].result_hash.is_empty(),
        "Executed 状态 result_hash 应已填充"
    );

    // 完整性验证:更新后审计链仍然完整
    assert!(
        chain.verify().unwrap(),
        "pre-execution audit 完成后审计链应完整"
    );
}

// =============================================================================
// N5 修复核心测试 2:append 失败阻止执行
// =============================================================================
// 验证:当审计记录阶段失败时,调用方应能感知并阻止命令执行。
// 设计:append_intent 返回 Result<RecordId, SecCoreError>;
//      update_status 对无效 RecordId 返回 Err(API 契约:必须先 append 才能 update)。
//      调用方用 `?` 短路:任何 Err 都阻止后续 execute。

#[test]
fn test_audit_chain_append_failure_blocks_execution() {
    let mut chain = AuditChain::new();

    // 场景 1:update_status 对未追加的 RecordId 返回 Err
    // WHY: 这是 pre-execution 模式的安全契约 — 必须先 append_intent 拿到有效 ID 才能更新。
    //      如果 append_intent 阶段失败(无有效 ID),调用方应短路阻止执行。
    let invalid_id: u64 = 99;
    let result = chain.update_status(invalid_id, AuditRecordStatus::Executed, None);
    assert!(
        result.is_err(),
        "对未追加的 RecordId 调用 update_status 应返回 Err,阻止继续执行"
    );

    // 场景 2:update_status 对已过期(非链尾)的 RecordId 返回 Err
    // WHY: update_status 只允许更新链尾块 — 中间块的 merkle_root 改变会破坏后续块 prev_hash 链。
    //      这确保 pre-execution 模式严格串行:append_intent → 立即执行 → 立即 update_status。
    let spec = make_spec();
    let id0 = chain.append_intent(&spec).expect("append_intent 应成功");
    let id1 = chain.append_intent(&spec).expect("append_intent 应成功");

    // 尝试更新 id0(非链尾),应失败
    let result = chain.update_status(id0, AuditRecordStatus::Executed, Some(&make_result()));
    assert!(
        result.is_err(),
        "更新非链尾块的 RecordId 应返回 Err,防止破坏 merkle 链"
    );

    // 更新 id1(链尾),应成功
    let result = chain.update_status(id1, AuditRecordStatus::Executed, Some(&make_result()));
    assert!(
        result.is_ok(),
        "更新链尾块的 RecordId 应成功: {:?}",
        result.err()
    );

    // 场景 3:Failed 状态路径 — 执行失败时记录 Failed 状态
    // WHY: 执行失败也要更新审计链,保持完整痕迹(不能让 Intent 永远悬挂)。
    let id2 = chain.append_intent(&spec).expect("append_intent 应成功");
    chain
        .update_status(id2, AuditRecordStatus::Failed, None)
        .expect("Failed 状态更新应成功");
    assert_eq!(
        chain.blocks[id2 as usize].status,
        AuditRecordStatus::Failed,
        "失败执行应记录 Failed 状态"
    );
    assert!(chain.verify().unwrap(), "含 Failed 记录的审计链应仍完整");

    // 验证 record_id 严格递增
    assert_eq!(id0, 0);
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);
}

// =============================================================================
// 兼容性测试:既有 append() 方法保留向后兼容
// =============================================================================

#[test]
fn test_audit_chain_legacy_append_still_works() {
    // WHY: 保留 append() 向后兼容,内部委托 append_intent + update_status(Executed)
    //      避免破坏既有调用点(如 sandbox.rs 原有流程、security.rs 测试)。
    let mut chain = AuditChain::new();
    let spec = make_spec();
    let result = make_result();

    chain.append(&spec, &result).expect("legacy append 应成功");
    assert_eq!(chain.len(), 1);
    assert_eq!(
        chain.blocks[0].status,
        AuditRecordStatus::Executed,
        "legacy append 应直接写入 Executed 状态"
    );
    assert!(chain.verify().unwrap(), "legacy append 后审计链应完整");
}
