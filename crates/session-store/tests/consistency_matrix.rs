//! A7 跨会话一致性矩阵 — 存储面场景（1/2/4/5/6）
//!
//! # 判定口径（每个场景同时锁三件事）
//! - **零丢失**：已确认（`flush` 返回）的事件必须全部可回放；
//! - **零重复**：同一事件不得在回放中出现两次；
//! - **零顺序破坏**：回放顺序 = 写入顺序（`seq` 主序 + 段内 `row` 次序）。
//!
//! # 场景编号（与 Spec A7 对齐）
//! - `scenario_01`：正常重启 —— 段文件权威源回放逐条一致；
//! - `scenario_02`：写中 kill -9 —— 已确认前缀完整，未确认事件不得以「半条」形态出现；
//! - `scenario_04`：索引损坏重建 —— 删除 SQLite 索引后段文件回放仍零丢失；
//! - `scenario_05`：半写段重启 —— 尾部半条被截断，后续写入与回放不受污染；
//! - `scenario_06`：多会话隔离 —— 两会话交错写入，回放互不污染。
//!
//! 其余场景落点：`scenario_03`（threads 404 回放）在 `nexus-app-server`；
//! `scenario_07`（检查点卫星态恢复）在 `quest-engine`；`scenario_08`（经验卡幂等重放）
//! 在 `event-bus`/`cmt-tiering` 的持久面。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production
#![forbid(unsafe_code)]

use session_store::{CbmrWriter, Offset, SessionEvent, SessionId, StoreConfig, TreeIndex};

/// 打开写入器（测试配置：临时目录 + 固定批窗口）
async fn open(dir: &std::path::Path) -> CbmrWriter {
    CbmrWriter::open(StoreConfig::test_config(dir))
        .await
        .expect("打开写入器")
}

/// 追加一条带单字节负载的事件
async fn append(writer: &CbmrWriter, sid: &SessionId, tag: &str, byte: u8) {
    writer
        .append(sid, SessionEvent::with_payload(tag, vec![byte]))
        .await
        .expect("追加事件");
}

/// 回放负载序列（段文件权威源）
async fn replay_payloads(writer: &CbmrWriter, sid: &SessionId) -> Vec<Vec<u8>> {
    writer
        .replay_session(sid, Offset::new(0, 0))
        .await
        .expect("段文件回放")
        .into_iter()
        .map(|item| item.event.payload.unwrap_or_default())
        .collect()
}

/// 找到该会话的段文件（`<session_id>.<index>.jsonl`）
fn segment_of(dir: &std::path::Path, sid: &SessionId) -> std::path::PathBuf {
    let prefix = format!("{}.", sid.as_str());
    std::fs::read_dir(dir)
        .expect("枚举数据目录")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".jsonl"))
        })
        .expect("该会话必须已有段文件")
}

// ============================================================
// scenario_01：正常重启
// ============================================================

#[tokio::test]
async fn scenario_01_normal_restart_preserves_exact_sequence() {
    let dir = tempfile::tempdir().expect("临时目录");
    let sid = SessionId::new("normal-restart");
    let expected: Vec<Vec<u8>> = (0..8u8).map(|i| vec![i]).collect();

    {
        let writer = open(dir.path()).await;
        for i in 0..8u8 {
            append(&writer, &sid, "turn", i).await;
        }
        writer.flush().await.expect("冲刷");
        writer.shutdown().await.expect("优雅关闭");
    }

    // 「重启」：新实例、同一数据目录
    let writer = open(dir.path()).await;
    let replayed = replay_payloads(&writer, &sid).await;
    assert_eq!(
        replayed, expected,
        "正常重启后回放必须逐条、保序、零重复；实际 {replayed:?}"
    );

    // 索引读面与段文件权威源同序（双写一致性）
    let indexed: Vec<Vec<u8>> = writer
        .read_events(&sid, None)
        .await
        .expect("索引读")
        .into_iter()
        .map(|stored| stored.event.payload.unwrap_or_default())
        .collect();
    assert_eq!(indexed, expected, "索引读面与段文件回放必须逐条一致");
}

// ============================================================
// scenario_02：写中 kill -9（未 flush 事件不得以半条形态出现）
// ============================================================

#[tokio::test]
async fn scenario_02_crash_mid_write_keeps_confirmed_prefix() {
    let dir = tempfile::tempdir().expect("临时目录");
    let sid = SessionId::new("crash-mid-write");

    {
        let writer = open(dir.path()).await;
        for i in 0..3u8 {
            append(&writer, &sid, "confirmed", i).await;
        }
        // 已确认 = flush 返回（lib.rs「append 返回 Ok 前 fsync」的 flush 语义）
        writer.flush().await.expect("确认前缀落盘");
        // 之后追加但**不** flush，随即 drop 模拟 kill -9（Drop 只 abort，不冲刷）
        for i in 3..6u8 {
            append(&writer, &sid, "unconfirmed", i).await;
        }
    }

    let writer = open(dir.path()).await;
    let replayed = replay_payloads(&writer, &sid).await;
    assert!(
        replayed.len() >= 3,
        "已确认前缀不得丢失：实际 {} 条",
        replayed.len()
    );
    assert!(
        replayed.len() <= 6,
        "回放不得超过写入总量（不存在凭空补条）"
    );
    // 已确认前缀逐条一致
    assert_eq!(
        &replayed[..3],
        &[vec![0u8], vec![1], vec![2]],
        "已确认前缀必须保序：{replayed:?}"
    );
    // 未确认事件要么整条在场、要么整条缺席 —— 不得出现半条/乱序
    let expected_prefix: Vec<Vec<u8>> = (0..replayed.len() as u8).map(|i| vec![i]).collect();
    assert_eq!(
        replayed, expected_prefix,
        "崩溃后回放必须是写入序列的**严格前缀**（无空洞、无乱序、无重复）"
    );
}

