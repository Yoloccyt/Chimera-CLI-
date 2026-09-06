//! execpolicy — 命令分类规则引擎 + 六模式映射 + 单次提权（P3-T4，v4.0 WI-23）
//!
//! 对应架构层: **L4 Security**（seccore，ADR-147 裁决：D-P8 排期漂移修正——v4.0 标注
//! 「Ⅲ期 W11-13」但全库零命中，纳入 W16）
//! 对应任务: **P3-T4**（手册 W16，WI-23：execpolicy 规则引擎 + 六模式 + SingleUse）
//!
//! # 设计（v4.0 WI-23 规格）
//! (a) **规则引擎**:`pattern → allow/ask/deny`,作用域规则如 `Bash(npm *)`——
//!     program 精确匹配 + args 通配符匹配;
//! (b) **六模式映射**:L0 [`PermissionMode`]（nexus-contracts）→ [`ModePolicy`],
//!     ScopeSpec（writable_patterns / preapproved / isolated）承载模式参数;
//! (c) **单次提权**:[`SingleUseToken`] 消耗型变体（当次生效不常驻）。
//!
//! # 红线
//! `#![forbid(unsafe_code)]` 由 crate 顶层保证;auto 模式默认不启用（fail-closed）;
//! 禁 feature 标志（模式经构造参数表达）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use nexus_contracts::app::PermissionMode;

/// 策略动作 — 三态裁决
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyAction {
    /// 放行
    Allow,
    /// 触发审批（ask 规则命中 → 审批流）
    Ask,
    /// 拒绝
    Deny,
}

impl PolicyAction {
    /// 是否放行（Allow 或 Ask——Ask 由审批流决定最终执行）
    #[must_use]
    pub const fn permits(self) -> bool {
        matches!(self, Self::Allow | Self::Ask)
    }
}

/// 决策审计统计 — 按动作分桶计数（P3-T4 补:全量决策留痕）
///
/// WHY Mutex 而非 Atomic:统计为低频审计路径（决策计数）,Mutex 简单可靠;
/// 锁内无 await（纯 u64 累加）,不违反持锁跨 await 红线。
#[derive(Debug, Default)]
pub struct DecisionStats {
    /// 分桶计数（allow/ask/deny）
    inner: std::sync::Mutex<StatsInner>,
}

/// 分桶内部状态
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct StatsInner {
    /// Allow 计数
    allow: u64,
    /// Ask 计数
    ask: u64,
    /// Deny 计数
    deny: u64,
}

impl Clone for DecisionStats {
    fn clone(&self) -> Self {
        Self {
            inner: std::sync::Mutex::new(
                self.inner.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            ),
        }
    }
}

impl DecisionStats {
    /// 记录一次决策
    pub fn record(&self, action: PolicyAction) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        match action {
            PolicyAction::Allow => g.allow += 1,
            PolicyAction::Ask => g.ask += 1,
            PolicyAction::Deny => g.deny += 1,
        }
    }

    /// Allow 次数
    #[must_use]
    pub fn allow_count(&self) -> u64 {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).allow
    }

    /// Ask 次数
    #[must_use]
    pub fn ask_count(&self) -> u64 {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).ask
    }

    /// Deny 次数
    #[must_use]
    pub fn deny_count(&self) -> u64 {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).deny
    }

    /// 总决策数
    #[must_use]
    pub fn total(&self) -> u64 {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.allow + g.ask + g.deny
    }
}

/// 规则模式 — program 精确 + args 通配（`Bash(npm *)` 作用域规则）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulePattern {
    /// 程序名（小写匹配,大小写不敏感）
    pub program: String,
    /// 参数通配（`*` 匹配任意;`npm *` 匹配任意以 npm 开头的参数序列）
    pub args_glob: String,
}

