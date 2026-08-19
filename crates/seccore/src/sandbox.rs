//! 沙箱执行 — 零信任沙箱的核心执行层
//!
//! 对应尸检教训:
//! - Claude Code 命令直接在用户 shell 执行,无隔离
//! - 审计日志在执行后才记录,可被绕过
//!
//! 跨平台策略(WHY):
//! - **Linux 生产环境**:应通过 gVisor(runsc)运行时 + seccomp 过滤器启动子进程,
//!   实现内核级隔离与系统调用过滤。gVisor 拦截系统调用,seccomp 限制调用集合。
//! - **Windows/macOS 降级**:无 gVisor/seccomp 等效物,降级为"进程隔离 + 白名单"
//!   模拟层。用 `tokio::process::Command` 直接执行,依赖策略层的静态分析拦截
//!   危险命令。这是**降级方案**,安全性弱于 Linux 生产环境。
//! - **ADR-001**:沙箱运行时选择 gVisor,Linux 优先。
//!
//! 四层防御(对应架构红线):
//! 1. 静态分析(validate_command):拦截注入/越权/逃逸/泄露/篡改
//! 2. 环境过滤(validate_env):拦截 SECRET/KEY/TOKEN 泄露
//! 3. 沙箱执行(execute_in_sandbox):进程隔离(Windows 降级)/gVisor(Linux)
//! 4. 审计记录(audit_chain.append):SHA-256 Merkle 链,不可篡改

use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use event_bus::{EventBus, EventMetadata, NexusEvent};
use sha2::{Digest, Sha256};
use tokio::process::Command as TokioCommand;
use tracing::{info, warn};

use crate::asa::{AsaAuditor, OperationAuditInput};
use crate::audit::{AuditChain, AuditRecordStatus, DecisionChainBuilder, RecordId};
use crate::error::SecCoreError;
use crate::escalation::{DefaultEscalationHandler, EscalationHandler};
use crate::gvisor::GvisorRuntime;
// §16.5(Phase 10 Wave 6):沙箱拦截率统计(真实采集)
use crate::interception_stats::InterceptorStats;
use crate::policy::{validate_command, validate_env, CommandPolicy, EnvPolicy};
use crate::types::{Command, CommandSpec, EscalationTier, ExecutionResult};

/// 零信任沙箱 — 封装策略、环境策略与审计链,提供统一的执行入口。
///
/// 所有外部命令必须经 `Sandbox::audit_and_execute` 执行,
/// 确保经过四层防御:静态分析 → 环境过滤 → 沙箱执行 → 审计记录。
pub struct Sandbox {
    /// 命令策略(白名单 + 拦截模式)
    pub policy: CommandPolicy,
    /// 环境变量策略(白名单 + 敏感模式)
    pub env_policy: EnvPolicy,
    /// 审计链(SHA-256 Merkle 链)
    pub audit_chain: AuditChain,
    /// 沙箱执行超时 — 防止恶意命令(如 `sleep infinity`)永久阻塞,导致 DoS (F-002)。
    ///
    /// WHY: 无超时限制时,恶意命令可永久阻塞子进程,耗尽调度资源造成 DoS。
    /// 默认 30 秒,可通过 `with_timeout` 按场景调整(如长命令设为 5 分钟)。
    pub timeout: Duration,
    /// 高危操作升级处理器(`risk_score ∈ [71,90]` 时调用)。
    ///
    /// WHY D6 修复: seccore 位于 L4,parliament 位于 L8,依赖铁律禁止 L4 → L8。
    /// 通过 trait 注入,上层(chimera-cli / quest-engine)注入实际 Parliament 实现,
    /// seccore 仅定义契约。默认为 `DefaultEscalationHandler`(拒绝所有 Parliament 档操作,
    /// 强制调用方显式配置真实 handler,避免"忘记配置"导致高危操作静默执行)。
    escalation_handler: Box<dyn EscalationHandler>,
    /// ASA 审计器 — 可选,配置后对 Parliament 档操作做前置实时审计(P1-W3.2 / D6 修复)。
    ///
    /// WHY: spec.md D6 修复要求高危操作(risk_score ∈ [71,90])前置实时审计。
    /// ASA 审计在 `escalation_handler.parliament_debate()` **之前**执行:
    /// - ASA Block → 返回 `AsaBlocked` 错误,Parliament 辩论被跳过(事中拦截优先)
    /// - ASA Allow/Warn → 继续进入 Parliament 辩论(handler 决定是否批准)
    /// - 未配置(None)→ 直接进入 Parliament 辩论(回退到 P1-W3.1 既有行为)
    ///
    /// ReadOnly/Normal/EscalateToHuman 档不触发 ASA(快速路径,零开销)。
    asa_auditor: Option<AsaAuditor>,
    /// 是否启用 gVisor 内核级隔离 — Linux 上默认启用,非 Linux 平台自动降级为进程隔离。
    ///
    /// WHY: gVisor(runsc) 提供内核级系统调用拦截与 seccomp 过滤,是 Linux 生产环境
    /// 的推荐沙箱运行时(ADR-001)。非 Linux 平台(Windows/macOS)无等效物,需降级为
    /// `tokio::process::Command` 进程隔离。调用方可通过 `with_gvisor(false)` 显式禁用
    /// (如测试环境或无需内核隔离的受控场景)。
    ///
    /// 默认 `true` — Linux 上启用 gVisor 隔离,非 Linux 平台 `execute_in_sandbox()`
    /// 内部自动降级(不需调用方感知)。
    pub use_gvisor: bool,
    /// gVisor 运行时实例 — 封装 runsc 路径检测与子进程启动(Task 12)。
    ///
    /// WHY: `use_gvisor=true` 仅表示"意图启用",实际是否可用取决于:
    /// 1. 当前平台是否为 Linux
    /// 2. runsc 二进制是否存在且可执行
    ///
    /// 运行时实例在 `execute_in_sandbox()` 中被检测,不可用时自动降级。
    ///
    /// 默认 `None` — 调用方通过 `with_gvisor_runtime()` 或 `with_gvisor_config()`
    /// 注入运行时实例。未注入时即使 `use_gvisor=true` 也会降级为进程隔离。
    gvisor_runtime: Option<GvisorRuntime>,
    /// 事件总线(可选)— 注入后沙箱违规拦截路径发布 SandboxViolation 事件(P2-4)
    ///
    /// WHY Option 而非必填:与 `AsaAuditor::new()` 保留私有总线不同,Sandbox 有
    /// 63+ 处测试调用点(`with_default_policy()`),默认 None 时发布静默跳过,
    /// 既有测试零改动。生产代码通过 `with_event_bus` 注入共享总线,
    /// 复用 asa.rs 的注入模式(L4 → L1 依赖合规)。
    event_bus: Option<EventBus>,
    /// §16.5(Phase 10 Wave 6):拦截率统计(请求/拦截原子计数,周期报告)
    stats: Arc<InterceptorStats>,
}

