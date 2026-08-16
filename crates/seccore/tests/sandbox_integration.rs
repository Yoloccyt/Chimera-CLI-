//! Sandbox 沙箱集成测试 — 从 src/sandbox.rs 内联测试模块外移(L4-P2-1)
//!
//! 外移说明:原 #[cfg(test)] mod tests 混在生产文件(531 行,占 37%),
//! 外移后 sandbox.rs 仅保留生产代码。覆盖:命令注入/环境泄露拦截、
//! gVisor 文件系统隔离、进程隔离、升级通道档位分类、SandboxViolation 事件发布。
//! 私有 API 白盒测试(handle_escalation/post_execution_audit)保留在
//! src/sandbox.rs private_api_tests 模块(访问 pub(crate) 方法)。
use std::time::Duration;

use event_bus::{EventBus, NexusEvent};
use seccore::types::{AttackType, Command, EscalationTier, GvisorConfig};
use seccore::{GvisorRuntime, Sandbox, SecCoreError};

#[tokio::test]
async fn test_sandbox_blocks_injection() {
    let mut sandbox = Sandbox::with_default_policy();
    let cmd = Command::new("echo").arg("$(whoami)");
    let result = sandbox.audit_and_execute(cmd).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_sandbox_blocks_env_leak() {
    let mut sandbox = Sandbox::with_default_policy();
    let cmd = Command::new("echo").arg("hello").env("SECRET_KEY", "leak");
    let result = sandbox.audit_and_execute(cmd).await;
    assert!(result.is_err());
}

// ============================================================
// SubTask 13.1: 文件系统隔离测试
// ============================================================

/// 测试 gVisor 模式下执行 cat /etc/shadow 被策略拦截。
///
/// WHY: `/etc/shadow` 匹配 CommandPolicy 的 DataLeak 拦截模式,
/// 验证沙箱阻止敏感文件访问,即使 cat 在白名单中。
#[tokio::test]
async fn test_gvisor_filesystem_isolation_blocks_etc_shadow() {
    let mut sandbox = Sandbox::with_default_policy();
    let cmd = Command::new("cat").arg("/etc/shadow");
    let result = sandbox.audit_and_execute(cmd).await;
    assert!(result.is_err(), "cat /etc/shadow 应被策略拦截");
    // 验证拦截类型为 DataLeak(敏感文件访问)
    match result {
        Err(SecCoreError::CommandBlocked { attack_type, .. }) => {
            assert_eq!(
                attack_type,
                AttackType::DataLeak,
                "拦截类型应为 DataLeak,而非其他攻击类型"
            );
        }
        other => panic!("期望 CommandBlocked(DataLeak), 实际: {:?}", other),
    }
}

/// 测试 runsc 不可用时沙箱降级为进程隔离,策略仍然生效。
///
/// WHY: 使用不存在的 runsc 路径调用 with_gvisor_config,
/// GvisorRuntime::detect 返回 None 导致 gvisor_runtime 未注入,
/// 但 use_gvisor 保持 true(意图启用),执行时自动降级为进程隔离,
/// 策略层仍应拦截 cat /etc/shadow。
#[tokio::test]
async fn test_gvisor_fallback_blocks_shadow_when_runsc_unavailable() {
    let mut sandbox = Sandbox::with_default_policy();
    let config = GvisorConfig {
        runsc_path: "/nonexistent/runsc".to_string(),
        ..GvisorConfig::default()
    };
    sandbox = sandbox.with_gvisor_config(&config);
    // use_gvisor 仍为 true(意图启用 gVisor),但 gvisor_runtime 为 None
    assert!(
        sandbox.use_gvisor,
        "runsc 不存在时 use_gvisor 仍应为 true(意图启用)"
    );
    // 策略层仍应拦截 cat /etc/shadow(降级后策略不失效)
    let cmd = Command::new("cat").arg("/etc/shadow");
    let result = sandbox.audit_and_execute(cmd).await;
    assert!(
        result.is_err(),
        "runsc 不可用时策略层仍应拦截 cat /etc/shadow"
    );
}

// ============================================================
// SubTask 13.2: 网络隔离测试
// ============================================================

/// 测试 GvisorConfig::default() 中 network_disabled 为 true。
///
/// WHY: gVisor 默认配置禁用网络,防止沙箱内进程发起外连,
/// 降低数据泄露风险。此测试验证默认值的正确性。
#[test]
fn test_gvisor_config_network_disabled_by_default() {
    let config = GvisorConfig::default();
    assert!(
        config.network_disabled,
        "GvisorConfig 默认应禁用网络(network_disabled=true)"
    );
}

/// 测试 GvisorRuntime::spawn() 包含 --network=none 参数。
///
/// WHY: spawn 方法硬编码 --network=none 参数以禁用网络访问,
/// 此测试通过静态源码验证确保该参数未被意外移除。
/// 结合 GvisorConfig::default().network_disabled=true,
/// 两者共同保证 gVisor 模式下网络被完全禁用。
#[test]
fn test_gvisor_runtime_spawn_includes_network_none() {
    // 验证 GvisorConfig 默认值与 spawn 方法行为一致
    let config = GvisorConfig::default();
    assert!(config.network_disabled);
    // 静态验证:gvisor.rs spawn() 方法硬编码了 --network=none 参数
    // 通过 include_str! 在编译期读取源码验证参数存在
    // 外移后 include_str 相对 tests/ 目录,需回指 src/gvisor.rs
    let gvisor_source = include_str!("../src/gvisor.rs");
    assert!(
        gvisor_source.contains("--network=none"),
        "GvisorRuntime::spawn() 必须包含 --network=none 参数以禁用网络"
    );
}

// ============================================================
// SubTask 13.3: 进程空间隔离测试
// ============================================================

/// 测试沙箱内 kill -9 1 被策略拦截。
///
/// WHY: `kill` 不在 CommandPolicy 白名单中,应被判定为 Abuse(未授权命令)。
/// 验证沙箱阻止进程间信号操作,实现进程空间隔离。
#[tokio::test]
async fn test_process_isolation_blocks_kill_command() {
    let mut sandbox = Sandbox::with_default_policy();
    let cmd = Command::new("kill").arg("-9").arg("1");
    let result = sandbox.audit_and_execute(cmd).await;
    assert!(result.is_err(), "kill 命令应被策略拦截");
    // 验证拦截类型为 Abuse(kill 不在白名单)
    match result {
        Err(SecCoreError::CommandBlocked { attack_type, .. }) => {
            assert_eq!(
                attack_type,
                AttackType::Abuse,
                "kill 不在白名单,拦截类型应为 Abuse"
            );
        }
        other => panic!("期望 CommandBlocked(Abuse), 实际: {:?}", other),
    }
}

/// 测试高风险命令的 EscalationTier 正确分级。
///
/// WHY: D6 修复引入 risk_score 数值评分,不同评分映射到不同 EscalationTier:
/// - ReadOnly(0-30):只读命令直接执行
/// - Normal(31-70):常规命令直接执行
/// - Parliament(71-90):高危命令需议会辩论
/// - EscalateToHuman(91-100):灾难性命令必须人工决策
///
/// 此测试覆盖所有档位边界值,确保分级逻辑正确。
#[test]
fn test_escalation_tier_correct_classification() {
    // ReadOnly 档 (0-30):只读命令
    assert_eq!(
        EscalationTier::from_score(10),
        EscalationTier::ReadOnly,
        "risk_score=10 应为 ReadOnly 档"
    );
    assert_eq!(
        EscalationTier::from_score(30),
        EscalationTier::ReadOnly,
        "risk_score=30(边界) 应为 ReadOnly 档"
    );

    // Normal 档 (31-70):常规命令
    assert_eq!(
        EscalationTier::from_score(31),
        EscalationTier::Normal,
        "risk_score=31(边界) 应为 Normal 档"
    );
    assert_eq!(
        EscalationTier::from_score(50),
        EscalationTier::Normal,
        "risk_score=50 应为 Normal 档"
    );
    assert_eq!(
        EscalationTier::from_score(70),
        EscalationTier::Normal,
        "risk_score=70(边界) 应为 Normal 档"
    );

    // Parliament 档 (71-90):高危命令,需议会辩论
    assert_eq!(
        EscalationTier::from_score(71),
        EscalationTier::Parliament,
        "risk_score=71(边界) 应为 Parliament 档"
    );
    assert_eq!(
        EscalationTier::from_score(80),
        EscalationTier::Parliament,
        "risk_score=80(rm 命令) 应为 Parliament 档"
    );
    assert_eq!(
        EscalationTier::from_score(90),
        EscalationTier::Parliament,
        "risk_score=90(边界) 应为 Parliament 档"
    );

    // EscalateToHuman 档 (91-100):灾难性命令,必须人工决策
    assert_eq!(
        EscalationTier::from_score(91),
        EscalationTier::EscalateToHuman,
        "risk_score=91(边界) 应为 EscalateToHuman 档"
    );
    assert_eq!(
        EscalationTier::from_score(95),
        EscalationTier::EscalateToHuman,
        "risk_score=95(dd 命令) 应为 EscalateToHuman 档"
    );
    assert_eq!(
        EscalationTier::from_score(100),
        EscalationTier::EscalateToHuman,
        "risk_score=100(边界) 应为 EscalateToHuman 档"
    );

    // 防御性:超 100 的评分归入 EscalateToHuman(异常输入不漏过人工升级)
    assert_eq!(
        EscalationTier::from_score(101),
        EscalationTier::EscalateToHuman,
        "risk_score=101(超范围) 应防御性归入 EscalateToHuman 档"
    );
}

// ============================================================
// SubTask 13.4: 降级路径测试
// ============================================================

/// 测试 Sandbox::with_gvisor_config() 在 runsc 不可用时不注入运行时。
///
/// WHY: 当 runsc 路径不存在时,GvisorRuntime::detect 返回 None,
/// gvisor_runtime 保持 None,但 use_gvisor 仍为 true(意图启用),
/// execute_in_sandbox 内部自动降级为进程隔离。
#[test]
fn test_sandbox_with_gvisor_config_nonexistent_runsc() {
    let sandbox = Sandbox::with_default_policy();
    let config = GvisorConfig {
        runsc_path: "/nonexistent/path/to/runsc".to_string(),
        ..GvisorConfig::default()
    };
    let sandbox = sandbox.with_gvisor_config(&config);
    // use_gvisor 应保持 true(意图启用 gVisor,执行时自动降级)
    assert!(
        sandbox.use_gvisor,
        "runsc 不存在时 use_gvisor 仍应为 true(意图启用)"
    );
    // gvisor_runtime 为 None(内部状态,通过 execute_in_sandbox 的降级行为间接验证)
    // 注: gvisor_runtime 是私有字段,后续 audit_and_execute 测试验证降级无 panic
}

/// 测试 Sandbox::with_gvisor_runtime() 正确注入运行时。
///
/// WHY: 通过 GvisorRuntime::detect 检测一个存在的文件路径(模拟 runsc),
/// 验证运行时被成功注入到 Sandbox,use_gvisor 保持 true。
#[test]
fn test_sandbox_with_gvisor_runtime_injects_correctly() {
    // 使用 Cargo.toml(测试运行时必然存在)模拟 runsc 路径
    // GvisorRuntime::detect 仅检查文件存在性,不验证是否为真实 runsc
    let runtime = GvisorRuntime::detect("Cargo.toml");
    assert!(runtime.is_some(), "Cargo.toml 应存在,detect 应返回 Some");
    let sandbox = Sandbox::with_default_policy().with_gvisor_runtime(runtime.unwrap());
    // use_gvisor 应保持 true
    assert!(
        sandbox.use_gvisor,
        "注入 GvisorRuntime 后 use_gvisor 应为 true"
    );
    // 注: gvisor_runtime 是私有字段,但 with_gvisor_runtime 将 Some 值移入,
    // 通过 execute_in_sandbox 的三层检测逻辑验证注入生效
}

/// 测试 Sandbox::with_gvisor(false) 禁用 gVisor 后不尝试检测。
///
/// WHY: 显式禁用 gVisor 后,execute_in_sandbox 跳过 gVisor 可用性检测,
/// 直接使用进程隔离执行。适用于测试环境或受控内网场景。
#[test]
fn test_sandbox_with_gvisor_false_disables_detection() {
    let sandbox = Sandbox::with_default_policy().with_gvisor(false);
    assert!(
        !sandbox.use_gvisor,
        "with_gvisor(false) 应将 use_gvisor 设为 false"
    );
}

/// 测试 Sandbox::new() 默认 use_gvisor=true 且 gvisor_runtime=None。
///
/// WHY: 新创建的 Sandbox 默认启用 gVisor 意图(use_gvisor=true),
/// 但未注入运行时实例(gvisor_runtime=None)。
/// 这意味着 execute_in_sandbox 会检测到 gvisor_runtime 为 None,
/// 自动降级为进程隔离,确保在不配置 gVisor 时也能安全工作。
#[test]
fn test_sandbox_new_defaults_gvisor_enabled_no_runtime() {
    let sandbox = Sandbox::with_default_policy();
    assert!(
        sandbox.use_gvisor,
        "Sandbox::new() 默认 use_gvisor=true(意图启用 gVisor)"
    );
    // gvisor_runtime 默认为 None(私有字段,通过 execute_in_sandbox 的降级
    // 行为间接验证:use_gvisor=true 但 gvisor_runtime=None 时,
    // gvisor_available = false,自动走进程隔离路径)
}

// ============================================================
// P2-4: SandboxViolation 事件发布测试
// ============================================================

/// 命令拦截发布 SandboxViolation 事件(Injection)
#[tokio::test]
async fn test_blocked_command_publishes_sandbox_violation() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut sandbox = Sandbox::with_default_policy().with_event_bus(bus);

    let cmd = Command::new("echo").arg("$(whoami)");
    let result = sandbox.audit_and_execute(cmd).await;
    assert!(result.is_err(), "注入命令应被拦截");

    let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("应收到 SandboxViolation")
        .expect("recv 不应失败");
    match event {
        NexusEvent::SandboxViolation {
            violation_type,
            detail,
            ..
        } => {
            assert!(
                violation_type.contains("Injection"),
                "违规类型应为 Injection, 实际: {violation_type}"
            );
            assert!(!detail.is_empty(), "违规详情不应为空");
        }
        other => panic!("应收到 SandboxViolation: {other:?}"),
    }
}

/// 环境变量拦截发布 SandboxViolation 事件(env_blocked)
#[tokio::test]
async fn test_blocked_env_publishes_sandbox_violation() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut sandbox = Sandbox::with_default_policy().with_event_bus(bus);

    let cmd = Command::new("echo").arg("hello").env("SECRET_KEY", "leak");
    let result = sandbox.audit_and_execute(cmd).await;
    assert!(result.is_err(), "环境变量泄露应被拦截");

    let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("应收到 SandboxViolation")
        .expect("recv 不应失败");
    assert!(
        matches!(&event, NexusEvent::SandboxViolation { violation_type, .. }
                if violation_type == "env_blocked"),
        "应收到 env_blocked 违规, 实际: {event:?}"
    );
}

/// 未注入 EventBus 时发布静默跳过(向后兼容,既有测试零改动)
#[tokio::test]
async fn test_no_event_bus_skips_publish() {
    let mut sandbox = Sandbox::with_default_policy(); // 无 with_event_bus
    let cmd = Command::new("echo").arg("$(whoami)");
    let result = sandbox.audit_and_execute(cmd).await;
    assert!(result.is_err(), "拦截仍应生效(不依赖事件发布)");
}
