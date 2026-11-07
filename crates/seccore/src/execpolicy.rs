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
/// WHY 原为 Mutex:统计曾被视为低频审计路径;R1-4(ADR-260)改判——record 在
/// 每次派发的**双重策略评估**热路径上（runtime.rs 防御性设计）,并发批量下
/// 同一 `Arc<ExecPolicy>` 的统计锁是共享争用点（batch.rs 自陈选 8 并发即为
/// 避它）。三桶独立单调 u64 → 原子加无锁化,临界区从「锁 + 三次写」降为
/// 一次 fetch_add。
///
/// 快照语义取舍（ADR-260 §3 登记）:并发下 `allow/ask/deny` 逐桶 load 组成
/// 的快照是近似瞬时值（不要求跨桶原子一致）——审计场景只要求各桶单调收敛,
/// 与 Mutex 时代的强一致快照相比无消费者受损（全仓快照读数仅诊断/测试面）。
#[derive(Debug, Default)]
pub struct DecisionStats {
    /// Allow 计数
    allow: std::sync::atomic::AtomicU64,
    /// Ask 计数
    ask: std::sync::atomic::AtomicU64,
    /// Deny 计数
    deny: std::sync::atomic::AtomicU64,
}

impl Clone for DecisionStats {
    fn clone(&self) -> Self {
        use std::sync::atomic::Ordering::Relaxed;
        Self {
            allow: std::sync::atomic::AtomicU64::new(self.allow.load(Relaxed)),
            ask: std::sync::atomic::AtomicU64::new(self.ask.load(Relaxed)),
            deny: std::sync::atomic::AtomicU64::new(self.deny.load(Relaxed)),
        }
    }
}

impl DecisionStats {
    /// 记录一次决策
    pub fn record(&self, action: PolicyAction) {
        use std::sync::atomic::Ordering::Relaxed;
        let counter = match action {
            PolicyAction::Allow => &self.allow,
            PolicyAction::Ask => &self.ask,
            PolicyAction::Deny => &self.deny,
        };
        counter.fetch_add(1, Relaxed);
    }

