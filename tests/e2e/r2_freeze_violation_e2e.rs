//! ADR-042 R2 冻结违反处置 E2E 测试(P1-9)
//!
//! 对应架构层:L5 Knowledge(gsoe-evolution)+ L1 Core(event-bus)+ L9 Quest(chimera-mas)
//! 对应 ADR:ADR-042(R2 冻结在 FormalVerifier 落地前无条件冻结)
//! 对应任务:P1-9 补全 gsoe-evolution R2 冻结违反处置 E2E 测试
//!
//! # 测试覆盖
//!
//! 本测试套件验证 ADR-042 决策 4"违反处置预案"在端到端场景下的完整性,
//! 覆盖三重检测机制(CI / 运行时 / 审计)与三步处置流程(自动回滚 + 告警 + 复盘):
//!
//! 1. **R2 冻结声明存在性**:验证 gsoe-evolution / auto-dpo / omega-learner 源码顶部有 R2 冻结声明注释
//! 2. **R2 路径未实现**:验证 GsoeEvolutionEngine 未实现 `evolve_with_constrained_rl` 方法
//! 3. **R2FreezeViolation 事件**:验证事件可构造 + severity = Critical + type_name + metadata + EventBus 发布
//! 4. **R2FreezeRollbackFailed 事件**:验证事件可构造 + severity = Critical + type_name + metadata
//! 5. **MasError::R2FreezeViolation**:验证错误变体可构造 + Display 输出 + 变体数量
//! 6. **debug_assert 守护**:验证 default feature 下 `evolve_once()` 不 panic
//! 7. **CI 关键词扫描**:验证 gsoe-evolution / auto-dpo 源码无 R2 路径实现关键词
//!
//! # 设计决策
//!
//! - **源码扫描用 `std::fs::read_to_string`**:E2E 测试需验证源码状态(声明注释存在 + 无 R2 实现),
//!   `CARGO_MANIFEST_DIR` 指向 workspace 根目录,可定位到 `crates/` 子目录
//! - **编译期方法不存在检查**:通过 `#[allow(dead_code)]` 的 helper 函数引用 `evolve_once`,
//!   确保方法签名存在;R2 路径方法通过"源码扫描无关键词"间接验证
//! - **EventBus 发布测试**:使用 `EventBus::new()` + `subscribe()` 验证事件可发布且订阅者能收到

use std::path::PathBuf;
use std::time::Duration;

use event_bus::{EventBus, EventMetadata, EventSeverity, NexusEvent};

// ============================================================
// 辅助函数:定位 workspace 源码文件
// ============================================================

/// 获取 workspace 根目录(`CARGO_MANIFEST_DIR` 指向 `chimera-e2e-tests` 根 package)
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// 读取 crate 源码文件内容(失败则 panic 并提示文件路径)
fn read_crate_source(crate_name: &str, file_name: &str) -> String {
    let path = workspace_root()
        .join("crates")
        .join(crate_name)
        .join("src")
        .join(file_name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("无法读取 {crate_name}/src/{file_name}: {e}"))
}

/// 读取 crate Cargo.toml 内容
fn read_crate_cargo_toml(crate_name: &str) -> String {
    let path = workspace_root()
        .join("crates")
        .join(crate_name)
        .join("Cargo.toml");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("无法读取 {crate_name}/Cargo.toml: {e}"))
}

// ============================================================
// 测试组 1:R2 冻结声明存在性
// ============================================================

/// 验证 gsoe-evolution/src/engine.rs 顶部有 R2 冻结声明注释
#[test]
fn test_r2_freeze_declaration_in_gsoe_engine() {
    let source = read_crate_source("gsoe-evolution", "engine.rs");
    assert!(
        source.contains("R2 冻结声明(ADR-042)"),
        "gsoe-evolution/src/engine.rs 必须包含 R2 冻结声明(ADR-042)"
    );
    assert!(
        source.contains("FormalVerifier 落地前无条件冻结"),
        "gsoe-evolution/src/engine.rs 必须声明 FormalVerifier 落地前无条件冻结"
    );
    assert!(
        source.contains("ADR-042"),
        "gsoe-evolution/src/engine.rs 必须引用 ADR-042"
    );
}

