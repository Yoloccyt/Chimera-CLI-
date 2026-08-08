//! RuntimeAuditor 第 0 维（契约遵守）消费测试（Milestone B-4）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §6 B-4 验收）：
//! "RuntimeAuditor 五维度第 0 维度（契约遵守）可产出"——平台接地规格
//! 注入后 audit 报告应包含契约遵守分数。

#![forbid(unsafe_code)]

use efficiency_monitor::auditor::RuntimeAuditor;
use nexus_contracts::platform_grounding::PlatformGroundingSpec;

/// 未注入规格 → 中性 0.5
#[test]
fn contract_compliance_neutral_without_spec() {
    let auditor = RuntimeAuditor::new();
    let report = auditor.generate_report();
    assert_eq!(report.contract_compliance, 0.5);
}

/// 全部要求满足 → 1.0
#[test]
fn contract_compliance_full_when_grounded() {
    let spec = PlatformGroundingSpec::from_doc(
        "pg-audit",
        "windows-gnu",
        "PG-ENV: 工具链就位\nPG-PATH: 路径可用",
    );
    let auditor = RuntimeAuditor::new()
        .with_grounding(spec, vec!["工具链就位".to_string(), "路径可用".to_string()]);
    let report = auditor.generate_report();
    assert_eq!(report.contract_compliance, 1.0);
}

/// 部分满足 → 覆盖比例（2 条要求满足 1 条 → 0.5）
#[test]
fn contract_compliance_partial_coverage() {
    let spec = PlatformGroundingSpec::from_doc(
        "pg-audit",
        "windows-gnu",
        "PG-ENV: 工具链就位\nPG-PATH: 路径可用",
    );
    let auditor = RuntimeAuditor::new().with_grounding(spec, vec!["工具链就位".to_string()]);
    let report = auditor.generate_report();
    assert_eq!(report.contract_compliance, 0.5);
}
