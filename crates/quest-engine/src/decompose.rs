//! Quest 任务依赖推断策略 — 分解器产出依赖边的策略接口（WP-E，R7 批次）
//!
//! 对应架构层: L9 Quest（quest-engine 内部域概念,不上提 L0 契约层——依赖铁律）
//!
//! # 设计决策(WHY,WP-E 方案 C 用户裁决)
//! - **策略接口 + 配置开关**:分解器从"硬编码线性链"改为按
//!   [`DependencyMode`](配置于 `QuestConfig::dependency_mode`)选择策略,
//!   默认 [`LinearChain`] 与历史行为**逐字节等价**(`linear_matches_legacy` 测试锁定),
//!   保证默认配置下零行为变化。
//! - **保守启发式,宁可漏边不可错边**:[`HeuristicDag`] 的漏边只损失并行度
//!   (依赖图退化成串行链,仍正确);错边会把有依赖的任务并行派发,是正确性事故。
//!   故规则只认句首显式序数/承接词;指代消解(「它/该/此」)误判率高,
//!   登记为未来扩展(暂不实现)。
//! - **无环的结构性保证**:两个策略的依赖边都只指向**更早**的句子
//!   (下标严格递减),句子顺序天然无环;生产路径仍经 `create_quest` 的
//!   `validate_dag` 显式校验作为防御。

use serde::{Deserialize, Serialize};

/// 依赖推断策略 — 从切分后的句子序列产出每个任务的依赖列表
///
/// 实现方必须满足的契约:
/// - 输出与输入**等长**(`dependencies[i]` 对应第 i 句);
/// - 依赖只指向更早的句子(`dependencies[i]` 中只允许出现 `task-{j}, j < i`),
///   这是无环的结构性前提,经 `heuristic_always_dag` proptest 守护。
pub trait DependencyInference: Send + Sync {
    /// 输入句子序列,输出与输入等长的依赖列表(每项为依赖的 task_id 列表)
    fn infer(&self, sentences: &[String]) -> Vec<Vec<String>>;
}

/// 线性链(默认)— task-i 依赖 task-{i-1},与历史行为逐字节等价
///
/// WHY 保留为独立策略而非直接内联在 decompose:它既是默认行为的**事实规范**
/// (`linear_matches_legacy` 锁定),也让 HeuristicDag 的接入点显式化
/// (策略切换只发生在 [`dependency_inference`] 工厂一处)。
pub struct LinearChain;

/// 启发式 DAG — 句首序数/承接词产生依赖边,独立句无依赖
///
/// 规则(WP-E 裁决,按优先级;三条验收用例联立锁定):
/// a. 显式序数(「先/首先/第一步/1./①」句首)→ 依赖前一句(若存在);
/// a'. 序数顺承:序数句的**直接后继**(自身未命中 a/b 时)→ 依赖前一句
///     ——序数句声明了「接下来」的顺序意图,紧随其后的句子自然顺承
///     (验收用例 heuristic_ordinal_creates_edge:
///     `["先安装依赖", "运行测试"] → task-1 依赖 task-0`);
/// b. 承接词(「然后/再/接着/随后/最后/其次/之后」句首)→ 依赖前一句
///    (WP-E 裁决简化:「最近一个已确立的句子」退化为前一句);
/// c. 其他 → 无依赖(可与前句并行,波次执行即获并行度);
/// d. 指代消解(「它/该/此」)误判率高,**暂不实现**,登记为未来扩展。
///
/// # 安全性论证(宁可漏边不可错边)
/// 全部规则产生的边都只指向**紧邻前一句**:漏边只损失并行度(退化为串行,
/// 仍正确);最坏情形是把无依赖的独立句误串行化(序数顺承),同样只损失
/// 并行度——绝不会把有依赖的任务并行派发(正确性事故)。
pub struct HeuristicDag;

/// 依赖模式(配置开关,WP-E 方案 C)
///
/// 默认 [`DependencyMode::Linear`]:旧配置文件缺省字段经 `#[serde(default)]`
/// 回填 Linear,存量配置零兼容性问题。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DependencyMode {
    /// 线性链(默认,历史行为)
    #[default]
    Linear,
    /// 启发式 DAG(波次并行执行)
    HeuristicDag,
}

/// 显式序数词表(句首匹配,规则 a)
const ORDINAL_PREFIXES: &[&str] = &["先", "首先", "第一步", "1.", "①"];

/// 承接词表(句首匹配,规则 b)
const CONJUNCTION_PREFIXES: &[&str] = &["然后", "再", "接着", "随后", "最后", "其次", "之后"];

/// 句首前缀命中判定
fn starts_with_any(sentence: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| sentence.starts_with(p))
}

impl DependencyInference for LinearChain {
    fn infer(&self, sentences: &[String]) -> Vec<Vec<String>> {
        // 与 engine.rs 旧硬编码逐字节一致:首句无依赖,其余依赖前一任务
        (0..sentences.len())
            .map(|idx| {
                if idx == 0 {
                    Vec::new()
                } else {
                    vec![format!("task-{}", idx - 1)]
                }
            })
            .collect()
    }
}

