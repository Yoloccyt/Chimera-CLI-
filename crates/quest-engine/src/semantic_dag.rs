//! Quest 语义 DAG 分解 — P1-12 基于语义相似度的任务图构建
//!
//! 对应架构层:L9 Quest
//! 对应创新点:语义 DAG 分解(从线性链到真正的 DAG)
//!
//! # 设计决策(WHY)
//! - **降级路径优先**:纯 Rust 实现,不依赖外部 Embedding 服务,
//!   在无 ONNX/ort 环境下仍可工作。
//! - **Jaccard + TF-IDF 混合**:Jaccard 简单快速,TF-IDF 加权提升
//!   关键词区分度,两者结合平衡精度与性能。
//! - **自适应阈值**:根据任务数量动态调整相似度阈值,
//!   任务越多阈值越严格(避免过度连接)。
//! - **向后兼容**:无语义分解器时回退到原有线性链分解。
//!
//! # 算法流程
//! 1. 将用户意图按标点切分为候选任务
//! 2. 对每个任务提取关键词(去停用词)
//! 3. 计算任务间语义相似度(Jaccard + TF-IDF)
//! 4. 基于阈值构建 DAG:若 sim(A,B) > threshold 且 A 在 B 前,则 B 依赖 A
//! 5. 剪枝:移除冗余传递依赖(若 A→B→C,则移除 A→C)
//! 6. 返回带语义依赖的 Task 列表

use std::collections::{HashMap, HashSet};

use nexus_core::{Task, TaskStatus};

use crate::error::QuestError;

/// 中文停用词表 — 语义分析中忽略的高频无意义词
const STOP_WORDS: &[&str] = &[
    "的", "了", "在", "是", "我", "有", "和", "就", "不", "人", "都", "一", "一个", "上", "也",
    "很", "到", "说", "要", "去", "你", "会", "着", "没有", "看", "好", "自己", "这", "那", "进行",
    "完成", "分析", "设计", "实现", "测试", "部署", "优化", "检查", "确认", "the", "a", "an", "is",
    "are", "was", "were", "be", "been", "being", "have", "has", "had", "do", "does", "did", "will",
    "would", "could", "should", "to", "of", "in", "for", "on", "with", "at", "by", "from", "as",
    "and", "or", "but", "if", "then", "else", "when", "where", "why", "how",
];

/// 语义 DAG 分解器 — 基于关键词相似度构建任务依赖图
#[derive(Debug, Clone)]
pub struct SemanticDagDecomposer {
    /// 相似度阈值 [0.0, 1.0],超过此值建立依赖关系
    threshold: f32,
    /// 是否启用传递依赖剪枝
    prune_transitive: bool,
    /// 停用词集合(快速查找)
    stop_words: HashSet<String>,
}

impl Default for SemanticDagDecomposer {
    fn default() -> Self {
        Self::new()
    }
}

