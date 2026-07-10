//! SqliteHistoryStore 持久化集成测试 — Task P1 (v1.4.0-omega)
//!
//! 对应架构层:L1 Core(model-router)
//!
//! # 测试目标
//! 1. record/get 基础语义(success_count / total_count / latency_samples 正确)
//! 2. 跨重启持久化(Drop → 重新打开同路径,数据保留)
//! 3. UPSERT 累积(多次 record 同一 model_id,计数累加 + 滑动窗口)
//! 4. latency_samples VecDeque<f32> 序列化/反序列化一致性(空/满/超出容量)
//! 5. async 上下文下 spawn_blocking 包装不阻塞 runtime
//! 6. RouterConfig 默认 Memory(向后兼容)
//! 7. RouterConfig 配置 Sqlite 为 opt-in
//!
//! # 设计约束
//! - HistoryStore trait 是同步的(`&self` 方法),SqliteHistoryStore 的 get/record 也是同步的
//! - 调用方在 async 上下文中调用时,需用 `spawn_blocking` 包装整个调用(测试 5 验证)
//! - SQLite 单行 UPSERT/SELECT 是微秒级操作,同步上下文中调用可接受

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use model_router::{
    HistoryPersistence, HistoryRecord, HistoryStore, InMemoryHistoryStore, RouterConfig,
    SqliteHistoryStore,
};

// ============================================================
// 测试 1:record 后 get 返回正确数据
// ============================================================

#[test]
fn test_sqlite_history_record_and_get() {
    let tmp = tempfile::tempdir().expect("tempdir 创建失败");
    let db_path = tmp.path().join("history_basic.db");
    let store = SqliteHistoryStore::new(&db_path).expect("SqliteHistoryStore 打开失败");

    // 无记录时返回 None
    assert!(store.get("absent").is_none());

    // 单次 record
    store.record("gpt-4", 200.0, true);
    let record = store.get("gpt-4").expect("record 后应能 get");
    assert_eq!(record.total_count, 1);
    assert_eq!(record.success_count, 1);
    assert_eq!(record.latency_samples.len(), 1);
    assert!((record.latency_samples[0] - 200.0).abs() < 1e-6);

    // 失败记录不增加 success_count
    store.record("gpt-4", 300.0, false);
    let record = store.get("gpt-4").expect("第二次 record 后应能 get");
    assert_eq!(record.total_count, 2);
    assert_eq!(record.success_count, 1, "失败记录不应增加 success_count");
    assert_eq!(record.latency_samples.len(), 2);
}

// ============================================================
// 测试 2:跨重启持久化(Drop → 重新打开同路径,数据保留)
// ============================================================

#[test]
fn test_sqlite_history_persistence_across_restart() {
    let tmp = tempfile::tempdir().expect("tempdir 创建失败");
    let db_path = tmp.path().join("history_restart.db");

    // 第一次会话:写入数据后 Drop
    {
        let store = SqliteHistoryStore::new(&db_path).expect("首次打开失败");
        store.record("model-1", 100.0, true);
        store.record("model-1", 200.0, false);
        store.record("model-2", 150.0, true);
        // store 离开作用域,Connection 关闭
    }

    // 第二次会话:重新打开同一路径,验证数据保留
    let store = SqliteHistoryStore::new(&db_path).expect("重新打开失败");
    let r1 = store.get("model-1").expect("model-1 数据应持久化");
    assert_eq!(r1.total_count, 2, "total_count 应跨重启保留");
    assert_eq!(r1.success_count, 1, "success_count 应跨重启保留");
    assert_eq!(r1.latency_samples.len(), 2, "latency_samples 应跨重启保留");
    assert!((r1.latency_samples[0] - 100.0).abs() < 1e-6);
    assert!((r1.latency_samples[1] - 200.0).abs() < 1e-6);

    let r2 = store.get("model-2").expect("model-2 数据应持久化");
    assert_eq!(r2.total_count, 1);
    assert_eq!(r2.success_count, 1);
}

// ============================================================
// 测试 3:UPSERT 累积 — 多次 record 同一 model_id,计数累加 + 滑动窗口
// ============================================================

