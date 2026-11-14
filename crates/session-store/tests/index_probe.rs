//! 树索引内容指纹 + 启动期一致性探针（R12 W3-1）
//!
//! 锁定 5 个契约：
//! 1. 同一段集合重复计算内容指纹恒相等（两侧同一口径，无随机性）；
//! 2. 老库 meta 无 `content_hash` 字段 ⇒ 视为未知 ⇒ 触发一次重建并补记；
//! 3. 段文件被篡改 ⇒ 指纹不符 ⇒ 重建恰好一次 + 记录（报告/warn 同文本）+ 计数；
//! 4. `CHIMERA_INDEX_PROBE=off` ⇒ 整体跳过，索引保持原样（逃生开关真实生效）；
//! 5. 写入中断（尾部半条）后重启 ⇒ 探针判不一致并重建，重建后再次重启即健康。
//!
//! 环境变量是进程级状态：本文件所有会触达探针的用例共用 [`ENV_LOCK`] 串行化，
//! 并用 [`EnvGuard`] 在 panic 路径上也能恢复原值（不给同二进制内其它用例留脏环境）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use session_store::{
    compose_content_fingerprint, content_probe_enabled, content_probe_verdict, list_segment_files,
    segment_path, ContentProbeEntry, ContentProbeOutcome, ContentProbeVerdict, EventRow,
    IndexContentFingerprint, SegmentId, SegmentWriter, SessionEvent, SessionId, TreeIndex,
    INDEX_PROBE_ENV,
};

/// 串行化 env 相关用例（cargo test 默认多线程，环境变量是进程级共享）
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 环境变量守卫：进入时设置（`None` = 删除），离开时（含 panic）恢复原值
struct EnvGuard {
    previous: Option<String>,
}

impl EnvGuard {
    fn set(value: Option<&str>) -> Self {
        let previous = std::env::var(INDEX_PROBE_ENV).ok();
        match value {
            Some(v) => std::env::set_var(INDEX_PROBE_ENV, v),
            None => std::env::remove_var(INDEX_PROBE_ENV),
        }
        Self { previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.previous.as_deref() {
            Some(v) => std::env::set_var(INDEX_PROBE_ENV, v),
            None => std::env::remove_var(INDEX_PROBE_ENV),
        }
    }
}

/// 构造测试事件（event_type 带序号便于断言顺序）
fn ev(i: u64) -> SessionEvent {
    SessionEvent::with_payload(format!("ev-{i}"), vec![i as u8])
}

/// 打开落盘树索引（`data_dir = 树库父目录`，写路径据此补记内容指纹）
fn open_tree(dir: &Path) -> TreeIndex {
    TreeIndex::open(&dir.join("sessions.sqlite3")).expect("打开树索引")
}

/// 写入 n 条事件（每段 per_seg 条），走 flush 写入口 ⇒ 内容指纹随写事务自动补记
fn seed(tree: &TreeIndex, dir: &Path, sid: &SessionId, n: u64, per_seg: u64) {
    let mut idx = 0u32;
    let mut start = 0u64;
    while start < n {
        let mut writer = SegmentWriter::open_or_create(dir, sid, idx, start).expect("打开段写入器");
        let take = per_seg.min(n - start);
        let events: Vec<SessionEvent> = (start..start + take).map(ev).collect();
        let offsets = writer.append_batch(&events).expect("追加事件");
        let seg_id = SegmentId::generate();
        let meta = writer.meta(seg_id.clone(), None);
        let rows: Vec<EventRow> = offsets
            .iter()
            .zip(events)
            .map(|(off, event)| EventRow {
                offset: off.seq,
                session_id: sid.clone(),
                segment_id: seg_id.clone(),
                event,
            })
            .collect();
        tree.insert_flush_batch(&[meta], &rows)
            .expect("flush 批量写索引");
        start += take;
        idx += 1;
    }
}

/// 段文件逐段 (段索引, 字节数) —— 与探针实况侧同源的采集方式
fn file_lens(dir: &Path, sid: &SessionId) -> Vec<(u32, u64)> {
    list_segment_files(dir, sid)
        .expect("枚举段文件")
        .into_iter()
        .map(|idx| {
            let path = segment_path(dir, sid, idx);
            let len = std::fs::metadata(&path).expect("stat 段文件").len();
            (idx, len)
        })
        .collect()
}

/// 在最后一个段文件尾部追加若干原始字节（模拟半条残留 / 手工篡改）
fn append_raw(dir: &Path, sid: &SessionId, bytes: &[u8]) {
    let idx = *list_segment_files(dir, sid)
        .expect("枚举段文件")
        .last()
        .expect("至少一个段文件");
    let path = segment_path(dir, sid, idx);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("以追加模式打开段文件");
    file.write_all(bytes).expect("追加原始字节");
    file.flush().expect("flush 段文件");
}

/// 删掉某会话的内容指纹行（模拟「老库无该字段」）
fn delete_fingerprint(dir: &Path, sid: &SessionId) {
    let conn = rusqlite::Connection::open(dir.join("sessions.sqlite3")).expect("打开树索引库");
    conn.execute(
        "DELETE FROM index_meta WHERE key = ?1",
        [format!("content_hash:{sid}")],
    )
    .expect("删除内容指纹行");
}

/// 取某会话的探针记录（断言用）
fn entry_of<'a>(entries: &'a [ContentProbeEntry], sid: &str) -> &'a ContentProbeEntry {
    entries
        .iter()
        .find(|e| e.session_id == sid)
        .expect("报告必须含目标会话")
}

