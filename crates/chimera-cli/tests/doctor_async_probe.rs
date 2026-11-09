//! doctor 探针的异步 IO 验收（ADR-241 W3-T1）
//!
//! # WHY 这个文件
//!
//! `check_sqlite_path` 此前在 `async fn` 内直接调用 `std::fs::write` /
//! `std::fs::remove_file`。该探针经 `doctor.rs` 的 `gather_collected` 与
//! 其他探针**并发**执行 —— 同步文件 IO 会**阻塞所在 Tokio worker 线程**，
//! 使并发探针的墙钟收益被抵消（ADR-241 P-08）。
//!
//! 本文件锁定五件事：
//! 1. **源码层**：该探针内零 `std::fs::` 调用（用 `tokio::fs` 取代）；
//! 2. **功能层**：可写父目录仍返回 `Ok`，不可写仍返回 `Warn`（行为未变）；
//! 3. **并发层**：探针执行期间**不占用** Tokio worker —— 用一个同 runtime
//!    的独立任务证明它能在探针 IO 期间继续推进。
//! 4. **模块级（ADR-272 补盲）**：`probes.rs` 内零阻塞路径探测 —— 判据从
//!    「不含 `std::fs::`」扩为「且不含 `.exists(`」，覆盖全部 5 个路径探针；
//! 5. **诚实层（ADR-272）**：LLM 探针成功文案必须自陈 mock，且结论确定。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::path::Path;

use chimera_cli::ChimeraConfig;

/// 探针源码内零同步文件 IO。
///
/// WHY 用源码扫描而非运行时观测：运行时观测（"IO 期间 worker 是否空闲"）
/// 依赖调度器实现细节，是**弱判据**；源码层断言是**强判据** —— 任何
/// `std::fs::` 的回归都会立刻被抓住。
#[test]
fn sqlite_path_probe_has_no_sync_fs_calls() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("commands")
        .join("doctor")
        .join("probes.rs");
    let text = std::fs::read_to_string(&path).expect("应能读取 probes.rs");

    // 截取 check_sqlite_path 函数体（到下一个 `pub(super) async fn` 或 `pub(super) fn`）
    let start = text
        .find("async fn check_sqlite_path")
        .expect("probes.rs 应含 check_sqlite_path");
    let rest = &text[start..];
    let end = rest
        .find("pub(super) async fn")
        .or_else(|| rest.find("pub(super) fn"))
        .unwrap_or(rest.len());
    let body = &rest[..end];

    assert!(
        !body.contains("std::fs::"),
        "check_sqlite_path 内不得有同步文件 IO（应改用 tokio::fs）；实际命中:\n{body}"
    );
    assert!(
        body.contains("tokio::fs::write"),
        "check_sqlite_path 应用 tokio::fs::write 做可写性探测"
    );
}

/// 功能不变：可写父目录 → `Ok`；不存在的父目录 → `Warn`。
///
/// WHY 这条是「功能不削减」的直接证据：把同步 IO 换成异步 IO 是**行为等价**
/// 的重构，判定结果与消息不得改变。
#[tokio::test]
async fn sqlite_path_probe_behavior_unchanged() {
    // 可写父目录（tempdir 必然可写）
    let tmp = tempfile::tempdir().expect("应能创建临时目录");
    let mut cfg = ChimeraConfig::default();
    cfg.repo_wiki.db_path = tmp.path().join("db.sqlite").display().to_string();

    let check = chimera_cli::commands::doctor::probes_check_sqlite_path(&cfg).await;
    assert_eq!(check.name, "sqlite_path");
    assert!(
        check.message.contains("可写"),
        "可写父目录应判定为可写，实际: {}",
        check.message
    );

    // 不存在的父目录 → Warn（不是故障）
    let missing = tmp.path().join("definitely-absent-dir");
    let mut cfg2 = ChimeraConfig::default();
    cfg2.repo_wiki.db_path = missing.join("db.sqlite").display().to_string();

    let check2 = chimera_cli::commands::doctor::probes_check_sqlite_path(&cfg2).await;
    assert_eq!(check2.name, "sqlite_path");
    assert!(
        check2.message.contains("不存在"),
        "父目录不存在应降级为 Warn 并说明原因，实际: {}",
        check2.message
    );
}

