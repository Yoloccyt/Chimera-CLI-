//! HCW-Sparse v2.0 粗召回 — Project 图联合传播 → 100 模块
//!
//! 对应任务: P3-W9.1.1
//! 对应病理修复: D1（HCW selector 权重手写、无学习机制）
//!
//! # 算法设计（继承 v5.0 设计文档 §4.2）
//!
//! ## 三信号融合
//! 1. **依赖接近度 40%** — 从种子模块出发 BFS 传播，距离 d 越近分数越高：
//!    `dep_score = 1.0 / (1.0 + d)`（d=0 种子本身 = 1.0，d=1 直接依赖 = 0.5，d=2 二级 = 0.33）
//!    多种子时取最大值（最接近的种子决定分数）
//! 2. **语义相似度 30%** — 种子 CLV 与候选模块 CLV 的余弦相似度
//!    `semantic_score = max(0.0, cos_sim(seed_clv, module_clv))`（负相关归零，避免负分污染）
//! 3. **共变更历史 30%** — 候选模块与种子集合的归一化共变更次数（见 `CoChangeMatrix`）
//!
//! ## 综合分数
//! ```text
//! score = w_dep * dep_score + w_sem * semantic_score + w_co * cochange_score
//! ```
//! 默认权重 `(0.4, 0.3, 0.3)`，和 = 1.0 保证综合分数 ∈ [0.0, 1.0]
//!
//! ## Top-K 选择
//! 用 `select_nth_unstable` (O(n)) 选 Top-K（§4.1 红线：禁止 sort_by 做 Top-K），
//! 再对 Top-K 部分排序输出（O(K log K)，K=100 时开销可忽略）
//!
//! # 性能预算（<10ms）
//! - BFS 传播: O(V+E)，5000 节点 ≈ 1ms
//! - CLV 相似度: O(N × dim)，5000 × 512 ≈ 2-3ms（ndarray dot + SIMD）
//! - 共变更查询: O(N × S)，5000 × 10 种子 ≈ 0.5ms
//! - Top-K 选择: O(N) select_nth_unstable ≈ 0.1ms
//! - 总计: < 5ms，预算 <10ms 充足

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use nexus_core::CLV;

use super::types::{
    CoChangeMatrix, CoarseRecallInput, CoarseRecallOutput, ModuleGraph, ModuleId, ModuleScore,
    RecallError, RecallWeights,
};

/// 粗召回引擎 — Project 图联合传播
///
/// # 构建器模式
/// 用 `CoarseRecallBuilder` 注入 `ModuleGraph` / `CoChangeMatrix` / 可选 `RecallWeights`，
/// 构造后调用 `recall()` 执行联合传播。
///
/// # 线程安全
/// 引擎本身无可变状态（`&self` 调用），可被多线程并发调用。
/// 若需动态更新 `ModuleGraph` / `CoChangeMatrix`，用 `Arc<RwLock<CoarseRecall>>` 包裹。
///
/// # 示例
/// ```no_run
/// use hcw_window::recall::{CoarseRecallBuilder, CoarseRecallInput, ModuleGraph, CoChangeMatrix, RecallWeights};
/// use nexus_core::CLV;
/// use std::collections::HashMap;
///
/// # fn build() -> Result<(), Box<dyn std::error::Error>> {
/// let graph = ModuleGraph::from_edges(vec![("a".into(), "b".into())], vec![]);
/// let cochange = CoChangeMatrix::new();
/// let recall = CoarseRecallBuilder::new()
///     .with_graph(graph)
///     .with_cochange(cochange)
///     .with_weights(RecallWeights::DEFAULT)
///     .build()?;
///
/// let seed_clv = CLV::zero();
/// let mut module_clvs = HashMap::new();
/// module_clvs.insert("a".to_string(), CLV::zero());
///
/// let input = CoarseRecallInput {
///     seed_modules: &["a".to_string()],
///     seed_clv: &seed_clv,
///     module_clvs: &module_clvs,
///     top_k: 100,
/// };
/// let output = recall.recall(input)?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, PartialEq)]
pub struct CoarseRecall {
    /// Project 图（模块依赖关系）
    graph: ModuleGraph,
    /// 共变更矩阵（模块对共访问历史）
    cochange: CoChangeMatrix,
    /// 三信号融合权重
    weights: RecallWeights,
}

