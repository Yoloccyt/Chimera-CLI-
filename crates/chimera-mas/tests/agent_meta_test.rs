//! Task 7.1 (TDD RED) — Agent 元数据与类型失败测试
//!
//! 对应 SubTask 7.1: AgentMeta/AgentType/AgentStatus 序列化、Eq、Clone
//! 对应 SubTask 7.4: TaskComplexity → ThinkingMode 映射不变量
//!
//! ## TDD 阶段
//!
//! - **RED**: 当前阶段,测试应全部失败(方法尚未实现)
//! - **GREEN**: SubTask 7.2-7.4 实现后,测试应全部通过
//!
//! ## 架构合规
//!
//! - 复用 `nexus_core::ThinkingMode`(ADR-026 决策 6,不新建 ThinkingMode::Max)
//! - AgentMeta/AgentType/AgentStatus 定义在 chimera-mas crate 内(§A.3.4 不下沉 nexus-core)
//! - 序列化兼容(serde_json + rmp-serde 往返,ADR-004)

#![forbid(unsafe_code)]

use chimera_mas::prelude::*;
use nexus_core::ThinkingMode;

// ============================================================
// SubTask 7.1: AgentType / AgentStatus 基础不变量
// ============================================================

#[test]
fn test_agent_type_root_orchestrator_depth() {
    let agent_type = AgentType::RootOrchestrator;
    assert_eq!(agent_type.depth(), 0, "RootOrchestrator depth 必须为 0");
}

#[test]
fn test_agent_type_main_agent_depth() {
    let agent_type = AgentType::MainAgent {
        domain: "frontend".into(),
    };
    assert_eq!(agent_type.depth(), 1, "MainAgent depth 必须为 1");
}

#[test]
fn test_agent_type_sub_agent_depth() {
    let agent_type = AgentType::SubAgent {
        parent_id: "agent-1".into(),
        task_scope: "implement-api".into(),
    };
    assert_eq!(agent_type.depth(), 2, "SubAgent depth 必须为 2");
}

#[test]
fn test_agent_type_grand_agent_depth() {
    let agent_type = AgentType::GrandAgent {
        parent_id: "agent-2".into(),
        task_scope: "refactor-module".into(),
    };
    assert_eq!(agent_type.depth(), 3, "GrandAgent depth 必须为 3");
}

#[test]
fn test_agent_type_expert_agent_depth() {
    // ExpertAgent 不参与层级委托,depth 应为 0(独立咨询角色)
    let agent_type = AgentType::ExpertAgent {
        specialty: vec!["security".into(), "cryptography".into()],
    };
    assert_eq!(
        agent_type.depth(),
        0,
        "ExpertAgent 不参与层级委托,depth 必须为 0"
    );
}

#[test]
fn test_agent_status_eq_clone_copy() {
    let s1 = AgentStatus::Idle;
    let s2 = s1; // Copy
    assert_eq!(s1, s2, "AgentStatus 应派生 Copy + Eq");

    let s3 = AgentStatus::Running;
    assert_ne!(s1, s3, "Idle ≠ Running");
}

// ============================================================
// SubTask 7.1: ModelConfig Default 实现
// ============================================================

#[test]
fn test_model_config_default() {
    let cfg = ModelConfig::default();
    // 默认配置应有合理值,不能 panic
    assert!(
        cfg.max_tokens > 0,
        "默认 max_tokens 必须大于 0,实际: {}",
        cfg.max_tokens
    );
    assert!(
        (0.0..=2.0).contains(&cfg.temperature),
        "默认 temperature 必须在 [0.0, 2.0] 范围内,实际: {}",
        cfg.temperature
    );
    assert!(!cfg.provider.is_empty(), "默认 provider 不能为空字符串");
    assert!(!cfg.model.is_empty(), "默认 model 不能为空字符串");
    assert_eq!(
        cfg.thinking_mode,
        ThinkingMode::Standard,
        "默认 thinking_mode 必须为 Standard(平衡速度与深度)"
    );
}

// ============================================================
// SubTask 7.1: AgentMeta::new() 构造函数
// ============================================================

#[test]
fn test_agent_meta_new_root_orchestrator() {
    let meta = AgentMeta::new_root_orchestrator("root-1");
    assert_eq!(meta.agent_id, "root-1");
    assert_eq!(meta.agent_type, AgentType::RootOrchestrator);
    assert_eq!(meta.depth, 0, "RootOrchestrator depth 必须为 0");
    assert_eq!(meta.status, AgentStatus::Idle, "新创建的 Agent 必须 Idle");
    assert!(meta.parent_id.is_none(), "RootOrchestrator 无父 Agent");
    assert!(
        meta.children_ids.is_empty(),
        "新创建的 Agent 不应有子 Agent"
    );
    assert_eq!(
        meta.context_window, 1_048_576,
        "1M Token 等效上下文窗口(128K 实际 + 8× 稀疏压缩)"
    );
}

#[test]
fn test_agent_meta_new_main_agent() {
    let parent_id = "root-1";
    let meta = AgentMeta::new_main_agent("main-1", parent_id, "backend", ModelConfig::default());
    assert_eq!(meta.agent_id, "main-1");
    assert_eq!(
        meta.agent_type,
        AgentType::MainAgent {
            domain: "backend".into()
        }
    );
    assert_eq!(meta.depth, 1, "MainAgent depth 必须为 1");
    assert_eq!(meta.status, AgentStatus::Idle);
    assert_eq!(meta.parent_id.as_deref(), Some(parent_id));
}

