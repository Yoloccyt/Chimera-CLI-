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
//! # 确定性契约(定序结果由 `create_quest` 消费)
//! 同层(入度同时归零)节点按 task_id 字典序入队——同一任务集的
//! 拓扑序输出永远一致(proptest 守护:新旧实现判环一致 + 序确定性)。
//!
//! # 架构红线
//! - 所有 Quest 在创建时必须通过 DAG 校验(防止执行时死锁)
//! - 定序在 `create_quest` 入口生效;`gqep-executor` 当前不消费 `Task::dependencies`

use std::collections::{HashMap, HashSet, VecDeque};

use nexus_core::Task;

use crate::error::QuestError;

/// Kahn 图状态 — 入度表(task_id → 入度)+ dependents 反向邻接索引(dep_id → 依赖它的节点)
///
/// WHY 独立 type 别名:元组嵌套 HashMap 触发 clippy type_complexity;
/// 命名后两个消费方(kahn_core/kahn_layers)的解构语义也更可读。
type KahnState<'a> = (HashMap<&'a str, usize>, HashMap<&'a str, Vec<&'a str>>);

/// Kahn 图状态构建 — 悬空依赖校验 + 入度表 + dependents 反向邻接索引
///
/// WHY 从 kahn_core 提取共享(WP-E):`kahn_core`(拓扑序)与 `kahn_layers`
/// (波次分层)消费同一份图状态,各写一份即两处潜在分叉——悬空/判环语义漂移。
/// 提取后 `kahn_core` 的算法行为零变化(纯代码搬家,既有 proptest 守护)。
fn build_kahn_state(tasks: &[Task]) -> Result<KahnState<'_>, QuestError> {
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
    Ok((in_degree, dependents))
}

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
    let (mut in_degree, dependents) = build_kahn_state(tasks)?;

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

