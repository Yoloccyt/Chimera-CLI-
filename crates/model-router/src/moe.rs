//! MoE(Mixture of Experts)稀疏门控 — 大规模模型注册表的粗筛层
//!
//! 对应架构层:L1 Core,作为 `route_auto` 策略的前置优化层
//! 对应定律:Ω-Sparse(全维稀疏 — 仅激活 Top-K 专家,而非全量评估)
//!
//! # 设计动机
//! 50 以上模型规模下,`route_auto` 的全量评估(两遍遍历求 max_cost/max_latency、
//! 逐模型归一化评分、生成 n-1 个 candidates 的 String clone)成为
//! O(n) 高常数因子瓶颈。MoE 门控先用**轻量级评分函数**粗筛 Top-K(K ≤ 5)
//! 候选,再仅对 Top-K 做完整评估,将高常数因子操作从 O(n) 降至 O(k)。
//!
//! # 门控评分(轻量级)
//! - **无需全局 max 归一化**(避免两遍遍历),支持单遍 O(n) 评分
//! - 用倒数形式 `1/(1+x)`,值域 (0,1],方向与完整评分一致(越小越好 → 分越高)
//! - 纯算术,无字符串 `format!`,常数因子远低于完整评估
//! - 权重 0.4/0.4/0.2 与 `route_auto` 完整评分一致,保证粗筛排序近似
//!
//! # 阈值退化(向后兼容)
//! 模型数 < `threshold`(默认 50)时,门控返回全部模型引用,`route_auto`
//! 退化为全量评估,行为与未启用 MoE 时完全一致。默认 3 模型配置(<< 50)
//! 走退化路径,现有测试与行为不受影响。
//!
//! WHY 阈值选 50:默认配置 3 模型 + 安全余量。50 以下全量评估的绝对耗时
//! 在微秒级(见 `registry_bench`),优化收益不足以抵消门控评分开销;
//! 50 以上全量归一化与 candidates 生成的累积开销才开始显著。

#![forbid(unsafe_code)]

use std::cmp::Ordering;

use crate::types::{ModelInfo, RoutingRequest};

/// 默认稀疏化触发阈值 — 模型数达到此值时启用 Top-K 门控
///
/// WHY 50:任务规格定义的验收门槛(50+ 模型规模)。低于此规模时
/// O(n) 与 O(k) 差异可忽略,退化路径避免引入 partition 开销与候选列表
/// 语义变化(退化路径候选含全部模型,稀疏路径仅含 Top-K)。
pub const DEFAULT_MOE_THRESHOLD: usize = 50;

/// 默认 Top-K 激活数 — 稀疏路径下完整评估的候选数量
///
/// WHY 5:覆盖典型路由的候选广度(主选 + 4 个降级候选),与 CACR
/// Downgrade 的"次优候选"语义衔接,同时将完整评估开销固定为常数。
pub const DEFAULT_MOE_TOP_K: usize = 5;

/// MoE 稀疏门控 — 控制 `route_auto` 的大规模稀疏化行为
///
/// 不可变值类型,`Copy` 语义便于在路由热路径上零开销传递。
/// 既是配置载体(threshold/top_k),也是门控执行者(`gate()` 方法)。
///
/// `RouterConfig` 用独立标量字段(`moe_threshold`/`moe_top_k`)而非内嵌
/// `MoeGate` 序列化,保持与 `cacr` 字段一致的渐进式 serde default 设计;
/// 构造 `MoeGate` 时从 `RouterConfig` 字段取值传入 `MoeGate::new`。
///
/// # 使用示例
/// ```
/// use model_router::{MoeGate, ModelRegistry, RouterConfig, RoutingRequest, RoutingStrategy};
/// use nexus_core::{UserIntent, MultimodalInput};
///
/// let registry = ModelRegistry::from_config(&RouterConfig::default());
/// let gate = MoeGate::default();
/// let req = RoutingRequest {
///     quest_id: "q".into(),
///     intent: UserIntent {
///         intent_id: "i".into(),
///         raw_text: "hi".into(),
///         multimodal_inputs: vec![MultimodalInput::Text("hi".into())],
///         risk_level: 10,
///     },
///     estimated_tokens: 1000,
///     strategy: RoutingStrategy::Auto,
/// };
/// let models = registry.list();
/// let candidates = gate.gate(&models, &req);
/// // 3 模型 < 50 阈值,退化为全量(返回全部引用)
/// assert_eq!(candidates.len(), models.len());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoeGate {
    /// 稀疏化触发阈值:模型数 >= threshold 时启用 Top-K 稀疏门控
    pub threshold: usize,
    /// Top-K 激活数:稀疏路径下仅完整评估 top_k 个候选
    pub top_k: usize,
}

