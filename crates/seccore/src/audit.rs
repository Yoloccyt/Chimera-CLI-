//! SHA-256 Merkle 审计链 — 零信任沙箱的不可篡改执行日志
//!
//! 对应尸检教训:Claude Code 审计日志可被静默篡改,导致攻击无迹可循。
//!
//! 设计决策(WHY):
//! - **链式哈希**:每个块的 merkle_root 依赖前一块的哈希,形成单向链。
//!   篡改任意块会导致后续所有块的 prev_hash 不匹配,被 `verify` 检测。
//! - **独立计算**:审计链验证时重新计算 command_hash/result_hash,不信任
//!   存储的 audit_hash 字段,防止字段被篡改后绕过验证。
//! - **SHA-256**:抗碰撞,工业标准。使用 sha2 crate 的纯 Rust 实现。
//! - **Pre-execution audit (N5 修复)**:借鉴数据库 WAL 思想,执行前先写 Intent
//!   记录,执行后更新为 Executed/Failed。这样即使执行中崩溃或 append 失败,
//!   审计链仍保留意图痕迹,关闭"执行成功但 append 失败导致无痕"的漏洞窗口。

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::asa::AuditResult;
use crate::error::SecCoreError;
use crate::types::{CommandSpec, ExecutionResult};

/// 审计记录状态 — pre-execution audit 模式的状态机(N5 修复)。
///
/// WHY: 引入状态机让审计链能区分"意图已记录但未执行"与"已执行"两种状态,
///      消除后置 append 模式的漏洞窗口(执行成功但 append 失败时无痕)。
///
/// 状态流转:
/// - `Intent` → `Executed`(执行成功,result_hash 填充)
/// - `Intent` → `Failed`(执行失败或被拦截,result_hash 保持空占位)
///
/// status 字段纳入 merkle_root 计算,防止攻击者将 Intent 篡改为 Executed
/// 伪造执行证据。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditRecordStatus {
    /// 意图记录:命令执行前已记录意图,等待执行结果。
    /// result_hash 为空占位,执行后由 update_status 填充。
    Intent,
    /// 已执行:命令执行成功,result_hash 已填充。
    Executed,
    /// 执行失败:命令执行失败或被拦截,result_hash 保持空占位。
    Failed,
}

/// 审计记录 ID — pre-execution 模式下 append_intent 返回的记录定位符。
///
/// WHY: 调用方在 append_intent 后拿到 RecordId,执行命令后用同一 ID 调用
///      update_status 更新对应记录。ID 即块索引,严格递增。
pub type RecordId = u64;

/// 决策链步骤类型 — 高危操作决策链的各个阶段(P1-W3.3)。
///
/// WHY: spec.md:206 要求高危操作(risk_score ∈ [71,100])的完整决策链
/// (提案→辩论→自白→执行→结果)全量上 Merkle 审计链。每个步骤类型对应
/// 决策链的一个阶段,支持事后完整重放(replay_decision_chain)。
///
/// `#[repr(u8)]` 保证判别值固定(0-5),用于 `hash_decision_chain` 中
/// `step.step_type as u8` 的确定性哈希,避免默认表示的不可移植性。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum DecisionStepType {
    /// 提案:命令规格校验通过,risk_score 已确定。
    Proposal = 0,
    /// ASA 审计:AsaAuditor 前置实时审计结果(可选步骤,仅配置 ASA 时存在)。
    AsaAudit = 1,
    /// 辩论:Parliament 完整辩论结果(批准/否决)。
    Debate = 2,
    /// 自白:操作意图披露 + 风险确认。
    Confession = 3,
    /// 执行:沙箱执行启动。
    Execution = 4,
    /// 结果:执行结果记录(退出码或拒绝状态)。
    Result = 5,
}

/// 决策链步骤 — 单个决策记录(P1-W3.3)。
///
/// 每个步骤携带类型、时间戳、内容哈希与结果,所有字段纳入
/// `hash_decision_chain` 计算,防止篡改。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionStep {
    /// 步骤类型(决定该步骤在决策链中的语义位置)
    pub step_type: DecisionStepType,
    /// UTC 时间戳(秒,记录步骤发生时间)
    pub timestamp: i64,
    /// 步骤内容哈希(SHA-256 十六进制,64 字符,防篡改)
    ///
    /// WHY: 对步骤关键内容(命令规格/审计结果/辩论决议/自白文本/退出码)
    ///      计算 SHA-256,使任何内容变更可被 `verify()` 检测。
    pub step_hash: String,
    /// 步骤结果(人类可读,如 "approved"/"blocked"/"exit_code=0")
    ///
    /// WHY: 纳入 `hash_decision_chain` 计算,防止攻击者篡改决议结果
    ///      (如将 "rejected" 篡改为 "approved")。
    pub outcome: String,
}