// ============================================================
// scenario_04：索引损坏重建（段文件为权威源）
// ============================================================

#[tokio::test]
async fn scenario_04_corrupt_index_rebuilds_from_segments() {
    let dir = tempfile::tempdir().expect("临时目录");
    let sid = SessionId::new("corrupt-index");
    let expected: Vec<Vec<u8>> = (0..6u8).map(|i| vec![i]).collect();

    {
        let writer = open(dir.path()).await;
        for i in 0..6u8 {
            append(&writer, &sid, "turn", i).await;
        }
        writer.flush().await.expect("冲刷");
        writer.shutdown().await.expect("优雅关闭");
    }

    // 破坏索引：SQLite 树库是派生视图，删除等价于「索引损坏到不可读」
    let index = dir.path().join("sessions.sqlite3");
    assert!(index.exists(), "前提：树索引文件必须已生成");
    std::fs::remove_file(&index).expect("删除索引模拟损坏");

    // 重启期自愈：树索引重建（段文件为权威源）+ 内容指纹探针
    let tree = TreeIndex::open(&index).expect("重建树索引");
    let report = tree.probe_content_consistency(dir.path()).expect("内容探针");
    assert!(
        report.rebuilt_count() >= 1,
        "索引损坏必须触发重建，实际 rebuilt={}",
        report.rebuilt_count()
    );

    let writer = open(dir.path()).await;
    let replayed = replay_payloads(&writer, &sid).await;
    assert_eq!(
        replayed, expected,
        "索引重建后段文件回放必须零丢失；实际 {replayed:?}"
    );
}

// ============================================================
// scenario_05：半写段重启（尾部半条截断）
// ============================================================

#[tokio::test]
async fn scenario_05_partial_tail_is_truncated_before_next_write() {
    let dir = tempfile::tempdir().expect("临时目录");
    let sid = SessionId::new("partial-tail");

    {
        let writer = open(dir.path()).await;
        for i in 0..4u8 {
            append(&writer, &sid, "turn", i).await;
        }
        writer.flush().await.expect("冲刷");
        writer.shutdown().await.expect("优雅关闭");
    }

    // 模拟写入中断：长度前缀写全（100 字节）、数据只写 5 字节
    let segment = segment_of(dir.path(), &sid);
    let mut half: Vec<u8> = Vec::new();
    half.extend_from_slice(&100u32.to_le_bytes());
    half.extend_from_slice(&[0u8; 5]);
    {
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&segment)
            .expect("打开段文件");
        file.write_all(&half).expect("追加半条");
        file.sync_all().expect("落盘半条");
    }

    // 「重启 + 继续写」：恢复扫描截断半条尾部后再追加
    let writer = open(dir.path()).await;
    append(&writer, &sid, "after-recovery", 4).await;
    writer.flush().await.expect("恢复后冲刷");

    let replayed = replay_payloads(&writer, &sid).await;
    let expected: Vec<Vec<u8>> = (0..5u8).map(|i| vec![i]).collect();
    assert_eq!(
        replayed, expected,
        "半条尾部必须被截断：完整记录 4 条 + 恢复后 1 条，且顺序不变；实际 {replayed:?}"
    );
}

// ============================================================
// scenario_06：多会话隔离
// ============================================================

#[tokio::test]
async fn scenario_06_multi_session_isolation_under_interleaving() {
    let dir = tempfile::tempdir().expect("临时目录");
    let a = SessionId::new("sess-matrix-a");
    let b = SessionId::new("sess-matrix-b");

    {
        let writer = open(dir.path()).await;
        // 交错写入：A/B 交替，验证回放不串流、不串序
        for i in 0..4u8 {
            append(&writer, &a, "a.event", i).await;
            append(&writer, &b, "b.event", 100 + i).await;
        }
        writer.flush().await.expect("冲刷");
        writer.shutdown().await.expect("优雅关闭");
    }

    let writer = open(dir.path()).await;
    let a_events = replay_payloads(&writer, &a).await;
    let b_events = replay_payloads(&writer, &b).await;
    assert_eq!(
        a_events,
        vec![vec![0u8], vec![1], vec![2], vec![3]],
        "会话 A 回放只能看到自己的事件且保序"
    );
    assert_eq!(
        b_events,
        vec![vec![100u8], vec![101], vec![102], vec![103]],
        "会话 B 回放只能看到自己的事件且保序"
    );
}
