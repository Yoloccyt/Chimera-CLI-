//! 契约 smoke 测试（W2 下放）— facade 双路径类型同一性 + 掩码行为/往返
//!
//! WHY: W3 双公开路径（ADR-222 D4）是「34 消费 crate 零改动」承诺的承重墙；
//! 类型同一性用 `Option<T>` 技巧在编译期证明（同 types.rs 既有模式），
//! 双路径任一漂移即编译错误。
#![forbid(unsafe_code)]

use nexus_contracts::masks::SparseMask as ViaFacade;
use nexus_contracts_sparse::sparse::masks::SparseMask as ViaSibling;

#[test]
fn facade_and_sibling_paths_are_same_type() {
    fn same<T>(_: Option<T>, _: Option<T>) {}
    same(None::<ViaFacade<String>>, None::<ViaSibling<String>>);
}

#[test]
fn select_top_k_via_facade_path() {
    let ids = vec!["a".to_string(), "b".to_string()];
    let scores = vec![0.9f32, 0.1];
    let mask = ViaFacade::select_top_k(&ids, &scores, 1);
    assert!(mask.is_active(&"a".to_string()));
    assert!(!mask.is_active(&"b".to_string()));
}

#[test]
fn mask_json_roundtrip_via_facade() {
    let ids = vec!["x".to_string(), "y".to_string()];
    let mask = ViaFacade::select_top_k(&ids, &[0.7f32, 0.2], 1);
    let json = serde_json::to_string(&mask).expect("序列化失败");
    let back: ViaFacade<String> = serde_json::from_str(&json).expect("反序列化失败");
    assert_eq!(mask, back);
}