/// 审计块 — 审计链中的单个记录,对应一次命令执行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditBlock {
    /// 块索引(从 0 开始,严格递增)
    pub index: u64,
    /// UTC 时间戳(秒)
    pub timestamp: i64,
    /// 命令哈希(SHA-256,程序名+参数+环境变量)
    pub command_hash: String,
    /// 结果哈希(SHA-256,退出码+stdout+stderr+duration)
    ///
    /// N5 修复:Intent 状态下为空字符串占位,Executed 状态下由 update_status 填充。
    pub result_hash: String,
    /// 前一块的 merkle_root(创世块为 64 个 '0')
    pub prev_hash: String,
    /// 本块的 Merkle 根(SHA-256(index||timestamp||command_hash||result_hash||prev_hash||status))
    pub merkle_root: String,
    /// 审计记录状态(N5 修复:pre-execution audit 状态机)
    ///
    /// WHY: 纳入 merkle_root 计算,防止 Intent 被篡改为 Executed 伪造执行证据。
    pub status: AuditRecordStatus,
    /// 决策链(P1-W3.3:高危操作决策链全量上链)。
    ///
    /// WHY: spec.md:206 要求高危操作的完整决策链(提案→辩论→自白→执行→结果)
    ///      全量上 Merkle 审计链。非高危操作(ReadOnly/Normal)为空 Vec;
    ///      高危操作(Parliament/EscalateToHuman)记录完整决策链。
    ///      `decision_chain` 纳入 `merkle_root` 计算(经 `hash_decision_chain`),
    ///      篡改任意步骤(step_hash/outcome/删除步骤)均被 `verify()` 检测。
    pub decision_chain: Vec<DecisionStep>,
}

/// 审计链 — 由 AuditBlock 组成的单向链表,支持完整性验证。
///
/// 链式结构:每个块的 prev_hash 指向前一块的 merkle_root,
/// 篡改任意块会导致链断裂,被 `verify` 检测。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditChain {
    /// 审计块列表(按追加顺序)
    pub blocks: Vec<AuditBlock>,
    /// 当前链尾哈希(最后一块的 merkle_root,空链为 64 个 '0')
    pub current_hash: String,
}

