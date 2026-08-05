//! 命令验证契约 — 攻击类型/命令/策略/trait 上提(ADR-054 决策 3,P9-T4)
//!
//! 对应架构层: **L0 Contracts**(从 L4 `seccore` 上提,消除 L8 parliament → L4 seccore
//! 生产依赖违规边,ADR-054 裁决)
//! 对应 ADR: **ADR-054 决策 3**(MemoryStrategyProvider 先例:L0 trait 解耦)
//!
//! # 核心职责
//!
//! 承载命令静态分析的三方共享契约:
//! - `AttackType`:6 类攻击向量枚举(原 `seccore/src/types.rs`)
//! - `Command`:待校验的原始命令(原 `seccore/src/types.rs`)
//! - `BlockedPattern` / `CommandPolicy`:拦截模式与白名单策略(原 `seccore/src/policy.rs`)
//! - `CommandValidationError` / `CommandValidator` trait:L0 校验抽象,seccore 实现、
//!   parliament 注入(L8 → L0 依赖,替代原 L8 → L4 直接调用)
//!
//! # 设计约束(ADR-033)
//!
//! - **纯类型 + 基础构造**: `default_secure()` 为语义冻结迁移(与 `budget_tier` 的
//!   `as_str()` 同类先例),仅移动定义位置,逐字保留 seccore 原实现
//! - **零 crate 依赖**(serde derive 例外): 与 L0 其余模块一致
//!
//! # 语义对齐(WHY)
//!
//! `default_secure()` 的 allow/block 链与 seccore 原实现**逐字一致**,禁止简化或
//! 遗漏拦截模式——该策略被 AHIRT 探测率验证与 `Sandbox::with_default_policy` 消费,
//! 任何改动都会破坏安全边界。

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// 攻击类型 — 对应 6 种需拦截的攻击向量(对齐验收标准)。
///
/// 每个变体对应一类尸检教训:
/// - `Injection`:Claude CVE-2026-35022 命令注入($(...)、|、;、&&)
/// - `PrivilegeEscalation`:sudo/su/chmod 提权
/// - `DataLeak`:SECRET/PASSWORD/敏感文件泄露
/// - `SandboxEscape`:路径遍历(../)、/proc//sys 逃逸
/// - `Tamper`:审计链/日志篡改
/// - `Abuse`:未授权命令(白名单外)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AttackType {
    /// 命令注入:shell 插值、管道、分隔符
    Injection,
    /// 权限提升:sudo/su/chmod
    PrivilegeEscalation,
    /// 数据泄露:SECRET/敏感文件
    DataLeak,
    /// 沙箱逃逸:路径遍历、系统目录
    SandboxEscape,
    /// 审计篡改:日志删除、链损坏
    Tamper,
    /// 滥用:未授权命令
    Abuse,
}

/// 原始命令 — 用户或上层提交的待执行命令。
///
/// 零信任模型下,此结构的内容**不可信**,必须经 `validate_command`
/// 与 `validate_env` 校验后才能进入沙箱执行层。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    /// 可执行程序名(如 "echo"、"ls")
    pub program: String,
    /// 命令参数列表(已拆分,禁止 shell 二次解析)
    pub args: Vec<String>,
    /// 环境变量映射(用户显式设置,非继承)
    pub env: HashMap<String, String>,
}

