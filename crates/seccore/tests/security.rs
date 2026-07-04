//! SecCore 安全测试 — 6 种攻击拦截 + 审计链完整性 + 环境变量白名单
//!
//! 对应验收标准:
//! - 6 种攻击拦截(注入/越权/泄露/逃逸/篡改/滥用)✓
//! - SHA-256 Merkle 审计链 ✓
//! - 命令白名单(禁止 shell 插值)✓
//! - 环境变量白名单(防止 SECRET 泄露)✓

use std::collections::HashMap;
use std::time::Duration;

use seccore::{
    AttackType, AuditChain, Command, CommandPolicy, CommandSpec, EnvPolicy, ExecutionResult,
    RiskLevel, Sandbox, SecCoreError,
};

/// 构建测试用沙箱(默认安全策略)。
fn make_sandbox() -> Sandbox {
    Sandbox::with_default_policy()
}

// =============================================================================
// 1. Injection:命令注入拦截
// =============================================================================

#[tokio::test]
async fn test_injection_blocked() {
    let mut sandbox = make_sandbox();
    // echo $(whoami) — 尝试通过命令替换执行任意命令
    let cmd = Command::new("echo").arg("$(whoami)");
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "命令注入应被拦截");
    match result.unwrap_err() {
        SecCoreError::CommandBlocked { attack_type, .. } => {
            assert_eq!(
                attack_type,
                AttackType::Injection,
                "应识别为 Injection 攻击"
            );
        }
        e => panic!("期望 CommandBlocked(Injection), 实际: {e:?}"),
    }
}

// =============================================================================
// 2. PrivilegeEscalation:权限提升拦截
// =============================================================================

#[tokio::test]
async fn test_privilege_escalation_blocked() {
    let mut sandbox = make_sandbox();
    // sudo ls — 尝试通过 sudo 提权执行命令
    let cmd = Command::new("sudo").arg("ls");
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "权限提升应被拦截");
    match result.unwrap_err() {
        SecCoreError::CommandBlocked { attack_type, .. } => {
            assert_eq!(
                attack_type,
                AttackType::PrivilegeEscalation,
                "应识别为 PrivilegeEscalation 攻击"
            );
        }
        e => panic!("期望 CommandBlocked(PrivilegeEscalation), 实际: {e:?}"),
    }
}

// =============================================================================
// 3. DataLeak:数据泄露拦截
// =============================================================================

#[tokio::test]
async fn test_data_leak_blocked() {
    let mut sandbox = make_sandbox();
    // cat /etc/passwd — 尝试读取敏感系统文件
    let cmd = Command::new("cat").arg("/etc/passwd");
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "数据泄露应被拦截");
    match result.unwrap_err() {
        SecCoreError::CommandBlocked { attack_type, .. } => {
            assert_eq!(attack_type, AttackType::DataLeak, "应识别为 DataLeak 攻击");
        }
        e => panic!("期望 CommandBlocked(DataLeak), 实际: {e:?}"),
    }
}

// =============================================================================
// 4. SandboxEscape:沙箱逃逸拦截
// =============================================================================

#[tokio::test]
async fn test_sandbox_escape_blocked() {
    let mut sandbox = make_sandbox();
    // cat ../../etc/foo — 尝试通过路径遍历逃逸沙箱
    let cmd = Command::new("cat").arg("../../etc/foo");
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "沙箱逃逸应被拦截");
    match result.unwrap_err() {
        SecCoreError::CommandBlocked { attack_type, .. } => {
            assert_eq!(
                attack_type,
                AttackType::SandboxEscape,
                "应识别为 SandboxEscape 攻击"
            );
        }
        e => panic!("期望 CommandBlocked(SandboxEscape), 实际: {e:?}"),
    }
}

// =============================================================================
// 5. Tamper:审计链篡改检测
// =============================================================================

#[test]
fn test_tamper_detected() {
    let mut chain = AuditChain::new();

    // 构造命令规格与执行结果
    let spec = CommandSpec {
        program: "echo".to_string(),
        allowed_args: vec!["hello".to_string()],
        env_whitelist: HashMap::new(),
        risk_level: RiskLevel::Low,
    };
    let result = ExecutionResult {
        exit_code: 0,
        stdout: "hello\n".to_string(),
        stderr: String::new(),
        duration: Duration::from_millis(10),
        audit_hash: "0".repeat(64),
    };

    // 追加两条审计记录
    chain.append(&spec, &result).unwrap();
    chain.append(&spec, &result).unwrap();
    assert!(chain.verify().unwrap(), "未篡改前审计链应完整");

    // 篡改第一个块的 result_hash
    chain.blocks[0].result_hash = "1".repeat(64);

    // 篡改后验证应失败
    assert!(!chain.verify().unwrap(), "篡改后审计链应检测到异常");
}

