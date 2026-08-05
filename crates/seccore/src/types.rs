//! 核心领域类型 — SecCore 零信任沙箱的数据契约
//!
//! 对应架构层:L4 Security
//! 对应尸检教训:Claude CVE-2026-35022 命令注入、环境变量泄露、权限提升

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// 风险等级 — 用于命令分类与审计追溯。
///
/// 零信任模型下,所有命令默认按风险分级处理:
/// - `Low`:只读、无副作用(echo / pwd / whoami)
/// - `Medium`:有输出重定向或通配符
/// - `High`:破坏性命令(rm / dd / mkfs)
/// - `Critical`:理论上不应到达执行层(应被策略拦截)
/// - `Unknown`:风险无法评估(如 ASA 审计时调用者未提供 risk_keywords 列表)
///
/// WHY `Unknown` 变体(N4 安全修复):旧实现中 `AsaAuditor::audit()` 在 risk_keywords
/// 为空时将风险等同于 Low,调用者可通过省略关键字列表绕过检测。修复后空关键字 →
/// `Unknown`,作为信号触发 Parliament/下游消费者的额外审计检查。`assess_risk()`
/// (命令静态风险评估)不会产生此变体,仅 ASA 动态审计路径使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RiskLevel {
    /// 低风险:只读命令
    Low,
    /// 中风险:有副作用但可控
    Medium,
    /// 高风险:破坏性命令
    High,
    /// 临界:应被策略拦截,不应执行
    Critical,
    /// 未知风险:风险无法评估(如 ASA 审计未提供 risk_keywords)。
    /// 触发下游额外审计检查,防止调用者通过省略关键字列表绕过检测。
    Unknown,
}

impl RiskLevel {
    /// 从 0-100 风险评分派生 `RiskLevel` — 保持向后兼容的派生映射。
    ///
    /// WHY: D6 修复引入 `risk_score: u8` 作为升级主信号,但旧 API 仍依赖
    /// `RiskLevel` 枚举。此方法提供从数值评分到枚举的映射,避免破坏既有消费者
    /// (审计、限流、ASA 等)。映射规则与 `EscalationTier::from_score` 对齐:
    /// - ReadOnly (0-30)   → Low
    /// - Normal (31-70)    → Medium
    /// - Parliament (71-90) → High
    /// - EscalateToHuman (91-100) → Critical
    pub fn from_score(score: u8) -> Self {
        match EscalationTier::from_score(score) {
            EscalationTier::ReadOnly => Self::Low,
            EscalationTier::Normal => Self::Medium,
            EscalationTier::Parliament => Self::High,
            EscalationTier::EscalateToHuman => Self::Critical,
        }
    }
}

/// 升级档位 — 基于 `risk_score` (0-100) 的 4 级分类,决定操作的执行路径。
///
/// WHY: spec.md D6 修复要求按 risk_score 分级处理高危操作:
/// - `ReadOnly` (0-30): 只读操作,直接执行
/// - `Normal` (31-70): 常规操作,直接执行
/// - `Parliament` (71-90): 高危操作,强制 Parliament 辩论 + 自白通道复核
/// - `EscalateToHuman` (91-100): 极高危操作,拒绝执行并升级人工处理
///
/// 与 `RiskLevel` 的区别:`EscalationTier` 是**执行路径决策信号**(决定走哪条通道),
/// `RiskLevel` 是**审计/限流分类标签**(向后兼容旧消费者)。两者通过
/// `RiskLevel::from_score` 保持映射一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EscalationTier {
    /// 只读档 (risk_score 0-30):无副作用,直接执行
    ReadOnly,
    /// 常规档 (risk_score 31-70):有副作用但可控,直接执行
    Normal,
    /// 议会档 (risk_score 71-90):高危,强制 Parliament 辩论 + 自白通道复核
    Parliament,
    /// 人工升级档 (risk_score 91-100):极高危,拒绝执行并升级人工决策
    EscalateToHuman,
}

impl EscalationTier {
    /// 从 0-100 风险评分推导升级档位。
    ///
    /// 防御性:超过 100 的评分(理论不应出现)归入 `EscalateToHuman`,
    /// 避免异常评分漏过人工升级通道。
    pub fn from_score(score: u8) -> Self {
        match score {
            0..=30 => Self::ReadOnly,
            31..=70 => Self::Normal,
            71..=90 => Self::Parliament,
            91..=100 => Self::EscalateToHuman,
            // WHY: 超 100 的评分视为极高危,确保异常输入不漏过人工升级
            _ => Self::EscalateToHuman,
        }
    }
}

/// 攻击类型 — 对应 6 种需拦截的攻击向量(对齐验收标准)。
///
/// ⚠️ ADR-054 决策 3(P9-T4):权威定义已上提至 L0
/// `nexus_contracts::command_validation`,此处 re-export 保兼容
/// (对外 API 零破坏,`seccore::types::AttackType` 路径不变)。
pub use nexus_contracts::command_validation::AttackType;

/// 原始命令 — 用户或上层提交的待执行命令。
///
/// 零信任模型下,此结构的内容**不可信**,必须经 `policy::validate_command`
/// 与 `policy::validate_env` 校验后才能进入沙箱执行层。
///
/// ⚠️ ADR-054 决策 3(P9-T4):权威定义已上提至 L0
/// `nexus_contracts::command_validation`,此处 re-export 保兼容
/// (对外 API 零破坏,`seccore::types::Command` 路径不变)。
pub use nexus_contracts::command_validation::Command;

