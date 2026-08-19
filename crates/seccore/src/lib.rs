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
//! # 设计文档落层偏差记录(L4 深度优化)
//! 设计文档 §9.2 将两项列为 L4 优化方向,实际按依赖铁律落层如下:
//! - §9.2 问题 1(AutoBuilder 环境构建验证)→ L7 `pvl-layer/src/auto_builder.rs`
//! - §9.2 问题 2(输出校验 D6 契约)→ L0 `nexus-contracts/harness_dimensions.rs`
//!   (D6 OutputProcessingContract)+ 本 crate command_validator.rs(L0 trait 实现)
//!
//! seccore 保持纯安全职责:沙箱/审计/衰减/零孤儿,不承载构建验证与输出契约。
//!
//! # Phase 4 新增模块(v3.4.0 §9)
//! - §9.1 Paddock-Sandbox 解耦(paddock_sandbox.rs,SandboxRuntime trait 抽象,
//!   铁律10: Paddock 不依赖 Sandbox 具体实现)
//! - §9.3 错误签名收集器(error_signature_collector.rs,5 正则模式 + SHA-256 哈希
//!   去重聚类,铁律7;哈希计算与 L3 idx_error_hash 对齐)
//! - §9.3 六类状态反馈集成器(execution_feedback.rs,ExecutionFeedbackIntegrator
//!   纯函数,铁律8 全链路追踪)
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
/// Phase 4 §9.3:错误签名收集器(OpenMLE 结构化收集 + SHA-256 哈希去重聚类,铁律7)
pub mod error_signature_collector;
pub mod escalation;
/// Phase 4 §9.3:六类状态反馈集成器(ExecutionFeedbackIntegrator 纯函数,铁律8)
pub mod execution_feedback;
/// gVisor runsc 运行时检测与子进程启动(ADR-001)
pub mod gvisor;
/// §16.5(Phase 10 Wave 6):沙箱拦截率统计(真实采集,误拦截率 v4.0 预留)
pub mod interception_stats;
/// P4-W15.1.3: Spec Merkle 完整性校验(复用 audit.rs SHA-256 实现)
pub mod merkle;
/// Phase 4 §9.1:Paddock-Sandbox 解耦(Dressage what-to-do/where-it-runs,铁律10)
pub mod paddock_sandbox;
pub mod policy;
pub mod rl_security;
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
// §16.5(Phase 10 Wave 6):沙箱拦截率周期报告器(真实采集,组合根装配)
pub use sandbox::spawn_interception_reporter;
// §16.5(Phase 10 Wave 6):拦截率统计器(原子计数,可独立采样)
pub use interception_stats::InterceptorStats;
// SandboxBackend 始终可用(默认 Process 变体);Wasm 变体仅 wasm-sandbox feature 启用时可用
pub use sandbox_wasm::SandboxBackend;
// WasmSandbox / WasmExecutionResult 仅 wasm-sandbox feature 启用时可用(ADR-035 决策 2)
#[cfg(feature = "wasm-sandbox")]
pub use sandbox_wasm::{WasmExecutionResult, WasmSandbox};
pub use types::{
    AttackType, Command, CommandSpec, EscalationTier, ExecutionResult, GvisorConfig, RiskLevel,
};

// === Phase 4 L4 安全层新增组件导出(v3.4.0 §9) ===
// §9.3 错误签名收集器(OpenMLE 结构化收集 + SHA-256 哈希去重聚类,铁律7)
pub use error_signature_collector::{compute_error_hash, ErrorSignatureCollector};
// §9.3 六类状态反馈集成器(ExecutionFeedbackIntegrator 纯函数,铁律8)
pub use execution_feedback::ExecutionFeedbackIntegrator;
// §9.1 Paddock-Sandbox 解耦(Dressage,铁律10: Paddock 仅依赖 SandboxRuntime trait)
pub use paddock_sandbox::{
    Paddock, ProcessSandboxRuntime, RolloutContext, RolloutOutcome, SandboxExecutionOutput,
    SandboxProvider, SandboxRuntime, SandboxType, StepResult,
};
