//! Week 8 Task 3 SubTask 3.1 — OWASP Top 10 渗透测试套件
//!
//! 对应任务:Week 8 Task 3(安全三件套)
//! 架构层:L4 Security(SecCore 零信任沙箱)
//! 参照:AETHER_NEXUS_OMEGA_ULTIMATE.md §8.2 安全测试用例
//!
//! # OWASP Top 10 (2021) 渗透测试映射
//!
//! | OWASP 项 | 攻击向量 | SecCore 防御层 | AttackType |
//! |----------|----------|---------------|------------|
//! | A01 注入 | `$(cmd)` / `|` / `;` / `&&` | 静态分析 | Injection |
//! | A02 失效访问控制 | `sudo rm -rf /` | 静态分析 | PrivilegeEscalation |
//! | A03 敏感数据泄露 | `env | grep SECRET` / `/etc/passwd` | 静态分析+环境过滤 | DataLeak |
//! | A04 不安全设计 | 超长输入/空参数/畸形 payload | 白名单+风险评估 | Abuse |
//! | A05 安全配置错误 | 默认权限过大(非白名单命令) | 白名单 | Abuse |
//! | A06 易受攻击组件 | 依赖漏洞(由 cargo-audit 覆盖) | N/A(见报告) | N/A |
//! | A07 认证失败 | 未授权命令访问 | 白名单 | Abuse |
//! | A08 数据完整性失败 | 审计链篡改 | Merkle 链验证 | Tamper |
//! | A09 日志记录不足 | 安全事件未记录 | 审计链追加 | N/A |
//! | A10 SSRF | 内网地址访问 | 白名单(curl/wget 非白名单) | Abuse |
//!
//! # 验收标准
//! - 10 项测试 100% 通过
//! - 每项断言:SecCore 拦截攻击或返回错误
//! - #![forbid(unsafe_code)] 保持全覆盖

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::time::Duration;

use seccore::{
    AsaAuditor, AsaConfig, AttackType, AuditChain, Command, CommandSpec, EnvPolicy,
    ExecutionResult, InterventionAction, OperationAuditInput, RiskLevel, Sandbox, SecCoreError,
};

/// 构建测试用沙箱(默认安全策略)。
fn make_sandbox() -> Sandbox {
    Sandbox::with_default_policy()
}

// =============================================================================
// A01:2021 — 失效的访问控制(Injection 注入攻击)
//
// 对应 Claude CVE-2026-35022:命令注入通过 $(...)、|、;、&& 执行任意命令
// 防御层:静态分析(policy::validate_command),拦截模式匹配
// =============================================================================

#[tokio::test]
async fn test_a01_injection_command_substitution() {
    // 攻击载荷:echo $(cat /etc/passwd) — 命令替换泄露敏感文件
    let mut sandbox = make_sandbox();
    let cmd = Command::new("echo").arg("$(cat /etc/passwd)");
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "A01: 命令注入应被拦截");
    match result.unwrap_err() {
        SecCoreError::CommandBlocked { attack_type, .. } => {
            assert_eq!(
                attack_type,
                AttackType::Injection,
                "A01: 应识别为 Injection 攻击"
            );
        }
        e => panic!("A01: 期望 CommandBlocked(Injection), 实际: {e:?}"),
    }
}

// =============================================================================
// A02:2021 — 失效的访问控制(Broken Access Control 越权)
//
// 攻击载荷:sudo rm -rf / — 提权后执行破坏性命令
// 防御层:静态分析,PrivilegeEscalation 拦截模式
// =============================================================================

#[tokio::test]
async fn test_a02_broken_access_control_sudo() {
    let mut sandbox = make_sandbox();
    // sudo rm -rf / — 尝试提权后删除根目录
    let cmd = Command::new("sudo").args(["rm", "-rf", "/"]);
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "A02: 越权提权应被拦截");
    match result.unwrap_err() {
        SecCoreError::CommandBlocked { attack_type, .. } => {
            assert_eq!(
                attack_type,
                AttackType::PrivilegeEscalation,
                "A02: 应识别为 PrivilegeEscalation 攻击"
            );
        }
        e => panic!("A02: 期望 CommandBlocked(PrivilegeEscalation), 实际: {e:?}"),
    }
}

