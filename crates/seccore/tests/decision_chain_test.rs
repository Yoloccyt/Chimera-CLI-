//! Merkle 审计链决策链全覆盖 TDD 测试 — P1-W3.3
//!
//! 对应文档:
//! - spec.md:206 "决策链(提案→辩论→自白→执行→结果)全量上 Merkle 审计链,支持事后完整重放"
//! - tasks.md P1-W3.3
//!
//! 核心契约:
//! - 高危操作(risk_score ∈ [71,100])的完整决策链 MUST 全量上 Merkle 审计链
//! - 决策链纳入 merkle_root 计算,篡改任意步骤被 verify() 检测
//! - 支持事后完整重放(replay_decision_chain)
//! - 向后兼容:append_intent 保留,空决策链与非高危操作不受影响
//!
//! TDD 流程:本文件先写(RED),实现 audit.rs 改造后转 GREEN。

use std::collections::HashMap;
use std::time::Duration;

use seccore::{
    AuditChain, AuditRecordStatus, AuditResult, Command, CommandPolicy, DecisionChainBuilder,
    DecisionStepType, EnvPolicy, EscalationHandler, ExecutionResult, InterventionAction, RiskLevel,
    Sandbox, SecCoreError,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// =============================================================================
// 辅助构造函数
// =============================================================================

/// 构造测试用命令规格(低风险,ReadOnly 档)。
fn make_spec() -> seccore::CommandSpec {
    seccore::CommandSpec {
        program: "echo".to_string(),
        allowed_args: vec!["hello".to_string()],
        env_whitelist: HashMap::new(),
        risk_level: RiskLevel::Low,
        risk_score: 10,
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

/// 构造测试用 ASA 审计结果(Allow 档)。
fn make_asa_result_allow() -> AuditResult {
    AuditResult {
        safety_score: 0.9,
        correctness_score: 1.0,
        efficiency_score: 0.8,
        intervention: InterventionAction::Allow,
        audit_reason: "safe operation".to_string(),
        risk_level: RiskLevel::Low,
    }
}

// =============================================================================
// Test 1: DecisionStepType 序列化/反序列化
// =============================================================================
// 验证 6 种步骤类型可正确序列化与反序列化,确保审计链持久化兼容。

#[test]
fn test_decision_step_type_serialization() {
    let steps = vec![
        DecisionStepType::Proposal,
        DecisionStepType::AsaAudit,
        DecisionStepType::Debate,
        DecisionStepType::Confession,
        DecisionStepType::Execution,
        DecisionStepType::Result,
    ];

    for step in &steps {
        let json = serde_json::to_string(step).expect("序列化应成功");
        let deserialized: DecisionStepType = serde_json::from_str(&json).expect("反序列化应成功");
        assert_eq!(*step, deserialized, "序列化往返应保持一致");
    }

    // 验证 6 种类型互不相等
    for (i, a) in steps.iter().enumerate() {
        for (j, b) in steps.iter().enumerate() {
            if i != j {
                assert_ne!(*a, *b, "不同步骤类型应不相等");
            }
        }
    }
}

// =============================================================================
// Test 2: DecisionChainBuilder 完整决策链构建(6 步)
// =============================================================================
// 验证 builder 能按顺序收集 6 个步骤:Proposal → AsaAudit → Debate → Confession → Execution → Result

#[test]
fn test_decision_chain_builder_full_flow() {
    let spec = make_spec();
    let asa_result = make_asa_result_allow();

    let chain = DecisionChainBuilder::new()
        .add_proposal(&spec)
        .add_asa_audit(&asa_result)
        .add_debate(true)
        .add_confession("rm -f 临时文件清理")
        .add_execution()
        .add_result(0)
        .build();

    assert_eq!(chain.len(), 6, "完整决策链应有 6 个步骤");

    assert_eq!(chain[0].step_type, DecisionStepType::Proposal);
    assert_eq!(chain[1].step_type, DecisionStepType::AsaAudit);
    assert_eq!(chain[2].step_type, DecisionStepType::Debate);
    assert_eq!(chain[3].step_type, DecisionStepType::Confession);
    assert_eq!(chain[4].step_type, DecisionStepType::Execution);
    assert_eq!(chain[5].step_type, DecisionStepType::Result);

    // 每个步骤的 step_hash 不应为空(SHA-256 十六进制)
    for (i, step) in chain.iter().enumerate() {
        assert!(!step.step_hash.is_empty(), "步骤 {i} 的 step_hash 不应为空");
        assert_eq!(
            step.step_hash.len(),
            64,
            "步骤 {i} 的 step_hash 应为 SHA-256 十六进制(64 字符)"
        );
    }

    // 验证 outcome 内容
    assert_eq!(
        chain[2].outcome, "approved",
        "Debate 步骤 outcome 应为 approved"
    );
    assert_eq!(
        chain[5].outcome, "exit_code=0",
        "Result 步骤 outcome 应为 exit_code=0"
    );

    // 时间戳应单调非递减
    for i in 1..chain.len() {
        assert!(
            chain[i].timestamp >= chain[i - 1].timestamp,
            "时间戳应单调非递减(步骤 {i})"
        );
    }
}

// =============================================================================
// Test 3: append_intent_with_chain + extend_decision_chain 完整流程
// =============================================================================
// 验证:执行前 append 带决策链的 Intent → 执行后 extend 补充 Execution/Result → 审计链完整

#[test]
fn test_append_intent_with_chain() {
    let mut chain = AuditChain::new();
    let spec = make_spec();
    let asa_result = make_asa_result_allow();

    // 阶段 1:执行前构建 pre-execution 决策链(Proposal → AsaAudit → Debate → Confession)
    let pre_chain = DecisionChainBuilder::new()
        .add_proposal(&spec)
        .add_asa_audit(&asa_result)
        .add_debate(true)
        .add_confession("高危操作意图披露")
        .build();

    let record_id = chain
        .append_intent_with_chain(&spec, pre_chain)
        .expect("append_intent_with_chain 应成功");

    assert_eq!(record_id, 0, "首条记录 ID 应为 0");
    assert_eq!(chain.len(), 1, "Intent 记录后链长应为 1");

    // 验证 decision_chain 已存储(4 个 pre-execution 步骤)
    assert_eq!(
        chain.blocks[0].decision_chain.len(),
        4,
        "pre-execution 决策链应有 4 个步骤"
    );
    assert_eq!(
        chain.blocks[0].status,
        AuditRecordStatus::Intent,
        "执行前状态应为 Intent"
    );

    // Intent 阶段审计链应完整(merkle_root 含 decision_chain)
    assert!(chain.verify().unwrap(), "Intent 阶段(带决策链)审计链应完整");

    // 阶段 2:执行后补充 Execution + Result 步骤(extend_decision_chain)
    let post_chain = DecisionChainBuilder::new()
        .add_execution()
        .add_result(0)
        .build();

    chain
        .extend_decision_chain(record_id, post_chain)
        .expect("extend_decision_chain 应成功");

    // 验证 decision_chain 现有 6 个步骤(4 pre + 2 post)
    assert_eq!(
        chain.blocks[0].decision_chain.len(),
        6,
        "完整决策链应有 6 个步骤(4 pre + 2 post)"
    );

    // extend 后审计链仍应完整(merkle_root 已重算)
    assert!(
        chain.verify().unwrap(),
        "extend_decision_chain 后审计链应完整"
    );

    // 阶段 3:更新为 Executed 状态
    let result = make_result();
    chain
        .update_status(record_id, AuditRecordStatus::Executed, Some(&result))
        .expect("update_status 应成功");

    assert_eq!(
        chain.blocks[0].status,
        AuditRecordStatus::Executed,
        "执行后状态应更新为 Executed"
    );
    assert!(chain.verify().unwrap(), "完整流程后审计链应完整");
}

// =============================================================================
// Test 4: 决策链篡改被 merkle_root 检测
// =============================================================================
// 验证:篡改 decision_chain 内容(如删除步骤)后 verify() 返回 false

#[test]
fn test_decision_chain_in_merkle_root() {
    let mut chain = AuditChain::new();
    let spec = make_spec();

    let decision_chain = DecisionChainBuilder::new()
        .add_proposal(&spec)
        .add_debate(true)
        .add_confession("test intent")
        .build();

    chain
        .append_intent_with_chain(&spec, decision_chain)
        .expect("append 应成功");

    // 篡改前:审计链完整
    assert!(chain.verify().unwrap(), "篡改前审计链应完整");

    // 篡改:删除一个决策步骤(改变 decision_chain 内容)
    chain.blocks[0].decision_chain.pop();

    // 篡改后:merkle_root 重算时应检测到 decision_chain 变更
    assert!(
        !chain.verify().unwrap(),
        "篡改 decision_chain(删除步骤)应被 merkle_root 检测"
    );
}

// =============================================================================
// Test 5: replay_decision_chain 事后完整重放
// =============================================================================
// 验证:通过 RecordId 重放决策链,返回完整步骤切片

#[test]
fn test_replay_decision_chain() {
    let mut chain = AuditChain::new();
    let spec = make_spec();
    let asa_result = make_asa_result_allow();

    // 构建完整 6 步决策链
    let pre_chain = DecisionChainBuilder::new()
        .add_proposal(&spec)
        .add_asa_audit(&asa_result)
        .add_debate(true)
        .add_confession("重放测试意图")
        .build();

    let id = chain
        .append_intent_with_chain(&spec, pre_chain)
        .expect("append 应成功");

    let post_chain = DecisionChainBuilder::new()
        .add_execution()
        .add_result(0)
        .build();
    chain
        .extend_decision_chain(id, post_chain)
        .expect("extend 应成功");

    // 重放:通过 RecordId 获取完整决策链
    let replayed = chain.replay_decision_chain(id).expect("replay 应成功");

    assert_eq!(replayed.len(), 6, "重放应返回完整 6 步决策链");

    // 验证步骤类型顺序
    assert_eq!(replayed[0].step_type, DecisionStepType::Proposal);
    assert_eq!(replayed[1].step_type, DecisionStepType::AsaAudit);
    assert_eq!(replayed[2].step_type, DecisionStepType::Debate);
    assert_eq!(replayed[3].step_type, DecisionStepType::Confession);
    assert_eq!(replayed[4].step_type, DecisionStepType::Execution);
    assert_eq!(replayed[5].step_type, DecisionStepType::Result);

    // 重放不存在的 RecordId 应返回 Err
    let invalid = chain.replay_decision_chain(999);
    assert!(invalid.is_err(), "重放不存在的 RecordId 应返回 Err");
}

// =============================================================================
// Test 6: 空决策链向后兼容
// =============================================================================
// 验证:append_intent(委托 append_intent_with_chain 传空 Vec)行为与旧实现一致

#[test]
fn test_empty_decision_chain_backward_compat() {
    let mut chain = AuditChain::new();
    let spec = make_spec();

    // 旧 API:append_intent 不传决策链
    let id = chain.append_intent(&spec).expect("append_intent 应成功");

    // 空决策链
    assert!(
        chain.blocks[id as usize].decision_chain.is_empty(),
        "append_intent 应产生空决策链(向后兼容)"
    );

    // 审计链完整
    assert!(chain.verify().unwrap(), "空决策链的审计链应完整(向后兼容)");

    // 旧 API append 也应工作
    let result = make_result();
    chain.append(&spec, &result).expect("legacy append 应成功");
    assert!(chain.verify().unwrap(), "legacy append 后审计链应完整");

    // replay 空决策链应返回空切片
    let replayed = chain.replay_decision_chain(id).expect("replay 应成功");
    assert!(replayed.is_empty(), "空决策链重放应返回空切片");
}

// =============================================================================
// Test 7: EscalateToHuman 档决策链(仅 Proposal + Result rejected)
// =============================================================================
// 验证:risk_score ∈ [91,100] 的操作被拒绝时,审计链记录 Proposal + Result(rejected)

struct ApprovingHandler;
impl EscalationHandler for ApprovingHandler {
    fn parliament_debate(
        &self,
        _spec: &seccore::CommandSpec,
        _risk_score: u8,
    ) -> Result<(), SecCoreError> {
        Ok(())
    }
}

#[tokio::test]
async fn test_escalate_to_human_decision_chain() {
    let policy = CommandPolicy::new().allow_command("dd");
    let env_policy = EnvPolicy::default_secure();
    let mut sandbox =
        Sandbox::new(policy, env_policy).with_escalation_handler(Box::new(ApprovingHandler));

    // dd → risk_score=95 (EscalateToHuman)
    let cmd = Command::new("dd").arg("if=/dev/zero");
    let result = sandbox.audit_and_execute(cmd).await;

    // 应返回 EscalateToHuman 错误
    assert!(
        matches!(result, Err(SecCoreError::EscalateToHuman { risk_score, .. }) if risk_score >= 91),
        "EscalateToHuman 档应返回 EscalateToHuman 错误"
    );

    // 审计链应有 1 条记录(拒绝意图也上链)
    assert_eq!(
        sandbox.audit_chain.len(),
        1,
        "EscalateToHuman 拒绝应在审计链记录 1 条决策链"
    );

    // 决策链应有 2 步:Proposal + Result(rejected)
    let block = &sandbox.audit_chain.blocks[0];
    assert_eq!(
        block.decision_chain.len(),
        2,
        "EscalateToHuman 决策链应有 2 步(Proposal + Result rejected)"
    );
    assert_eq!(
        block.decision_chain[0].step_type,
        DecisionStepType::Proposal
    );
    assert_eq!(block.decision_chain[1].step_type, DecisionStepType::Result);
    assert!(
        block.decision_chain[1].outcome.contains("rejected"),
        "Result 步骤 outcome 应含 'rejected', got: {}",
        block.decision_chain[1].outcome
    );

    // 审计链应完整(拒绝操作的决策链也纳入 merkle_root)
    assert!(
        sandbox.audit_chain.verify().unwrap(),
        "EscalateToHuman 拒绝后审计链应完整"
    );
}

// =============================================================================
// Test 8: Parliament 档完整决策链集成
// =============================================================================
// 验证:risk_score ∈ [71,90] 的操作经完整流程后,审计链含 6 步决策链(或 4 步无 ASA)

struct RecordingApprovingHandler {
    called: Arc<AtomicBool>,
}
impl EscalationHandler for RecordingApprovingHandler {
    fn parliament_debate(
        &self,
        _spec: &seccore::CommandSpec,
        _risk_score: u8,
    ) -> Result<(), SecCoreError> {
        self.called.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn test_parliament_tier_decision_chain_integration() {
    let policy = CommandPolicy::new().allow_command("rm");
    let env_policy = EnvPolicy::default_secure();
    let called = Arc::new(AtomicBool::new(false));
    let handler = RecordingApprovingHandler {
        called: called.clone(),
    };
    let mut sandbox = Sandbox::new(policy, env_policy).with_escalation_handler(Box::new(handler));

    // rm → risk_score=80 (Parliament),无 ASA 配置 → 决策链 5 步(无 AsaAudit)
    // Proposal → Debate → Confession → Execution → Result
    let cmd = Command::new("rm").arg("-f");
    let result = sandbox.audit_and_execute(cmd).await;

    // handler 应被调用
    assert!(
        called.load(Ordering::SeqCst),
        "Parliament 档应调用 EscalationHandler"
    );

    // 不应返回 EscalateToHuman 或 PolicyViolation
    assert!(
        !matches!(result, Err(SecCoreError::EscalateToHuman { .. })),
        "Parliament 批准的操作不应返回 EscalateToHuman"
    );

    // 审计链应有 1 条记录
    assert_eq!(
        sandbox.audit_chain.len(),
        1,
        "Parliament 档应在审计链记录 1 条决策链"
    );

    let block = &sandbox.audit_chain.blocks[0];

    // 无 ASA 时决策链应有 5 步(Proposal + Debate + Confession + Execution + Result)
    assert_eq!(
        block.decision_chain.len(),
        5,
        "Parliament 档(无 ASA)决策链应有 5 步, got: {}",
        block.decision_chain.len()
    );

    // 验证步骤类型
    assert_eq!(
        block.decision_chain[0].step_type,
        DecisionStepType::Proposal
    );
    assert_eq!(block.decision_chain[1].step_type, DecisionStepType::Debate);
    assert_eq!(
        block.decision_chain[2].step_type,
        DecisionStepType::Confession
    );
    assert_eq!(
        block.decision_chain[3].step_type,
        DecisionStepType::Execution
    );
    assert_eq!(block.decision_chain[4].step_type, DecisionStepType::Result);

    // 审计链应完整
    assert!(
        sandbox.audit_chain.verify().unwrap(),
        "Parliament 档完整决策链后审计链应完整"
    );

    // replay 应返回完整决策链
    let replayed = sandbox
        .audit_chain
        .replay_decision_chain(0)
        .expect("replay 应成功");
    assert_eq!(replayed.len(), 5, "重放应返回完整 5 步决策链");
}

// =============================================================================
// Test 9: 篡改 step_hash 被检测
// =============================================================================
// 验证:篡改决策链步骤的 step_hash 后 verify() 返回 false

#[test]
fn test_decision_chain_tamper_step_hash() {
    let mut chain = AuditChain::new();
    let spec = make_spec();

    let decision_chain = DecisionChainBuilder::new()
        .add_proposal(&spec)
        .add_debate(true)
        .build();

    chain
        .append_intent_with_chain(&spec, decision_chain)
        .expect("append 应成功");

    assert!(chain.verify().unwrap(), "篡改前审计链应完整");

    // 篡改:修改第一个步骤的 step_hash
    chain.blocks[0].decision_chain[0].step_hash = "f".repeat(64);

    assert!(
        !chain.verify().unwrap(),
        "篡改 step_hash 应被 merkle_root 检测"
    );
}

// =============================================================================
// Test 10: 篡改 outcome 被检测
// =============================================================================
// 验证:篡改决策链步骤的 outcome 后 verify() 返回 false

#[test]
fn test_decision_chain_tamper_outcome() {
    let mut chain = AuditChain::new();
    let spec = make_spec();

    let decision_chain = DecisionChainBuilder::new()
        .add_proposal(&spec)
        .add_debate(true)
        .add_confession("原始意图")
        .build();

    chain
        .append_intent_with_chain(&spec, decision_chain)
        .expect("append 应成功");

    assert!(chain.verify().unwrap(), "篡改前审计链应完整");

    // 篡改:修改 Confession 步骤的 outcome(模拟篡改自白内容)
    chain.blocks[0].decision_chain[2].outcome = "篡改后的意图".to_string();

    assert!(
        !chain.verify().unwrap(),
        "篡改 outcome 应被 merkle_root 检测"
    );
}