impl Command {
    /// 创建新命令,仅指定程序名。
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: HashMap::new(),
        }
    }

    /// 链式添加单个参数。
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// 链式添加多个参数。
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// 链式设置环境变量。
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }
}

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
    ///
    /// ⚠️ 语义冻结迁移(ADR-054 决策 3,P9-T4):此方法从 `seccore/src/policy.rs`
    /// 逐字迁移至 L0,禁止简化或遗漏拦截模式。seccore 经 re-export 保兼容,
    /// 任何改动都会破坏 AHIRT 探测率验证与 Sandbox 默认策略的安全边界。
    pub fn default_secure() -> Self {
        let mut policy = Self::new();

        // === 安全命令白名单(只读、无副作用) ===
        // 安全决策(WHY 不含 cmd/PowerShell):
        // cmd.exe 是通用 shell 启动器,`cmd /c "任意命令"` 可绕过全部 4 层防御
        // (白名单通过 + 无 blocked_pattern 匹配),构成零信任模型的致命漏洞。
        // Windows 兼容性测试应使用受限 PowerShell ExecutionPolicy 沙箱,
        // 而非在白名单中保留 cmd。参见 N1 安全审计报告。
        for cmd in [
            "echo", "ls", "cat", "pwd", "whoami", "date", "true", "false", "printf", "head",
            "tail", "wc", "sort", "uniq", "cut", "tr", "basename", "dirname",
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

/// 命令校验错误 — L0 契约层错误载体(seccore `SecCoreError::CommandBlocked` 的 L0 映射)。
///
/// WHY 独立于 seccore 错误类型:parliament 仅依赖 L0,不能感知 L4 具体错误
/// (ADR-054 决策 3 层间解耦),此类型是 trait 返回值的统一错误表示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandValidationError {
    /// 拦截的攻击类型
    pub attack_type: AttackType,
    /// 拦截详情(人类可读)
    pub detail: String,
}

impl std::fmt::Display for CommandValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.attack_type, self.detail)
    }
}

impl std::error::Error for CommandValidationError {}

/// 命令校验抽象 — L0 契约 trait,seccore 实现,parliament 注入。
///
/// WHY `Send + Sync`:validator 被 `AhirtRedTeam` 以 `Arc<dyn CommandValidator>`
/// 持有并跨线程共享(并发探测),必须满足 Send + Sync(§4.1 async 约束)。
pub trait CommandValidator: Send + Sync {
    /// 校验命令 — 通过返回 `Ok(())`,拦截返回 `Err(CommandValidationError)`。
    fn validate(&self, cmd: &Command, policy: &CommandPolicy)
        -> Result<(), CommandValidationError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全部攻击类型清单 — 供遍历式测试复用,避免遗漏新增变体
    const ALL_ATTACK_TYPES: [AttackType; 6] = [
        AttackType::Injection,
        AttackType::PrivilegeEscalation,
        AttackType::DataLeak,
        AttackType::SandboxEscape,
        AttackType::Tamper,
        AttackType::Abuse,
    ];

    /// proptest 策略:全变体空间任意 `AttackType`
    ///
    /// WHY 用 `prop::sample::select` 显式覆盖 6 类,而非为纯枚举实现 `Arbitrary`:
    /// 保持 L0 零逻辑约束,测试专用策略不进入生产 API(ADR-033)。
    fn any_attack_type() -> impl proptest::strategy::Strategy<Value = AttackType> {
        proptest::sample::select(vec![
            AttackType::Injection,
            AttackType::PrivilegeEscalation,
            AttackType::DataLeak,
            AttackType::SandboxEscape,
            AttackType::Tamper,
            AttackType::Abuse,
        ])
    }

    /// default_secure 不变量: 白名单非空 + 拦截模式非空 + 覆盖 5 类模式攻击
    ///
    /// WHY 断言 5 类而非 6 类:seccore 原实现中 Abuse 由白名单机制处理
    /// (validate_command 非白名单命令判定为 Abuse),`blocked_patterns` 仅含
    /// Injection/PrivilegeEscalation/SandboxEscape/DataLeak/Tamper 五类模式,
    /// 此处为语义冻结迁移,不新增 Abuse 拦截模式(避免改变策略语义)。
    #[test]
    fn test_default_secure_invariants() {
        let policy = CommandPolicy::default_secure();
        assert!(
            !policy.allowed_commands.is_empty(),
            "白名单不应为空(至少含只读命令)"
        );
        assert!(
            !policy.blocked_patterns.is_empty(),
            "拦截模式不应为空(至少含 5 类攻击模式)"
        );
        // 5 类模式攻击类型均应有拦截模式覆盖(Abuse 由白名单机制处理,见上注释)
        const PATTERNED_ATTACK_TYPES: [AttackType; 5] = [
            AttackType::Injection,
            AttackType::PrivilegeEscalation,
            AttackType::DataLeak,
            AttackType::SandboxEscape,
            AttackType::Tamper,
        ];
        for attack_type in PATTERNED_ATTACK_TYPES {
            assert!(
                policy
                    .blocked_patterns
                    .iter()
                    .any(|p| p.attack_type == attack_type),
                "缺少 {attack_type:?} 类拦截模式"
            );
        }
        // 白名单含常见只读命令(语义冻结,与 seccore 原策略一致)
        assert!(policy.allowed_commands.contains("echo"), "白名单应含 echo");
        assert!(policy.allowed_commands.contains("ls"), "白名单应含 ls");
    }