impl AuditChain {
    /// 创建空审计链(创世前驱哈希为 64 个 '0')。
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            current_hash: "0".repeat(64),
        }
    }

    /// Pre-execution append:命令执行前记录 Intent 状态的审计块(N5 修复)。
    ///
    /// WHY: 这是修复 N5 漏洞的核心 API。在命令执行前先写入 Intent 记录,
    ///      确保即使后续执行中崩溃或 update_status 失败,审计链仍保留
    ///      "曾尝试执行该命令"的意图痕迹。append_intent 失败时返回 Err,
    ///      调用方必须用 `?` 短路阻止命令执行。
    ///
    /// 典型流程:
    /// ```text
    /// let id = chain.append_intent(&spec)?;       // 执行前记录意图
    /// let result = execute(cmd).await;            // 执行命令
    /// match result {
    ///     Ok(r) => chain.update_status(id, AuditRecordStatus::Executed, Some(&r))?,
    ///     Err(_) => { let _ = chain.update_status(id, AuditRecordStatus::Failed, None); }
    /// }
    /// ```
    ///
    /// # 参数
    /// - `command`:校验通过的命令规格(已通过 policy::validate_command)
    ///
    /// # 返回
    /// - `Ok(RecordId)`:追加成功,返回记录 ID(即块索引,用于后续 update_status)
    /// - `Err(SecCoreError::AuditError)`:哈希计算失败(理论上不会发生)
    pub fn append_intent(&mut self, command: &CommandSpec) -> Result<RecordId, SecCoreError> {
        // WHY(P1-W3.3 向后兼容): 委托 append_intent_with_chain 传空决策链,
        //      保持旧调用点(sandbox.rs 原有流程、security.rs 测试)行为不变。
        self.append_intent_with_chain(command, Vec::new())
    }

    /// Pre-execution append(带决策链):命令执行前记录 Intent 状态的审计块(P1-W3.3)。
    ///
    /// WHY: spec.md:206 要求高危操作(risk_score ∈ [71,100])的完整决策链全量上
    ///      Merkle 审计链。此方法在命令执行前写入 Intent 记录,同时携带 pre-execution
    ///      决策链(Proposal/AsaAudit/Debate/Confession)。执行后通过
    ///      `extend_decision_chain` 补充 Execution/Result 步骤。
    ///
    /// 决策链纳入 `merkle_root` 计算(经 `hash_decision_chain`),篡改任意步骤
    /// (step_hash/outcome/删除步骤)均被 `verify()` 检测。
    ///
    /// # 参数
    /// - `command`:校验通过的命令规格
    /// - `decision_chain`:pre-execution 决策链(可为空 Vec 表示非高危操作)
    ///
    /// # 返回
    /// - `Ok(RecordId)`:追加成功,返回记录 ID(用于 extend/update)
    /// - `Err(SecCoreError::AuditError)`:哈希计算失败(理论上不会发生)
    pub fn append_intent_with_chain(
        &mut self,
        command: &CommandSpec,
        decision_chain: Vec<DecisionStep>,
    ) -> Result<RecordId, SecCoreError> {
        let index = self.blocks.len() as u64;
        let timestamp = Utc::now().timestamp();
        let command_hash = hash_command(command);
        // Intent 状态:result_hash 为空占位,执行后由 update_status 填充
        let result_hash = String::new();
        let prev_hash = self.current_hash.clone();
        let status = AuditRecordStatus::Intent;
        let decision_chain_hash = hash_decision_chain(&decision_chain);
        let merkle_root = compute_block_hash(
            index,
            timestamp,
            &command_hash,
            &result_hash,
            &prev_hash,
            status,
            &decision_chain_hash,
        );

        let block = AuditBlock {
            index,
            timestamp,
            command_hash,
            result_hash,
            prev_hash,
            merkle_root: merkle_root.clone(),
            status,
            decision_chain,
        };

        self.current_hash = merkle_root;
        self.blocks.push(block);
        Ok(index)
    }

    /// Post-execution extend:执行后向链尾块的决策链追加步骤(P1-W3.3)。
    ///
    /// WHY: N5 pre-execution audit 模式要求执行前先写 Intent,但完整决策链包含
    ///      Execution/Result 步骤(执行后才可知)。此方法在执行后补充这些步骤,
    ///      重算 `merkle_root` 保持链完整性。
    ///
    /// # 安全约束
    /// 仅允许扩展**链尾块**(与 `update_status` 一致),防止中间块 merkle_root
    /// 变更破坏后续 prev_hash 链。
    ///
    /// # 参数
    /// - `id`:append_intent_with_chain 返回的 RecordId
    /// - `steps`:待追加的决策步骤(如 Execution + Result)
    ///
    /// # 返回
    /// - `Ok(())`:扩展成功
    /// - `Err(SecCoreError::AuditError)`:id 无效或非链尾块
    pub fn extend_decision_chain(
        &mut self,
        id: RecordId,
        steps: Vec<DecisionStep>,
    ) -> Result<(), SecCoreError> {
        // 校验:id 必须是有效的链尾块索引(与 update_status 一致的安全约束)
        if self.blocks.is_empty() {
            return Err(SecCoreError::AuditError(format!(
                "审计链为空,RecordId {id} 不存在(需先调用 append_intent_with_chain)"
            )));
        }
        let last_index = (self.blocks.len() - 1) as u64;
        if id != last_index {
            return Err(SecCoreError::AuditError(format!(
                "RecordId {id} 不是链尾块(当前链尾索引 {last_index}),扩展非链尾块会破坏 merkle 链"
            )));
        }

        let block = &mut self.blocks[id as usize];
        block.decision_chain.extend(steps);
        // WHY: decision_chain 变更后必须重算 merkle_root(含新的 decision_chain_hash),
        //      并更新 current_hash,保持链尾块 merkle_root 与 current_hash 一致
        let decision_chain_hash = hash_decision_chain(&block.decision_chain);
        let new_root = compute_block_hash(
            block.index,
            block.timestamp,
            &block.command_hash,
            &block.result_hash,
            &block.prev_hash,
            block.status,
            &decision_chain_hash,
        );
        self.current_hash = new_root.clone();
        block.merkle_root = new_root;
        Ok(())
    }

    /// 重放决策链 — 事后完整重放(P1-W3.3)。
    ///
    /// WHY: spec.md:206 要求支持事后完整重放。此方法通过 RecordId 返回
    ///      对应审计块的完整决策链切片,供取证分析使用。
    ///
    /// # 参数
    /// - `id`:append_intent_with_chain 返回的 RecordId
    ///
    /// # 返回
    /// - `Ok(&[DecisionStep])`:完整决策链切片
    /// - `Err(SecCoreError::AuditError)`:id 无效(超出链范围)
    pub fn replay_decision_chain(&self, id: RecordId) -> Result<&[DecisionStep], SecCoreError> {
        let len = self.blocks.len() as u64;
        if id >= len {
            return Err(SecCoreError::AuditError(format!(
                "RecordId {id} 超出审计链范围(当前链长 {len})"
            )));
        }
        Ok(&self.blocks[id as usize].decision_chain)
    }

    /// Post-execution update:命令执行后更新对应记录的状态与结果(N5 修复)。
    ///
    /// WHY: 配合 append_intent 使用。执行后用 append_intent 返回的 RecordId
    ///      更新记录为 Executed(填充 result_hash)或 Failed(保持空占位)。
    ///      重新计算 merkle_root 并更新 current_hash,保持链完整性。
    ///
    /// # 安全约束
    /// 仅允许更新**链尾块** — 更新中间块会改变其 merkle_root,导致后续所有块
    /// 的 prev_hash 链断裂。这强制调用方严格遵循 append_intent → 立即执行 →
    /// 立即 update_status 的串行模式,防止 Intent 记录悬挂。
    ///
    /// # 参数
    /// - `id`:append_intent 返回的 RecordId
    /// - `status`:目标状态(Executed / Failed)
    /// - `result`:执行结果(Executed 状态必传,Failed 状态可传 None)
    ///
    /// # 返回
    /// - `Ok(())`:更新成功
    /// - `Err(SecCoreError::AuditError)`:id 无效或非链尾块
    pub fn update_status(
        &mut self,
        id: RecordId,
        status: AuditRecordStatus,
        result: Option<&ExecutionResult>,
    ) -> Result<(), SecCoreError> {
        // 校验:id 必须是有效的链尾块索引
        // WHY: 仅允许更新链尾块,防止中间块 merkle_root 变更破坏后续 prev_hash 链
        if self.blocks.is_empty() {
            return Err(SecCoreError::AuditError(format!(
                "审计链为空,RecordId {id} 不存在(需先调用 append_intent)"
            )));
        }
        let last_index = (self.blocks.len() - 1) as u64;
        if id != last_index {
            return Err(SecCoreError::AuditError(format!(
                "RecordId {id} 不是链尾块(当前链尾索引 {last_index}),更新非链尾块会破坏 merkle 链"
            )));
        }

        let block = &mut self.blocks[id as usize];
        block.status = status;
        if let Some(result) = result {
            block.result_hash = hash_result(result);
        }
        // WHY: status 或 result_hash 变更后必须重算 merkle_root,并更新 current_hash
        //      保持链尾块的 merkle_root 与 current_hash 一致(verify 检查 4)
        // WHY(P1-W3.3): decision_chain_hash 也纳入 merkle_root,保持决策链防篡改
        let decision_chain_hash = hash_decision_chain(&block.decision_chain);
        let new_root = compute_block_hash(
            block.index,
            block.timestamp,
            &block.command_hash,
            &block.result_hash,
            &block.prev_hash,
            block.status,
            &decision_chain_hash,
        );
        self.current_hash = new_root.clone();
        block.merkle_root = new_root;
        Ok(())
    }

    /// 追加一条已完成的审计记录 — 向后兼容接口(N5 修复保留)。
    ///
    /// WHY: 保留此方法避免破坏既有调用点(如 sandbox.rs 原有流程、security.rs 测试)。
    ///      内部委托 append_intent + update_status(Executed),等价于 pre-execution
    ///      模式的快捷路径(执行前记录意图 + 立即标记为已执行)。
    ///
    /// # 参数
    /// - `command`:校验通过的命令规格
    /// - `result`:执行结果
    ///
    /// # 返回
    /// - `Ok(())`:追加成功
    /// - `Err(SecCoreError::AuditError)`:序列化或哈希失败(理论上不会发生)
    pub fn append(
        &mut self,
        command: &CommandSpec,
        result: &ExecutionResult,
    ) -> Result<(), SecCoreError> {
        let id = self.append_intent(command)?;
        self.update_status(id, AuditRecordStatus::Executed, Some(result))?;
        Ok(())
    }

    /// 验证审计链完整性 — 检测任何篡改。
    ///
    /// 验证逻辑:
    /// 1. 每个块的 index 严格递增(0, 1, 2, ...)
    /// 2. 每个块的 prev_hash 等于前一块的 merkle_root
    /// 3. 每个块的 merkle_root 等于重新计算的哈希
    /// 4. current_hash 等于最后一块的 merkle_root
    ///
    /// # 返回
    /// - `Ok(true)`:链完整
    /// - `Ok(false)`:检测到篡改
    /// - `Err`:验证过程出错(理论上不会发生)
    pub fn verify(&self) -> Result<bool, SecCoreError> {
        let mut prev_hash = "0".repeat(64);

        for (i, block) in self.blocks.iter().enumerate() {
            // 检查1:index 严格递增
            if block.index != i as u64 {
                warn!(
                    expected = i,
                    actual = block.index,
                    "审计链篡改: index 不匹配"
                );
                return Ok(false);
            }

            // 检查2:prev_hash 链接正确
            if block.prev_hash != prev_hash {
                warn!(block_index = i, "审计链篡改: prev_hash 不匹配");
                return Ok(false);
            }

            // 检查3:merkle_root 重新计算匹配(含 status + decision_chain,防止状态/决策链篡改)
            let decision_chain_hash = hash_decision_chain(&block.decision_chain);
            let expected_root = compute_block_hash(
                block.index,
                block.timestamp,
                &block.command_hash,
                &block.result_hash,
                &block.prev_hash,
                block.status,
                &decision_chain_hash,
            );
            if block.merkle_root != expected_root {
                warn!(block_index = i, "审计链篡改: merkle_root 不匹配");
                return Ok(false);
            }

            prev_hash = block.merkle_root.clone();
        }

        // 检查4:current_hash 等于最后一块的 merkle_root
        if self.current_hash != prev_hash {
            warn!("审计链篡改: current_hash 不匹配链尾");
            return Ok(false);
        }

        Ok(true)
    }

    /// 返回审计块数量。
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// 审计链是否为空。
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

