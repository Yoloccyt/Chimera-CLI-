//! Hook 执行器 — 沙箱校验 + 超时熔断 + 审计（P3-T3，v4.0 WI-24）
//!
//! 对应架构层: L9 Quest（nexus-hook，ADR-146）
//!
//! # 执行管线（每条 hook 规格）
//! 1. **信任检查**:`TrustLevel::Untrusted` → 全部拒否（fail-closed）;
//! 2. **沙箱检查**:seccore `ProcessFence::check(program, args)` — 逃逸拒绝
//!    （写 /etc、越界网络,任意模式）;
//! 3. **执行**:`tokio::process::Command` + 超时（spec.timeout_ms,默认 5s）;
//!    环境变量注入 `$TOOL_NAME` / `$SESSION_ID` / `$GOAL_ID`;
//! 4. **中断判定**:可中断事件（Pre 类）非零退出码 → `interrupted = true`
//!    （调用方据此拒否该次工具调用）;
//! 5. **审计**:全量记录进 [`HookAudit`]。

use std::sync::Arc;
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};

use crate::audit::{make_entry, AuditSink, HookAudit, NoopAuditSink};
use crate::config::{HookConfig, HookSpec};
use crate::lifecycle::LifecycleEvent;
use seccore::os_backend::{ProcessFence, SandboxMode};
use thiserror::Error;

/// 单条 hook 执行结果
#[derive(Debug, Clone, PartialEq)]
pub struct HookResult {
    /// 命令
    pub command: String,
    /// 退出码（None = 超时熔断 / 未执行）
    pub exit_code: Option<i32>,
    /// 是否中断（可中断事件 + 非零退出码 → 调用方拒否）
    pub interrupted: bool,
    /// 是否被沙箱拒绝
    pub sandbox_denied: bool,
}

/// 触发选项 — Phase 5 的并行/总预算开关。
///
/// 默认 `parallel_post=false`，保持旧串行语义；Pre 类事件始终串行（保序 +
/// fail-closed），只有 PostToolUse 允许在显式 opt-in 后并行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookTriggerOpts {
    /// 是否允许 PostToolUse 并行（默认 false）。
    pub parallel_post: bool,
    /// Post 并行上限（默认 4；钳到 ≥1）。
    pub max_parallel: usize,
    /// 整批总预算（默认 30s；超时 Pre fail-closed / Post 只记录）。
    pub total_timeout_ms: u64,
}

impl Default for HookTriggerOpts {
    fn default() -> Self {
        Self {
            parallel_post: false,
            max_parallel: 4,
            total_timeout_ms: 30_000,
        }
    }
}

/// 执行错误 — 库层 thiserror（§4.1）
#[derive(Debug, Error)]
pub enum HookError {
    /// 沙箱拒绝（逃逸检测失败）
    #[error("hook sandbox denied: {0}")]
    SandboxDenied(String),
    /// 命令启动失败
    #[error("hook spawn failed: {0}")]
    Spawn(String),
}

/// Hook 执行器 — 配置 + 沙箱 + 审计
pub struct HookExecutor {
    /// 挂载配置
    config: HookConfig,
    /// 进程围栏（沙箱;default_secure 策略 + Standard 模式）
    ///
    /// WHY Mutex:ProcessFence::check 为 `&mut self`（内部统计 denied_ops）,无 Clone;
    /// 锁内无 await（check 为纯 CPU 校验）,不违反持锁跨 await 红线
    fence: std::sync::Mutex<ProcessFence>,
    /// 审计（append-only）
    audit: Arc<HookAudit>,
    /// 审计汇出（session-store 注入点;默认 Noop = 纯内存审计）
    sink: Arc<dyn AuditSink>,
}

impl std::fmt::Debug for HookExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Arc<dyn AuditSink> 无 Debug（trait 对象边界）:仅输出配置与审计计数
        f.debug_struct("HookExecutor")
            .field("config", &self.config)
            .field("audit_len", &self.audit.len())
            .finish_non_exhaustive()
    }
}

impl HookExecutor {
    /// 新建执行器（构造参数:配置 + 沙箱策略 + 审计;非 feature 标志——禁 feature 红线）
    #[must_use]
    pub fn new(config: HookConfig, fence: ProcessFence, audit: Arc<HookAudit>) -> Self {
        Self {
            config,
            fence: std::sync::Mutex::new(fence),
            audit,
            sink: Arc::new(NoopAuditSink),
        }
    }