    /// Command 构造: `new` + `arg` + `env` 链式字段正确
    #[test]
    fn test_command_construction() {
        let cmd = Command::new("ls").arg("-l").env("PATH", "/usr/bin");
        assert_eq!(cmd.program, "ls");
        assert_eq!(cmd.args, vec!["-l"]);
        assert_eq!(cmd.env.get("PATH").map(String::as_str), Some("/usr/bin"));
        assert!(Command::new("echo").args(["a", "b"]).args.len() == 2);
    }

    /// AttackType 序列化往返: 每个变体 serde_json 序列化 → 反序列化后与原值相等
    #[test]
    fn test_attack_type_serde_roundtrip_all_variants() {
        for attack_type in ALL_ATTACK_TYPES {
            let json = serde_json::to_string(&attack_type).unwrap();
            let restored: AttackType = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, attack_type, "变体 {attack_type:?} 序列化往返失败");
        }
    }

    /// Command 序列化往返: 结构体字段完整保留
    #[test]
    fn test_command_serde_roundtrip() {
        let cmd = Command::new("echo").arg("hello");
        let json = serde_json::to_string(&cmd).unwrap();
        let restored: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, cmd);
    }

    /// CommandValidationError Display 格式: `{attack_type}: {detail}`
    #[test]
    fn test_validation_error_display() {
        let err = CommandValidationError {
            attack_type: AttackType::Injection,
            detail: "检测到命令替换 $(...)".to_string(),
        };
        assert_eq!(err.to_string(), "Injection: 检测到命令替换 $(...)");
    }

    // proptest 属性: 全变体往返 + default_secure 构建不 panic
    //
    // WHY 使用普通注释而非 doc comment:proptest! 宏生成测试包装时,宏外部与
    // 内部的 doc comment 均无法附着到生成项,新版 clippy 会报 unused_doc_comments
    // (与既有 budget_tier.rs:120 同类问题,此处新代码不再引入新 lint 错误)。
    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]

        // 任意攻击类型 serde_json 往返后与原值相等(覆盖全变体空间)
        #[test]
        fn prop_attack_type_roundtrip(attack_type in any_attack_type()) {
            let json = serde_json::to_string(&attack_type).unwrap();
            let restored: AttackType = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, attack_type, "变体 {attack_type:?} 往返失败");
        }

        // default_secure 构建不 panic + 每类模式攻击均被策略覆盖
        // WHY Abuse 特殊处理:Abuse 由白名单机制处理(validate_command 非白名单
        // 判定),`blocked_patterns` 不含 Abuse 模式,故仅断言其余 5 类覆盖。
        #[test]
        fn prop_default_secure_builds(attack_type in any_attack_type()) {
            let policy = CommandPolicy::default_secure();
            assert!(!policy.allowed_commands.is_empty(), "白名单不应为空");
            if attack_type != AttackType::Abuse {
                assert!(
                    policy
                        .blocked_patterns
                        .iter()
                        .any(|p| p.attack_type == attack_type),
                    "缺少 {attack_type:?} 类拦截模式"
                );
            }
        }
    }
}
