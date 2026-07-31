//! Task 3.10 类型一致性测试 — L0 nexus-contracts 类型扩展验证
//!
//! 对应架构: L0 Contracts(ADR-033,Task 3.10 类型扩展)
//!
//! # 测试覆盖
//!
//! 1. `test_event_metadata_serialization_roundtrip`: JSON 序列化 + 反序列化 roundtrip 一致
//! 2. `test_event_metadata_new_fields`: 验证 correlation_id / payload_version 新字段
//! 3. `test_task_status_all_variants_serializable`: 所有 6 个变体可序列化 + 反序列化
//! 4. `test_checkpoint_messagepack_roundtrip`: MessagePack(rmp-serde)序列化 roundtrip
//! 5. `test_checkpoint_description_field`: 验证 description 可选字段
//! 6. `test_nexus_contracts_zero_workspace_crate_dependencies`: 验证 L0 零 workspace crate 依赖
//!    (ADR-033 例外: serde/chrono/uuid 外部基础类型库允许;workspace crate 禁止)
//! 7. `test_backward_compat_nexus_core_reexport`: 验证 nexus-core re-export 路径仍可工作
//!
//! # TDD 守恒
//!
//! 先写失败测试(类型未定义,编译失败)→ 实现类型 → 测试通过。
//! 测试不删除已有测试,仅新增。

use nexus_contracts::{Checkpoint, EventMetadata, TaskStatus};
use serde::Serialize;

// ============================================================
// 测试 1: EventMetadata JSON 序列化 roundtrip
// ============================================================

#[test]
fn test_event_metadata_serialization_roundtrip() {
    let metadata = EventMetadata::new("test-crate");
    let json = serde_json::to_string(&metadata).expect("EventMetadata 序列化失败");
    let decoded: EventMetadata = serde_json::from_str(&json).expect("EventMetadata 反序列化失败");

    assert_eq!(
        metadata, decoded,
        "EventMetadata JSON roundtrip 后应保持一致"
    );
    assert_eq!(decoded.source, "test-crate", "source 字段应保留原值");
}

// ============================================================
// 测试 2: EventMetadata 新字段验证(correlation_id / payload_version)
// ============================================================

/// Task 3.10 新增: 验证 EventMetadata 的 correlation_id 与 payload_version 字段
#[test]
fn test_event_metadata_new_fields() {
    // 默认构造: correlation_id 为 None, payload_version 为 1
    let meta = EventMetadata::new("test-crate");
    assert_eq!(meta.correlation_id, None, "默认 correlation_id 应为 None");
    assert_eq!(meta.payload_version, 1, "默认 payload_version 应为 1");

    // 带关联 ID 构造
    let meta_with_corr = EventMetadata::with_correlation("test-crate", "quest-001-step-3");
    assert_eq!(
        meta_with_corr.correlation_id,
        Some("quest-001-step-3".to_string()),
        "with_correlation 应设置 correlation_id"
    );
    assert_eq!(meta_with_corr.payload_version, 1);

    // 序列化含 correlation_id 的元数据
    let json = serde_json::to_string(&meta_with_corr).expect("序列化失败");
    let decoded: EventMetadata = serde_json::from_str(&json).expect("反序列化失败");
    assert_eq!(decoded.correlation_id, Some("quest-001-step-3".to_string()));
    assert_eq!(decoded.payload_version, 1);

    // 序列化不含 correlation_id 的元数据 — 验证 skip_serializing_if 生效
    let json_no_corr = serde_json::to_string(&meta).expect("序列化失败");
    assert!(
        !json_no_corr.contains("correlation_id"),
        "correlation_id 为 None 时应被 skip_serializing_if 省略"
    );

    // 手工设置 payload_version
    let mut meta_v2 = EventMetadata::new("test-crate");
    meta_v2.payload_version = 2;
    let json_v2 = serde_json::to_string(&meta_v2).expect("序列化失败");
    let decoded_v2: EventMetadata = serde_json::from_str(&json_v2).expect("反序列化失败");
    assert_eq!(decoded_v2.payload_version, 2, "payload_version 应保留原值");
}

