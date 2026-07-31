//! OSA 评分体系测试 — 验证 TaskProfile 携带评分时 Top-K 按评分排序
//!
//! 对应 Task 1:OSA 评分体系重构
//!
//! # 测试覆盖
//! 1. routing_scores 携带时 Top-K 按评分排序(非前 K 个)
//! 2. routing_scores = None 时 fallback 到 heuristic_scores(前 K 个)
//! 3. 空评分向量边界(Some(vec![]) 安全处理,不 panic)
//! 4. context_scores / memory_scores 对称性验证(三维评分来源行为一致)
//!
//! # 设计决策(WHY)
//! - 评分向量设计为**非降序**(交错评分),确保"按评分 Top-K"与"前 K 个"
//!   产生不同结果,从而有区分度地验证评分来源是否生效
//! - 默认配置 complexity=0.0 → Simple 档位 → k=8,候选集 10 个,
//!   确保 k < len 才能验证 Top-K 选择(而非全选)

#![forbid(unsafe_code)]

use event_bus::EventBus;
use osa_coordinator::{
    AffectedScope, FileId, MemoryId, OmniSparseCoordinator, RiskLevel, TaskId, TaskProfile,
    TaskType, TimePressure, ToolId,
};

// ============================================================
// 辅助函数
// ============================================================

/// 构造测试用 TaskProfile(可控候选集与评分)
///
/// - `complexity`:复杂度分数(Simple 档位 → k=8)
/// - `available_tools`/`available_files`/`available_memories`:候选集
/// - `routing_scores`/`context_scores`/`memory_scores`:可选评分
fn make_profile(
    complexity: f32,
    available_tools: Vec<ToolId>,
    available_files: Vec<FileId>,
    available_memories: Vec<MemoryId>,
    routing_scores: Option<Vec<f32>>,
    context_scores: Option<Vec<f32>>,
    memory_scores: Option<Vec<f32>>,
) -> TaskProfile {
    TaskProfile {
        task_id: TaskId::new("t-1"),
        task_type: TaskType::Read,
        complexity_score: complexity,
        risk_level: RiskLevel::Low,
        time_pressure: TimePressure::Low,
        affected_scope: AffectedScope::Local,
        available_tools,
        available_files,
        available_memories,
        recent_operations: Vec::new(),
        active_tasks: Vec::new(),
        routing_scores,
        context_scores,
        memory_scores,
        // Task 2: task_phase 默认 None,评分测试不涉及 S2 自适应
        task_phase: None,
    }
}

/// 生成 n 个工具候选集 ["tool-0", "tool-1", ..., "tool-(n-1)"]
fn make_tools(n: usize) -> Vec<ToolId> {
    (0..n).map(|i| ToolId::new(format!("tool-{i}"))).collect()
}

/// 生成 n 个文件候选集 ["file-0", ..., "file-(n-1)"]
fn make_files(n: usize) -> Vec<FileId> {
    (0..n).map(|i| FileId::new(format!("file-{i}"))).collect()
}

/// 生成 n 个记忆候选集 ["mem-0", ..., "mem-(n-1)"]
fn make_memories(n: usize) -> Vec<MemoryId> {
    (0..n).map(|i| MemoryId::new(format!("mem-{i}"))).collect()
}

// ============================================================
// 测试 1:routing_scores 携带时 Top-K 按评分排序(非前 K 个)
// ============================================================

/// 验证 routing_scores = Some(vec) 时,Top-K 按真实评分降序选择
///
/// 设计:10 个工具,交错评分(非降序),Simple 档位 k=8
/// - 评分:[0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6, 0.5, 0.0]
/// - 期望 Top-8(按评分降序):tool-1(0.9), tool-3(0.8), tool-5(0.7),
///   tool-7(0.6), tool-8(0.5), tool-6(0.4), tool-4(0.3), tool-2(0.2)
/// - 被排除:tool-0(0.1), tool-9(0.0)
///
/// WHY 交错评分:若评分恰好降序,则"按评分 Top-K"与"前 K 个"结果相同,
/// 无法区分两种行为。交错评分确保两种策略产生不同结果。
#[test]
fn test_routing_scores_selects_by_score_not_index() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    let tools = make_tools(10);
    // 交错评分:非降序,确保按评分排序与"前 K 个"不同
    let scores = vec![0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6, 0.5, 0.0];
    let profile = make_profile(
        0.0, // Simple 档位 → k=8
        tools.clone(),
        Vec::new(),
        Vec::new(),
        Some(scores),
        None,
        None,
    );

    let mask = coord.compute_routing_mask(&profile);

    // Top-K 数量 = 8(k=8 < len=10)
    assert_eq!(mask.active_count(), 8, "Simple 档位 k=8, 应选 8 个工具");

    // 最高分 tool-1(0.9)应排第一
    assert_eq!(
        mask.active_ids[0],
        ToolId::new("tool-1"),
        "最高分 tool-1(0.9) 应排第一"
    );
    // 第二高 tool-3(0.8)
    assert_eq!(
        mask.active_ids[1],
        ToolId::new("tool-3"),
        "第二高 tool-3(0.8) 应排第二"
    );

    // 低分工具应被排除:tool-0(0.1)和 tool-9(0.0)不在 Top-8
    assert!(
        !mask.is_active(&ToolId::new("tool-0")),
        "tool-0(0.1) 评分过低应被排除"
    );
    assert!(
        !mask.is_active(&ToolId::new("tool-9")),
        "tool-9(0.0) 评分最低应被排除"
    );

    // 高分工具应在 Top-8
    assert!(
        mask.is_active(&ToolId::new("tool-1")),
        "tool-1(0.9) 最高分应在 Top-8"
    );
}