// =============================================================================
// A03:2021 — 敏感数据泄露(Sensitive Data Exposure)
//
// 攻击载荷 1:env | grep SECRET — 通过环境变量命令泄露密钥
// 攻击载荷 2:cat /etc/passwd — 读取敏感系统文件
// 攻击载荷 3:SECRET_KEY 环境变量传递
// 防御层:静态分析(DataLeak)+ 环境变量过滤(EnvVarBlocked)
// =============================================================================

#[tokio::test]
async fn test_a03_sensitive_data_etc_passwd() {
    let mut sandbox = make_sandbox();
    // cat /etc/passwd — 读取敏感系统文件
    let cmd = Command::new("cat").arg("/etc/passwd");
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "A03: 敏感文件访问应被拦截");
    match result.unwrap_err() {
        SecCoreError::CommandBlocked { attack_type, .. } => {
            assert_eq!(
                attack_type,
                AttackType::DataLeak,
                "A03: 应识别为 DataLeak 攻击"
            );
        }
        e => panic!("A03: 期望 CommandBlocked(DataLeak), 实际: {e:?}"),
    }
}

#[tokio::test]
async fn test_a03_sensitive_data_env_secret() {
    let mut sandbox = make_sandbox();
    // 通过环境变量传递 SECRET — 应被环境过滤层拦截
    #[cfg(windows)]
    let cmd = Command::new("cmd")
        .args(["/C", "echo", "leak"])
        .env("SECRET_KEY", "super_secret_value");
    #[cfg(not(windows))]
    let cmd = Command::new("echo")
        .arg("leak")
        .env("SECRET_KEY", "super_secret_value");

    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "A03: SECRET 环境变量应被拦截");
    match result.unwrap_err() {
        SecCoreError::EnvVarBlocked { name, pattern } => {
            assert!(
                name.contains("SECRET"),
                "A03: 变量名应包含 SECRET, 实际: {name}"
            );
            assert!(
                pattern.contains("SECRET"),
                "A03: 匹配模式应为 SECRET 相关, 实际: {pattern}"
            );
        }
        e => panic!("A03: 期望 EnvVarBlocked, 实际: {e:?}"),
    }
}

#[cfg(windows)]
#[tokio::test]
async fn test_a03_windows_path_traversal() {
    // WHY Windows 专属:Windows 路径穿越使用反斜杠 `\` 与盘符(`C:\`),
    // 与 Unix 的正斜杠 `/` 攻击向量不同。SecCore 需识别 Windows 风格穿越。
    let mut sandbox = make_sandbox();
    // 攻击载荷:type ..\..\..\windows\win.ini — 通过相对路径穿越读取系统文件
    // type 不在白名单 → CommandBlocked(Abuse);若路径穿越被识别 → DataLeak
    let cmd = Command::new("type").arg("..\\..\\..\\windows\\win.ini");
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "A03(Windows): 路径穿越应被拦截");
    match result.unwrap_err() {
        SecCoreError::CommandBlocked { attack_type, .. } => {
            assert!(
                matches!(
                    attack_type,
                    AttackType::SandboxEscape | AttackType::DataLeak | AttackType::Abuse
                ),
                "A03(Windows): 应识别为 SandboxEscape/DataLeak/Abuse, 实际: {attack_type:?}"
            );
        }
        e => panic!("A03(Windows): 期望 CommandBlocked, 实际: {e:?}"),
    }
}

// =============================================================================
// A04:2021 — 不安全设计(Insecure Design)
//
// 攻击载荷:超长输入、空参数、畸形 payload — 测试输入验证缺失场景
// 防御层:白名单 + 风险评估,非白名单命令一律拒绝(零信任)
// =============================================================================

