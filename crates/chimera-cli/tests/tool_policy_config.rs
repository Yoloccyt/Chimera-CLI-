//! 策略规则声明式配置源验收（F-01/U-16/U-19，ADR-246）
//!
//! # WHY 需要这个文件
//!
//! ADR-240 把四环链接入组合根时，策略环的规则集与默认动作来自
//! `tool_runtime_policy()` 的**硬编码空集** —— 策略环在生产路径形同虚设
//! （永远放行）。ADR-246 为策略环补上声明式配置源：`ChimeraConfig.tool_policy`
//! 段（nexus-core 纯类型 + chimera-cli Figment 加载 + 组合根映射到 execpolicy）。
//!
//! 本文件从 crate 外部边界验证三层契约：
//! 1. **缺省不削减（A5）**：无 `tool_policy` 段时派生结果与硬编码时代逐位一致
//!    —— `(vec![], Allow)`；缺省姿态下内置工具照常可执行。
//! 2. **声明生效**：显式 `deny` 规则经组合根装配后，策略环真实拦截匹配工具
//!    （归因 `DispatchError::Policy`，先于沙箱/传输环短路）；未匹配工具不受影响。
//! 3. **边界诚实**：非法 action 在配置加载边界硬失败（可读错误）；空 program
//!    的规则被跳过并告警（fail-soft per-rule，不拖垮整份配置）；env 覆盖
//!    （figment 既有 `CHIMERA_` 前缀链）对 default_action 真实生效。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use chimera_cli::composition::{build, tool_runtime_policy};
use chimera_cli::ChimeraConfig;
use gqep_executor::tool_runtime::{DispatchError, DispatchRequest};
use nexus_core::config::{ToolPolicyAction, ToolPolicyConfig, ToolPolicyRuleDef};

/// 构造带指定策略段的配置（其余段取缺省）。
fn config_with_policy(section: ToolPolicyConfig) -> ChimeraConfig {
    ChimeraConfig {
        tool_policy: section,
        ..ChimeraConfig::default()
    }
}

/// 快捷构造 deny 规则（pattern 语法见 ADR-246：`"<program> [args_glob]"`）。
fn deny_rule(pattern: &str) -> ToolPolicyRuleDef {
    ToolPolicyRuleDef {
        pattern: pattern.to_string(),
        action: ToolPolicyAction::Deny,
    }
}

/// 写入临时 YAML 并在用例结束后清理（进程级唯一文件名，避免并发互踩）。
fn write_temp_yaml(tag: &str, content: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "chimera_tool_policy_{tag}_{}.yaml",
        std::process::id()
    ));
    std::fs::write(&path, content).expect("写临时配置文件应成功");
    path
}

// ============================================================
// 1. 缺省不削减（A5）
// ============================================================

/// 容器级 `#[serde(default)]` 回填的 `tool_policy` 段必须等价于
/// 硬编码时代的 `(vec![], Allow)` —— 这是 ADR-246 的兼容姿态底线。
#[test]
fn tool_policy_section_defaults_to_allow_and_empty() {
    let section = ChimeraConfig::default().tool_policy;
    assert_eq!(
        section.default_action,
        ToolPolicyAction::Allow,
        "缺省 default_action 必须是 Allow（零信任 Deny 会削减既有功能）"
    );
    assert!(
        section.rules.is_empty(),
        "缺省规则集必须为空（无声明即无拦截）"
    );
}

/// `tool_runtime_policy` 对缺省配置的派生结果与硬编码时代逐位一致。
#[test]
fn tool_runtime_policy_default_stays_allow() {
    let (rules, action) = tool_runtime_policy(&ChimeraConfig::default());
    assert!(rules.is_empty(), "缺省配置不得派生出任何规则");
    assert_eq!(
        action,
        seccore::execpolicy::PolicyAction::Allow,
        "缺省配置的默认动作必须保持 Allow"
    );
}

// ============================================================
// 2. 声明式规则 → execpolicy 映射
// ============================================================

/// 声明规则经组合根映射为 execpolicy 规则：program 大小写不敏感、
/// `"<program> <args_glob>"` 双段语法、先命中先裁决、缺省动作独立于规则。
#[test]
fn declarative_rules_map_to_execpolicy() {
    let cfg = config_with_policy(ToolPolicyConfig {
        default_action: ToolPolicyAction::Allow,
        rules: vec![deny_rule("rm"), deny_rule("git push")],
    });
    let (rules, action) = tool_runtime_policy(&cfg);
    assert_eq!(action, seccore::execpolicy::PolicyAction::Allow);
    assert_eq!(rules.len(), 2, "两条合法规则都应派生");

    let policy = rules.into_iter().fold(
        seccore::execpolicy::ExecPolicy::with_default_action(action),
        |p, r| p.add_rule(r),
    );
    // program 大小写不敏感（RulePattern::new 归一小写）
    assert_eq!(
        policy.evaluate("RM", &["-rf /".to_string()]),
        seccore::execpolicy::PolicyAction::Deny,
        "program 匹配必须大小写不敏感"
    );
    // 双段语法："git push" → program=git, args_glob=push
    assert_eq!(
        policy.evaluate("git", &["push".to_string()]),
        seccore::execpolicy::PolicyAction::Deny,
        "args_glob 应对参数命中"
    );
    assert_eq!(
        policy.evaluate("git", &["status".to_string()]),
        seccore::execpolicy::PolicyAction::Allow,
        "未命中 glob 应回默认动作（规则必须外科手术式）"
    );
    // 省略 args_glob 等价 "*"：任意参数都命中
    assert_eq!(
        policy.evaluate("rm", &[]),
        seccore::execpolicy::PolicyAction::Deny,
        "单段 pattern 应匹配任意参数（含无参调用）"
    );
}

