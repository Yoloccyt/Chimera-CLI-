//! 任务图 DAG 校验 — 基于 Kahn 算法的拓扑排序与环检测
//!
//! 对应架构层:L9 Quest
//!
//! # 设计决策(WHY)
//! - 选择 Kahn 算法而非 DFS 三色标记法:Kahn 天然产出拓扑序,
//!   一举两得(校验 + 排序),且实现简洁
//! - 依赖关系通过 `Task::dependencies: Vec<String>` 表达,
//!   引用其他 task_id;若引用不存在的 task_id 视为悬空依赖(返回错误)
//!
//! # 架构红线
//! - 所有 Quest 在创建时必须通过 DAG 校验(防止执行时死锁)
//! - GQEP 执行器按拓扑序调度 Task,确保依赖先完成

use std::collections::{HashMap, HashSet, VecDeque};

use nexus_core::Task;

use crate::error::QuestError;

/// 校验任务图无环 — 使用 Kahn 算法,有环返回 `CyclicDependency`
///
/// 算法步骤:
/// 1. 统计每个节点的入度(被依赖次数)
/// 2. 入度为 0 的节点入队
/// 3. 出队节点,将其后继节点入度减 1,若归零则入队
/// 4. 若出队节点数 < 总节点数,则存在环
pub fn validate_dag(tasks: &[Task]) -> Result<(), QuestError> {
    // 构建任务 ID 集合,用于校验依赖引用合法性
    let task_ids: HashSet<&str> = tasks.iter().map(|t| t.task_id.as_str()).collect();

    // 校验所有依赖引用都指向已存在的 task_id
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

    // 计算入度:task_id → 被依赖次数
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    for task in tasks {
        in_degree.entry(task.task_id.as_str()).or_insert(0);
    }
    for task in tasks {
        // task 依赖 dep,意味着 dep → task 存在边,task 的入度 +1
        for _ in &task.dependencies {
            *in_degree.entry(task.task_id.as_str()).or_insert(0) += 1;
        }
    }

    // 入度为 0 的节点入队
    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&id, _)| id)
        .collect();

    let mut processed = 0usize;
    while let Some(id) = queue.pop_front() {
        processed += 1;
        // 找到所有依赖 id 的 task,将其入度减 1
        for task in tasks {
            if task.dependencies.iter().any(|d| d == id) {
                if let Some(deg) = in_degree.get_mut(task.task_id.as_str()) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(task.task_id.as_str());
                    }
                }
            }
        }
    }

    if processed < tasks.len() {
        Err(QuestError::CyclicDependency)
    } else {
        Ok(())
    }
}

/// 返回拓扑排序后的 task_id 序列 — 入度为 0 的节点优先
///
/// 若存在环,返回 `CyclicDependency` 错误。
/// 同层(入度同时归零)节点的顺序按 task_id 字典序保证确定性。
pub fn topological_order(tasks: &[Task]) -> Result<Vec<String>, QuestError> {
    // 复用 validate_dag 的校验逻辑,提前检测环与悬空依赖
    validate_dag(tasks)?;

    // 重新计算入度(validate_dag 消耗了 in_degree,此处重建)
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    for task in tasks {
        in_degree.entry(task.task_id.as_str()).or_insert(0);
        for _ in &task.dependencies {
            *in_degree.entry(task.task_id.as_str()).or_insert(0) += 1;
        }
    }

    // 构建反向索引:dep_id → 依赖它的 task_id 列表
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for task in tasks {
        for dep in &task.dependencies {
            dependents
                .entry(dep.as_str())
                .or_default()
                .push(task.task_id.as_str());
        }
    }

    // 初始入队:入度为 0 的节点,按 task_id 字典序排序保证确定性
    let mut initial: Vec<&str> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&id, _)| id)
        .collect();
    initial.sort();
    let mut queue: VecDeque<&str> = initial.into_iter().collect();

    let mut order = Vec::with_capacity(tasks.len());
    while let Some(id) = queue.pop_front() {
        order.push(id.to_string());
        // 收集所有因 id 完成而入度归零的节点
        let mut newly_ready: Vec<&str> = Vec::new();
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
        // 按 task_id 字典序入队,保证拓扑序确定性
        newly_ready.sort();
        for rid in newly_ready {
            queue.push_back(rid);
        }
    }

    Ok(order)
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
}