/// 并发层：探针的 IO 不占用 Tokio worker。
///
/// WHY 这条有意义：同步 `std::fs::write` 会在 worker 上**阻塞整个线程**；
/// 异步 `tokio::fs` 把 IO 交给阻塞池，worker 立即回到调度器。
/// 本测试用「探针运行期间另一个任务能获得执行机会」来观察这个差异。
///
/// 判据设计：并发跑 N 个探针 + 1 个「心跳」任务。心跳任务必须能在探针
/// 全部完成**之前**取得至少一次推进 —— 若探针阻塞了 worker，心跳会被推迟到
/// 探针全部结束之后（此时断言会因计数为 0 而红）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sqlite_path_probe_does_not_block_worker() {
    let tmp = tempfile::tempdir().expect("应能创建临时目录");
    let db_path = tmp.path().join("db.sqlite").display().to_string();

    let mut cfg = ChimeraConfig::default();
    cfg.repo_wiki.db_path = db_path.clone();

    // 心跳任务：在探针仍在进行时推进计数
    let heartbeat = tokio::spawn(async move {
        let mut ticks = 0u32;
        for _ in 0..50 {
            tokio::task::yield_now().await;
            ticks += 1;
        }
        ticks
    });

    // 并发探针
    let mut probes = Vec::new();
    for _ in 0..8 {
        let cfg = cfg.clone();
        probes.push(tokio::spawn(async move {
            chimera_cli::commands::doctor::probes_check_sqlite_path(&cfg).await
        }));
    }

    let ticks = heartbeat.await.expect("心跳任务不应 panic");

    for probe in probes {
        let check = probe.await.expect("探针任务不应 panic");
        assert_eq!(check.name, "sqlite_path");
    }

    assert!(
        ticks > 0,
        "探针 IO 期间心跳任务应能取得执行机会（若为 0，说明探针阻塞了 worker）"
    );
}

/// 门补盲（ADR-272）：`probes.rs` **整个模块**零阻塞路径存在性探测。
///
/// WHY 从「只禁 `std::fs::`」扩到「禁 `.exists(`」：`Path::exists` 是阻塞 stat，
/// 而旧判据只锚 `std::fs::` —— 于是 `probes.rs` 里 7 处 `Path::exists` 全部在门内
/// **恒绿**（门绿由缺陷撑着，F-07）。现判据覆盖 5 个路径探针：
/// config_file / cargo_lock / sqlite_path / storage_health / model_asset_health。
#[test]
fn probes_module_has_no_sync_path_exists() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("commands")
        .join("doctor")
        .join("probes.rs");
    let text = std::fs::read_to_string(&path).expect("应能读取 probes.rs");

    assert!(
        !text.contains(".exists("),
        "probes.rs 内不得出现 `Path::exists`（应改用 tokio::fs::try_exists）"
    );
    assert!(
        !text.contains("std::fs::"),
        "probes.rs 内不得有同步文件 IO（应改用 tokio::fs）"
    );
    assert!(
        text.matches("tokio::fs::try_exists").count() >= 5,
        "5 个路径探针都应走 tokio::fs::try_exists，实际命中 {} 处",
        text.matches("tokio::fs::try_exists").count()
    );
}

/// 诚实化（ADR-272）：LLM 探针的成功文案必须自陈 mock，且结论确定。
///
/// 旧实现 sleep 238ms + 纳秒奇偶 50/50：每次 doctor 必然多付 238ms，
/// 且成功文案写成固定 `ping` 延迟，等于把 mock 伪装成真实 RTT。
#[tokio::test]
async fn llm_probe_message_declares_mock_and_is_deterministic() {
    let cfg = ChimeraConfig::default();
    let a = chimera_cli::commands::doctor::probes_check_llm_provider(&cfg).await;
    let b = chimera_cli::commands::doctor::probes_check_llm_provider(&cfg).await;

    assert_eq!(a.name, "llm_provider");
    assert_eq!(a.status, b.status, "结论必须确定（旧实现 50/50 随机）");
    assert_eq!(a.message, b.message, "文案必须逐字确定");
    assert!(
        a.message.contains("mock"),
        "必须自陈 mock，实际: {}",
        a.message
    );
    assert!(
        a.message.contains("未探测"),
        "必须声明连通性未探测，实际: {}",
        a.message
    );
    assert!(
        !a.message.contains("ping"),
        "不得伪装真实 RTT，实际: {}",
        a.message
    );
}