// =============================================================================
// 6. Abuse:未授权命令拦截
// =============================================================================

#[tokio::test]
async fn test_abuse_blocked() {
    let mut sandbox = make_sandbox();
    // curl evil.com — curl 不在白名单,判定为 Abuse
    let cmd = Command::new("curl").arg("evil.com");
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "未授权命令应被拦截");
    match result.unwrap_err() {
        SecCoreError::CommandBlocked { attack_type, .. } => {
            assert_eq!(attack_type, AttackType::Abuse, "应识别为 Abuse 攻击");
        }
        e => panic!("期望 CommandBlocked(Abuse), 实际: {e:?}"),
    }
}

// =============================================================================
// 7. 审计链完整性:正常执行后审计链完整
// =============================================================================

#[tokio::test]
async fn test_audit_chain_integrity() {
    let mut sandbox = make_sandbox();

    // 跨平台命令:Windows 用 cmd /C echo hello,Linux 用 echo hello
    // 注意:Windows 的 echo 是 cmd 内置命令,需通过 cmd /C 调用
    #[cfg(windows)]
    let cmd = Command::new("cmd").args(["/C", "echo", "hello"]);
    #[cfg(not(windows))]
    let cmd = Command::new("echo").arg("hello");

    let result = sandbox.audit_and_execute(cmd).await;
    assert!(result.is_ok(), "正常命令应执行成功: {:?}", result.err());

    let result = result.unwrap();
    assert_eq!(result.exit_code, 0, "退出码应为 0");

    // 验证审计链完整性
    assert!(sandbox.audit_chain.verify().unwrap(), "执行后审计链应完整");
    assert_eq!(sandbox.audit_chain.len(), 1, "应有一条审计记录");
}

// =============================================================================
// 8. 环境变量白名单:白名单内通过,SECRET 被拦截
// =============================================================================

#[tokio::test]
async fn test_env_whitelist() {
    let mut sandbox = make_sandbox();

    // 场景1:白名单内环境变量(PATH)应通过
    #[cfg(windows)]
    let cmd_ok = Command::new("cmd")
        .args(["/C", "echo", "ok"])
        .env("PATH", "/usr/bin");
    #[cfg(not(windows))]
    let cmd_ok = Command::new("echo").arg("ok").env("PATH", "/usr/bin");

    let result = sandbox.audit_and_execute(cmd_ok).await;
    assert!(result.is_ok(), "白名单内环境变量应通过: {:?}", result.err());

    // 场景2:SECRET_KEY 应被拦截
    #[cfg(windows)]
    let cmd_leak = Command::new("cmd")
        .args(["/C", "echo", "leak"])
        .env("SECRET_KEY", "sensitive_value");
    #[cfg(not(windows))]
    let cmd_leak = Command::new("echo")
        .arg("leak")
        .env("SECRET_KEY", "sensitive_value");

    let result = sandbox.audit_and_execute(cmd_leak).await;
    assert!(result.is_err(), "SECRET 环境变量应被拦截");
    match result.unwrap_err() {
        SecCoreError::EnvVarBlocked { name, pattern } => {
            assert!(name.contains("SECRET"), "变量名应包含 SECRET");
            assert!(pattern.contains("SECRET"), "匹配模式应为 SECRET 相关");
        }
        e => panic!("期望 EnvVarBlocked, 实际: {e:?}"),
    }
}

// =============================================================================
// 补充:多种注入变体拦截
// =============================================================================

#[tokio::test]
async fn test_injection_variants_blocked() {
    let mut sandbox = make_sandbox();

    // 反引号命令替换
    let cmd = Command::new("echo").arg("`whoami`");
    assert!(
        sandbox.audit_and_execute(cmd).await.is_err(),
        "反引号注入应被拦截"
    );

    // 管道符
    let cmd = Command::new("echo").arg("hello|cat");
    assert!(
        sandbox.audit_and_execute(cmd).await.is_err(),
        "管道符注入应被拦截"
    );

    // 命令分隔符
    let cmd = Command::new("echo").arg("hello;whoami");
    assert!(
        sandbox.audit_and_execute(cmd).await.is_err(),
        "分号注入应被拦截"
    );

    // 命令链 &&
    let cmd = Command::new("echo").arg("hello&&whoami");
    assert!(
        sandbox.audit_and_execute(cmd).await.is_err(),
        "&& 注入应被拦截"
    );
}

// =============================================================================
// 补充:环境变量策略单元测试
// =============================================================================