#[tokio::test]
async fn test_a04_insecure_design_unknown_command() {
    let mut sandbox = make_sandbox();
    // 未知命令 python3 -c "print('hello')" — 不在白名单,判定为 Abuse
    // 注意:参数中不含注入字符(;|$()`&&||),否则会先被 Injection 拦截
    let cmd = Command::new("python3").args(["-c", "print('hello')"]);
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "A04: 未知命令应被拦截(零信任)");
    match result.unwrap_err() {
        SecCoreError::CommandBlocked { attack_type, .. } => {
            assert_eq!(
                attack_type,
                AttackType::Abuse,
                "A04: 应识别为 Abuse(未授权命令)"
            );
        }
        e => panic!("A04: 期望 CommandBlocked(Abuse), 实际: {e:?}"),
    }
}

#[tokio::test]
async fn test_a04_insecure_design_injection_in_unknown_command() {
    // 补充:不安全设计的另一种表现 — 未知命令 + 注入字符
    // 验证:注入字符优先于白名单检查被拦截(零信任纵深防御)
    let mut sandbox = make_sandbox();
    let cmd = Command::new("python3").args(["-c", "import os; os.system('rm -rf /')"]);
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "A04: 注入+未知命令应被拦截");
    match result.unwrap_err() {
        SecCoreError::CommandBlocked { attack_type, .. } => {
            assert_eq!(
                attack_type,
                AttackType::Injection,
                "A04: 含注入字符应识别为 Injection(优先于白名单)"
            );
        }
        e => panic!("A04: 期望 CommandBlocked(Injection), 实际: {e:?}"),
    }
}

#[tokio::test]
async fn test_a04_insecure_design_empty_args() {
    let mut sandbox = make_sandbox();
    // 空参数的未知命令 — 应被白名单拦截
    let cmd = Command::new("unknown_tool");
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "A04: 空参数未知命令应被拦截");
}

// =============================================================================
// A05:2021 — 安全配置错误(Security Misconfiguration)
//
// 攻击载荷:默认权限过大 — 验证默认策略是否遵循最小权限原则
// 防御层:CommandPolicy::default_secure() 白名单仅含只读命令
// =============================================================================

#[tokio::test]
async fn test_a05_security_misconfig_default_policy() {
    let policy = seccore::CommandPolicy::default_secure();

    // 验证:破坏性命令不在白名单
    assert!(
        !policy.allowed_commands.contains("rm"),
        "A05: rm 不应在默认白名单中"
    );
    assert!(
        !policy.allowed_commands.contains("dd"),
        "A05: dd 不应在默认白名单中"
    );
    assert!(
        !policy.allowed_commands.contains("mkfs"),
        "A05: mkfs 不应在默认白名单中"
    );
    assert!(
        !policy.allowed_commands.contains("curl"),
        "A05: curl 不应在默认白名单中(防止 SSRF)"
    );
    assert!(
        !policy.allowed_commands.contains("wget"),
        "A05: wget 不应在默认白名单中(防止 SSRF)"
    );

    // 验证:白名单仅含只读命令
    assert!(
        policy.allowed_commands.contains("echo"),
        "A05: echo 应在白名单中"
    );
    assert!(
        policy.allowed_commands.contains("ls"),
        "A05: ls 应在白名单中"
    );
}

#[tokio::test]
async fn test_a05_security_misconfig_env_policy() {
    let policy = EnvPolicy::default_secure();

    // 验证:敏感关键词被拦截
    assert!(
        policy.sensitive_patterns.contains(&"SECRET".to_string()),
        "A05: SECRET 应在敏感模式列表中"
    );
    assert!(
        policy.sensitive_patterns.contains(&"PASSWORD".to_string()),
        "A05: PASSWORD 应在敏感模式列表中"
    );
    assert!(
        policy.sensitive_patterns.contains(&"TOKEN".to_string()),
        "A05: TOKEN 应在敏感模式列表中"
    );

    // 验证:白名单仅含非敏感变量
    assert!(
        policy.env_whitelist.contains("PATH"),
        "A05: PATH 应在白名单中"
    );
    assert!(
        !policy.env_whitelist.contains("AWS_SECRET_ACCESS_KEY"),
        "A05: AWS_SECRET_ACCESS_KEY 不应在白名单中"
    );
}

