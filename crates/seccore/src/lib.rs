//! 零信任执行核心 — SecCore
//!
//! 基于 gVisor + seccomp 的沙箱化命令执行(Linux 生产环境),
//! Windows/macOS 降级为进程隔离 + 白名单模拟层(见 ADR-001)。
//!
//! 对应架构层:L4 Security
//! 对应尸检教训:
//! - Claude CVE-2026-35022:命令注入($(...)、|、;、&&)
//! - 环境变量泄露:SECRET/KEY/TOKEN/PASSWORD 明文传递
//! - 权限提升:sudo/su 未授权提权
//! - 审计篡改:日志可被静默修改
//!
//! 四层防御:
//! 1. 静态分析(`policy::validate_command`):拦截注入/越权/逃逸/泄露/篡改/滥用
//! 2. 环境过滤(`policy::validate_env`):拦截 SECRET/KEY/TOKEN 泄露
//! 3. 沙箱执行(`sandbox::Sandbox`):进程隔离(Windows 降级)/gVisor(Linux)
//! 4. 审计记录(`audit::AuditChain`):SHA-256 Merkle 链,不可篡改
//! 5. ASA 审计(`asa::AsaAuditor`):基于规则的实时评分,干预分级时发布 `AsaIntervention` 事件

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod asa;
pub mod audit;
pub mod error;
pub mod policy;
pub mod sandbox;
pub mod types;

// === 公开 API 导出 ===
pub use asa::{
    AsaAuditor, AsaConfig, AsaSandboxCoordinator, AuditResult, InterventionAction,
    OperationAuditInput,
};
pub use audit::{AuditBlock, AuditChain};
pub use error::SecCoreError;
pub use policy::{validate_command, validate_env, BlockedPattern, CommandPolicy, EnvPolicy};
pub use sandbox::Sandbox;
pub use types::{AttackType, Command, CommandSpec, ExecutionResult, RiskLevel};