/// ① 同一段集合两次计算指纹相等 + 编码往返 + 两侧同一合成口径
#[test]
fn hash_roundtrip_stable() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("临时目录");
    let tree = open_tree(dir.path());
    let sid = SessionId::new("hash-stable");
    seed(&tree, dir.path(), &sid, 6, 3);

    let first = tree
        .live_content_fingerprint(dir.path(), &sid)
        .expect("实况指纹 1");
    let second = tree
        .live_content_fingerprint(dir.path(), &sid)
        .expect("实况指纹 2");
    assert_eq!(first, second, "同一段集合两次计算必须相等（无随机性）");
    assert_eq!(first.rows, 6, "完整记录数口径");
    assert_eq!(
        first.bytes,
        file_lens(dir.path(), &sid)
            .iter()
            .map(|(_, l)| *l)
            .sum::<u64>(),
        "bytes 分量必须等于段文件字节数之和"
    );

    // 文本形态稳定往返（index_meta.value 的落库形态）
    assert_eq!(
        IndexContentFingerprint::parse(&first.encode()),
        Some(first),
        "编码→解析必须无损"
    );
    assert_eq!(
        IndexContentFingerprint::parse("v1:1:2"),
        None,
        "截断值按不可解析处理"
    );
    assert_eq!(
        IndexContentFingerprint::parse("v2:1:2:3"),
        None,
        "未知版本按不可解析处理"
    );

    // 实况侧与纯函数必须同口径（否则两侧漂移会误判）
    let lens = file_lens(dir.path(), &sid);
    assert_eq!(
        compose_content_fingerprint(sid.as_str(), &lens, first.rows, Some(5)),
        first,
        "实况侧与 compose_content_fingerprint 必须逐字段一致"
    );

    // 写路径（insert_flush_batch）已把指纹补记 ⇒ 首次探针即健康（不重建）
    let report = tree.probe_content_consistency(dir.path()).expect("探针");
    assert!(!report.disabled, "默认必须开启");
    let hit = entry_of(&report.entries, "hash-stable");
    assert_eq!(hit.verdict, ContentProbeVerdict::Healthy);
    assert_eq!(hit.outcome, ContentProbeOutcome::Healthy);
    assert_eq!(hit.live, Some(first));
    assert_eq!(tree.content_probe_rebuild_count(), 0, "健康库不得重建");
}