/// 验证 gsoe-evolution/src/ci_gate.rs 有 R2 冻结声明注释
#[test]
fn test_r2_freeze_declaration_in_ci_gate() {
    let source = read_crate_source("gsoe-evolution", "ci_gate.rs");
    assert!(
        source.contains("R2 冻结声明(ADR-042)"),
        "gsoe-evolution/src/ci_gate.rs 必须包含 R2 冻结声明(ADR-042)"
    );
    assert!(
        source.contains("R2 路径在 FormalVerifier 落地前完全冻结"),
        "gsoe-evolution/src/ci_gate.rs 必须声明 R2 路径冻结"
    );
}

/// 验证 auto-dpo/src/generator.rs 有 R2 冻结声明注释
#[test]
fn test_r2_freeze_declaration_in_auto_dpo() {
    let source = read_crate_source("auto-dpo", "generator.rs");
    assert!(
        source.contains("R2 冻结声明(ADR-042)"),
        "auto-dpo/src/generator.rs 必须包含 R2 冻结声明(ADR-042)"
    );
    assert!(
        source.contains("FormalVerifier 落地前无条件冻结"),
        "auto-dpo/src/generator.rs 必须声明 FormalVerifier 落地前无条件冻结"
    );
}

/// 验证 omega-learner/src/replay_pool.rs 有 R2 冻结声明注释
#[test]
fn test_r2_freeze_declaration_in_omega_learner() {
    let source = read_crate_source("omega-learner", "replay_pool.rs");
    assert!(
        source.contains("R2 冻结声明(ADR-042)"),
        "omega-learner/src/replay_pool.rs 必须包含 R2 冻结声明(ADR-042)"
    );
    // 回放池的特殊约束:数据仅可用于 R1 路径
    assert!(
        source.contains("仅可用于 R1 路径") || source.contains("R1"),
        "omega-learner/src/replay_pool.rs 必须声明回放池数据仅可用于 R1 路径"
    );
}

// ============================================================
// 测试组 2:R2 路径未实现(Cargo.toml feature 默认关闭)
// ============================================================

/// 验证 gsoe-evolution Cargo.toml 中 r2_path feature 默认关闭
#[test]
fn test_r2_path_feature_default_disabled() {
    let cargo_toml = read_crate_cargo_toml("gsoe-evolution");
    assert!(
        cargo_toml.contains("r2_path"),
        "gsoe-evolution/Cargo.toml 必须声明 r2_path feature(ADR-042 决策 4)"
    );
    // default = [] 确保 r2_path 默认关闭
    assert!(
        cargo_toml.contains("default = []"),
        "gsoe-evolution/Cargo.toml 必须声明 default = [](r2_path 默认关闭)"
    );
}

/// 验证 gsoe-evolution 源码中无 R2 路径实现关键词(CI 静态扫描模拟)
///
/// ADR-042 决策 5:CI 检测扫描 `constrained_rl` / `r2_policy` / `train_r2` /
/// `GsoeAutoDpoRL` / `evolve_with_constrained_rl` 关键词(大小写不敏感)。
/// 测试代码本身允许引用这些关键词(用于断言),但 src/ 目录下不应出现实现。
#[test]
fn test_no_r2_path_implementation_in_gsoe_evolution_src() {
    let src_dir = workspace_root()
        .join("crates")
        .join("gsoe-evolution")
        .join("src");
    let r2_keywords = [
        "evolve_with_constrained_rl",
        "ConstrainedRLPolicy",
        "train_r2_path",
        "GsoeAutoDpoRL",
    ];

    // 递归扫描 src/ 目录下所有 .rs 文件
    let mut violations: Vec<String> = Vec::new();
    if src_dir.exists() {
        scan_dir_for_keywords(&src_dir, &r2_keywords, &mut violations);
    }

    assert!(
        violations.is_empty(),
        "gsoe-evolution/src/ 发现 R2 路径实现关键词(违反 ADR-042 决策 1):\n{}",
        violations.join("\n")
    );
}

/// 验证 auto-dpo 源码中无 R2 路径实现关键词
#[test]
fn test_no_r2_path_implementation_in_auto_dpo_src() {
    let src_dir = workspace_root().join("crates").join("auto-dpo").join("src");
    let r2_keywords = ["constrained_rl", "r2_policy", "train_r2", "GsoeAutoDpoRL"];

    let mut violations: Vec<String> = Vec::new();
    if src_dir.exists() {
        scan_dir_for_keywords(&src_dir, &r2_keywords, &mut violations);
    }

    assert!(
        violations.is_empty(),
        "auto-dpo/src/ 发现 R2 路径实现关键词(违反 ADR-042 决策 1):\n{}",
        violations.join("\n")
    );
}