impl Default for AuditChain {
    fn default() -> Self {
        Self::new()
    }
}

/// 决策链构建器 — 在升级通道流程中逐步收集决策步骤(P1-W3.3)。
///
/// WHY: spec.md:206 要求高危操作的完整决策链(提案→辩论→自白→执行→结果)
///      全量上 Merkle 审计链。此构建器在 `Sandbox::audit_and_execute` 流程中
///      按阶段逐步收集 `DecisionStep`,最终通过 `build()` 产出 `Vec<DecisionStep>`
///      传给 `append_intent_with_chain` / `extend_decision_chain`。
///
/// 使用模式:
/// - **pre-execution**(执行前):`add_proposal` → `add_asa_audit`(可选) →
///   `add_debate` → `add_confession`,然后 `build()` 传给 `append_intent_with_chain`
/// - **post-execution**(执行后):新建 builder,`add_execution` → `add_result`,
///   然后 `build()` 传给 `extend_decision_chain`
#[derive(Debug, Clone, Default)]
pub struct DecisionChainBuilder {
    /// 已收集的决策步骤
    steps: Vec<DecisionStep>,
}

impl DecisionChainBuilder {
    /// 创建空决策链构建器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加提案步骤 — 命令规格校验通过,risk_score 已确定。
    ///
    /// step_hash 为命令规格的 SHA-256(复用 `hash_command`),outcome 含 risk_score。
    pub fn add_proposal(&mut self, spec: &CommandSpec) -> &mut Self {
        let step_hash = hash_command(spec);
        self.steps.push(DecisionStep {
            step_type: DecisionStepType::Proposal,
            timestamp: Utc::now().timestamp(),
            step_hash,
            outcome: format!("risk_score={}", spec.risk_score),
        });
        self
    }