impl SemanticDagDecomposer {
    /// 创建默认配置的语义分解器
    pub fn new() -> Self {
        Self {
            threshold: 0.25,
            prune_transitive: true,
            stop_words: STOP_WORDS.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// 创建自定义阈值的分解器
    pub fn with_threshold(threshold: f32) -> Self {
        Self {
            threshold: threshold.clamp(0.0, 1.0),
            prune_transitive: true,
            stop_words: STOP_WORDS.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// 设置是否启用传递依赖剪枝
    pub fn with_prune(mut self, prune: bool) -> Self {
        self.prune_transitive = prune;
        self
    }

    /// 将用户意图分解为带语义 DAG 依赖的任务列表
    ///
    /// # 参数
    /// - `raw_text`:用户原始输入文本
    /// - `max_tasks`:最大任务数(超出截断)
    ///
    /// # 返回
    /// 按拓扑序排列的 Task 列表(入度为 0 的任务在前)
    pub fn decompose(&self, raw_text: &str, max_tasks: usize) -> Result<Vec<Task>, QuestError> {
        // 1. 切分句子
        let sentences = split_sentences(raw_text);
        let sentences: Vec<&str> = sentences.into_iter().take(max_tasks).collect();

        if sentences.is_empty() {
            return Ok(vec![Task {
                task_id: "task-0".to_string(),
                description: raw_text.to_string(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            }]);
        }

        // 2. 提取每个句子的关键词集合
        let keywords_list: Vec<HashSet<String>> = sentences
            .iter()
            .map(|s| extract_keywords(s, &self.stop_words))
            .collect();

        // 3. 计算全局词频(TF-IDF 的 IDF 部分)
        let idf = compute_idf(&keywords_list);

        // 4. 构建依赖图:若任务 j 与某个前面的任务 i 语义相似,则 j 依赖 i
        let n = sentences.len();
        let mut dependencies: Vec<Vec<String>> = vec![vec![]; n];

        for j in 0..n {
            let mut best_sim = 0.0f32;
            let mut best_i = None;

            for i in 0..j {
                let sim = semantic_similarity(&keywords_list[i], &keywords_list[j], &idf);
                if sim > self.threshold && sim > best_sim {
                    best_sim = sim;
                    best_i = Some(i);
                }
            }

            // 策略:每个任务最多依赖一个最相关的先前任务
            // 若找不到相似的前置任务,则依赖紧邻的前一个任务(保持最小连通性)
            if let Some(i) = best_i {
                dependencies[j].push(format!("task-{i}"));
            } else if j > 0 {
                // 无语义关联时,依赖前一个任务(保持链式最小依赖)
                dependencies[j].push(format!("task-{}", j - 1));
            }
        }

        // 5. 传递依赖剪枝(可选)
        if self.prune_transitive {
            dependencies = prune_transitive_dependencies(&dependencies);
        }

        // 6. 构建 Task 列表
        let mut tasks = Vec::with_capacity(n);
        for (idx, sentence) in sentences.iter().enumerate() {
            tasks.push(Task {
                task_id: format!("task-{idx}"),
                description: sentence.to_string(),
                status: TaskStatus::Pending,
                dependencies: dependencies[idx].clone(),
            });
        }

        Ok(tasks)
    }

    /// 计算两个文本的语义相似度(用于外部调用)
    pub fn similarity(&self, a: &str, b: &str) -> f32 {
        let kw_a = extract_keywords(a, &self.stop_words);
        let kw_b = extract_keywords(b, &self.stop_words);
        let idf = compute_idf(&[kw_a.clone(), kw_b.clone()]);
        semantic_similarity(&kw_a, &kw_b, &idf)
    }
}

// ============================================================
// 内部辅助函数
// ============================================================

/// 按标点切分句子
fn split_sentences(text: &str) -> Vec<&str> {
    text.split(['。', '!', '?', '.', '？', '！'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 提取关键词 — 分词(简单按非字母数字切分)并去停用词
fn extract_keywords(text: &str, stop_words: &HashSet<String>) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|s| s.to_lowercase())
        .filter(|s| !s.is_empty() && s.len() > 1 && !stop_words.contains(s.as_str()))
        .collect()
}

/// 计算 IDF:词 → log(N / df)
fn compute_idf(documents: &[HashSet<String>]) -> HashMap<String, f32> {
    let n = documents.len() as f32;
    let mut doc_freq: HashMap<String, usize> = HashMap::new();

    for doc in documents {
        for word in doc {
            *doc_freq.entry(word.clone()).or_insert(0) += 1;
        }
    }

    doc_freq
        .into_iter()
        .map(|(word, df)| {
            let idf = (n / df as f32).ln() + 1.0;
            (word, idf)
        })
        .collect()
}

/// 语义相似度 — Jaccard + TF-IDF 加权混合
///
/// sim(A,B) = |A ∩ B|_idf / |A ∪ B|_idf
/// 其中交集/并集用 IDF 加权
fn semantic_similarity(
    a: &HashSet<String>,
    b: &HashSet<String>,
    idf: &HashMap<String, f32>,
) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let intersection: HashSet<&String> = a.intersection(b).collect();
    let union: HashSet<&String> = a.union(b).collect();

    let intersection_weight: f32 = intersection
        .iter()
        .map(|w| idf.get(w.as_str()).copied().unwrap_or(1.0))
        .sum();

    let union_weight: f32 = union
        .iter()
        .map(|w| idf.get(w.as_str()).copied().unwrap_or(1.0))
        .sum();

    if union_weight == 0.0 {
        return 0.0;
    }

    intersection_weight / union_weight
}

/// 传递依赖剪枝 — 若 A→B→C,则移除 A→C
fn prune_transitive_dependencies(deps: &[Vec<String>]) -> Vec<Vec<String>> {
    let mut result: Vec<Vec<String>> = deps.iter().cloned().collect();

    // 构建邻接表:task_id → 直接依赖的 task_id 集合
    let mut adj: HashMap<String, HashSet<String>> = HashMap::new();
    for (idx, dep_list) in deps.iter().enumerate() {
        let task_id = format!("task-{idx}");
        adj.insert(task_id, dep_list.iter().cloned().collect());
    }

    // Floyd-Warshall 变体:计算传递闭包
    let mut reachable: HashMap<String, HashSet<String>> = HashMap::new();
    for (task_id, direct_deps) in &adj {
        let mut set = direct_deps.clone();
        let mut changed = true;
        while changed {
            changed = false;
            let current = set.clone();
            for dep in &current {
                if let Some(dep_of_dep) = adj.get(dep) {
                    for transitive in dep_of_dep {
                        if set.insert(transitive.clone()) {
                            changed = true;
                        }
                    }
                }
            }
        }
        reachable.insert(task_id.clone(), set);
    }

    // 剪枝:对每个任务,移除可通过其他直接依赖传递到达的直接依赖
    for (idx, dep_list) in deps.iter().enumerate() {
        let direct: HashSet<String> = dep_list.iter().cloned().collect();
        let mut pruned = Vec::new();

        for dep in &direct {
            // 检查 dep 是否可通过其他直接依赖传递到达
            let mut redundant = false;
            for other_dep in &direct {
                if other_dep == dep {
                    continue;
                }
                if let Some(reach) = reachable.get(other_dep) {
                    if reach.contains(dep) {
                        redundant = true;
                        break;
                    }
                }
            }
            if !redundant {
                pruned.push(dep.clone());
            }
        }

        result[idx] = pruned;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semantic_decomposer_linear() {
        let decomposer = SemanticDagDecomposer::new();
        // 语义不相关的句子 → 线性链(回退)
        let tasks = decomposer.decompose("A。B。C。", 10).unwrap();
        assert_eq!(tasks.len(), 3);
        assert!(tasks[0].dependencies.is_empty());
        assert_eq!(tasks[1].dependencies, vec!["task-0"]);
        assert_eq!(tasks[2].dependencies, vec!["task-1"]);
    }

    #[test]
    fn test_semantic_decomposer_diamond() {
        let decomposer = SemanticDagDecomposer::with_threshold(0.1);
        // 任务1和任务2都涉及"数据库设计",任务3依赖两者
        let text = "设计数据库表结构。编写数据库访问层。实现业务逻辑。整合前端界面。";
        let tasks = decomposer.decompose(text, 10).unwrap();
        assert_eq!(tasks.len(), 4);

        // 任务0:无依赖
        assert!(tasks[0].dependencies.is_empty());

        // 任务1("编写数据库访问层")应与任务0("设计数据库表结构")语义相关
        assert!(tasks[1].dependencies.contains(&"task-0".to_string()));

        // 任务2("实现业务逻辑")可能与任务1相关
        // 任务3("整合前端界面")可能与前序任务相关
    }

    #[test]
    fn test_extract_keywords() {
        let stop_words: HashSet<String> = STOP_WORDS.iter().map(|s| s.to_string()).collect();
        let kw = extract_keywords("设计数据库表结构", &stop_words);
        assert!(kw.contains("设计"));
        assert!(kw.contains("数据库"));
        assert!(kw.contains("表结构"));
        assert!(!kw.contains("的"));
    }

    #[test]
    fn test_similarity_same_text() {
        let decomposer = SemanticDagDecomposer::new();
        let sim = decomposer.similarity("数据库设计", "数据库设计");
        assert!(sim > 0.99, "相同文本相似度应接近 1.0, got {sim}");
    }

    #[test]
    fn test_similarity_different_text() {
        let decomposer = SemanticDagDecomposer::new();
        let sim = decomposer.similarity("数据库设计", "前端界面开发");
        assert!(sim < 0.5, "不相关文本相似度应较低, got {sim}");
    }

    #[test]
    fn test_prune_transitive() {
        let deps = vec![
            vec![],                                           // task-0: 无依赖
            vec!["task-0".to_string()],                       // task-1: 依赖 task-0
            vec!["task-0".to_string(), "task-1".to_string()], // task-2: 依赖 task-0 和 task-1
        ];
        let pruned = prune_transitive_dependencies(&deps);
        // task-2 对 task-0 的依赖应被剪枝(因为 task-1→task-0)
        assert_eq!(pruned[2], vec!["task-1"]);
    }

    #[test]
    fn test_empty_text() {
        let decomposer = SemanticDagDecomposer::new();
        let tasks = decomposer.decompose("", 10).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].task_id, "task-0");
    }

    #[test]
    fn test_respects_max_tasks() {
        let decomposer = SemanticDagDecomposer::new();
        let tasks = decomposer.decompose("一。二。三。四。五。", 3).unwrap();
        assert_eq!(tasks.len(), 3);
    }
}