#[test]
fn test_env_policy_unit() {
    let policy = EnvPolicy::default_secure();

    // 白名单内变量通过
    let mut env = HashMap::new();
    env.insert("PATH".to_string(), "/usr/bin".to_string());
    env.insert("HOME".to_string(), "/home/user".to_string());
    let result = seccore::validate_env(&env, &policy);
    assert!(result.is_ok(), "白名单内变量应通过");
    let filtered = result.unwrap();
    assert_eq!(filtered.len(), 2);

    // 敏感变量被拦截
    let mut env = HashMap::new();
    env.insert("API_KEY".to_string(), "secret".to_string());
    let result = seccore::validate_env(&env, &policy);
    assert!(result.is_err(), "API_KEY 应被拦截");

    // 非白名单非敏感变量被拦截(零信任)
    let mut env = HashMap::new();
    env.insert("CUSTOM_VAR".to_string(), "value".to_string());
    let result = seccore::validate_env(&env, &policy);
    assert!(result.is_err(), "非白名单变量应被拦截(零信任)");
}

// =============================================================================
// 9. F-002:沙箱超时机制 — 长时间运行的命令应被强制终止
// =============================================================================
// 对应尸检教训:恶意命令(如 sleep infinity)可永久阻塞子进程,导致 DoS
// 修复:沙箱增加可配置超时,超时后强制终止子进程

#[test]
fn test_sandbox_default_timeout_is_30s() {
    // 默认超时应为 30 秒(可配置,满足大多数命令执行需求)
    let sandbox = Sandbox::with_default_policy();
    assert_eq!(
        sandbox.timeout,
        Duration::from_secs(30),
        "默认超时应为 30 秒"
    );
}

#[tokio::test]
async fn test_sandbox_timeout_kills_long_running_process() {
    // 跨平台长时间运行命令(运行约 30 秒,远超 1 秒超时)
    // Windows:cmd 在默认白名单内,用 ping -n 30(约 29 秒)
    //   注意:沙箱 env_clear() 清除 PATH,需显式传递 PATH 让 cmd 找到 ping.exe
    // Linux:sleep 不在默认白名单,构造自定义策略允许 sleep
    #[cfg(windows)]
    let cmd = Command::new("cmd")
        .args(["/C", "ping", "-n", "30", "127.0.0.1"])
        .env("PATH", std::env::var("PATH").unwrap_or_default());
    #[cfg(not(windows))]
    let cmd = Command::new("sleep").arg("30");

    #[cfg(windows)]
    let policy = CommandPolicy::default_secure();
    #[cfg(not(windows))]
    let policy = CommandPolicy::new().allow_command("sleep");

    // 设置 1 秒超时:长时间命令应在 1 秒后被强制终止
    let mut sandbox =
        Sandbox::new(policy, EnvPolicy::default_secure()).with_timeout(Duration::from_secs(1));

    let start = std::time::Instant::now();
    let result = sandbox.audit_and_execute(cmd).await;

    // 验证:应返回 SandboxTimeout 错误(而非等待 30 秒)
    assert!(
        result.is_err(),
        "长时间运行的命令应被超时拦截,实际: {result:?}"
    );
    match result.unwrap_err() {
        SecCoreError::SandboxTimeout { timeout, program } => {
            assert_eq!(timeout, Duration::from_secs(1), "超时时长应匹配配置值");
            #[cfg(windows)]
            assert_eq!(program, "cmd", "程序名应匹配被终止的进程");
            #[cfg(not(windows))]
            assert_eq!(program, "sleep", "程序名应匹配被终止的进程");
        }
        e => panic!("期望 SandboxTimeout, 实际: {e:?}"),
    }

    // 验证:实际耗时应接近超时阈值(1秒),不应等待完整 30 秒
    // 允许 4 秒调度开销(子进程启动 + kill 清理)
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "超时应在 1 秒后触发,实际耗时: {:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn test_sandbox_normal_command_not_affected_by_timeout() {
    // 验证:合理的超时设置不影响正常命令执行
    // 设置 5 秒超时,执行瞬时命令(echo),应正常返回而非超时
    let mut sandbox = Sandbox::with_default_policy().with_timeout(Duration::from_secs(5));

    #[cfg(windows)]
    let cmd = Command::new("cmd").args(["/C", "echo", "hello"]);
    #[cfg(not(windows))]
    let cmd = Command::new("echo").arg("hello");

    let result = sandbox.audit_and_execute(cmd).await;
    assert!(
        result.is_ok(),
        "正常命令应在超时内完成,实际: {:?}",
        result.err()
    );

    let result = result.unwrap();
    assert_eq!(result.exit_code, 0, "退出码应为 0");
}
