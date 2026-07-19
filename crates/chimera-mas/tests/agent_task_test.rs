//! Task 11 (TDD RED) — AgentTask wrapper 类型失败测试
//!
//! 对应 SubTask 11.1: 编写失败测试(RED 阶段)
//! 对应 SubTask 11.2: AgentTask 结构定义(inner + complexity + estimated_tokens +
//!   acceptable_latency + quality_requirement)
//! 对应 SubTask 11.3: AgentTask::new() 5 参数构造 + 复用 Task 7 的
//!   `From<TaskComplexity> for ThinkingMode`
//!
//! ## TDD 阶段
//!
//! - **RED**: 当前阶段,测试应失败(`AgentTask::new()` 5 参数版本尚未实现)
//! - **GREEN**: SubTask 11.2-11.3 实现后,测试应全部通过
//!
//! ## 架构合规
//!
//! - ADR-026 决策 3: AgentTask wrapper **不修改** `nexus_core::Task`
//!   (§3.3.1 第 4 条领域类型稳定性)
//! - `acceptable_latency` 使用 `std::time::Duration`(非 chrono::Duration,
//!   因 chrono::Duration 不能用于 `tokio::time::sleep`)
//! - AgentTask 实现 Clone + Debug + Serialize + Deserialize + PartialEq
//! - 复用 Task 7 的 `From<TaskComplexity> for ThinkingMode`(同文件 delegation.rs)

#![forbid(unsafe_code)]

use chimera_mas::prelude::*;
use chimera_mas::QualityLevel;
use nexus_core::{Task, TaskStatus, ThinkingMode};
use std::time::Duration;

// ============================================================
// 辅助函数 — 创建测试用 nexus_core::Task
// ============================================================

/// 创建测试用 Task(Pending 状态,无依赖)
fn make_test_task(id: &str, desc: &str) -> Task {
    Task {
        task_id: id.into(),
        description: desc.into(),
        status: TaskStatus::Pending,
        dependencies: vec![],
    }
}

/// 创建带依赖的测试 Task
fn make_test_task_with_deps(id: &str, desc: &str, deps: Vec<String>) -> Task {
    Task {
        task_id: id.into(),
        description: desc.into(),
        status: TaskStatus::Pending,
        dependencies: deps,
    }
}

// ============================================================
// SubTask 11.2: AgentTask 字段正确性(5 核心字段)
// ============================================================

#[test]
fn test_agent_task_new_with_all_five_fields() {
    // 验证 5 参数 new() 构造函数:task + complexity + estimated_tokens +
    // acceptable_latency + quality_requirement
    let task = make_test_task("t-001", "实现登录模块");
    let agent_task = AgentTask::new(
        task.clone(),
        TaskComplexity::Medium,
        5000,
        Duration::from_secs(120),
        QualityLevel::High,
    );

    // 1. inner 字段正确包装 nexus_core::Task
    assert_eq!(agent_task.inner, task, "inner 字段应等于传入的 Task");
    // 2. complexity 字段
    assert_eq!(
        agent_task.complexity,
        TaskComplexity::Medium,
        "complexity 字段应为 Medium"
    );
    // 3. estimated_tokens 字段
    assert_eq!(
        agent_task.estimated_tokens, 5000,
        "estimated_tokens 应为 5000"
    );
    // 4. acceptable_latency 字段(std::time::Duration)
    assert_eq!(
        agent_task.acceptable_latency,
        Duration::from_secs(120),
        "acceptable_latency 应为 120s"
    );
    // 5. quality_requirement 字段
    assert_eq!(
        agent_task.quality_requirement,
        QualityLevel::High,
        "quality_requirement 应为 High"
    );
}