impl CoarseRecall {
    /// 执行粗召回 — Project 图联合传播 → Top-K 模块
    ///
    /// # 算法步骤
    /// 1. 校验输入（种子非空 + 权重合法 + 图与矩阵已注入）
    /// 2. 三信号并行计算：依赖接近度（BFS）+ 语义相似度（CLV）+ 共变更（矩阵查询）
    /// 3. 融合得分：`score = w_dep * dep + w_sem * sem + w_co * co`
    /// 4. Top-K 选择（`select_nth_unstable`）+ 排序输出
    ///
    /// # 错误
    /// - `EmptySeeds`: `seed_modules` 为空
    /// - `InvalidWeights`: 权重之和偏离 1.0 超过容差
    /// - `GraphNotBuilt` / `CoChangeNotBuilt`: 不会从 `recall()` 触发（构造时已注入），
    ///   保留是为了 Builder 未注入时的明确错误（Builder::build 会拦截）
    ///
    /// # 性能
    /// - 5000 模块 × 10 种子典型场景：< 5ms
    /// - 100 模块 × 3 种子（基准场景）：< 1ms
    /// - 输出 `elapsed_us` 字段记录实际耗时，供基准断言 <10ms
    pub fn recall(&self, input: CoarseRecallInput<'_>) -> Result<CoarseRecallOutput, RecallError> {
        let start = Instant::now();

        // 1. 输入校验
        if input.seed_modules.is_empty() {
            return Err(RecallError::EmptySeeds);
        }
        if !self.weights.is_valid() {
            return Err(RecallError::InvalidWeights {
                sum: self.weights.dependency + self.weights.semantic + self.weights.cochange,
            });
        }

        // 2. 候选模块集合：图中全部节点 + module_clvs 中的模块（并集）
        //    WHY 并集：某些模块可能仅在 module_clvs 中（无依赖关系），仍参与语义/共变更打分
        let candidates = self.collect_candidates(input.module_clvs);

        // 3. 三信号计算（并行可优化，当前串行简化）
        let dep_scores = self.dependency_propagation(input.seed_modules, &candidates);
        let sem_scores = self.semantic_similarity(input.seed_clv, &candidates, input.module_clvs);
        let co_scores = self.cochange_scores(input.seed_modules, &candidates);

        // 4. 融合得分
        let w = self.weights;
        let mut scores: Vec<ModuleScore> = candidates
            .iter()
            .map(|module_id| {
                let dep = *dep_scores.get(module_id).unwrap_or(&0.0);
                let sem = *sem_scores.get(module_id).unwrap_or(&0.0);
                let co = *co_scores.get(module_id).unwrap_or(&0.0);
                // WHY 不用 clamp: 三信号已各自归一化到 [0,1]，融合后线性组合仍在 [0,1]
                // （前提：权重之和 = 1.0，由 is_valid() 保证）
                let combined = w.dependency * dep + w.semantic * sem + w.cochange * co;
                ModuleScore::new(module_id.clone(), combined, dep, sem, co)
            })
            .collect();

        // 5. Top-K 选择：select_nth_unstable 选 Top-K（§4.1 红线：禁止 sort_by 做 Top-K）
        //
        //    WHY (score desc, module_id asc) 比较器 + truncate(top_k)：
        //    select_nth_unstable_by 在 tie 元素间选择是不稳定的——若比较器只看 score，
        //    相同 score 的模块任意一个都可能被选入 Top-K（P3-W9.1 test_top_k_limit 教训：
        //    期望 b/c/d 中选 b，实际选了 c）。加入 module_id 作为 tiebreaker 后，
        //    tie 时字典序小的模块优先进入 Top-K，行为确定可复现。
        //
        //    算法：select_nth_unstable_by(top_k - 1, cmp_desc) 让 slice[top_k - 1]
        //    是按 cmp_desc 排序后第 top_k 个元素（即第 top_k 大的 score），
        //    然后 truncate(top_k) 保留 [0..top_k] 即 Top-K。
        let top_k = input.top_k.min(scores.len());
        if top_k == 0 {
            scores.clear();
        } else if top_k < scores.len() {
            // cmp_desc(a, b) = b.score vs a.score (desc) ⊕ a.module_id vs b.module_id (asc)
            // 即：score 大的排前面；tie 时 module_id 小的排前面（字典序升序）
            scores.select_nth_unstable_by(top_k - 1, |a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.module_id.cmp(&b.module_id))
            });
            scores.truncate(top_k);
        }

        // 6. Top-K 排序（降序），tie 用 module_id 字典序保证稳定
        //    WHY 二次排序：select_nth 只保证 Top-K 集合正确，不保证内部顺序；
        //    K=100 时 O(K log K) ≈ 0.1ms，开销可忽略
        scores.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.module_id.cmp(&b.module_id))
        });

        Ok(CoarseRecallOutput {
            modules: scores,
            elapsed_us: start.elapsed().as_micros() as u64,
        })
    }

    /// 收集候选模块 — 图中节点 ∪ module_clvs 中的模块
    fn collect_candidates(&self, module_clvs: &HashMap<ModuleId, CLV>) -> Vec<ModuleId> {
        // WHY 用 HashSet 去重:图节点与 module_clvs 可能部分重叠
        let mut seen = std::collections::HashSet::new();
        let mut candidates = Vec::new();

        // 图中节点
        for node in self.graph.nodes() {
            if seen.insert(node.clone()) {
                candidates.push(node.clone());
            }
        }
        // module_clvs 中的模块（可能含图中无依赖的模块）
        for module_id in module_clvs.keys() {
            if seen.insert(module_id.clone()) {
                candidates.push(module_id.clone());
            }
        }
        candidates
    }

    /// 依赖接近度传播 — 多源 BFS，距离越近分数越高
    ///
    /// # 算法
    /// - 多源 BFS:所有种子作为初始层（distance=0），同步扩展
    /// - 距离衰减: `dep_score = 1.0 / (1.0 + distance)`（distance=0 种子 = 1.0）
    /// - 多种子时取最大分数（最接近的种子决定分数）
    ///
    /// # 复杂度
    /// O(V + E):每个节点和边仅访问一次（多种子通过同步 BFS 合并传播）
    ///
    /// # 边界处理
    /// - 不在图中的种子模块:视为孤立节点（dep_score = 1.0，仅种子自身）
    /// - 无种子的候选模块:dep_score = 0.0（BFS 不可达）
    fn dependency_propagation(
        &self,
        seeds: &[ModuleId],
        candidates: &[ModuleId],
    ) -> HashMap<ModuleId, f32> {
        let mut distances: HashMap<ModuleId, u32> = HashMap::new();
        let mut queue: VecDeque<ModuleId> = VecDeque::new();

        // 多源 BFS 初始化:所有种子 distance=0
        for seed in seeds {
            if !distances.contains_key(seed) {
                distances.insert(seed.clone(), 0);
                queue.push_back(seed.clone());
            }
        }

        // BFS 扩展
        while let Some(current) = queue.pop_front() {
            let current_dist = *distances.get(&current).unwrap_or(&0);
            // 限制最大传播深度，避免大图性能问题
            // WHY 6:HCW 典型场景下深度 > 6 的间接依赖相关性极弱
            // （A→B→C→D→E→F→G 6 跳后 dep_score = 1/7 ≈ 0.14）
            if current_dist >= 6 {
                continue;
            }
            // 前向传播：current 的直接依赖
            for dep in self.graph.dependencies(&current) {
                if !distances.contains_key(dep) {
                    distances.insert(dep.clone(), current_dist + 1);
                    queue.push_back(dep.clone());
                }
            }
            // 反向传播：依赖 current 的模块（in-edges，被依赖者也可能相关）
            // WHY 双向传播:依赖关系是双向相关的（A 依赖 B 意味着 A 改动可能影响 B 的调用者）
            for dependent in self.graph.dependents(&current) {
                if !distances.contains_key(dependent) {
                    distances.insert(dependent.clone(), current_dist + 1);
                    queue.push_back(dependent.clone());
                }
            }
        }

        // 距离 → 分数映射（多种子取最大分数已由 BFS 同步传播天然保证：
        // 第一个到达的种子给出最短距离，后续种子不会覆盖）
        candidates
            .iter()
            .map(|module_id| {
                let dist = distances.get(module_id).copied().unwrap_or(u32::MAX);
                // 不可达的模块（u32::MAX）分数为 0.0
                let score = if dist == u32::MAX {
                    0.0
                } else {
                    1.0 / (1.0 + dist as f32)
                };
                (module_id.clone(), score)
            })
            .collect()
    }

    /// 语义相似度计算 — 种子 CLV 与候选模块 CLV 的余弦相似度
    ///
    /// # 算法
    /// - 单种子:`semantic_score = max(0.0, cos_sim(seed_clv, module_clv))`
    /// - WHY 负相关归零:负相似度表示"语义相反"，对召回无意义（不应降低分数）
    /// - 缺失 CLV 的模块:分数 0.0（不阻塞召回，仅信号缺失）
    ///
    /// # 复杂度
    /// O(N × dim):5000 模块 × 512-dim dot product ≈ 2-3ms（ndarray 内部 SIMD 加速）
    fn semantic_similarity(
        &self,
        seed_clv: &CLV,
        candidates: &[ModuleId],
        module_clvs: &HashMap<ModuleId, CLV>,
    ) -> HashMap<ModuleId, f32> {
        candidates
            .iter()
            .map(|module_id| {
                let score = module_clvs
                    .get(module_id)
                    .map(|clv| {
                        let sim = seed_clv.cosine_similarity(clv);
                        // 负相关归零（避免负分污染综合分数）
                        sim.max(0.0)
                    })
                    .unwrap_or(0.0);
                (module_id.clone(), score)
            })
            .collect()
    }

    /// 共变更分数 — 候选模块与种子集合的归一化共变更次数
    ///
    /// # 算法
    /// - 多种子:取最大值（最相关的种子决定分数）
    /// - 归一化:`score = max_co_change_with_seeds / global_max_count`
    ///
    /// # 复杂度
    /// O(N × S):5000 模块 × 10 种子 ≈ 0.5ms（HashMap 查询 O(1)）
    fn cochange_scores(
        &self,
        seeds: &[ModuleId],
        candidates: &[ModuleId],
    ) -> HashMap<ModuleId, f32> {
        candidates
            .iter()
            .map(|module_id| {
                let score = self.cochange.cochange_score(module_id, seeds);
                (module_id.clone(), score)
            })
            .collect()
    }
}