// ============================================================
// 升级通道处理结果枚举
// ============================================================

/// 升级通道处理结果 — `handle_escalation` 的返回值(P2-1 提取)。
///
/// WHY: 封装 `audit_and_execute` 步骤3中升级通道的三种分档决策结果,
/// 使调用方能通过 match 清晰处理"继续执行"与"拒绝返回"两种路径。
enum EscalationOutcome {
    /// 升级通过，继续执行流程（ReadOnly/Normal 直接执行，或 Parliament 辩论通过）
    Proceed,
    /// 操作被拒绝，含错误原因（EscalateToHuman / ASA Block / Parliament 否决）
    Rejected(SecCoreError),
}

impl Sandbox {
    /// 创建沙箱,携带指定的命令策略与环境变量策略。
    ///
    /// 默认超时 30 秒(防止恶意命令永久阻塞),可用 `with_timeout` 调整。
    /// 默认升级处理器为 `DefaultEscalationHandler`(拒绝 Parliament 档操作),
    /// 可用 `with_escalation_handler` 注入实际 Parliament 实现。
    pub fn new(policy: CommandPolicy, env_policy: EnvPolicy) -> Self {
        Self {
            policy,
            env_policy,
            audit_chain: AuditChain::new(),
            timeout: Duration::from_secs(30),
            escalation_handler: Box::new(DefaultEscalationHandler),
            asa_auditor: None,
            use_gvisor: true,
            gvisor_runtime: None,
            event_bus: None,
            stats: Arc::new(InterceptorStats::new()),
        }
    }

    /// 创建使用默认安全策略的沙箱。
    pub fn with_default_policy() -> Self {
        Self::new(CommandPolicy::default_secure(), EnvPolicy::default_secure())
    }

    /// 链式设置沙箱执行超时(F-002)。
    ///
    /// WHY: 不同场景命令耗时差异大,需可配置超时。短命令设小超时快速失败,
    /// 长命令(如构建)设大超时避免误杀。默认 30 秒。
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// 链式设置升级处理器 — 注入 Parliament 辩论实现(D6 修复)。
    ///
    /// WHY: 默认 `DefaultEscalationHandler` 会拒绝所有 Parliament 档操作
    /// (`risk_score ∈ [71,90]`),强制调用方显式注入实际 handler。
    /// 上层(chimera-cli / quest-engine)负责构造 Parliament 实现并注入,
    /// 避免 seccore 直接依赖 L8 的 parliament crate(违反依赖铁律)。
    pub fn with_escalation_handler(mut self, handler: Box<dyn EscalationHandler>) -> Self {
        self.escalation_handler = handler;
        self
    }

    /// 链式设置 ASA 审计器 — 配置后对 Parliament 档操作做前置实时审计(P1-W3.2 / D6 修复)。
    ///
    /// WHY: spec.md D6 修复要求高危操作(risk_score ∈ [71,90])在 Parliament 辩论前
    /// 先经 ASA 实时审计。ASA Block 时操作被拦截不进入辩论;Allow/Warn 时继续辩论。
    /// 未调用此方法时 `asa_auditor` 为 `None`,回退到 P1-W3.1 既有行为(直接辩论)。
    ///
    /// 注意:`AsaAuditor` 由 Sandbox 拥有(非 Arc 共享)。如需执行后访问审计器做
    /// 反馈闭环(record_success/record_failure),请通过 `asa_auditor()` getter 获取引用。
    pub fn with_asa_auditor(mut self, auditor: AsaAuditor) -> Self {
        self.asa_auditor = Some(auditor);
        self
    }

    /// 链式注入事件总线 — 沙箱违规时发布 SandboxViolation 事件(P2-4)
    ///
    /// WHY:复用 asa.rs 的 EventBus 注入 + publish_blocking 模式。
    /// EventBus 内部为 Arc,Clone 廉价;生产代码注入共享总线使违规可被
    /// quest-engine/L9 订阅者感知并中止/告警 Quest(§6.2 安全事件观测)。
    pub fn with_event_bus(mut self, bus: EventBus) -> Self {
        self.event_bus = Some(bus);
        self
    }

    /// 发布 SandboxViolation 事件(P2-4)— 违规路径统一出口
    ///
    /// WHY publish_blocking:`audit_and_execute` 错误路径在返回前同步调用,
    /// 用 event-bus 官方同步 API(与 asa.rs 一致);未注入 EventBus 时
    /// 静默跳过(向后兼容,既有测试零改动)。发布失败仅 warn 不上抛——
    /// 违规拦截的主语义是"拒绝执行",事件发布是观测增强。
    fn publish_violation(&self, violation_type: &str, detail: String) {
        if let Some(bus) = &self.event_bus {
            let event = NexusEvent::SandboxViolation {
                metadata: EventMetadata::new("seccore"),
                violation_type: violation_type.to_string(),
                detail,
            };
            if let Err(e) = bus.publish_blocking(event) {
                tracing::warn!(error = %e, "发布 SandboxViolation 事件失败");
            }
        }
    }

    /// 链式设置是否启用 gVisor 内核级隔离 — 为 gVisor 集成做准备(Task 10)。
    ///
    /// WHY: Linux 生产环境推荐启用 gVisor(runsc) 实现内核级系统调用拦截与 seccomp
    /// 过滤(ADR-001)。调用方可通过 `with_gvisor(false)` 显式禁用,适用场景:
    /// - 测试环境(无需真实内核隔离,快速验证)
    /// - 非 Linux 平台(无 gVisor 等效物,自动降级为进程隔离)
    /// - 受控内网环境(已通过其他手段保证安全)
    ///
    /// 默认 `true`(Linux 上启用 gVisor 隔离,非 Linux 平台自动降级)。
    pub fn with_gvisor(mut self, enabled: bool) -> Self {
        self.use_gvisor = enabled;
        self
    }

    /// 链式注入 gVisor 运行时实例 — 为 gVisor 集成做准备(Task 12)。
    ///
    /// WHY: `GvisorRuntime` 封装 runsc 路径检测与子进程启动逻辑,
    /// 注入后 `execute_in_sandbox()` 在 Linux 平台且 runsc 可用时
    /// 通过 gVisor 执行命令,否则降级为进程隔离。
    ///
    /// 调用方通常使用 `with_gvisor_config()` 便捷方法,由 `GvisorRuntime::detect()`
    /// 自动检测 runsc 路径。此方法适用于需要自定义 `GvisorRuntime` 的场景(如测试 mock)。
    pub fn with_gvisor_runtime(mut self, runtime: GvisorRuntime) -> Self {
        self.gvisor_runtime = Some(runtime);
        self
    }

