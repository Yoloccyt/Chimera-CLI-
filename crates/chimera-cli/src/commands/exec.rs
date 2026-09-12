//! `chimera exec <prompt>` — 非交互执行契约（WI-02）
//!
//! # stdout 纪律（WI-02 核心）
//! - 默认模式: stdout 仅最终结果（无流式动画、无 [done] 噪音——CI 管道
//!   `chimera exec "x" | jq .` 可直接消费）
//! - `--json` 模式: stdout = JSONL（每行一 AppEvent 形态，保持与全局
//!   envelope 一致的 jq 可解性）
//! - 日志/进度全走 stderr（tracing + eprintln），stdout 纯净断言进 CI
//!
//! # 退出码四类语义（WI-02 契约）
//! | 码 | 语义 | 映射 |
//! |----|------|------|
//! | 0 | 成功 | Ok |
//! | 2 | 审批拒否 | `PermissionDenied`（权限策略拒绝） |
//! | 3 | 预算耗尽 | `EngineError`（引擎/预算类故障） |
//! | 4 | 工具失败 | 其他错误（工具执行失败等） |
//!
//! # 与全局矩阵的关系（ADR-060）
//! exec 是**独立子命令契约**，其 0/2/3/4 语义不改变全局 0-6 矩阵
//! （兼容红线：全局矩阵服务所有命令，exec 映射仅在 dispatch 的
//! Exec 分支应用，经 [`exec_exit_code`] 显式转换）。

use anyhow::Result;
use event_bus::EventBus;
use nexus_core::{MultimodalInput, UserIntent};
use quest_engine::QuestEngine;
use uuid::Uuid;

use crate::config::ChimeraConfig;
use crate::error::ChimeraCliError;
use crate::orchestrator::{build_error_reply, build_quest_reply};
use crate::permission::PermissionCtx;

/// 执行 exec 命令 — 非交互 + stdout 纪律（WI-02）
///
/// # 与 `chimera run` 的关系
/// 复用相同 QuestEngine 分解路径（单一真相源），差异在**输出纪律**：
/// run 保留流式动画（人类消费），exec 输出纯净（机器消费）。
pub async fn execute(
    prompt: &str,
    _config: &ChimeraConfig,
    json: bool,
    _perm: &PermissionCtx,
) -> Result<()> {
    tracing::info!(prompt = %prompt, "exec: 收到非交互任务");

    // 1. 构造进程内 ephemeral EventBus + QuestEngine（与 run 一致）
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    // 2. 封装 UserIntent（UUIDv7 时间有序）
    let intent = UserIntent {
        intent_id: format!("exec-{}", Uuid::now_v7()),
        raw_text: prompt.to_string(),
        multimodal_inputs: vec![MultimodalInput::Text(prompt.to_string())],
        risk_level: 0,
    };

    // 3. 真实 L9 分解（错误 → 人类可读到 stderr，不污染 stdout）
    let quest = match engine.create_quest(intent).await {
        Ok(q) => q,
        Err(e) => {
            let msg = build_error_reply(&e);
            eprintln!("{msg}");
            return Err(ChimeraCliError::EngineError(msg).into());
        }
    };

    // 4. 输出纪律（WI-02 核心）
    if json {
        // JSONL 模式: 每行一 AppEvent（ThreadStarted / TurnCompleted 骨架流，
        // 后续挂接真实 AppEvent 流——协议面类型已就绪 nexus-contracts::app）
        let mut lines = serde_json::Map::new();
        lines.insert("event".into(), serde_json::json!("quest_created"));
        lines.insert("quest_id".into(), serde_json::json!(quest.quest_id));
        lines.insert("task_count".into(), serde_json::json!(quest.tasks.len()));
        println!(
            "{}",
            serde_json::to_string(&serde_json::Value::Object(lines))?
        );
    } else {
        // 纯净模式: stdout 仅最终结果（单行回复，无流式动画）
        println!("{}", build_quest_reply(&quest));
    }

    Ok(())
}

/// exec 退出码映射 — WI-02 四类语义（0/2/3/4）
///
/// # 映射原则
/// - 审批拒否（权限策略拒绝）→ 2（与全局 PermissionDenied=5 不同——exec 契约）
/// - 引擎/预算类故障 → 3
/// - 其余（工具失败等）→ 4
///
/// 返回 `u8`；`Ok` 路径的 0 由调用方直接返回（dispatch 层）。
pub fn exec_exit_code(err: &ChimeraCliError) -> u8 {
    match err {
        // 审批拒否（权限策略拒绝）
        ChimeraCliError::PermissionDenied(_) => 2,
        // 预算耗尽/引擎故障
        ChimeraCliError::EngineError(_) => 3,
        // 其余一律按工具失败
        _ => 4,
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_exit_code_semantics() {
        // WI-02 四类语义: 2 审批拒否 / 3 预算耗尽 / 4 工具失败
        assert_eq!(
            exec_exit_code(&ChimeraCliError::PermissionDenied("denied".into())),
            2
        );
        assert_eq!(
            exec_exit_code(&ChimeraCliError::EngineError("budget exhausted".into())),
            3
        );
        assert_eq!(
            exec_exit_code(&ChimeraCliError::Timeout("tool timeout".into())),
            4,
            "工具超时按工具失败归 4"
        );
        assert_eq!(
            exec_exit_code(&ChimeraCliError::IoError(std::io::Error::other("tool io"))),
            4,
            "工具 IO 失败归 4"
        );
    }

    #[test]
    fn exec_does_not_override_global_matrix() {
        // 兼容红线: 全局矩阵保持 ADR-060 语义（exec 映射仅局部应用）
        assert_eq!(
            ChimeraCliError::PermissionDenied("x".into()).exit_code_value(),
            5,
            "全局矩阵 PermissionDenied 仍为 5（exec 的 2 仅在 Exec 分支应用）"
        );
    }
}