    /// 添加 ASA 审计步骤 — AsaAuditor 前置实时审计结果。
    ///
    /// step_hash 为 AuditResult 关键字段的 SHA-256(safety_score/intervention/risk_level),
    /// outcome 含 intervention 动作与 safety_score。
    pub fn add_asa_audit(&mut self, result: &AuditResult) -> &mut Self {
        let step_hash = hash_asa_result(result);
        self.steps.push(DecisionStep {
            step_type: DecisionStepType::AsaAudit,
            timestamp: Utc::now().timestamp(),
            step_hash,
            outcome: format!(
                "intervention={:?}, safety={:.2}",
                result.intervention, result.safety_score
            ),
        });
        self
    }

    /// 添加辩论步骤 — Parliament 完整辩论结果。
    ///
    /// step_hash 为 "debate:{approved}" 的 SHA-256,outcome 为 "approved"/"rejected"。
    pub fn add_debate(&mut self, approved: bool) -> &mut Self {
        let outcome = if approved { "approved" } else { "rejected" };
        let step_hash = hash_simple_string(&format!("debate:{approved}"));
        self.steps.push(DecisionStep {
            step_type: DecisionStepType::Debate,
            timestamp: Utc::now().timestamp(),
            step_hash,
            outcome: outcome.to_string(),
        });
        self
    }