impl RulePattern {
    /// 新建模式
    #[must_use]
    pub fn new(program: impl Into<String>, args_glob: impl Into<String>) -> Self {
        Self {
            program: program.into().to_lowercase(),
            args_glob: args_glob.into(),
        }
    }

    /// 匹配 — program 精确（小写）+ args 通配
    ///
    /// 空参数特例:`npm *` 应匹配无参调用 `npm`（单独调用也属该系列）
    #[must_use]
    pub fn matches(&self, program: &str, args: &[String]) -> bool {
        if program.to_lowercase() != self.program {
            return false;
        }
        let joined = args.join(" ");
        if joined.is_empty() && self.args_glob.ends_with(" *") {
            return true;
        }
        glob_match(&self.args_glob, &joined)
    }
}

/// 执行策略规则 — pattern → action
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecPolicyRule {
    /// 匹配模式
    pub pattern: RulePattern,
    /// 动作
    pub action: PolicyAction,
}

/// 执行策略引擎 — 规则优先序 + 默认动作
#[derive(Debug, Clone)]
pub struct ExecPolicy {
    /// 规则（先命中先裁决）
    rules: Vec<ExecPolicyRule>,
    /// 无规则命中的默认动作（零信任默认 Deny）
    default_action: PolicyAction,
    /// 决策审计统计（P3-T4 补:全量决策留痕,含 Auto 模式）
    stats: DecisionStats,
}

impl Default for ExecPolicy {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            // 零信任默认:未声明即拒绝（WI-07 同口径:未声明=不可并发+可能写）
            default_action: PolicyAction::Deny,
            stats: DecisionStats::default(),
        }
    }
}

impl ExecPolicy {
    /// 新建策略（零信任默认 Deny）
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加规则（先添加优先——调用方按具体→通用排序）
    pub fn add_rule(mut self, rule: ExecPolicyRule) -> Self {
        self.rules.push(rule);
        self
    }

    /// 评估 — 先命中规则优先,无命中回默认动作（每次决策计入审计统计）
    #[must_use]
    pub fn evaluate(&self, program: &str, args: &[String]) -> PolicyAction {
        let action = self.evaluate_uncounted(program, args);
        self.record_decision(action);
        action
    }

    /// 未计数评估 — 内部辅助（避免重复计数;has_matching_rule 等调用不受影响）
    fn evaluate_uncounted(&self, program: &str, args: &[String]) -> PolicyAction {
        for rule in &self.rules {
            if rule.pattern.matches(program, args) {
                return rule.action;
            }
        }
        self.default_action
    }

    /// 决策审计 — 记录一次决策（全量留痕;Auto 模式经此计入）
    pub fn record_decision(&self, action: PolicyAction) {
        self.stats.record(action);
    }

    /// 决策统计快照（诊断/审计导出）
    #[must_use]
    pub fn decision_stats(&self) -> DecisionStats {
        self.stats.clone()
    }

    /// 是否有规则命中 — 区分「命中 Deny 规则」与「默认 Deny」
    /// （Default 模式:无命中放行,规则负责设防——六模式语义）
    #[must_use]
    pub fn has_matching_rule(&self, program: &str, args: &[String]) -> bool {
        self.rules.iter().any(|r| r.pattern.matches(program, args))
    }

    /// 规则数（诊断）
    #[must_use]
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

/// 作用域规格 — 六模式参数承载（v4.0 WI-23 ScopeSpec）
#[derive(Debug, Clone, Default)]
pub struct ScopeSpec {
    /// acceptEdits 自动批准的写模式（writable_patterns）
    pub writable_patterns: Vec<RulePattern>,
    /// dontAsk 预批准清单（headless 仅此清单放行）
    pub preapproved: Vec<RulePattern>,
    /// bypassPermissions 仅限 isolated 环境（容器/CI）
    pub isolated: bool,
}

/// 模式策略 — L0 PermissionMode 的决策语义映射（六模式）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModePolicy {
    /// 规划模式:全只读 + DryRun 投影（写命令全拒）
    Plan,
    /// 自动批准写（writable_patterns 白名单内 Allow）
    AcceptEdits,
    /// 默认:ask 规则触发审批（规则 Ask → Ask;其余 Allow）
    Default,
    /// 仅预批准清单（headless;清单外 Deny）
    DontAsk,
    /// 分类器裁决（默认不启用——fail-closed Deny,接入方显式注入才生效）
    Auto,
    /// 仅容器/CI（isolated=true 才放行;false → 降级 Default）
    BypassPermissions,
}

