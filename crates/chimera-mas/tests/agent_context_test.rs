#![forbid(unsafe_code)]

//! Task 9.1 (RED): AgentContext 上下文管理失败测试
//!
//! 覆盖 5 类场景（共 14 个测试）：
//! 1. 1M Token 限制（2 个）— max_tokens = 1_048_576，build_prompt 稀疏化 150K → < 128K
//! 2. build_prompt HCW 稀疏化（3 个）— Critical 保留、永不压缩、优先级降序
//! 3. ContextBlock 优先级（4 个）— 排序、谓词、默认可压缩、Critical 不可压缩
//! 4. ContextIsolationGuard（3 个）— owner 允许、跨 Agent 拒绝、safe_summary 提取
//! 5. add_block Token 预算（2 个）— current_tokens 追踪、超限返回 TokenBudgetExceeded
//!
//! TDD RED 阶段：本文件引用尚未实现的类型（ContextPriority/ContextBlock）和方法
//! （AgentContext::new/add_block/build_prompt、create_safe_summary 新签名），
//! 编译失败为预期。GREEN 阶段实现后全部通过。

use chimera_mas::prelude::*;
use event_bus::EventBus;

/// 1M Token 等效上限（1_048_576 = 1024 × 1024）
const ONE_M_TOKENS: usize = 1_048_576;
/// L3 实际容量 = 1M / 8 = 128K = 131_072（8× 稀疏压缩，Ω-Compress）
const L3_EFFECTIVE_CAPACITY: usize = 131_072;

/// 构造测试用 EventBus
fn make_bus() -> EventBus {
    EventBus::new()
}

// ============================================================
// 1. 1M Token 限制（2 个测试）
// ============================================================

/// 验证 AgentContext 接受 1M Token 上限（1M 等效 = 128K 实际 + 8× 稀疏）
#[tokio::test]
async fn test_agent_context_accepts_1m_token_limit() {
    let ctx =
        AgentContext::new("agent-1", ONE_M_TOKENS, make_bus()).expect("1M Token 上限应被接受");
    assert_eq!(ctx.max_tokens, ONE_M_TOKENS);
    assert_eq!(ctx.current_tokens, 0);
}

/// 验证 build_prompt 将 150K Token 稀疏化至 < 128K（L3 实际容量）
///
/// 插入 150K 的 Optional 块（超过 L2 128K），build_prompt 应通过 HCW 稀疏化
/// 使输出 < L3_EFFECTIVE_CAPACITY，且 Critical 块保留。
#[tokio::test]
async fn test_build_prompt_sparsifies_oversized_context() {
    let mut ctx = AgentContext::new("agent-1", ONE_M_TOKENS, make_bus()).unwrap();
    // 插入 150K token 的 Optional 块（超过 L2 128K，触发 L3 稀疏化）
    for i in 0..150 {
        ctx.add_block(ContextBlock::new(
            format!("wiki-{i}"),
            "x".repeat(1000),
            1000,
            ContextPriority::Optional,
        ))
        .expect("add_block 应成功");
    }
    // 插入一个 Critical 块确保输出非空
    ctx.add_block(ContextBlock::new(
        "system-prompt",
        "You are a helpful assistant.".to_string(),
        10,
        ContextPriority::Critical,
    ))
    .unwrap();

    let prompt = ctx.build_prompt().await.expect("build_prompt 应成功");
    // 输出应非空（至少包含 Critical 块）
    assert!(!prompt.is_empty(), "build_prompt 输出不应为空");
    // Critical 块应保留
    assert!(
        prompt.contains("helpful assistant"),
        "Critical 块内容应保留在输出中"
    );
    // 估算输出 token 数（字符数 / 4），应 < L3 实际容量 128K
    let estimated_tokens = prompt.len() / 4;
    assert!(
        estimated_tokens < L3_EFFECTIVE_CAPACITY,
        "build_prompt 输出应经稀疏化 < 128K (估计 {estimated_tokens} tokens)，实际 {} chars",
        prompt.len()
    );
}

// ============================================================
// 2. build_prompt HCW 稀疏化（3 个测试）
// ============================================================

/// 验证 build_prompt 保留 Critical 块（永不因稀疏化丢失）
#[tokio::test]
async fn test_build_prompt_preserves_critical_block() {
    let mut ctx = AgentContext::new("agent-1", ONE_M_TOKENS, make_bus()).unwrap();
    ctx.add_block(ContextBlock::new(
        "system-prompt",
        "[CRITICAL] You are Chimera CLI.".to_string(),
        20,
        ContextPriority::Critical,
    ))
    .unwrap();
    // 添加大量 Optional 块触发稀疏化
    for i in 0..200 {
        ctx.add_block(ContextBlock::new(
            format!("wiki-{i}"),
            format!("wiki content {i}"),
            1000,
            ContextPriority::Optional,
        ))
        .unwrap();
    }
    let prompt = ctx.build_prompt().await.expect("build_prompt 应成功");
    assert!(
        prompt.contains("[CRITICAL]"),
        "Critical 块应保留在 build_prompt 输出中"
    );
}