    /// 添加自白步骤 — 操作意图披露 + 风险确认。
    ///
    /// step_hash 为自白文本的 SHA-256,outcome 即意图文本。
    pub fn add_confession(&mut self, intent: &str) -> &mut Self {
        let step_hash = hash_simple_string(intent);
        self.steps.push(DecisionStep {
            step_type: DecisionStepType::Confession,
            timestamp: Utc::now().timestamp(),
            step_hash,
            outcome: intent.to_string(),
        });
        self
    }

    /// 添加执行步骤 — 沙箱执行启动。
    ///
    /// step_hash 为 "execution_started" 的 SHA-256,outcome 为 "execution_started"。
    pub fn add_execution(&mut self) -> &mut Self {
        let step_hash = hash_simple_string("execution_started");
        self.steps.push(DecisionStep {
            step_type: DecisionStepType::Execution,
            timestamp: Utc::now().timestamp(),
            step_hash,
            outcome: "execution_started".to_string(),
        });
        self
    }

    /// 添加结果步骤 — 执行结果记录。
    ///
    /// step_hash 为 exit_code 的 SHA-256,outcome 为 "exit_code={exit_code}"。
    /// 被拒绝的操作用 `add_result(-1)` 并在外部将 outcome 设为 "rejected";
    /// 但为语义清晰,拒绝操作应使用 `add_rejected_result`。
    pub fn add_result(&mut self, exit_code: i32) -> &mut Self {
        let step_hash = hash_simple_string(&format!("exit_code:{exit_code}"));
        self.steps.push(DecisionStep {
            step_type: DecisionStepType::Result,
            timestamp: Utc::now().timestamp(),
            step_hash,
            outcome: format!("exit_code={exit_code}"),
        });
        self
    }

    /// 添加拒绝结果步骤 — 操作被拒绝(EscalateToHuman / Parliament 否决 / ASA Block)。
    ///
    /// WHY: 区分"执行失败(exit_code=-1)"与"被拒绝未执行",outcome 含 "rejected"
    /// 语义,便于事后重放时区分拒绝路径与执行失败路径。
    pub fn add_rejected_result(&mut self, reason: &str) -> &mut Self {
        let step_hash = hash_simple_string(&format!("rejected:{reason}"));
        self.steps.push(DecisionStep {
            step_type: DecisionStepType::Result,
            timestamp: Utc::now().timestamp(),
            step_hash,
            outcome: format!("rejected:{reason}"),
        });
        self
    }

    /// 构建决策链 — 提取已收集的步骤向量。
    ///
    /// WHY: 使用 `&mut self` + `std::mem::take` 而非 `self`,既支持链式调用
    ///      (`Builder::new().add_x().build()`),又支持条件构建(`if cond { b.add_y(); }`),
    ///      且零拷贝(take 替换为空 Vec,原 Vec 所有权转移)。
    pub fn build(&mut self) -> Vec<DecisionStep> {
        std::mem::take(&mut self.steps)
    }
}

