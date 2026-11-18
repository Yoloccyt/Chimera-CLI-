//! 三态派发计划的公开契约 — `TaskKind` 登记表完整性 + `route` 的 Inline/Rayon 切割
//!
//! `bridge::decide` 是 `pub(crate)` 纯函数,集成测试只能经公开 `ComputeBridge::route`
//! 观测三态。Async 分支在当前登记面不可达（十类全为 CPU 型,手册 §8.4）,
//! 本文件以 `is_io_bound()` 契约 + `route` 永不返回 Async 双向锁定这一事实。
//!
//! 只读纪律:本文件**不调用** `update_threshold`,全局单例表保持出厂值,
//! 以免与同二进制的其他用例互相干扰（集成测试并行执行）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code

use nexus_compute::{bridge, DispatchPlan, TaskKind};

#[test]
fn task_kind_all_is_complete_and_duplicate_free() {
    // 契约:`ALL` 是登记清单唯一来源 —— 长度与 COUNT 单源一致,且无重复变体
    assert_eq!(
        TaskKind::ALL.len(),
        TaskKind::COUNT,
        "ALL 长度必须等于 COUNT（编译期绑定）"
    );
    assert_eq!(TaskKind::COUNT, 10, "手册 §8.4 十类登记");
    for (i, a) in TaskKind::ALL.iter().enumerate() {
        for (j, b) in TaskKind::ALL.iter().enumerate() {
            assert_eq!(i == j, a == b, "ALL[{i}] 与 ALL[{j}] 重复或比较失真");
        }
    }
}

#[test]
fn route_is_inline_strictly_below_threshold_and_rayon_from_threshold() {
    // 契约:阈值是干净切割点（off-by-one 由 n−1 / n 双侧锁定）
    let b = bridge();
    for kind in TaskKind::ALL {
        let threshold = b.threshold_of(kind);
        assert_eq!(
            b.route(kind, 0),
            DispatchPlan::Inline,
            "{kind:?}: 零条目必须就地 Inline"
        );
        assert_eq!(
            b.route(kind, threshold.saturating_sub(1)),
            DispatchPlan::Inline,
            "{kind:?}: n = threshold−1 必须 Inline"
        );
        assert_eq!(
            b.route(kind, threshold),
            DispatchPlan::Rayon,
            "{kind:?}: n = threshold 必须 Rayon（含边界）"
        );
        assert_eq!(
            b.route(kind, threshold.saturating_add(10_000)),
            DispatchPlan::Rayon,
            "{kind:?}: 远超阈值必须 Rayon"
        );
    }
}

#[test]
fn route_never_dispatches_registered_kinds_to_async() {
    // 契约:十类均为 CPU 计算型 → Async 分支在公开 API 不可达
    // （IO 类任务登记时 `is_io_bound` 返回 true 才会触发,本文件锁定当前事实）
    let b = bridge();
    for kind in TaskKind::ALL {
        assert!(
            !kind.is_io_bound(),
            "{kind:?} 当前登记应为 CPU 型（手册 §8.4）"
        );
        assert_ne!(
            b.route(kind, usize::MAX),
            DispatchPlan::Async,
            "{kind:?}: 任意规模都不得落到 Async（IO 不上 rayon 的对偶约束）"
        );
    }
}

#[test]
fn dispatch_async_is_reserved_or_reachable() {
    // 裁决（L1-H-3 b）:TaskKind 十类全为 CPU 型（无 IO / 网络 / 磁盘语义）⇒
    // `is_io_bound()` 恒 false ⇒ `DispatchPlan::Async` 是**预留变体**
    // （已标 `#[doc(hidden)]`），公开 `route` 对任何登记类型、任何规模都不可达。
    //
    // WHY 需要这个测试:`Async` 属死面但不得删除（会破坏下游 match 穷尽性）,
    // 于是「它不可达」这一事实若只写在注释里就会静默漂移。本测试把裁决变成可
    // 失败断言:若未来新增 IO 型变体,此处先红,提示同批解除 `Async` 的
    // `#[doc(hidden)]` 与 `scripts/l1_dead_surface_freeze.txt` 的预留登记。
    let io_bound: Vec<TaskKind> = TaskKind::ALL
        .iter()
        .copied()
        .filter(|kind| kind.is_io_bound())
        .collect();
    assert!(
        io_bound.is_empty(),
        "当前裁决假定无 IO 型 TaskKind;新增后须同步解除 Async 的 doc(hidden) 与冻结登记: {io_bound:?}"
    );

    let b = bridge();
    for kind in TaskKind::ALL {
        for n in [0usize, 1, 10_000, usize::MAX] {
            assert_ne!(
                b.route(kind, n),
                DispatchPlan::Async,
                "{kind:?} 在 n={n} 时不得落到预留的 Async 态"
            );
        }
    }
    // 变体本身必须仍存在且可比较（预留 ≠ 删除:下游比较/matches 不被破坏）
    assert_eq!(DispatchPlan::Async, DispatchPlan::Async);
}

#[test]
fn dispatch_plan_tristate_variants_are_mutually_distinct() {
    // 契约:三态互斥且可比较（路由结果可嵌入断言/日志,无需模式匹配解引用）
    let plans = [
        DispatchPlan::Inline,
        DispatchPlan::Rayon,
        DispatchPlan::Async,
    ];
    for (i, a) in plans.iter().enumerate() {
        for (j, b) in plans.iter().enumerate() {
            assert_eq!(i == j, a == b, "第 {i} 与第 {j} 态比较失真");
        }
    }
}
