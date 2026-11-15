//! 契约 smoke 测试（W2 下放）— mca 契约 facade 等价 + ProtocolDialect 往返
//!
//! WHY: mca 契约面（渠道亲和/协议方言）由 hcw-window/mca-gateway 跨层共享，
//! 方言枚举的 wire 稳定性须在 owning crate 内钉死。
#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
// 集成测试惯用 expect/unwrap（G-82 文档化豁免：integration-test idiom 整文件豁免）

use nexus_contracts::affinity::ProtocolDialect as ViaFacade;
use nexus_contracts_mca::mca::affinity::ProtocolDialect as ViaSibling;

#[test]
fn facade_and_sibling_paths_are_same_type() {
    fn same<T>(_: Option<T>, _: Option<T>) {}
    same(None::<ViaFacade>, None::<ViaSibling>);
}

#[test]
fn protocol_dialect_json_roundtrip() {
    for dialect in [
        ViaSibling::OpenAiChat,
        ViaSibling::AnthropicMessages,
        ViaSibling::OpenAiResponses,
    ] {
        let json = serde_json::to_string(&dialect).expect("序列化失败");
        let back: ViaSibling = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(dialect, back);
    }
}
