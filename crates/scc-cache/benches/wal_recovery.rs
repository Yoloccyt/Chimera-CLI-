//! WAL 崩溃恢复压测基准 — 1000 次崩溃恢复无数据丢失验证
//!
//! 对应架构层:L3 Storage
//! 对应任务:Week 8 Task 1.1(WAL 崩溃恢复压测)
//!
//! # 验证目标
//! - 1000 次崩溃恢复循环,每次:写入 N 条 → 模拟崩溃(drop)→ 重开 → recover → 验证数据完整
//! - 测量单次崩溃恢复的端到端延迟
//! - 确保零数据丢失(已 commit 的 entry 不在 recover 列表,未 commit 的 entry 全部恢复)
//!
//! # 运行
//! ```bash
//! # 完整基准(含 1000 次正确性验证 + 延迟测量)
//! cargo bench -p scc-cache --bench wal_recovery
//!
//! # 快速模式(缩短 warmup/measurement 时间)
//! cargo bench -p scc-cache --bench wal_recovery -- --warm-up-time 1 --measurement-time 3
//! ```
//!
//! # 关于 `--ignored`
//! Criterion 的 `harness = false` 不支持 `#[ignore]` 属性过滤。
//! 本基准在 `bench_crash_recovery` 入口处同步调用 `verify_1000_crash_recoveries()`,
//! 确保 1000 次崩溃恢复无数据丢失的验证在每次基准运行时都执行。
//!
//! # 架构红线
//! - `#![forbid(unsafe_code)]`:与 workspace 所有 crate 一致(40/40 覆盖不可破坏)
//! - 单函数 ≤ 200 行
//! - 测试/bench 代码可用 expect()/unwrap(),生产代码禁止

#![forbid(unsafe_code)]

use std::time::Instant;

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use scc_cache::wal::{SqliteWal, WalEntry, WalOperation, WalTrait};
use tempfile::{tempdir, TempDir};

/// 单次崩溃恢复周期:写入 N 条 → drop(模拟崩溃)→ 重开 → recover → 验证完整性
///
/// # 参数
/// - `wal_path`:SQLite WAL 文件路径
/// - `num_entries`:每次写入的 entry 总数(其中前一半 commit,后一半不 commit)
///
/// # 返回
/// - `Duration`:recover() 调用的端到端延迟
///
/// # 断言
/// - 未 commit 的 entry 全部出现在 recover 列表
/// - 已 commit 的 entry 不出现在 recover 列表
/// - 每条恢复 entry 的 payload 字段完整
///
/// # WHY 单函数 ≤ 80 行:遵循 §6 架构红线(单函数 ≤ 200 行)
fn single_crash_recovery_cycle(wal_path: &str, num_entries: usize) -> std::time::Duration {
    let half = num_entries / 2;

    // 阶段 1:写入 N 条 entry,commit 前一半,然后 drop(模拟崩溃)
    let committed_count = {
        let wal = SqliteWal::new(wal_path).expect("SqliteWal 创建应成功");
        for i in 0..num_entries {
            let id = format!("entry-{i}");
            let entry = WalEntry::new(
                &id,
                WalOperation::Insert,
                format!("ctx-{i}"),
                vec![i as u8; 16],
            );
            wal.write_ahead_log(&entry).expect("write_ahead_log 应成功");
            if i < half {
                wal.commit_log(&id).expect("commit_log 应成功");
            }
        }
        half
        // wal 在此 drop,模拟进程崩溃(未 commit 的 entry 留在 wal_entries 表)
    };

    // 阶段 2:重开同一文件,调用 recover(测量延迟)
    let start = Instant::now();
    let wal = SqliteWal::new(wal_path).expect("SqliteWal 重开应成功");
    let recovered = wal.recover().expect("recover 应成功");
    let elapsed = start.elapsed();

    // 阶段 3:验证数据完整性(零丢失)
    let expected_uncommitted = num_entries - committed_count;
    assert_eq!(
        recovered.len(),
        expected_uncommitted,
        "恢复条目数应为 {expected_uncommitted}, got {}",
        recovered.len()
    );

    for entry in &recovered {
        // 已 commit 的 entry 不应出现
        let idx: usize = entry
            .entry_id
            .strip_prefix("entry-")
            .and_then(|s| s.parse().ok())
            .expect("entry_id 应可解析为索引");
        assert!(
            idx >= half,
            "已 commit 的 entry-{idx} 不应出现在 recover 列表"
        );
        // payload 完整性
        assert_eq!(
            entry.payload,
            vec![idx as u8; 16],
            "entry-{idx} payload 应完整恢复"
        );
        // operation 字段
        assert_eq!(
            entry.operation,
            WalOperation::Insert,
            "entry-{idx} operation 应为 Insert"
        );
    }

    elapsed
}

/// 验证 1000 次崩溃恢复无数据丢失
///
/// # 流程
/// 每次循环:
/// 1. 创建新的临时目录(避免文件锁冲突)
/// 2. 写入 10 条 entry,commit 前 5 条
/// 3. drop SqliteWal(模拟崩溃)
/// 4. 重开同一文件,调用 recover
/// 5. 验证恢复 5 条未 commit entry,payload 完整
///
/// # WHY 1000 次:对应任务要求"1000 次崩溃恢复无数据丢失"
/// # WHY 每次新临时目录:Windows 上 SQLite WAL 文件锁可能导致重开失败
fn verify_1000_crash_recoveries() {
    const CYCLES: usize = 1000;
    const ENTRIES_PER_CYCLE: usize = 10;

    for cycle in 0..CYCLES {
        let dir = tempdir().expect("创建临时目录失败");
        let db_path = dir.path().join(format!("crash-{cycle}.db"));
        let path = db_path.to_str().expect("路径转 str 失败");

        let elapsed = single_crash_recovery_cycle(path, ENTRIES_PER_CYCLE);

        // 性能断言:单次崩溃恢复应 < 100ms(SQLite WAL 模式下通常 < 10ms)
        // WHY 100ms 阈值:留足余量,避免 CI 环境抖动导致误报
        assert!(
            elapsed.as_millis() < 100,
            "cycle {cycle} 崩溃恢复耗时 {}ms 超过 100ms 阈值",
            elapsed.as_millis()
        );

        black_box(elapsed);
        // TempDir 在此 drop,自动清理临时文件
    }
}

/// Criterion 基准:单次崩溃恢复延迟
///
/// # 流程
/// 1. 入口处先运行 1000 次正确性验证(确保零数据丢失)
/// 2. 使用 `iter_batched` 测量单次崩溃恢复周期的端到端延迟
/// 3. 每次 iter 创建新的临时目录,避免文件锁冲突
fn bench_crash_recovery(c: &mut Criterion) {
    // 步骤 1:1000 次崩溃恢复正确性验证(对应任务要求)
    verify_1000_crash_recoveries();

    // 步骤 2:延迟测量
    c.bench_function("crash_recovery_single", |b| {
        b.iter_batched(
            || {
                // setup:每次创建新的临时目录(避免文件锁冲突)
                let dir: TempDir = tempdir().expect("创建临时目录失败");
                let db_path = dir.path().join("bench-crash.db");
                let path = db_path.to_str().expect("路径转 str 失败").to_string();
                (path, dir)
            },
            |(path, _dir): (String, TempDir)| {
                // 测量:单次崩溃恢复周期(10 条 entry,5 条 commit)
                let elapsed = single_crash_recovery_cycle(&path, 10);
                black_box(elapsed);
            },
            BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, bench_crash_recovery);
criterion_main!(benches);
