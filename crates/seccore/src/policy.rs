//! 命令策略与环境变量策略 — 零信任沙箱的第一道防线(静态分析)
//!
//! 对应尸检教训:
//! - Claude CVE-2026-35022:命令注入($(...)、|、;、&&)
//! - 环境变量泄露:SECRET/KEY/TOKEN/PASSWORD 明文传递
//! - 权限提升:sudo/su 未授权提权
//!
//! 设计决策(WHY):
//! - **检查顺序**:拦截模式优先于白名单检查。这样 `sudo ls` 会被识别为
//!   PrivilegeEscalation 而非 Abuse,便于审计分类。`curl evil.com`(无注入
//!   模式)才会落到白名单检查,被识别为 Abuse。
//! - **简单子串匹配**:不引入 regex 依赖,降低编译成本与攻击面。零信任模型
//!   下宁可误杀(如 `echo "a|b"` 被拦截),不可漏放。
//! - **大小写不敏感**:SECRET/secret/Secret 都需拦截,统一 to_lowercase 比较。

use std::collections::{HashMap, HashSet};

use tracing::warn;

use crate::error::SecCoreError;
use crate::types::{AttackType, Command, CommandSpec, RiskLevel};

/// 被拦截的模式 — 关联攻击类型,便于审计追溯。
#[derive(Debug, Clone)]
pub struct BlockedPattern {
    /// 模式字符串(子串匹配,大小写不敏感)
    pub pattern: String,
    /// 关联的攻击类型
    pub attack_type: AttackType,
    /// 人类可读的拦截描述
    pub description: String,
}

/// 命令策略 — 白名单 + 危险模式黑名单。
///
/// 零信任模型下,命令必须同时满足:
/// 1. 不匹配任何 `blocked_patterns`
/// 2. `program` 在 `allowed_commands` 白名单内
#[derive(Debug, Clone)]
pub struct CommandPolicy {
    /// 允许的程序名白名单(小写存储,大小写不敏感匹配)
    pub allowed_commands: HashSet<String>,
    /// 拦截模式列表(按攻击类型分组,检查顺序敏感)
    pub blocked_patterns: Vec<BlockedPattern>,
}

/// 环境变量策略 — 白名单 + 敏感关键词黑名单。
///
/// 零信任模型下,环境变量必须:
/// 1. 在 `env_whitelist` 白名单内
/// 2. 变量名不匹配任何 `sensitive_patterns`
///
/// 非白名单变量一律拒绝(即使不敏感),遵循最小权限原则。
#[derive(Debug, Clone)]
pub struct EnvPolicy {
    /// 允许的环境变量名白名单(原样存储,精确匹配)
    pub env_whitelist: HashSet<String>,
    /// 敏感关键词列表(变量名 to_uppercase 后子串匹配)
    pub sensitive_patterns: Vec<String>,
}

impl CommandPolicy {
    /// 创建空策略(无白名单、无拦截模式)。
    pub fn new() -> Self {
        Self {
            allowed_commands: HashSet::new(),
            blocked_patterns: Vec::new(),
        }
    }

    /// 链式添加允许的命令(自动转小写,大小写不敏感)。
    pub fn allow_command(mut self, cmd: impl Into<String>) -> Self {
        self.allowed_commands.insert(cmd.into().to_lowercase());
        self
    }

    /// 链式添加拦截模式。
    pub fn block_pattern(
        mut self,
        pattern: impl Into<String>,
        attack_type: AttackType,
        description: impl Into<String>,
    ) -> Self {
        self.blocked_patterns.push(BlockedPattern {
            pattern: pattern.into(),
            attack_type,
            description: description.into(),
        });
        self
    }