    /// 链式配置 gVisor — 从 `GvisorConfig` 自动检测 runsc 并注入运行时(Task 12)。
    ///
    /// WHY: 便捷方法,封装 `GvisorRuntime::detect()` 调用。
    /// 调用方只需提供 `GvisorConfig`,无需手动构造 `GvisorRuntime`。
    /// runsc 不可用时 `gvisor_runtime` 保持 `None`,`execute_in_sandbox()`
    /// 自动降级为进程隔离。
    ///
    /// # 参数
    /// - `config`: gVisor 配置(含 runsc_path 等)
    pub fn with_gvisor_config(mut self, config: &crate::types::GvisorConfig) -> Self {
        if let Some(runtime) = GvisorRuntime::detect(&config.runsc_path) {
            self.gvisor_runtime = Some(runtime);
        }
        self
    }

    /// 获取 ASA 审计器引用 — 用于执行后反馈闭环(record_success/record_failure)。
    ///
    /// WHY: `audit_and_execute()` 内部对 Parliament 档调用 ASA 审计,但不会自动
    /// 调用 record_success/record_failure(因为成功/失败需调用方根据业务语义判断)。
    /// 上层调用方执行后可通过此 getter 获取审计器,按结果更新历史失败率。
    pub fn asa_auditor(&self) -> Option<&AsaAuditor> {
        self.asa_auditor.as_ref()
    }

    /// §16.5(Phase 10 Wave 6):拦截率统计只读快照 `(total, blocked, rate)`
    ///
    /// 真实采集:总请求数在 `audit_and_execute` 入口递增,拦截数在任一
    /// 防御层拒绝时递增。误拦截率需人工真值标注,标注 v4.0 预留(不假采集)。
    pub fn interception_stats(&self) -> (u64, u64, f64) {
        let (total, blocked) = self.stats.snapshot();
        (total, blocked, self.stats.interception_rate())
    }

    /// §16.5(Phase 10 Wave 6):拦截统计器 Arc 引用(供周期报告器共享采样)
    pub fn interception_stats_handle(&self) -> Arc<InterceptorStats> {
        Arc::clone(&self.stats)
    }