/// 验证 build_prompt 永不压缩 Critical 块（多次调用内容完整）
#[tokio::test]
async fn test_build_prompt_never_compresses_critical() {
    let mut ctx = AgentContext::new("agent-1", ONE_M_TOKENS, make_bus()).unwrap();
    let critical_content = "NEVER_COMPRESS_THIS_CRITICAL_CONTENT";
    ctx.add_block(ContextBlock::new(
        "system-prompt",
        critical_content.to_string(),
        10,
        ContextPriority::Critical,
    ))
    .unwrap();
    // 多次调用 build_prompt，验证 Critical 块始终完整
    for _ in 0..3 {
        let prompt = ctx.build_prompt().await.expect("build_prompt 应成功");
        assert!(
            prompt.contains(critical_content),
            "Critical 块应始终完整保留（永不压缩）"
        );
    }
}

/// 验证 build_prompt 按优先级降序排列输出
#[tokio::test]
async fn test_build_prompt_priority_descending() {
    let mut ctx = AgentContext::new("agent-1", ONE_M_TOKENS, make_bus()).unwrap();
    // 插入 3 个不同优先级的块，总量 ≥ 4096 使 complexity = 0.4 (Regular, Top-10 文件)
    // Regular 档位选 10 个文件，3 个块全部被 OSA 选中
    ctx.add_block(ContextBlock::new(
        "normal-block",
        "NORMAL_CONTENT".to_string(),
        2000,
        ContextPriority::Normal,
    ))
    .unwrap();
    ctx.add_block(ContextBlock::new(
        "critical-block",
        "CRITICAL_CONTENT".to_string(),
        1048,
        ContextPriority::Critical,
    ))
    .unwrap();
    ctx.add_block(ContextBlock::new(
        "high-block",
        "HIGH_CONTENT".to_string(),
        1048,
        ContextPriority::High,
    ))
    .unwrap();
    // total = 4096, complexity = 0.4 (Regular), OSA 选 Top-10 文件 → 3 个全选
    let prompt = ctx.build_prompt().await.expect("build_prompt 应成功");
    let critical_pos = prompt.find("CRITICAL_CONTENT");
    let high_pos = prompt.find("HIGH_CONTENT");
    let normal_pos = prompt.find("NORMAL_CONTENT");
    assert!(critical_pos.is_some(), "Critical 块应在输出中");
    assert!(high_pos.is_some(), "High 块应在输出中");
    assert!(normal_pos.is_some(), "Normal 块应在输出中");
    // 验证优先级降序排列：Critical > High > Normal
    if let (Some(c), Some(h), Some(n)) = (critical_pos, high_pos, normal_pos) {
        assert!(c < h, "Critical 应在 High 之前, got c={c}, h={h}");
        assert!(h < n, "High 应在 Normal 之前, got h={h}, n={n}");
    }
}

// ============================================================
// 3. ContextBlock 优先级（4 个测试）
// ============================================================

/// 验证 ContextPriority 排序：Critical > High > Normal > Low > Optional
#[test]
fn test_context_priority_ordering() {
    assert!(ContextPriority::Critical > ContextPriority::High);
    assert!(ContextPriority::High > ContextPriority::Normal);
    assert!(ContextPriority::Normal > ContextPriority::Low);
    assert!(ContextPriority::Low > ContextPriority::Optional);
}

/// 验证 ContextPriority 谓词：is_critical() / is_optional()
#[test]
fn test_context_priority_predicates() {
    assert!(ContextPriority::Critical.is_critical());
    assert!(!ContextPriority::High.is_critical());
    assert!(ContextPriority::Optional.is_optional());
    assert!(!ContextPriority::Low.is_optional());
}

/// 验证 ContextBlock 默认 is_compressible = true（非 Critical 块）
#[test]
fn test_context_block_default_compressible() {
    let block = ContextBlock::new("block-1", "content", 100, ContextPriority::Normal);
    assert!(block.is_compressible, "Normal 块默认应可压缩");
}

/// 验证 Critical 块 is_compressible = false（永不压缩）
#[test]
fn test_critical_block_not_compressible() {
    let block = ContextBlock::new("system-prompt", "content", 100, ContextPriority::Critical);
    assert!(
        !block.is_compressible,
        "Critical 块不可压缩（ADR-026 决策 7 红线）"
    );
}

// ============================================================
// 4. ContextIsolationGuard（3 个测试）
// ============================================================

/// 验证 owner Agent 允许访问自己的上下文
#[test]
fn test_isolation_guard_owner_allowed() {
    let guard = ContextIsolationGuard::new("agent-1");
    assert!(
        guard.verify_access("agent-1").is_ok(),
        "owner Agent 应允许访问自己的上下文"
    );
}