// ============================================================
// 测试 3: TaskStatus 所有 6 个变体可序列化 + 反序列化
// ============================================================

#[test]
fn test_task_status_all_variants_serializable() {
    let variants = [
        TaskStatus::Pending,
        TaskStatus::Running,
        TaskStatus::Completed,
        TaskStatus::Failed,
        TaskStatus::Cancelled,
        TaskStatus::Paused,
    ];

    for status in &variants {
        let json = serde_json::to_string(status).expect("TaskStatus 序列化失败");
        let decoded: TaskStatus = serde_json::from_str(&json).expect("TaskStatus 反序列化失败");
        assert_eq!(
            *status, decoded,
            "TaskStatus::{:?} JSON roundtrip 后应保持一致",
            status
        );
    }

    // 验证序列化字符串格式(serde 默认对 enum variant 使用 quoted string)
    assert_eq!(
        serde_json::to_string(&TaskStatus::Pending).unwrap(),
        "\"Pending\"",
        "TaskStatus::Pending 应序列化为 \"Pending\""
    );
    assert_eq!(
        serde_json::to_string(&TaskStatus::Running).unwrap(),
        "\"Running\"",
        "TaskStatus::Running 应序列化为 \"Running\""
    );
    assert_eq!(
        serde_json::to_string(&TaskStatus::Completed).unwrap(),
        "\"Completed\"",
        "TaskStatus::Completed 应序列化为 \"Completed\""
    );
    assert_eq!(
        serde_json::to_string(&TaskStatus::Failed).unwrap(),
        "\"Failed\"",
        "TaskStatus::Failed 应序列化为 \"Failed\""
    );
    assert_eq!(
        serde_json::to_string(&TaskStatus::Cancelled).unwrap(),
        "\"Cancelled\"",
        "TaskStatus::Cancelled 应序列化为 \"Cancelled\""
    );
    assert_eq!(
        serde_json::to_string(&TaskStatus::Paused).unwrap(),
        "\"Paused\"",
        "TaskStatus::Paused 应序列化为 \"Paused\""
    );
}

// ============================================================
// 测试 3: Checkpoint MessagePack(rmp-serde)序列化 roundtrip
// ============================================================

#[test]
fn test_checkpoint_messagepack_roundtrip() {
    let checkpoint = Checkpoint::new(
        "quest-001",
        "checkpoint-001",
        "sha256:abc123def456",
        vec![0x01, 0x02, 0x03, 0x04, 0x05],
    );

    // MessagePack 序列化(ADR-004: 消息序列化协议为 MessagePack)
    let msgpack_bytes = rmp_serde::to_vec(&checkpoint).expect("Checkpoint MessagePack 序列化失败");
    let decoded: Checkpoint =
        rmp_serde::from_slice(&msgpack_bytes).expect("Checkpoint MessagePack 反序列化失败");

    assert_eq!(
        checkpoint.quest_id, decoded.quest_id,
        "quest_id 字段 MessagePack roundtrip 后应保持一致"
    );
    assert_eq!(
        checkpoint.checkpoint_id, decoded.checkpoint_id,
        "checkpoint_id 字段 MessagePack roundtrip 后应保持一致"
    );
    assert_eq!(
        checkpoint.memory_snapshot_hash, decoded.memory_snapshot_hash,
        "memory_snapshot_hash 字段 MessagePack roundtrip 后应保持一致"
    );
    assert_eq!(
        checkpoint.serialized_state, decoded.serialized_state,
        "serialized_state 字段 MessagePack roundtrip 后应保持一致"
    );
    assert_eq!(
        checkpoint.created_at, decoded.created_at,
        "created_at 字段 MessagePack roundtrip 后应保持一致"
    );
    // description 默认 None,roundtrip 后应保持 None
    assert_eq!(decoded.description, None, "默认 description 应为 None");
}

// ============================================================
// 测试 4: Checkpoint description 可选字段验证
// ============================================================

