//! 契约 smoke 测试（W2 下放）— 错误层 facade 等价 + RecoveryStrategy 往返
//!
//! WHY: safety 是 L0-A 唯一成体系错误层（NexusError/Recoverable/RecoveryStrategy），
//! 其 facade 双路径同一性必须由 owning crate 内的测试钉死，而非只靠 umbrella 集成面。
#![forbid(unsafe_code)]

use nexus_contracts::errors::{NexusError as ViaFacade, RecoveryStrategy as RSFacade};
use nexus_contracts_safety::safety::errors::{
    NexusError as ViaSibling, RecoveryStrategy as RSSibling,
};

#[test]
fn facade_and_sibling_paths_are_same_type() {
    fn same<T>(_: Option<T>, _: Option<T>) {}
    same(None::<ViaFacade>, None::<ViaSibling>);
    same(None::<RSFacade>, None::<RSSibling>);
}

#[test]
fn recovery_strategy_json_roundtrip() {
    for strategy in [
        RSSibling::Retry { max_attempts: 3 },
        RSSibling::RetryWithBackoff,
        RSSibling::FallbackToBuiltin,
    ] {
        let json = serde_json::to_string(&strategy).expect("序列化失败");
        let back: RSSibling = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(strategy, back);
    }
}