/// 验证 routing_scores = Some(vec) 时 Top-K 完整顺序按评分降序
#[test]
fn test_routing_scores_full_ordering() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    let tools = make_tools(10);
    let scores = vec![0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6, 0.5, 0.0];
    let profile = make_profile(0.0, tools, Vec::new(), Vec::new(), Some(scores), None, None);

    let mask = coord.compute_routing_mask(&profile);

    // 期望顺序(按评分降序):tool-1, tool-3, tool-5, tool-7, tool-8, tool-6, tool-4, tool-2
    let expected = [
        ToolId::new("tool-1"), // 0.9
        ToolId::new("tool-3"), // 0.8
        ToolId::new("tool-5"), // 0.7
        ToolId::new("tool-7"), // 0.6
        ToolId::new("tool-8"), // 0.5
        ToolId::new("tool-6"), // 0.4
        ToolId::new("tool-4"), // 0.3
        ToolId::new("tool-2"), // 0.2
    ];
    assert_eq!(mask.active_ids, expected, "Top-8 应按评分降序排列");
}

// ============================================================
// 测试 2:routing_scores = None 时 fallback 到 heuristic_scores
// ============================================================

/// 验证 routing_scores = None 时,Top-K 退化为"前 K 个"(heuristic_scores)
///
/// heuristic_scores(len=10) = [1.0, 0.9, 0.8, ..., 0.1](索引负相关)
/// Simple 档位 k=8 → 前 8 个:tool-0 到 tool-7
#[test]
fn test_routing_scores_none_fallback_to_heuristic() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    let tools = make_tools(10);
    let profile = make_profile(
        0.0, // Simple 档位 → k=8
        tools.clone(),
        Vec::new(),
        Vec::new(),
        None, // 无评分 → fallback 到 heuristic_scores
        None,
        None,
    );

    let mask = coord.compute_routing_mask(&profile);

    // Top-K 数量 = 8
    assert_eq!(mask.active_count(), 8, "Simple 档位 k=8, 应选 8 个工具");

    // heuristic_scores 使前 K 个被选:tool-0 排第一(评分最高 1.0)
    assert_eq!(
        mask.active_ids[0],
        ToolId::new("tool-0"),
        "heuristic_scores 使 tool-0(索引 0)排第一"
    );

    // 前 8 个被选,后 2 个被排除
    assert!(
        mask.is_active(&ToolId::new("tool-7")),
        "tool-7 在前 8 个中应被选"
    );
    assert!(
        !mask.is_active(&ToolId::new("tool-8")),
        "tool-8 不在前 8 个中应被排除"
    );
    assert!(
        !mask.is_active(&ToolId::new("tool-9")),
        "tool-9 不在前 8 个中应被排除"
    );
}

// ============================================================
// 测试 3:空评分向量边界(Some(vec![]) 安全处理,不 panic)
// ============================================================

/// 验证 Some(vec![]) + 非空候选集(k < total)时安全降级为空掩码(不 panic)
///
/// WHY:scores.len() (0) != ids.len() (10),select_top_k 防御性返回空掩码。
/// 注意:必须用 k < total 的场景(10 个工具,Simple 档位 k=8),
/// 因为 select_top_k 在 k >= total 时会跳过 scores 长度检查直接返回全掩码
#[test]
fn test_empty_scores_vector_with_nonempty_candidates() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    // 用 10 个工具确保 k=8 < total=10,scores 长度检查才会生效
    let tools = make_tools(10);
    let profile = make_profile(
        0.0, // Simple 档位 → k=8
        tools,
        Vec::new(),
        Vec::new(),
        Some(Vec::new()), // 空评分向量
        None,
        None,
    );

    // 不应 panic,因 scores 长度不匹配返回空掩码
    let mask = coord.compute_routing_mask(&profile);
    assert_eq!(
        mask.active_count(),
        0,
        "空评分向量 + 非空候选集(k<total)应降级为空掩码"
    );
}