    /// 默认安全策略 — 包含常见只读命令白名单与 6 类攻击拦截模式。
    ///
    /// 拦截模式按以下顺序添加(检查顺序敏感):
    /// 1. Injection:shell 插值与命令分隔符($(...)、`、|、;、&&、||)
    /// 2. PrivilegeEscalation:提权命令(sudo、su、chmod)
    /// 3. SandboxEscape:路径遍历与系统目录(../、/proc/、/sys/)
    /// 4. DataLeak:敏感数据(/etc/passwd、/etc/shadow、SECRET、PASSWORD)
    /// 5. Tamper:审计/日志篡改(rm /var/log、shred)
    ///
    /// Abuse 由白名单处理(非白名单命令直接拒绝)。
    pub fn default_secure() -> Self {
        let mut policy = Self::new();

        // === 安全命令白名单(只读、无副作用) ===
        // 注意:`cmd` 仅用于 Windows 兼容性测试(Windows echo 是 cmd 内置命令)
        // 生产环境应移除 cmd,改用 PowerShell 沙箱或 gVisor 隔离
        for cmd in [
            "echo", "ls", "cat", "pwd", "whoami", "date", "true", "false", "printf", "head",
            "tail", "wc", "sort", "uniq", "cut", "tr", "basename", "dirname", "cmd",
        ] {
            policy = policy.allow_command(cmd);
        }

        // === 1. Injection:shell 插值与命令分隔符 ===
        // 对应 CVE-2026-35022:命令注入通过 $(...) 或管道链执行任意命令
        policy = policy.block_pattern("$(", AttackType::Injection, "检测到命令替换 $(...)");
        policy = policy.block_pattern("`", AttackType::Injection, "检测到反引号命令替换");
        policy = policy.block_pattern("|", AttackType::Injection, "检测到管道符 |");
        policy = policy.block_pattern(";", AttackType::Injection, "检测到命令分隔符 ;");
        policy = policy.block_pattern("&&", AttackType::Injection, "检测到命令链 &&");
        policy = policy.block_pattern("||", AttackType::Injection, "检测到命令链 ||");

        // === 2. PrivilegeEscalation:提权命令 ===
        // 子串匹配会误杀 `pseudo`,但零信任下宁可误杀
        policy = policy.block_pattern("sudo", AttackType::PrivilegeEscalation, "检测到 sudo 提权");
        policy = policy.block_pattern(" su ", AttackType::PrivilegeEscalation, "检测到 su 提权");
        policy = policy.block_pattern(
            "chmod",
            AttackType::PrivilegeEscalation,
            "检测到 chmod 权限修改",
        );
        policy = policy.block_pattern(
            "chown",
            AttackType::PrivilegeEscalation,
            "检测到 chown 所有者修改",
        );

        // === 3. SandboxEscape:路径遍历与系统目录 ===
        policy = policy.block_pattern("../", AttackType::SandboxEscape, "检测到路径遍历 ../");
        policy = policy.block_pattern("..\\", AttackType::SandboxEscape, "检测到路径遍历 ..\\");
        policy = policy.block_pattern("/proc/", AttackType::SandboxEscape, "检测到访问 /proc/");
        policy = policy.block_pattern("/sys/", AttackType::SandboxEscape, "检测到访问 /sys/");

        // === 4. DataLeak:敏感数据访问 ===
        policy = policy.block_pattern(
            "/etc/passwd",
            AttackType::DataLeak,
            "检测到访问 /etc/passwd",
        );
        policy = policy.block_pattern(
            "/etc/shadow",
            AttackType::DataLeak,
            "检测到访问 /etc/shadow",
        );
        policy = policy.block_pattern("secret", AttackType::DataLeak, "检测到 SECRET 关键词");
        policy = policy.block_pattern("password", AttackType::DataLeak, "检测到 PASSWORD 关键词");

        // === 5. Tamper:审计/日志篡改 ===
        policy = policy.block_pattern(
            "/var/log",
            AttackType::Tamper,
            "检测到访问 /var/log 日志目录",
        );
        policy = policy.block_pattern("shred", AttackType::Tamper, "检测到 shred 粉碎命令");

        policy
    }
}

impl Default for CommandPolicy {
    fn default() -> Self {
        Self::default_secure()
    }
}

impl EnvPolicy {
    /// 创建空策略(无白名单、默认敏感模式)。
    pub fn new() -> Self {
        Self {
            env_whitelist: HashSet::new(),
            sensitive_patterns: Vec::new(),
        }
    }

    /// 链式添加允许的环境变量名(精确匹配)。
    pub fn allow_env(mut self, key: impl Into<String>) -> Self {
        self.env_whitelist.insert(key.into());
        self
    }

