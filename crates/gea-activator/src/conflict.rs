//! GEA 专家冲突消解 — 功能重叠检测与 Top-K 选择
//!
//! 对应架构层:L6 Router
//! 对应创新点:GEA(Gated Expert Activation)
//!
//! # 设计决策(WHY)
//! - 综合评分 = gate_value × expert_priority:门控值反映任务匹配度,
//!   优先级反映专家本身的能力权重,两者相乘得到最终排序依据
//! - 功能重叠检测基于 CLV 余弦相似度:复用 `nexus_core::cosine_similarity_slices`,
//!   重叠度 > overlap_threshold 时仅保留评分更高者,避免冗余激活
//! - Top-K 排序用 `select_nth_unstable`:Top-K 选择 O(n),
//!   优于全排序 `sort_by` 的 O(n log n)(继承 Week 3 经验)
//!
//! # 重叠检测热路径优化(专家 Agent 优化 2026-08-11)
//! - **范数预计算**:候选专家向量范数一次 O(n·d) 预算完成,随 ScoredCandidate
//!   携带(无额外 HashMap),内层检测直接用 `dot / (na × nb)`,省去每对
//!   检测的 2× 范数 + 2× sqrt(原 `cosine_similarity_slices` 每次调用重算)
//! - **4 路累加点积**:与 `cosine_similarity_slices` 完全相同的累加结构
//!   (chunks_exact(4) + 4 路累加器 + 尾部补算),SIMD 友好且结果逐位一致
//! - **全非负单调剪枝**:当两向量均无负分量时,点积部分和单调不减——
//!   部分和一旦超过 `threshold × na × nb × (1+ε)` 数学上必冲突,提前 break
//!   (严格正确:单调性保证方向,ε 吸收乘法/除法舍入差异)
//! - **含负值路径**:跳过剪枝,完整 4 路点积 + 预计算范数判定(与基线一致)
//!
//! # 正确性论证(为何零 flaky)
//! 1. 全非负时 dot 部分和单调不减(IEEE 加法加非负数不会减小)→ 剪枝结论必然成立
//! 2. 未剪枝时 overlap = dot/(na·nb),dot/na/nb 与基线逐位一致(同累加结构)
//! 3. ε = 1e-6 覆盖乘法(剪枝)与除法(判定)的舍入差 → 边界判定一致

use std::collections::HashMap;

use crate::config::GeaConfig;
use crate::error::GeaError;
use crate::types::{ActivationResult, ExpertId, ExpertProfile};

/// 单调剪枝的相对裕量
///
/// WHY 1e-6:剪枝比较用乘法(`dot > threshold × na × nb × (1+ε)`),
/// 精确判定用除法(`dot / (na × nb) > threshold`)。ε 吸收两种舍入方向的
/// 差异,保证剪枝触发的候选在精确判定下必然冲突;边界带内不剪枝,走精确路径。
const PRUNE_EPSILON: f32 = 1e-6;

/// 候选专家条目:(ExpertId, gate_value)
pub type Candidate = (ExpertId, f32);

/// 专家向量范数信息 — 注册时预计算,冲突消解热路径复用
///
/// `l2_norm` 为向量前缀 L2 范数(与 cosine_similarity_slices 位级一致)
/// `all_non_negative` 标记向量是否全非负(单调剪枝前提)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExpertNorm {
    /// 向量 L2 范数
    pub l2_norm: f32,
    /// 向量是否全非负
    pub all_non_negative: bool,
}

impl ExpertNorm {
    /// 从向量计算范数信息(单 pass,与 norm_and_nonneg 一致)
    pub fn from_vector(v: &[f32]) -> Self {
        let (l2_norm, all_non_negative) = norm_and_nonneg(v, v.len());
        Self {
            l2_norm,
            all_non_negative,
        }
    }
}

/// 综合评分条目 — 含预计算范数信息(避免内层检测重复计算)
///
/// `composite_score = gate_value × expert_priority`
/// `l2_norm` 为向量前缀 L2 范数(与 cosine_similarity_slices 位级一致)
/// `all_non_negative` 标记向量是否全非负(启用单调剪枝的前提)
#[derive(Clone)]
struct ScoredCandidate {
    /// 专家 ID
    id: ExpertId,
    /// 门控值
    gate: f32,
    /// 综合评分 = gate × priority
    composite: f32,
    /// 向量前缀 L2 范数(预计算)
    l2_norm: f32,
    /// 向量是否全非负(单调剪枝前提)
    all_non_negative: bool,
}