#[test]
fn test_agent_task_inner_wraps_nexus_core_task() {
    // 验证 AgentTask.inner 持有原始 Task 的所有字段(不修改核心类型)
    let task = make_test_task_with_deps(
        "t-002",
        "重构路由模块",
        vec!["t-001".into(), "t-000".into()],
    );
    let agent_task = AgentTask::new(
        task.clone(),
        TaskComplexity::Complex,
        20000,
        Duration::from_secs(600),
        QualityLevel::Production,
    );

    // 验证 Task 的所有字段被完整保留(不丢失、不修改)
    assert_eq!(agent_task.inner.task_id, "t-002");
    assert_eq!(agent_task.inner.description, "重构路由模块");
    assert_eq!(agent_task.inner.status, TaskStatus::Pending);
    assert_eq!(agent_task.inner.dependencies.len(), 2);
    assert_eq!(agent_task.inner.dependencies[0], "t-001");
    // 通过 PartialEq 验证整个 Task 相等
    assert_eq!(agent_task.inner, task);
}

#[test]
fn test_agent_task_complexity_all_variants() {
    // 验证 4 种 TaskComplexity 变体均可正确存入 AgentTask
    let task = make_test_task("t-003", "测试任务");

    let simple = AgentTask::new(
        task.clone(),
        TaskComplexity::Simple,
        100,
        Duration::from_secs(10),
        QualityLevel::Draft,
    );
    assert_eq!(simple.complexity, TaskComplexity::Simple);

    let medium = AgentTask::new(
        task.clone(),
        TaskComplexity::Medium,
        1000,
        Duration::from_secs(60),
        QualityLevel::Standard,
    );
    assert_eq!(medium.complexity, TaskComplexity::Medium);

    let complex = AgentTask::new(
        task.clone(),
        TaskComplexity::Complex,
        10000,
        Duration::from_secs(300),
        QualityLevel::High,
    );
    assert_eq!(complex.complexity, TaskComplexity::Complex);

    let very_complex = AgentTask::new(
        task,
        TaskComplexity::VeryComplex,
        100000,
        Duration::from_secs(3600),
        QualityLevel::Production,
    );
    assert_eq!(very_complex.complexity, TaskComplexity::VeryComplex);
}

#[test]
fn test_agent_task_estimated_tokens_zero_and_large() {
    // 验证 estimated_tokens 边界值:0(最小)和 1_000_000(1M,大任务)
    let task = make_test_task("t-004", "边界测试");

    let zero_tokens = AgentTask::new(
        task.clone(),
        TaskComplexity::Simple,
        0,
        Duration::from_secs(1),
        QualityLevel::Draft,
    );
    assert_eq!(zero_tokens.estimated_tokens, 0);

    let large_tokens = AgentTask::new(
        task,
        TaskComplexity::VeryComplex,
        1_000_000,
        Duration::from_secs(7200),
        QualityLevel::Production,
    );
    assert_eq!(large_tokens.estimated_tokens, 1_000_000);
}

#[test]
fn test_agent_task_acceptable_latency_is_std_duration() {
    // 验证 acceptable_latency 是 std::time::Duration(非 chrono::Duration)
    // WHY: chrono::Duration 不能用于 tokio::time::sleep,必须用 std::time::Duration
    let task = make_test_task("t-005", "延迟测试");
    let latency = Duration::from_millis(500);
    let agent_task = AgentTask::new(
        task,
        TaskComplexity::Simple,
        500,
        latency,
        QualityLevel::Standard,
    );

    assert_eq!(agent_task.acceptable_latency, latency);
    // 验证类型确实是 std::time::Duration(编译期保证,若类型不匹配则编译失败)
    let _: Duration = agent_task.acceptable_latency;
}

// ============================================================
// SubTask 11.3: 复用 Task 7 的 From<TaskComplexity> for ThinkingMode
// ============================================================

#[test]
fn test_task_complexity_to_thinking_mode_simple() {
    // 复用 Task 7.4 实现的 From<TaskComplexity> for ThinkingMode
    // Simple → Fast(快速响应)
    let mode: ThinkingMode = TaskComplexity::Simple.into();
    assert_eq!(mode, ThinkingMode::Fast);
}

#[test]
fn test_task_complexity_to_thinking_mode_medium() {
    // Medium → Standard(标准深度)
    let mode: ThinkingMode = TaskComplexity::Medium.into();
    assert_eq!(mode, ThinkingMode::Standard);
}

#[test]
fn test_task_complexity_to_thinking_mode_complex() {
    // Complex → Deep(深度推理)
    let mode: ThinkingMode = TaskComplexity::Complex.into();
    assert_eq!(mode, ThinkingMode::Deep);
}

