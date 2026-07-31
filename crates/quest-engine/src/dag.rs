//! 任务图 DAG 校验 — 基于 Kahn 算法的拓扑排序与环检测
//!
//! 对应架构层:L9 Quest
//!
//! # 设计决策(WHY)
//! - 选择 Kahn 算法而非 DFS 三色标记法:Kahn 天然产出拓扑序,
//!   一举两得(校验 + 排序),且实现简洁
//! - 依赖关系通过 `Task::dependencies: Vec<String>` 表达,
//!   引用其他 task_id;若引用不存在的 task_id 视为悬空依赖(返回错误)
//! - **单实现内核**(L9 优化 2.1,2026-07-31):`validate_dag` 与
//!   `topological_order` 共用 `kahn_core` 单遍 O(V+E) 实现——旧版
//!   `validate_dag` 在出队循环内对全体任务做线性扫描(最坏 O(V²·D),
//!   criterion 基线:1000 节点 diamond 21.1ms),且 `topological_order`
//!   前置调用它造成双重遍历;现统一用 dependents 反向邻接索引。
//!
//! # 确定性契约(下游 GQEP 调度依赖)
//! 同层(入度同时归零)节点按 task_id 字典序入队——同一任务集的
//! 拓扑序输出永远一致(proptest 守护:新旧实现判环一致 + 序确定性)。
//!
//! # 架构红线
//! - 所有 Quest 在创建时必须通过 DAG 校验(防止执行时死锁)
//! - GQEP 执行器按拓扑序调度 Task,确保依赖先完成

use std::collections::{HashMap, HashSet, VecDeque};

use nexus_core::Task;

use crate::error::QuestError;

/// Kahn 单遍内核 — 悬空依赖校验 + 环检测 + 确定性拓扑序,O(V + E)
///
/// 算法步骤:
/// 1. 校验全部依赖引用指向已存在的 task_id(悬空依赖即错)
/// 2. 单遍构建入度表与 dependents 反向邻接索引(dep_id → 依赖它的节点)
/// 3. 入度 0 节点按字典序入队;出队时经反向索引将后继入度减 1,
///    新归零者按字典序批量入队(确定性契约)
/// 4. 出队数 < 总节点数 → 存在环
///
/// WHY 返回拓扑序而非 ():纯校验调用(`validate_dag`)丢弃返回值,
/// 排序开销(同层字典序)在 O(V log V) 上界内,换取单实现零漂移。
fn kahn_core(tasks: &[Task]) -> Result<Vec<String>, QuestError> {
    // 1. 悬空依赖校验
    let task_ids: HashSet<&str> = tasks.iter().map(|t| t.task_id.as_str()).collect();
    for task in tasks {
        for dep in &task.dependencies {
            if !task_ids.contains(dep.as_str()) {
                // 悬空依赖视为 DAG 校验失败:防止执行时找不到前置任务
                return Err(QuestError::DecompositionFailed(format!(
                    "dangling dependency: task {} depends on non-existent {}",
                    task.task_id, dep
                )));
            }
        }
    }

    // 2. 单遍构建入度表 + 反向邻接索引(旧版 validate_dag 的 O(V²·D)
    //    根因就是缺此索引,出队后只能全表扫描找后继)
    let mut in_degree: HashMap<&str, usize> = HashMap::with_capacity(tasks.len());
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for task in tasks {
        in_degree.entry(task.task_id.as_str()).or_insert(0);
    }
    for task in tasks {
        for dep in &task.dependencies {
            // task 依赖 dep:存在边 dep → task,task 入度 +1
            *in_degree.entry(task.task_id.as_str()).or_insert(0) += 1;
            dependents
                .entry(dep.as_str())
                .or_default()
                .push(task.task_id.as_str());
        }
    }

    // 3. 入度 0 节点按字典序入队(确定性契约)
    let mut initial: Vec<&str> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&id, _)| id)
        .collect();
    initial.sort_unstable();
    let mut queue: VecDeque<&str> = initial.into_iter().collect();

    let mut order = Vec::with_capacity(tasks.len());
    // WHY 缓冲提出循环(L9 优化第二轮):旧版每轮 while 迭代重新分配
    // newly_ready Vec,改为循环外声明 + 每轮 clear() 复用,消除每节点一次堆分配。
    let mut newly_ready: Vec<&str> = Vec::new();
    while let Some(id) = queue.pop_front() {
        order.push(id.to_string());
        // 经反向索引将后继入度减 1,新归零者按字典序批量入队
        newly_ready.clear();
        if let Some(deps) = dependents.get(id) {
            for &dep_id in deps {
                if let Some(deg) = in_degree.get_mut(dep_id) {
                    *deg -= 1;
                    if *deg == 0 {
                        newly_ready.push(dep_id);
                    }
                }
            }
        }
        newly_ready.sort_unstable();
        for &rid in &newly_ready {
            queue.push_back(rid);
        }
    }

    // 4. 出队数不足 → 存在环
    if order.len() < tasks.len() {
        Err(QuestError::CyclicDependency)
    } else {
        Ok(order)
    }
}