/// ② 老库无指纹字段 ⇒ 视为未知 ⇒ 触发重建路径 + 计数 + 补记后转健康
#[test]
fn missing_field_treated_as_unknown_and_rebuilds() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("临时目录");
    let tree = open_tree(dir.path());
    let sid = SessionId::new("legacy-no-field");
    seed(&tree, dir.path(), &sid, 4, 4);

    // 模拟老库：内容指纹 key 不存在
    delete_fingerprint(dir.path(), &sid);
    assert_eq!(
        tree.content_fingerprint(&sid).expect("读指纹"),
        None,
        "老库必须读回 None（字段可缺省）"
    );

    // 纯判定层：None ⇒ MissingField（未知并入不一致通道）
    let live = tree
        .live_content_fingerprint(dir.path(), &sid)
        .expect("实况指纹");
    assert_eq!(
        content_probe_verdict(None, &live, &live),
        ContentProbeVerdict::MissingField
    );

    let mut observed: Vec<ContentProbeEntry> = Vec::new();
    let report = tree
        .probe_content_consistency_with(dir.path(), |e| observed.push(e.clone()))
        .expect("探针");
    let hit = entry_of(&report.entries, "legacy-no-field");
    assert_eq!(
        hit.verdict,
        ContentProbeVerdict::MissingField,
        "缺字段判未知"
    );
    assert_eq!(hit.outcome, ContentProbeOutcome::Rebuilt, "必须触发重建");
    assert_eq!(report.rebuilt_count(), 1, "本次恰好重建一个会话");
    assert_eq!(
        observed.len(),
        report.entries.len(),
        "观察者必须收到逐会话记录（通知路径真实被调用）"
    );
    assert_eq!(tree.content_probe_rebuild_count(), 1, "计数 +1");

    // 补记后：字段在场且与实况一致；二次探针不再重建
    assert_eq!(
        tree.content_fingerprint(&sid).expect("读指纹"),
        Some(live),
        "重建后必须按实况补记指纹"
    );
    let again = tree
        .probe_content_consistency(dir.path())
        .expect("二次探针");
    assert_eq!(
        entry_of(&again.entries, "legacy-no-field").outcome,
        ContentProbeOutcome::Healthy,
        "补记后应转健康"
    );
    assert_eq!(tree.content_probe_rebuild_count(), 1, "不得重复重建");
}

/// ③ 篡改段文件 ⇒ 指纹不符 ⇒ 重建一次 + 记录（报告条目与 warn 同文本）+ 计数
#[test]
fn mismatch_triggers_rebuild_and_event() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("临时目录");
    let tree = open_tree(dir.path());
    let sid = SessionId::new("tampered");
    seed(&tree, dir.path(), &sid, 6, 3);
    let before = tree
        .content_fingerprint(&sid)
        .expect("读指纹")
        .expect("指纹在场");
    let event_rows_before = tree.event_count(Some(&sid)).expect("事件数");
    assert_eq!(event_rows_before, 6);

    // 篡改：尾部追加 3 字节（< 4 字节长度前缀 ⇒ 半条前缀，读侧丢弃但不报错）
    append_raw(dir.path(), &sid, &[0xFF, 0x00, 0x01]);

    let live = tree
        .live_content_fingerprint(dir.path(), &sid)
        .expect("实况指纹");
    assert_ne!(before, live, "篡改后实况指纹必须变化");
    assert_eq!(
        content_probe_verdict(Some(&before), &live, &live),
        ContentProbeVerdict::Mismatch
    );

    let mut observed: Vec<ContentProbeEntry> = Vec::new();
    let report = tree
        .probe_content_consistency_with(dir.path(), |e| observed.push(e.clone()))
        .expect("探针");
    let hit = entry_of(&report.entries, "tampered");
    assert_eq!(hit.verdict, ContentProbeVerdict::Mismatch);
    assert_eq!(hit.outcome, ContentProbeOutcome::Rebuilt, "不符必须重建");
    assert!(
        hit.detail.contains("已重建"),
        "记录必须留下处置说明: {}",
        hit.detail
    );
    assert_eq!(observed.len(), 1, "观察者恰收到一条记录（warn 同文本）");
    assert_eq!(report.rebuilt_count(), 1, "重建恰好一次");
    assert_eq!(report.failed_open_count(), 0);
    assert_eq!(tree.content_probe_rebuild_count(), 1, "计数 +1");

    // 重建后：索引仍完整可读 + 指纹已对齐实况
    assert_eq!(tree.event_count(Some(&sid)).expect("事件数"), 6);
    assert_eq!(
        tree.read_events(&sid, None).expect("读事件").len(),
        6,
        "重建后读面必须完整"
    );
    assert_eq!(
        tree.content_fingerprint(&sid).expect("读指纹"),
        Some(live),
        "重建后指纹必须对齐段文件实况"
    );
}