#[test]
fn test_task_complexity_to_thinking_mode_very_complex() {
    // VeryComplex → Deep(超深度推理,与 Complex 相同 ThinkingMode,
    // 差异在 estimated_tokens / acceptable_latency 体现)
    let mode: ThinkingMode = TaskComplexity::VeryComplex.into();
    assert_eq!(mode, ThinkingMode::Deep);
}

#[test]
fn test_agent_task_complexity_consistent_with_thinking_mode() {
    // 验证 AgentTask.complexity 与 From<TaskComplexity> 映射一致
    let task = make_test_task("t-006", "一致性测试");

    let simple_task = AgentTask::new(
        task.clone(),
        TaskComplexity::Simple,
        100,
        Duration::from_secs(10),
        QualityLevel::Draft,
    );
    let expected_mode: ThinkingMode = simple_task.complexity.into();
    assert_eq!(expected_mode, ThinkingMode::Fast);

    let complex_task = AgentTask::new(
        task,
        TaskComplexity::Complex,
        10000,
        Duration::from_secs(300),
        QualityLevel::High,
    );
    let expected_mode: ThinkingMode = complex_task.complexity.into();
    assert_eq!(expected_mode, ThinkingMode::Deep);
}

// ============================================================
// AgentTask trait 派生:Clone + Debug + Serialize + Deserialize + PartialEq
// ============================================================

#[test]
fn test_agent_task_clone() {
    // 验证 Clone trait:克隆后两个实例应相等
    let task = make_test_task("t-007", "克隆测试");
    let original = AgentTask::new(
        task,
        TaskComplexity::Medium,
        2000,
        Duration::from_secs(180),
        QualityLevel::Standard,
    );

    let cloned = original.clone();
    // PartialEq 验证克隆相等
    assert_eq!(original, cloned);
    assert_eq!(cloned.inner.task_id, "t-007");
    assert_eq!(cloned.complexity, TaskComplexity::Medium);
    assert_eq!(cloned.estimated_tokens, 2000);
}

#[test]
fn test_agent_task_debug_format() {
    // 验证 Debug trait:{:?} 格式化应包含关键字段
    let task = make_test_task("t-008", "调试测试");
    let agent_task = AgentTask::new(
        task,
        TaskComplexity::Complex,
        8000,
        Duration::from_secs(400),
        QualityLevel::High,
    );

    let debug_str = format!("{:?}", agent_task);
    assert!(debug_str.contains("AgentTask"), "Debug 输出应包含类型名");
    assert!(debug_str.contains("Complex"), "Debug 输出应包含 complexity");
    assert!(
        debug_str.contains("8000"),
        "Debug 输出应包含 estimated_tokens"
    );
    assert!(
        debug_str.contains("High"),
        "Debug 输出应包含 quality_requirement"
    );
}

#[test]
fn test_agent_task_partial_eq() {
    // 验证 PartialEq trait:相同字段值的两个实例应相等,任一字段不同应不等
    let task1 = make_test_task("t-009", "相等性测试");
    let task2 = make_test_task("t-009", "相等性测试");

    let at1 = AgentTask::new(
        task1,
        TaskComplexity::Medium,
        3000,
        Duration::from_secs(150),
        QualityLevel::Standard,
    );
    let at2 = AgentTask::new(
        task2,
        TaskComplexity::Medium,
        3000,
        Duration::from_secs(150),
        QualityLevel::Standard,
    );

    // 相同字段 → 相等
    assert_eq!(at1, at2, "相同字段值的 AgentTask 应相等");

    // 任一字段不同 → 不等
    let at3 = AgentTask::new(
        make_test_task("t-009", "相等性测试"),
        TaskComplexity::Complex, // 不同 complexity
        3000,
        Duration::from_secs(150),
        QualityLevel::Standard,
    );
    assert_ne!(at1, at3, "complexity 不同应不等");

    let at4 = AgentTask::new(
        make_test_task("t-009", "相等性测试"),
        TaskComplexity::Medium,
        4000, // 不同 estimated_tokens
        Duration::from_secs(150),
        QualityLevel::Standard,
    );
    assert_ne!(at1, at4, "estimated_tokens 不同应不等");

    let at5 = AgentTask::new(
        make_test_task("t-009", "相等性测试"),
        TaskComplexity::Medium,
        3000,
        Duration::from_secs(999), // 不同 acceptable_latency
        QualityLevel::Standard,
    );
    assert_ne!(at1, at5, "acceptable_latency 不同应不等");

    let at6 = AgentTask::new(
        make_test_task("t-009", "相等性测试"),
        TaskComplexity::Medium,
        3000,
        Duration::from_secs(150),
        QualityLevel::High, // 不同 quality_requirement
    );
    assert_ne!(at1, at6, "quality_requirement 不同应不等");
}

