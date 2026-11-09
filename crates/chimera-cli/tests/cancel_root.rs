//! W4 / ADR-270 — 取消通道的**生产者**接线（Ctrl+C → `CancellationToken`）。
//!
//! # 背景
//! `call_budget::call_with_budget` 的取消分支（预先取消 + 在飞 `select!`）早已存在，
//! 但生产侧从未真正触发：`exec.rs` / `run.rs` 各自 `CancellationToken::new()` 后
//! 丢弃句柄 —— 令牌永远处于「未取消」，取消语义在生产上是死代码。
//!
//! # 三段判据
//! 1. **单例身份**：`cancel_root()` 多次调用返回同一实例（否则每个命令一个令牌，
//!    Ctrl+C 只能停掉其中一个）。
//! 2. **源码结构锁**：两个生产入口不再新建「永不触发」的令牌（信号 → cancel 的
//!    绑定在测试里无法真实触发，只能结构性锁住「用的是根令牌」）。
//! 3. **传播**：取消根后，在飞 `call_with_budget` 立即返回 `Cancelled`
//!    （证明消费者与生产者接在同一个令牌上）。
//!
//! 运行：`cargo test -p chimera-cli --test cancel_root`

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::path::Path;
use std::time::Duration;

use chimera_cli::call_budget::{call_with_budget, cancel_root, CallBudgetError};
use chimera_cli::composition::build;
use chimera_cli::ChimeraConfig;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_root_singleton_and_propagation() {
    // --- 1) 单例身份 ---
    let a = cancel_root() as *const _;
    let b = cancel_root() as *const _;
    assert_eq!(
        a, b,
        "取消根必须是进程级单例（否则 Ctrl+C 只能停掉部分命令）"
    );

    // --- 2) 源码结构锁 ---
    let ws = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for rel in [
        "crates/chimera-cli/src/commands/exec.rs",
        "crates/chimera-cli/src/commands/run.rs",
    ] {
        let text = std::fs::read_to_string(ws.join(rel))
            .unwrap_or_else(|e| panic!("读取 {rel} 应成功: {e}"));
        assert!(
            !text.contains("CancellationToken::new()"),
            "{rel} 仍在新建「永不触发」的令牌 —— 取消语义在该入口是死代码"
        );
        assert!(
            text.contains("cancel_root()"),
            "{rel} 应使用进程级取消根（ADR-270）"
        );
    }

    // --- 3) 传播 ---
    let ctx = build(&ChimeraConfig::default()).expect("组合根装配应成功");
    let token = cancel_root().clone();
    let handle = tokio::spawn(async move {
        let inner = async {
            // 远长于取消延迟：若取消未生效，本测试会等到超时断言处失败
            tokio::time::sleep(Duration::from_secs(30)).await;
            1u8
        };
        call_with_budget(
            inner,
            Duration::from_secs(60),
            &token,
            "test.cancel_root",
            &ctx.bus,
        )
        .await
    });

    // 让任务真正进入 poll（否则是「预先取消」分支，测不到在飞路径）
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancel_root().cancel();

    let joined = tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("取消应迅速生效（2s 内），实得超时")
        .expect("任务不应 panic");
    assert!(
        matches!(joined, Err(CallBudgetError::Cancelled { .. })),
        "在飞调用应被取消，实得: {joined:?}"
    );
}