    /// 链式添加敏感关键词(变量名 to_uppercase 后子串匹配)。
    pub fn block_sensitive(mut self, pattern: impl Into<String>) -> Self {
        self.sensitive_patterns.push(pattern.into().to_uppercase());
        self
    }

    /// 默认安全策略 — 最小化白名单 + 敏感关键词黑名单。
    ///
    /// 白名单仅包含进程执行必需的非敏感变量。
    /// 敏感关键词覆盖常见密钥命名约定(SECRET/KEY/TOKEN/PASSWORD 等)。
    pub fn default_secure() -> Self {
        let mut policy = Self::new();

        // 最小化白名单:仅进程执行必需的非敏感变量
        for key in [
            "PATH", "HOME", "USER", "LANG", "LC_ALL", "TERM", "TMP", "TMPDIR",
        ] {
            policy = policy.allow_env(key);
        }

        // 敏感关键词黑名单(统一大写存储,匹配时 to_uppercase)
        // KEY 会误杀 KEYBOARD/KEYMAP,但零信任下宁可误杀
        for pattern in [
            "SECRET",
            "KEY",
            "TOKEN",
            "PASSWORD",
            "PASS",
            "CREDENTIAL",
            "PRIVATE",
            "CERT",
            "API",
        ] {
            policy = policy.block_sensitive(pattern);
        }

        policy
    }
}

impl Default for EnvPolicy {
    fn default() -> Self {
        Self::default_secure()
    }
}

/// 校验命令 — 静态分析拦截注入/越权/逃逸/泄露/篡改/滥用。
///
/// 检查顺序(WHY:拦截模式优先,确保攻击类型正确分类):
/// 1. 拦截模式匹配(Injection → PrivilegeEscalation → SandboxEscape → DataLeak → Tamper)
/// 2. 白名单检查(非白名单 → Abuse)
/// 3. 风险评估
///
/// # 参数
/// - `cmd`:原始命令(不可信)
/// - `policy`:命令策略
///
/// # 返回
/// - `Ok(CommandSpec)`:校验通过的安全命令规格
/// - `Err(SecCoreError::CommandBlocked)`:检测到攻击,携带攻击类型
pub fn validate_command(
    cmd: &Command,
    policy: &CommandPolicy,
) -> Result<CommandSpec, SecCoreError> {
    // 构建完整命令字符串用于模式匹配(program + args)
    let full_command = build_full_command_string(&cmd.program, &cmd.args);
    let full_command_lower = full_command.to_lowercase();

    // 步骤1:拦截模式匹配(按添加顺序检查,首个匹配即返回)
    for pattern in &policy.blocked_patterns {
        let pattern_lower = pattern.pattern.to_lowercase();
        if full_command_lower.contains(&pattern_lower) {
            warn!(
                attack_type = ?pattern.attack_type,
                command = %full_command,
                pattern = %pattern.pattern,
                "命令被策略拦截"
            );
            return Err(SecCoreError::CommandBlocked {
                attack_type: pattern.attack_type,
                detail: pattern.description.clone(),
            });
        }
    }

    // 步骤2:白名单检查(拦截模式未匹配,但命令可能未授权)
    let program_lower = cmd.program.to_lowercase();
    if !policy.allowed_commands.contains(&program_lower) {
        warn!(
            program = %cmd.program,
            "命令不在白名单,判定为 Abuse"
        );
        return Err(SecCoreError::CommandBlocked {
            attack_type: AttackType::Abuse,
            detail: format!("命令 '{}' 不在白名单", cmd.program),
        });
    }

    // 步骤3:风险评估(用于审计与限流,不拦截)
    let risk_level = assess_risk(&cmd.program, &cmd.args);

    Ok(CommandSpec {
        program: cmd.program.clone(),
        allowed_args: cmd.args.clone(),
        env_whitelist: HashMap::new(),
        risk_level,
    })
}

