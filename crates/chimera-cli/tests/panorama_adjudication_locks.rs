//! P4 / ADR-259 回归锁 — U 项三态裁决的文本/行为锁（deepdive §9.4）。
//!
//! 五锁：U-20 已接线行为锁 / U-08 注记对齐锁 / F-09 刻意无生产者锁 /
//! U-05 投票预留锁 / 事件计数机器锁。零新增事件变体（六锁同批纪律）。
//!
//! 跨 crate 路径纪律（§9.4 missing-11）：一律以 `CARGO_MANIFEST_DIR` 起算
//! （= crates/chimera-cli），禁止依赖测试进程 CWD。
//!
//! 运行：`cargo test -p chimera-cli --test panorama_adjudication_locks -- --test-threads=1`

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::path::Path;

/// workspace 根（= CARGO_MANIFEST_DIR/../..）
fn ws_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../")
}

/// 读 workspace 内 crate 源文件（路径拼接 + 存在性显式失败）
fn src_of(rel: &str) -> String {
    let p = ws_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("读取 {rel} 应成功: {e}"))
}

/// 锁 1（U-20 已接线行为锁）：设 `CHIMERA_METRICS_DUMP_PATH`（tempdir）→
/// experience_loop 跑过 dump 周期（5s tick ×2）→ 文件存在且含
/// `nexus_event_total`（Prometheus 文本导出面,ADR-259 D6 终态回归锁）。
/// WHY 经 build() 组合根：BusLogger 仅在组合根 set_logger 装配（W3）——
/// 裸 EventBus::new() 的 logger() 恒 None,dump 周期空转。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn u20_metrics_dump_env_path() {
    use chimera_cli::composition::build;
    use chimera_cli::experience_loop::spawn_experience_loop;
    use chimera_cli::ChimeraConfig;

    let dir = tempfile::tempdir().expect("tempdir 应成功");
    let dump = dir.path().join("metrics.txt");
    // SAFETY(测试语境):单测试串行写本 env（测试文件内唯一 env 写点,无并行互踩面）
    std::env::set_var("CHIMERA_METRICS_DUMP_PATH", &dump);

    let ctx = build(&ChimeraConfig::default()).expect("组合根装配应成功");
    let _handles = spawn_experience_loop(ctx.bus, std::sync::Arc::new(ctx.engine), false)
        .await
        .expect("经验闭环装配应成功");

    // dump 周期 5s(首个 tick 跳过,第二个 tick 落盘)——等 6.5s 覆盖一个写点
    tokio::time::sleep(std::time::Duration::from_millis(6_500)).await;

    let text =
        std::fs::read_to_string(&dump).unwrap_or_else(|e| panic!("指标 dump 文件应存在: {e}"));
    // 指标族名 = nexus_event（prometheus-client 渲染 HELP/Sample 以族名为锚,
    // 实测无 _total 后缀——deepdive §9.4 原引 nexus_event_total 为预写形态）
    assert!(
        text.contains("nexus_event"),
        "dump 文本应含 nexus_event 指标族,实际:\n{text}"
    );
    std::env::remove_var("CHIMERA_METRICS_DUMP_PATH");
}

/// 锁 2（U-08 注记对齐）：session-store lib.rs 三订正段含
/// 「composition.rs + with_session_store」文本（防「生产装配未启用双写」
/// 漂移再现——deepdive Z-3 订正的机器锚）。
#[test]
fn u08_session_store_note_parity() {
    let lib = src_of("crates/session-store/src/lib.rs");
    assert!(lib.contains("装配态三订正"), "三订正段应在场（P4 落档）");
    assert!(
        lib.contains("with_session_store") && lib.contains("composition.rs"),
        "三订正段应指向 composition.rs with_session_store 生产装配"
    );
    assert!(
        lib.contains("PRODUCTION-WIRED"),
        "写面口径应登记为 PRODUCTION-WIRED"
    );
}

/// 锁 3（F-09）：`TuiActionProgressed` 无 request_id 是**刻意决策**——
/// types.rs 注释段在场（防误判缺陷私加 request_id,ADR-259 D7）。
#[test]
fn f09_no_producer_is_deliberate() {
    let types = src_of("crates/event-bus/src/types.rs");
    assert!(
        types.contains("本变体**不携带** `request_id`"),
        "F-09 刻意决策注释段应在场（types.rs TuiActionProgressed）"
    );
    assert!(
        types.contains("再以独立 ADR 补充"),
        "解锁路径注释应在场（防单方面恢复填充制造半途契约）"
    );
}

/// 锁 4（U-05）：投票预留——event_loop.rs `publish_vote` 的诚实降级注释块
/// 在场且函数体无任何 publish 调用（FC-A 语义锁定,ADR-259 D1）。
#[test]
fn u05_vote_remains_unpublished() {
    let src = src_of("crates/chimera-tui/src/app/event_loop.rs");
    assert!(src.contains("诚实降级(FC-A"), "U-05 诚实降级注释块应在场");
    // 提取 publish_vote 函数体（fn 起点至下一个 fn），断言无发布调用
    let start = src.find("fn publish_vote").expect("publish_vote 应在场");
    let rest = &src[start..];
    let end = rest["fn publish_vote".len()..]
        .find("\n    fn ")
        .map(|i| i + "fn publish_vote".len())
        .unwrap_or(rest.len());
    let body = &rest[..end];
    assert!(
        !body.contains(".publish("),
        "publish_vote 不得发布事件（L8 消费通道未落地,U-05 维持预留）"
    );
    assert!(
        body.contains("vote_unwired") || body.contains("set_status"),
        "状态栏诚实告知应在场"
    );
}

/// 锁 5（事件计数机器锁）：`VARIANT_COUNT_LOCK = 154` 不变——零新增事件
/// 变体的六锁同批纪律主锚（ADR-259 D9;P1~P4 全程零新增实证）。
#[test]
fn variant_count_lock_unchanged() {
    let test = src_of("crates/event-bus/tests/variant_count_test.rs");
    // ADR-291（R9 WP-0）:+2 Normal 观测变体后 154→156；本锁继续锚定
    // 「变体新增必须同批同步全部锁面」的六锁纪律（本锁自身即当时的漏同步点，
    // 吸收验收 R9b 补齐——引以为戒：锁面清单以 grep VARIANT_COUNT_LOCK 全仓为准）
    assert!(
        test.contains("VARIANT_COUNT_LOCK: usize = 156"),
        "事件变体计数锁必须与 event-bus variant_count_test 同值（当前 156）"
    );
}
