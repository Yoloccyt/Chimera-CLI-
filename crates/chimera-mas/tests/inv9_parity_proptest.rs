//! INV-9 双源**对账**测试（Spec v3.5 W6-5「INV-9 双源漂移 → 先加对账 proptest」）
//!
//! ## 为什么需要这个文件
//!
//! INV-9（委托图无环）在仓库里有**两份独立实现**：
//!
//! - **L9**：`chimera_mas::invariants::InvariantChecker::check_inv9_delegation_acyclic`
//! - **L5**：`gsoe_evolution::check_inv9_delegation_acyclic`（`ci_gate.rs:134`）
//!
//! L5 不能依赖 L9（向上违规），故各留一份镜像；此前两边的"语义镜像"只写在注释里
//! （`chimera-mas/src/error.rs:208-210`、`gsoe-evolution/src/ci_gate.rs:23-33`）——
//! 即本仓已定性过两次的**「注释即治理」**：改一方不会有任何机器信号。
//! ADR-045 决策 8 承认了这处偏差，终局目标是下沉 L0 后删除镜像（那部分需 ADR）；
//! 而**对账测试本身不需 ADR**，故先把"漂移必红"建立起来。
//!
//! ## 与已有对账测的关系（互补而非重复）
//!
//! `tests/invariants_test.rs:245-293`（早前批次的 W6-5 交付）已有一组**固定样本集**双源对账；
//! 本文件补的是固定集盖不到的三类形状：随机图上的**结论全等**（2000 例）、
//! 报错路径的**真实性**（路径每一段都得是输入里的真边，防“形状对、内容编”），
//! 以及**输入顺序无关性**（邻接表构建顺不得影响判定）。
//!
//! ## 判据（刻意只校"结论"，不校"路径字符串"）
//!
//! 两份实现的 DFS 起点/邻接表顺序可以合法不同，环路径的**具体序列**因此不保证相等；
//! 若把"逐字相等"当判据，会造出一只随哈希序抖动的假红门。故断言收敛为三条：
//!
//! 1. **结论全等**：`is_ok()` 必须一致（一方说无环、另一方说有环 ⇒ 真漂移 ⇒ 红）
//! 2. **报错皆为真环**：判红的任何一方，其 `cycle_path` 必须首尾相同、长度 ≥ 3、
//!    且每对相邻节点确实是输入里的一条边（防止把"路径形状"编出来）
//! 3. **双向可复核**：同一输入在两处得到的环路径可以不同，但都必须是该图里的环
//!
//! ## 依赖方向（零新增边）
//!
//! 本测试在 **L9** crate 内直接引用 `gsoe_evolution`——它是本 crate 的**现有生产依赖**
//! （`Cargo.toml:63`，L9 → L5 为**向下**边，依 §2.2 铁律合法，已册
//! `scripts/dep_edge_freeze.txt` 的 `chimera-mas|gsoe-evolution`）⇒
//! **不需也不应再加一条 dev-dependencies 同名录项**（本轮初稿就错加了这么一条，已删）。
//! 反向的 L5 → L9 才是被 ADR-045 决策 8 否掉的那条违规边。

use chimera_mas::error::MasError;
use chimera_mas::invariants::{DelegationEdge, InvariantChecker};
use gsoe_evolution::{check_inv9_delegation_acyclic as l5_check, DelegationEdge as L5Edge};
use proptest::prelude::*;
use std::collections::HashSet;

/// L9 结论：`(是否通过, 环路径)`
fn l9_verdict(edges: &[DelegationEdge]) -> (bool, Vec<String>) {
    match InvariantChecker::check_inv9_delegation_acyclic(edges) {
        Ok(()) => (true, Vec::new()),
        Err(MasError::DelegationCycleDetected { cycle_path }) => (false, cycle_path),
        Err(other) => panic!("INV-9 只应返回 DelegationCycleDetected，实际：{other:?}"),
    }
}

/// L5 结论：`(是否通过, 环路径)`
fn l5_verdict(edges: &[DelegationEdge]) -> (bool, Vec<String>) {
    let l5: Vec<L5Edge> = edges
        .iter()
        .map(|e| L5Edge::new(e.from.clone(), e.to.clone()))
        .collect();
    match l5_check(&l5) {
        Ok(()) => (true, Vec::new()),
        Err(cycle_path) => (false, cycle_path),
    }
}

/// 断言 `path` 确实是 `edges` 里的一个环：首尾相同、长度 ≥ 2、每对相邻点皆真实边
///
/// 长度下界取 2 而非 3：**自环 `A→A` 的路径形状就是 `[A, A]`**（见
/// `gsoe-evolution/src/ci_gate.rs:131` 的边界场景注：单节点环 ⇒ `[A, A]`），
/// 写 ≥3 会把正确行为误判为红（本轮首跑就是这样假红两个用例，改的是断言不是实现）。
///
/// 为何用普通 `assert!` 而不用 `prop_assert!`：本仓 workspace lints 禁 `unwrap_used`
/// （连 dev profile 也禁，本轮 clippy 实拦），而 `prop_assert!` 要求本函数返回
/// `TestCaseResult`，调用处就不可避免地要 unwrap。proptest 会把 panic 当失败报告并
/// 继续收缩输入，因此不损失可定位性。
fn assert_is_real_cycle(path: &[String], edges: &[DelegationEdge], who: &str) {
    assert!(
        path.len() >= 2,
        "{who} 报出的路径长度不足 2，不构成任何环：{path:?}"
    );
    assert_eq!(
        path[0],
        path[path.len() - 1],
        "{who} 环路径首尾不同：{path:?}"
    );
    let pool: HashSet<(String, String)> = edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone()))
        .collect();
    for w in path.windows(2) {
        assert!(
            pool.contains(&(w[0].clone(), w[1].clone())),
            "{who} 的环路径含非输入边：{}→{}（路径 {path:?}）",
            w[0],
            w[1]
        );
    }
}