    /// 注入审计汇出（session-store 适配器;组合根装配）
    #[must_use]
    pub fn with_audit_sink(mut self, sink: Arc<dyn AuditSink>) -> Self {
        self.sink = sink;
        self
    }

    /// 便捷构造 — 默认围栏 + 独立审计
    #[must_use]
    pub fn with_config(config: HookConfig) -> Self {
        let fence = ProcessFence::new(
            Arc::new(nexus_contracts::command_validation::CommandPolicy::default()),
            SandboxMode::Standard,
        );
        Self::new(config, fence, Arc::new(HookAudit::new()))
    }

    /// 触发事件 — 执行全部挂载 hook,返回结果列表
    ///
    /// # 参数
    /// - `event`:生命周期事件
    /// - `tool_name` / `session_id` / `goal_id`:环境变量注入值（可选,空则跳过注入）
    ///
    /// # 返回
    /// 每条 hook 的结果;可中断事件任一 `interrupted` → 调用方拒否。
    pub async fn trigger(
        &self,
        event: LifecycleEvent,
        tool_name: Option<&str>,
        session_id: Option<&str>,
        goal_id: Option<&str>,
    ) -> Vec<HookResult> {
        self.trigger_with(
            event,
            tool_name,
            session_id,
            goal_id,
            HookTriggerOpts::default(),
        )
        .await
    }

    /// 带选项的触发入口（Phase 5）：Pre 串行、Post 可并行、整批总预算。
    pub async fn trigger_with(
        &self,
        event: LifecycleEvent,
        tool_name: Option<&str>,
        session_id: Option<&str>,
        goal_id: Option<&str>,
        opts: HookTriggerOpts,
    ) -> Vec<HookResult> {
        let specs = self.config.specs_for(event);
        // 信任检查（fail-closed:非 Trusted 一律拒否,不执行不审计）
        if !self.config.trust.allows_execution() {
            return specs
                .iter()
                .map(|spec| HookResult {
                    command: spec.command.clone(),
                    exit_code: None,
                    interrupted: false,
                    sandbox_denied: true,
                })
                .collect();
        }
        if opts.parallel_post && matches!(event, LifecycleEvent::PostToolUse) && specs.len() > 1 {
            return self
                .run_post_parallel(event, specs, tool_name, session_id, goal_id, opts)
                .await;
        }
        self.run_serial(event, specs, tool_name, session_id, goal_id, opts)
            .await
    }

    async fn run_serial(
        &self,
        event: LifecycleEvent,
        specs: &[HookSpec],
        tool_name: Option<&str>,
        session_id: Option<&str>,
        goal_id: Option<&str>,
        opts: HookTriggerOpts,
    ) -> Vec<HookResult> {
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(opts.total_timeout_ms.max(1));
        let mut results = Vec::with_capacity(specs.len());
        for spec in specs {
            match tokio::time::timeout_at(
                deadline,
                self.run_one(event, spec, tool_name, session_id, goal_id),
            )
            .await
            {
                Ok(result) => results.push(result),
                Err(_) => results.push(HookResult {
                    command: spec.command.clone(),
                    exit_code: None,
                    // Pre 类超时 fail-closed；Post/观测类只记录。
                    interrupted: event.interruptible(),
                    sandbox_denied: false,
                }),
            }
        }
        results
    }

    async fn run_post_parallel(
        &self,
        event: LifecycleEvent,
        specs: &[HookSpec],
        tool_name: Option<&str>,
        session_id: Option<&str>,
        goal_id: Option<&str>,
        opts: HookTriggerOpts,
    ) -> Vec<HookResult> {
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(opts.total_timeout_ms.max(1));
        let max_parallel = opts.max_parallel.max(1);
        // WHY 走 run_indexed 而非就地 async block:no two async blocks have the same type
        // (E0308) —— 窗口内补位与初始填充必须共用同一个 Future 类型
        let mut futures = FuturesUnordered::new();
        let mut pending = specs.iter().enumerate();
        for _ in 0..max_parallel {
            let Some((index, spec)) = pending.next() else {
                break;
            };
            futures.push(self.run_indexed(event, index, spec, tool_name, session_id, goal_id));
        }

        let mut results: Vec<Option<HookResult>> = (0..specs.len()).map(|_| None).collect();
        while !futures.is_empty() {
            match tokio::time::timeout_at(deadline, futures.next()).await {
                Ok(Some((index, result))) => {
                    results[index] = Some(result);
                    if let Some((next_index, spec)) = pending.next() {
                        futures.push(
                            self.run_indexed(
                                event, next_index, spec, tool_name, session_id, goal_id,
                            ),
                        );
                    }
                }
                Ok(None) | Err(_) => break,
            }
        }

        results
            .into_iter()
            .enumerate()
            .map(|(index, result)| {
                result.unwrap_or_else(|| HookResult {
                    command: specs[index].command.clone(),
                    exit_code: None,
                    interrupted: false,
                    sandbox_denied: false,
                })
            })
            .collect()
    }

