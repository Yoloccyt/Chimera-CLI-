//! Task 2.3 — 指标历史 SQLite 持久化(RED 阶段测试)
//!
//! 对应 spec:enterprise-tui-monitoring-task-viz §二·系统监控增强
//!
//! # 设计目标
//! 验证 `MetricsHistory` 满足三项硬性要求:
//! 1. **表创建幂等**:`new()` 多次调用不应冲突(用 `CREATE TABLE IF NOT EXISTS`)
//! 2. **`ON CONFLICT REPLACE` 幂等**:同 `(unix_ts, metric)` 重复插入只保留最后值
//! 3. **保留期清理**:`cleanup(retention_days)` 删除早于 `now - retention_days` 的行
//!
//! # TDD 流程
//! 当前为 RED 阶段:`MetricsHistory` 尚未实现,测试编译失败 / 找不到符号
//! (`MetricsHistory` 不存在 + 方法 `insert/query_range/cleanup` 不存在),
//! 编译失败即符合预期。GREEN 阶段实现 `data::metrics_history` 模块后转绿。
//!
//! # 关键约束(来自全局规则 §4.4)
//! - 所有 rusqlite 调用必须 `tokio::task::spawn_blocking`(绝不可同步阻塞 async runtime)
//! - 测试 DB 用 `tempfile::tempdir()` 隔离,避免污染用户 `~/.chimera/`
//! - 库层错误用 `TuiError`(`thiserror` enum),不引入 `anyhow`

#![forbid(unsafe_code)]

use chimera_tui::data::metrics_history::MetricsHistory;
use tempfile::tempdir;

// ============================================================
// 1) 表创建幂等测试 — 同一路径两次 open 不应失败
// ============================================================

/// 验证:同一 DB 路径多次 `MetricsHistory::new()` 不应失败(表已存在场景)
///
/// WHY: 守护进程重启、CRIU 恢复、container 复制等场景下,SQLite 文件
/// 已有 schema,`new()` 必须幂等(依赖 `CREATE TABLE IF NOT EXISTS`)。
/// 若 schema 创建非幂等,二次 open 会因 "table already exists" 失败。
#[tokio::test]
async fn test_create_table_idempotent() {
    let tmp = tempdir().expect("create temp dir");
    let db_path = tmp.path().join("history.db");

    // 第一次 open:创建表
    let first = MetricsHistory::new(&db_path)
        .await
        .expect("first open should succeed");
    drop(first);

    // 第二次 open:表已存在,应成功(不报 "table already exists")
    let second = MetricsHistory::new(&db_path)
        .await
        .expect("second open should succeed (CREATE TABLE IF NOT EXISTS)");

    // 插入一行,确认第二次 open 后 DB 可用
    second
        .insert(1_000_000, "cpu_usage", 0.42)
        .await
        .expect("insert after re-open should succeed");

    let rows = second
        .query_range("cpu_usage", 0, 2_000_000)
        .await
        .expect("query_range should succeed");
    assert_eq!(rows.len(), 1, "应能读到刚插入的 1 行");
    assert!((rows[0].value - 0.42).abs() < 1e-5);
}

// ============================================================
// 2) INSERT ... ON CONFLICT REPLACE 幂等测试
// ============================================================

/// 验证:同 `(unix_ts, metric)` 重复插入应替换为最新值(而非追加)
///
/// WHY: 资源监控采样可能因重试 / 重连产生重复时间戳;幂等写入保证
/// 同一时刻只有一条记录(避免历史曲线出现阶梯)。复合主键
/// `(unix_ts, metric)` 配合 `ON CONFLICT REPLACE` 是 SQLite 标准 UPSERT。
#[tokio::test]
async fn test_insert_with_on_conflict_replace() {
    let tmp = tempdir().expect("create temp dir");
    let db_path = tmp.path().join("history.db");
    let store = MetricsHistory::new(&db_path)
        .await
        .expect("open should succeed");

    let ts: u64 = 1_700_000_000_000; // 固定毫秒时间戳
    let metric = "cpu_usage";

    // 第一次插入:value = 0.30
    store
        .insert(ts, metric, 0.30)
        .await
        .expect("first insert should succeed");
    // 第二次插入:同 (ts, metric),value = 0.85 → 应 REPLACE
    store
        .insert(ts, metric, 0.85)
        .await
        .expect("second insert should succeed (ON CONFLICT REPLACE)");
    // 第三次插入:同 (ts, metric),value = 0.50 → 应再次 REPLACE
    store
        .insert(ts, metric, 0.50)
        .await
        .expect("third insert should succeed (ON CONFLICT REPLACE)");

    // 关键断言:同主键下只有 1 行(非 3 行)
    let rows = store
        .query_range(metric, ts.saturating_sub(1), ts + 1)
        .await
        .expect("query_range should succeed");
    assert_eq!(
        rows.len(),
        1,
        "ON CONFLICT REPLACE 应保证同主键只有 1 行,实际 {} 行",
        rows.len()
    );
    // 断言最终值为最后一次插入
    assert!(
        (rows[0].value - 0.50).abs() < 1e-9,
        "REPLACE 后值应为最后一次插入(0.50),实际 {}",
        rows[0].value
    );
}