#[test]
fn test_sqlite_history_upsert_accumulates() {
    let tmp = tempfile::tempdir().expect("tempdir 创建失败");
    let db_path = tmp.path().join("history_upsert.db");
    let store = SqliteHistoryStore::new(&db_path).expect("打开失败");

    // 150 次 record:total_count 应 = 150,latency_samples 应只保留最后 100 个(滑动窗口)
    for i in 0..150u32 {
        let latency = i as f32 * 1.0;
        let success = i % 3 != 0; // 失败率 ~1/3
        store.record("accumulator", latency, success);
    }

    let record = store.get("accumulator").expect("累积后应能 get");
    assert_eq!(record.total_count, 150, "total_count 应累计 150");
    // 成功次数:i % 3 != 0 → i=0 失败,i=1,2 成功,i=3 失败... 每 3 个有 2 个成功
    // 150 / 3 = 50 个失败 → 100 个成功
    assert_eq!(record.success_count, 100, "success_count 应累计 100");
    assert_eq!(
        record.latency_samples.len(),
        100,
        "滑动窗口应保持容量 100,丢弃最旧的 50 个"
    );
    // 滑动窗口应保留最后 100 个:latency = 50.0 ~ 149.0
    assert!(
        (record.latency_samples[0] - 50.0).abs() < 1e-6,
        "最旧样本应为 50.0"
    );
    assert!(
        (record.latency_samples[99] - 149.0).abs() < 1e-6,
        "最新样本应为 149.0"
    );
}

// ============================================================
// 测试 4:VecDeque<f32> 序列化/反序列化一致性(空/满/超出容量)
// ============================================================

#[test]
fn test_sqlite_history_latency_samples_roundtrip() {
    let tmp = tempfile::tempdir().expect("tempdir 创建失败");
    let db_path = tmp.path().join("history_roundtrip.db");
    let store = SqliteHistoryStore::new(&db_path).expect("打开失败");

    // 边界 1:空 samples(无记录)
    assert!(store.get("empty").is_none());

    // 边界 2:单个 sample
    store.record("single", 42.5, true);
    let r = store.get("single").expect("single 应存在");
    assert_eq!(r.latency_samples.len(), 1);
    assert!((r.latency_samples[0] - 42.5).abs() < 1e-6);

    // 边界 3:恰好 100 个(窗口满,不淘汰)
    for i in 0..100u32 {
        store.record("full", i as f32 * 0.5, true);
    }
    let r = store.get("full").expect("full 应存在");
    assert_eq!(r.latency_samples.len(), 100, "恰好 100 个应全部保留");
    assert!((r.latency_samples[0] - 0.0).abs() < 1e-6);
    assert!((r.latency_samples[99] - 49.5).abs() < 1e-6);

    // 边界 4:超出容量 1 个 → 淘汰最旧 1 个,保留最新 100 个
    store.record("full", 999.0, true);
    let r = store.get("full").expect("full 应存在");
    assert_eq!(r.latency_samples.len(), 100, "超出 1 个后仍应保持 100");
    assert!(
        (r.latency_samples[0] - 0.5).abs() < 1e-6,
        "最旧的 0.0 应被淘汰,新的最旧应为 0.5"
    );
    assert!(
        (r.latency_samples[99] - 999.0).abs() < 1e-6,
        "最新应为 999.0"
    );
    assert_eq!(r.total_count, 101, "total_count 应累计 101(不随窗口滑动)");

    // 边界 5:含 NaN/Inf 的延迟(序列化应能 roundtrip,不 panic)
    store.record("nan-model", f32::NAN, true);
    store.record("inf-model", f32::INFINITY, true);
    let r_nan = store.get("nan-model").expect("nan-model 应存在");
    assert_eq!(r_nan.latency_samples.len(), 1);
    assert!(r_nan.latency_samples[0].is_nan());
    let r_inf = store.get("inf-model").expect("inf-model 应存在");
    assert!(r_inf.latency_samples[0].is_infinite());
}

// ============================================================
// 测试 5:async 上下文下 spawn_blocking 包装不阻塞 runtime
//
// WHY 此测试:HistoryStore trait 是同步的,SqliteHistoryStore::record 是同步阻塞调用。
// 调用方在 async 上下文中调用时,必须用 tokio::task::spawn_blocking 包装,
// 否则会阻塞 Tokio runtime 的工作线程。此测试用 timeout 验证 spawn_blocking
// 包装后不阻塞 runtime(轻量 async 任务能并发完成)。
// ============================================================

#[tokio::test]
async fn test_sqlite_history_spawn_blocking_not_blocking_runtime() {
    let tmp = tempfile::tempdir().expect("tempdir 创建失败");
    let db_path = tmp.path().join("history_blocking.db");
    let store = Arc::new(SqliteHistoryStore::new(&db_path).expect("打开失败"));

    // 预置一些数据
    for i in 0..50u32 {
        let s = Arc::clone(&store);
        tokio::task::spawn_blocking(move || {
            s.record("preheat", i as f32 * 1.0, true);
        })
        .await
        .expect("预置 spawn_blocking 失败");
    }

    // 并发执行:DB 操作(spawn_blocking)+ 轻量 async 计时任务
    let store_for_db = Arc::clone(&store);
    let db_task = tokio::spawn(async move {
        tokio::task::spawn_blocking(move || {
            // 在 spawn_blocking 中执行同步的 record 调用
            for i in 0..100u32 {
                store_for_db.record("batch", i as f32 * 1.5, i % 5 != 0);
            }
        })
        .await
        .expect("spawn_blocking join 失败");
    });

    // 轻量 async 任务:多次 yield,验证 runtime 未被阻塞
    let lightweight = tokio::time::timeout(Duration::from_millis(200), async {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        42
    })
    .await;

    assert!(
        lightweight.is_ok(),
        "轻量 async 任务超时 — SqliteHistoryStore 在 spawn_blocking 外可能阻塞了 runtime"
    );
    assert_eq!(lightweight.unwrap(), 42);

    // 等待 DB 任务完成,验证功能正确性
    db_task.await.expect("db task join 失败");
    let record = store.get("batch").expect("batch 应存在");
    assert_eq!(record.total_count, 100);
    assert_eq!(record.latency_samples.len(), 100);
}