    /// 并行窗口专用:结果绑定挂载下标,保证回读顺序与 specs 一致
    async fn run_indexed(
        &self,
        event: LifecycleEvent,
        index: usize,
        spec: &HookSpec,
        tool_name: Option<&str>,
        session_id: Option<&str>,
        goal_id: Option<&str>,
    ) -> (usize, HookResult) {
        (
            index,
            self.run_one(event, spec, tool_name, session_id, goal_id)
                .await,
        )
    }

    /// 执行单条 hook（管线:沙箱 → 执行 → 中断判定 → 审计）
    async fn run_one(
        &self,
        event: LifecycleEvent,
        spec: &crate::config::HookSpec,
        tool_name: Option<&str>,
        session_id: Option<&str>,
        goal_id: Option<&str>,
    ) -> HookResult {
        let started = std::time::Instant::now();
        // 1. 命令解析（空白拆分:首个 token = program,其余 = args）
        let tokens: Vec<&str> = spec.command.split_whitespace().collect();
        if tokens.is_empty() {
            return HookResult {
                command: spec.command.clone(),
                exit_code: None,
                interrupted: false,
                sandbox_denied: true,
            };
        }
        let program = tokens[0];
        let args: Vec<String> = tokens[1..].iter().map(|s| s.to_string()).collect();
        // 2. 沙箱检查（逃逸拒绝:写 /etc、越界网络）
        {
            let mut fence = self.fence.lock().unwrap_or_else(|p| p.into_inner());
            if let Err(_e) = fence.check(program, &args) {
                let entry = make_entry(event, &spec.command, None, started.elapsed(), false, true);
                self.audit.push(entry.clone());
                self.sink.push_audit(&entry);
                return HookResult {
                    command: spec.command.clone(),
                    exit_code: None,
                    interrupted: false,
                    sandbox_denied: true,
                };
            }
        }
        // 3. 执行（tokio::process + 超时熔断）
        let timeout = Duration::from_millis(spec.timeout_ms);
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(&args)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // 环境变量注入（WI-24:$TOOL_NAME/$SESSION_ID/$GOAL_ID）
        if let Some(v) = tool_name {
            cmd.env("TOOL_NAME", v);
        }
        if let Some(v) = session_id {
            cmd.env("SESSION_ID", v);
        }
        if let Some(v) = goal_id {
            cmd.env("GOAL_ID", v);
        }
        let output = match tokio::time::timeout(timeout, cmd.output()).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                // 启动失败:记录审计（不 panic,不阻主流程）
                let entry = make_entry(event, &spec.command, None, started.elapsed(), false, false);
                self.audit.push(entry.clone());
                self.sink.push_audit(&entry);
                tracing::warn!(command = %spec.command, error = %e, "hook spawn failed");
                return HookResult {
                    command: spec.command.clone(),
                    exit_code: None,
                    interrupted: false,
                    sandbox_denied: false,
                };
            }
            Err(_) => {
                // 超时熔断:kill 子进程,记录审计（不阻主流程）
                let entry = make_entry(event, &spec.command, None, started.elapsed(), false, false);
                self.audit.push(entry.clone());
                self.sink.push_audit(&entry);
                tracing::warn!(command = %spec.command, timeout_ms = spec.timeout_ms, "hook timed out");
                return HookResult {
                    command: spec.command.clone(),
                    exit_code: None,
                    interrupted: false,
                    sandbox_denied: false,
                };
            }
        };
        let exit_code = output.status.code();
        // 4. 中断判定:可中断事件 + 非零退出码
        let interrupted = event.interruptible() && exit_code != Some(0);
        // 5. 审计（内存 + 汇出）
        let entry = make_entry(
            event,
            &spec.command,
            exit_code,
            started.elapsed(),
            interrupted,
            false,
        );
        self.audit.push(entry.clone());
        self.sink.push_audit(&entry);
        HookResult {
            command: spec.command.clone(),
            exit_code,
            interrupted,
            sandbox_denied: false,
        }
    }

    /// 审计引用（导出/接 session-store）
    #[must_use]
    pub fn audit(&self) -> &Arc<HookAudit> {
        &self.audit
    }
}