/// ④ `CHIMERA_INDEX_PROBE=off` ⇒ 不重建（逃生开关真实生效），恢复默认即自愈
#[test]
fn probe_off_keeps_legacy_behavior() {
    let _lock = env_lock();
    // 纯判定层：只有明确关闭值才关（默认开启）
    assert!(content_probe_enabled(None), "未设置 ⇒ 默认开启");
    assert!(content_probe_enabled(Some("on")));
    assert!(!content_probe_enabled(Some("off")));
    assert!(!content_probe_enabled(Some(" OFF ")), "大小写/空白不敏感");
    assert!(!content_probe_enabled(Some("0")));
    assert!(!content_probe_enabled(Some("false")));

    let dir = tempfile::tempdir().expect("临时目录");
    let tree = open_tree(dir.path());
    let sid = SessionId::new("probe-off");
    seed(&tree, dir.path(), &sid, 4, 4);

    // 制造不一致：删掉索引中段一行（段文件仍权威完整）
    {
        let conn =
            rusqlite::Connection::open(dir.path().join("sessions.sqlite3")).expect("打开树索引库");
        conn.execute(
            "DELETE FROM events WHERE session_id = 'probe-off' AND offset = 1",
            [],
        )
        .expect("删除中段索引行");
    }
    let rows_damaged = tree.event_count(Some(&sid)).expect("事件数");
    assert_eq!(rows_damaged, 3, "缺口已造成");

    let guard = EnvGuard::set(Some("off"));
    let report = tree
        .probe_content_consistency(dir.path())
        .expect("探针(已关闭)");
    assert!(report.disabled, "关闭态必须在报告中显式体现");
    assert!(report.entries.is_empty(), "关闭态不得产出逐会话记录");
    assert_eq!(tree.content_probe_rebuild_count(), 0, "关闭态不得重建");
    assert_eq!(
        tree.event_count(Some(&sid)).expect("事件数"),
        rows_damaged,
        "关闭态必须保持旧行为（缺口保留、不干预）"
    );

    // 恢复默认（重新开启）⇒ 同一条不一致被探针发现并自愈
    drop(guard);
    let healed = tree
        .probe_content_consistency(dir.path())
        .expect("探针(已开启)");
    assert!(!healed.disabled);
    let hit = entry_of(&healed.entries, "probe-off");
    assert_eq!(hit.outcome, ContentProbeOutcome::Rebuilt, "开启后应重建");
    assert_eq!(
        tree.event_count(Some(&sid)).expect("事件数"),
        4,
        "重建后中段缺口被补齐"
    );
    assert_eq!(tree.content_probe_rebuild_count(), 1);
}

/// ⑤ 索引越前（孤儿行）⇒ 只报告不重建（补入式重建修不掉，避免每次启动白重建）
#[test]
fn orphan_index_rows_are_report_only() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("临时目录");
    let tree = open_tree(dir.path());
    let sid = SessionId::new("orphan-rows");
    seed(&tree, dir.path(), &sid, 3, 3);
    let live = tree
        .live_content_fingerprint(dir.path(), &sid)
        .expect("实况指纹");

    // 手工塞一行段文件里不存在的索引行（索引越前;payload 为合法 rmp 编码）
    {
        let conn =
            rusqlite::Connection::open(dir.path().join("sessions.sqlite3")).expect("打开树索引库");
        let payload = rmp_serde::to_vec_named(&ev(9)).expect("rmp 编码");
        conn.execute(
            "INSERT INTO events (offset, session_id, segment_id, payload) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![9i64, sid.as_str(), "orphan-seg", payload],
        )
        .expect("插入孤儿行");
    }
    assert_eq!(tree.event_count(Some(&sid)).expect("事件数"), 4);

    let report = tree.probe_content_consistency(dir.path()).expect("探针");
    let hit = entry_of(&report.entries, "orphan-rows");
    assert_eq!(
        hit.verdict,
        ContentProbeVerdict::IndexAhead,
        "索引行数多于段文件完整记录数 ⇒ 索引越前: {}",
        hit.detail
    );
    assert_eq!(
        hit.outcome,
        ContentProbeOutcome::ReportedOnly,
        "只报告不重建"
    );
    assert!(hit.detail.contains("ADR-290 D5"), "报告须指明处置归属");
    assert_eq!(tree.content_probe_rebuild_count(), 0, "越前不得触发重建");
    assert_eq!(
        tree.event_count(Some(&sid)).expect("事件数"),
        4,
        "孤儿行保持原样（重建只补不删）"
    );
    assert_eq!(
        tree.content_fingerprint(&sid).expect("读指纹"),
        Some(live),
        "只报告路径不得改动指纹（否则会把越前状态记成健康）"
    );
}