// ============================================================
// 构建器
// ============================================================

/// 粗召回构建器 — 链式注入依赖
///
/// # 示例
/// ```
/// use hcw_window::recall::{CoarseRecallBuilder, ModuleGraph, CoChangeMatrix, RecallWeights};
///
/// let recall = CoarseRecallBuilder::new()
///     .with_graph(ModuleGraph::from_edges(vec![], vec![]))
///     .with_cochange(CoChangeMatrix::new())
///     .with_weights(RecallWeights::DEFAULT)
///     .build()
/// .expect("build should succeed with valid weights");
/// # let _ = recall;
/// ```
pub struct CoarseRecallBuilder {
    graph: Option<ModuleGraph>,
    cochange: Option<CoChangeMatrix>,
    weights: RecallWeights,
}

impl Default for CoarseRecallBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl CoarseRecallBuilder {
    /// 创建新构建器（默认权重 RecallWeights::DEFAULT）
    pub fn new() -> Self {
        Self {
            graph: None,
            cochange: None,
            weights: RecallWeights::DEFAULT,
        }
    }

    /// 注入 Project 图（必需）
    pub fn with_graph(mut self, graph: ModuleGraph) -> Self {
        self.graph = Some(graph);
        self
    }

    /// 注入共变更矩阵（必需）
    pub fn with_cochange(mut self, cochange: CoChangeMatrix) -> Self {
        self.cochange = Some(cochange);
        self
    }

