//! 契约 smoke 测试（W2 下放）— core 域类型 serde 往返钉死
//!
//! WHY: 6 个 L0-A sibling 原先零 `tests/` 目录，契约级回归资产全在 umbrella；
//! 本文件把最小回归面下沉到 owning crate。core 无 facade dev-dep
//! （零 doctest 的既定待遇），故只做 sibling 自有路径断言。
#![forbid(unsafe_code)]

use nexus_contracts_core::core_domain::task::TaskStatus;

#[test]
fn task_status_json_roundtrip() {
    for status in [
        TaskStatus::Pending,
        TaskStatus::Running,
        TaskStatus::Completed,
        TaskStatus::Failed,
    ] {
        let json = serde_json::to_string(&status).expect("序列化失败");
        let back: TaskStatus = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(status, back);
    }
}