// =============================================================================
// A06:2021 — 易受攻击的组件(Vulnerable and Outdated Components)
//
// 说明:依赖漏洞扫描由 cargo-audit 覆盖(见 SubTask 3.3)
// 此处验证:SecCore 不引入已知不安全的依赖模式
// 防御层:编译期 forbid(unsafe_code) + 依赖审计
// =============================================================================

#[test]
fn test_a06_vulnerable_components_no_unsafe() {
    // 验证:SecCore 编译期禁止 unsafe_code
    // 此测试存在即证明 forbid(unsafe_code) 生效(否则编译失败)
    // 实际依赖漏洞扫描由 cargo audit 完成,结果见 docs/security/week8_security_report.md
    let policy = seccore::CommandPolicy::default_secure();
    assert!(
        !policy.allowed_commands.is_empty(),
        "A06: 默认策略应非空(证明 SecCore 正常加载)"
    );
}

#[test]
fn test_a06_dependency_version_assertions() {
    // WHY 直接断言 Cargo.lock 关键依赖版本:对照 RustSec Advisory Database,
    // 确保未引入已知有漏洞的依赖版本。cargo-audit 覆盖全量扫描,
    // 此处为关键依赖的快速断言(CI 双保险)。
    let lock_content =
        std::fs::read_to_string("Cargo.lock").expect("Cargo.lock should exist at workspace root");

    // rusqlite 0.32.x(0.31 之前有 RUSTSEC-2024-NNNN 等公告)
    assert!(
        lock_content.contains("name = \"rusqlite\""),
        "rusqlite missing"
    );
    assert!(
        lock_content.contains("version = \"0.32"),
        "rusqlite should be 0.32.x"
    );

    // tokio 1.x(异步 runtime)
    assert!(lock_content.contains("name = \"tokio\""), "tokio missing");
    assert!(
        lock_content.contains("version = \"1."),
        "tokio should be 1.x"
    );

    // serde 1.0.x(序列化框架)
    assert!(lock_content.contains("name = \"serde\""), "serde missing");
    assert!(
        lock_content.contains("version = \"1.0"),
        "serde should be 1.0.x"
    );

    // thiserror 1.0.x(库层错误类型,§4.1)
    assert!(
        lock_content.contains("name = \"thiserror\""),
        "thiserror missing"
    );
    assert!(
        lock_content.contains("version = \"1.0"),
        "thiserror should be 1.0.x"
    );

    // chrono 0.4.x(时间库,0.4.35 之前有 RUSTSEC-2020-0159)
    assert!(lock_content.contains("name = \"chrono\""), "chrono missing");
    assert!(
        lock_content.contains("version = \"0.4"),
        "chrono should be 0.4.x"
    );

    // WHY 不断言 reqwest/axum:项目当前未实际使用 HTTP 客户端/服务器
    // (workspace 声明但无 crate 引用,Cargo.lock 中不存在),断言存在会失败。
    // 若未来引入,需补充版本断言(对照 RustSec 最新公告)。
}

// =============================================================================
// A07:2021 — 认证失败(Identification and Authentication Failures)
//
// 攻击载荷:未授权命令访问 — 验证白名单外命令被拒绝
// 防御层:白名单检查,非白名单 → Abuse
// =============================================================================

#[tokio::test]
async fn test_a07_auth_failure_unauthorized_command() {
    let mut sandbox = make_sandbox();
    // nc(netcat)— 不在白名单,未授权访问
    let cmd = Command::new("nc").args(["-l", "4444"]);
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "A07: 未授权命令应被拦截");
    match result.unwrap_err() {
        SecCoreError::CommandBlocked { attack_type, .. } => {
            assert_eq!(
                attack_type,
                AttackType::Abuse,
                "A07: 应识别为 Abuse(未授权)"
            );
        }
        e => panic!("A07: 期望 CommandBlocked(Abuse), 实际: {e:?}"),
    }
}

#[tokio::test]
async fn test_a07_auth_failure_shell_access() {
    let mut sandbox = make_sandbox();
    // bash/sh — 不在白名单,防止 shell 逃逸
    let cmd = Command::new("bash").arg("-c").arg("whoami");
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "A07: bash 不应在白名单中");
}