    /// Allow 次数
    #[must_use]
    pub fn allow_count(&self) -> u64 {
        self.allow.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Ask 次数
    #[must_use]
    pub fn ask_count(&self) -> u64 {
        self.ask.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Deny 次数
    #[must_use]
    pub fn deny_count(&self) -> u64 {
        self.deny.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 总决策数
    #[must_use]
    pub fn total(&self) -> u64 {
        self.allow_count() + self.ask_count() + self.deny_count()
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

/// 拼接参数序列为空格分隔的单串（与 `&[String]::join(" ")` 语义逐位一致）
///
/// WHY 手写而非 `map+collect+join`：单次分配（collect 会多一次 Vec 分配）。
/// W4 泛型化判决核心的共用辅助——拥有型 `&[String]` 与借用型 `&[&str]`
/// 走同一实现，匹配形状分叉即判决分叉（安全语义）。
fn join_args<S: AsRef<str>>(args: &[S]) -> String {
    let mut joined = String::new();
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            joined.push(' ');
        }
        joined.push_str(arg.as_ref());
    }
    joined
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
    ///
    /// W4 泛型化：拥有型 `&[String]` 与借用型 `&[&str]` 共用同一实现
    /// （`String`/`&str` 均 `AsRef<str>`），判决逐位一致。
    /// R1-2(ADR-260)：公共入口归一输入后走 [`Self::matches_normalized`];
    /// 热路径(evaluate/索引试探)应直接调后者,避免每规则重复归一分配。
    #[must_use]
    pub fn matches<S: AsRef<str>>(&self, program: &str, args: &[S]) -> bool {
        self.matches_normalized(&program.to_lowercase(), args)
    }

    /// 匹配（已归一 program）— `program_lower` 必须已是小写形态;
    /// 热路径专用:归一由调用方单次完成,规则试探路径零重复分配。
    ///
    /// 判决体与 [`Self::matches`] 单源（安全语义:形状分叉即判决分叉）。
    #[must_use]
    pub fn matches_normalized<S: AsRef<str>>(&self, program_lower: &str, args: &[S]) -> bool {
        if program_lower != self.program {
            return false;
        }
        // R1-2 补充(ADR-260):`*` 通配一切(join_args 结果与 DP 均不影响判决),
        // 短路消掉主力白名单规则形态的 join_args 分配;与下方空参特例同族。
        if self.args_glob == "*" {
            return true;
        }
        let joined = join_args(args);
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
    /// 规则（先命中先裁决;顺序与统计的权威存储）
    rules: Vec<ExecPolicyRule>,
    /// R1-1(ADR-260):program(小写) → 规则下标桶,`add_rule` 构造期维护。
    /// WHY 语义安全:program 匹配是**精确**小写比较,跨 program 规则互斥 ⇒
    /// 命中只可能来自同 program 规则;桶内保持先添加序 = 全局先匹配序,
    /// 索引查找与线性扫描判决逐位等价(索引等价 proptest 锁定)。
    /// 非 `RulePattern::new` 构造的直接字面量 program(未小写)在旧线性扫描下
    /// 本就永不命中(输入被归一为小写),按小写分桶后同样永不命中,语义一致。
    index: std::collections::HashMap<Box<str>, Vec<u32>>,
    /// 无规则命中的默认动作（零信任默认 Deny）
    default_action: PolicyAction,
    /// 决策审计统计（P3-T4 补:全量决策留痕,含 Auto 模式）
    stats: DecisionStats,
}

impl Default for ExecPolicy {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            index: std::collections::HashMap::new(),
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

    /// 新建策略并指定「无规则命中时」的默认动作
    ///
    /// WHY 需要这个构造器：零信任默认（`Deny`）适用于「声明式白名单」语义 ——
    /// 调用方必须逐条列出允许的工具，未声明即拒绝。而「默认放行 + 规则设防」
    /// 语义（规则负责拦截危险工具，其余放行）无法用 `new()` + `add_rule()` 表达：
    /// 后者只能加规则，无法把「未命中」从 `Deny` 改为 `Allow`。
    ///
    /// 二者都合法，取决于部署侧选择哪种心智模型。本构造器只暴露选择权，
    /// **不改变** `new()` 的零信任默认值 —— 默认值的安全性不变。
    ///
    /// # 示例
    /// ```
    /// use seccore::execpolicy::{ExecPolicy, PolicyAction};
    /// // 默认放行，但显式拒绝 rm
    /// let policy = ExecPolicy::with_default_action(PolicyAction::Allow);
    /// // W4 泛型化后空切片须消歧（evaluate<S: AsRef<str>> 多 impl 候选）
    /// assert_eq!(policy.evaluate("rm", &[] as &[String]), PolicyAction::Allow);
    /// ```
    #[must_use]
    pub fn with_default_action(default_action: PolicyAction) -> Self {
        Self {
            rules: Vec::new(),
            index: std::collections::HashMap::new(),
            default_action,
            stats: DecisionStats::default(),
        }
    }

    /// 添加规则（先添加优先——调用方按具体→通用排序）
    pub fn add_rule(mut self, rule: ExecPolicyRule) -> Self {
        // R1-1:构造期维护 program → 规则下标索引（键统一小写,与判决归一
        // 口径一致;O(R) 线性扫描成本一次性前移到规则构造期）
        let key: Box<str> = rule.pattern.program.to_lowercase().into_boxed_str();
        let idx = self.rules.len() as u32;
        self.index.entry(key).or_default().push(idx);
        self.rules.push(rule);
        self
    }

    /// 评估 — 先命中规则优先,无命中回默认动作（每次决策计入审计统计）
    ///
    /// W4 泛型化：拥有型 `&[String]` 与借用型 `&[&str]` 共用同一判决体
    /// （`RulePattern::matches` 同步泛型化），判决逐位一致。
    #[must_use]
    pub fn evaluate<S: AsRef<str>>(&self, program: &str, args: &[S]) -> PolicyAction {
        let action = self.evaluate_uncounted(program, args);
        self.record_decision(action);
        action
    }

    /// 借用评估入口 — 语义与 [`Self::evaluate`] 逐位一致（含决策计数）
    ///
    /// W4（ADR-249）：链内策略层与 `ExecPolicyToolExecutor` 的借用路径
    /// 调本入口，避免 `args_json.to_string()` 的入参升级分配。存在意义是
    /// **命名意图**（借用路径的显式签名），判决体与 `evaluate` 单源。
    #[must_use]
    pub fn evaluate_ref(&self, program: &str, args: &[&str]) -> PolicyAction {
        self.evaluate(program, args)
    }

    /// 索引定位首个命中规则（R1-1/R1-2,ADR-260）— 判决唯一入口。
    ///
    /// O(R) 线性扫描 → program 索引 O(1) 定位 + 同 program 桶内扫描;
    /// program 归一单次化:全 ASCII 且无大写 ⇒ 已是规范小写,零分配借用
    /// 视图直传;否则一次 `to_lowercase` 复用（旧实现对**每条规则**试探
    /// 各做一次 `to_lowercase`,2k 次分配/派发）。
    fn find_matching<'a, S: AsRef<str>>(
        &'a self,
        program: &str,
        args: &[S],
    ) -> Option<&'a ExecPolicyRule> {
        let lower_owned;
        let program_lower: &str =
            if program.is_ascii() && !program.bytes().any(|b| b.is_ascii_uppercase()) {
                program
            } else {
                lower_owned = program.to_lowercase();
                &lower_owned
            };
        let bucket = self.index.get(program_lower)?;
        for &idx in bucket {
            let rule = &self.rules[idx as usize];
            if rule.pattern.matches_normalized(program_lower, args) {
                return Some(rule);
            }
        }
        None
    }

    /// 未计数评估 — 内部辅助（避免重复计数;has_matching_rule 等调用不受影响）
    fn evaluate_uncounted<S: AsRef<str>>(&self, program: &str, args: &[S]) -> PolicyAction {
        match self.find_matching(program, args) {
            Some(rule) => rule.action,
            None => self.default_action,
        }
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
        // R1-1:与 evaluate_uncounted 同走索引判定（语义与线性 any 逐位等价）
        self.find_matching(program, args).is_some()
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
///
/// R1-3(ADR-260):迭代式双指针回溯,O(n·m) 时间 **O(1) 空间零堆分配**——
/// 替换原「Vec\<char\>×2 + n·m bool DP 表」三重分配。原注释「性能非关键路径」
/// 已过时:判决体在每派发双重评估热路径上（runtime.rs 防御性设计）。
/// 字节级比较安全性:`*`(0x2A) 为唯一元字符,UTF-8 多字节序列不含 ASCII
/// 字节 ⇒ 字节相等 ⇔ 字符相等,逐字节试探与按字符试探判决一致。
/// 与原 DP 实现逐位等价,由 `glob_zero_alloc_matches_dp_reference` proptest
/// 对拍锁定(参考实现保留于 tests 模块)。
fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut pi, mut ti) = (0usize, 0usize);
    // 最近一次 `*` 的 pattern 下标与其当前试探的 text 起点(usize::MAX=尚未见过 `*`)
    let (mut star_p, mut star_t) = (usize::MAX, 0usize);
    while ti < t.len() {
        match p.get(pi).copied() {
            Some(b'*') => {
                star_p = pi;
                star_t = ti;
                pi += 1;
            }
            Some(c) if c == t[ti] => {
                pi += 1;
                ti += 1;
            }
            _ => {
                if star_p != usize::MAX {
                    // 回溯:让最近一个 `*` 多吞一个字符,从其后重新试探
                    star_t += 1;
                    ti = star_t;
                    pi = star_p + 1;
                } else {
                    return false;
                }
            }
        }
    }
    // 文本耗尽:pattern 剩余部分必须全是 `*`(可匹配空)
    while p.get(pi) == Some(&b'*') {
        pi += 1;
    }
    pi == p.len()
}

/// glob 参考实现（原迭代 DP,O(n·m) 时间 O(n·m) 空间,三重堆分配）
///
/// WHY 保留于 tests:零分配重写与原实现的逐位等价由 proptest 对拍锁定,
/// 参考体即等价性的真值源;生产路径不再触达。
#[cfg(test)]
fn glob_match_dp_reference(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (n, m) = (p.len(), t.len());
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
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
        assert!(p.matches("bash", &["npm".to_string(), "install".to_string()]));
        assert!(p.matches("bash", &[] as &[String]), "空 args 也属 npm 系列");
        assert!(!p.matches("sh", &["npm".to_string(), "install".to_string()]));
        assert!(!p.matches("bash", &["npx".to_string(), "install".to_string()]));
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
            policy.evaluate("bash", &["npm".to_string(), "i".to_string()]),
            PolicyAction::Allow
        );
        // rm 命中 Deny
        assert_eq!(
            policy.evaluate(
                "bash",
                &["rm".to_string(), "-rf".to_string(), "/".to_string()]
            ),
            PolicyAction::Deny
        );
        // 无命中 → 默认 Deny
        assert_eq!(
            policy.evaluate("git", &["status".to_string()]),
            PolicyAction::Deny
        );
        // 空规则策略全拒
        let empty = ExecPolicy::new();
        assert_eq!(empty.evaluate("echo", &[] as &[String]), PolicyAction::Deny);
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
            ModePolicy::Plan.evaluate(
                &policy,
                &spec,
                "bash",
                &["npm".to_string(), "i".to_string()],
                false
            ),
            PolicyAction::Deny
        );
        // AcceptEdits:writable_patterns 自动批准;其余走规则
        assert_eq!(
            ModePolicy::AcceptEdits.evaluate(
                &policy,
                &spec,
                "bash",
                &["git".to_string(), "add".to_string(), "x".to_string()],
                false
            ),
            PolicyAction::Allow
        );
        assert_eq!(
            ModePolicy::AcceptEdits.evaluate(
                &policy,
                &spec,
                "bash",
                &["npm".to_string(), "i".to_string()],
                false
            ),
            PolicyAction::Allow
        );
        // Default:ask 规则触发审批;无命中 Allow（此处无 ask 规则 → Allow）
        assert_eq!(
            ModePolicy::Default.evaluate(&policy, &spec, "echo", &["hi".to_string()], false),
            PolicyAction::Allow
        );
        // DontAsk:仅 preapproved
        assert_eq!(
            ModePolicy::DontAsk.evaluate(
                &policy,
                &spec,
                "bash",
                &["git".to_string(), "status".to_string()],
                false
            ),
            PolicyAction::Allow
        );
        assert_eq!(
            ModePolicy::DontAsk.evaluate(&policy, &spec, "echo", &["hi".to_string()], false),
            PolicyAction::Deny
        );
        // Auto:默认不启用（fail-closed）
        assert_eq!(
            ModePolicy::Auto.evaluate(&policy, &spec, "echo", &["hi".to_string()], false),
            PolicyAction::Deny
        );
        // BypassPermissions:isolated + 容器才放行
        assert_eq!(
            ModePolicy::BypassPermissions.evaluate(
                &policy,
                &spec,
                "echo",
                &["hi".to_string()],
                true
            ),
            PolicyAction::Allow
        );
        assert_eq!(
            ModePolicy::BypassPermissions.evaluate(
                &policy,
                &spec,
                "echo",
                &["hi".to_string()],
                false
            ),
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
                &["git".to_string(), "push".to_string(), "origin".to_string()],
                false
            ),
            PolicyAction::Ask
        );
        assert!(PolicyAction::Ask.permits(), "Ask 需经审批流,permits=true");
        // 无规则命中 → Allow（默认放行,规则负责设防）
        assert_eq!(
            ModePolicy::Default.evaluate(&policy, &spec, "echo", &["hi".to_string()], false),
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
            policy.evaluate("bash", &["npm".to_string(), "i".to_string()]),
            PolicyAction::Allow
        );
        assert_eq!(
            policy.evaluate("git", &["status".to_string()]),
            PolicyAction::Deny
        );
        assert_eq!(
            policy.evaluate("echo", &["hi".to_string()]),
            PolicyAction::Deny
        );
        let stats = policy.decision_stats();
        assert_eq!(stats.allow_count(), 1);
        assert_eq!(stats.deny_count(), 2);
        assert_eq!(stats.total(), 3);
        // Auto 模式:fail-closed Deny 且计入审计
        let spec = ScopeSpec::default();
        let auto_outcome =
            ModePolicy::Auto.evaluate(&policy, &spec, "echo", &["hi".to_string()], false);
        assert_eq!(auto_outcome, PolicyAction::Deny);
        assert_eq!(policy.decision_stats().deny_count(), 3, "Auto 决策必须留痕");
        // has_matching_rule 不计数（纯查询）
        assert!(policy.has_matching_rule("bash", &["npm".to_string(), "i".to_string()]));
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
/// W4-T2(ADR-249):evaluate(&[String]) 与 evaluate_ref(&[&str]) 判决逐位一致;
/// RulePattern::matches 泛型化后双形态一致——借用路径不得成为判决分叉面。
#[test]
fn evaluate_ref_matches_owned_verdict_bitwise() {
    let policy = ExecPolicy::new()
        .add_rule(ExecPolicyRule {
            pattern: RulePattern::new("rm", "*"),
            action: PolicyAction::Deny,
        })
        .add_rule(ExecPolicyRule {
            pattern: RulePattern::new("git", "push*"),
            action: PolicyAction::Ask,
        });
    // 遍历程序名 × 参数形态的代表性笛卡尔积(含大小写/空参数/多参数边界)
    let programs = ["rm", "RM", "git", "sh", ""];
    let owned_sets: Vec<Vec<String>> = vec![
        vec![],
        vec!["-rf /".to_string()],
        vec!["push".to_string(), "--force".to_string()],
        vec!["status".to_string()],
    ];
    let ref_sets: Vec<Vec<&str>> = vec![
        vec![],
        vec!["-rf /"],
        vec!["push", "--force"],
        vec!["status"],
    ];
    for program in programs {
        for (owned, refs) in owned_sets.iter().zip(ref_sets.iter()) {
            let owned_action = policy.evaluate(program, owned);
            let ref_action = policy.evaluate_ref(program, refs);
            assert_eq!(
                owned_action, ref_action,
                "双形态判决必须逐位一致: program={program} owned={owned:?} refs={refs:?}"
            );
        }
    }
    // RulePattern::matches 双形态直测
    let pattern = RulePattern::new("git", "push*");
    assert!(pattern.matches("git", &["push".to_string()]));
    assert!(pattern.matches("git", &["push"])); // 借用形态(&[&str])走同一泛型体
    assert!(!pattern.matches("git", &["pull".to_string()]));
    assert!(!pattern.matches("git", &["pull"]));
}