// ============================================================
// 3) 保留期清理测试 — cleanup(retention_days) 删除过期行
// ============================================================

/// 验证:`cleanup(retention_days)` 删除早于 `now - retention_days * 86_400_000` 的行
///
/// WHY: spec §Requirement "监控历史持久化" 要求保留期由
/// `TuiConfig.metrics_history_retention_days` 控制(默认 7 天)。
/// 清理任务只删除过期行,保留期内的数据不动。
#[tokio::test]
async fn test_retention_cleanup_deletes_old_rows() {
    use std::time::{SystemTime, UNIX_EPOCH};

    let tmp = tempdir().expect("create temp dir");
    let db_path = tmp.path().join("history.db");
    let store = MetricsHistory::new(&db_path)
        .await
        .expect("open should succeed");

    // 时间基线必须用真实 `SystemTime::now()`,与 `MetricsHistory::cleanup()`
    // 内部读取的时钟保持一致(否则用硬编码 2023 时间戳会与 2026 真实时间
    // 错位,导致全部数据被判定为过期)。这是 TDD 中常被忽视的"隐式耦合":
    // 测试逻辑时钟 == 实现逻辑时钟。
    let now_ms: u64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after unix epoch")
        .as_millis() as u64;
    let one_day_ms: u64 = 86_400_000; // 24 * 60 * 60 * 1000

    // 3 行历史:10 天前 / 5 天前 / 1 天前
    let rows_to_insert: [(u64, &str, f64); 3] = [
        (now_ms - 10 * one_day_ms, "cpu_usage", 0.10), // 过期
        (now_ms - 5 * one_day_ms, "cpu_usage", 0.50),  // 保留(7 天内)
        (now_ms - one_day_ms, "cpu_usage", 0.80),      // 保留
    ];
    for (ts, metric, value) in rows_to_insert {
        store
            .insert(ts, metric, value)
            .await
            .expect("insert should succeed");
    }

    // 保留期 7 天:10 天前的行应被删除,5 天/1 天前的保留
    // WHY 用 `tokio::time::sleep` 推进 1ms:保证 5 天前/1 天前的行的 ts 严格 < now,
    // cleanup 用 SystemTime::now() 时才能正确计算过期边界。
    // 若不 sleep,某些高频时钟下 1 天前那行可能被误判为"未来时间"。
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let deleted = store.cleanup(7).await.expect("cleanup should succeed");
    assert_eq!(deleted, 1, "应删除 1 行过期数据(10 天前)");

    // 验证:全表只剩 2 行
    // WHY 上界用 `i64::MAX as u64` 而非 `u64::MAX`:
    //   实现的 query_range 内部把 `end` cast 到 `i64` 绑定参数,
    //   `u64::MAX as i64 == -1`,会让 SQL 谓词 `unix_ts <= -1` 永远不匹配
    //   (unix_ts 总是非负)。`i64::MAX as u64 = 9_223_372_036_854_775_807`
    //   cast 回 i64 还是 `i64::MAX`,等价于"无上界"语义。
    let upper_bound = i64::MAX as u64;
    let remaining = store
        .query_range("cpu_usage", 0, upper_bound)
        .await
        .expect("query_range should succeed");
    assert_eq!(remaining.len(), 2, "清理后应剩 2 行");
    // 验证剩下的 2 行均为 7 天内数据
    for row in &remaining {
        let age_days = (now_ms - row.ts) / one_day_ms;
        assert!(
            age_days <= 7,
            "剩余数据应在 7 天内,实际 age={} 天 (ts={})",
            age_days,
            row.ts
        );
    }
    // 断言旧行已被删(10 天前那行不在结果中)
    assert!(
        !remaining.iter().any(|r| (r.value - 0.10).abs() < 1e-9),
        "10 天前那行(0.10)应已被清理"
    );
}