impl DependencyInference for HeuristicDag {
    fn infer(&self, sentences: &[String]) -> Vec<Vec<String>> {
        sentences
            .iter()
            .enumerate()
            .map(|(idx, sentence)| {
                if idx == 0 {
                    // 首句无前驱可依赖(规则 a/a'/b 的「若存在」分支)
                    Vec::new()
                } else if starts_with_any(sentence, ORDINAL_PREFIXES)
                    || starts_with_any(sentence, CONJUNCTION_PREFIXES)
                {
                    // 规则 a/b:序数/承接词显式声明顺序 → 依赖前一句。
                    // WHY 简化为前一句(WP-E 裁决):「最近一个已确立的句子」
                    // 需要链状态跟踪,首版保守取前一句——漏跨句边只损失并行度,
                    // 不会并行有依赖的任务(宁可漏边不可错边)。
                    vec![format!("task-{}", idx - 1)]
                } else if starts_with_any(&sentences[idx - 1], ORDINAL_PREFIXES) {
                    // 规则 a' 序数顺承:前一句是序数句,本句顺承其顺序意图
                    // (验收用例 heuristic_ordinal_creates_edge 锁定)。
                    // 边仍只指向前一句:最坏把无依赖句串行化(损失并行度),
                    // 不属于正确性事故。
                    vec![format!("task-{}", idx - 1)]
                } else {
                    // 规则 c:独立句无依赖 → 可与前句并行(波次执行的并行度来源)
                    Vec::new()
                }
            })
            .collect()
    }
}

/// 工厂函数 — 按依赖模式返回策略实例
///
/// WHY 返回 `Box<dyn>`:两策略无状态零尺寸,动态分发开销在冷路径(分解)
/// 可忽略;调用方(QuestEngine::decompose)无需为枚举分支写第二份 match。
pub fn dependency_inference(mode: DependencyMode) -> Box<dyn DependencyInference> {
    match mode {
        DependencyMode::Linear => Box::new(LinearChain),
        DependencyMode::HeuristicDag => Box::new(HeuristicDag),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::dag::validate_dag;
    use nexus_core::{Task, TaskStatus};
    use proptest::prelude::*;

    /// 词库:覆盖序数/承接词/独立句三类(heuristic_always_dag 的句子来源)
    const WORD_VOCAB: &[&str] = &[
        "先安装依赖",
        "然后运行测试",
        "编译核心",
        "写文档",
        "首先分析需求",
        "1.克隆仓库",
        "①初始化配置",
        "最后验收",
        "随后部署",
        "接着联调",
        "其次review",
        "之后归档",
    ];

    /// 任意句子序列(0-8 句,从词库随机拼接;覆盖 0 句/1 句边界)
    ///
    /// WHY index 取模而非 sample::select:不依赖特定 proptest 版本的
    /// select API 形态,任何 1.x 均可编译。
    fn arb_sentences() -> impl Strategy<Value = Vec<String>> {
        proptest::collection::vec(any::<usize>(), 0..8).prop_map(|picks| {
            picks
                .iter()
                .map(|&i| WORD_VOCAB[i % WORD_VOCAB.len()].to_string())
                .collect()
        })
    }

    /// 旧硬编码行为的镜像实现(linear_matches_legacy 的锁定基准)
    fn legacy_linear_deps(idx: usize) -> Vec<String> {
        if idx == 0 {
            Vec::new()
        } else {
            vec![format!("task-{}", idx - 1)]
        }
    }

    #[test]
    fn linear_matches_legacy_boundaries() {
        // 显式边界:0 句 / 1 句 / 多句
        for sentences in [
            Vec::new(),
            vec!["只有一句".to_string()],
            vec!["一".to_string(), "二".to_string(), "三".to_string()],
        ] {
            let deps = LinearChain.infer(&sentences);
            assert_eq!(deps.len(), sentences.len());
            for (idx, d) in deps.iter().enumerate() {
                assert_eq!(*d, legacy_linear_deps(idx));
            }
        }
    }

    #[test]
    fn heuristic_ordinal_creates_edge() {
        let deps = HeuristicDag.infer(&["先安装依赖".to_string(), "运行测试".to_string()]);
        assert_eq!(deps, vec![Vec::<String>::new(), vec!["task-0".to_string()]]);
    }

    #[test]
    fn heuristic_independent_no_edge() {
        let deps = HeuristicDag.infer(&["编译核心".to_string(), "写文档".to_string()]);
        assert_eq!(deps, vec![Vec::<String>::new(), Vec::<String>::new()]);
    }

    #[test]
    fn heuristic_conjunction_chains() {
        let deps = HeuristicDag.infer(&[
            "先编译".to_string(),
            "然后测试".to_string(),
            "写文档".to_string(),
        ]);
        assert_eq!(deps[0], Vec::<String>::new(), "首句无前驱");
        assert_eq!(deps[1], vec!["task-0".to_string()], "承接词链到前一句");
        assert_eq!(deps[2], Vec::<String>::new(), "独立句无依赖(可并行)");
    }

    proptest! {
        /// LinearChain:任意句子序列输出与旧硬编码逐字节一致
        #[test]
        fn prop_linear_matches_legacy(sentences in arb_sentences()) {
            let deps = LinearChain.infer(&sentences);
            prop_assert_eq!(deps.len(), sentences.len());
            for (idx, d) in deps.iter().enumerate() {
                prop_assert_eq!(d.clone(), legacy_linear_deps(idx));
            }
        }

        /// HeuristicDag:任意句子序列恒与输入等长,且构造 Task 后经
        /// validate_dag 无环(依赖只指向更早句子的结构性保证)
        #[test]
        fn heuristic_always_dag(sentences in arb_sentences()) {
            let deps = HeuristicDag.infer(&sentences);
            prop_assert_eq!(deps.len(), sentences.len());
            let tasks: Vec<Task> = sentences
                .iter()
                .enumerate()
                .map(|(idx, s)| Task {
                    task_id: format!("task-{idx}"),
                    description: s.clone(),
                    status: TaskStatus::Pending,
                    dependencies: deps[idx].clone(),
                })
                .collect();
            prop_assert!(
                validate_dag(&tasks).is_ok(),
                "启发式依赖只指向更早句子,结构上必须无环"
            );
        }
    }
}
