//! W0 / ADR-266 回归锁 — 「四件同批」违规的防再发锁。
//!
//! 背景：`06581d15`（ADR-258/工单 F）把 nexus-hook 注册面接进组合根，却未同批回收
//! 两张台账，导致 `check_gated_wiring_substance.py` 与 `check_declared_dep_usage.py`
//! 双双判红。本文件把「台账与代码必须同批」这件事变成可机检的文本锁，
//! 并补上 ADR-259 五锁的盲区（那五锁只锚 `session-store/src/lib.rs`，
//! 抓不到 `doctor/probes.rs` 里同一事实的另一份陈述）。
//!
//! 跨 crate 路径纪律（ADR-259 §9.4 missing-11）：一律以 `CARGO_MANIFEST_DIR` 起算
//! （= crates/chimera-cli），禁止依赖测试进程 CWD。
//!
//! 运行：`cargo test -p chimera-cli --test ledger_parity_locks`

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::path::Path;

/// workspace 根（= CARGO_MANIFEST_DIR/../..）
fn ws_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../")
}

/// 读 workspace 内文件（路径拼接 + 存在性显式失败，不静默跳过）
fn read_rel(rel: &str) -> String {
    let p = ws_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("读取 {rel} 应成功: {e}"))
}

/// 取台账/豁免表的数据行（去注释、去空行）
fn data_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .collect()
}

/// 锁 1：六个 r2_island_repayment 岛的 verdict 必须是 PARTIAL，且 reason 溯源到 ADR-266。
///
/// WHY 断言 reason 而不只断言 verdict：verdict 可以靠改一个词蒙对，
/// 但「为什么是 PARTIAL」必须区分**真实接线**（nexus-hook）与
/// **feature 级共享 enabler 计数抬升**（其余 5 岛）——把后者读成「已接线」
/// 正是本轮要防的误判。
#[test]
fn gated_wiring_ledger_rotated_with_provenance() {
    let text = read_rel("scripts/gated_wiring_baseline.txt");
    let shared = [
        "chtc-bridge",
        "csn-substitutor",
        "mtpe-executor",
        "sesa-router",
        "ssra-fusion",
    ];

    for dep in shared {
        let row = text
            .lines()
            .find(|l| l.starts_with(&format!("{dep}|")))
            .unwrap_or_else(|| panic!("台账缺 {dep} 行"));
        assert!(
            row.starts_with(&format!("{dep}|PARTIAL")),
            "{dep} 应为 PARTIAL（共享 enabler 计数），实为: {row}"
        );
        assert!(
            row.contains("ADR-266"),
            "{dep} 行 reason 须溯源 ADR-266: {row}"
        );
        assert!(
            row.contains("零引用"),
            "{dep} 行须明写本岛 src 仍零引用（防把计数抬升读成已接线）: {row}"
        );
    }

    // nexus-hook 是唯一真实接线者，reason 必须指向 ADR-258 与 register_all
    let hook = text
        .lines()
        .find(|l| l.starts_with("nexus-hook|"))
        .expect("台账缺 nexus-hook 行");
    assert!(hook.starts_with("nexus-hook|PARTIAL"), "实为: {hook}");
    assert!(
        hook.contains("ADR-258") && hook.contains("register_all"),
        "nexus-hook 行须写明真实接线来源: {hook}"
    );

    // 反向：这 6 个岛不得再有任何 DECORATIVE 行（否则门会判 STALE）
    for dep in shared.iter().chain(["nexus-hook"].iter()) {
        assert!(
            !text.contains(&format!("{dep}|DECORATIVE")),
            "{dep} 仍留 DECORATIVE 行 ⇒ 门会判 STALE"
        );
    }
}

/// 锁 2：nexus-hook 的依赖豁免必须已回收，且剩余条目数与说明段一致。
///
/// WHY 计数也要断言：本轮发现说明段原写「7 条」而实际只有 6 条（既存漂移）。
/// 豁免表的计数一旦失真，下一个人无法靠读注释判断该回收几行。
#[test]
fn nexus_hook_dep_exception_reclaimed_and_count_honest() {
    let text = read_rel("scripts/declared_dep_exceptions.txt");
    assert!(
        !text.contains("chimera-cli -> nexus-hook"),
        "nexus-hook 已在 composition.rs 真实引用，豁免前提失效，必须回收该行"
    );

    let rows = data_lines(&text);
    assert_eq!(
        rows.len(),
        5,
        "回收后应剩 5 条预留位（chtc/csn/mtpe/sesa/ssra），实为 {} 条: {rows:?}",
        rows.len()
    );
    assert!(
        text.contains("**5 条**"),
        "说明段计数须与数据行一致（防再次出现「注释 7 条 / 实际 6 条」漂移）"
    );
    // 其余 5 行的 reason 不得再声称 DECORATIVE（台账已旋转为 PARTIAL）
    assert!(
        !text.contains("已按 DECORATIVE 登记"),
        "豁免 reason 仍写 DECORATIVE，与已旋转的台账不一致"
    );
}

/// 锁 3（补 ADR-259 五锁盲区）：doctor probes 不得再声称 CbmrWriter 零生产构造。
///
/// 事实：写面已由 `composition.rs` 的 `CbmrWriter::open` →
/// `AppServer::with_session_store` 接线，宿主为 `serve`/`acp`；
/// 读面 `replay_since` 才是 ADR-259 D3 维持预留的那一半。
/// ADR-259 的 `u08_session_store_note_parity` 只锚 session-store/src/lib.rs，
/// 抓不到本文件这同一事实的另一份陈述 —— 故在此补锁。
#[test]
fn probes_storage_note_matches_wired_write_face() {
    let text = read_rel("crates/chimera-cli/src/commands/doctor/probes.rs");
    assert!(
        !text.contains("全库零生产构造点"),
        "probes 仍声称 CbmrWriter 全库零生产构造，与 composition.rs/serve.rs/acp.rs 矛盾"
    );
    assert!(
        !text.contains("生产装配尚未构造"),
        "probes 仍声称生产装配尚未构造 CbmrWriter（写面已接线）"
    );
    assert!(
        text.contains("CbmrWriter::open") && text.contains("ADR-259"),
        "订正后的注记须指向真实构造点与读面预留的裁决来源"
    );
}

/// 锁 4：装饰器顺序契约的溯源不得误引 ADR-257（那是 nexus-subagent 装配）。
///
/// 真实落地者是 commit `c6eb9b62`。误引会让后续规划把「失败归因契约」
/// 归到错误的决策记录上 —— 本轮勘察即因此产生过一次错误归因。
#[test]
fn decorator_order_provenance_points_at_real_commit() {
    let text = read_rel("crates/chimera-cli/src/exec_plan.rs");
    assert!(
        !text.contains("ADR-257"),
        "exec_plan.rs 仍误引 ADR-257（nexus-subagent）作为装饰器顺序契约来源"
    );
    assert!(
        text.contains("c6eb9b62"),
        "装饰器顺序契约须溯源到真实落地 commit c6eb9b62"
    );
    // 契约本体与反向对照必须仍在（防订正溯源时顺手删掉行为锁）
    assert!(
        text.contains("op_timeout_is_counted_as_failed_node"),
        "失败归因契约的判据测试不得缺失"
    );
    assert!(
        text.contains("failed_old"),
        "反向对照（旧顺序必重现不计数）不得缺失，否则该测试失去判别力"
    );
}