    /// 审计并执行命令 — 零信任四层防御的统一入口。
    ///
    /// 执行流程(N5 修复 + D6 修复 + P1-W3.2 ASA 前置审计 + P1-W3.3 决策链上链):
    /// 1. `validate_command`:静态分析,拦截注入/越权/逃逸/泄露/篡改/滥用
    /// 2. `validate_env`:环境变量过滤,拦截 SECRET/KEY/TOKEN 泄露
    /// 3. **escalation check (D6 修复 + P1-W3.2 + P1-W3.3)**:按 `risk_score` 分档处理
    ///    - `EscalateToHuman` (91-100):拒绝执行,决策链 [Proposal, Result(rejected)] 上链
    ///    - `Parliament` (71-90):
    ///      - a. 若配置了 `asa_auditor`,先做 ASA 前置实时审计(P1-W3.2)
    ///        - ASA Block → 返回 `AsaBlocked`,决策链 [Proposal, Result(rejected)] 上链
    ///        - ASA Allow/Warn → AsaAudit 步骤加入决策链,继续
    ///      - b. 调用 `escalation_handler.parliament_debate()`
    ///        - 通过 → Debate(approved) + Confession 步骤加入决策链,继续
    ///        - 否决 → 决策链 [Proposal, (AsaAudit), Debate(rejected), Result(rejected)] 上链
    ///    - `ReadOnly`/`Normal`:直接执行(决策链为空,向后兼容)
    /// 4. `audit_chain.append_intent_with_chain`(高危)/`append_intent`(低危):
    ///    **执行前**记录 Intent 审计块(关闭 N5 漏洞,P1-W3.3 携带 pre-execution 决策链)
    /// 5. `execute_in_sandbox`:进程隔离执行(Windows 降级 / Linux gVisor)
    ///    - P1-W3.3 子步骤:执行后 `extend_decision_chain` 补充 Execution + Result 步骤(仅高危)
    /// 6. `audit_chain.update_status`:执行后更新为 Executed/Failed
    ///
    /// WHY(N5 修复): 原实现步骤4在步骤5之后(后置 append),若执行成功但 append
    /// 失败则无审计痕迹。改为 pre-execution 模式:执行前先写 Intent,即使后续
    /// 崩溃也有意图痕迹;执行失败也更新为 Failed,保持审计链完整。
    ///
    /// WHY(D6 修复): escalation check 位于 `validate_env` 之后、`append_intent` 之前:
    /// - handler 调用前 spec 已通过完整校验(注入/越权/逃逸/泄露/滥用/环境变量),
    ///   Parliament 辩论基于已验证的安全命令规格,不暴露未校验的攻击向量。
    ///
    /// WHY(P1-W3.2 ASA 前置审计): spec.md D6 修复要求高危操作(risk_score ∈ [71,90])
    /// 在 Parliament 辩论前先经 ASA 实时审计。ASA Block 时操作被拦截不进入辩论(事中
    /// 拦截优先,避免危险操作触发 Parliament 资源浪费)。ReadOnly/Normal 档不触发 ASA
    /// (零开销快速路径)。
    ///
    /// WHY(P1-W3.3 决策链上链): spec.md:206 要求高危操作的完整决策链
    /// (提案→辩论→自白→执行→结果)全量上 Merkle 审计链,支持事后完整重放。
    /// 决策链分两阶段记录:N5 pre-execution audit 模式要求执行前先写 Intent,
    /// 故 pre-execution 步骤(Proposal/AsaAudit/Debate/Confession)在步骤4写入,
    /// post-execution 步骤(Execution/Result)在步骤5a 通过 `extend_decision_chain` 补充。
    /// 拒绝路径(EscalateToHuman/ASA Block/Parliament 否决)也上链,消除审计盲区。
    /// `decision_chain` 纳入 `merkle_root` 计算,篡改任意步骤被 `verify()` 检测。
    ///
    /// # 参数
    /// - `command`:原始命令(不可信,需经策略校验)
    ///
    /// # 返回
    /// - `Ok(ExecutionResult)`:执行成功,携带退出码、输出、审计哈希
    /// - `Err(SecCoreError::EscalateToHuman)`:`risk_score ≥ 91`,操作被拒绝升级人工
    /// - `Err(SecCoreError)`:任一防御层拦截、Parliament 否决或执行失败
    ///
    /// # P1-W4.1 tracing 贯穿观测
    /// 顶层 span 携带 `program` / `risk_score` / `tier` 三个核心字段,
    /// 供 efficiency-monitor 与下游订阅者关联同一操作的完整决策链。
    /// `risk_score` 与 `tier` 在函数内部由 `spec` 计算后通过
    /// `Span::current().record()` 填充(instrument fields 不能引用局部变量)。
    #[tracing::instrument(
        skip(self, command),
        fields(
            program = %command.program,
            risk_score,
            tier,
            decision_chain_id = tracing::field::Empty
        )
    )]
    pub async fn audit_and_execute(
        &mut self,
        command: Command,
    ) -> Result<ExecutionResult, SecCoreError> {
        // §16.5(Phase 10 Wave 6):入口记录总请求数(真实采集拦截率分子)
        self.stats.record_request();

        // 步骤1:静态分析 — 拦截注入/越权/逃逸/泄露/篡改/滥用
        // P2-4:错误路径注入 SandboxViolation 发布(违规不再只写 Merkle 审计链)
        let mut spec = match validate_command(&command, &self.policy) {
            Ok(spec) => spec,
            Err(SecCoreError::CommandBlocked {
                attack_type,
                detail,
            }) => {
                self.publish_violation(&format!("{attack_type:?}"), detail.clone());
                self.stats.record_blocked();
                return Err(SecCoreError::CommandBlocked {
                    attack_type,
                    detail,
                });
            }
            Err(e) => return Err(e),
        };

        // 步骤2:环境变量过滤 — 拦截 SECRET/KEY/TOKEN 泄露
        let filtered_env = match validate_env(&command.env, &self.env_policy) {
            Ok(filtered) => filtered,
            Err(SecCoreError::EnvVarBlocked { name, pattern }) => {
                self.publish_violation(
                    "env_blocked",
                    format!("env var '{name}' matched pattern '{pattern}'"),
                );
                self.stats.record_blocked();
                return Err(SecCoreError::EnvVarBlocked { name, pattern });
            }
            Err(e) => return Err(e),
        };
        spec.env_whitelist = filtered_env;

        // 步骤3(D6 修复 + P1-W3.3):高危操作强制升级通道 — 按 risk_score 分档处理
        // WHY: 位于 validate_env 之后(已通过完整校验)、append_intent 之前(拒绝意图不污染审计链)
        //      P1-W3.3:高危操作(Parliament/EscalateToHuman)的完整决策链全量上 Merkle 审计链
        let tier = EscalationTier::from_score(spec.risk_score);
        // P1-W4.1: 填充 instrument span 的延迟字段(risk_score / tier 在 spec 校验后才确定)
        // WHY Span::current().record:instrument fields 表达式只能引用函数参数,不能引用
        // 局部变量 `spec.risk_score` / `tier`。声明空占位字段后用 record 填充是 tracing
        // 处理"运行时才能确定的 span 字段"的惯用法,确保 span 元信息完整可用。
        // WHY tracing::field::debug:&tier 是自定义 enum,未实现 tracing::Value,
        // 用 debug() 包装为 Debug Value(?value 是宏语法,record 方法不接受)
        tracing::Span::current()
            .record("risk_score", spec.risk_score)
            .record("tier", tracing::field::debug(&tier));
        let is_high_risk = matches!(
            tier,
            EscalationTier::Parliament | EscalationTier::EscalateToHuman
        );

        // P1-W3.3:高危操作创建决策链构建器,逐步收集决策步骤
        let mut decision_builder = DecisionChainBuilder::new();
        if is_high_risk {
            decision_builder.add_proposal(&spec);
        }

        if let EscalationOutcome::Rejected(e) =
            self.handle_escalation(&spec, tier, &mut decision_builder)
        {
            // §16.5(Phase 10 Wave 6):升级通道拒绝(EscalateToHuman/ASA Block/否决)
            self.stats.record_blocked();
            return Err(e);
        }

        info!(
            program = %spec.program,
            risk_level = ?spec.risk_level,
            risk_score = spec.risk_score,
            "命令通过策略校验,进入沙箱执行"
        );

        // 步骤4(N5 修复 + P1-W3.3):pre-execution audit — 执行前记录 Intent(带决策链)
        // WHY: append_intent 失败时 `?` 短路,阻止命令执行,确保无意图无执行
        //      P1-W3.3:高危操作用 append_intent_with_chain 携带 pre-execution 决策链
        let record_id = if is_high_risk {
            let pre_chain = decision_builder.build();
            self.audit_chain
                .append_intent_with_chain(&spec, pre_chain)?
        } else {
            self.audit_chain.append_intent(&spec)?
        };
        // P1-W4.1: record_id 即决策链标识,填充到顶层 span 的 decision_chain_id 字段
        // 供 efficiency-monitor 与 TUI 关联同一操作的完整审计链与 tracing 事件
        tracing::Span::current().record("decision_chain_id", record_id);

        // 步骤5:沙箱执行 — 进程隔离(Windows 降级 / Linux gVisor)
        let exec_result = self.execute_in_sandbox(&spec).await;

        // 步骤5b-6(P1-W3.3 + N5 修复):post-execution audit — 补充决策链 + 更新审计状态
        // WHY: P1-W3.3:高危操作补充 Execution + Result 步骤到决策链
        //      N5:无论成功失败都要更新审计链,防止 Intent 记录永久悬挂
        self.post_execution_audit(record_id, &exec_result, is_high_risk);
        exec_result
    }

    /// 升级通道处理 — 按 EscalationTier 分档处理高危操作(P2-1 提取)。
    ///
    /// 对应 `audit_and_execute` 步骤3:
    /// - EscalateToHuman (91-100):拒绝执行,决策链 [Proposal, Result(rejected)] 上链
    /// - Parliament (71-90):
    ///   - a. 若配置了 asa_auditor,先做 ASA 前置实时审计(P1-W3.2)
    ///   - b. 调用 escalation_handler.parliament_debate()
    /// - ReadOnly/Normal:直接执行(决策链为空,向后兼容)
    ///
    /// # 返回
    /// - `EscalationOutcome::Proceed`:升级通过,继续执行流程
    /// - `EscalationOutcome::Rejected(e)`:操作被拒绝,含错误原因
    fn handle_escalation(
        &mut self,
        spec: &CommandSpec,
        tier: EscalationTier,
        decision_builder: &mut DecisionChainBuilder,
    ) -> EscalationOutcome {
        match tier {
            EscalationTier::EscalateToHuman => {
                // risk_score ∈ [91,100]:拒绝执行,升级人工
                // WHY: 不调用 escalation_handler,直接返回错误。
                //      此类操作过于危险,Parliament 辩论不足以承担风险。
                // P1-W4.1: tier / decision_chain_id 与顶层 span 字段对齐
                // decision_chain_id 在 append_intent_with_chain 后填充(占位 Empty)
                tracing::warn!(
                    program = %spec.program,
                    risk_score = spec.risk_score,
                    tier = ?tier,
                    decision_chain_id = tracing::field::Empty,
                    "高危操作强制升级人工 (risk_score ≥ 91)"
                );
                // P1-W3.3:拒绝操作决策链上链 [Proposal, Result(rejected)]
                decision_builder.add_rejected_result("escalate_to_human");
                let chain = decision_builder.build();
                if let Err(e) = self.audit_chain.append_intent_with_chain(spec, chain) {
                    tracing::error!(error = %e, "EscalateToHuman 决策链 append 失败");
                } else if let Err(e) = self.audit_chain.update_status(
                    (self.audit_chain.len() - 1) as u64,
                    AuditRecordStatus::Failed,
                    None,
                ) {
                    tracing::error!(error = %e, "EscalateToHuman update_status(Failed) 失败");
                }
                return EscalationOutcome::Rejected(SecCoreError::EscalateToHuman {
                    risk_score: spec.risk_score,
                    program: spec.program.clone(),
                    reason: format!(
                        "risk_score {} ≥ 91, 操作过于危险, 必须人工确认后执行",
                        spec.risk_score
                    ),
                });
            }
            EscalationTier::Parliament => {
                // risk_score ∈ [71,90]:ASA 前置实时审计 → Parliament 辩论 + 自白通道复核
                //
                // WHY(P1-W3.2 / D6 修复): spec.md 要求高危操作在 Parliament 辩论前
                // 先经 ASA 实时审计。ASA Block → 返回 AsaBlocked(辩论跳过,事中拦截优先);
                // ASA Allow/Warn → 继续进入 parliament_debate(handler 决定是否批准)。
                // 未配置 asa_auditor 时回退到 P1-W3.1 既有行为(直接辩论)。
                tracing::info!(
                    program = %spec.program,
                    risk_score = spec.risk_score,
                    tier = ?tier,
                    decision_chain_id = tracing::field::Empty,
                    "高危操作进入 ASA 前置审计 + Parliament 辩论"
                );
                // P1-W3.3:ASA 审计结果纳入决策链
                if let Some(ref asa) = self.asa_auditor {
                    let input = build_asa_input(spec, &self.audit_chain);
                    match asa.audit_and_intervene(&input) {
                        Ok(asa_result) => {
                            // ASA 通过(Allow/Warn):记录审计结果,继续进入 Parliament 辩论
                            decision_builder.add_asa_audit(&asa_result);
                        }
                        Err(e) => {
                            // P1-W3.3:ASA Block 决策链上链 [Proposal, Result(rejected:asa_blocked)]
                            decision_builder.add_rejected_result("asa_blocked");
                            let chain = decision_builder.build();
                            if let Err(audit_err) =
                                self.audit_chain.append_intent_with_chain(spec, chain)
                            {
                                tracing::error!(error = %audit_err, "ASA Block 决策链 append 失败");
                            } else if let Err(audit_err) = self.audit_chain.update_status(
                                (self.audit_chain.len() - 1) as u64,
                                AuditRecordStatus::Failed,
                                None,
                            ) {
                                tracing::error!(
                                    error = %audit_err,
                                    "ASA Block update_status(Failed) 失败"
                                );
                            }
                            return EscalationOutcome::Rejected(e);
                        }
                    }
                }
                // Parliament 辩论 + 自白通道复核
                // P1-W4.1: 嵌套 span 标记 Parliament 辩论阶段,在 tracing 树中
                // 与外层 audit_and_execute span 形成父子关系,便于效率监控定位辩论耗时
                let debate_span = tracing::info_span!(
                    "parliament_debate",
                    program = %spec.program,
                    risk_score = spec.risk_score,
                    tier = ?tier,
                    decision_chain_id = tracing::field::Empty
                );
                let _debate_guard = debate_span.enter();
                match self
                    .escalation_handler
                    .parliament_debate(spec, spec.risk_score)
                {
                    Ok(()) => {
                        // 辩论通过:记录辩论结果 + 自白,继续执行流程
                        tracing::info!(decision = "approved", "Parliament 辩论通过");
                        decision_builder.add_debate(true);
                        // 自白:操作意图披露(program + risk_score)
                        decision_builder.add_confession(&format!(
                            "program={}, risk_score={}",
                            spec.program, spec.risk_score
                        ));
                    }
                    Err(e) => {
                        // P1-W3.3:Parliament 否决决策链上链 [Proposal, (AsaAudit), Debate(rejected), Result(rejected)]
                        tracing::info!(decision = "rejected", "Parliament 辩论否决");
                        decision_builder.add_debate(false);
                        decision_builder.add_rejected_result("parliament_rejected");
                        let chain = decision_builder.build();
                        if let Err(audit_err) =
                            self.audit_chain.append_intent_with_chain(spec, chain)
                        {
                            tracing::error!(error = %audit_err, "Parliament 否决决策链 append 失败");
                        } else if let Err(audit_err) = self.audit_chain.update_status(
                            (self.audit_chain.len() - 1) as u64,
                            AuditRecordStatus::Failed,
                            None,
                        ) {
                            tracing::error!(
                                error = %audit_err,
                                "Parliament 否决 update_status(Failed) 失败"
                            );
                        }
                        return EscalationOutcome::Rejected(e);
                    }
                }
            }
            EscalationTier::ReadOnly | EscalationTier::Normal => {
                // 直接执行,无需升级(决策链为空,向后兼容)
                // P1-W4.1: 即使低危路径也记录 tier,与高危路径字段对齐便于过滤聚合
                tracing::info!(
                    program = %spec.program,
                    risk_score = spec.risk_score,
                    tier = ?tier,
                    "低危操作直接进入沙箱执行(无需升级通道)"
                );
            }
        }
        EscalationOutcome::Proceed
    }

    /// 后执行审计 — 补充决策链 + 更新审计状态(P2-1 提取)。
    ///
    /// 对应 `audit_and_execute` 步骤5b-6:
    /// - P1-W3.3:高危操作补充 Execution + Result 步骤到决策链
    /// - N5 修复:根据执行结果更新审计状态(Executed/Failed)
    fn post_execution_audit(
        &mut self,
        record_id: RecordId,
        exec_result: &Result<ExecutionResult, SecCoreError>,
        is_high_risk: bool,
    ) {
        // P1-W3.3:执行后补充 Execution + Result 步骤到决策链(仅高危操作)
        if is_high_risk {
            let mut post_builder = DecisionChainBuilder::new();
            post_builder.add_execution();
            match exec_result {
                Ok(result) => {
                    post_builder.add_result(result.exit_code);
                }
                Err(_) => {
                    post_builder.add_result(-1);
                }
            }
            if let Err(e) = self
                .audit_chain
                .extend_decision_chain(record_id, post_builder.build())
            {
                tracing::error!(
                    record_id = record_id,
                    error = %e,
                    "审计链 extend_decision_chain 失败,决策链可能不完整(缺 Execution/Result 步骤)"
                );
            }
        }

        // 步骤6(N5 修复):post-execution update — 根据执行结果更新审计状态
        // WHY: 无论成功失败都要更新审计链,防止 Intent 记录永久悬挂
        match exec_result {
            Ok(result) => {
                // 执行成功:更新为 Executed,填充 result_hash
                if let Err(e) = self.audit_chain.update_status(
                    record_id,
                    AuditRecordStatus::Executed,
                    Some(result),
                ) {
                    // WHY: update_status 失败不影响已执行的命令结果,但记录错误供审计
                    // 审计链更新失败是严重异常(理论上不会发生),仅记日志不阻塞返回
                    tracing::error!(
                        record_id = record_id,
                        error = %e,
                        "审计链 update_status(Executed) 失败,执行结果仍返回但审计可能不完整"
                    );
                }

                info!(
                    exit_code = result.exit_code,
                    audit_hash = %result.audit_hash,
                    "命令执行完成,审计记录已更新为 Executed"
                );
            }
            Err(e) => {
                // 执行失败:更新为 Failed,保持审计链完整(记录失败意图)
                // WHY: 用 let _ = 忽略 update_status 的二次错误,优先返回原始执行错误
                //      审计更新失败仅记日志,不掩盖原始执行失败原因
                if let Err(audit_err) =
                    self.audit_chain
                        .update_status(record_id, AuditRecordStatus::Failed, None)
                {
                    tracing::error!(
                        record_id = record_id,
                        error = %audit_err,
                        "审计链 update_status(Failed) 失败,执行错误仍返回但审计可能不完整"
                    );
                }

                info!(error = %e, "命令执行失败,审计记录已更新为 Failed");
            }
        }
    }

    /// 在沙箱中执行校验通过的命令规格。
    ///
    /// 跨平台策略(Task 12 gVisor 集成):
    /// - **Linux + gVisor 可用**:通过 `GvisorRuntime::spawn()` 在 gVisor 用户空间内核
    ///   中执行命令,实现内核级系统调用拦截与 seccomp 过滤(ADR-001)
    /// - **Linux + gVisor 不可用**:降级为 `tokio::process::Command` 进程隔离,
    ///   记录 `warn!` 日志(须人工排查 runsc 部署问题)
    /// - **Windows/macOS**:始终使用 `tokio::process::Command` 进程隔离,
    ///   记录 `info!` 日志(平台限制,预期行为)
    ///
    /// # 安全提示
    /// 此函数只接受 `CommandSpec`(已通过策略校验),不接受原始 `Command`。
    /// 调用方必须先调用 `validate_command`。
    ///
    /// # gVisor 可用性判定(三层检测)
    /// 1. `self.use_gvisor` — 配置层是否启用
    /// 2. `self.gvisor_runtime` — 运行时是否已注入
    /// 3. `runtime.is_available()` — 平台(仅 Linux) + runsc 二进制是否存在
    async fn execute_in_sandbox(
        &self,
        spec: &CommandSpec,
    ) -> Result<ExecutionResult, SecCoreError> {
        let start = Instant::now();

        // ── gVisor 可用性检测 ──
        // WHY: 三层检测确保"意图启用 → 已配置 → 运行时可用"全链路通过,
        // 任一层不满足即降级为进程隔离,避免在非预期环境(如测试)误用 gVisor
        let gvisor_available = self.use_gvisor
            && self
                .gvisor_runtime
                .as_ref()
                .is_some_and(|rt| rt.is_available());

        // ── 执行路径选择 ──
        let output = if gvisor_available {
            // 路径 A: gVisor 内核级隔离 (Linux 生产环境)
            let runtime = self
                .gvisor_runtime
                .as_ref()
                .expect("gvisor_runtime 存在性已在 is_some_and 中检查,此 expect 不会触发");

            info!(
                runsc_path = %runtime.runsc_path(),
                program = %spec.program,
                "通过 gVisor runsc 执行命令 (内核级隔离)"
            );

            // WHY: 超时保护 — gVisor spawn 与 TokioCommand 使用相同的超时机制,
            // 防止恶意命令(如 sleep infinity)永久阻塞,导致 DoS (F-002)
            match tokio::time::timeout(self.timeout, runtime.spawn(spec)).await {
                Ok(Ok(output)) => output,
                Ok(Err(e)) => {
                    return Err(SecCoreError::SandboxError(format!(
                        "gVisor runsc 执行失败: {}",
                        e
                    )));
                }
                Err(_) => {
                    return Err(SecCoreError::SandboxTimeout {
                        timeout: self.timeout,
                        program: spec.program.clone(),
                    });
                }
            }
        } else {
            // 路径 B: 进程隔离降级 (Windows/macOS 或 Linux 无 runsc)
            // WHY: 区分降级原因 — warn 表示"预期启用但不可用"(需人工排查),
            // info 表示"平台限制或显式禁用"(预期行为)
            if self.use_gvisor {
                warn!(
                    program = %spec.program,
                    "gVisor 不可用，降级为进程隔离执行 (请检查 runsc 是否已安装)"
                );
            } else {
                info!(
                    program = %spec.program,
                    "gVisor 已禁用，使用进程隔离执行"
                );
            }

            // 构建子进程命令
            // 注意:此处不使用 shell(无 sh -c),避免 shell 注入风险
            // 参数直接传递给 execve,不经 shell 二次解析
            let mut cmd = TokioCommand::new(&spec.program);
            cmd.args(&spec.allowed_args);

            // 仅传递白名单过滤后的环境变量(零信任:不继承父进程环境)
            cmd.env_clear();
            for (k, v) in &spec.env_whitelist {
                cmd.env(k, v);
            }

            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());
            cmd.kill_on_drop(true);

            // WHY: 超时保护 — 防止恶意命令永久阻塞(F-002)
            // 2026-08-07 ultra-plan 修复:原 `cmd.output()` 在超时后无法访问 child 句柄,
            // 只能依赖 kill_on_drop 杀直接子进程;Windows 上 `cmd /C <long-cmd>` 的孙进程
            // (如 ping)不随父进程退出,既造成进程泄漏(违背沙箱"超时即终止"承诺),
            // 又阻塞 tokio current_thread runtime 的 drop(blocking 清理等待孙进程自然
            // 退出,实测 1s 超时测试被拖至 ~29s)。重构为 spawn + 手动收集输出,超时后
            // 显式终止整个进程树,把清理时间收敛到 ~1s(probe 实测 29.3s → 1.35s)。
            let mut child = cmd
                .spawn()
                .map_err(|e| SecCoreError::SandboxError(format!("进程启动失败: {e}")))?;

            // 取走 stdout/stderr 管道:输出收集与 wait 并行,且不消费 child 本体,
            // 保证超时后仍可访问 child 做进程树清理(对比 wait_with_output 按值消费)。
            let mut stdout_pipe = child.stdout.take();
            let mut stderr_pipe = child.stderr.take();

            let collect = async {
                let mut stdout_buf = Vec::new();
                let mut stderr_buf = Vec::new();
                if let Some(s) = stdout_pipe.as_mut() {
                    tokio::io::AsyncReadExt::read_to_end(s, &mut stdout_buf).await?;
                }
                if let Some(e) = stderr_pipe.as_mut() {
                    tokio::io::AsyncReadExt::read_to_end(e, &mut stderr_buf).await?;
                }
                let status = child.wait().await?;
                Ok::<std::process::Output, std::io::Error>(std::process::Output {
                    status,
                    stdout: stdout_buf,
                    stderr: stderr_buf,
                })
            };

            // match 作为 else 块最终表达式(与路径 A 的 match 结构一致)
            match tokio::time::timeout(self.timeout, collect).await {
                Ok(Ok(output)) => output,
                Ok(Err(e)) => {
                    return Err(SecCoreError::SandboxError(format!("进程执行失败: {e}")));
                }
                Err(_) => {
                    // 超时:先终止整个进程树再返回超时错误(修复进程泄漏)
                    kill_process_tree(&mut child).await?;
                    return Err(SecCoreError::SandboxTimeout {
                        timeout: self.timeout,
                        program: spec.program.clone(),
                    });
                }
            }
        };

        // ── 共享输出处理(两个路径的输出格式一致) ──
        let duration = start.elapsed();

        // 解码输出(UTF-8 失败时用替换字符,避免 panic)
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

        // 退出码:信号终止时 code() 返回 None,用 -1 表示
        let exit_code = output.status.code().unwrap_or(-1);

        // 计算审计哈希(执行结果摘要,用于审计链)
        let audit_hash = compute_audit_hash(exit_code, &stdout, &stderr, duration);

        Ok(ExecutionResult {
            exit_code,
            stdout,
            stderr,
            duration,
            audit_hash,
        })
    }
}