impl Default for MoeGate {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_MOE_THRESHOLD,
            top_k: DEFAULT_MOE_TOP_K,
        }
    }
}

impl MoeGate {
    /// 创建门控,指定阈值与 Top-K
    ///
    /// WHY top_k 至少为 1:Top-0 无意义,且会导致候选列表为空。
    /// threshold 不做强约束(should_sparsify 用 >= 判定,由调用方负责语义)。
    pub fn new(threshold: usize, top_k: usize) -> Self {
        Self {
            threshold,
            top_k: top_k.max(1),
        }
    }

    /// 模型数 >= threshold 时启用稀疏门控
    pub fn should_sparsify(&self, n: usize) -> bool {
        n >= self.threshold
    }

    /// 有效 Top-K — 不超过模型数,避免 select_nth 越界
    pub fn effective_k(&self, n: usize) -> usize {
        self.top_k.min(n)
    }

    /// 门控评分(轻量级)— 无需全局 max 归一化,每模型独立计算
    ///
    /// WHY 倒数形式 `1/(1+x)`:
    /// - 值域 (0,1],方向与完整评分 `1 - x/max` 一致(越小越好 → 分越高)
    /// - 无需预计算全局 max,支持单遍 O(n) 评分 + Top-K 选取
    /// - 纯算术,无字符串操作,常数因子远低于完整评估(含归一化遍历 + format!)
    ///
    /// 权重 0.4/0.4/0.2 与 `route_auto` 完整评分一致,保证粗筛排序近似。
    /// `quality_score` 为 f32,显式 `as f64` 转换参与 f64 运算(此处为算术
    /// 运算而非比较,不触发 §4.4 #6 的 f32→f64 比较精度问题)。
    fn gate_score(m: &ModelInfo) -> f64 {
        // 成本倒数:cost_per_1k_tokens 单位为美元,量级 0.0001~0.05,
        // 乘以 1000 放大到 0.1~50 区间,使倒数有合理区分度
        let cost_gate = 1.0 / (1.0 + m.cost_per_1k_tokens * 1000.0);
        // 延迟倒数:avg_latency_ms 单位为毫秒,量级 50~1000,
        // 除以 100 归一到 0.5~10 区间,使倒数有合理区分度
        let latency_gate = 1.0 / (1.0 + m.avg_latency_ms as f64 / 100.0);
        let quality = m.quality_score as f64;
        0.4 * cost_gate + 0.4 * latency_gate + 0.2 * quality
    }

