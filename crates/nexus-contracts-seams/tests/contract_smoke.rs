//! 契约 smoke 测试（W2 下放）— seams 值对象 facade 等价 + DensityTier 往返
//!
//! WHY: seams 是 L0-A 逻辑最重域（EWMA 状态机/策略族），其策略值对象的
//! wire 稳定性由 owning crate 内测试钉死；状态机级属性测试见 src 内 proptest（W2b）。
#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
// 集成测试惯用 expect/unwrap（G-82 文档化豁免：integration-test idiom 整文件豁免）

use nexus_contracts::density::DensityTier as ViaFacade;
use nexus_contracts_seams::seams::density::DensityTier as ViaSibling;

#[test]
fn facade_and_sibling_paths_are_same_type() {
    fn same<T>(_: Option<T>, _: Option<T>) {}
    same(None::<ViaFacade>, None::<ViaSibling>);
}

#[test]
fn density_tier_json_roundtrip() {
    for tier in [ViaSibling::Rho05] {
        let json = serde_json::to_string(&tier).expect("序列化失败");
        let back: ViaSibling = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(tier, back);
    }
}