/// Kahn 逐层剥叶 — 与 kahn_core 同源图状态上的波次分层,O(V + E)
///
/// 算法:初始层 = 入度 0 节点(字典序);每层处理全部节点并将后继入度减 1,
/// 新归零者组成下一层(字典序)。已分层节点数 < 总节点数 → 存在环
/// (与 kahn_core 的出队判据同构:重复 task_id 使入度表去重,同样落此分支)。
///
/// WHY 不复用 kahn_core 的输出再二次分组:kahn_core 的同层节点可跨多轮
/// 出队(依赖链长度不同),其序中同层节点不保证相邻,二次分组需另算层号;
/// 直接剥叶一遍即得分层,且天然满足"层内字典序"确定性契约。
fn kahn_layers(tasks: &[Task]) -> Result<Vec<Vec<String>>, QuestError> {
    let (mut in_degree, dependents) = build_kahn_state(tasks)?;

    // 初始层:入度 0 节点按字典序(确定性契约)
    let mut current: Vec<&str> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&id, _)| id)
        .collect();
    current.sort_unstable();

    let mut layers: Vec<Vec<String>> = Vec::new();
    let mut done = 0usize;
    while !current.is_empty() {
        let mut next: Vec<&str> = Vec::with_capacity(current.len());
        let mut layer: Vec<String> = Vec::with_capacity(current.len());
        for &id in &current {
            layer.push(id.to_string());
            done += 1;
            // 经反向索引将后继入度减 1,新归零者进入下一层
            if let Some(deps) = dependents.get(id) {
                for &dep_id in deps {
                    if let Some(deg) = in_degree.get_mut(dep_id) {
                        *deg -= 1;
                        if *deg == 0 {
                            next.push(dep_id);
                        }
                    }
                }
            }
        }
        next.sort_unstable();
        // current 已按字典序排序,layer 顺序天然字典序(确定性契约)
        layers.push(layer);
        current = next;
    }

    // 已分层节点数不足 → 存在环(与 kahn_core 判据同构)
    if done < tasks.len() {
        Err(QuestError::CyclicDependency)
    } else {
        Ok(layers)
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

/// 按拓扑分层 — 返回波次执行序:同层任务可并行,层间必须串行(依赖序)
///
/// WP-E(R7 批次):`run --execute` 的 HeuristicDag 波次路径消费本函数。
///
/// # 错误语义(与 `topological_order` 零漂移)
/// 悬空依赖返回 `DecompositionFailed`,成环返回 `CyclicDependency`——
/// 与 `kahn_core` 同源单实现(`build_kahn_state`),不养第二套判环语义。
///
/// # 确定性契约
/// 层内按 task_id 字典序(与 `topological_order` 同层字典序一致);
/// 同一任务集的分层输出永远一致。空输入返回空层表(0 层)。
pub fn topological_layers(tasks: &[Task]) -> Result<Vec<Vec<String>>, QuestError> {
    kahn_layers(tasks)
}

/// 按拓扑序重排任务表 —— 保证每个任务都排在其全部依赖之后
///
/// 参数:`tasks` 按值接收(所有权移入,避免调用方先 clone 再排序的冗余分配)
///
/// 返回:`Ok(Vec<Task>)` 为输入的**排列**(元素不增不减,仅顺序符合依赖);
/// 存在环返回 `CyclicDependency`,悬空依赖返回 `DecompositionFailed`
/// (两者均与 `topological_order` 同源,错误语义零漂移)。
///
/// WHY 独立成函数:`topological_order` 只产出 task_id 序,调用方仍需把序映射回
/// `Task` 实体。把这一步固化在 dag.rs 内而非散落到各调用方 —— 依赖序是调度
/// 正确性前提,各写一份重排即多份潜在分叉。
///
/// WHY 用 HashMap drain 而非 `sort_by_key`:前者 O(V+E) 且无需为"理论上不可达"
/// 的查表失败分支写 `unwrap_or` 兜底(兜底值会掩盖逻辑错误);后者是
/// O(V log V) 比较排序。本仓红线要求可线性化的场景不做多余排序。
///
/// 确定性契约:继承 `kahn_core` 的同层字典序入队(:74-80/101),故同输入恒
/// 得同输出;幂等(`sort(sort(x)) == sort(x)`),proptest 守护。
pub fn sort_tasks_topologically(tasks: Vec<Task>) -> Result<Vec<Task>, QuestError> {
    // 单点取序:此处经公开 API `topological_order` 走 kahn_core,
    // 与 `validate_dag` 共享同一判环语义,不另立第二套。
    let order = topological_order(&tasks)?;
    // `topological_order` 已成功,故 `order` 与 `tasks` 的 id 集同构:
    // 重复 task_id 会让 `kahn_core` 的入度表去重,使 `order.len() < tasks.len()`
    // 而提前判 `CyclicDependency` 退出,不可能走到这里静默丢任务。
    let mut by_id: HashMap<String, Task> =
        tasks.into_iter().map(|t| (t.task_id.clone(), t)).collect();
    let sorted: Vec<Task> = order.iter().filter_map(|id| by_id.remove(id)).collect();
    // 内部逻辑不变量(非系统边界防御):release 零成本,debug 下若未来有人
    // 改动 kahn_core 使序与集不等,立即暴露而非静默丢任务。
    debug_assert_eq!(
        sorted.len(),
        order.len(),
        "kahn_core 成功即保证 order 与 tasks 同集同量"
    );
    Ok(sorted)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
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
    // sort_tasks_topologically — 依赖定序接入生产路径后的行为守护
    // ============================================================

    /// 断言任务表中每个任务都排在其全部依赖之后（测试专用助手）
    fn assert_dep_first(tasks: &[Task]) {
        let pos: HashMap<&str, usize> = tasks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.task_id.as_str(), i))
            .collect();
        for task in tasks {
            for dep in &task.dependencies {
                assert!(
                    pos[dep.as_str()] < pos[task.task_id.as_str()],
                    "依赖 {dep} 必须先于 {}",
                    task.task_id
                );
            }
        }
    }

    /// diamond 以打乱输入序给入,输出必须满足依赖先行
    ///
    /// 这是本函数的核心断言:输入序与依赖序**故意不一致**
    /// (d 排在 b/c 之前、a 排在最后),重排后 a→(b,c)→d 必须成立。
    #[test]
    fn sort_topo_diamond_reordered() {
        let shuffled = vec![
            make_task("d", vec!["b", "c"]),
            make_task("a", vec![]),
            make_task("c", vec!["a"]),
            make_task("b", vec!["a"]),
        ];
        let sorted = sort_tasks_topologically(shuffled).unwrap();
        let ids: Vec<&str> = sorted.iter().map(|t| t.task_id.as_str()).collect();
        // 根层 a 必在首位;同层 b/c 按字典序(b 先于 c);d 收尾
        assert_eq!(ids, vec!["a", "b", "c", "d"]);
        assert_dep_first(&sorted);
    }

    /// 输出是输入的排列:不增不减(守住 HashMap drain 不丢任务)
    #[test]
    fn sort_topo_is_permutation() {
        let input = vec![
            make_task("d", vec!["b", "c"]),
            make_task("a", vec![]),
            make_task("c", vec!["a"]),
            make_task("b", vec!["a"]),
        ];
        let sorted = sort_tasks_topologically(input.clone()).unwrap();
        assert_eq!(sorted.len(), input.len());
        let mut got: Vec<String> = sorted.iter().map(|t| t.task_id.clone()).collect();
        let mut want: Vec<String> = input.iter().map(|t| t.task_id.clone()).collect();
        got.sort();
        want.sort();
        assert_eq!(got, want, "输出 id 多重集必须等于输入");
        // 任务实体内容随序搬迁,不得被篡改
        assert_eq!(
            sorted
                .iter()
                .find(|t| t.task_id == "d")
                .unwrap()
                .dependencies,
            vec!["b", "c"]
        );
    }

    /// 幂等:对已定序的表再排一次必得同一结果(不产生振荡)
    #[test]
    fn sort_topo_idempotent() {
        let input = vec![
            make_task("c", vec!["a", "b"]),
            make_task("b", vec!["a"]),
            make_task("a", vec![]),
        ];
        let once = sort_tasks_topologically(input.clone()).unwrap();
        let twice = sort_tasks_topologically(once.clone()).unwrap();
        assert_eq!(once, twice, "定序必须幂等");
    }

    /// I1 逐位等价闸:线性链按句子序输入 ⇒ 输出与输入**逐位相等**
    ///
    /// WHY 这条测试是本批变更的安全基座:`decompose()` 产出的就是按序线性链
    /// (task_i 依赖 task_{i-1}),其拓扑序恒等于句子序,故接线后当前唯一
    /// 生产者的输出零行为变更。若此测试转红,说明定序改了生产路径输出。
    #[test]
    fn sort_topo_linear_chain_unchanged() {
        let chain = vec![
            make_task("t0", vec![]),
            make_task("t1", vec!["t0"]),
            make_task("t2", vec!["t1"]),
            make_task("t3", vec!["t2"]),
        ];
        let sorted = sort_tasks_topologically(chain.clone()).unwrap();
        assert_eq!(
            sorted, chain,
            "按序线性链必须逐位不变(守护 decompose 产线零行为变更)"
        );
    }

    /// 有环输入必判 `CyclicDependency`,与 `topological_order` 同源
    #[test]
    fn sort_topo_cycle_rejected() {
        let cyclic = vec![make_task("a", vec!["b"]), make_task("b", vec!["a"])];
        assert!(matches!(
            sort_tasks_topologically(cyclic),
            Err(QuestError::CyclicDependency)
        ));
    }

    /// 悬空依赖必判 `DecompositionFailed`,与 `topological_order` 同源
    #[test]
    fn sort_topo_dangling_rejected() {
        let dangling = vec![make_task("a", vec![]), make_task("b", vec!["nonexistent"])];
        assert!(matches!(
            sort_tasks_topologically(dangling),
            Err(QuestError::DecompositionFailed(_))
        ));
    }

    /// 重复 task_id 必报错而非静默丢任务(锁死实现注释里的所有权论断)
    ///
    /// 风险场景:若重复 id 被 HashMap 折叠,两个任务会只剩一个 —— 本测试
    /// 钉住该分支不可达(`kahn_core` 入度表去重使 order.len() < tasks.len() 先触发成环判定)。
    #[test]
    fn sort_topo_duplicate_id_rejected() {
        let dup = vec![make_task("a", vec![]), make_task("a", vec![])];
        let result = sort_tasks_topologically(dup);
        assert!(result.is_err(), "重复 task_id 不得被静默折叠,必须报错");
    }

    /// 空输入 → 空输出且 Ok(不 panic)
    #[test]
    fn sort_topo_empty() {
        assert_eq!(sort_tasks_topologically(Vec::new()).unwrap(), Vec::new());
    }

    /// 确定性:同一输入两次调用输出逐位相等(继承 kahn_core 同层字典序契约)
    #[test]
    fn sort_topo_deterministic() {
        let input = vec![
            make_task("z", vec![]),
            make_task("m", vec![]),
            make_task("a", vec![]),
            make_task("leaf", vec!["z", "m", "a"]),
        ];
        let first = sort_tasks_topologically(input.clone()).unwrap();
        let second = sort_tasks_topologically(input).unwrap();
        assert_eq!(first, second, "定序不得依赖 HashMap 迭代随机性");
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

    /// 输入序扰动助手：把任务表循环右移 k 位，制造与依赖序不一致的输入序
    ///
    /// WHY 用循环移位而非随机全排列：它足以破坏"输入序恰好等于依赖序"这一
    /// 巧合，且确定、可读、无额外依赖；真正的全排列空间由 `arb_acyclic_tasks`
    /// 自身的图多样性 + 多个 k 值共同覆盖。
    fn rotated(tasks: &[Task], k: usize) -> Vec<Task> {
        let n = tasks.len();
        if n == 0 {
            return Vec::new();
        }
        let k = k % n;
        // 右移 k 位：末 k 个元素搬到队首，其余依次后移
        tasks[n - k..]
            .iter()
            .chain(tasks[..n - k].iter())
            .cloned()
            .collect()
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

        /// 定序输出四重不变量:排列守恒 + 依赖先行 + 与 id 序同根 + 幂等
        ///
        /// WHY 要拿 `rotated` 把输入序故意打乱:线性序输入下定序是恒等映射,
        /// 只能证"不改坏",证不了"能改对"。只有输入序≠依赖序时仍断言
        /// 依赖先行,才能堵住 HashMap drain 错序/丢任务这类真 bug。
        #[test]
        fn prop_sort_topo_permutation_dep_first(tasks in arb_acyclic_tasks(), k in 0usize..32usize) {
            let shuffled = rotated(&tasks, k);
            let sorted = sort_tasks_topologically(shuffled.clone()).unwrap();

            // 1. 排列守恒:不得增删任务
            prop_assert_eq!(sorted.len(), shuffled.len());
            let mut got: Vec<String> = sorted.iter().map(|t| t.task_id.clone()).collect();
            let mut want: Vec<String> = shuffled.iter().map(|t| t.task_id.clone()).collect();
            got.sort();
            want.sort();
            prop_assert_eq!(got, want, "定序不得增删任务");

            // 2. 依赖先行
            let pos: std::collections::HashMap<&str, usize> = sorted
                .iter()
                .enumerate()
                .map(|(i, t)| (t.task_id.as_str(), i))
                .collect();
            for t in &sorted {
                for d in &t.dependencies {
                    prop_assert!(
                        pos[d.as_str()] < pos[t.task_id.as_str()],
                        "依赖 {} 必须先于 {}",
                        d,
                        t.task_id
                    );
                }
            }

            // 3. 实体序 == id 序:两者必须同根,不得养出第二套定序语义
            let ids: Vec<String> = sorted.iter().map(|t| t.task_id.clone()).collect();
            prop_assert_eq!(
                ids,
                topological_order(&shuffled).unwrap(),
                "实体序必须逐项等于 topological_order 的 id 序"
            );

            // 4. 幂等
            prop_assert_eq!(
                sorted.clone(),
                sort_tasks_topologically(sorted).unwrap(),
                "定序必须幂等"
            );
        }

        /// 成环输入被判环且绝不 panic：定序与两个公开校验 API 三方结论一致
        #[test]
        fn prop_sort_topo_cyclic_rejected(tasks in arb_acyclic_tasks()) {
            prop_assume!(tasks.len() >= 2);
            let mut cyclic = tasks;
            let last_id = cyclic.last().unwrap().task_id.clone();
            let first_id = cyclic[0].task_id.clone();
            // 双向往返边→必成环(0 依 last,last 依 0)
            cyclic[0].dependencies.push(last_id);
            let n = cyclic.len() - 1;
            cyclic[n].dependencies.push(first_id);

            prop_assert!(matches!(
                sort_tasks_topologically(cyclic.clone()),
                Err(QuestError::CyclicDependency)
            ));
            prop_assert!(matches!(
                topological_order(&cyclic),
                Err(QuestError::CyclicDependency)
            ));
            prop_assert!(matches!(
                validate_dag(&cyclic),
                Err(QuestError::CyclicDependency)
            ));
        }
    }

    // ============================================================
    // topological_layers — 波次分层(WP-E)的行为守护
    // ============================================================

    /// 线性链 → N 层每层恰 1 个任务(完全串行,无并行度)
    #[test]
    fn test_topological_layers_linear_chain() {
        let tasks = vec![
            make_task("a", vec![]),
            make_task("b", vec!["a"]),
            make_task("c", vec!["b"]),
        ];
        let layers = topological_layers(&tasks).unwrap();
        assert_eq!(layers, vec![vec!["a"], vec!["b"], vec!["c"]]);
    }

    /// 菱形 DAG → 三层:b/c 同层并行,d 收尾
    #[test]
    fn test_topological_layers_diamond() {
        let tasks = vec![
            make_task("a", vec![]),
            make_task("b", vec!["a"]),
            make_task("c", vec!["a"]),
            make_task("d", vec!["b", "c"]),
        ];
        let layers = topological_layers(&tasks).unwrap();
        assert_eq!(layers, vec![vec!["a"], vec!["b", "c"], vec!["d"]]);
    }

    /// 空输入 → 空层表(0 层),不 panic
    #[test]
    fn test_topological_layers_empty() {
        assert_eq!(topological_layers(&[]).unwrap(), Vec::<Vec<String>>::new());
    }

    /// 有环 → `CyclicDependency`,与 topological_order 同源
    #[test]
    fn test_topological_layers_cyclic() {
        let tasks = vec![make_task("a", vec!["b"]), make_task("b", vec!["a"])];
        assert!(matches!(
            topological_layers(&tasks),
            Err(QuestError::CyclicDependency)
        ));
    }

    /// 悬空依赖 → `DecompositionFailed`,与 topological_order 同源
    #[test]
    fn test_topological_layers_dangling() {
        let tasks = vec![make_task("a", vec!["nonexistent"])];
        assert!(matches!(
            topological_layers(&tasks),
            Err(QuestError::DecompositionFailed(_))
        ));
    }

    /// 层内字典序:输入乱序,同层输出必须按 task_id 排序(确定性契约)
    #[test]
    fn test_topological_layers_intra_layer_lexicographic() {
        let tasks = vec![
            make_task("c", vec![]),
            make_task("b", vec![]),
            make_task("a", vec![]),
            make_task("z", vec!["a", "b", "c"]),
        ];
        let layers = topological_layers(&tasks).unwrap();
        assert_eq!(layers, vec![vec!["a", "b", "c"], vec!["z"]]);
    }

    proptest! {
        /// 分层三重不变量:排列守恒 + 任一任务的依赖都在更早层 + 层内字典序
        #[test]
        fn prop_layers_cover_all_and_dep_earlier(tasks in arb_acyclic_tasks()) {
            let layers = topological_layers(&tasks).unwrap();

            // 1. 排列守恒:全部层拼接 = 全部任务 id(不增不减)
            let mut flat: Vec<String> = layers.iter().flatten().cloned().collect();
            let mut ids: Vec<String> = tasks.iter().map(|t| t.task_id.clone()).collect();
            flat.sort();
            ids.sort();
            prop_assert_eq!(flat, ids, "分层不得增删任务");

            // 2. 依赖必在更早层(层间串行的正确性前提)
            let layer_index: std::collections::HashMap<&str, usize> = layers
                .iter()
                .enumerate()
                .flat_map(|(li, layer)| layer.iter().map(move |id| (id.as_str(), li)))
                .collect();
            for t in &tasks {
                let li = layer_index[t.task_id.as_str()];
                for d in &t.dependencies {
                    prop_assert!(
                        layer_index[d.as_str()] < li,
                        "依赖 {} 必须在 {} 之前的层",
                        d,
                        t.task_id
                    );
                }
            }

            // 3. 层内字典序(确定性)
            for layer in &layers {
                let mut sorted = layer.clone();
                sorted.sort();
                prop_assert_eq!(layer, &sorted);
            }
        }
    }
}