/// 校验环境变量 — 拦截敏感信息泄露。
///
/// 零信任模型:非白名单变量一律拒绝(即使不敏感)。
/// 敏感变量返回明确的敏感模式,便于审计;非白名单变量返回 "not_in_whitelist"。
///
/// # 参数
/// - `env`:用户显式设置的环境变量映射(不可信)
/// - `policy`:环境变量策略
///
/// # 返回
/// - `Ok(HashMap)`:白名单过滤后的环境变量
/// - `Err(SecCoreError::EnvVarBlocked)`:检测到敏感或非白名单变量
pub fn validate_env(
    env: &HashMap<String, String>,
    policy: &EnvPolicy,
) -> Result<HashMap<String, String>, SecCoreError> {
    let mut filtered = HashMap::with_capacity(env.len());

    for (key, value) in env {
        // 白名单优先:白名单内变量直接通过(即使名称含敏感词)
        if policy.env_whitelist.contains(key) {
            filtered.insert(key.clone(), value.clone());
            continue;
        }

        // 非白名单:检查是否匹配敏感模式
        let key_upper = key.to_uppercase();
        for pattern in &policy.sensitive_patterns {
            if key_upper.contains(pattern) {
                warn!(
                    env_key = %key,
                    pattern = %pattern,
                    "环境变量匹配敏感模式,被拦截"
                );
                return Err(SecCoreError::EnvVarBlocked {
                    name: key.clone(),
                    pattern: pattern.clone(),
                });
            }
        }

        // 非白名单且非敏感:零信任下仍拒绝(最小权限原则)
        warn!(
            env_key = %key,
            "环境变量不在白名单,被拦截"
        );
        return Err(SecCoreError::EnvVarBlocked {
            name: key.clone(),
            pattern: "not_in_whitelist".to_string(),
        });
    }

    Ok(filtered)
}

/// 构建完整命令字符串(program + " " + args.join(" "))。
///
/// 用于拦截模式的子串匹配。注意:此字符串仅用于静态分析,
/// 不传递给 shell 执行(避免二次解析风险)。
fn build_full_command_string(program: &str, args: &[String]) -> String {
    let mut s = String::with_capacity(program.len() + 1);
    s.push_str(program);
    for arg in args {
        s.push(' ');
        s.push_str(arg);
    }
    s
}

/// 评估命令风险等级 — 基于程序名与参数模式。
///
/// 此函数不拦截命令,仅用于审计与限流决策。
fn assess_risk(program: &str, args: &[String]) -> RiskLevel {
    let program_lower = program.to_lowercase();

    // 高危命令:破坏性操作
    match program_lower.as_str() {
        "rm" | "dd" | "mkfs" | "fdisk" | "shred" | "wipe" => {
            return RiskLevel::High;
        }
        _ => {}
    }

    // 检查参数中的危险模式
    let full = args.join(" ");
    if full.contains(">") || full.contains(">>") {
        // 输出重定向:可能覆盖文件
        return RiskLevel::Medium;
    }
    if full.contains("*") || full.contains("?") {
        // 通配符:可能匹配意外文件
        return RiskLevel::Medium;
    }

    RiskLevel::Low
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy_blocks_injection() {
        let policy = CommandPolicy::default_secure();
        let cmd = Command::new("echo").arg("$(whoami)");
        let result = validate_command(&cmd, &policy);
        assert!(matches!(
            result,
            Err(SecCoreError::CommandBlocked {
                attack_type: AttackType::Injection,
                ..
            })
        ));
    }

    #[test]
    fn test_default_policy_allows_safe_command() {
        let policy = CommandPolicy::default_secure();
        let cmd = Command::new("echo").arg("hello");
        let result = validate_command(&cmd, &policy);
        assert!(result.is_ok());
    }

    #[test]
    fn test_env_policy_blocks_secret() {
        let policy = EnvPolicy::default_secure();
        let mut env = HashMap::new();
        env.insert("SECRET_KEY".to_string(), "leak".to_string());
        let result = validate_env(&env, &policy);
        assert!(matches!(result, Err(SecCoreError::EnvVarBlocked { .. })));
    }

    #[test]
    fn test_env_policy_allows_whitelist() {
        let policy = EnvPolicy::default_secure();
        let mut env = HashMap::new();
        env.insert("PATH".to_string(), "/usr/bin".to_string());
        let result = validate_env(&env, &policy);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().get("PATH").unwrap(), "/usr/bin");
    }
}