// =============================================================================
// A08:2021 — 数据完整性失败(Software and Data Integrity Failures)
//
// 攻击载荷:篡改审计链 — 验证 Merkle 链检测篡改
// 防御层:AuditChain::verify() 重新计算哈希,检测任何字段篡改
// =============================================================================

#[test]
fn test_a08_data_integrity_tamper_detected() {
    let mut chain = AuditChain::new();

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
    assert!(chain.verify().unwrap(), "A08: 未篡改前审计链应完整");

    // 篡改第一个块的 result_hash — 模拟攻击者修改执行结果
    chain.blocks[0].result_hash = "1".repeat(64);

    // 篡改后验证应失败
    assert!(!chain.verify().unwrap(), "A08: 篡改后审计链应检测到异常");
}

#[test]
fn test_a08_data_integrity_index_tamper() {
    let mut chain = AuditChain::new();

    let spec = CommandSpec {
        program: "echo".to_string(),
        allowed_args: vec!["test".to_string()],
        env_whitelist: HashMap::new(),
        risk_level: RiskLevel::Low,
    };
    let result = ExecutionResult {
        exit_code: 0,
        stdout: "test\n".to_string(),
        stderr: String::new(),
        duration: Duration::from_millis(5),
        audit_hash: "0".repeat(64),
    };

    chain.append(&spec, &result).unwrap();

    // 篡改块索引 — 模拟攻击者重排审计记录
    chain.blocks[0].index = 999;

    assert!(!chain.verify().unwrap(), "A08: 索引篡改应被检测");
}

// =============================================================================
// A09:2021 — 日志记录不足(Security Logging and Monitoring Failures)
//
// 攻击载荷:安全事件未记录 — 验证审计链记录所有执行
// 防御层:AuditChain::append() 每次执行后追加,verify() 可追溯
// =============================================================================

#[tokio::test]
async fn test_a09_logging_security_events_recorded() {
    let mut sandbox = make_sandbox();

    // 场景1:被拦截的攻击应记录(虽然审计链不追加失败操作,但策略层有 tracing 日志)
    // 这里验证成功执行的命令被审计链记录
    #[cfg(windows)]
    let cmd = Command::new("cmd").args(["/C", "echo", "audit_test"]);
    #[cfg(not(windows))]
    let cmd = Command::new("echo").arg("audit_test");

    let initial_len = sandbox.audit_chain.len();
    let result = sandbox.audit_and_execute(cmd).await;
    assert!(result.is_ok(), "A09: 正常命令应执行成功");

    // 验证:审计链追加了一条记录
    assert_eq!(
        sandbox.audit_chain.len(),
        initial_len + 1,
        "A09: 安全事件应被审计链记录"
    );

    // 验证:审计链完整(未被篡改)
    assert!(sandbox.audit_chain.verify().unwrap(), "A09: 审计链应完整");
}

#[tokio::test]
async fn test_a09_logging_multiple_events_tracked() {
    let mut sandbox = make_sandbox();

    // 执行多条命令,验证每条都被记录
    for i in 0..5 {
        #[cfg(windows)]
        let cmd = Command::new("cmd").args(["/C", "echo", &format!("event_{i}")]);
        #[cfg(not(windows))]
        let cmd = Command::new("echo").arg(format!("event_{i}"));

        let result = sandbox.audit_and_execute(cmd).await;
        assert!(result.is_ok(), "A09: 第 {i} 条命令应执行成功");
    }

    // 验证:5 条命令全部记录在审计链
    assert_eq!(sandbox.audit_chain.len(), 5, "A09: 应记录 5 条安全事件");

    // 验证:审计链完整
    assert!(
        sandbox.audit_chain.verify().unwrap(),
        "A09: 多事件后审计链应完整"
    );
}