impl ModePolicy {
    /// 从 L0 PermissionMode 映射
    #[must_use]
    pub fn from_mode(mode: PermissionMode) -> Self {
        match mode {
            PermissionMode::Plan => Self::Plan,
            PermissionMode::AcceptEdits => Self::AcceptEdits,
            PermissionMode::Default => Self::Default,
            PermissionMode::DontAsk => Self::DontAsk,
            PermissionMode::Auto => Self::Auto,
            PermissionMode::BypassPermissions => Self::BypassPermissions,
        }
    }

    /// 模式决策 — 综合策略引擎 + 作用域 + 环境
    ///
    /// # 参数
    /// - `policy`:规则引擎（Default 模式用）
    /// - `spec`:作用域参数
    /// - `is_container`:bypassPermissions 环境判定
    #[must_use]
    pub fn evaluate(
        self,
        policy: &ExecPolicy,
        spec: &ScopeSpec,
        program: &str,
        args: &[String],
        is_container: bool,
    ) -> PolicyAction {
        match self {
            // 全只读:写命令（非只读程序）全拒;只读程序放行（DryRun 投影由调用方记录）
            ModePolicy::Plan => {
                if is_readonly_program(program) {
                    PolicyAction::Allow
                } else {
                    PolicyAction::Deny
                }
            }
            // writable_patterns 内写命令自动批准;其余走默认规则
            ModePolicy::AcceptEdits => {
                if spec
                    .writable_patterns
                    .iter()
                    .any(|p| p.matches(program, args))
                {
                    PolicyAction::Allow
                } else {
                    policy.evaluate(program, args)
                }
            }
            // ask 规则触发审批;无规则命中 Allow（默认放行,规则负责设防）
            ModePolicy::Default => match policy.evaluate(program, args) {
                PolicyAction::Ask => PolicyAction::Ask,
                PolicyAction::Deny if policy.has_matching_rule(program, args) => PolicyAction::Deny,
                PolicyAction::Deny => PolicyAction::Allow,
                PolicyAction::Allow => PolicyAction::Allow,
            },
            // 仅预批准清单:清单外 Deny（headless fail-closed）
            ModePolicy::DontAsk => {
                if spec.preapproved.iter().any(|p| p.matches(program, args)) {
                    PolicyAction::Allow
                } else {
                    PolicyAction::Deny
                }
            }
            // 分类器裁决:默认不启用（fail-closed Deny）——接入方注入分类器后替换;
            // 决策计入审计统计（P3-T4:全量决策留痕）
            ModePolicy::Auto => {
                policy.record_decision(PolicyAction::Deny);
                PolicyAction::Deny
            }
            // 仅容器/CI:isolated=false 降级 Default（deny 语义经规则引擎表达）
            ModePolicy::BypassPermissions => {
                if spec.isolated && is_container {
                    PolicyAction::Allow
                } else {
                    policy.evaluate(program, args)
                }
            }
        }
    }
}

/// 只读程序白名单（Plan 模式:全只读）
const READONLY_PROGRAMS: [&str; 10] = [
    "ls", "cat", "pwd", "whoami", "date", "head", "tail", "wc", "grep", "find",
];

/// 只读程序判定（Plan 模式用;未识别默认非只读 → 拒绝,保守）
#[must_use]
fn is_readonly_program(program: &str) -> bool {
    READONLY_PROGRAMS.contains(&program.to_lowercase().as_str())
}