/// 共用的边生成器：先出一个随机 DAG，再以随机概率追加一条"回边"（可能构成自环/两节点环）
#[derive(Debug, Clone)]
struct Graph {
    edges: Vec<DelegationEdge>,
}

fn arb_graph() -> impl Strategy<Value = Graph> {
    (
        3usize..9,
        0usize..14,
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(n, extra, back, self_loop, dup)| {
            let mut edges: Vec<DelegationEdge> = Vec::new();
            // 主干：i → j 且 i < j，天然无环
            for i in 0..n - 1 {
                edges.push(DelegationEdge::new(format!("n{i}"), format!("n{}", i + 1)));
            }
            // 额外前向边（仍无环）
            for k in 0..extra {
                let a = (k * 3) % (n - 1);
                let b = a + 1 + ((k * 5) % (n - a - 1)).max(1);
                if b < n {
                    edges.push(DelegationEdge::new(format!("n{a}"), format!("n{b}")));
                }
            }
            // 回边：制造真环
            if back && n >= 3 {
                edges.push(DelegationEdge::new(format!("n{}", n - 1), "n0"));
            }
            // 自环：单节点环
            if self_loop {
                edges.push(DelegationEdge::new("n0", "n0"));
            }
            // 重复边：考验去重与稳定性
            if dup && !edges.is_empty() {
                edges.push(edges[0].clone());
            }
            Graph { edges }
        })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 2000,
        .. ProptestConfig::default()
    })]

    /// 性质 1 — 结论全等：两份实现对任意随机图必须给出相同的"有环/无环"判定
    #[test]
    fn inv9_verdicts_agree(graph in arb_graph()) {
        let (l9_ok, l9_path) = l9_verdict(&graph.edges);
        let (l5_ok, l5_path) = l5_verdict(&graph.edges);
        prop_assert_eq!(
            l9_ok, l5_ok,
            "双源漂移：L9 ok={} path={:?} vs L5 ok={} path={:?}，输入 {:?}",
            l9_ok,
            l9_path,
            l5_ok,
            l5_path,
            graph.edges
        );
    }

    /// 性质 2 — 报错皆为真环：任一方判红时，其环路径必须是输入图中真实存在的环
    #[test]
    fn inv9_reported_cycles_are_real(graph in arb_graph()) {
        let (l9_ok, l9_path) = l9_verdict(&graph.edges);
        let (l5_ok, l5_path) = l5_verdict(&graph.edges);
        if !l9_ok {
            assert_is_real_cycle(&l9_path, &graph.edges, "L9");
        }
        if !l5_ok {
            assert_is_real_cycle(&l5_path, &graph.edges, "L5");
        }
    }

    /// 性质 3 — 顺序无关：把输入边打乱，两方的结论都必须不变（防"邻接表顺序影响判定"）
    #[test]
    fn inv9_is_order_insensitive(graph in arb_graph(), seed in any::<u64>()) {
        let mut rev = graph.edges.clone();
        rev.reverse();
        let _ = seed; // 保留 seed 参数位以便日后扩展多轮置换
        let a = l9_verdict(&graph.edges).0;
        let b = l9_verdict(&rev).0;
        prop_assert_eq!(a, b, "L9 结论随输入边顺序改变");
        let c = l5_verdict(&graph.edges).0;
        let d = l5_verdict(&rev).0;
        prop_assert_eq!(c, d, "L5 结论随输入边顺序改变");
    }
}

/// 边界矩阵：两边必须逐项同结论（proptest 的随机图不保证覆盖这些形状）
#[test]
fn inv9_parity_boundary_matrix() {
    let cases: Vec<(&str, Vec<DelegationEdge>)> = vec![
        ("空图", Vec::new()),
        ("单条边 A→B", vec![DelegationEdge::new("A", "B")]),
        ("自环 A→A", vec![DelegationEdge::new("A", "A")]),
        (
            "两节点环 A→B→A",
            vec![DelegationEdge::new("A", "B"), DelegationEdge::new("B", "A")],
        ),
        (
            "三节点环 A→B→C→A",
            vec![
                DelegationEdge::new("A", "B"),
                DelegationEdge::new("B", "C"),
                DelegationEdge::new("C", "A"),
            ],
        ),
        (
            "菱形 DAG（无环）",
            vec![
                DelegationEdge::new("A", "B"),
                DelegationEdge::new("A", "C"),
                DelegationEdge::new("B", "D"),
                DelegationEdge::new("C", "D"),
            ],
        ),
        (
            "两连通分量，其一含环",
            vec![
                DelegationEdge::new("A", "B"),
                DelegationEdge::new("B", "C"),
                DelegationEdge::new("X", "Y"),
                DelegationEdge::new("Y", "X"),
            ],
        ),
        (
            "重复边（同一条写两次）",
            vec![DelegationEdge::new("A", "B"), DelegationEdge::new("A", "B")],
        ),
        (
            "孤点在 to 侧（X 无出边）",
            vec![DelegationEdge::new("A", "B"), DelegationEdge::new("B", "X")],
        ),
    ];

    for (name, edges) in &cases {
        let (l9_ok, l9_path) = l9_verdict(edges);
        let (l5_ok, l5_path) = l5_verdict(edges);
        assert_eq!(
            l9_ok, l5_ok,
            "边界用例「{name}」双源结论漂移：L9={l9_ok}({l9_path:?}) L5={l5_ok}({l5_path:?})"
        );
        if !l9_ok {
            assert_is_real_cycle(&l9_path, edges, "L9");
            assert_is_real_cycle(&l5_path, edges, "L5");
        }
    }
}
