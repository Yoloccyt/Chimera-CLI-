//! 契约 smoke 测试（W2 下放）— memory 契约 facade 等价 + 归档单调/配额错误钉死
//!
//! WHY: memory 承载 Ω-Card 契约（ExperienceCard/TokenLedger），且是 W1 错误模型
//! 统一（QuotaError/InvariantViolation）的落地 crate——本文件把 facade 双路径
//! 同一性与错误语义钉在 owning crate 内。
#![forbid(unsafe_code)]

use nexus_contracts::quota::NamespaceQuota as ViaFacade;
use nexus_contracts_memory::memory::archive_monotonicity::{
    assert_archive_monotonicity, ArchiveTier,
};
use nexus_contracts_memory::memory::quota::NamespaceQuota as ViaSibling;
use nexus_contracts_memory::memory::quota::QuotaError;

#[test]
fn facade_and_sibling_paths_are_same_type() {
    fn same<T>(_: Option<T>, _: Option<T>) {}
    same(None::<ViaFacade>, None::<ViaSibling>);
}

#[test]
fn archive_monotonicity_holds_and_violates() {
    assert!(assert_archive_monotonicity(ArchiveTier::Hot, ArchiveTier::Cold).is_ok());
    let err = assert_archive_monotonicity(ArchiveTier::Ice, ArchiveTier::Warm)
        .expect_err("Ice->Warm 回升必须违反 INV-8");
    assert!(err.msg.contains("INV-8"));
}

#[test]
fn quota_error_display_preserved() {
    // W1/ADR-305：String 错误结构化后，人类可读消息经 Display 保留。
    assert!(QuotaError::EmptyNamespaceId
        .to_string()
        .contains("namespace_id"));
}

#[test]
fn namespace_quota_validate_roundtrip() {
    let quota = ViaFacade::new("smoke-ns");
    assert!(quota.validate().is_ok());
    let json = serde_json::to_string(&quota).expect("序列化失败");
    let back: ViaFacade = serde_json::from_str(&json).expect("反序列化失败");
    assert_eq!(quota, back);
}