// ============================================================
// 3. 组合根集成：声明在四环链策略环真实生效
// ============================================================

/// 显式 deny 规则经 `build()` 装配后，`text.echo` 在策略环被拦截
/// （归因 `DispatchError::Policy`，先于沙箱/传输环短路）。
///
/// WHY 这是 F-01 的核心断言：此前策略环只有硬编码空集，任何规则都不可能
/// 在生产装配中生效。本测试红了说明映射层或装配层断链。
#[tokio::test]
async fn declarative_deny_blocks_tool_at_policy_ring() {
    let cfg = config_with_policy(ToolPolicyConfig {
        default_action: ToolPolicyAction::Allow,
        rules: vec![deny_rule("text.echo")],
    });
    let ctx = build(&cfg).expect("组合根装配应成功");
    let req = DispatchRequest {
        tool_name: "text.echo".to_string(),
        args_json: r#"{"text":"hi"}"#.to_string(),
    };
    match ctx.tool_runtime.dispatch(&req).await {
        Err(DispatchError::Policy(msg)) => assert!(
            msg.contains("denied"),
            "策略拒绝文案需保留 `denied` 关键词（审计/审批流路由锚点），实际: {msg}"
        ),
        other => panic!("期望策略环拦截（DispatchError::Policy），实际: {other:?}"),
    }
}

/// deny 规则必须外科手术式：未匹配工具不受影响（防「一条规则 = 全局拒绝」回归）。
#[tokio::test]
async fn deny_rule_is_surgical() {
    let cfg = config_with_policy(ToolPolicyConfig {
        default_action: ToolPolicyAction::Allow,
        rules: vec![deny_rule("repo.search")],
    });
    let ctx = build(&cfg).expect("组合根装配应成功");
    let req = DispatchRequest {
        tool_name: "text.echo".to_string(),
        args_json: r#"{"text":"hi"}"#.to_string(),
    };
    let outcome = ctx
        .tool_runtime
        .dispatch(&req)
        .await
        .expect("未被 deny 规则匹配的内置工具应照常执行");
    assert!(
        outcome.content_json.contains("hi"),
        "text.echo 应回显参数，实际: {}",
        outcome.content_json
    );
}

// ============================================================
// 4. 配置边界诚实
// ============================================================

/// 非法 action 值必须在 `load()` 边界硬失败（可读错误），不得静默回退
/// —— 「写了 deny 却没生效」比「加载失败」危险得多。
#[test]
fn invalid_action_fails_config_load() {
    let path = write_temp_yaml(
        "bad_action",
        "tool_policy:\n  rules:\n    - pattern: \"x\"\n      action: \"Bogus\"\n",
    );
    let result = chimera_cli::config::load(Some(path.clone()));
    let _ = std::fs::remove_file(&path);
    let err = result.expect_err("非法 action 必须在配置边界硬失败");
    // WHY 遍历错误链:anyhow 外层只有 `加载配置失败:<路径>` context,
    // 「unknown variant」根因在 cause 链内(figment 包装 serde 错误)。
    let chain: Vec<String> = err.chain().map(|c| c.to_string()).collect();
    let joined = chain.join("\n");
    assert!(
        joined.contains("unknown variant") || joined.contains("Bogus"),
        "错误链需可定位到非法值，实际链: {joined}"
    );
}

/// env 覆盖（figment 既有 `CHIMERA_` 前缀 + `__` 分隔链）对 default_action
/// 真实生效；大小写别名（`Deny`/`deny`）均合法（serde alias，YAML/env 同权）。
#[test]
fn env_and_yaml_case_alias_override_default_action() {
    // env 路径：大写 `Deny`（alias）
    let path = write_temp_yaml("env_override", "# minimal config\n");
    // 仅本文件内的两个 load() 用例读 env；本变量只影响 default_action 键，
    // 不影响另一用例的「非法 action 必失败」断言（键正交）。
    std::env::set_var("CHIMERA_TOOL_POLICY__DEFAULT_ACTION", "Deny");
    let loaded = chimera_cli::config::load(Some(path.clone()));
    std::env::remove_var("CHIMERA_TOOL_POLICY__DEFAULT_ACTION");
    let _ = std::fs::remove_file(&path);
    let cfg = loaded.expect("最小配置 + env 覆盖应加载成功");
    assert_eq!(
        cfg.tool_policy.default_action,
        ToolPolicyAction::Deny,
        "env 覆盖必须真实生效（显式声明才允许 Deny，这是 ADR-246 的语义）"
    );

    // YAML 路径：大写 `Deny`（alias）与缺省 rules 同样合法
    let path = write_temp_yaml(
        "yaml_alias",
        "tool_policy:\n  default_action: \"Deny\"\n  rules: []\n",
    );
    let loaded = chimera_cli::config::load(Some(path.clone()));
    let _ = std::fs::remove_file(&path);
    let cfg = loaded.expect("大写 Deny 经 serde alias 应合法");
    assert_eq!(cfg.tool_policy.default_action, ToolPolicyAction::Deny);
}

/// 空 program 的规则在组合根映射时被跳过（fail-soft per-rule + warn），
/// 其余规则照常派生 —— 一条笔误不拖垮整份策略，但也不会静默吞掉整段。
#[test]
fn empty_program_rule_is_skipped() {
    let cfg = config_with_policy(ToolPolicyConfig {
        default_action: ToolPolicyAction::Allow,
        rules: vec![
            ToolPolicyRuleDef {
                pattern: "   ".to_string(),
                action: ToolPolicyAction::Deny,
            },
            deny_rule("rm"),
        ],
    });
    let (rules, _) = tool_runtime_policy(&cfg);
    assert_eq!(rules.len(), 1, "空 program 规则应被跳过，有效规则必须保留");
}