#[test]
fn test_agent_task_serde_json_roundtrip() {
    // 验证 serde Serialize + Deserialize(JSON 往返不变量)
    let task = make_test_task("t-010", "JSON 序列化测试");
    let original = AgentTask::new(
        task,
        TaskComplexity::Complex,
        15000,
        Duration::from_secs(500),
        QualityLevel::Production,
    );

    // 序列化为 JSON
    let json = serde_json::to_string(&original).expect("AgentTask 应可序列化为 JSON");

    // 反序列化回 AgentTask
    let decoded: AgentTask = serde_json::from_str(&json).expect("JSON 应可反序列化为 AgentTask");

    // 往返后应相等(PartialEq)
    assert_eq!(original, decoded, "JSON 序列化往返后 AgentTask 应相等");
}

#[test]
fn test_agent_task_serde_msgpack_roundtrip() {
    // 验证 serde Serialize + Deserialize(MessagePack 往返,ADR-004)
    let task = make_test_task("t-011", "MessagePack 序列化测试");
    let original = AgentTask::new(
        task,
        TaskComplexity::VeryComplex,
        50000,
        Duration::from_secs(1800),
        QualityLevel::Production,
    );

    // 序列化为 MessagePack
    let bytes = rmp_serde::to_vec(&original).expect("AgentTask 应可序列化为 MessagePack");

    // 反序列化回 AgentTask
    let decoded: AgentTask =
        rmp_serde::from_slice(&bytes).expect("MessagePack 应可反序列化为 AgentTask");

    // 往返后应相等(PartialEq)
    assert_eq!(
        original, decoded,
        "MessagePack 序列化往返后 AgentTask 应相等"
    );
}

// ============================================================
// QualityLevel 不变量(4 个变体)
// ============================================================

#[test]
fn test_quality_level_all_variants() {
    // 验证 QualityLevel 4 个变体存在且可构造
    let draft = QualityLevel::Draft;
    let standard = QualityLevel::Standard;
    let high = QualityLevel::High;
    let production = QualityLevel::Production;

    // 验证变体互不相等
    assert_ne!(draft, standard);
    assert_ne!(standard, high);
    assert_ne!(high, production);
    assert_ne!(draft, production);
}

#[test]
fn test_quality_level_serde_json_roundtrip() {
    // 验证 QualityLevel JSON 序列化往返
    for level in [
        QualityLevel::Draft,
        QualityLevel::Standard,
        QualityLevel::High,
        QualityLevel::Production,
    ] {
        let json = serde_json::to_string(&level).expect("QualityLevel 应可序列化");
        let decoded: QualityLevel = serde_json::from_str(&json).expect("QualityLevel 应可反序列化");
        assert_eq!(level, decoded, "QualityLevel JSON 往回应保持相等");
    }
}

#[test]
fn test_quality_level_clone_debug() {
    // 验证 QualityLevel Clone + Copy + Debug
    // WHY QualityLevel 派生 Copy: 简单枚举无数据,Copy 语义更自然;
    //   clippy::clone_on_copy 会警告对 Copy 类型调用 clone(),因此直接赋值验证复制语义
    let level = QualityLevel::High;
    let cloned = level; // Copy 语义:直接赋值即复制(Clone trait 自动派生)
    assert_eq!(level, cloned);

    let debug_str = format!("{:?}", level);
    assert!(debug_str.contains("High"));
}
