//! P1-3:谱系 DAG 快照接入真实数据测试
//!
//! 覆盖:
//! - register 后全局快照新增节点(active/registered 状态)
//! - parent 链推导 evolves 边
//! - rollback 后状态迁移(active → deprecated / parent → active)
//! - reset_spec_dag 测试隔离
//!
//! WHY TEST_LOCK 串行化:SPEC_DAG 是进程级全局快照,rust test 默认多线程
//! 并行执行会相互污染(一个测试 reset 时另一个测试正在 register)。
//! 全局锁串行化本文件的快照相关测试,断言再按 spec name 过滤,
//! 与其他测试文件(integration.rs 等)的 register 调用互不干扰。

use std::sync::{Mutex, MutexGuard};

use gsoe_evolution::{spec_dag_snapshot, SpecRegistry};
use nexus_contracts::{ContractSpec, HarnessMeta, HarnessSpec, HopSpec, RetryPolicy};

/// 进程级测试锁 — 串行化所有触碰 SPEC_DAG 快照的测试
static TEST_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 构造最小合法 HarnessSpec(与 tests/integration.rs 同款结构)
fn make_spec(name: &str, version: u32, parent: Option<u32>) -> HarnessSpec {
    HarnessSpec {
        meta: HarnessMeta {
            name: name.to_string(),
            version,
            immutable: false,
            parent,
            task_type: None,
        },
        contracts: vec![ContractSpec {
            name: "no_panic".to_string(),
            property: "fuzz_target_must_not_panic".to_string(),
            description: None,
            from: None,
            to: None,
            fields: vec![],
        }],
        hops: vec![HopSpec {
            name: "generate_input".to_string(),
            input_type: None,
            output_type: None,
            contracts: vec!["no_panic".to_string()],
            description: None,
            order: vec!["Architect.propose".to_string()],
            on_veto: None,
            fallback: None,
        }],
        retry: RetryPolicy::default(),
        auxiliary: Some(
            "acceptance_gates = [\"tests_pass\", \"bench_no_regression\", \"invariants_clean\", \"redline_scan_clean\"]"
                .to_string(),
        ),
    }
}

/// 注册初始版本后快照出现 active 节点
#[test]
fn test_spec_dag_updates_on_register() {
    let _guard = lock();
    gsoe_evolution::reset_spec_dag();

    let mut registry = SpecRegistry::new();
    registry.register(make_spec("alpha", 1, None)).unwrap();

    let snapshot = spec_dag_snapshot();
    // 按 name 过滤断言(其他测试可能并行注册不同 name 的 spec)
    let alpha_nodes: Vec<_> = snapshot
        .nodes
        .iter()
        .filter(|n| n.id.starts_with("alpha@"))
        .collect();
    assert_eq!(alpha_nodes.len(), 1);
    assert_eq!(alpha_nodes[0].id, "alpha@v1");
    assert_eq!(alpha_nodes[0].status, "active");
    assert!(
        snapshot.edges.iter().all(|e| !e.from.starts_with("alpha@")),
        "初始版本无 parent,无 alpha 相关边"
    );
}

/// 注册子版本后快照新增节点与 evolves 边
#[test]
fn test_spec_dag_evolves_edge_from_parent() {
    let _guard = lock();
    gsoe_evolution::reset_spec_dag();

    let mut registry = SpecRegistry::new();
    registry.register(make_spec("alpha", 1, None)).unwrap();
    registry.register(make_spec("alpha", 2, Some(1))).unwrap();

    let snapshot = spec_dag_snapshot();
    let alpha_nodes: Vec<_> = snapshot
        .nodes
        .iter()
        .filter(|n| n.id.starts_with("alpha@"))
        .collect();
    assert_eq!(alpha_nodes.len(), 2);
    let evolves: Vec<_> = snapshot
        .edges
        .iter()
        .filter(|e| e.from.starts_with("alpha@") && e.to.starts_with("alpha@"))
        .collect();
    assert_eq!(evolves.len(), 1);
    assert_eq!(evolves[0].from, "alpha@v1");
    assert_eq!(evolves[0].to, "alpha@v2");
    assert_eq!(evolves[0].relation, "evolves");
    // register 不改变 active(v2 为 registered 状态)
    let n2 = snapshot.nodes.iter().find(|n| n.id == "alpha@v2").unwrap();
    assert_eq!(n2.status, "registered");
}

/// rollback 后旧 active 标 deprecated,parent 恢复 active
#[test]
fn test_spec_dag_rollback_marks_deprecated() {
    let _guard = lock();
    gsoe_evolution::reset_spec_dag();

    let mut registry = SpecRegistry::new();
    registry.register(make_spec("alpha", 1, None)).unwrap();
    registry.register(make_spec("alpha", 2, Some(1))).unwrap();
    // 激活 v2(模拟 promote)再回滚到 v1
    registry.set_candidate("alpha", 2).unwrap();
    registry.promote_candidate("alpha").unwrap();
    registry.rollback("alpha").unwrap();

    let snapshot = spec_dag_snapshot();
    let n2 = snapshot.nodes.iter().find(|n| n.id == "alpha@v2").unwrap();
    assert_eq!(n2.status, "deprecated");
    let n1 = snapshot.nodes.iter().find(|n| n.id == "alpha@v1").unwrap();
    assert_eq!(n1.status, "active");
}

/// 快照跨多个 registry 实例累积(全进程谱系)
#[test]
fn test_spec_dag_accumulates_across_registries() {
    let _guard = lock();
    gsoe_evolution::reset_spec_dag();

    let mut r1 = SpecRegistry::new();
    r1.register(make_spec("alpha", 1, None)).unwrap();
    let mut r2 = SpecRegistry::new();
    r2.register(make_spec("beta", 1, None)).unwrap();

    let snapshot = spec_dag_snapshot();
    assert!(
        snapshot.nodes.iter().any(|n| n.id == "alpha@v1"),
        "registry r1 的谱系入快照"
    );
    assert!(
        snapshot.nodes.iter().any(|n| n.id == "beta@v1"),
        "registry r2 的谱系入快照"
    );
}
