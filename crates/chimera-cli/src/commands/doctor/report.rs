//! Doctor 报告模型与人类可读渲染（T2.5 模块拆分）。
//!
//! WHY 独立：报告 schema 是 `--json` 对外契约，探针实现是其生产者；
//! 两者拆开后，修改探针不需要触碰序列化契约，反之亦然。

use serde::Serialize;

use crate::output;

/// 健康检查状态(三态:OK / WARN / FAIL)
///
/// - `Ok`:检查通过,该项健康
/// - `Warn`:检查告警,功能可用但存在潜在问题(如配置文件缺失,使用默认值)
/// - `Fail`:检查失败,该项不可用(如配置文件解析错误)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HealthStatus {
    /// 检查通过
    Ok,
    /// 检查告警(功能可用但存在潜在问题)
    Warn,
    /// 检查失败(该项不可用)
    Fail,
}

/// 单项健康检查结果
#[derive(Debug, Serialize)]
pub struct HealthCheck {
    /// 检查项名称(程序化标识符,如 `config_file` / `cargo_lock`)
    pub name: &'static str,
    /// 检查项人类可读描述(如"配置文件路径与有效性")
    pub description: &'static str,
    /// 检查状态(OK / WARN / FAIL)
    pub status: HealthStatus,
    /// 检查详情(路径 / 计数 / 错误信息等)
    pub message: String,
}

/// 健康检查报告汇总
#[derive(Debug, Serialize)]
pub struct HealthReport {
    /// 11 项检查结果
    pub checks: Vec<HealthCheck>,
    /// 汇总统计
    pub summary: HealthSummary,
}

/// 汇总统计
#[derive(Debug, Serialize)]
pub struct HealthSummary {
    /// OK 项数
    pub ok: usize,
    /// WARN 项数
    pub warn: usize,
    /// FAIL 项数
    pub fail: usize,
    /// 总检查项数
    pub total: usize,
}

/// 人类可读模式渲染报告（状态与详情走 stderr，避免污染 stdout 数据流）。
///
/// # 示例
///
/// ```text
/// === Chimera CLI 系统健康检查 ===
///
/// [OK] 配置文件路径与有效性
///         config: /home/user/.chimera/omega.yaml
///
/// === 汇总: 11 OK / 0 WARN / 0 FAIL (共 11 项) ===
/// ```
pub fn print_report_human(report: &HealthReport) {
    output::print_info("=== Chimera CLI 系统健康检查 ===");
    eprintln!();

    for check in &report.checks {
        let label = match check.status {
            HealthStatus::Ok => "OK",
            HealthStatus::Warn => "WARN",
            HealthStatus::Fail => "FAIL",
        };

        // 状态标签带颜色输出到 stderr(诊断信息,不污染 stdout 数据流)
        match check.status {
            HealthStatus::Ok => output::print_success(&format!("[{label}] {}", check.description)),
            HealthStatus::Warn => {
                output::print_warning(&format!("[{label}] {}", check.description))
            }
            HealthStatus::Fail => output::print_error(&format!("[{label}] {}", check.description)),
        }
        // 详情输出到 stderr(缩进对齐)
        eprintln!("        {}", check.message);
        eprintln!();
    }

    eprintln!(
        "=== 汇总: {} OK / {} WARN / {} FAIL (共 {} 项) ===",
        report.summary.ok, report.summary.warn, report.summary.fail, report.summary.total
    );
}