/// 计算执行结果的审计哈希(SHA-256 十六进制)。
///
/// 哈希内容:exit_code || stdout || stderr || duration_nanos。
/// 此哈希存储在 `ExecutionResult.audit_hash`,用于快速比对。
/// 审计链验证时会重新计算(不信任此字段),防止篡改。
fn compute_audit_hash(exit_code: i32, stdout: &str, stderr: &str, duration: Duration) -> String {
    let mut hasher = Sha256::new();
    hasher.update(exit_code.to_le_bytes());
    hasher.update(stdout.as_bytes());
    hasher.update(stderr.as_bytes());
    hasher.update(duration.as_nanos().to_le_bytes());
    hex::encode(hasher.finalize())
}

/// 终止整个进程树(超时清理路径)
///
/// WHY: `kill_on_drop` 只终止直接子进程;Windows 上 `cmd /C <long-cmd>` 的孙进程
/// (如 ping)不随父进程退出。若不清理,泄漏进程会阻塞 tokio runtime 的 blocking
/// 清理(实测 1s 超时测试被拖至 ~29s),且违背沙箱"超时即终止"的安全承诺。
/// Windows 用 `taskkill /T`(终止进程树);Unix 直接 kill(目标命令为单进程)。
#[cfg(windows)]
async fn kill_process_tree(child: &mut tokio::process::Child) -> Result<(), SecCoreError> {
    let pid = child.id().ok_or_else(|| {
        SecCoreError::SandboxError("子进程已退出,无法获取 PID 做进程树清理".to_string())
    })?;
    // taskkill /PID <pid> /T /F:终止 pid 及其全部后代进程
    let status = TokioCommand::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status()
        .await
        .map_err(|e| SecCoreError::SandboxError(format!("taskkill 失败: {e}")))?;
    if !status.success() {
        // taskkill 失败(进程可能已自行退出),降级为直接 kill
        let _ = child.kill().await;
    }
    // 等待子进程退出,确保清理完成(进程已死,立即返回)
    let _ = child.wait().await;
    Ok(())
}