/// 计算命令规格的 SHA-256 哈希。
///
/// 哈希内容:program + 每个参数 + 每个环境变量(key=value)。
/// 用 \x00 分隔字段,防止参数拼接产生歧义(如 "ab" + "c" vs "a" + "bc")。
fn hash_command(command: &CommandSpec) -> String {
    let mut hasher = Sha256::new();
    hasher.update(command.program.as_bytes());
    for arg in &command.allowed_args {
        hasher.update(b"\x00");
        hasher.update(arg.as_bytes());
    }
    for (k, v) in &command.env_whitelist {
        hasher.update(b"\x00");
        hasher.update(k.as_bytes());
        hasher.update(b"=");
        hasher.update(v.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// 计算执行结果的 SHA-256 哈希。
///
/// 哈希内容:exit_code + stdout + stderr + duration_nanos。
/// 注意:不包含 `audit_hash` 字段,防止循环依赖与篡改绕过。
fn hash_result(result: &ExecutionResult) -> String {
    let mut hasher = Sha256::new();
    hasher.update(result.exit_code.to_le_bytes());
    hasher.update(result.stdout.as_bytes());
    hasher.update(result.stderr.as_bytes());
    hasher.update(result.duration.as_nanos().to_le_bytes());
    hex::encode(hasher.finalize())
}

/// 计算审计块的 Merkle 根(SHA-256)。
///
/// 哈希内容:index || timestamp || command_hash || result_hash || prev_hash || status || decision_chain_hash。
/// 这是链式结构的核心:每个块的哈希依赖前一块,形成单向链。
///
/// WHY(N5 修复):status 纳入哈希,防止攻击者将 Intent 状态篡改为 Executed
/// 伪造执行证据。status 用单字节表示(Intent=0 / Executed=1 / Failed=2)。
///
/// WHY(P1-W3.3):decision_chain_hash 纳入哈希,防止决策链步骤被篡改
/// (step_hash/outcome/删除步骤)。decision_chain_hash 由 `hash_decision_chain`
/// 预计算后传入,避免在块哈希中重复遍历决策链。
fn compute_block_hash(
    index: u64,
    timestamp: i64,
    command_hash: &str,
    result_hash: &str,
    prev_hash: &str,
    status: AuditRecordStatus,
    decision_chain_hash: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(index.to_le_bytes());
    hasher.update(timestamp.to_le_bytes());
    hasher.update(command_hash.as_bytes());
    hasher.update(result_hash.as_bytes());
    hasher.update(prev_hash.as_bytes());
    // WHY: status 作为单字节纳入哈希,防止状态字段被篡改后绕过验证
    hasher.update([status as u8]);
    // WHY(P1-W3.3): decision_chain_hash 纳入哈希,防止决策链被篡改
    hasher.update(decision_chain_hash.as_bytes());
    hex::encode(hasher.finalize())
}

/// 计算决策链的 SHA-256 哈希(P1-W3.3)。
///
/// 哈希内容:每个步骤的 step_type(单字节) || timestamp || step_hash || outcome,
/// 步骤间用 `\x00` 分隔,防止拼接歧义(如 "ab"+"c" vs "a"+"bc")。
///
/// WHY: 将完整决策链压缩为单一哈希值,纳入 `compute_block_hash` 的 merkle_root 计算。
///      任何步骤的字段变更(step_hash/outcome/删除步骤/添加步骤)都会改变此哈希,
///      从而被 `verify()` 检测。空决策链返回空输入的 SHA-256(确定性常量)。
fn hash_decision_chain(chain: &[DecisionStep]) -> String {
    let mut hasher = Sha256::new();
    for step in chain {
        hasher.update([step.step_type as u8]);
        hasher.update(step.timestamp.to_le_bytes());
        hasher.update(step.step_hash.as_bytes());
        hasher.update(step.outcome.as_bytes());
        // WHY: 分隔符防止相邻步骤的字段拼接产生歧义
        hasher.update(b"\x00");
    }
    hex::encode(hasher.finalize())
}

/// 计算简单字符串的 SHA-256 哈希(辅助函数)。
///
/// WHY: DecisionChainBuilder 的 Debate/Confession/Execution/Result 步骤
///      需要对简短文本计算哈希,此函数统一处理。
fn hash_simple_string(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hex::encode(hasher.finalize())
}

/// 计算 ASA 审计结果的 SHA-256 哈希(P1-W3.3)。
///
/// 哈希内容:safety_score || correctness_score || efficiency_score ||
/// intervention(单字节) || audit_reason || risk_level(单字节)。
///
/// WHY: AuditResult 不实现 Serialize,需手动哈希关键字段。
///      f32 用 `to_le_bytes()` 保证确定性(§4.4:f32 禁止隐式转 f64)。
fn hash_asa_result(result: &AuditResult) -> String {
    let mut hasher = Sha256::new();
    hasher.update(result.safety_score.to_le_bytes());
    hasher.update(result.correctness_score.to_le_bytes());
    hasher.update(result.efficiency_score.to_le_bytes());
    hasher.update([result.intervention as u8]);
    hasher.update(result.audit_reason.as_bytes());
    hasher.update([result.risk_level as u8]);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RiskLevel;
    use std::collections::HashMap;
    use std::time::Duration;

    fn make_spec() -> CommandSpec {
        CommandSpec {
            program: "echo".to_string(),
            allowed_args: vec!["hello".to_string()],
            env_whitelist: HashMap::new(),
            risk_level: RiskLevel::Low,
            risk_score: 10,
        }
    }

    fn make_result() -> ExecutionResult {
        ExecutionResult {
            exit_code: 0,
            stdout: "hello\n".to_string(),
            stderr: String::new(),
            duration: Duration::from_millis(10),
            audit_hash: "0".repeat(64),
        }
    }

    #[test]
    fn test_chain_append_and_verify() {
        let mut chain = AuditChain::new();
        assert!(chain.is_empty());

        chain.append(&make_spec(), &make_result()).unwrap();
        assert_eq!(chain.len(), 1);
        assert!(chain.verify().unwrap());
    }

    #[test]
    fn test_chain_tamper_detected() {
        let mut chain = AuditChain::new();
        chain.append(&make_spec(), &make_result()).unwrap();
        chain.append(&make_spec(), &make_result()).unwrap();

        // 篡改第一个块的 result_hash
        chain.blocks[0].result_hash = "1".repeat(64);

        // 篡改后验证应失败
        assert!(!chain.verify().unwrap());
    }

    #[test]
    fn test_chain_multiple_blocks() {
        let mut chain = AuditChain::new();
        for _ in 0..5 {
            chain.append(&make_spec(), &make_result()).unwrap();
        }
        assert_eq!(chain.len(), 5);
        assert!(chain.verify().unwrap());
    }

    /// N5 修复验证:status 字段纳入 merkle_root,篡改 status 应被检测。
    ///
    /// WHY: 防止攻击者将 Intent 状态篡改为 Executed 伪造执行证据。
    #[test]
    fn test_chain_status_tamper_detected() {
        let mut chain = AuditChain::new();
        // 写入一条 Intent 记录(不调用 update_status)
        chain.append_intent(&make_spec()).unwrap();
        assert_eq!(chain.blocks[0].status, AuditRecordStatus::Intent);
        assert!(chain.verify().unwrap(), "Intent 状态审计链应完整");

        // 篡改:将 Intent 改为 Executed,但不更新 result_hash 与 merkle_root
        chain.blocks[0].status = AuditRecordStatus::Executed;

        // 篡改后验证应失败(merkle_root 重算时 status 字段不匹配)
        assert!(
            !chain.verify().unwrap(),
            "篡改 status 应被 merkle_root 重算检测"
        );
    }

    /// N5 修复验证:pre-execution 流程(Intent → Executed)后审计链完整。
    #[test]
    fn test_chain_pre_execution_flow() {
        let mut chain = AuditChain::new();
        let spec = make_spec();
        let result = make_result();

        // 执行前记录意图
        let id = chain.append_intent(&spec).unwrap();
        assert_eq!(chain.blocks[0].status, AuditRecordStatus::Intent);
        assert!(chain.verify().unwrap(), "Intent 阶段审计链应完整");

        // 执行后更新为 Executed
        chain
            .update_status(id, AuditRecordStatus::Executed, Some(&result))
            .unwrap();
        assert_eq!(chain.blocks[0].status, AuditRecordStatus::Executed);
        assert!(chain.verify().unwrap(), "Executed 阶段审计链应完整");

        // 验证 update_status 重算了 merkle_root(result_hash 从空变为实际哈希)
        assert!(!chain.blocks[0].result_hash.is_empty());
    }

    /// N5 修复验证:Failed 状态路径 — 执行失败时记录 Failed,审计链仍完整。
    #[test]
    fn test_chain_failed_status_flow() {
        let mut chain = AuditChain::new();
        let spec = make_spec();

        let id = chain.append_intent(&spec).unwrap();
        // 执行失败:更新为 Failed,不传 result(result_hash 保持空占位)
        chain
            .update_status(id, AuditRecordStatus::Failed, None)
            .unwrap();
        assert_eq!(chain.blocks[0].status, AuditRecordStatus::Failed);
        assert!(chain.blocks[0].result_hash.is_empty());
        assert!(chain.verify().unwrap(), "Failed 状态审计链应完整");
    }
}
