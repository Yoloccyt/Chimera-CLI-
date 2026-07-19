//! ContextIsolationGuard 上下文隔离守卫 — 跨 Agent 上下文隔离与安全摘要
//!
//! 本文件定义 `ContextIsolationGuard`,确保 Agent 无法访问其他 Agent 的上下文。
//! 违规时触发 `MasError::ContextIsolationViolation`。
//!
//! ## 隔离规则
//!
//! - 每个 Agent 拥有独立的 `AgentContext`,通过 `agent_id` 标识所有权
//! - 任何跨 Agent 访问必须通过 `verify_access()` 校验
//! - 违规访问立即返回 `MasError::ContextIsolationViolation`
//! - `create_safe_summary()` 提供脱敏摘要,可用于跨 Agent 通信

use crate::context::manager::AgentContext;
use crate::error::{MasError, Result};
use serde::{Deserialize, Serialize};

/// 上下文隔离守卫 — 校验 Agent 对上下文的访问权限
///
/// 该守卫确保 MAS 子系统的核心隔离约束:Agent 无法直接读取其他 Agent 的上下文。
/// 跨 Agent 信息交换必须通过 `EventBus`(NexusEvent)或 `create_safe_summary()`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextIsolationGuard {
    /// 守卫保护的 Agent ID(上下文所有者)
    pub owner_agent_id: String,
}

impl ContextIsolationGuard {
    /// 创建新的隔离守卫
    ///
    /// ## 参数
    /// - `owner_agent_id`: 上下文所有者 Agent ID
    pub fn new(owner_agent_id: impl Into<String>) -> Self {
        Self {
            owner_agent_id: owner_agent_id.into(),
        }
    }

    /// 校验访问权限 — 检查请求方是否有权访问上下文
    ///
    /// ## 参数
    /// - `requesting_agent_id`: 请求访问的 Agent ID
    ///
    /// ## 返回
    /// - `Ok(())`: 访问合法(requesting_agent_id == owner_agent_id)
    /// - `Err(MasError::ContextIsolationViolation)`: 访问违规
    ///
    /// ## 示例
    ///
    /// ```no_run
    /// use chimera_mas::prelude::*;
    ///
    /// let guard = ContextIsolationGuard::new("agent-1");
    /// assert!(guard.verify_access("agent-1").is_ok());
    /// assert!(guard.verify_access("agent-2").is_err());
    /// ```
    pub fn verify_access(&self, requesting_agent_id: &str) -> Result<()> {
        if requesting_agent_id == self.owner_agent_id {
            Ok(())
        } else {
            Err(MasError::ContextIsolationViolation {
                agent_id: requesting_agent_id.into(),
                context_id: self.owner_agent_id.clone(),
            })
        }
    }

    /// 创建安全摘要 — 脱敏后的上下文摘要,可用于跨 Agent 通信
    ///
    /// 按 block.name 模式匹配提取可共享内容(任务状态/关键决策/结论),
    /// 排除 raw_conversation 等敏感块。每段内容截断至 200 字符防止泄露。
    ///
    /// ## 参数
    /// - `context`: 要提取摘要的 Agent 上下文(必须属于本守卫的 owner)
    ///
    /// ## 返回
    /// - `Ok(String)`: 结构化 Markdown 摘要
    /// - `Err(MasError::ContextIsolationViolation)`: guard owner 与 context.agent_id 不匹配
    pub fn create_safe_summary(&self, context: &AgentContext) -> Result<String> {
        // 防御性检查:守卫必须与上下文所有者匹配(§6.2 红线:跨 Agent 永远拒绝)
        if self.owner_agent_id != context.agent_id {
            return Err(MasError::ContextIsolationViolation {
                agent_id: self.owner_agent_id.clone(),
                context_id: context.agent_id.clone(),
            });
        }

        let mut sections = Vec::new();
        for block in context.blocks_iter() {
            let name_lower = block.name.to_lowercase();
            // 每段内容截断至 200 字符,防止泄露过多上下文
            let content: String = block.content.chars().take(200).collect();

            if name_lower.contains("status") {
                sections.push(format!("## Task Status\n{content}"));
            } else if name_lower.contains("decision") {
                sections.push(format!("## Key Decision\n{content}"));
            } else if name_lower.contains("conclusion") {
                sections.push(format!("## Conclusion\n{content}"));
            }
            // raw_conversation 等其他块不匹配任何模式,自动排除(脱敏)
        }

        if sections.is_empty() {
            Ok(format!(
                "Agent {} context summary (no extractable content)",
                self.owner_agent_id
            ))
        } else {
            Ok(sections.join("\n\n"))
        }
    }
}