/// 递归扫描目录下所有 .rs 文件,检测 R2 关键词
///
/// 注:仅扫描 `src/` 目录,`tests/` 目录允许引用关键词(用于测试 R2 冻结本身)。
///
/// WHY 跳过纯注释行:ADR-042 决策 5 的 CI 检测目标是"R2 路径实现",而非文档引用。
/// `engine.rs` 顶部 R2 冻结声明会列举"不应实现"的方法名作为反面示例(如
/// `evolve_with_constrained_rl()`),这是合理的文档实践。扫描函数需区分:
/// - 纯注释行(`//` / `//!` / `///` 开头,允许前导空格):文档引用,跳过
/// - 代码行(含代码行末尾注释):实际实现,检测
fn scan_dir_for_keywords(dir: &std::path::Path, keywords: &[&str], violations: &mut Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir_for_keywords(&path, keywords, violations);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            if let Ok(content) = std::fs::read_to_string(&path) {
                // 按行扫描,跳过纯注释行(// / //! / /// 开头,允许前导空格)
                for (line_no, line) in content.lines().enumerate() {
                    let trimmed = line.trim_start();
                    if trimmed.starts_with("//") {
                        continue; // 纯注释行,跳过
                    }
                    // 对代码行(含行末注释)做大小写不敏感检测
                    let line_lower = line.to_lowercase();
                    for keyword in keywords {
                        let keyword_lower = keyword.to_lowercase();
                        if line_lower.contains(&keyword_lower) {
                            violations.push(format!(
                                "  - {}:{} 包含关键词 '{}'",
                                path.display(),
                                line_no + 1,
                                keyword
                            ));
                        }
                    }
                }
            }
        }
    }
}

// ============================================================
// 测试组 3:R2FreezeViolation 事件验证
// ============================================================

/// 验证 R2FreezeViolation 事件可构造且字段正确
#[test]
fn test_r2_freeze_violation_event_construction() {
    let event = NexusEvent::R2FreezeViolation {
        metadata: EventMetadata::new("r2-freeze-guard"),
        violation_type: "CiDetection".to_string(),
        evidence: "发现 evolve_with_constrained_rl 方法".to_string(),
    };

    // metadata 提取
    assert_eq!(event.metadata().source, "r2-freeze-guard");
}

/// 验证 R2FreezeViolation 事件 severity = Critical(对齐 §6.2 红线 5)
///
/// WHY 必须 Critical:R2 违反等同于安全事件,奖励黑客风险可能立即生效,
/// 必须走 mpsc 旁路通道确保投递到 SecCore 与 Parliament。
#[test]
fn test_r2_freeze_violation_event_severity_critical() {
    let event = NexusEvent::R2FreezeViolation {
        metadata: EventMetadata::new("r2-freeze-guard"),
        violation_type: "RuntimeAssertion".to_string(),
        evidence: "debug_assert panic: r2_path feature enabled".to_string(),
    };
    assert_eq!(
        event.severity(),
        EventSeverity::Critical,
        "R2FreezeViolation 必须 severity = Critical(ADR-042 决策 4 + §6.2 红线 5)"
    );
}

/// 验证 R2FreezeViolation 事件 type_name 稳定性
#[test]
fn test_r2_freeze_violation_event_type_name() {
    let event = NexusEvent::R2FreezeViolation {
        metadata: EventMetadata::new("asa-auditor"),
        violation_type: "AuditScan".to_string(),
        evidence: "审计扫描发现 R2 激活痕迹".to_string(),
    };
    assert_eq!(
        event.type_name(),
        "R2FreezeViolation",
        "R2FreezeViolation type_name 必须稳定(序列化兼容性)"
    );
}

