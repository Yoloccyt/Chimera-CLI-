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
    /// ```ignore
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
        let index = self.blocks.len() as u64;
        let timestamp = Utc::now().timestamp();
        let command_hash = hash_command(command);
        // Intent 状态:result_hash 为空占位,执行后由 update_status 填充
        let result_hash = String::new();
        let prev_hash = self.current_hash.clone();
        let status = AuditRecordStatus::Intent;
        let merkle_root = compute_block_hash(
            index,
            timestamp,
            &command_hash,
            &result_hash,
            &prev_hash,
            status,
        );

        let block = AuditBlock {
            index,
            timestamp,
            command_hash,
            result_hash,
            prev_hash,
            merkle_root: merkle_root.clone(),
            status,
        };

        self.current_hash = merkle_root;
        self.blocks.push(block);
        Ok(index)
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
        let new_root = compute_block_hash(
            block.index,
            block.timestamp,
            &block.command_hash,
            &block.result_hash,
            &block.prev_hash,
            block.status,
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

            // 检查3:merkle_root 重新计算匹配(含 status 字段,防止状态篡改)
            let expected_root = compute_block_hash(
                block.index,
                block.timestamp,
                &block.command_hash,
                &block.result_hash,
                &block.prev_hash,
                block.status,
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

    /// P0-8:持久化审计链到SQLite数据库
    ///
    /// 将审计链的所有块写入SQLite,使用WAL模式保证写入性能。
    /// 表结构包含所有AuditBlock字段,并建立索引加速查询。
    ///
    /// # 参数
    /// - `db_path`:SQLite数据库文件路径
    ///
    /// # 返回
    /// - `Ok(())`:持久化成功
    /// - `Err(SecCoreError::AuditError)`:数据库操作失败
    pub fn persist_to_sqlite(&self, db_path: &str) -> Result<(), SecCoreError> {
        use rusqlite::{params, Connection};

        let conn = Connection::open(db_path).map_err(|e| {
            SecCoreError::AuditError(format!("无法打开SQLite数据库: {e}"))
        })?;

        // 启用WAL模式提升并发写入性能
        conn.execute("PRAGMA journal_mode=WAL;", [])
            .map_err(|e| SecCoreError::AuditError(format!("无法设置WAL模式: {e}")))?;

        // 创建审计块表(若不存在)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS audit_blocks (
                index_val INTEGER PRIMARY KEY,
                timestamp INTEGER NOT NULL,
                command_hash TEXT NOT NULL,
                result_hash TEXT NOT NULL,
                prev_hash TEXT NOT NULL,
                merkle_root TEXT NOT NULL,
                status INTEGER NOT NULL
            );",
            [],
        )
        .map_err(|e| SecCoreError::AuditError(format!("创建表失败: {e}")))?;

        // 创建索引加速查询
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_audit_status ON audit_blocks(status);",
            [],
        )
        .map_err(|e| SecCoreError::AuditError(format!("创建索引失败: {e}")))?;

        // 插入所有审计块(使用事务批量写入)
        let tx = conn
            .transaction()
            .map_err(|e| SecCoreError::AuditError(format!("开始事务失败: {e}")))?;

        for block in &self.blocks {
            tx.execute(
                "INSERT OR REPLACE INTO audit_blocks 
                 (index_val, timestamp, command_hash, result_hash, prev_hash, merkle_root, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7);",
                params![
                    block.index as i64,
                    block.timestamp,
                    &block.command_hash,
                    &block.result_hash,
                    &block.prev_hash,
                    &block.merkle_root,
                    block.status as i64,
                ],
            )
            .map_err(|e| SecCoreError::AuditError(format!("插入审计块失败: {e}")))?;
        }

        tx.commit()
            .map_err(|e| SecCoreError::AuditError(format!("提交事务失败: {e}")))?;

        Ok(())
    }

    /// P0-8:从SQLite数据库恢复审计链
    ///
    /// 从SQLite读取所有审计块,重建AuditChain。
    /// 恢复后应调用verify()验证链完整性。
    ///
    /// # 参数
    /// - `db_path`:SQLite数据库文件路径
    ///
    /// # 返回
    /// - `Ok(AuditChain)`:恢复成功
    /// - `Err(SecCoreError::AuditError)`:数据库操作失败或数据损坏
    pub fn restore_from_sqlite(db_path: &str) -> Result<Self, SecCoreError> {
        use rusqlite::Connection;

        let conn = Connection::open(db_path).map_err(|e| {
            SecCoreError::AuditError(format!("无法打开SQLite数据库: {e}"))
        })?;

        let mut stmt = conn
            .prepare(
                "SELECT index_val, timestamp, command_hash, result_hash, 
                        prev_hash, merkle_root, status
                 FROM audit_blocks ORDER BY index_val;",
            )
            .map_err(|e| SecCoreError::AuditError(format!("准备查询失败: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                let status_int: i64 = row.get(6)?;
                let status = match status_int {
                    0 => AuditRecordStatus::Intent,
                    1 => AuditRecordStatus::Executed,
                    2 => AuditRecordStatus::Failed,
                    _ => AuditRecordStatus::Intent, // 默认值
                };
                Ok(AuditBlock {
                    index: row.get::<_, i64>(0)? as u64,
                    timestamp: row.get(1)?,
                    command_hash: row.get(2)?,
                    result_hash: row.get(3)?,
                    prev_hash: row.get(4)?,
                    merkle_root: row.get(5)?,
                    status,
                })
            })
            .map_err(|e| SecCoreError::AuditError(format!("查询失败: {e}")))?;

        let mut blocks = Vec::new();
        let mut current_hash = "0".repeat(64);

        for row in rows {
            let block = row.map_err(|e| {
                SecCoreError::AuditError(format!("读取审计块失败: {e}"))
            })?;
            current_hash = block.merkle_root.clone();
            blocks.push(block);
        }

        Ok(Self {
            blocks,
            current_hash,
        })
    }
}