/// R1 等价性 proptest（ADR-260）——索引判决 vs 线性参考、零分配 glob vs DP 参考。
/// WHY 独立 cfg(test) mod:proptest 是 dev-dependency,顶层裸 proptest! 会进入
/// 非测试构建导致 dev-dep 缺失编译失败。
#[cfg(test)]
mod r1_equivalence_proptests {
    use super::*;
    use proptest::prelude::*;

    /// glob 对拍字母表策略:三字母 + 元字符 `*` + 空格,长度 0..24
    /// (覆盖空串/纯 `*`/连续 `*`/尾部 `*`/无 `*` 全形态)
    fn glob_str() -> impl Strategy<Value = String> {
        prop::collection::vec(
            prop_oneof![Just('a'), Just('b'), Just('c'), Just('*'), Just(' ')],
            0..24,
        )
        .prop_map(|chars| chars.into_iter().collect())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

    /// R1-1(ADR-260)索引等价锁:program 索引判定与线性扫描参考判决逐位一致。
    ///
    /// 域覆盖:程序名桶(大小写混杂含未登记名)、通配形态(全 `*`/前缀 `*`/
    /// 含参精确)、参数个数(空/单/多)——桶内先添加序 = 全局先匹配序的
    /// 等价性论证(ADR-260 §2)由本对拍兜底为机器事实。
    #[test]
    fn index_verdict_matches_linear_reference(
        programs in proptest::prelude::prop::collection::vec(
            proptest::prelude::prop_oneof![
                Just("rm".to_string()), Just("RM".to_string()), Just("Git".to_string()),
                Just("git".to_string()), Just("npm".to_string()), Just("nope".to_string()),
            ],
            0..8,
        ),
        globs in proptest::prelude::prop::collection::vec(
            proptest::prelude::prop_oneof![
                Just("*".to_string()), Just("push*".to_string()),
                Just("install *".to_string()), Just("-rf *".to_string()),
            ],
            0..8,
        ),
        query_program in "rm|RM|git|GIT|npm|sh|nope",
        query_args in proptest::prelude::prop::collection::vec(
            proptest::prelude::prop_oneof![
                Just("push".to_string()), Just("install".to_string()),
                Just("-rf".to_string()), Just("status".to_string()),
            ],
            0..3,
        ),
    ) {
        use PolicyAction::{Allow, Ask, Deny};
        let actions = [Allow, Ask, Deny];
        let mut policy = ExecPolicy::with_default_action(Deny);
        let mut reference: Vec<(String, String, PolicyAction)> = Vec::new();
        for (i, prog) in programs.iter().enumerate() {
            let g = if globs.is_empty() {
                "*".to_string()
            } else {
                globs[i % globs.len()].clone()
            };
            policy = policy.add_rule(ExecPolicyRule {
                pattern: RulePattern::new(prog.clone(), g.clone()),
                action: actions[i % actions.len()],
            });
            // 线性参考:与旧 evaluate_uncounted 相同的逐条试探(输入归一小写)
            reference.push((prog.to_lowercase(), g, actions[i % actions.len()]));
        }
        let optimized = policy.evaluate(&query_program, &query_args);
        let query_lower = query_program.to_lowercase();
        let expected = reference
            .iter()
            .find(|(p, g, _)| {
                if *p != query_lower {
                    return false;
                }
                let joined = query_args.join(" ");
                if joined.is_empty() && g.ends_with(" *") {
                    return true;
                }
                glob_match_dp_reference(g, &joined)
            })
            .map_or(Deny, |(_, _, a)| *a);
        prop_assert_eq!(optimized, expected, "索引判决偏离线性参考: query={:?}", query_program);
    }

    /// R1-3(ADR-260)零分配 glob 与原 DP 参考实现对拍(唯一元字符 `*`)。
    #[test]
    fn glob_zero_alloc_matches_dp_reference(
        pattern in glob_str(),
        text in glob_str(),
    ) {
        prop_assert_eq!(
            glob_match(&pattern, &text),
            glob_match_dp_reference(&pattern, &text),
            "glob 判决分叉: pattern={:?} text={:?}",
            pattern,
            text
        );
    }
    }
}
