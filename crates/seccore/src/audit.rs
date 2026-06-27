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

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::error::SecCoreError;
use crate::types::{CommandSpec, ExecutionResult};

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
    pub result_hash: String,
    /// 前一块的 merkle_root(创世块为 64 个 '0')
    pub prev_hash: String,
    /// 本块的 Merkle 根(SHA-256(index||timestamp||command_hash||result_hash||prev_hash))
    pub merkle_root: String,
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

    /// 追加一条审计记录 — 计算哈希并链接到链尾。
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
        let index = self.blocks.len() as u64;
        let timestamp = Utc::now().timestamp();
        let command_hash = hash_command(command);
        let result_hash = hash_result(result);
        let prev_hash = self.current_hash.clone();
        let merkle_root =
            compute_block_hash(index, timestamp, &command_hash, &result_hash, &prev_hash);

        let block = AuditBlock {
            index,
            timestamp,
            command_hash,
            result_hash,
            prev_hash,
            merkle_root: merkle_root.clone(),
        };

        self.current_hash = merkle_root;
        self.blocks.push(block);
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

            // 检查3:merkle_root 重新计算匹配
            let expected_root = compute_block_hash(
                block.index,
                block.timestamp,
                &block.command_hash,
                &block.result_hash,
                &block.prev_hash,
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
/// 哈希内容:index || timestamp || command_hash || result_hash || prev_hash。
/// 这是链式结构的核心:每个块的哈希依赖前一块,形成单向链。
fn compute_block_hash(
    index: u64,
    timestamp: i64,
    command_hash: &str,
    result_hash: &str,
    prev_hash: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(index.to_le_bytes());
    hasher.update(timestamp.to_le_bytes());
    hasher.update(command_hash.as_bytes());
    hasher.update(result_hash.as_bytes());
    hasher.update(prev_hash.as_bytes());
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
}