// ============================================================
// 测试 6:RouterConfig 默认 Memory(向后兼容)
// ============================================================

#[test]
fn test_config_persistence_memory_default() {
    // 默认配置应使用 Memory 模式(v1.3.0 行为不变)
    let config = RouterConfig::default();
    assert!(
        matches!(config.history_persistence, HistoryPersistence::Memory),
        "默认应为 Memory,保证 v1.3.0 向后兼容"
    );
}

#[test]
fn test_config_persistence_backward_compatible_without_field() {
    // 旧配置文件(v1.3.0)无 history_persistence 字段,反序列化应使用默认 Memory
    let json = r#"{
        "models": [],
        "default_strategy": "Lite"
    }"#;
    let de: RouterConfig = serde_json::from_str(json).expect("旧配置应能反序列化");
    assert!(
        matches!(de.history_persistence, HistoryPersistence::Memory),
        "缺失 history_persistence 字段时应回退到 Memory"
    );
}

// ============================================================
// 测试 7:RouterConfig 配置 Sqlite 为 opt-in
// ============================================================

#[test]
fn test_config_persistence_sqlite_opt_in() {
    // 显式配置 Sqlite 模式
    let json = r#"{
        "models": [],
        "default_strategy": "Lite",
        "history_persistence": {
            "Sqlite": {
                "db_path": "history.db"
            }
        }
    }"#;
    let de: RouterConfig = serde_json::from_str(json).expect("Sqlite 配置应能反序列化");
    match de.history_persistence {
        HistoryPersistence::Sqlite { ref db_path } => {
            assert_eq!(*db_path, PathBuf::from("history.db"));
        }
        ref other => panic!("应为 Sqlite 变体,实际: {other:?}"),
    }
}

// ============================================================
// 附加:Memory 与 SQLite 实现行为等价性(交叉验证)
// ============================================================

#[test]
fn test_memory_and_sqlite_behavior_equivalence() {
    let tmp = tempfile::tempdir().expect("tempdir 创建失败");
    let db_path = tmp.path().join("history_equiv.db");
    let sqlite = SqliteHistoryStore::new(&db_path).expect("打开失败");
    let memory = InMemoryHistoryStore::new();

    // 两种实现执行相同的 record 序列
    for i in 0..120u32 {
        let latency = i as f32 * 1.0;
        let success = i % 4 != 0; // 75% 成功率
        memory.record("model-x", latency, success);
        sqlite.record("model-x", latency, success);
    }

    let r_mem = memory.get("model-x").expect("memory 应有记录");
    let r_sql = sqlite.get("model-x").expect("sqlite 应有记录");

    // 验证两实现语义等价
    assert_eq!(r_mem.total_count, r_sql.total_count, "total_count 应一致");
    assert_eq!(
        r_mem.success_count, r_sql.success_count,
        "success_count 应一致"
    );
    assert_eq!(
        r_mem.latency_samples.len(),
        r_sql.latency_samples.len(),
        "滑动窗口大小应一致"
    );
    // 逐元素比对滑动窗口内容
    for (i, (a, b)) in r_mem
        .latency_samples
        .iter()
        .zip(r_sql.latency_samples.iter())
        .enumerate()
    {
        assert!(
            (a - b).abs() < 1e-6,
            "latency_samples[{i}] 不一致: memory={a}, sqlite={b}"
        );
    }
    // 验证 VecDeque 内容 = 最后 100 个
    assert_eq!(r_mem.latency_samples, r_sql.latency_samples);

    // 验证 HistoryRecord 方法行为等价
    assert_eq!(r_mem.success_rate(), r_sql.success_rate());
    assert!((r_mem.latency_variance() - r_sql.latency_variance()).abs() < 1e-5);
    assert_eq!(r_mem.is_sufficient(), r_sql.is_sufficient());
    assert!(r_mem.is_sufficient(), "120 条应充足(>= 100)");
}

// 辅助:避免未使用导入警告(HistoryRecord 用于 type hint)
#[allow(dead_code)]
fn _type_hint(_r: HistoryRecord) {
    // VecDeque 类型已在测试中使用
    let _ = VecDeque::<f32>::new();
}
