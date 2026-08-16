//! Token 证据契约集成测试 — 轨迹分段与证据链协同（v3.4.0 §5.3）
//!
//! 覆盖: 铁律9 分段身份共享 / anchor 语义 / 分段↔账本证据链 /
//! 分段创建原因全格式 roundtrip / MsgPack 体积优势验证

#![forbid(unsafe_code)]

use nexus_contracts::{SegmentCreationReason, SegmentMetadata, TokenLedgerEntry, ToolCallRecord};

/// 构造样例轨迹（1 父轨迹 = 2 分段，anchor 为分段 0）
fn sample_trajectory() -> (Vec<SegmentMetadata>, Vec<TokenLedgerEntry>) {
    let ledgers = vec![
        TokenLedgerEntry::new(
            "ledger-001",
            0,
            "session-1",
            "instance-1",
            vec![101],
            vec![201, 202],
            vec![0.9, 0.8],
            vec![true, true],
            "v2.26.0-omega",
            vec![ToolCallRecord::new("read_file", "{}", "内容", 12)],
            None,
            1_700_000_000_000,
        ),
        TokenLedgerEntry::new(
            "ledger-002",
            1,
            "session-1",
            "instance-1",
            vec![103],
            vec![203],
            vec![0.7],
            vec![true],
            "v2.26.0-omega",
            vec![],
            None,
            1_700_000_001_000,
        ),
    ];
    let segments = vec![
        SegmentMetadata::new(
            "seg-001",
            "traj-1",
            0,
            true,
            vec![Box::from("ledger-001")],
            vec![0x1F, 0x8B],
            0,
            0,
            SegmentCreationReason::NaturalBoundary,
        ),
        SegmentMetadata::new(
            "seg-002",
            "traj-1",
            1,
            false,
            vec![Box::from("ledger-002")],
            vec![],
            1,
            1,
            SegmentCreationReason::HistoryCompaction,
        ),
    ];
    (segments, ledgers)
}

// ----------------------------------------------------------
// 铁律9: 分段身份与 anchor 语义
// ----------------------------------------------------------

#[test]
fn parent_traj_id_shared_across_segments() {
    let (segments, _) = sample_trajectory();
    // 同一父轨迹的全部分段共享 parent_traj_id
    assert_eq!(segments[0].parent_traj_id, segments[1].parent_traj_id);
    assert_eq!(segments[0].parent_traj_id.as_ref(), "traj-1");
    // anchor 唯一性: 仅 anchor segment 承载终局 reward
    assert!(segments[0].is_anchor_segment());
    assert!(!segments[1].is_anchor_segment());
    // 分段序号严格递增
    assert!(segments[1].segment_index > segments[0].segment_index);
}

#[test]
fn segment_evidence_chain_resolution() {
    let (segments, ledgers) = sample_trajectory();
    // 每分段的 token_entries 必须能解析到账本条目（证据链闭环）
    let ledger_by_id: std::collections::HashMap<&str, &TokenLedgerEntry> =
        ledgers.iter().map(|l| (l.entry_id.as_ref(), l)).collect();
    for seg in &segments {
        for entry_id in &seg.token_entries {
            assert!(
                ledger_by_id.contains_key(entry_id.as_ref()),
                "分段 {} 的证据链断裂: {} 不可解析",
                seg.segment_id,
                entry_id
            );
        }
    }
}

// ----------------------------------------------------------
// 序列化与线格式
// ----------------------------------------------------------

#[test]
fn segment_msgpack_roundtrip() {
    let (segments, _) = sample_trajectory();
    for seg in &segments {
        let bytes = rmp_serde::to_vec(seg).expect("MsgPack 序列化失败");
        let back: SegmentMetadata = rmp_serde::from_slice(&bytes).expect("MsgPack 反序列化失败");
        assert_eq!(&back, seg);
    }
}

#[test]
fn ledger_msgpack_volume_advantage() {
    // 性能证据: 大载荷证据场景 MsgPack 显著小于 JSON（训练数据面存储成本）
    let (_, ledgers) = sample_trajectory();
    let json_len = serde_json::to_string(&ledgers)
        .expect("JSON 序列化失败")
        .len();
    let msgpack_len = rmp_serde::to_vec(&ledgers)
        .expect("MsgPack 序列化失败")
        .len();
    assert!(
        msgpack_len * 2 < json_len,
        "MsgPack ({msgpack_len}B) 应不足 JSON ({json_len}B) 一半"
    );
}

#[test]
fn creation_reasons_wire_format_frozen() {
    // 六类分段原因线格式冻结（跨进程传输兼容）
    let cases = [
        (
            SegmentCreationReason::HistoryCompaction,
            "history_compaction",
        ),
        (
            SegmentCreationReason::ToolSchemaChange,
            "tool_schema_change",
        ),
        (SegmentCreationReason::MessageRewrite, "message_rewrite"),
        (SegmentCreationReason::Titofallback, "titofallback"),
        (SegmentCreationReason::NaturalBoundary, "natural_boundary"),
        (
            SegmentCreationReason::MaxLengthReached,
            "max_length_reached",
        ),
    ];
    for (reason, expected) in cases {
        assert_eq!(
            serde_json::to_string(&reason).expect("JSON 序列化失败"),
            format!("\"{expected}\"")
        );
    }
}

#[test]
fn segment_wire_format_frozen() {
    let (segments, _) = sample_trajectory();
    let json = serde_json::to_string(&segments[0]).expect("JSON 序列化失败");
    // 关键字段的 JSON 形态不可漂移
    assert!(json.contains("\"parent_traj_id\":\"traj-1\""));
    assert!(json.contains("\"is_anchor\":true"));
    assert!(json.contains("\"segment_index\":0"));
    assert!(json.contains("\"creation_reason\":\"natural_boundary\""));
}

// ----------------------------------------------------------
// 边界条件
// ----------------------------------------------------------

#[test]
fn empty_segment_boundaries() {
    // 单回合分段（start == end）合法
    let single = SegmentMetadata::new(
        "seg-x",
        "traj-x",
        0,
        false,
        vec![],
        vec![],
        5,
        5,
        SegmentCreationReason::NaturalBoundary,
    );
    assert_eq!(single.turn_span(), 1);
    // 空账本（无 token 证据）合法——黑盒 Agent 无 token 返回场景
    let no_evidence = TokenLedgerEntry::new(
        "ledger-x",
        0,
        "s",
        "i",
        vec![],
        vec![],
        vec![],
        vec![],
        "v",
        vec![],
        None,
        0,
    );
    assert_eq!(no_evidence.output_len(), 0);
    assert!(!no_evidence.has_tool_calls());
}

#[test]
fn invalid_segment_rejected() {
    // 非法分段（end < start）必须被拒绝（构造器断言）
    let result = std::panic::catch_unwind(|| {
        SegmentMetadata::new(
            "bad",
            "traj-x",
            0,
            false,
            vec![],
            vec![],
            10,
            5,
            SegmentCreationReason::NaturalBoundary,
        )
    });
    assert!(result.is_err());
}