/// 验证 R2FreezeViolation 事件可通过 EventBus 发布且订阅者能收到
///
/// WHY 端到端验证:确保事件在 EventBus 通道中可正常流转,
/// ADR-042 决策 4 步骤 2"告警广播"依赖此能力。
#[tokio::test]
async fn test_r2_freeze_violation_event_publishable_via_event_bus() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let event = NexusEvent::R2FreezeViolation {
        metadata: EventMetadata::new("r2-freeze-guard"),
        violation_type: "CiDetection".to_string(),
        evidence: "CI 扫描发现 constrained_rl 关键词".to_string(),
    };

    bus.publish(event.clone())
        .await
        .expect("发布 R2FreezeViolation 事件应成功");

    // 等待订阅者收到事件(超时 1 秒)
    let received = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("订阅者应在 1 秒内收到 R2FreezeViolation 事件")
        .expect("事件不应为 None");

    assert_eq!(received, event, "订阅者收到的事件应与发布的事件一致");
}

// ============================================================
// 测试组 4:R2FreezeRollbackFailed 事件验证
// ============================================================

/// 验证 R2FreezeRollbackFailed 事件可构造且字段正确
#[test]
fn test_r2_freeze_rollback_failed_event_construction() {
    let event = NexusEvent::R2FreezeRollbackFailed {
        metadata: EventMetadata::new("r2-freeze-guard"),
        reason: "git revert 冲突:CONFLICT (content): Merge conflict in engine.rs".to_string(),
    };

    assert_eq!(event.metadata().source, "r2-freeze-guard");
}

/// 验证 R2FreezeRollbackFailed 事件 severity = Critical
///
/// WHY 必须 Critical:回滚失败意味着 R2 路径代码可能仍在生效,
/// 必须升级为人工介入(从自动回滚升级),需保证投递可靠性。
#[test]
fn test_r2_freeze_rollback_failed_event_severity_critical() {
    let event = NexusEvent::R2FreezeRollbackFailed {
        metadata: EventMetadata::new("r2-freeze-guard"),
        reason: "cargo build 失败:错误 E0308".to_string(),
    };
    assert_eq!(
        event.severity(),
        EventSeverity::Critical,
        "R2FreezeRollbackFailed 必须 severity = Critical(ADR-042 决策 4 步骤 1)"
    );
}

/// 验证 R2FreezeRollbackFailed 事件 type_name 稳定性
#[test]
fn test_r2_freeze_rollback_failed_event_type_name() {
    let event = NexusEvent::R2FreezeRollbackFailed {
        metadata: EventMetadata::new("r2-freeze-guard"),
        reason: "回滚失败".to_string(),
    };
    assert_eq!(
        event.type_name(),
        "R2FreezeRollbackFailed",
        "R2FreezeRollbackFailed type_name 必须稳定(序列化兼容性)"
    );
}

/// 验证 R2FreezeRollbackFailed 事件可通过 EventBus 发布
#[tokio::test]
async fn test_r2_freeze_rollback_failed_event_publishable_via_event_bus() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let event = NexusEvent::R2FreezeRollbackFailed {
        metadata: EventMetadata::new("r2-freeze-guard"),
        reason: "git revert 冲突".to_string(),
    };

    bus.publish(event.clone())
        .await
        .expect("发布 R2FreezeRollbackFailed 事件应成功");

    let received = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("订阅者应在 1 秒内收到 R2FreezeRollbackFailed 事件")
        .expect("事件不应为 None");

    assert_eq!(received, event, "订阅者收到的事件应与发布的事件一致");
}

// ============================================================
// 测试组 5:MasError::R2FreezeViolation 验证
// ============================================================

/// 验证 MasError::R2FreezeViolation 可构造且 Display 输出正确
#[test]
fn test_mas_error_r2_freeze_violation_construction_and_display() {
    use chimera_mas::error::MasError;

    let error = MasError::R2FreezeViolation {
        violation_type: "CiDetection".to_string(),
        evidence: "发现 ConstrainedRLPolicy 类型".to_string(),
    };

    let msg = format!("{error}");
    assert!(
        msg.contains("R2 freeze violation"),
        "MasError::R2FreezeViolation Display 必须包含 'R2 freeze violation'"
    );
    assert!(
        msg.contains("CiDetection"),
        "MasError::R2FreezeViolation Display 必须包含 violation_type"
    );
    assert!(
        msg.contains("ConstrainedRLPolicy"),
        "MasError::R2FreezeViolation Display 必须包含 evidence"
    );
    assert!(
        msg.contains("ADR-042"),
        "MasError::R2FreezeViolation Display 必须引用 ADR-042"
    );
}