/// 通配符匹配 — `*` 匹配任意序列（零个或多个字符）
///
/// 简化 glob:仅支持 `*`;`npm *` 匹配 "npm" 或 "npm install" 等任意参数序列。
/// 实现为迭代 DP（O(n·m),规则匹配低频,性能非关键路径）。
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (n, m) = (p.len(), t.len());
    // dp[i][j]: pattern[..i] 是否匹配 text[..j]
    let mut dp = vec![vec![false; m + 1]; n + 1];
    dp[0][0] = true;
    for i in 1..=n {
        if p[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }
    for i in 1..=n {
        for j in 1..=m {
            if p[i - 1] == '*' {
                dp[i][j] = dp[i - 1][j] || dp[i][j - 1];
            } else if p[i - 1] == t[j - 1] {
                dp[i][j] = dp[i - 1][j - 1];
            }
        }
    }
    dp[n][m]
}

/// 单次提权令牌 — 消耗型（当次生效不常驻,WI-23 (c)）
#[derive(Debug)]
pub struct SingleUseToken {
    /// 令牌 ID（审计追溯）
    id: String,
    /// 过期时刻
    expiry: Instant,
    /// 是否已消耗（原子,并发安全）
    used: AtomicBool,
}

impl SingleUseToken {
    /// 新建令牌（TTL 后自动失效）
    #[must_use]
    pub fn new(id: impl Into<String>, ttl_ms: u64) -> Self {
        Self {
            id: id.into(),
            expiry: Instant::now() + std::time::Duration::from_millis(ttl_ms),
            used: AtomicBool::new(false),
        }
    }

    /// 消耗 — 首次成功（返回 true）,二次/过期均失败（false）
    #[must_use]
    pub fn consume(&self) -> bool {
        if self.expiry <= Instant::now() {
            return false;
        }
        // 先占先得:唯一成功者 fetch 到 false 的旧值
        !self.used.swap(true, Ordering::AcqRel)
    }

