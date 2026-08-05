//! `chimera audit` — 红队安全审计,真实接入 L8 parliament::ahirt crate
//!
//! v2.9.0-omega Task 1.9:调用 AHIRT 红队执行 4 类攻击向量探测。
//!
//! # 流程
//! 1. 构造 AhirtRedTeam(默认载荷库 + 默认 EventBus)
//! 2. 调用 `verify_security()` 执行全量探测(100 个载荷 × 4 类攻击向量)
//! 3. 输出安全报告(统计 + 漏洞清单 + 修复建议)
//!
//! # 攻击向量(4 类)
//! - `PromptInjection` — 提示词注入(绕过系统提示词约束)
//! - `CommandInjection` — 命令注入(§6 红线:命令插值禁用)
//! - `PrivilegeEscalation` — 权限提升(绕过能力衰减)
//! - `SandboxEscape` — 沙箱逃逸(绕过 SecCore gVisor 沙箱)
//!
//! # 设计决策(WHY)
//! - **进程内 ephemeral 红队**:与 `chimera run` 一致,每次调用创建独立 AhirtRedTeam。
//!   探测结果不跨进程保留,适合"快速审计看安全状态"场景。
//! - **`--severity` 过滤**:支持 `critical` / `high` / `medium` / `low` 4 级过滤,
//!   默认输出全部严重度。当前实现将所有漏洞类型视为 `high`(AHIRT 探测的是攻击向量,
//!   未拦截即为高风险)。
//! - **报告输出到 stderr,统计摘要到 stdout**:WHY 分流 — 详细报告是诊断信息
//!   (人类可读),统计摘要是数据(stdout 便于 `jq` 消费)。

use std::sync::Arc;

use anyhow::Result;
use parliament::ahirt::{AhirtRedTeam, SecurityReport};
// ADR-054 决策 3(P9-T4):seccore 实现 L0 CommandValidator trait,注入 AHIRT 红队
use seccore::SecCoreCommandValidator;

use crate::config::ChimeraConfig;
use crate::output;

/// 执行 audit 子命令 — 真实接入 parliament::ahirt API
///
/// `json` flag(Task 1.7):`true` 时输出 JSON envelope(完整 SecurityReport)。
/// `severity`(Task 1.9.4):可选严重度过滤(如 "high" 仅显示高危及以上)。
pub async fn execute(_config: &ChimeraConfig, json: bool, severity: Option<&str>) -> Result<()> {
    tracing::info!(?severity, "红队安全审计");

    // 1. 构造进程内 ephemeral AhirtRedTeam(默认载荷库 100 个载荷)
    //    ADR-054 决策 3:注入 SecCoreCommandValidator,保持命令类探测能力
    //    (未注入时 AHIRT 命令类探测降级为 skipped,audit 会误报全量漏洞)
    let red_team = AhirtRedTeam::default().with_validator(Arc::new(SecCoreCommandValidator));

    // 2. 执行全量探测(4 类 × 25 载荷 = 100 探测)
    let report = red_team.verify_security();

    // 3. 根据 severity 过滤(当前实现:所有漏洞类型视为 high,过滤仅影响显示)
    let filtered_report = filter_by_severity(report, severity);

    // 4. 输出
    if json {
        // JSON 模式:输出完整 SecurityReport envelope
        output::print_json(&filtered_report)?;
    } else {
        // 人类可读模式:报告标题 + 统计摘要 + 漏洞清单
        print_audit_report_human(&filtered_report, severity);
    }

    Ok(())
}

/// 根据 severity 过滤报告(Task 1.9.4)
///
/// 当前实现:所有 ProbeType 视为 `high` 严重度(AHIRT 探测的是攻击向量,未拦截即为高风险)。
/// `severity=None` 或 `"low"` 以上均显示全部;`severity="critical"` 显示空(无 critical 级)。
fn filter_by_severity(mut report: SecurityReport, severity: Option<&str>) -> SecurityReport {
    if let Some(level) = severity {
        // 简化过滤:high 及以上保留全部,medium 保留全部,critical 清空(无 critical 级漏洞)
        if level.eq_ignore_ascii_case("critical") {
            report.vulnerable_types.clear();
            report.remediation_suggestions.clear();
        }
        // 其余级别(high/medium/low)保留全部,因 AHIRT 漏洞均归为 high
    }
    report
}

/// 人类可读模式输出审计报告(SubTask 1.9.2)
///
/// 格式:
/// ```text
/// === 红队安全审计报告 ===
/// 探测总数: 100 | 通过: 95 | 失败: 5 | 检测率: 95.00%
///
/// 漏洞清单:
///   [HIGH] PromptInjection — 5 个载荷未拦截
///     修复建议: 加强系统提示词约束,启用输入过滤...
/// ```
fn print_audit_report_human(report: &SecurityReport, severity: Option<&str>) {
    output::print_info("=== 红队安全审计报告 ===");
    eprintln!(
        "探测总数: {} | 通过: {} | 失败: {} | 检测率: {:.2}%",
        report.stats.total,
        report.stats.passed,
        report.stats.failed,
        report.stats.detection_rate * 100.0
    );

    if report.vulnerable_types.is_empty() {
        output::print_success("未发现漏洞(所有攻击向量均被正确拦截)");
    } else {
        eprintln!();
        eprintln!("漏洞清单:");
        for (i, probe_type) in report.vulnerable_types.iter().enumerate() {
            let failed_count = report
                .stats
                .by_type
                .get(probe_type)
                .map(|s| s.failed)
                .unwrap_or(0);
            let sev_label = severity.unwrap_or("high");
            eprintln!(
                "  [{}] {:?} — {} 个载荷未拦截",
                sev_label.to_uppercase(),
                probe_type,
                failed_count
            );
            if let Some(suggestion) = report.remediation_suggestions.get(i) {
                eprintln!("    修复建议: {}", suggestion);
            }
        }
    }
}