/// 验证 ContextIsolationGuard 永远拒绝跨 Agent 访问
#[test]
fn test_isolation_guard_rejects_cross_agent() {
    let guard = ContextIsolationGuard::new("agent-1");
    let result = guard.verify_access("agent-2");
    assert!(result.is_err(), "跨 Agent 访问应被拒绝");
    assert!(
        matches!(
            result.unwrap_err(),
            MasError::ContextIsolationViolation { .. }
        ),
        "跨 Agent 访问应返回 ContextIsolationViolation"
    );
}

/// 验证 create_safe_summary 提取任务状态/关键决策/结论，排除 raw_conversation
#[tokio::test]
async fn test_create_safe_summary_extracts_content() {
    let mut ctx = AgentContext::new("agent-1", ONE_M_TOKENS, make_bus()).unwrap();
    ctx.add_block(ContextBlock::new(
        "task-status",
        "Task is 50% complete, waiting for review.".to_string(),
        20,
        ContextPriority::High,
    ))
    .unwrap();
    ctx.add_block(ContextBlock::new(
        "key-decision",
        "Decided to use Rust for performance.".to_string(),
        15,
        ContextPriority::Normal,
    ))
    .unwrap();
    ctx.add_block(ContextBlock::new(
        "conclusion",
        "The implementation is complete.".to_string(),
        10,
        ContextPriority::High,
    ))
    .unwrap();
    ctx.add_block(ContextBlock::new(
        "raw-conversation",
        "Secret API key: sk-xxxxx".to_string(),
        50,
        ContextPriority::Low,
    ))
    .unwrap();

    let guard = ContextIsolationGuard::new("agent-1");
    let summary = guard
        .create_safe_summary(&ctx)
        .expect("safe summary 应成功");
    // 应提取任务状态/关键决策/结论
    assert!(
        summary.contains("50% complete") || summary.contains("review"),
        "应提取任务状态"
    );
    assert!(
        summary.contains("Rust") || summary.contains("performance"),
        "应提取关键决策"
    );
    assert!(summary.contains("complete"), "应提取结论");
    // 不应包含原始对话中的敏感信息
    assert!(
        !summary.contains("sk-xxxxx"),
        "不应包含 raw-conversation 的敏感信息"
    );
}

/// 验证 create_safe_summary 拒绝 mismatched guard（owner != context.agent_id）
#[tokio::test]
async fn test_create_safe_summary_rejects_mismatched_guard() {
    let ctx = AgentContext::new("agent-1", ONE_M_TOKENS, make_bus()).unwrap();
    // guard 的 owner 是 agent-2，但 context 属于 agent-1
    let guard = ContextIsolationGuard::new("agent-2");
    let result = guard.create_safe_summary(&ctx);
    assert!(result.is_err(), "mismatched guard 应返回错误");
    assert!(
        matches!(
            result.unwrap_err(),
            MasError::ContextIsolationViolation { .. }
        ),
        "mismatched guard 应返回 ContextIsolationViolation"
    );
}

// ============================================================
// 5. add_block Token 预算（2 个测试）
// ============================================================

/// 验证 add_block 正确追踪 current_tokens
#[test]
fn test_add_block_tracks_current_tokens() {
    let mut ctx = AgentContext::new("agent-1", 10_000, make_bus()).unwrap();
    assert_eq!(ctx.current_tokens, 0, "初始 current_tokens 应为 0");
    ctx.add_block(ContextBlock::new(
        "block-1",
        "content-1",
        500,
        ContextPriority::Normal,
    ))
    .unwrap();
    assert_eq!(
        ctx.current_tokens, 500,
        "添加 500 token 后 current_tokens 应为 500"
    );
    ctx.add_block(ContextBlock::new(
        "block-2",
        "content-2",
        300,
        ContextPriority::High,
    ))
    .unwrap();
    assert_eq!(
        ctx.current_tokens, 800,
        "再添加 300 token 后 current_tokens 应为 800"
    );
}

/// 验证 add_block 超过 max_tokens 返回 TokenBudgetExceeded
#[test]
fn test_add_block_exceeding_budget_returns_error() {
    let mut ctx = AgentContext::new("agent-1", 1000, make_bus()).unwrap();
    ctx.add_block(ContextBlock::new(
        "block-1",
        "content-1",
        800,
        ContextPriority::Normal,
    ))
    .unwrap();
    // 再添加 300 token，总 1100 > max 1000，应返回 TokenBudgetExceeded
    let result = ctx.add_block(ContextBlock::new(
        "block-2",
        "content-2",
        300,
        ContextPriority::Normal,
    ));
    assert!(result.is_err(), "超过 max_tokens 应返回错误");
    let err = result.unwrap_err();
    assert!(
        matches!(
            &err,
            MasError::TokenBudgetExceeded {
                agent_id,
                current_tokens,
                max_tokens
            } if agent_id == "agent-1" && *current_tokens == 1100 && *max_tokens == 1000
        ),
        "应返回 TokenBudgetExceeded(agent=agent-1, current=1100, max=1000), 实际: {err}"
    );
}