#[test]
fn test_agent_meta_new_sub_agent() {
    let parent_id = "main-1";
    let meta =
        AgentMeta::new_sub_agent("sub-1", parent_id, "implement-api", ModelConfig::default());
    assert_eq!(meta.agent_id, "sub-1");
    assert_eq!(
        meta.agent_type,
        AgentType::SubAgent {
            parent_id: parent_id.into(),
            task_scope: "implement-api".into()
        }
    );
    assert_eq!(meta.depth, 2, "SubAgent depth 必须为 2");
    assert_eq!(meta.parent_id.as_deref(), Some(parent_id));
}

// ============================================================
// SubTask 7.1: AgentMeta 序列化往返(serde_json + rmp-serde)
// ============================================================

#[test]
fn test_agent_meta_serde_json_roundtrip() {
    let meta = AgentMeta::new_root_orchestrator("root-roundtrip");
    let json = serde_json::to_string(&meta).expect("AgentMeta 必须可 JSON 序列化");
    let restored: AgentMeta = serde_json::from_str(&json).expect("AgentMeta 必须可 JSON 反序列化");
    assert_eq!(meta.agent_id, restored.agent_id);
    assert_eq!(meta.agent_type, restored.agent_type);
    assert_eq!(meta.depth, restored.depth);
    assert_eq!(meta.status, restored.status);
}

#[test]
fn test_agent_meta_serde_msgpack_roundtrip() {
    // MessagePack 序列化往返(ADR-004 跨层通信协议)
    let meta =
        AgentMeta::new_main_agent("main-msgpack", "root-1", "database", ModelConfig::default());
    let bytes = rmp_serde::to_vec(&meta).expect("AgentMeta 必须可 MessagePack 序列化");
    let restored: AgentMeta =
        rmp_serde::from_slice(&bytes).expect("AgentMeta 必须可 MessagePack 反序列化");
    assert_eq!(meta.agent_id, restored.agent_id);
    assert_eq!(meta.depth, restored.depth);
    assert_eq!(meta.parent_id, restored.parent_id);
}

// ============================================================
// SubTask 7.1: AgentType 序列化往返
// ============================================================

#[test]
fn test_agent_type_serde_json_roundtrip() {
    let cases = vec![
        AgentType::RootOrchestrator,
        AgentType::MainAgent {
            domain: "frontend".into(),
        },
        AgentType::SubAgent {
            parent_id: "p1".into(),
            task_scope: "scope".into(),
        },
        AgentType::GrandAgent {
            parent_id: "p2".into(),
            task_scope: "scope2".into(),
        },
        AgentType::ExpertAgent {
            specialty: vec!["sec".into()],
        },
    ];
    for original in cases {
        let json = serde_json::to_string(&original).expect("AgentType 必须可 JSON 序列化");
        let restored: AgentType =
            serde_json::from_str(&json).expect("AgentType 必须可 JSON 反序列化");
        assert_eq!(original, restored, "AgentType 序列化往返不一致: {json}");
    }
}

// ============================================================
// SubTask 7.4: TaskComplexity → ThinkingMode 映射不变量
// ADR-026 决策 6: Simple→Fast, Medium→Standard, Complex/VeryComplex→Deep
// ============================================================

#[test]
fn test_task_complexity_to_thinking_mode_simple() {
    let mode: ThinkingMode = TaskComplexity::Simple.into();
    assert_eq!(mode, ThinkingMode::Fast, "Simple 必须 → Fast");
}

#[test]
fn test_task_complexity_to_thinking_mode_medium() {
    let mode: ThinkingMode = TaskComplexity::Medium.into();
    assert_eq!(mode, ThinkingMode::Standard, "Medium 必须 → Standard");
}

#[test]
fn test_task_complexity_to_thinking_mode_complex() {
    let mode: ThinkingMode = TaskComplexity::Complex.into();
    assert_eq!(mode, ThinkingMode::Deep, "Complex 必须 → Deep");
}

#[test]
fn test_task_complexity_to_thinking_mode_very_complex() {
    let mode: ThinkingMode = TaskComplexity::VeryComplex.into();
    assert_eq!(mode, ThinkingMode::Deep, "VeryComplex 必须 → Deep");
}

// ============================================================
// SubTask 7.1: AgentStatus 序列化往返
// ============================================================

#[test]
fn test_agent_status_serde_json_roundtrip() {
    let cases = vec![
        AgentStatus::Idle,
        AgentStatus::Running,
        AgentStatus::Paused,
        AgentStatus::Completed,
        AgentStatus::Failed,
        AgentStatus::Crashed,
    ];
    for original in cases {
        let json = serde_json::to_string(&original).expect("AgentStatus 必须可 JSON 序列化");
        let restored: AgentStatus =
            serde_json::from_str(&json).expect("AgentStatus 必须可 JSON 反序列化");
        assert_eq!(original, restored, "AgentStatus 序列化往返不一致: {json}");
    }
}

// ============================================================
// SubTask 7.1: AgentMeta Clone 后相等
// ============================================================

#[test]
fn test_agent_meta_clone_equal() {
    let meta = AgentMeta::new_root_orchestrator("root-clone");
    let cloned = meta.clone();
    assert_eq!(meta.agent_id, cloned.agent_id);
    assert_eq!(meta.depth, cloned.depth);
    assert_eq!(meta.status, cloned.status);
}

// ============================================================
// SubTask 7.1: AgentType depth 与 AgentMeta depth 一致性
// ============================================================

#[test]
fn test_agent_meta_depth_matches_agent_type_depth() {
    let root = AgentMeta::new_root_orchestrator("root");
    assert_eq!(root.depth, root.agent_type.depth());

    let main = AgentMeta::new_main_agent("main", "root", "backend", ModelConfig::default());
    assert_eq!(main.depth, main.agent_type.depth());

    let sub = AgentMeta::new_sub_agent("sub", "main", "scope", ModelConfig::default());
    assert_eq!(sub.depth, sub.agent_type.depth());
}