/// 命令规格 — 经策略校验后的安全命令表示。
///
/// 这是 `Command` 通过策略校验后的产物,携带校验时确定的:
/// - `allowed_args`:已确认安全的参数列表
/// - `env_whitelist`:已通过环境变量白名单过滤的映射
/// - `risk_level`:基于程序名与参数评估的风险等级(向后兼容,由 `risk_score` 派生)
/// - `risk_score`:0-100 数值风险评分(D6 修复:升级通道的主信号)
///
/// 沙箱执行层只接受 `CommandSpec`,不接受原始 `Command`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    /// 校验通过的程序名
    pub program: String,
    /// 校验通过的参数列表
    pub allowed_args: Vec<String>,
    /// 白名单过滤后的环境变量
    pub env_whitelist: HashMap<String, String>,
    /// 风险等级(用于审计与限流,由 `risk_score` 经 `RiskLevel::from_score` 派生)
    pub risk_level: RiskLevel,
    /// 风险评分 (0-100) — D6 修复:高危操作强制升级通道的主信号。
    ///
    /// WHY: `risk_level` 枚举只有 4 档粗粒度,无法区分"破坏性但可恢复"(rm,71-90)
    /// 与"不可恢复的灾难性操作"(dd/mkfs,91-100)。引入数值评分作为升级决策主信号,
    /// `risk_level` 保留用于向后兼容旧消费者(审计/限流)。
    pub risk_score: u8,
}

/// 执行结果 — 沙箱执行后的结构化输出。
///
/// `audit_hash` 是执行结果的 SHA-256 摘要,用于审计链链接。
/// 审计链验证时会重新计算此哈希,防止字段被篡改。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// 进程退出码(信号终止时为 -1)
    pub exit_code: i32,
    /// 标准输出(UTF-8 解码,失败时用替换字符)
    pub stdout: String,
    /// 标准错误(UTF-8 解码,失败时用替换字符)
    pub stderr: String,
    /// 执行耗时
    pub duration: Duration,
    /// 执行结果摘要(SHA-256 十六进制)
    pub audit_hash: String,
}

/// gVisor 运行时配置 — 为 gVisor 集成做准备(Task 10)。
///
/// 定义 gVisor(runsc) 运行时的路径、沙箱命名、网络策略和平台限制。
/// Linux 上通过 `Sandbox::use_gvisor` 控制是否启用 gVisor 隔离,
/// 非 Linux 平台自动降级为进程隔离(参考 `Sandbox` 文档注释)。
///
/// 对应 ADR-001:沙箱运行时选择 gVisor,Linux 优先。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GvisorConfig {
    /// runsc 二进制路径(默认 "/usr/local/bin/runsc")
    pub runsc_path: String,
    /// 沙箱名称前缀(默认 "chimera"),实际沙箱名称为 `{prefix}-{uuid}`
    pub sandbox_name_prefix: String,
    /// 是否禁用网络(默认 true)。禁用网络可防止沙箱内进程发起外连,
    /// 降低数据泄露风险,适用于大多数命令执行场景。
    pub network_disabled: bool,
    /// 平台限制:仅 Linux 可用(预留字段,用于运行时平台检测)
    pub platform: String,
    /// OCI bundle 根目录路径（默认: None，使用临时目录）
    ///
    /// 设置此字段后，OCI bundle 将在指定目录下创建而非临时目录。
    /// 主要用于测试和调试场景。
    pub bundle_dir: Option<String>,
    /// rootfs 路径（默认: None，使用 "/tmp/chimera-rootfs" 作为最小 rootfs）
    ///
    /// 指向包含 Linux 最小文件系统的目录（如 busybox 根文件系统）。
    /// 当为 None 时，使用预设的最小 rootfs 路径。
    pub rootfs_path: Option<String>,
}

impl Default for GvisorConfig {
    fn default() -> Self {
        Self {
            runsc_path: "/usr/local/bin/runsc".into(),
            sandbox_name_prefix: "chimera".into(),
            network_disabled: true,
            platform: "linux".into(),
            bundle_dir: None,
            rootfs_path: None,
        }
    }
}

impl GvisorConfig {
    /// 获取生效的 bundle 目录路径
    ///
    /// 如果 `bundle_dir` 为 None，返回临时目录路径（`std::env::temp_dir()` + "chimera-bundles"）。
    pub fn effective_bundle_dir(&self) -> std::path::PathBuf {
        self.bundle_dir
            .as_ref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("chimera-bundles"))
    }

    /// 获取生效的 rootfs 路径
    ///
    /// 如果 `rootfs_path` 为 None，返回 "/tmp/chimera-rootfs"。
    /// 注意：此路径仅用于 runsc 的 OCI bundle 配置，不保证文件系统存在。
    pub fn effective_rootfs(&self) -> std::path::PathBuf {
        self.rootfs_path
            .as_ref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp/chimera-rootfs"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gvisor_config_default() {
        let config = GvisorConfig::default();
        assert_eq!(config.runsc_path, "/usr/local/bin/runsc");
        assert_eq!(config.sandbox_name_prefix, "chimera");
        assert!(config.network_disabled);
        assert_eq!(config.platform, "linux");
        assert!(config.bundle_dir.is_none(), "bundle_dir 默认为 None");
        assert!(config.rootfs_path.is_none(), "rootfs_path 默认为 None");
    }
}