/// Task 3.10 新增: 验证 Checkpoint 的 description 可选字段
#[test]
fn test_checkpoint_description_field() {
    // 不含 description 的检查点
    let cp_no_desc = Checkpoint::new("q1", "c1", "hash1", vec![1, 2, 3]);
    assert_eq!(cp_no_desc.description, None, "默认 description 应为 None");

    // 序列化不含 description 的检查点 — 验证 skip_serializing_if 生效
    let json_no_desc = serde_json::to_string(&cp_no_desc).expect("序列化失败");
    assert!(
        !json_no_desc.contains("description"),
        "description 为 None 时应被 skip_serializing_if 省略"
    );

    // 含 description 的检查点
    let cp_with_desc = Checkpoint::with_description(
        "q2",
        "c2",
        "hash2",
        vec![4, 5, 6],
        "用户登录流程完成后的检查点",
    );
    assert_eq!(
        cp_with_desc.description,
        Some("用户登录流程完成后的检查点".to_string()),
        "with_description 应设置 description"
    );

    // 序列化含 description 的检查点
    let json_with_desc = serde_json::to_string(&cp_with_desc).expect("序列化失败");
    assert!(
        json_with_desc.contains("description"),
        "description 为 Some 时应出现在 JSON 中"
    );

    let decoded: Checkpoint = serde_json::from_str(&json_with_desc).expect("反序列化失败");
    assert_eq!(
        decoded.description,
        Some("用户登录流程完成后的检查点".to_string()),
        "description roundtrip 后应保持一致"
    );
}

// ============================================================
// 测试 5: L0 零 workspace crate 依赖保证(ADR-033)
// ============================================================
//
// WHY 调整: 任务描述要求 "验证 Cargo.toml [dependencies] 为空",
// 但 ADR-033 实际允许 serde/chrono/uuid 作为基础类型库例外(无运行时业务逻辑)。
// 真正的约束是 "零 workspace crate 依赖"(禁止依赖 nexus-core/event-bus 等)。
// 本测试通过解析 Cargo.toml 验证 [dependencies] 段不包含任何 workspace path 依赖。

#[test]
fn test_nexus_contracts_zero_workspace_crate_dependencies() {
    // 读取 nexus-contracts/Cargo.toml 验证 [dependencies] 段
    let cargo_toml_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let content =
        std::fs::read_to_string(&cargo_toml_path).expect("应能读取 nexus-contracts/Cargo.toml");

    // 提取 [dependencies] 段(在 [dev-dependencies] 之前)
    let deps_section = extract_section(&content, "[dependencies]", "[dev-dependencies]")
        .expect("应存在 [dependencies] 段");

    // ADR-033 例外白名单: serde / chrono / uuid(基础类型库,无运行时业务逻辑)
    let allowed_external_deps = ["serde", "chrono", "uuid"];

    // 验证 [dependencies] 段中无任何 workspace path 依赖(即无 workspace crate 依赖)
    for line in deps_section.lines() {
        let trimmed = line.trim();
        // 跳过空行与注释
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // 检测 workspace path 依赖(形如 `nexus-core = { path = "..." }`)
        if trimmed.contains("path =") || trimmed.contains("path=") {
            panic!(
                "ADR-033 违规: [dependencies] 段发现 workspace path 依赖: {}",
                trimmed
            );
        }
        // 验证外部依赖在白名单内
        let dep_name = trimmed.split('=').next().unwrap_or("").trim();
        if !dep_name.is_empty() && !allowed_external_deps.contains(&dep_name) {
            panic!(
                "ADR-033 违规: [dependencies] 段发现非白名单外部依赖 '{}',白名单: {:?}",
                dep_name, allowed_external_deps
            );
        }
    }
}

/// 从 TOML 内容中提取指定段(start_marker 到 end_marker 之间的内容)
fn extract_section(content: &str, start_marker: &str, end_marker: &str) -> Option<String> {
    let start_idx = content.find(start_marker)?;
    let rest = &content[start_idx + start_marker.len()..];
    let end_idx = rest.find(end_marker).unwrap_or(rest.len());
    Some(rest[..end_idx].to_string())
}

// ============================================================
// 测试 6: 向后兼容验证 — nexus-core re-export 路径仍可工作
// ============================================================