#[cfg(test)]
mod tests {
    // WHY allow(field_reassign_with_default):测试配置用 Default + hooks.insert
    // 模式（insert 语义无法自动字面量化,8 处用例统一风格）,可读性优于嵌套 HashMap 字面量
    // WHY allow(unwrap/expect):测试码 unwrap 为 Rust 惯用法；E-5 lint 意在治理生产码
    #![allow(
        clippy::field_reassign_with_default,
        clippy::unwrap_used,
        clippy::expect_used
    )]

    use super::*;
    use crate::config::HookSpec;
    use crate::config::TrustLevel;

    /// 测试辅助:解析平台可用命令路径
    ///
    /// Windows GNU:coreutils 位于 D:/msys64/usr/bin（不在默认 PATH）,
    /// 返回绝对路径使 spawn 可达;其他平台直接用命令名。
    fn resolve(cmd: &str) -> String {
        if cfg!(windows) {
            for dir in ["D:/msys64/usr/bin", "D:/msys64/mingw64/bin"] {
                let p = format!("{dir}/{cmd}.exe");
                if std::path::Path::new(&p).exists() {
                    return p;
                }
            }
        }
        cmd.to_string()
    }

    /// 测试辅助:构造执行器（自定义宽松策略:仅放行测试命令,无拦截模式）
    fn test_executor(cfg: HookConfig, cmds: &[&str]) -> HookExecutor {
        let mut policy = nexus_contracts::command_validation::CommandPolicy::new();
        for c in cmds {
            policy = policy.allow_command(resolve(c));
        }
        let fence = ProcessFence::new(Arc::new(policy), SandboxMode::Standard);
        HookExecutor::new(cfg, fence, Arc::new(HookAudit::new()))
    }

    /// 空配置 — 无 hook 触发零结果（空载 = 现状,回退路径）
    #[tokio::test]
    async fn empty_config_noop() {
        let ex = HookExecutor::with_config(HookConfig::default());
        let results = ex
            .trigger(LifecycleEvent::PreToolUse, None, None, None)
            .await;
        assert!(results.is_empty());
        assert!(ex.audit().is_empty());
    }

    /// Untrusted — 全拒否不执行（fail-closed）
    #[tokio::test]
    async fn untrusted_fail_closed() {
        let mut cfg = HookConfig::default();
        cfg.trust = TrustLevel::Untrusted;
        cfg.hooks.insert(
            LifecycleEvent::PostToolUse,
            vec![HookSpec {
                command: resolve("true"),
                timeout_ms: 1000,
            }],
        );
        let ex = test_executor(cfg, &["true"]);
        let results = ex
            .trigger(LifecycleEvent::PostToolUse, None, None, None)
            .await;
        assert_eq!(results.len(), 1);
        assert!(results[0].sandbox_denied, "Untrusted 必须 fail-closed 拒否");
        assert!(ex.audit().is_empty(), "拒否不审计（未执行）");
    }

    /// 正常执行 — 退出码 0,审计记录
    ///
    /// 命令选 `true`:退出码恒 0;路径经 resolve 保证 Windows spawn 可达
    #[tokio::test]
    async fn happy_path_executes() {
        let mut cfg = HookConfig::default();
        cfg.trust = TrustLevel::Trusted;
        let cmd = resolve("true");
        cfg.hooks.insert(
            LifecycleEvent::PostToolUse,
            vec![HookSpec {
                command: cmd.clone(),
                timeout_ms: 2000,
            }],
        );
        let ex = test_executor(cfg, &["true"]);
        let results = ex
            .trigger(
                LifecycleEvent::PostToolUse,
                Some("tool-x"),
                Some("s1"),
                Some("g1"),
            )
            .await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].exit_code, Some(0));
        assert!(!results[0].interrupted);
        assert!(!results[0].sandbox_denied);
        assert_eq!(ex.audit().len(), 1);
        let entry = &ex.audit().snapshot()[0];
        assert_eq!(entry.command, cmd);
        assert_eq!(entry.exit_code, Some(0));
    }

    /// 可中断事件非零退出 — PreToolUse 拒否语义
    ///
    /// 命令选 `false`:退出码恒 1（非零 → 中断拒否）
    #[tokio::test]
    async fn pre_tool_use_nonzero_interrupts() {
        let mut cfg = HookConfig::default();
        cfg.trust = TrustLevel::Trusted;
        cfg.hooks.insert(
            LifecycleEvent::PreToolUse,
            vec![HookSpec {
                command: resolve("false"),
                timeout_ms: 2000,
            }],
        );
        let ex = test_executor(cfg, &["false"]);
        let results = ex
            .trigger(LifecycleEvent::PreToolUse, Some("tool-x"), None, None)
            .await;
        assert_eq!(results.len(), 1);
        assert!(
            results[0].interrupted,
            "PreToolUse 非零退出必须中断（拒否）"
        );
        assert_eq!(ex.audit().interrupted_count(), 1);
    }

    /// 非可中断事件非零退出 — 不中断（仅记录）
    #[tokio::test]
    async fn post_event_nonzero_no_interrupt() {
        let mut cfg = HookConfig::default();
        cfg.trust = TrustLevel::Trusted;
        cfg.hooks.insert(
            LifecycleEvent::PostToolUse,
            vec![HookSpec {
                command: resolve("false"),
                timeout_ms: 2000,
            }],
        );
        let ex = test_executor(cfg, &["false"]);
        let results = ex
            .trigger(LifecycleEvent::PostToolUse, None, None, None)
            .await;
        assert!(!results[0].interrupted, "Post 类非零退出不中断");
    }

    /// 超时熔断 — 慢 hook 不阻主流程（超时后返回 None 退出码）
    #[tokio::test]
    async fn timeout_fuse() {
        let mut cfg = HookConfig::default();
        cfg.trust = TrustLevel::Trusted;
        // sleep 2s 但超时 100ms → 熔断
        cfg.hooks.insert(
            LifecycleEvent::PostToolUse,
            vec![HookSpec {
                command: format!("{} 2", resolve("sleep")),
                timeout_ms: 100,
            }],
        );
        let ex = test_executor(cfg, &["sleep"]);
        let started = std::time::Instant::now();
        let results = ex
            .trigger(LifecycleEvent::PostToolUse, None, None, None)
            .await;
        assert!(
            started.elapsed() < std::time::Duration::from_millis(1500),
            "必须快速返回"
        );
        assert_eq!(results[0].exit_code, None, "超时熔断 exit_code=None");
        assert_eq!(ex.audit().len(), 1);
    }

    /// 沙箱拒绝 — 逃逸命令（写 /etc）被拒
    ///
    /// 逃逸检测与 program 白名单无关（is_escape_attempt 先于策略校验）;
    /// sh 用于构造 `> /etc/` 写路径的 args（resolve 保证 spawn 可达,但不会执行）
    #[tokio::test]
    async fn sandbox_denies_escape() {
        let mut cfg = HookConfig::default();
        cfg.trust = TrustLevel::Trusted;
        let sh = resolve("sh");
        cfg.hooks.insert(
            LifecycleEvent::PostToolUse,
            vec![HookSpec {
                command: format!("{sh} -c \"echo pwn > /etc/pwned\""),
                timeout_ms: 2000,
            }],
        );
        let ex = test_executor(cfg, &["sh"]);
        let results = ex
            .trigger(LifecycleEvent::PostToolUse, None, None, None)
            .await;
        assert!(results[0].sandbox_denied, "写 /etc 逃逸必须被沙箱拒绝");
        assert_eq!(ex.audit().sandbox_denied_count(), 1);
    }

    /// 审计汇出 — 全量审计到达注入的 sink（session-store 接线模拟,P3-T3 补）
    ///
    /// 记录 sink 模拟 session-store 适配器:每条 hook 执行（含沙箱拒绝/正常/超时）
    /// 都必须汇出;默认 Noop 时纯内存审计不受影响。
    #[tokio::test]
    async fn audit_sink_receives_all_entries() {
        #[derive(Debug, Default)]
        struct RecordingSink {
            entries: std::sync::Mutex<Vec<crate::audit::HookAuditEntry>>,
        }
        impl AuditSink for RecordingSink {
            fn push_audit(&self, entry: &crate::audit::HookAuditEntry) {
                self.entries
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(entry.clone());
            }
        }
        // 正常执行 + 沙箱拒绝两条路径
        let mut cfg = HookConfig::default();
        cfg.trust = TrustLevel::Trusted;
        let cmd = resolve("true");
        cfg.hooks.insert(
            LifecycleEvent::PostToolUse,
            vec![HookSpec {
                command: cmd,
                timeout_ms: 2000,
            }],
        );
        let sink = std::sync::Arc::new(RecordingSink::default());
        let ex = test_executor(cfg, &["true"]).with_audit_sink(sink.clone());
        let results = ex
            .trigger(LifecycleEvent::PostToolUse, None, None, None)
            .await;
        assert_eq!(results[0].exit_code, Some(0));
        assert_eq!(
            sink.entries.lock().unwrap().len(),
            1,
            "正常执行必须汇出 1 条"
        );
        assert_eq!(ex.audit().len(), 1, "内存审计同步保留");
        // 逃逸拒绝路径也汇出
        let mut cfg2 = HookConfig::default();
        cfg2.trust = TrustLevel::Trusted;
        let sh = resolve("sh");
        cfg2.hooks.insert(
            LifecycleEvent::PreToolUse,
            vec![HookSpec {
                command: format!("{sh} -c \"echo pwn > /etc/pwned\""),
                timeout_ms: 2000,
            }],
        );
        let ex2 = test_executor(cfg2, &["sh"]).with_audit_sink(sink.clone());
        let results2 = ex2
            .trigger(LifecycleEvent::PreToolUse, None, None, None)
            .await;
        assert!(results2[0].sandbox_denied);
        assert_eq!(sink.entries.lock().unwrap().len(), 2, "沙箱拒绝必须汇出");
        // 默认 Noop:构造不带 sink 不 panic（回退路径）
        let ex3 = test_executor(HookConfig::default(), &["true"]);
        let _ = ex3
            .trigger(LifecycleEvent::PostToolUse, None, None, None)
            .await;
    }

    /// Ask 信任级 fail-closed — 「未确认」不等于「可执行」(Phase 5 安全修复)
    ///
    /// WHY:配置默认 trust=Ask 而 `trigger` 没有审批回调,旧实现把 Ask 放行,
    /// 等于默认挂载即静默运行 hook（子进程任意代码）。现在 Ask 与 Untrusted
    /// 同样拒否且不审计（未执行无审计对象）。
    #[tokio::test]
    async fn ask_is_fail_closed() {
        let mut cfg = HookConfig::default();
        cfg.trust = TrustLevel::Ask;
        cfg.hooks.insert(
            LifecycleEvent::PostToolUse,
            vec![HookSpec {
                command: resolve("true"),
                timeout_ms: 1000,
            }],
        );
        let ex = test_executor(cfg, &["true"]);
        let results = ex
            .trigger(LifecycleEvent::PostToolUse, None, None, None)
            .await;
        assert_eq!(results.len(), 1);
        assert!(results[0].sandbox_denied, "Ask 必须 fail-closed 拒否");
        assert!(ex.audit().is_empty(), "未执行不审计");
    }

    /// PostToolUse 有界并行 — 8 条 0.5s hook,cap=4 ⇒ 两波而非八波
    ///
    /// 断言口径:墙钟只做「串行 ≫ 并行」的方向判定（串行理论 4.0s,阈值 2.5s,
    /// 需 1.6× 劣化才假红）;结构性保证交给下标对齐 + 全量审计（并行不得丢结果、
    /// 不得打乱与 specs 的对应关系）。
    #[tokio::test]
    async fn post_hooks_bounded_parallel() {
        let mut cfg = HookConfig::default();
        cfg.trust = TrustLevel::Trusted;
        let cmd = format!("{} 0.5", resolve("sleep"));
        cfg.hooks.insert(
            LifecycleEvent::PostToolUse,
            (0..8)
                .map(|_| HookSpec {
                    command: cmd.clone(),
                    timeout_ms: 4_000,
                })
                .collect(),
        );
        let ex = test_executor(cfg, &["sleep"]);
        let started = std::time::Instant::now();
        let results = ex
            .trigger_with(
                LifecycleEvent::PostToolUse,
                None,
                None,
                None,
                HookTriggerOpts {
                    parallel_post: true,
                    max_parallel: 4,
                    total_timeout_ms: 20_000,
                },
            )
            .await;
        let elapsed = started.elapsed();
        assert_eq!(results.len(), 8, "并行窗口必须补齐全部 8 条");
        assert!(
            results.iter().all(|r| r.exit_code == Some(0)),
            "并行不得丢弃或改写结果"
        );
        assert!(
            results.iter().all(|r| r.command == cmd),
            "结果须与 specs 下标对齐"
        );
        assert_eq!(ex.audit().len(), 8, "每条各审计一次");
        assert!(
            elapsed < std::time::Duration::from_millis(2_500),
            "串行需 ≥4s,cap=4 两波应远低于此;实测 {elapsed:?}"
        );
    }

    /// Pre 类始终串行保序 — parallel_post 旗不得影响 PreToolUse
    ///
    /// WHY:Pre 的语义是「按挂载顺序逐条把关,任一中断即拒否」;若并行则完成顺序
    /// 倒置,调用方会看到与配置不一致的拒否点。三条命令耗时刻意递减错位
    /// （0.3s / 即时 / 0.1s）,一旦并行,下标必然被打乱。
    #[tokio::test]
    async fn pre_hooks_keep_order() {
        let mut cfg = HookConfig::default();
        cfg.trust = TrustLevel::Trusted;
        let slow = format!("{} 0.3", resolve("sleep"));
        let fast = format!("{} 0.1", resolve("sleep"));
        let ok = resolve("true");
        let expected: [String; 3] = [slow.clone(), ok.clone(), fast.clone()];
        cfg.hooks.insert(
            LifecycleEvent::PreToolUse,
            vec![
                HookSpec {
                    command: slow,
                    timeout_ms: 2_000,
                },
                HookSpec {
                    command: ok,
                    timeout_ms: 2_000,
                },
                HookSpec {
                    command: fast,
                    timeout_ms: 2_000,
                },
            ],
        );
        let ex = test_executor(cfg, &["sleep", "true"]);
        let results = ex
            .trigger_with(
                LifecycleEvent::PreToolUse,
                None,
                None,
                None,
                HookTriggerOpts {
                    parallel_post: true,
                    ..HookTriggerOpts::default()
                },
            )
            .await;
        let got: Vec<&str> = results.iter().map(|r| r.command.as_str()).collect();
        assert_eq!(
            got,
            expected.iter().map(String::as_str).collect::<Vec<_>>(),
            "Pre 类必须按挂载顺序返回,parallel_post 不得生效"
        );
    }

    /// 整批总预算 — Pre 超时即 fail-closed 拒否（不逐条等到底）
    ///
    /// 单条 timeout_ms 刻意放大到 5s,让熔断只能来自 total_timeout_ms;
    /// Pre 属可中断事件 ⇒ 超时必须置 interrupted（调用方据此拒绝工具调用）。
    #[tokio::test]
    async fn pre_total_timeout_denies() {
        let mut cfg = HookConfig::default();
        cfg.trust = TrustLevel::Trusted;
        cfg.hooks.insert(
            LifecycleEvent::PreToolUse,
            vec![HookSpec {
                command: format!("{} 2", resolve("sleep")),
                timeout_ms: 5_000,
            }],
        );
        let ex = test_executor(cfg, &["sleep"]);
        let started = std::time::Instant::now();
        let results = ex
            .trigger_with(
                LifecycleEvent::PreToolUse,
                None,
                None,
                None,
                HookTriggerOpts {
                    total_timeout_ms: 200,
                    ..HookTriggerOpts::default()
                },
            )
            .await;
        assert!(
            started.elapsed() < std::time::Duration::from_millis(1_500),
            "总预算到点必须立刻返回"
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].exit_code, None, "超时未取到退出码");
        assert!(
            results[0].interrupted,
            "Pre 类总预算超时必须 fail-closed 置 interrupted"
        );
    }
}