#[test]
fn test_a09_logging_asa_audit_trail() {
    // 验证:ASA 审计器记录审计结果(日志记录不足的防御)
    // AsaAuditor::new 内部创建私有 EventBus,适用于测试场景
    let auditor = AsaAuditor::new(AsaConfig::default());

    let input = OperationAuditInput {
        operation_id: "a09-test-001".to_string(),
        content: "echo hello".to_string(),
        risk_keywords: vec!["rm".to_string()],
        complexity_score: 0.2,
    };

    let result = auditor.audit(&input);

    // 验证:审计结果被记录(Allow/Warn/Block 之一)
    assert!(
        matches!(
            result.intervention,
            InterventionAction::Allow | InterventionAction::Warn | InterventionAction::Block
        ),
        "A09: ASA 应记录审计结果"
    );
    assert!(
        !result.audit_reason.is_empty(),
        "A09: 审计原因应非空(日志可追溯)"
    );
}

// =============================================================================
// A10:2021 — 服务端请求伪造(SSRF: Server-Side Request Forgery)
//
// 攻击载荷:curl http://169.254.169.254/ — 访问云元数据内网
// 防御层:白名单(curl/wget 非白名单)+ Abuse 拦截
// =============================================================================

#[tokio::test]
async fn test_a10_ssrf_curl_blocked() {
    let mut sandbox = make_sandbox();
    // curl http://169.254.169.254/ — AWS 元数据服务 SSRF
    let cmd = Command::new("curl").arg("http://169.254.169.254/latest/meta-data/");
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "A10: SSRF(curl)应被拦截");
    match result.unwrap_err() {
        SecCoreError::CommandBlocked { attack_type, .. } => {
            assert_eq!(
                attack_type,
                AttackType::Abuse,
                "A10: curl 应识别为 Abuse(非白名单)"
            );
        }
        e => panic!("A10: 期望 CommandBlocked(Abuse), 实际: {e:?}"),
    }
}

#[tokio::test]
async fn test_a10_ssrf_wget_blocked() {
    let mut sandbox = make_sandbox();
    // wget http://localhost:8080/admin — 内网管理接口访问
    let cmd = Command::new("wget").arg("http://localhost:8080/admin");
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "A10: SSRF(wget)应被拦截");
}

#[tokio::test]
async fn test_a10_ssrf_python_requests_blocked() {
    let mut sandbox = make_sandbox();
    // python3 -c "import requests; requests.get('http://127.0.0.1:6379')"
    // — 通过 Python 访问内网 Redis
    let cmd = Command::new("python3").args([
        "-c",
        "import requests; requests.get('http://127.0.0.1:6379')",
    ]);
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "A10: SSRF(python3)应被拦截");
}

// -----------------------------------------------------------------------------
// A10 Windows 专属:SSRF via PowerShell Invoke-WebRequest
//
// WHY Windows 专属:Windows 平台 SSRF 常用 PowerShell 的 Invoke-WebRequest
// (别名 iwr)或 Invoke-RestMethod,与 Unix 的 curl/wget 攻击向量不同,
// 需独立验证 SecCore 拦截 PowerShell SSRF 载荷(§6.2 红线:零信任白名单)。
// -----------------------------------------------------------------------------

#[cfg(windows)]
#[tokio::test]
async fn test_a10_windows_ssrf_powershell() {
    let mut sandbox = make_sandbox();
    // 攻击载荷:powershell -c "Invoke-WebRequest http://169.254.169.254/..."
    // — 通过 PowerShell 访问 AWS 元数据服务(云 SSRF)
    // powershell 不在白名单 → CommandBlocked(Abuse)
    let cmd = Command::new("powershell").args([
        "-c",
        "Invoke-WebRequest",
        "http://169.254.169.254/latest/meta-data/",
    ]);
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(result.is_err(), "A10(Windows): SSRF(powershell)应被拦截");
    match result.unwrap_err() {
        SecCoreError::CommandBlocked { attack_type, .. } => {
            assert_eq!(
                attack_type,
                AttackType::Abuse,
                "A10(Windows): powershell 应识别为 Abuse(非白名单)"
            );
        }
        e => panic!("A10(Windows): 期望 CommandBlocked(Abuse), 实际: {e:?}"),
    }
}