    /// 门控评分降序比较器:评分高者优先,相同则 model_id 升序(保证确定性)
    ///
    /// WHY 命名为独立 fn(item 天生 Copy):可同时传递给
    /// `select_nth_unstable_by` 和 `sort_by`,避免闭包 move 后无法复用。
    fn cmp_gate_score_desc(a: &(f64, &ModelInfo), b: &(f64, &ModelInfo)) -> Ordering {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.1.model_id.cmp(&b.1.model_id))
    }

    /// 执行门控:返回 Top-K 候选引用(评分降序),或退化时返回全部
    ///
    /// # 复杂度
    /// - 退化(模型数 < threshold):O(n) 收集引用,不排序
    /// - 门控:O(n) 评分 + O(n) `select_nth_unstable_by` + O(k log k) 排序 = O(n)
    ///
    /// # 返回
    /// - 门控模式:Top-K 候选引用(按门控评分降序)
    /// - 退化模式:全部模型引用(原顺序,交给 `route_auto` 全量评估排序)
    ///
    /// WHY 退化模式不排序:保持与历史全量评估行为完全一致(由调用方排序),
    /// 避免引入额外的排序顺序差异。
    pub fn gate<'a>(&self, models: &'a [ModelInfo], _req: &RoutingRequest) -> Vec<&'a ModelInfo> {
        if !self.should_sparsify(models.len()) {
            // WHY 退化:模型数低于阈值,全量评估的绝对耗时在微秒级,
            // 门控评分开销不划算;返回全部引用交给 route_auto 全量评估。
            return models.iter().collect();
        }

        // 防御 top_k > models.len() 的边界(effective_k clamp)
        let k = self.effective_k(models.len());

        // 轻量级门控评分:O(n),纯算术无 format!
        let mut scored: Vec<(f64, &ModelInfo)> =
            models.iter().map(|m| (Self::gate_score(m), m)).collect();

        // Top-K 选取:O(n) select_nth_unstable_by
        // WHY select_nth_unstable_by:O(n) 部分排序,符合 §4.1 Engineering Convention,
        // 禁止 sort_by(O(n log n))做 Top-K。partition 后 [0..k] 为 Top-K(无序)。
        // 传入 k-1 使 [0..k] 恰好为 Top-K(含 k 个元素)。
        if k < scored.len() {
            scored.select_nth_unstable_by(k - 1, Self::cmp_gate_score_desc);
        }

        // 截取 Top-K 并按评分降序排序(便于完整评估优先处理高分候选)
        scored.truncate(k);
        scored.sort_by(Self::cmp_gate_score_desc);

        scored.into_iter().map(|(_, m)| m).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RoutingStrategy;
    use nexus_core::{MultimodalInput, UserIntent};

    fn make_model(id: &str, cost: f64, latency: u64, quality: f32) -> ModelInfo {
        ModelInfo {
            model_id: id.into(),
            provider: "test".into(),
            cost_per_1k_tokens: cost,
            avg_latency_ms: latency,
            max_context: 8192,
            quality_score: quality,
        }
    }

    fn make_request() -> RoutingRequest {
        RoutingRequest {
            quest_id: "q-1".into(),
            intent: UserIntent {
                intent_id: "i-1".into(),
                raw_text: "test".into(),
                multimodal_inputs: vec![MultimodalInput::Text("test".into())],
                risk_level: 10,
            },
            estimated_tokens: 1000,
            strategy: RoutingStrategy::Auto,
        }
    }

    #[test]
    fn test_default_constants() {
        let gate = MoeGate::default();
        assert_eq!(gate.threshold, DEFAULT_MOE_THRESHOLD);
        assert_eq!(gate.top_k, DEFAULT_MOE_TOP_K);
        assert_eq!(gate.threshold, 50);
        assert_eq!(gate.top_k, 5);
    }

    #[test]
    fn test_new_clamps_top_k_to_one() {
        // WHY top_k=0 无意义,强制为 1 保护候选列表非空语义
        let gate = MoeGate::new(50, 0);
        assert_eq!(gate.top_k, 1);
    }

    #[test]
    fn test_new_preserves_threshold() {
        let gate = MoeGate::new(100, 3);
        assert_eq!(gate.threshold, 100);
        assert_eq!(gate.top_k, 3);
    }

    #[test]
    fn test_should_sparsify_boundary() {
        let gate = MoeGate::new(50, 5);
        assert!(!gate.should_sparsify(49), "49 < 50 不应稀疏化");
        assert!(gate.should_sparsify(50), "50 >= 50 应稀疏化");
        assert!(gate.should_sparsify(200));
    }

    #[test]
    fn test_effective_k_capped_by_n() {
        let gate = MoeGate::new(50, 5);
        assert_eq!(gate.effective_k(200), 5);
        assert_eq!(gate.effective_k(3), 3, "n < top_k 时取 n");
        assert_eq!(gate.effective_k(5), 5);
    }

    #[test]
    fn test_gate_score_low_cost_high_score() {
        // 低成本应得高分(倒数形式)
        let cheap = make_model("cheap", 0.0001, 100, 0.6);
        let expensive = make_model("expensive", 0.05, 100, 0.6);
        assert!(MoeGate::gate_score(&cheap) > MoeGate::gate_score(&expensive));
    }

    #[test]
    fn test_gate_score_low_latency_high_score() {
        let fast = make_model("fast", 0.001, 50, 0.6);
        let slow = make_model("slow", 0.001, 1000, 0.6);
        assert!(MoeGate::gate_score(&fast) > MoeGate::gate_score(&slow));
    }

    #[test]
    fn test_gate_score_high_quality_high_score() {
        let good = make_model("good", 0.001, 100, 0.95);
        let bad = make_model("bad", 0.001, 100, 0.5);
        assert!(MoeGate::gate_score(&good) > MoeGate::gate_score(&bad));
    }

    #[test]
    fn test_gate_degrades_below_threshold() {
        // 3 模型 < 50 阈值,退化为全量
        let models = vec![
            make_model("m1", 0.001, 100, 0.8),
            make_model("m2", 0.002, 200, 0.7),
            make_model("m3", 0.003, 300, 0.6),
        ];
        let gate = MoeGate::default();
        let req = make_request();
        let result = gate.gate(&models, &req);
        assert_eq!(result.len(), 3, "退化模式应返回全部模型");
    }

    #[test]
    fn test_gate_activates_top_k_above_threshold() {
        // 50 模型 >= 50 阈值,激活 Top-5
        let models: Vec<ModelInfo> = (0..50)
            .map(|i| {
                make_model(
                    &format!("m{i}"),
                    0.001 + i as f64 * 0.0001,
                    100 + i * 10,
                    (0.5 + i as f32 * 0.01).min(1.0),
                )
            })
            .collect();
        let gate = MoeGate::default();
        let req = make_request();
        let result = gate.gate(&models, &req);
        assert_eq!(result.len(), 5, "门控模式应返回 Top-5");
    }

    #[test]
    fn test_gate_custom_top_k() {
        // 自定义 top_k=3
        let models: Vec<ModelInfo> = (0..60)
            .map(|i| {
                make_model(
                    &format!("m{i}"),
                    0.001 + i as f64 * 0.0001,
                    100 + i * 10,
                    0.5,
                )
            })
            .collect();
        let gate = MoeGate::new(50, 3);
        let req = make_request();
        let result = gate.gate(&models, &req);
        assert_eq!(result.len(), 3, "自定义 top_k=3 应返回 3 个候选");
    }

    #[test]
    fn test_gate_returns_sorted_descending() {
        // 门控结果应按评分降序
        let models: Vec<ModelInfo> = (0..55)
            .map(|i| {
                make_model(
                    &format!("m{i:02}"),
                    0.001 + i as f64 * 0.0001,
                    100 + i * 10,
                    0.5,
                )
            })
            .collect();
        let gate = MoeGate::default();
        let req = make_request();
        let result = gate.gate(&models, &req);
        // 验证降序:每个候选的 gate_score 应 >= 下一个
        for i in 0..result.len() - 1 {
            let score_curr = MoeGate::gate_score(result[i]);
            let score_next = MoeGate::gate_score(result[i + 1]);
            assert!(
                score_curr >= score_next,
                "候选应降序排列: [{}] score {:.6} < [{}] score {:.6}",
                i,
                score_curr,
                i + 1,
                score_next
            );
        }
    }

    #[test]
    fn test_gate_top_k_clamped_when_models_fewer_than_k() {
        // threshold=1(极低阈值)但只有 3 模型,top_k=5 应 clamp 到 3
        let models = vec![
            make_model("m1", 0.001, 100, 0.8),
            make_model("m2", 0.002, 200, 0.7),
            make_model("m3", 0.003, 300, 0.6),
        ];
        let gate = MoeGate::new(1, 5);
        let req = make_request();
        let result = gate.gate(&models, &req);
        assert_eq!(
            result.len(),
            3,
            "top_k > models.len() 时应 clamp 到 models.len()"
        );
    }

    #[test]
    fn test_gate_includes_best_model_in_top_k() {
        // 门控 Top-K 应包含真正评分最高的模型(召回验证)
        let best = make_model("best", 0.0001, 50, 0.99);
        let mut models: Vec<ModelInfo> = (0..60)
            .map(|i| {
                make_model(
                    &format!("m{i}"),
                    0.01 + i as f64 * 0.0001,
                    200 + i * 10,
                    0.5,
                )
            })
            .collect();
        models.push(best);
        let gate = MoeGate::default();
        let req = make_request();
        let result = gate.gate(&models, &req);
        let ids: Vec<&str> = result.iter().map(|m| m.model_id.as_str()).collect();
        assert!(ids.contains(&"best"), "Top-K 应包含最优模型, got {:?}", ids);
    }
}