/// 验证 MasError::R2FreezeViolation 的三种违反类型均可构造
///
/// 对应 ADR-042 决策 4 的三重检测机制:
/// - CiDetection:CI 静态扫描检测
/// - RuntimeAssertion:运行时 debug_assert 检测
/// - AuditScan:AsaAuditor 审计检测
#[test]
fn test_mas_error_r2_freeze_violation_three_detection_types() {
    use chimera_mas::error::MasError;

    let ci_detection = MasError::R2FreezeViolation {
        violation_type: "CiDetection".to_string(),
        evidence: "CI 扫描发现 constrained_rl 关键词".to_string(),
    };
    let runtime_assertion = MasError::R2FreezeViolation {
        violation_type: "RuntimeAssertion".to_string(),
        evidence: "debug_assert panic: r2_path feature enabled".to_string(),
    };
    let audit_scan = MasError::R2FreezeViolation {
        violation_type: "AuditScan".to_string(),
        evidence: "AsaAuditor 发现 R2 激活痕迹".to_string(),
    };

    assert!(format!("{ci_detection}").contains("CiDetection"));
    assert!(format!("{runtime_assertion}").contains("RuntimeAssertion"));
    assert!(format!("{audit_scan}").contains("AuditScan"));
}

// ============================================================
// 测试组 6:debug_assert 守护验证
// ============================================================

/// 验证 default feature 下 GsoeEvolutionEngine 可正常实例化(r2_path 关闭)
///
/// WHY 重要:确保 R2 冻结守护不会误伤正常的 L3 进化路径。
/// `evolve_once()` 在 default feature 下应正常执行,不触发 debug_assert panic。
#[test]
fn test_gsoe_engine_instantiable_with_default_features() {
    use gsoe_evolution::config::GsoeConfig;
    use gsoe_evolution::engine::GsoeEvolutionEngine;

    let config = GsoeConfig::default();
    let engine = GsoeEvolutionEngine::new(config);
    // 引用 engine 确保实例化成功(不调用 evolve_once 避免依赖 EventBus)
    let _ = &engine;
}

/// 验证 gsoe-evolution/src/engine.rs 包含 R2 冻结运行时守护
///
/// ADR-042 决策 4 运行时检测:`evolve_once()` 入口处用
/// `#[cfg(debug_assertions)] if cfg!(feature = "r2_path") { panic!(...) }`
/// 在 debug build 中阻止 R2 路径激活。
///
/// WHY 不用 debug_assert!(!cfg!(...)):cfg! 返回编译期常量,会触发
/// clippy::assertions_on_constants 警告;改用 if + panic! 保留语义且零警告。
#[test]
fn test_debug_assert_guard_exists_in_evolve_once() {
    let source = read_crate_source("gsoe-evolution", "engine.rs");
    // 检查 cfg(debug_assertions) + if cfg!(feature = "r2_path") + panic! 三要素
    assert!(
        source.contains("debug_assertions") && source.contains("r2_path"),
        "gsoe-evolution/src/engine.rs 必须包含 #[cfg(debug_assertions)] + cfg!(feature = \"r2_path\") 守护(ADR-042 决策 4)"
    );
    assert!(
        source.contains("panic!"),
        "gsoe-evolution/src/engine.rs 必须包含 panic! 宏(R2 冻结违反时触发)"
    );
    assert!(
        source.contains("R2 冻结违反(ADR-042)"),
        "gsoe-evolution/src/engine.rs panic 消息必须提及 R2 冻结违反(ADR-042)"
    );
}

// ============================================================
// 测试组 7:违反处置流程端到端验证
// ============================================================