/// 解决专家冲突:综合评分排序 + 功能重叠检测 + Top-K 选择
///
/// # 算法步骤
/// 1. 计算每个候选的综合评分:`gate_value × expert_priority`
/// 2. 按综合评分降序排序
/// 3. 贪心遍历:对每个候选,检查与已激活专家的重叠度,
///    重叠度 > `overlap_threshold` 则抑制(仅保留评分更高者)
/// 4. 取 Top-K 作为最终激活列表,其余为抑制列表
///
/// # 参数
/// - `candidates`:候选专家列表 `(ExpertId, gate_value)`
/// - `expert_profiles`:专家画像表,用于查询优先级与向量
/// - `config`:配置(含 overlap_threshold、top_k)
///
/// # 错误
/// - `ExpertNotFound`:候选专家不在 `expert_profiles` 中
/// - `ConflictResolutionFailed`:所有候选均被抑制(理论上不会发生,防御性返回)
pub fn resolve_conflicts(
    candidates: Vec<Candidate>,
    expert_profiles: &HashMap<ExpertId, ExpertProfile>,
    config: &GeaConfig,
) -> Result<ActivationResult, GeaError> {
    // 无外部范数缓存:单 pass 就地计算(无中间 HashMap,实测最优)
    resolve_inner(candidates, expert_profiles, config, |_, profile| {
        ExpertNorm::from_vector(&profile.expert_vector)
    })
}

/// 带预计算范数的冲突消解 — activate 热路径专用(专家 Agent 优化 2026-08-11)
///
/// 调用方(GeaActivator)持有注册时缓存的 `expert_norms`,候选构建阶段
/// 免去 O(n·d) 范数重算(高密度专家池下为主要开销之一,512 专家 ≈ 25µs)。
/// 内部逻辑与 `resolve_conflicts` 完全一致,仅范数来源不同。
pub fn resolve_conflicts_with_norms(
    candidates: Vec<Candidate>,
    expert_profiles: &HashMap<ExpertId, ExpertProfile>,
    norms: &HashMap<ExpertId, ExpertNorm>,
    config: &GeaConfig,
) -> Result<ActivationResult, GeaError> {
    resolve_inner(candidates, expert_profiles, config, |id, profile| {
        norms
            .get(id)
            .copied()
            .unwrap_or_else(|| ExpertNorm::from_vector(&profile.expert_vector))
    })
}