#[cfg(not(windows))]
async fn kill_process_tree(child: &mut tokio::process::Child) -> Result<(), SecCoreError> {
    let _ = child.kill().await;
    let _ = child.wait().await;
    Ok(())
}

/// 从 CommandSpec 构造 ASA 审计输入 — 用于 Parliament 档前置审计(P1-W3.2 / D6 修复)。
///
/// WHY: ASA 审计需要 `OperationAuditInput`(content/risk_keywords/complexity_score),
/// 但 Sandbox 内部只有 `CommandSpec`。此函数做适配转换:
/// - `operation_id`:用审计链长度作序号(单调递增,保证唯一)
/// - `content`:program + allowed_args 拼接(与 `assess_risk` 输入对齐)
/// - `risk_keywords`:预定义高危关键字列表(覆盖 rm/dd/mkfs/sudo/secret 等)
/// - `complexity_score`:risk_score / 100.0(高危操作复杂度高)
fn build_asa_input(spec: &CommandSpec, audit_chain: &AuditChain) -> OperationAuditInput {
    let operation_id = format!("sandbox-esc-{}", audit_chain.len());
    let content = format!("{} {}", spec.program, spec.allowed_args.join(" "));
    let risk_keywords = vec![
        "rm".to_string(),
        "dd".to_string(),
        "mkfs".to_string(),
        "fdisk".to_string(),
        "shred".to_string(),
        "wipe".to_string(),
        "sudo".to_string(),
        "chmod".to_string(),
        "chown".to_string(),
        "secret".to_string(),
        "password".to_string(),
    ];
    let complexity_score = (spec.risk_score as f32) / 100.0;
    OperationAuditInput {
        operation_id,
        content,
        risk_keywords,
        complexity_score,
    }
}

