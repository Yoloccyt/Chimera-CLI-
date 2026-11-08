//! P3 / ADR-258 集成验收 — hook.* 双轨注册面闭合（deepdive §8.4）。
//!
//! 覆盖面：组合根注册（14 个 hook.* 生命周期事件）、幂等、触发实例 roundtrip、
//! 失败形态不反噬主流程、subscribe-before-spawn 纪律反向断言。
//!
//! 运行：`cargo test -p chimera-cli --features r2_island_repayment --test hook_wiring`
#![cfg(feature = "r2_island_repayment")]
#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use chimera_cli::composition::build;
use chimera_cli::ChimeraConfig;
use nexus_contracts::event_v2::{EventNamespace, EventTypeId};
use nexus_hook::audit::HookAuditEntry;
use nexus_hook::lifecycle::LifecycleEvent;

/// 构造审计条目（HookAuditEntry 全 pub 字段直构——nexus-hook 的 make_entry
/// 为 pub(crate)，集成测试侧用同形字面量）。
fn entry(cmd: &str, interrupted: bool, denied: bool) -> HookAuditEntry {
    HookAuditEntry {
        event: LifecycleEvent::PreToolUse,
        command: cmd.to_string(),
        exit_code: Some(0),
        duration_ms: 5,
        interrupted,
        sandbox_denied: denied,
    }
}

/// 集成 1：组合根注册面闭合——ctx.hook_registry 挂载且 Hook 命名空间 14 项。
#[tokio::test]
async fn composition_build_wires_hook_registry() {
    let ctx = build(&ChimeraConfig::default()).expect("组合根装配应成功");
    let hooks = ctx.hook_registry.list_by_namespace(EventNamespace::Hook);
    assert_eq!(hooks.len(), 14, "14 个 hook.* 生命周期事件全部注册");
    assert!(
        ctx.hook_registry
            .get(&EventTypeId::new("hook.pre_tool_use"))
            .is_some(),
        "pre_tool_use 可查"
    );
}

/// 集成 2：幂等——build 两次互不干扰（各自独立 registry）;同一 registry 内
/// 重复 register_all 不增计数（Replaced 语义,deepdive §8.3）。
#[tokio::test]
async fn hook_register_idempotent_in_composition() {
    let ctx1 = build(&ChimeraConfig::default()).expect("第一次装配应成功");
    let ctx2 = build(&ChimeraConfig::default()).expect("第二次装配应成功");
    assert_eq!(
        ctx1.hook_registry
            .list_by_namespace(EventNamespace::Hook)
            .len(),
        14
    );
    assert_eq!(
        ctx2.hook_registry
            .list_by_namespace(EventNamespace::Hook)
            .len(),
        14
    );
    // 同 registry 重复注册（幂等,同 ID 覆盖为 Replaced）
    nexus_hook::event_bridge::HookEventBridge::register_all(&ctx1.hook_registry);
    assert_eq!(
        ctx1.hook_registry
            .list_by_namespace(EventNamespace::Hook)
            .len(),
        14,
        "重复 register_all 不增计数"
    );
}

/// 集成 3：触发实例 roundtrip——审计条目 → trigger_from_audit → register →
/// get → serialize 含 `sandbox_denied`/`command` 字段。
#[tokio::test]
async fn hook_trigger_event_roundtrip() {
    let ctx = build(&ChimeraConfig::default()).expect("组合根装配应成功");
    let e = nexus_hook::event_bridge::HookEventBridge::trigger_from_audit(
        "sess-hook-1",
        &entry("git stash", false, false),
    );
    ctx.hook_registry.register(std::sync::Arc::new(e.clone()));
    let fetched = ctx
        .hook_registry
        .get(&EventTypeId::new("hook.pre_tool_use"))
        .expect("注册后可查");
    let text = String::from_utf8(fetched.serialize().expect("序列化成功")).expect("JSON 合法");
    assert!(text.contains("sandbox_denied"), "载荷含沙箱字段: {text}");
    assert!(text.contains("command"), "载荷含命令字段: {text}");
    assert!(text.contains("git stash"), "载荷含命令值: {text}");
}

/// 集成 4：失败形态不反噬主流程——超时熔断（exit_code=None）/沙箱拒绝形态的
/// 审计条目经 trigger_from_audit → register 仍闭合（失败姿态 = 载荷重要性
/// 提升与 warn 计数,主流程 build() 恒 Ok）。
#[tokio::test]
async fn hook_failure_does_not_poison_main_flow() {
    let mut failed = entry("cmd /c pwn", true, true);
    failed.exit_code = None; // 超时熔断形态
    let ctx = build(&ChimeraConfig::default()).expect("失败形态下装配仍应 Ok");
    let e = nexus_hook::event_bridge::HookEventBridge::trigger_from_audit("s", &failed);
    ctx.hook_registry.register(std::sync::Arc::new(e));
    let fetched = ctx
        .hook_registry
        .get(&EventTypeId::new("hook.pre_tool_use"))
        .expect("失败形态条目仍注册可查");
    let text = String::from_utf8(fetched.serialize().expect("序列化成功")).expect("JSON");
    assert!(
        text.contains("\"sandbox_denied\":true") && text.contains("\"interrupted\":true"),
        "失败形态载荷如实记录: {text}"
    );
    assert!(
        fetched.importance().value() >= 0.9,
        "失败形态重要性提升（残留/压缩决策输入）"
    );
}

/// 集成 5（反向）：组合根 hook 接线块为纯同步构造+注册——无 spawn 点 ⇒
/// subscribe-before-spawn 纪律天然成立（§8.3 约束落码要点）。
#[test]
fn subscribe_before_spawn_discipline() {
    let src = include_str!("../src/composition.rs");
    // 定位 hook 接线块（ADR-258 注释行起至 spawn_critical_subscriber 调用行止;
    // 锚点搜索自 start 起——spawn_critical_subscriber 在文件更早的文档注释中
    // 也有出现,须取接线块之后的首个命中）
    let start = src
        .find("ADR-258（P3/U-03）")
        .expect("组合根 hook 接线块应在场");
    let end = src[start..]
        .find("spawn_critical_subscriber")
        .expect("锚点应在场")
        + start;
    let window = &src[start..end];
    assert!(
        !window.contains("tokio::spawn"),
        "hook 接线块不得含 spawn（纯同步构造+注册,纪律反向断言）"
    );
    assert!(
        window.contains("HookEventBridge::register_all"),
        "接线块含 register_all 调用"
    );
}
