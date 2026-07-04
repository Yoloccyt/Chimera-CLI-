//! SubTask 15.2:MLC 边界测试(空、满、超容)
//!
//! 验证 MLC 四级记忆在边界状态下的行为正确性。
//!
//! 注:以下边界场景已被现有测试覆盖,此处不重复编写:
//! - L0 满时 insert 触发 LRU 驱逐:tests/working_memory.rs::test_l0_lru_eviction_65_entries
//!   (验证 64 容量插入 65 条目后最久未访问的被驱逐)
//! - L2 recall_by_clv top_k=0 返回空 Vec:tests/semantic.rs::test_l2_recall_by_clv_zero_top_k
//! - L2 recall_by_clv 空记忆返回空:tests/semantic.rs::test_l2_recall_by_clv_empty

use mlc_engine::{MlcError, PatternSignature, ProceduralMemory, WorkingMemory};

/// L0 空时 get 返回 EntryNotFound(断言错误类型,而非仅 is_err)
///
/// 补充 tests/working_memory.rs::test_l0_get_nonexistent_returns_error,
/// 后者仅断言 is_err(),此处显式断言错误类型为 EntryNotFound,
/// 防止未来错误类型变更导致回归(如误改为 TierOverflow)。
#[test]
fn test_boundary_l0_empty_get_returns_entry_not_found() {
    let mem = WorkingMemory::new(64);
    let result = mem.get("nonexistent");
    assert!(
        matches!(result, Err(MlcError::EntryNotFound(_))),
        "空 L0 get 不存在 ID 应返回 EntryNotFound,实际: {result:?}"
    );
}

/// L3 match_pattern 空签名返回 None
///
/// 补充 tests/procedural.rs::test_l3_match_pattern_nonexistent,
/// 后者用非空签名(tool_sequence 非空),此处测试空签名边界:
/// tool_sequence 为空 Vec、context_hash 为空字符串,确保不 panic 且返回 None。
#[tokio::test]
async fn test_boundary_l3_match_pattern_empty_signature_returns_none() {
    let mem = ProceduralMemory::open_in_memory().unwrap();
    let empty_sig = PatternSignature::new(vec![], "");
    let result = mem.match_pattern(&empty_sig).await.unwrap();
    assert!(
        result.is_none(),
        "空签名 match_pattern 应返回 None,实际: {result:?}"
    );
}