    /// 是否有效（未消耗且未过期）
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.used.load(Ordering::Acquire) && self.expiry > Instant::now()
    }

    /// 令牌 ID（审计）
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// glob 匹配 — `*` 通配语义（纯函数严格语义;空参数特例在 RulePattern 层）
    #[test]
    fn glob_matches() {
        assert!(glob_match("npm *", "npm install"));
        // 严格语义:"npm *" 需 "npm " 前缀 + 任意;"npm"（无空格）不匹配
        assert!(!glob_match("npm *", "npm"));
        assert!(glob_match("*", ""));
        assert!(glob_match("*", "anything"));
        assert!(!glob_match("npm *", "npx install"));
        assert!(!glob_match("git *", "npm install"));
        assert!(glob_match("git status", "git status"));
    }

    /// RulePattern — program 精确（大小写不敏感）+ args 通配
    #[test]
    fn rule_pattern_matches() {
        let p = RulePattern::new("Bash", "npm *");
        assert!(p.matches("bash", &["npm".into(), "install".into()]));
        assert!(p.matches("bash", &[]), "空 args 也属 npm 系列");
        assert!(!p.matches("sh", &["npm".into(), "install".into()]));
        assert!(!p.matches("bash", &["npx".into(), "install".into()]));
    }

    /// 规则优先序 — 先添加先裁决;无命中回默认 Deny（零信任）
    #[test]
    fn rule_priority_and_default() {
        let policy = ExecPolicy::new()
            .add_rule(ExecPolicyRule {
                pattern: RulePattern::new("bash", "npm *"),
                action: PolicyAction::Allow,
            })
            .add_rule(ExecPolicyRule {
                pattern: RulePattern::new("bash", "rm *"),
                action: PolicyAction::Deny,
            });
        // npm 命中 Allow
        assert_eq!(
            policy.evaluate("bash", &["npm".into(), "i".into()]),
            PolicyAction::Allow
        );
        // rm 命中 Deny
        assert_eq!(
            policy.evaluate("bash", &["rm".into(), "-rf".into(), "/".into()]),
            PolicyAction::Deny
        );
        // 无命中 → 默认 Deny
        assert_eq!(
            policy.evaluate("git", &["status".into()]),
            PolicyAction::Deny
        );
        // 空规则策略全拒
        let empty = ExecPolicy::new();
        assert_eq!(empty.evaluate("echo", &[]), PolicyAction::Deny);
    }

    /// 六模式场景矩阵 — Plan/AcceptEdits/Default/DontAsk/Auto/BypassPermissions
    #[test]
    fn six_mode_matrix() {
        let policy = ExecPolicy::new().add_rule(ExecPolicyRule {
            pattern: RulePattern::new("bash", "npm *"),
            action: PolicyAction::Allow,
        });
        let spec = ScopeSpec {
            writable_patterns: vec![RulePattern::new("bash", "git add *")],
            preapproved: vec![RulePattern::new("bash", "git status")],
            isolated: true,
        };
        // Plan:只读程序放行,写程序全拒
        assert_eq!(
            ModePolicy::Plan.evaluate(&policy, &spec, "ls", &[], false),
            PolicyAction::Allow
        );
        assert_eq!(
            ModePolicy::Plan.evaluate(&policy, &spec, "bash", &["npm".into(), "i".into()], false),
            PolicyAction::Deny
        );
        // AcceptEdits:writable_patterns 自动批准;其余走规则
        assert_eq!(
            ModePolicy::AcceptEdits.evaluate(
                &policy,
                &spec,
                "bash",
                &["git".into(), "add".into(), "x".into()],
                false
            ),
            PolicyAction::Allow
        );
        assert_eq!(
            ModePolicy::AcceptEdits.evaluate(
                &policy,
                &spec,
                "bash",
                &["npm".into(), "i".into()],
                false
            ),
            PolicyAction::Allow
        );
        // Default:ask 规则触发审批;无命中 Allow（此处无 ask 规则 → Allow）
        assert_eq!(
            ModePolicy::Default.evaluate(&policy, &spec, "echo", &["hi".into()], false),
            PolicyAction::Allow
        );
        // DontAsk:仅 preapproved
        assert_eq!(
            ModePolicy::DontAsk.evaluate(
                &policy,
                &spec,
                "bash",
                &["git".into(), "status".into()],
                false
            ),
            PolicyAction::Allow
        );
        assert_eq!(
            ModePolicy::DontAsk.evaluate(&policy, &spec, "echo", &["hi".into()], false),
            PolicyAction::Deny
        );
        // Auto:默认不启用（fail-closed）
        assert_eq!(
            ModePolicy::Auto.evaluate(&policy, &spec, "echo", &["hi".into()], false),
            PolicyAction::Deny
        );
        // BypassPermissions:isolated + 容器才放行
        assert_eq!(
            ModePolicy::BypassPermissions.evaluate(&policy, &spec, "echo", &["hi".into()], true),
            PolicyAction::Allow
        );
        assert_eq!(
            ModePolicy::BypassPermissions.evaluate(&policy, &spec, "echo", &["hi".into()], false),
            PolicyAction::Deny,
            "非容器必须降级拒绝"
        );
    }

    /// Default 模式 ask 规则 — 规则 Ask → Ask;无规则命中 → Allow
    #[test]
    fn default_mode_ask_rule() {
        let policy = ExecPolicy::new().add_rule(ExecPolicyRule {
            pattern: RulePattern::new("bash", "git push *"),
            action: PolicyAction::Ask,
        });
        let spec = ScopeSpec::default();
        assert_eq!(
            ModePolicy::Default.evaluate(
                &policy,
                &spec,
                "bash",
                &["git".into(), "push".into(), "origin".into()],
                false
            ),
            PolicyAction::Ask
        );
        assert!(PolicyAction::Ask.permits(), "Ask 需经审批流,permits=true");
        // 无规则命中 → Allow（默认放行,规则负责设防）
        assert_eq!(
            ModePolicy::Default.evaluate(&policy, &spec, "echo", &["hi".into()], false),
            PolicyAction::Allow
        );
    }

    /// PermissionMode 映射 — 六模式全覆盖
    #[test]
    fn mode_mapping_complete() {
        assert_eq!(
            ModePolicy::from_mode(PermissionMode::Plan),
            ModePolicy::Plan
        );
        assert_eq!(
            ModePolicy::from_mode(PermissionMode::AcceptEdits),
            ModePolicy::AcceptEdits
        );
        assert_eq!(
            ModePolicy::from_mode(PermissionMode::Default),
            ModePolicy::Default
        );
        assert_eq!(
            ModePolicy::from_mode(PermissionMode::DontAsk),
            ModePolicy::DontAsk
        );
        assert_eq!(
            ModePolicy::from_mode(PermissionMode::Auto),
            ModePolicy::Auto
        );
        assert_eq!(
            ModePolicy::from_mode(PermissionMode::BypassPermissions),
            ModePolicy::BypassPermissions
        );
    }

    /// 决策审计 — evaluate 全量留痕 + Auto 模式计入（P3-T4 补）
    #[test]
    fn decision_audit_stats() {
        let policy = ExecPolicy::new().add_rule(ExecPolicyRule {
            pattern: RulePattern::new("bash", "npm *"),
            action: PolicyAction::Allow,
        });
        // evaluate ×3: Allow(npm) + Deny(默认 git) + Deny(默认 echo)
        assert_eq!(
            policy.evaluate("bash", &["npm".into(), "i".into()]),
            PolicyAction::Allow
        );
        assert_eq!(
            policy.evaluate("git", &["status".into()]),
            PolicyAction::Deny
        );
        assert_eq!(policy.evaluate("echo", &["hi".into()]), PolicyAction::Deny);
        let stats = policy.decision_stats();
        assert_eq!(stats.allow_count(), 1);
        assert_eq!(stats.deny_count(), 2);
        assert_eq!(stats.total(), 3);
        // Auto 模式:fail-closed Deny 且计入审计
        let spec = ScopeSpec::default();
        let auto_outcome = ModePolicy::Auto.evaluate(&policy, &spec, "echo", &["hi".into()], false);
        assert_eq!(auto_outcome, PolicyAction::Deny);
        assert_eq!(policy.decision_stats().deny_count(), 3, "Auto 决策必须留痕");
        // has_matching_rule 不计数（纯查询）
        assert!(policy.has_matching_rule("bash", &["npm".into(), "i".into()]));
        assert_eq!(policy.decision_stats().total(), 4, "查询不计数");
    }

    /// SingleUseToken — 单次消耗断言（并发安全）;过期失效
    #[test]
    fn single_use_token() {
        let t = SingleUseToken::new("t1", 60_000);
        assert!(t.is_valid());
        assert!(t.consume(), "首次消耗必须成功");
        assert!(!t.consume(), "二次消耗必须失败");
        assert!(!t.is_valid(), "消耗后无效");
        // 并发:8 线程竞争,恰 1 成功
        let t2 = std::sync::Arc::new(SingleUseToken::new("t2", 60_000));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let t2 = std::sync::Arc::clone(&t2);
                std::thread::spawn(move || t2.consume())
            })
            .collect();
        let wins = handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .filter(|ok| *ok)
            .count();
        assert_eq!(wins, 1, "并发竞争必须恰 1 次成功");
        // 过期令牌:0ms TTL → 立即失效
        let t3 = SingleUseToken::new("t3", 0);
        assert!(!t3.consume(), "0ms TTL 必须立即失效");
    }
}