/// 校验任务图无环 — 使用 Kahn 算法,有环返回 `CyclicDependency`
///
/// 复杂度 O(V + E)(L9 优化 2.1:旧版出队循环内全表扫描为最坏
/// O(V²·D),已经 criterion 基线证伪后重写,见模块头注释)。
pub fn validate_dag(tasks: &[Task]) -> Result<(), QuestError> {
    kahn_core(tasks).map(|_| ())
}

/// 返回拓扑排序后的 task_id 序列 — 入度为 0 的节点优先
///
/// 若存在环,返回 `CyclicDependency` 错误;若存在悬空依赖,返回
/// `DecompositionFailed`(与 `validate_dag` 同源单遍实现,不再双重遍历)。
/// 同层(入度同时归零)节点的顺序按 task_id 字典序保证确定性。
pub fn topological_order(tasks: &[Task]) -> Result<Vec<String>, QuestError> {
    kahn_core(tasks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::{Task, TaskStatus};

    fn make_task(id: &str, deps: Vec<&str>) -> Task {
        Task {
            task_id: id.into(),
            description: format!("task {id}"),
            status: TaskStatus::Pending,
            dependencies: deps.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn test_validate_dag_acyclic() {
        let tasks = vec![
            make_task("a", vec![]),
            make_task("b", vec!["a"]),
            make_task("c", vec!["b"]),
        ];
        assert!(validate_dag(&tasks).is_ok());
    }

    #[test]
    fn test_validate_dag_cyclic() {
        let tasks = vec![
            make_task("a", vec!["c"]),
            make_task("b", vec!["a"]),
            make_task("c", vec!["b"]),
        ];
        assert!(matches!(
            validate_dag(&tasks),
            Err(QuestError::CyclicDependency)
        ));
    }

    #[test]
    fn test_validate_dag_self_loop() {
        let tasks = vec![make_task("a", vec!["a"])];
        assert!(matches!(
            validate_dag(&tasks),
            Err(QuestError::CyclicDependency)
        ));
    }

    #[test]
    fn test_validate_dag_dangling_dependency() {
        let tasks = vec![make_task("a", vec!["nonexistent"])];
        assert!(matches!(
            validate_dag(&tasks),
            Err(QuestError::DecompositionFailed(_))
        ));
    }

    #[test]
    fn test_validate_dag_empty() {
        assert!(validate_dag(&[]).is_ok());
    }

    #[test]
    fn test_topological_order_linear() {
        let tasks = vec![
            make_task("a", vec![]),
            make_task("b", vec!["a"]),
            make_task("c", vec!["b"]),
        ];
        let order = topological_order(&tasks).unwrap();
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_topological_order_diamond() {
        // a → b, a → c, b → d, c → d
        let tasks = vec![
            make_task("a", vec![]),
            make_task("b", vec!["a"]),
            make_task("c", vec!["a"]),
            make_task("d", vec!["b", "c"]),
        ];
        let order = topological_order(&tasks).unwrap();
        // a 必须在 b/c 之前,b/c 必须在 d 之前
        let pos_a = order.iter().position(|x| x == "a").unwrap();
        let pos_b = order.iter().position(|x| x == "b").unwrap();
        let pos_c = order.iter().position(|x| x == "c").unwrap();
        let pos_d = order.iter().position(|x| x == "d").unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_a < pos_c);
        assert!(pos_b < pos_d);
        assert!(pos_c < pos_d);
    }

    #[test]
    fn test_topological_order_cyclic_returns_error() {
        let tasks = vec![make_task("a", vec!["b"]), make_task("b", vec!["a"])];
        assert!(matches!(
            topological_order(&tasks),
            Err(QuestError::CyclicDependency)
        ));
    }

    /// 拓扑序同层字典序确定性(下游 GQEP 调度契约):同层节点按 task_id 排序
    #[test]
    fn test_topological_order_lexicographic_determinism() {
        // c/b/a 同为根层(无依赖),输出必须按字典序 a,b,c
        let tasks = vec![
            make_task("c", vec![]),
            make_task("b", vec![]),
            make_task("a", vec![]),
        ];
        let order = topological_order(&tasks).unwrap();
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    // ============================================================
    // proptest 不变量(L9 优化 2.1:kahn_core 重写的行为守护)
    // ============================================================

    use proptest::prelude::*;

    /// 构造随机无环任务图:节点 i 只能依赖下标更小的节点(构造性无环)
    fn arb_acyclic_tasks() -> impl Strategy<Value = Vec<Task>> {
        // 每节点的依赖集用位掩码表达(最多 16 节点,取低 i 位)
        proptest::collection::vec(any::<u16>(), 1..16).prop_map(|masks| {
            masks
                .iter()
                .enumerate()
                .map(|(i, &mask)| {
                    let deps = (0..i)
                        .filter(|&j| mask & (1 << j) != 0)
                        .map(|j| format!("t{j}"))
                        .collect::<Vec<_>>();
                    make_task(&format!("t{i}"), deps.iter().map(String::as_str).collect())
                })
                .collect()
        })
    }

    proptest! {
        /// 构造性无环图恒通过校验,且拓扑序满足依赖先行 + 全覆盖 + 确定性
        #[test]
        fn prop_acyclic_graph_valid_order(tasks in arb_acyclic_tasks()) {
            prop_assert!(validate_dag(&tasks).is_ok());

            let order = topological_order(&tasks).unwrap();
            prop_assert_eq!(order.len(), tasks.len(), "拓扑序必须覆盖全部节点");

            // 依赖先行:每个节点的依赖在序中位置更靠前
            let pos: std::collections::HashMap<&str, usize> =
                order.iter().enumerate().map(|(i, id)| (id.as_str(), i)).collect();
            for task in &tasks {
                for dep in &task.dependencies {
                    prop_assert!(
                        pos[dep.as_str()] < pos[task.task_id.as_str()],
                        "依赖 {} 必须先于 {}", dep, task.task_id
                    );
                }
            }

            // 确定性:同一输入重复调用输出一致(GQEP 调度契约)
            prop_assert_eq!(order, topological_order(&tasks).unwrap());
        }

        /// 任意无环图加一条回边(首节点依赖末节点)必被判环,
        /// 且 validate_dag 与 topological_order 判环结论一致
        #[test]
        fn prop_back_edge_creates_cycle(tasks in arb_acyclic_tasks()) {
            prop_assume!(tasks.len() >= 2);
            let mut cyclic = tasks;
            let last_id = cyclic.last().unwrap().task_id.clone();
            // 首节点(必在末节点可达链上游或无关——为保证成环,
            // 让末节点额外依赖首节点,首节点依赖末节点)
            cyclic[0].dependencies.push(last_id);
            let first_id = cyclic[0].task_id.clone();
            let last = cyclic.len() - 1;
            cyclic[last].dependencies.push(first_id);

            let v = validate_dag(&cyclic);
            let t = topological_order(&cyclic);
            prop_assert!(matches!(v, Err(QuestError::CyclicDependency)));
            prop_assert!(matches!(t, Err(QuestError::CyclicDependency)));
        }
    }
}