/// 冲突消解核心逻辑 — 范数来源由调用方注入(计算 vs 查缓存)
///
/// WHY 泛型闭包而非复制两份:两个公共入口仅范数获取方式不同,
/// 闭包经 monomorphization 内联后无额外开销,消除重复代码。
fn resolve_inner<F>(
    candidates: Vec<Candidate>,
    expert_profiles: &HashMap<ExpertId, ExpertProfile>,
    config: &GeaConfig,
    mut norm_provider: F,
) -> Result<ActivationResult, GeaError>
where
    F: FnMut(&ExpertId, &ExpertProfile) -> ExpertNorm,
{
    if candidates.is_empty() {
        return Ok(ActivationResult::empty());
    }

    // 步骤 1:计算综合评分 + 获取范数信息,校验专家存在性
    let mut scored: Vec<ScoredCandidate> = Vec::with_capacity(candidates.len());
    for (expert_id, gate_value) in candidates {
        let profile = expert_profiles
            .get(&expert_id)
            .ok_or_else(|| GeaError::ExpertNotFound {
                expert_id: expert_id.to_string(),
            })?;
        let composite = gate_value * profile.priority;
        let norm = norm_provider(&expert_id, profile);
        scored.push(ScoredCandidate {
            id: expert_id,
            gate: gate_value,
            composite,
            l2_norm: norm.l2_norm,
            all_non_negative: norm.all_non_negative,
        });
    }

    // 步骤 2:按综合评分降序排序(全排序,因为后续需贪心遍历全部)
    // WHY 全排序而非 select_nth_unstable:冲突检测需按评分从高到低贪心遍历,
    // select_nth_unstable 仅保证 Top-K 在前 K 位但内部无序,无法满足贪心顺序要求。
    // Top-K 选择优化在步骤 4 之后对"已通过冲突检测的列表"使用。
    scored.sort_by(|a, b| {
        b.composite
            .partial_cmp(&a.composite)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // 步骤 3:贪心冲突检测 — 重叠度 > threshold 则抑制
    // WHY 早停(L9 优化第二轮,O(n²·d)→O(n·k·d)):scored 已按综合评分降序,
    // 一旦 activated 集满 top_k,剩余候选(评分更低)即使无冲突也必被后续
    // select_top_k 裁掉 → 直接全部抑制并 break。旧版对每个候选检查全部已激活
    // (可能 >top_k),早停后每候选只检查 ≤top_k 个已激活。最终 activated/suppressed
    // 集合与旧实现等价(activated = 最高分的 top_k 个无冲突候选,与 select_top_k 一致)。
    let mut activated_with_score: Vec<ScoredCandidate> = Vec::new();
    let mut suppressed: Vec<ExpertId> = Vec::new();

    let mut scored_iter = scored.into_iter();
    for candidate in scored_iter.by_ref() {
        let profile =
            expert_profiles
                .get(&candidate.id)
                .ok_or_else(|| GeaError::ExpertNotFound {
                    expert_id: candidate.id.to_string(),
                })?;

        // 检查与所有已激活专家的重叠度(早停后 activated 恒 ≤ top_k,故此内循环 O(k·d))
        let mut conflict = false;
        for activated in &activated_with_score {
            let activated_profile =
                expert_profiles
                    .get(&activated.id)
                    .ok_or_else(|| GeaError::ExpertNotFound {
                        expert_id: activated.id.to_string(),
                    })?;

            let (a, b) = (&profile.expert_vector, &activated_profile.expert_vector);
            let len = a.len().min(b.len());
            let na = candidate.l2_norm;
            let nb = activated.l2_norm;

            // 全非负 → 点积单调不减,可安全剪枝;
            // 含负值 → 完整 4 路点积 + 精确判定(与基线逐位一致)
            let threshold_product = config.overlap_threshold * na * nb;
            let overlap = if candidate.all_non_negative && activated.all_non_negative && len > 0 {
                // 单调剪枝:部分和一旦超过阈值×(1+ε)即必冲突,提前 break
                let bound = threshold_product * (1.0 + PRUNE_EPSILON);
                match dot_prune(a, b, len, bound) {
                    // None = 已剪枝(部分和超界,必冲突)
                    None => {
                        conflict = true;
                        break;
                    }
                    Some(dot) => {
                        if na > 0.0 && nb > 0.0 {
                            (dot / (na * nb)).clamp(-1.0, 1.0)
                        } else {
                            0.0
                        }
                    }
                }
            } else {
                let dot = dot_4way(a, b, len);
                if na > 0.0 && nb > 0.0 {
                    (dot / (na * nb)).clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            };
            if !conflict && overlap > config.overlap_threshold {
                conflict = true;
                break;
            }
        }

        if conflict {
            suppressed.push(candidate.id);
        } else {
            activated_with_score.push(candidate);
            // 早停:已集满 top_k,剩余候选分更低必被裁掉,无需再检查重叠
            // (top_k=0 退化时此条件不成立,交由下方 select_top_k 正确处理)
            if activated_with_score.len() == config.top_k {
                break;
            }
        }
    }
    // 剩余未处理候选(分更低)全部抑制——不可能进 top_k
    suppressed.extend(scored_iter.map(|c| c.id));

    // 步骤 4:Top-K 选择 — 使用 select_nth_unstable 优化(O(n))
    // WHY:已通过冲突检测的列表可能超过 top_k,只需前 K 个,无需全排序
    let top_gate_value = activated_with_score.first().map(|c| c.gate).unwrap_or(0.0);

    let (activated, extra_suppressed) = select_top_k(activated_with_score, config.top_k);

    // 未进入 Top-K 的也加入 suppressed
    let mut all_suppressed = suppressed;
    all_suppressed.extend(extra_suppressed);

    Ok(ActivationResult {
        activated,
        suppressed: all_suppressed,
        top_gate_value,
    })
}

/// 单 pass 计算前缀 L2 范数 + 全非负标记(候选构建热路径)
///
/// WHY 符号检查与范数累加分离:混入比较分支会阻碍 LLVM 对 4 路累加器的
/// auto-vectorization(实测 512 专家池候选构建 30µs+)。范数循环保持纯算术
/// (SIMD 友好),符号检查为独立简单比较(编译器同样可向量化)。
/// NaN 分量经 `!(x >= 0.0)` 判定归入"含负值"路径,与精确判定行为一致。
#[inline]
fn norm_and_nonneg(v: &[f32], len: usize) -> (f32, bool) {
    let prefix = &v[..len.min(v.len())];
    // 范数核心合并(F-R5-01):4 路累加范数体与 gating::prefix_l2_norm 位级一致,
    // 直接委托消除重复;nonneg 保留独立遍历(简单比较,可并行向量化)。
    let norm = crate::gating::prefix_l2_norm(v, len);
    // 符号检查独立遍历(简单比较,可向量化)
    let nonneg = prefix.iter().all(|x| *x >= 0.0);
    (norm, nonneg)
}

/// 4 路累加点积 — 与 `cosine_similarity_slices` 的 dot 累加结构逐位一致
///
/// chunks_exact(4) + 4 路累加器 + 尾部补算,LLVM 可 auto-vectorize 为 SIMD。
/// 返回值与基线实现的 dot 位级一致(同累加顺序)。
#[inline]
fn dot_4way(a: &[f32], b: &[f32], len: usize) -> f32 {
    let (mut d0, mut d1, mut d2, mut d3) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
    let (as_, bs) = (&a[..len], &b[..len]);
    for (ca, cb) in as_.chunks_exact(4).zip(bs.chunks_exact(4)) {
        d0 += ca[0] * cb[0];
        d1 += ca[1] * cb[1];
        d2 += ca[2] * cb[2];
        d3 += ca[3] * cb[3];
    }
    let mut dot = d0 + d1 + d2 + d3;
    let processed = (len / 4) * 4;
    for i in processed..len {
        dot += a[i] * b[i];
    }
    dot
}

/// 带单调剪枝的 4 路累加点积 — 仅用于两向量均全非负时
///
/// # 正确性
/// 全非负向量点积的部分和单调不减(IEEE 加法加非负数不会减小),
/// 故部分和一旦超过 `bound` 则最终点积必然 > bound → 必冲突,可提前返回 None。
/// 返回 `None` 表示已剪枝(必冲突);`Some(dot)` 表示完整点积(未超界)。
/// 完整点积的累加结构与 `dot_4way` 一致,逐位相同。
///
/// # 剪枝检查粒度(WHY 16 维)
/// 每 4 维检查一次会在热循环内插入高频分支,阻碍 LLVM auto-vectorization
/// (实测 128 高重叠 +9% 回退)。每 16 维(4 个 chunk)检查一次:分支频率降
/// 4 倍恢复 SIMD,且对"稀疏大分量对齐"场景(点积前几维即超界)剪枝收益不变;
/// 对均匀增长场景(如全 1.0 向量)最多多算 15 维,损失可忽略。
#[inline]
fn dot_prune(a: &[f32], b: &[f32], len: usize, bound: f32) -> Option<f32> {
    let (mut d0, mut d1, mut d2, mut d3) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
    let (as_, bs) = (&a[..len], &b[..len]);
    let chunks = as_.chunks_exact(4).zip(bs.chunks_exact(4));
    // 每 PRUNE_CHECK_STRIDE 个 chunk 检查一次部分和(16 维粒度)
    const PRUNE_CHECK_STRIDE: usize = 4;
    let mut chunk_count = 0usize;
    for (ca, cb) in chunks {
        d0 += ca[0] * cb[0];
        d1 += ca[1] * cb[1];
        d2 += ca[2] * cb[2];
        d3 += ca[3] * cb[3];
        chunk_count += 1;
        if chunk_count.is_multiple_of(PRUNE_CHECK_STRIDE) {
            // 4 路合并顺序与最终一致
            let partial = d0 + d1 + d2 + d3;
            if partial > bound {
                return None;
            }
        }
    }
    let mut dot = d0 + d1 + d2 + d3;
    let processed = (len / 4) * 4;
    for i in processed..len {
        dot += a[i] * b[i];
        if dot > bound {
            return None;
        }
    }
    Some(dot)
}

/// 从已通过冲突检测的列表中选择 Top-K
///
/// 分区 + 稳定降序核心委托 L0 `nexus_contracts::util::xts_top_k_by`
/// (select_nth_unstable_by O(n) + 前 k 段 sort_by O(k log k);F-R6-03)。
/// 双 Vec(id 列表)整形保留在函数内。
///
/// 返回 (activated_top_k, suppressed_extra)
///
/// WHY k 与 k-1 pivot 差异:xts_top_k_by 用 select_nth_unstable_by(k-1),
/// 前 k 段即 Top-K;原实现用 select_nth_unstable_by(k) 使 pivot 落 index k。
/// 两者 top-k **集合等价**,且 suppressed 均为分区后 `data[k..]`
/// (原实现 suppressed = pivot(k 处) + rest,恰为 data[k..]),故集合与序均一致。
fn select_top_k(mut scored: Vec<ScoredCandidate>, k: usize) -> (Vec<ExpertId>, Vec<ExpertId>) {
    use std::cmp::Ordering;
    let n = scored.len();
    let k_eff = k.min(n);

    // 分区 + 降序核心委托 L0(k>=len 时 xts_top_k_by 自动钳制为全量降序,等价旧 len<=k 分支)
    let top = nexus_contracts::util::xts_top_k_by(
        &mut scored,
        k,
        |a: &ScoredCandidate, b: &ScoredCandidate| {
            b.composite
                .partial_cmp(&a.composite)
                .unwrap_or(Ordering::Equal)
        },
    );

    let activated: Vec<ExpertId> = top.iter().map(|c| c.id.clone()).collect();
    // 剩余(data[k_eff..])均为未进入 Top-K 的抑制条目
    let suppressed: Vec<ExpertId> = scored[k_eff..].iter().map(|c| c.id.clone()).collect();
    (activated, suppressed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_profile(id: &str, vector: Vec<f32>, priority: f32) -> (ExpertId, ExpertProfile) {
        (
            ExpertId::new(id),
            ExpertProfile::new(id, vector, priority, vec![]),
        )
    }

    fn make_profiles(items: Vec<(&str, Vec<f32>, f32)>) -> HashMap<ExpertId, ExpertProfile> {
        items
            .into_iter()
            .map(|(id, v, p)| make_profile(id, v, p))
            .collect()
    }

    #[test]
    fn test_no_conflicts_basic() {
        // 三个专家向量正交,无冲突
        let mut v1 = vec![0.0; 64];
        v1[0] = 1.0;
        let mut v2 = vec![0.0; 64];
        v2[1] = 1.0;
        let mut v3 = vec![0.0; 64];
        v3[2] = 1.0;

        let profiles = make_profiles(vec![("e-1", v1, 0.5), ("e-2", v2, 0.5), ("e-3", v3, 0.5)]);

        let candidates: Vec<Candidate> = vec![
            (ExpertId::new("e-1"), 0.8),
            (ExpertId::new("e-2"), 0.7),
            (ExpertId::new("e-3"), 0.6),
        ];

        let config = GeaConfig::default();
        let result = resolve_conflicts(candidates, &profiles, &config).unwrap();

        // 无冲突,Top-3 全部激活
        assert_eq!(result.activated.len(), 3);
        assert!(result.suppressed.is_empty());
        // 按综合评分降序:0.8*0.5=0.4 > 0.7*0.5=0.35 > 0.6*0.5=0.3
        assert_eq!(result.activated[0], ExpertId::new("e-1"));
        assert_eq!(result.activated[1], ExpertId::new("e-2"));
        assert_eq!(result.activated[2], ExpertId::new("e-3"));
        assert!((result.top_gate_value - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_conflict_high_overlap() {
        // 两个专家向量高度重叠(相同向量),重叠度 = 1.0 > 0.8
        let v = vec![1.0; 64];
        let profiles = make_profiles(vec![("e-1", v.clone(), 0.5), ("e-2", v, 0.5)]);

        let candidates: Vec<Candidate> =
            vec![(ExpertId::new("e-1"), 0.8), (ExpertId::new("e-2"), 0.7)];

        let config = GeaConfig::default();
        let result = resolve_conflicts(candidates, &profiles, &config).unwrap();

        // e-1 评分更高(0.8 > 0.7),e-2 被抑制
        assert_eq!(result.activated.len(), 1);
        assert_eq!(result.activated[0], ExpertId::new("e-1"));
        assert_eq!(result.suppressed.len(), 1);
        assert_eq!(result.suppressed[0], ExpertId::new("e-2"));
    }

    #[test]
    fn test_top_k_boundary() {
        // 5 个无冲突专家,top_k = 3,应激活 3 个,抑制 2 个
        let profiles = make_profiles(vec![
            ("e-1", make_orthogonal(0), 0.5),
            ("e-2", make_orthogonal(1), 0.5),
            ("e-3", make_orthogonal(2), 0.5),
            ("e-4", make_orthogonal(3), 0.5),
            ("e-5", make_orthogonal(4), 0.5),
        ]);

        let candidates: Vec<Candidate> = vec![
            (ExpertId::new("e-1"), 0.9),
            (ExpertId::new("e-2"), 0.8),
            (ExpertId::new("e-3"), 0.7),
            (ExpertId::new("e-4"), 0.6),
            (ExpertId::new("e-5"), 0.5),
        ];

        let config = GeaConfig::default();
        let result = resolve_conflicts(candidates, &profiles, &config).unwrap();

        assert_eq!(result.activated.len(), 3);
        assert_eq!(result.suppressed.len(), 2);
        // Top-3 按评分降序
        assert_eq!(result.activated[0], ExpertId::new("e-1"));
        assert_eq!(result.activated[1], ExpertId::new("e-2"));
        assert_eq!(result.activated[2], ExpertId::new("e-3"));
    }

    #[test]
    fn test_empty_candidates() {
        let profiles = HashMap::new();
        let config = GeaConfig::default();
        let result = resolve_conflicts(vec![], &profiles, &config).unwrap();
        assert!(!result.has_activated());
        assert!((result.top_gate_value - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_expert_not_found() {
        let profiles = HashMap::new();
        let config = GeaConfig::default();
        let candidates: Vec<Candidate> = vec![(ExpertId::new("missing"), 0.8)];
        let result = resolve_conflicts(candidates, &profiles, &config);
        assert!(matches!(result, Err(GeaError::ExpertNotFound { .. })));
    }

    #[test]
    fn test_priority_influence() {
        // e-2 门控值更低但优先级更高,综合评分可能超过 e-1
        let profiles = make_profiles(vec![
            ("e-1", make_orthogonal(0), 0.1), // 0.8 * 0.1 = 0.08
            ("e-2", make_orthogonal(1), 1.0), // 0.7 * 1.0 = 0.70
        ]);

        let candidates: Vec<Candidate> =
            vec![(ExpertId::new("e-1"), 0.8), (ExpertId::new("e-2"), 0.7)];

        let config = GeaConfig::default();
        let result = resolve_conflicts(candidates, &profiles, &config).unwrap();

        // e-2 综合评分更高(0.70 > 0.08),应排第一
        assert_eq!(result.activated[0], ExpertId::new("e-2"));
        assert!((result.top_gate_value - 0.7).abs() < 1e-6);
    }

    #[test]
    fn test_negative_vector_uses_exact_path() {
        // 含负值向量:跳过单调剪枝,走完整 4 路点积 + 预计算范数,
        // 判定必须与 cosine_similarity_slices 精确一致
        let mut v1 = vec![1.0_f32; 64];
        v1[0] = -0.5; // 含负值分量 → all_non_negative = false
        let mut v2 = vec![1.0_f32; 64];
        v2[1] = 0.9;

        let profiles = make_profiles(vec![("e-1", v1.clone(), 0.5), ("e-2", v2.clone(), 0.5)]);
        let candidates: Vec<Candidate> =
            vec![(ExpertId::new("e-1"), 0.8), (ExpertId::new("e-2"), 0.7)];
        let config = GeaConfig::default();
        let result = resolve_conflicts(candidates, &profiles, &config).unwrap();

        // 参考:精确余弦判定
        let exact = nexus_core::cosine_similarity_slices(&v1, &v2);
        let expect_conflict = exact > config.overlap_threshold;
        let actual_conflict = result.suppressed.iter().any(|id| id.as_str() == "e-2");
        assert_eq!(
            actual_conflict, expect_conflict,
            "负值向量路径必须与精确判定一致(exact={exact})"
        );
    }

    #[test]
    fn test_zero_vector_safe() {
        // 零向量:范数 0 → overlap = 0.0,不与任何专家冲突
        let profiles = make_profiles(vec![
            ("e-1", vec![0.0; 64], 0.5),
            ("e-2", vec![1.0; 64], 0.5),
        ]);
        let candidates: Vec<Candidate> =
            vec![(ExpertId::new("e-1"), 0.8), (ExpertId::new("e-2"), 0.7)];
        let config = GeaConfig::default();
        let result = resolve_conflicts(candidates, &profiles, &config).unwrap();
        // 零向量与任何向量 overlap=0 → 无冲突,两者均激活
        assert_eq!(result.activated.len(), 2);
        assert!(result.suppressed.is_empty());
    }

    /// 生成正交向量:仅第 idx 维为 1.0,其余为 0.0
    fn make_orthogonal(idx: usize) -> Vec<f32> {
        let mut v = vec![0.0; 64];
        if idx < 64 {
            v[idx] = 1.0;
        }
        v
    }

    // ============================================================
    // F-R5-01: 范数核心合并 — norm_and_nonneg 与 prefix_l2_norm 等价性基线
    // ============================================================

    #[test]
    fn test_norm_and_nonneg_matches_prefix_l2_norm() {
        // 等价性契约:norm_and_nonneg 的范数部分必须与 gating::prefix_l2_norm 位级一致;
        // nonneg 必须等价于独立遍历 `prefix.iter().all(|x| *x >= 0.0)`(NaN 使
        // `!(NaN >= 0.0)` 成立 → 判为含负值,返回 false)。
        let cases: Vec<(Vec<f32>, usize)> = vec![
            (vec![], 0),                                  // 空切片
            (vec![], 5),                                  // 空切片,翘尾长度
            (vec![1.0f32], 1),                            // 单元素
            (vec![1.0f32, 2.0, 3.0, 4.0], 4),             // 整倍数
            (vec![0.3, 0.7, 0.2, 0.9, 0.5, 0.1, 0.8], 7), // 4k+3 翘尾
            (vec![1.0, -2.0, 3.0, -4.0, 5.0], 5),         // 含负值
            (vec![f32::NAN, 1.0, -1.0], 3),               // 含 NaN
            (vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0], 3),   // len < v.len(),前缀截断
        ];
        for (v, len) in cases {
            let (norm, nonneg) = norm_and_nonneg(&v, len);
            let expected_norm = crate::gating::prefix_l2_norm(&v, len);
            // NaN 分量:位级等价(两实现对同一 NaN 传播应产生相同位型;==NaN 为 false,故用位型比较)
            let norm_equal = norm.to_bits() == expected_norm.to_bits();
            assert!(
                norm_equal,
                "len={} 范数不等: norm_and_nonneg={norm}, prefix_l2_norm={expected_norm}, v={v:?}",
                len
            );
            let prefix = &v[..len.min(v.len())];
            let expected_nonneg = prefix.iter().all(|x| *x >= 0.0);
            assert_eq!(
                nonneg, expected_nonneg,
                "len={} nonneg 不等: got={nonneg}, expected={expected_nonneg}, v={v:?}",
                len
            );
        }
    }

    // ============================================================
    // F-R6-03: select_top_k 委托 L0 — 等价性基线(embedded 旧实现对照 + 不变量)
    // ============================================================

    /// 旧实现参考副本(重构前 select_top_k),嵌入测试仅用于等价性对照,不参与生产。
    fn old_select_top_k(
        mut scored: Vec<ScoredCandidate>,
        k: usize,
    ) -> (Vec<ExpertId>, Vec<ExpertId>) {
        use std::cmp::Ordering;
        if scored.len() <= k {
            scored.sort_by(|a, b| {
                b.composite
                    .partial_cmp(&a.composite)
                    .unwrap_or(Ordering::Equal)
            });
            let activated: Vec<ExpertId> = scored.into_iter().map(|c| c.id).collect();
            return (activated, Vec::new());
        }
        let (top_k, pivot, rest) = scored.select_nth_unstable_by(k, |a, b| {
            b.composite
                .partial_cmp(&a.composite)
                .unwrap_or(Ordering::Equal)
        });
        let mut top_k_sorted: Vec<ScoredCandidate> = top_k.to_vec();
        top_k_sorted.sort_by(|a, b| {
            b.composite
                .partial_cmp(&a.composite)
                .unwrap_or(Ordering::Equal)
        });
        let activated: Vec<ExpertId> = top_k_sorted.into_iter().map(|c| c.id).collect();
        let mut suppressed: Vec<ExpertId> = Vec::with_capacity(rest.len() + 1);
        suppressed.push(pivot.id.clone());
        suppressed.extend(rest.iter().map(|c| c.id.clone()));
        (activated, suppressed)
    }

    fn make_candidate(i: usize, composite: f32) -> ScoredCandidate {
        ScoredCandidate {
            id: ExpertId::new(format!("e-{i}")),
            gate: composite,
            composite,
            l2_norm: 1.0,
            all_non_negative: true,
        }
    }

    fn sorted_ids(ids: &[ExpertId]) -> Vec<String> {
        let mut v: Vec<String> = ids.iter().map(|id| id.as_str().to_string()).collect();
        v.sort();
        v
    }

    #[test]
    fn test_select_top_k_equivalence_with_old_impl() {
        // 输入覆盖:k=0 / k>=len / 正常 k / 空 / NaN 分量
        // 与嵌入旧实现对照:activated 精确序 + suppressed 集合序一致(委托 L0 后
        // suppressed 内部顺序属未定型实现细节,仅集合保证)。另断言三条不变量。
        let scored_sets: Vec<Vec<ScoredCandidate>> = vec![
            vec![],
            vec![make_candidate(0, 0.9), make_candidate(1, 0.8)],
            (0..5)
                .map(|i| make_candidate(i, 0.9 - i as f32 * 0.1))
                .collect(),
            vec![
                make_candidate(0, 0.3),
                make_candidate(1, 0.5),
                make_candidate(2, 0.4),
                make_candidate(3, 0.1),
            ],
            // NaN 分量:验证 partial_cmp 回落 Equal 路径不崩溃、总数/长度不变量保持
            vec![
                make_candidate(0, 0.9),
                make_candidate(1, f32::NAN),
                make_candidate(2, 0.7),
            ],
        ];
        for k in [0usize, 1, 2, 3, 5, 10] {
            for scored in &scored_sets {
                let comp: std::collections::HashMap<&str, f32> = scored
                    .iter()
                    .map(|c| (c.id.as_str(), c.composite))
                    .collect();
                let (new_act, new_sup) = select_top_k(scored.clone(), k);
                let (old_act, old_sup) = old_select_top_k(scored.clone(), k);
                // ① activated 与旧实现精确一致(降序)
                assert_eq!(
                    new_act,
                    old_act,
                    "k={k} len={} activated 与旧实现不等",
                    scored.len()
                );
                // ② suppressed 集合与旧实现一致(排序后比对)
                assert_eq!(
                    sorted_ids(&new_sup),
                    sorted_ids(&old_sup),
                    "k={k} len={} suppressed 集合不等",
                    scored.len()
                );
                // ③ 无重复/丢失:汇总 == 输入全量
                let mut all = sorted_ids(&new_act);
                all.extend(sorted_ids(&new_sup));
                all.sort();
                let mut input_ids: Vec<String> =
                    scored.iter().map(|c| c.id.as_str().to_string()).collect();
                input_ids.sort();
                assert_eq!(all, input_ids, "k={k} id 有重复或丢失");
                // ④ activated.len == min(k, len)
                assert_eq!(
                    new_act.len(),
                    k.min(scored.len()),
                    "k={k} activated 数量错误"
                );
                // ⑤ 无 NaN 时 suppressed 单调:activated 最小 >= suppressed 最大 composite
                let has_nan = scored.iter().any(|c| c.composite.is_nan());
                if !has_nan
                    && !new_act.is_empty()
                    && new_act.len() < scored.len()
                    && !scored.is_empty()
                {
                    let act_min = new_act
                        .iter()
                        .map(|id| comp[id.as_str()])
                        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                        .unwrap();
                    let sup_max = new_sup
                        .iter()
                        .map(|id| comp[id.as_str()])
                        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                        .unwrap();
                    assert!(
                        act_min >= sup_max,
                        "k={k} 破坏单调:activated min={act_min} < suppressed max={sup_max}"
                    );
                }
            }
        }
    }
}