impl Default for AuditChain {
    fn default() -> Self {
        Self::new()
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
/// 哈希内容:index || timestamp || command_hash || result_hash || prev_hash || status。
/// 这是链式结构的核心:每个块的哈希依赖前一块,形成单向链。
///
/// WHY(N5 修复):status 纳入哈希,防止攻击者将 Intent 状态篡改为 Executed
/// 伪造执行证据。status 用单字节表示(Intent=0 / Executed=1 / Failed=2)。
fn compute_block_hash(
    index: u64,
    timestamp: i64,
    command_hash: &str,
    result_hash: &str,
    prev_hash: &str,
    status: AuditRecordStatus,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(index.to_le_bytes());
    hasher.update(timestamp.to_le_bytes());
    hasher.update(command_hash.as_bytes());
    hasher.update(result_hash.as_bytes());
    hasher.update(prev_hash.as_bytes());
    // WHY: status 作为单字节纳入哈希,防止状态字段被篡改后绕过验证
    hasher.update([status as u8]);
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

    // P0-8: SQLite持久化测试
    #[test]
    fn test_persist_and_restore_sqlite() {
        let mut chain = AuditChain::new();
        for _ in 0..3 {
            chain.append(&make_spec(), &make_result()).unwrap();
        }
        assert_eq!(chain.len(), 3);
        assert!(chain.verify().unwrap());

        // 持久化到临时数据库
        let db_path = ":memory:"; // 使用内存数据库测试
        chain.persist_to_sqlite(db_path).unwrap();

        // 从数据库恢复
        let restored = AuditChain::restore_from_sqlite(db_path).unwrap();
        assert_eq!(restored.len(), 3);
        assert!(restored.verify().unwrap(), "恢复的审计链应完整");

        // 验证恢复的块与原块一致
        for (orig, restored_block) in chain.blocks.iter().zip(restored.blocks.iter()) {
            assert_eq!(orig.index, restored_block.index);
            assert_eq!(orig.command_hash, restored_block.command_hash);
            assert_eq!(orig.status, restored_block.status);
        }
    }

    #[test]
    fn test_restore_empty_sqlite() {
        // 测试空数据库恢复
        let db_path = ":memory:";
        // 创建空表
        {
            use rusqlite::Connection;
            let conn = Connection::open(db_path).unwrap();
            conn.execute(
                "CREATE TABLE audit_blocks (
                    index_val INTEGER PRIMARY KEY,
                    timestamp INTEGER NOT NULL,
                    command_hash TEXT NOT NULL,
                    result_hash TEXT NOT NULL,
                    prev_hash TEXT NOT NULL,
                    merkle_root TEXT NOT NULL,
                    status INTEGER NOT NULL
                );",
                [],
            ).unwrap();
        }

        let restored = AuditChain::restore_from_sqlite(db_path).unwrap();
        assert!(restored.is_empty());
    }
}
