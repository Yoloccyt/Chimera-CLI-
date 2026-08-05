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
//! 6. 高危操作强制升级通道(`escalation::EscalationHandler`,D6 修复):
//!    `risk_score ∈ [71,90]` 强制 Parliament 辩论;`risk_score ∈ [91,100]` 拒绝执行并升级人工
//!
//! # 快速示例
//! ```no_run
//! use seccore::{Command, Sandbox};
//!
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let mut sandbox = Sandbox::with_default_policy();
//! let cmd = Command::new("ls").arg("-la");
//! let result = sandbox.audit_and_execute(cmd).await?;
//! println!("exit_code={}, audit_hash={}", result.exit_code, result.audit_hash);
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod asa;
pub mod asa_ppo;
pub mod asa_score_fusion;
pub mod audit;
/// ADR-054 决策 3(P9-T4):L0 CommandValidator trait 的 seccore 实现(parliament 注入载体)
pub mod command_validator;
pub mod error;
pub mod escalation;
/// gVisor runsc 运行时检测与子进程启动(ADR-001)
pub mod gvisor;
/// P4-W15.1.3: Spec Merkle 完整性校验(复用 audit.rs SHA-256 实现)
pub mod merkle;
pub mod policy;
pub mod sandbox;
pub mod sandbox_wasm;
pub mod types;

// === 公开 API 导出 ===
pub use asa::{
    AsaAuditor, AsaConfig, AsaSandboxCoordinator, AuditResult, InterventionAction,
    OperationAuditInput,
};
pub use asa_ppo::PpoCritic;
pub use asa_score_fusion::ScoreFusion;
pub use audit::{
    AuditBlock, AuditChain, AuditRecordStatus, DecisionChainBuilder, DecisionStep,
    DecisionStepType, RecordId,
};
// ADR-054 决策 3(P9-T4):L0 CommandValidator trait 实现(供 L8 parliament 注入)
pub use command_validator::SecCoreCommandValidator;
pub use error::SecCoreError;
pub use escalation::{DefaultEscalationHandler, EscalationHandler};
pub use gvisor::GvisorRuntime;
// P4-W15.1.3: Merkle 完整性校验公共 API
pub use merkle::{
    compute_merkle_root, hash_spec_canonical_input, verify_merkle_root, verify_spec_integrity,
};
pub use policy::{validate_command, validate_env, BlockedPattern, CommandPolicy, EnvPolicy};
// polish-v2.7 P1-4: 不可学习安全红线常量表(ADR-049 决策 3,AEGIS/Variant 审议否决依据)
pub use policy::UNLEARNABLE_SECURITY_RULES;
pub use sandbox::Sandbox;
// SandboxBackend 始终可用(默认 Process 变体);Wasm 变体仅 wasm-sandbox feature 启用时可用
pub use sandbox_wasm::SandboxBackend;
// WasmSandbox / WasmExecutionResult 仅 wasm-sandbox feature 启用时可用(ADR-035 决策 2)
#[cfg(feature = "wasm-sandbox")]
pub use sandbox_wasm::{WasmExecutionResult, WasmSandbox};
pub use types::{
    AttackType, Command, CommandSpec, EscalationTier, ExecutionResult, GvisorConfig, RiskLevel,
};