/// 验证 Some(vec![]) + 候选集数量 ≤ k 时返回全掩码(不 panic)
///
/// WHY:select_top_k 在 k >= total 时跳过 scores 长度检查直接返回全掩码,
/// 这是 select_top_k 的既有行为(全选时 scores 不影响结果)。
/// 此测试文档化该边界行为,确保 Some(vec![]) 在此场景也不 panic
#[test]
fn test_empty_scores_when_k_exceeds_total() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    // 3 个工具,Simple 档位 k=8 >= total=3 → 全选(跳过 scores 检查)
    let tools = make_tools(3);
    let profile = make_profile(
        0.0,
        tools.clone(),
        Vec::new(),
        Vec::new(),
        Some(Vec::new()), // 空评分向量,但 k>=total 时不影响
        None,
        None,
    );

    let mask = coord.compute_routing_mask(&profile);
    // k >= total → 全选,不 panic
    assert_eq!(
        mask.active_count(),
        3,
        "k>=total 时全选,空评分向量不影响结果"
    );
}

/// 验证 Some(vec![]) + 空候选集时安全返回空掩码(不 panic)
#[test]
fn test_empty_scores_vector_with_empty_candidates() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    let profile = make_profile(
        0.0,
        Vec::new(), // 空候选集
        Vec::new(),
        Vec::new(),
        Some(Vec::new()), // 空评分向量
        None,
        None,
    );

    let mask = coord.compute_routing_mask(&profile);
    assert_eq!(mask.active_count(), 0, "空候选集 + 空评分向量应返回空掩码");
}

/// 验证 None + 空候选集时安全返回空掩码(不 panic)
#[test]
fn test_none_scores_with_empty_candidates() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    let profile = make_profile(
        0.0,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None, // 无评分
        None,
        None,
    );

    let mask = coord.compute_routing_mask(&profile);
    assert_eq!(mask.active_count(), 0, "空候选集应返回空掩码");
}

/// 验证评分长度与候选集长度不匹配时安全降级(不 panic)
///
/// WHY:scores.len() (3) != ids.len() (10),select_top_k 防御性返回空掩码。
/// 注意:必须用 k < total 的场景(10 个工具,Simple 档位 k=8),
/// 因为 select_top_k 在 k >= total 时会跳过 scores 长度检查直接返回全掩码
#[test]
fn test_scores_length_mismatch() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    // 用 10 个工具确保 k=8 < total=10,scores 长度检查才会生效
    let tools = make_tools(10);
    let profile = make_profile(
        0.0, // Simple 档位 → k=8
        tools,
        Vec::new(),
        Vec::new(),
        Some(vec![0.1, 0.2, 0.3]), // 长度 3 != 候选集 10
        None,
        None,
    );

    let mask = coord.compute_routing_mask(&profile);
    assert_eq!(
        mask.active_count(),
        0,
        "评分长度与候选集不匹配(k<total)应降级为空掩码"
    );
}

// ============================================================
// 测试 4:context_scores / memory_scores 对称性验证
// ============================================================

/// 验证 context_scores 携带时 Top-K 按评分排序(与 routing 行为一致)
///
/// WHY:三维评分来源逻辑相同(都走 unwrap_or(&heuristic) 模式),
/// 验证 context 维度的对称性确保三个 compute_*_mask 方法行为一致
#[test]
fn test_context_scores_selects_by_score() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    // Simple 档位 → context_scope = 1(只选 1 个文件)
    // 用 3 个文件,评分交错,验证最高分被选
    let files = make_files(3);
    let scores = vec![0.1, 0.9, 0.2]; // file-1 最高分
    let profile = make_profile(
        0.0, // Simple → context k=1
        Vec::new(),
        files,
        Vec::new(),
        None,
        Some(scores),
        None,
    );

    let mask = coord.compute_context_mask(&profile);

    // Simple 档位 context 只选 1 个
    assert_eq!(mask.active_count(), 1, "Simple 档位 context k=1");
    // 最高分 file-1(0.9)应被选
    assert_eq!(
        mask.active_ids[0],
        FileId::new("file-1"),
        "最高分 file-1(0.9) 应被选"
    );
}

/// 验证 memory_scores 携带时 Top-K 按评分排序(与 routing 行为一致)
#[test]
fn test_memory_scores_selects_by_score() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    // Simple 档位 → memory k=8(routing_top_k_for),10 个记忆选 8 个
    let memories = make_memories(10);
    let scores = vec![0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6, 0.5, 0.0];
    let profile = make_profile(
        0.0, // Simple → memory k=8
        Vec::new(),
        Vec::new(),
        memories,
        None,
        None,
        Some(scores),
    );

    let mask = coord.compute_memory_mask(&profile);

    assert_eq!(mask.active_count(), 8, "Simple 档位 memory k=8");
    // 最高分 mem-1(0.9)应排第一
    assert_eq!(
        mask.active_ids[0],
        MemoryId::new("mem-1"),
        "最高分 mem-1(0.9) 应排第一"
    );
    // 最低分 mem-9(0.0)应被排除
    assert!(
        !mask.is_active(&MemoryId::new("mem-9")),
        "mem-9(0.0) 评分最低应被排除"
    );
}