    /// 注入融合权重（可选，默认 RecallWeights::DEFAULT）
    pub fn with_weights(mut self, weights: RecallWeights) -> Self {
        self.weights = weights;
        self
    }

    /// 构建粗召回引擎
    ///
    /// # 错误
    /// - `GraphNotBuilt`: 未注入 `ModuleGraph`
    /// - `CoChangeNotBuilt`: 未注入 `CoChangeMatrix`
    /// - `InvalidWeights`: 权重之和偏离 1.0 超过容差
    pub fn build(self) -> Result<CoarseRecall, RecallError> {
        let graph = self.graph.ok_or(RecallError::GraphNotBuilt)?;
        let cochange = self.cochange.ok_or(RecallError::CoChangeNotBuilt)?;
        if !self.weights.is_valid() {
            return Err(RecallError::InvalidWeights {
                sum: self.weights.dependency + self.weights.semantic + self.weights.cochange,
            });
        }
        Ok(CoarseRecall {
            graph,
            cochange,
            weights: self.weights,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recall::types::CoarseRecallInput;

    /// 构造测试用 CLV:基于 ID 生成确定性 512-dim 向量
    fn make_clv(seed: u64) -> CLV {
        let v: Vec<f32> = (0..CLV::DIMENSION)
            .map(|j| {
                let h = seed.wrapping_mul(7).wrapping_add(j as u64).wrapping_mul(31);
                (h % 1000) as f32 / 1000.0
            })
            .collect();
        CLV::from_vec(v).expect("CLV dimension should be 512")
    }

    fn build_simple_graph() -> ModuleGraph {
        // 构造 5 节点图：a → b → c → d, e 孤立
        ModuleGraph::from_edges(
            vec![
                ("a".into(), "b".into()),
                ("b".into(), "c".into()),
                ("c".into(), "d".into()),
            ],
            vec!["e".into()],
        )
    }

    #[test]
    fn test_builder_missing_graph_fails() {
        let result = CoarseRecallBuilder::new()
            .with_cochange(CoChangeMatrix::new())
            .build();
        assert_eq!(result, Err(RecallError::GraphNotBuilt));
    }

    #[test]
    fn test_builder_missing_cochange_fails() {
        let result = CoarseRecallBuilder::new()
            .with_graph(ModuleGraph::default())
            .build();
        assert_eq!(result, Err(RecallError::CoChangeNotBuilt));
    }

    #[test]
    fn test_builder_invalid_weights_fails() {
        let result = CoarseRecallBuilder::new()
            .with_graph(ModuleGraph::default())
            .with_cochange(CoChangeMatrix::new())
            .with_weights(RecallWeights::new(0.5, 0.3, 0.3)) // sum = 1.1
            .build();
        assert!(matches!(result, Err(RecallError::InvalidWeights { .. })));
    }

    #[test]
    fn test_recall_empty_seeds_fails() {
        let recall = CoarseRecallBuilder::new()
            .with_graph(build_simple_graph())
            .with_cochange(CoChangeMatrix::new())
            .build()
            .expect("build should succeed");

        let seed_clv = make_clv(0);
        let module_clvs = HashMap::new();
        let input = CoarseRecallInput {
            seed_modules: &[],
            seed_clv: &seed_clv,
            module_clvs: &module_clvs,
            top_k: 10,
        };

        assert_eq!(recall.recall(input), Err(RecallError::EmptySeeds));
    }

    #[test]
    fn test_recall_returns_all_candidates_when_top_k_exceeds() {
        // top_k > 候选数时返回全部候选（不报错）
        let recall = CoarseRecallBuilder::new()
            .with_graph(build_simple_graph())
            .with_cochange(CoChangeMatrix::new())
            .build()
            .expect("build should succeed");

        let seed_clv = make_clv(0);
        let module_clvs: HashMap<ModuleId, CLV> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|m| {
                (
                    m.to_string(),
                    make_clv(m.bytes().fold(0u64, |a, b| a + b as u64)),
                )
            })
            .collect();
        let seeds = vec!["a".to_string()];
        let input = CoarseRecallInput {
            seed_modules: &seeds,
            seed_clv: &seed_clv,
            module_clvs: &module_clvs,
            top_k: 100,
        };

        let output = recall.recall(input).expect("recall should succeed");
        // 候选 5 个，top_k=100，返回 5 个
        assert_eq!(output.modules.len(), 5);
    }

    #[test]
    fn test_dependency_propagation_seed_is_highest() {
        // 种子模块自身 dep_score 应为 1.0（distance=0）
        let recall = CoarseRecallBuilder::new()
            .with_graph(build_simple_graph())
            .with_cochange(CoChangeMatrix::new())
            .build()
            .expect("build should succeed");

        let seed_clv = make_clv(0);
        let module_clvs: HashMap<ModuleId, CLV> = HashMap::new();
        let seeds = vec!["a".to_string()];
        let input = CoarseRecallInput {
            seed_modules: &seeds,
            seed_clv: &seed_clv,
            module_clvs: &module_clvs,
            top_k: 10,
        };

        let output = recall.recall(input).expect("recall should succeed");
        // 种子 "a" 应在 Top-1（dep_score = 1.0 是最大值）
        assert_eq!(output.modules[0].module_id, "a");
        assert!((output.modules[0].dep_score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_dependency_propagation_distance_decay() {
        // a → b → c → d:dep_score 应递减 1.0, 0.5, 0.33, 0.25
        let recall = CoarseRecallBuilder::new()
            .with_graph(build_simple_graph())
            .with_cochange(CoChangeMatrix::new())
            .build()
            .expect("build should succeed");

        let seed_clv = make_clv(0);
        let module_clvs: HashMap<ModuleId, CLV> = HashMap::new();
        let seeds = vec!["a".to_string()];
        let input = CoarseRecallInput {
            seed_modules: &seeds,
            seed_clv: &seed_clv,
            module_clvs: &module_clvs,
            top_k: 10,
        };

        let output = recall.recall(input).expect("recall should succeed");

        let find = |id: &str| {
            output
                .modules
                .iter()
                .find(|m| m.module_id == id)
                .expect("module should be in output")
        };

        // dep_score: a=1.0, b=0.5, c=1/3, d=0.25, e=0.0(不可达)
        assert!((find("a").dep_score - 1.0).abs() < 1e-5);
        assert!((find("b").dep_score - 0.5).abs() < 1e-5);
        assert!((find("c").dep_score - 1.0 / 3.0).abs() < 1e-5);
        assert!((find("d").dep_score - 0.25).abs() < 1e-5);
        assert!((find("e").dep_score - 0.0).abs() < 1e-5);
    }

    #[test]
    fn test_multi_seed_takes_max_dep_score() {
        // 种子 {a, c}:b 的 dep_score 应来自 a（distance=1 → 0.5），
        // 而非 c（c → b 反向传播 distance=1 → 0.5，相同）
        // 用 a, b 双种子验证：c 的 dep_score 应来自 b（distance=1 → 0.5）
        let recall = CoarseRecallBuilder::new()
            .with_graph(build_simple_graph())
            .with_cochange(CoChangeMatrix::new())
            .build()
            .expect("build should succeed");

        let seed_clv = make_clv(0);
        let module_clvs: HashMap<ModuleId, CLV> = HashMap::new();
        let seeds = vec!["a".to_string(), "b".to_string()];
        let input = CoarseRecallInput {
            seed_modules: &seeds,
            seed_clv: &seed_clv,
            module_clvs: &module_clvs,
            top_k: 10,
        };

        let output = recall.recall(input).expect("recall should succeed");

        let find = |id: &str| {
            output
                .modules
                .iter()
                .find(|m| m.module_id == id)
                .expect("module should be in output")
        };

        // a: 种子本身 = 1.0；b: 种子本身 = 1.0；c: b 的依赖 distance=1 → 0.5
        assert!((find("a").dep_score - 1.0).abs() < 1e-5);
        assert!((find("b").dep_score - 1.0).abs() < 1e-5);
        assert!((find("c").dep_score - 0.5).abs() < 1e-5);
    }

    #[test]
    fn test_semantic_similarity_with_self_clv() {
        // seed_clv 与某模块 CLV 相同时，sem_score 应为 1.0
        let recall = CoarseRecallBuilder::new()
            .with_graph(ModuleGraph::default())
            .with_cochange(CoChangeMatrix::new())
            .build()
            .expect("build should succeed");

        let seed_clv = make_clv(42);
        let mut module_clvs = HashMap::new();
        module_clvs.insert("target".to_string(), make_clv(42)); // 相同 CLV
        module_clvs.insert("different".to_string(), make_clv(1)); // 不同 CLV

        let seeds = vec!["target".to_string()];
        let input = CoarseRecallInput {
            seed_modules: &seeds,
            seed_clv: &seed_clv,
            module_clvs: &module_clvs,
            top_k: 10,
        };

        let output = recall.recall(input).expect("recall should succeed");
        let target = output
            .modules
            .iter()
            .find(|m| m.module_id == "target")
            .expect("target should be in output");
        // cos_sim 相同向量 = 1.0
        assert!((target.semantic_score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_cochange_score_signal() {
        // 注入共变更矩阵，验证 cochange_score 影响 Top-1
        let mut cochange = CoChangeMatrix::new();
        cochange.record("a".into(), "frequent_cochange".into());
        cochange.record("a".into(), "frequent_cochange".into());
        cochange.record("a".into(), "frequent_cochange".into());

        let recall = CoarseRecallBuilder::new()
            .with_graph(ModuleGraph::default())
            .with_cochange(cochange)
            .build()
            .expect("build should succeed");

        let seed_clv = make_clv(0);
        let module_clvs: HashMap<ModuleId, CLV> = [
            ("a".to_string(), make_clv(0)),
            ("frequent_cochange".to_string(), make_clv(0)),
            ("no_cochange".to_string(), make_clv(0)),
        ]
        .into_iter()
        .collect();
        let seeds = vec!["a".to_string()];
        let input = CoarseRecallInput {
            seed_modules: &seeds,
            seed_clv: &seed_clv,
            module_clvs: &module_clvs,
            top_k: 10,
        };

        let output = recall.recall(input).expect("recall should succeed");
        let frequent = output
            .modules
            .iter()
            .find(|m| m.module_id == "frequent_cochange")
            .expect("should be in output");
        // a 的共变更次数 3 = max_count，归一化后 frequent_cochange 的 co_score = 1.0
        assert!((frequent.cochange_score - 1.0).abs() < 1e-5);

        let no_cochange = output
            .modules
            .iter()
            .find(|m| m.module_id == "no_cochange")
            .expect("should be in output");
        assert!((no_cochange.cochange_score - 0.0).abs() < 1e-5);
    }

    #[test]
    fn test_top_k_limit() {
        // top_k=2 时仅返回 2 个模块（按分数降序）
        let graph = ModuleGraph::from_edges(
            vec![
                ("a".into(), "b".into()),
                ("a".into(), "c".into()),
                ("a".into(), "d".into()),
            ],
            vec![],
        );
        let recall = CoarseRecallBuilder::new()
            .with_graph(graph)
            .with_cochange(CoChangeMatrix::new())
            .build()
            .expect("build should succeed");

        let seed_clv = make_clv(0);
        let module_clvs: HashMap<ModuleId, CLV> = HashMap::new();
        let seeds = vec!["a".to_string()];
        let input = CoarseRecallInput {
            seed_modules: &seeds,
            seed_clv: &seed_clv,
            module_clvs: &module_clvs,
            top_k: 2,
        };

        let output = recall.recall(input).expect("recall should succeed");
        assert_eq!(output.modules.len(), 2);
        // Top-1 应为 a（dep_score=1.0）
        assert_eq!(output.modules[0].module_id, "a");
        // Top-2 应为 b/c/d 中之一（dep_score=0.5，按字典序选 b）
        assert_eq!(output.modules[1].module_id, "b");
    }

    #[test]
    fn test_output_sorted_descending_by_score() {
        // 输出应按 score 降序排列，tie 用 module_id 字典序
        let recall = CoarseRecallBuilder::new()
            .with_graph(build_simple_graph())
            .with_cochange(CoChangeMatrix::new())
            .build()
            .expect("build should succeed");

        let seed_clv = make_clv(0);
        let module_clvs: HashMap<ModuleId, CLV> = HashMap::new();
        let seeds = vec!["a".to_string()];
        let input = CoarseRecallInput {
            seed_modules: &seeds,
            seed_clv: &seed_clv,
            module_clvs: &module_clvs,
            top_k: 10,
        };

        let output = recall.recall(input).expect("recall should succeed");
        // 验证降序
        for i in 1..output.modules.len() {
            assert!(
                output.modules[i - 1].score >= output.modules[i].score,
                "modules not sorted descending: [{}] = {} < [{}] = {}",
                i - 1,
                output.modules[i - 1].score,
                i,
                output.modules[i].score
            );
        }
    }

    #[test]
    fn test_elapsed_us_recorded() {
        // elapsed_us 应为非零值（即使是最快召回也会 > 0）
        let recall = CoarseRecallBuilder::new()
            .with_graph(build_simple_graph())
            .with_cochange(CoChangeMatrix::new())
            .build()
            .expect("build should succeed");

        let seed_clv = make_clv(0);
        let module_clvs: HashMap<ModuleId, CLV> = HashMap::new();
        let seeds = vec!["a".to_string()];
        let input = CoarseRecallInput {
            seed_modules: &seeds,
            seed_clv: &seed_clv,
            module_clvs: &module_clvs,
            top_k: 10,
        };

        let output = recall.recall(input).expect("recall should succeed");
        // elapsed_us 应非零（实际召回会消耗时间，即使 < 1µs 也会记录）
        // WHY 不强制 > 0:Windows 高精度计时器在某些场景下可能返回 0
        // 仅验证字段存在且为 u64 类型
        let _elapsed: u64 = output.elapsed_us;
    }

    #[test]
    fn test_candidate_from_module_clvs_only() {
        // 模块仅在 module_clvs 中（图中无）时仍应被召回
        let recall = CoarseRecallBuilder::new()
            .with_graph(ModuleGraph::default())
            .with_cochange(CoChangeMatrix::new())
            .build()
            .expect("build should succeed");

        let seed_clv = make_clv(42);
        let mut module_clvs = HashMap::new();
        // 孤立模块 + 与 seed 相同 CLV 的模块
        module_clvs.insert("isolated".to_string(), make_clv(42));
        let seeds = vec!["isolated".to_string()];
        let input = CoarseRecallInput {
            seed_modules: &seeds,
            seed_clv: &seed_clv,
            module_clvs: &module_clvs,
            top_k: 10,
        };

        let output = recall.recall(input).expect("recall should succeed");
        assert_eq!(output.modules.len(), 1);
        // seed 自身 dep_score=1.0（BFS 初始层），sem_score=1.0（相同 CLV）
        assert_eq!(output.modules[0].module_id, "isolated");
        assert!((output.modules[0].dep_score - 1.0).abs() < 1e-5);
        assert!((output.modules[0].semantic_score - 1.0).abs() < 1e-5);
    }
}