#[test]
fn test_backward_compat_nexus_core_reexport() {
    // 验证 nexus_core::types::TaskStatus 与 nexus_contracts::TaskStatus 是同一类型
    // (通过 serde JSON roundtrip 间接验证类型一致性)
    let status = <nexus_core::types::TaskStatus as Serialize>::serialize(
        &nexus_core::types::TaskStatus::Running,
        serde_json::value::Serializer,
    )
    .expect("nexus_core::types::TaskStatus 序列化失败");

    let json = serde_json::to_string(&nexus_core::types::TaskStatus::Running)
        .expect("nexus_core::types::TaskStatus JSON 序列化失败");
    let contracts_decoded: TaskStatus = serde_json::from_str(&json)
        .expect("应能从 nexus_core JSON 反序列化为 nexus_contracts 类型");

    assert_eq!(
        contracts_decoded,
        TaskStatus::Running,
        "nexus_contracts::TaskStatus 与 nexus_core::types::TaskStatus 应通过 re-export 保持一致"
    );

    // 同样验证 Checkpoint 类型一致性
    let cp_core = nexus_core::types::Checkpoint::new(
        "q-backward",
        "c-backward",
        "hash-backward",
        vec![0xDE, 0xAD, 0xBE, 0xEF],
    );
    let json = serde_json::to_string(&cp_core).expect("nexus_core::Checkpoint JSON 序列化失败");
    let cp_contracts: Checkpoint = serde_json::from_str(&json)
        .expect("应能从 nexus_core JSON 反序列化为 nexus_contracts 类型");

    assert_eq!(cp_contracts.quest_id, "q-backward");
    assert_eq!(cp_contracts.checkpoint_id, "c-backward");
    assert_eq!(cp_contracts.memory_snapshot_hash, "hash-backward");
    assert_eq!(cp_contracts.serialized_state, vec![0xDE, 0xAD, 0xBE, 0xEF]);

    // 防止 unused warning
    let _ = status;
}

// ============================================================
// 测试 7: 向后兼容验证 — 旧格式 JSON 反序列化不失败
// ============================================================

/// Task 3.10 新增: 验证从旧格式 JSON(不含新增字段)反序列化不失败
///
/// 新字段均使用 Option / 默认值,旧格式 JSON 缺失这些字段时反序列化不应失败。
#[test]
fn test_backward_compatibility_old_json() {
    // 旧格式 EventMetadata JSON(不含 correlation_id / payload_version)
    let old_event_meta_json = r#"{
        "event_id": "018f3c6a-1234-7abc-8901-234567890abc",
        "timestamp": "2026-07-30T12:00:00Z",
        "source": "test-crate"
    }"#;
    let meta: EventMetadata = serde_json::from_str(old_event_meta_json)
        .expect("旧格式 EventMetadata JSON 反序列化不应失败");
    assert_eq!(meta.source, "test-crate");
    assert_eq!(meta.correlation_id, None, "缺失字段默认 None");
    assert_eq!(
        meta.payload_version, 1,
        "缺失字段默认 1(通过 default_payload_version 函数)"
    );

    // 旧格式 TaskStatus JSON(仅 4 变体,不含 Cancelled/Paused)
    let old_task_status_json = r#""Running""#;
    let status: TaskStatus = serde_json::from_str(old_task_status_json)
        .expect("旧格式 TaskStatus JSON 反序列化不应失败");
    assert_eq!(status, TaskStatus::Running);

    // 旧格式 Checkpoint JSON(不含 description)
    let old_checkpoint_json = r#"{
        "quest_id": "q-old",
        "checkpoint_id": "c-old",
        "memory_snapshot_hash": "sha256:oldhash",
        "serialized_state": [1, 2, 3],
        "created_at": "2026-07-30T12:00:00Z"
    }"#;
    let cp: Checkpoint =
        serde_json::from_str(old_checkpoint_json).expect("旧格式 Checkpoint JSON 反序列化不应失败");
    assert_eq!(cp.quest_id, "q-old");
    assert_eq!(cp.description, None, "缺失 description 字段默认 None");
}