/// 验证 context_scores = None 时 fallback 到 heuristic_scores
#[test]
fn test_context_scores_none_fallback() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    let files = make_files(10);
    let profile = make_profile(
        0.0, // Simple → context k=1
        Vec::new(),
        files,
        Vec::new(),
        None,
        None, // 无评分 → fallback
        None,
    );

    let mask = coord.compute_context_mask(&profile);

    // Simple 档位 context 只选 1 个,heuristic 使 file-0(索引 0)被选
    assert_eq!(mask.active_count(), 1, "Simple 档位 context k=1");
    assert_eq!(
        mask.active_ids[0],
        FileId::new("file-0"),
        "heuristic_scores 使 file-0 被选"
    );
}

/// 验证 memory_scores = None 时 fallback 到 heuristic_scores
#[test]
fn test_memory_scores_none_fallback() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    let memories = make_memories(10);
    let profile = make_profile(
        0.0, // Simple → memory k=8
        Vec::new(),
        Vec::new(),
        memories,
        None,
        None,
        None, // 无评分 → fallback
    );

    let mask = coord.compute_memory_mask(&profile);

    assert_eq!(mask.active_count(), 8, "Simple 档位 memory k=8");
    // heuristic 使前 8 个被选
    assert_eq!(
        mask.active_ids[0],
        MemoryId::new("mem-0"),
        "heuristic_scores 使 mem-0 排第一"
    );
    assert!(
        !mask.is_active(&MemoryId::new("mem-9")),
        "mem-9 不在前 8 个中应被排除"
    );
}

// ============================================================
// 测试 5:serde 向后兼容性验证
// ============================================================

/// 验证旧版序列化数据(无评分字段)能反序列化为 None(向后兼容)
///
/// WHY:`#[serde(default)]` 保证旧数据反序列化时评分字段为 None,
/// 不破坏已有的 TaskProfile 序列化数据
#[test]
fn test_serde_backward_compatibility_no_scores() {
    // 旧版 JSON(无 routing_scores/context_scores/memory_scores 字段)
    let old_json = r#"{
        "task_id": "t-legacy",
        "task_type": "Read",
        "complexity_score": 0.5,
        "risk_level": "Medium",
        "time_pressure": "Low",
        "affected_scope": "Local",
        "available_tools": [],
        "available_files": [],
        "available_memories": [],
        "recent_operations": [],
        "active_tasks": []
    }"#;

    let profile: TaskProfile = serde_json::from_str(old_json).expect("旧版 JSON 应能反序列化");

    // 评分字段应为 None(serde default)
    assert!(
        profile.routing_scores.is_none(),
        "旧版数据无 routing_scores,应反序列化为 None"
    );
    assert!(
        profile.context_scores.is_none(),
        "旧版数据无 context_scores,应反序列化为 None"
    );
    assert!(
        profile.memory_scores.is_none(),
        "旧版数据无 memory_scores,应反序列化为 None"
    );
}

/// 验证携带评分的 TaskProfile 能正确序列化/反序列化往返
#[test]
fn test_serde_roundtrip_with_scores() {
    let profile = make_profile(
        0.3,
        make_tools(3),
        Vec::new(),
        Vec::new(),
        Some(vec![0.1, 0.9, 0.2]),
        None,
        None,
    );

    let json = serde_json::to_string(&profile).expect("序列化应成功");
    let restored: TaskProfile = serde_json::from_str(&json).expect("反序列化应成功");

    assert_eq!(profile, restored, "序列化往返应保持一致");
    assert_eq!(
        restored.routing_scores,
        Some(vec![0.1, 0.9, 0.2]),
        "routing_scores 应正确往返"
    );
}

/// 验证 None 评分字段不被序列化(skip_serializing_if)
#[test]
fn test_serde_skip_none_scores() {
    let profile = make_profile(0.3, make_tools(3), Vec::new(), Vec::new(), None, None, None);

    let json = serde_json::to_string(&profile).expect("序列化应成功");
    // None 字段不应出现在 JSON 中
    assert!(
        !json.contains("routing_scores"),
        "None 的 routing_scores 不应被序列化"
    );
    assert!(
        !json.contains("context_scores"),
        "None 的 context_scores 不应被序列化"
    );
    assert!(
        !json.contains("memory_scores"),
        "None 的 memory_scores 不应被序列化"
    );
}