/// §16.5(Phase 10 Wave 6):沙箱拦截率周期报告器
///
/// 周期采样 [`InterceptorStats`] 快照,发布 `SecurityInterceptionReported`
/// 观测面事件(真实采集,非伪造指标)。首 tick 立即返回采样基线,
/// 窗口长度由 `interval_secs` 决定(下限 1 秒防忙循环)。
pub fn spawn_interception_reporter(
    bus: EventBus,
    stats: Arc<InterceptorStats>,
    interval_secs: u64,
) -> tokio::task::JoinHandle<()> {
    let window_secs = interval_secs.max(1);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(window_secs));
        interval.tick().await; // 基线采样:跳过首个立即 tick
        loop {
            interval.tick().await;
            let (total, blocked) = stats.snapshot();
            let event = NexusEvent::SecurityInterceptionReported {
                metadata: EventMetadata::new("seccore"),
                total_requests: total,
                blocked_requests: blocked,
                interception_rate: stats.interception_rate(),
            };
            if let Err(e) = bus.publish_blocking(event) {
                warn!(error = %e, "SecurityInterceptionReported 发布失败");
            }
        }
    })
}

#[cfg(test)]
mod private_api_tests {
    // L4 深度优化 P2-1:私有 API 白盒测试(handle_escalation/post_execution_audit
    // 访问私有字段)保留在 src 内;公共 API 测试已外移 tests/sandbox_integration.rs。
    use super::*;
    use crate::audit::{AuditRecordStatus, DecisionChainBuilder, DecisionStepType};
    use crate::types::{CommandSpec, EscalationTier, ExecutionResult, RiskLevel};
    use std::collections::HashMap;
    use std::time::Duration;
    fn make_spec(program: &str, risk_score: u8) -> CommandSpec {
        CommandSpec {
            program: program.to_string(),
            allowed_args: Vec::new(),
            env_whitelist: HashMap::new(),
            risk_level: RiskLevel::from_score(risk_score),
            risk_score,
        }
    }