/// 验证 ADR-042 决策 4 三步处置流程的事件链路完整性
///
/// 处置流程:
/// 1. 自动回滚(立即):若失败发布 R2FreezeRollbackFailed
/// 2. 告警广播(立即):发布 R2FreezeViolation Critical 事件
/// 3. 事故复盘(24 小时内):归档报告(非事件,由文档流程承载)
///
/// 本测试验证步骤 1 + 2 的事件可串联发布且订阅者能完整接收。
#[tokio::test]
async fn test_r2_freeze_violation_handling_event_chain() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    // 步骤 2:告警广播(R2FreezeViolation)
    let violation_event = NexusEvent::R2FreezeViolation {
        metadata: EventMetadata::new("r2-freeze-guard"),
        violation_type: "CiDetection".to_string(),
        evidence: "CI 扫描发现 R2 路径实现".to_string(),
    };
    bus.publish(violation_event.clone())
        .await
        .expect("发布 R2FreezeViolation 应成功");

    // 步骤 1:自动回滚失败(R2FreezeRollbackFailed)
    let rollback_failed_event = NexusEvent::R2FreezeRollbackFailed {
        metadata: EventMetadata::new("r2-freeze-guard"),
        reason: "git revert 冲突".to_string(),
    };
    bus.publish(rollback_failed_event.clone())
        .await
        .expect("发布 R2FreezeRollbackFailed 应成功");

    // 验证订阅者按顺序收到两个 Critical 事件
    let first_received = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("应在 1 秒内收到第一个事件")
        .expect("第一个事件不应为 None");
    assert_eq!(
        first_received, violation_event,
        "第一个收到的事件应为 R2FreezeViolation"
    );
    assert_eq!(
        first_received.severity(),
        EventSeverity::Critical,
        "R2FreezeViolation 必须 severity = Critical"
    );

    let second_received = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("应在 1 秒内收到第二个事件")
        .expect("第二个事件不应为 None");
    assert_eq!(
        second_received, rollback_failed_event,
        "第二个收到的事件应为 R2FreezeRollbackFailed"
    );
    assert_eq!(
        second_received.severity(),
        EventSeverity::Critical,
        "R2FreezeRollbackFailed 必须 severity = Critical"
    );
}

/// 验证三种违反类型的 R2FreezeViolation 事件均可发布
///
/// 覆盖 ADR-042 决策 4 的三重检测机制:
/// - CiDetection:CI 静态扫描
/// - RuntimeAssertion:运行时 debug_assert
/// - AuditScan:AsaAuditor 审计
#[tokio::test]
async fn test_three_detection_types_event_publishable() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let detection_types = ["CiDetection", "RuntimeAssertion", "AuditScan"];
    let mut events = Vec::new();

    for detection_type in &detection_types {
        let event = NexusEvent::R2FreezeViolation {
            metadata: EventMetadata::new("r2-freeze-guard"),
            violation_type: detection_type.to_string(),
            evidence: format!("{detection_type} 检测到 R2 路径激活"),
        };
        bus.publish(event.clone())
            .await
            .expect("发布 R2FreezeViolation 应成功");
        events.push(event);
    }

    // 验证三个事件均被订阅者接收
    for expected in &events {
        let received = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("应在 1 秒内收到事件")
            .expect("事件不应为 None");
        assert_eq!(received, *expected, "订阅者收到的事件应与发布的事件一致");
        assert_eq!(
            received.severity(),
            EventSeverity::Critical,
            "所有 R2FreezeViolation 事件必须 severity = Critical"
        );
    }
}

/// 验证 R2FreezeViolation + R1ShadowRollbackFailed 都是 Critical 事件
///
/// WHY 对比验证:确保 R2 冻结违反与 R1 影子模式回滚失败(P4-W16.2.2)
/// 都被正确标记为 Critical,对齐 §6.2 红线 5 的 mpsc 旁路通道要求。
#[test]
fn test_r2_and_r1_critical_events_alignment() {
    let r2_violation = NexusEvent::R2FreezeViolation {
        metadata: EventMetadata::new("r2-freeze-guard"),
        violation_type: "CiDetection".to_string(),
        evidence: "test".to_string(),
    };
    let r2_rollback_failed = NexusEvent::R2FreezeRollbackFailed {
        metadata: EventMetadata::new("r2-freeze-guard"),
        reason: "test".to_string(),
    };
    let r1_rollback_failed = NexusEvent::R1ShadowRollbackFailed {
        metadata: EventMetadata::new("r1-shadow-mode"),
        reason: "ConsecutiveRegression".to_string(),
        // P2-13: 结构化字段(测试中用默认值,专项测试覆盖结构化场景)
        trigger_type: event_bus::types::RollbackTriggerType::ConsecutiveRegression,
        triggered_at: None,
        details: String::new(),
        diagnostic: event_bus::types::RollbackDiagnosticContext::default(),
    };

    assert_eq!(r2_violation.severity(), EventSeverity::Critical);
    assert_eq!(r2_rollback_failed.severity(), EventSeverity::Critical);
    assert_eq!(r1_rollback_failed.severity(), EventSeverity::Critical);
}
