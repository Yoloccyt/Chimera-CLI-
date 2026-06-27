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
}

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
/// 零信任模型下,此结构的内容**不可信**,必须经 `policy::validate_command`
/// 与 `policy::validate_env` 校验后才能进入沙箱执行层。
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

/// 命令规格 — 经策略校验后的安全命令表示。
///
/// 这是 `Command` 通过策略校验后的产物,携带校验时确定的:
/// - `allowed_args`:已确认安全的参数列表
/// - `env_whitelist`:已通过环境变量白名单过滤的映射
/// - `risk_level`:基于程序名与参数评估的风险等级
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
    /// 风险等级(用于审计与限流)
    pub risk_level: RiskLevel,
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