    /// 辅助函数:构造测试用的 ExecutionResult
    fn make_result() -> ExecutionResult {
        ExecutionResult {
            exit_code: 0,
            stdout: "output".to_string(),
            stderr: String::new(),
            duration: Duration::from_millis(10),
            audit_hash: "0".repeat(64),
        }
    }

    /// 总是批准的 EscalationHandler mock — 用于测试 Parliament 档位通过路径。
    struct MockApprovingHandler;

    impl EscalationHandler for MockApprovingHandler {
        fn parliament_debate(
            &self,
            _spec: &CommandSpec,
            _risk_score: u8,
        ) -> Result<(), SecCoreError> {
            Ok(())
        }
    }

    /// 测试 handle_escalation: EscalateToHuman 档位返回 Rejected 并增加审计链长度。
    #[test]
    fn test_handle_escalation_escalate_to_human_rejected() {
        let mut sandbox = Sandbox::with_default_policy();
        let spec = make_spec("dd", 95);
        let tier = EscalationTier::EscalateToHuman;
        let mut decision_builder = DecisionChainBuilder::new();
        decision_builder.add_proposal(&spec);

        let initial_len = sandbox.audit_chain.len();
        let result = sandbox.handle_escalation(&spec, tier, &mut decision_builder);

        assert!(matches!(result, EscalationOutcome::Rejected(_)));
        assert_eq!(
            sandbox.audit_chain.len(),
            initial_len + 1,
            "EscalateToHuman 应追加审计记录"
        );
    }

    /// 测试 handle_escalation: ReadOnly 和 Normal 档位返回 Proceed。
    #[test]
    fn test_handle_escalation_readonly_normal_proceed() {
        let mut sandbox = Sandbox::with_default_policy();
        let mut decision_builder = DecisionChainBuilder::new();

        let spec = make_spec("echo", 10);
        let result =
            sandbox.handle_escalation(&spec, EscalationTier::ReadOnly, &mut decision_builder);
        assert!(matches!(result, EscalationOutcome::Proceed));

        let spec2 = make_spec("echo", 50);
        let mut decision_builder2 = DecisionChainBuilder::new();
        let result2 =
            sandbox.handle_escalation(&spec2, EscalationTier::Normal, &mut decision_builder2);
        assert!(matches!(result2, EscalationOutcome::Proceed));
    }

    /// 测试 handle_escalation: Parliament 档位 + 总是批准的 handler 返回 Proceed。
    #[test]
    fn test_handle_escalation_parliament_with_handler_approved_proceed() {
        let mut sandbox =
            Sandbox::with_default_policy().with_escalation_handler(Box::new(MockApprovingHandler));
        let spec = make_spec("rm", 80);
        let tier = EscalationTier::Parliament;
        let mut decision_builder = DecisionChainBuilder::new();
        decision_builder.add_proposal(&spec);

        let result = sandbox.handle_escalation(&spec, tier, &mut decision_builder);
        assert!(matches!(result, EscalationOutcome::Proceed));
    }

    /// 测试 handle_escalation: Parliament 档位 + 默认 handler 返回 Rejected。
    #[test]
    fn test_handle_escalation_parliament_default_handler_rejected() {
        let mut sandbox = Sandbox::with_default_policy();
        let spec = make_spec("rm", 80);
        let tier = EscalationTier::Parliament;
        let mut decision_builder = DecisionChainBuilder::new();
        decision_builder.add_proposal(&spec);

        let result = sandbox.handle_escalation(&spec, tier, &mut decision_builder);
        assert!(matches!(result, EscalationOutcome::Rejected(_)));
    }

    /// 测试 post_execution_audit: 成功执行后更新审计状态为 Executed。
    #[test]
    fn test_post_execution_audit_success_updates_executed() {
        let mut sandbox = Sandbox::with_default_policy();
        let spec = make_spec("echo", 10);
        let record_id = sandbox.audit_chain.append_intent(&spec).unwrap();
        let exec_result = Ok(make_result());

        sandbox.post_execution_audit(record_id, &exec_result, false);

        let block = &sandbox.audit_chain.blocks[record_id as usize];
        assert_eq!(block.status, AuditRecordStatus::Executed);
    }

    /// 测试 post_execution_audit: 执行失败后更新审计状态为 Failed。
    #[test]
    fn test_post_execution_audit_failure_updates_failed() {
        let mut sandbox = Sandbox::with_default_policy();
        let spec = make_spec("echo", 10);
        let record_id = sandbox.audit_chain.append_intent(&spec).unwrap();
        let exec_result: Result<ExecutionResult, SecCoreError> =
            Err(SecCoreError::SandboxError("test failure".to_string()));

        sandbox.post_execution_audit(record_id, &exec_result, false);

        let block = &sandbox.audit_chain.blocks[record_id as usize];
        assert_eq!(block.status, AuditRecordStatus::Failed);
    }

    /// 测试 post_execution_audit: 高危操作成功执行后更新状态为 Executed,决策链扩展。
    #[test]
    fn test_post_execution_audit_high_risk_extend_chain() {
        let mut sandbox = Sandbox::with_default_policy();
        let spec = make_spec("rm", 80);
        let pre_chain = DecisionChainBuilder::new().add_proposal(&spec).build();
        let record_id = sandbox
            .audit_chain
            .append_intent_with_chain(&spec, pre_chain)
            .unwrap();
        let exec_result = Ok(make_result());

        sandbox.post_execution_audit(record_id, &exec_result, true);

        let block = &sandbox.audit_chain.blocks[record_id as usize];
        assert_eq!(block.status, AuditRecordStatus::Executed);
        assert!(
            block
                .decision_chain
                .iter()
                .any(|s| s.step_type == DecisionStepType::Execution),
            "高危操作决策链应包含 Execution 步骤"
        );
        assert!(
            block
                .decision_chain
                .iter()
                .any(|s| s.step_type == DecisionStepType::Result),
            "高危操作决策链应包含 Result 步骤"
        );
    }
}