/// ⑥ 写入中断（尾部半条）后重启 ⇒ 判不一致并重建；重建后再重启 ⇒ 健康
#[test]
fn restart_after_partial_write_is_consistent() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("临时目录");
    let sid = SessionId::new("partial-write");
    {
        let tree = open_tree(dir.path());
        seed(&tree, dir.path(), &sid, 4, 4);
        // 模拟写入中断：长度前缀写全（100 字节）、数据只写了 5 字节
        let mut half: Vec<u8> = Vec::new();
        half.extend_from_slice(&100u32.to_le_bytes());
        half.extend_from_slice(&[0u8; 5]);
        append_raw(dir.path(), &sid, &half);
    }

    // 「重启」：重新打开树索引（探针是按启动期设计的，与栈上实例无关）
    let restarted = open_tree(dir.path());
    let live = restarted
        .live_content_fingerprint(dir.path(), &sid)
        .expect("实况指纹");
    assert_eq!(live.rows, 4, "半条不计入完整记录数");
    assert!(
        restarted
            .content_fingerprint(&sid)
            .expect("读指纹")
            .expect("指纹在场")
            != live,
        "半条残留使字节数变化 ⇒ 指纹必须判不一致"
    );

    let report = restarted
        .probe_content_consistency(dir.path())
        .expect("探针");
    let hit = entry_of(&report.entries, "partial-write");
    assert_eq!(hit.verdict, ContentProbeVerdict::Mismatch);
    assert_eq!(hit.outcome, ContentProbeOutcome::Rebuilt);
    assert_eq!(
        restarted.event_count(Some(&sid)).expect("事件数"),
        4,
        "半条不得被索引（重建只补完整记录）"
    );
    assert_eq!(
        restarted.content_fingerprint(&sid).expect("读指纹"),
        Some(live),
        "重建后指纹必须与实况一致"
    );

    // 再启动一次：一致 ⇒ 不重复重建（自愈收敛，不留每启动一次的重建风暴）
    let again = open_tree(dir.path());
    let report2 = again
        .probe_content_consistency(dir.path())
        .expect("二次探针");
    assert_eq!(
        entry_of(&report2.entries, "partial-write").outcome,
        ContentProbeOutcome::Healthy,
        "重建后再次启动必须判健康"
    );
    assert_eq!(again.content_probe_rebuild_count(), 0, "二次启动不得重建");
}

/// ⑦ 段文件损坏到读不出记录 ⇒ fail-open：不 panic、不重建、索引读面继续可用
#[test]
fn corrupt_segment_fails_open_without_panic() {
    let _lock = env_lock();
    let dir = tempfile::tempdir().expect("临时目录");
    let tree = open_tree(dir.path());
    let sid = SessionId::new("corrupt-seg");
    seed(&tree, dir.path(), &sid, 4, 4);

    // 破坏段文件：长度前缀合法、记录体不是 JSON ⇒ 只读器报 WalCorrupt
    let idx = *list_segment_files(dir.path(), &sid)
        .expect("枚举段文件")
        .last()
        .expect("至少一个段文件");
    let path = segment_path(dir.path(), &sid, idx);
    let body = b"not-json-at-all";
    let mut bytes: Vec<u8> = Vec::new();
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(body);
    bytes.push(b'\n');
    std::fs::write(&path, &bytes).expect("覆盖段文件");

    // 探针不得向上抛错、不得 panic（fail-open 的核心承诺）
    let report = tree
        .probe_content_consistency(dir.path())
        .expect("探针必须 fail-open 返回报告");
    let hit = entry_of(&report.entries, "corrupt-seg");
    assert_eq!(
        hit.verdict,
        ContentProbeVerdict::Undecidable,
        "实况不可读 ⇒ 无法判定: {}",
        hit.detail
    );
    assert_eq!(hit.outcome, ContentProbeOutcome::FailedOpen);
    assert_eq!(report.failed_open_count(), 1);
    assert_eq!(
        tree.content_probe_rebuild_count(),
        0,
        "读不了实况就无从重建"
    );

    // fail-open 承诺：索引本身没被破坏，读面照旧可用（陈旧而非不可用）
    assert_eq!(
        tree.read_events(&sid, None).expect("读事件").len(),
        4,
        "索引读面必须继续可用"
    );
}
