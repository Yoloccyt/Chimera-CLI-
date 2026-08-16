//! data::curator — ContextCurator 客户端上下文策展器(Concord W9 T9.1/T9.2/T9.4,ADR-081)
//!
//! 对应架构层:L10 Interface
//!
//! # 职责
//! 把 `/compact` 从"一次性全量摘要"升级为"策展系统"(方案 v2.0 §5.3):
//! 1. **五段分类**:system / pinned / recent / evictable(候选)
//!    —— 当前无 system/pin UI,诚实映射:System 预留恒空,Pinned = User
//!    轮次(用户消息不可驱逐,对齐主流 /compact 语义),Recent = 末 k 轮;
//! 2. **价值密度打分**:`α·recency + β·ref_count + γ·pinned − δ·token_cost`
//!    (f32 全程不转 f64,红线);
//! 3. **0-1 背包预算分配**:强制保留段先扣预算,候选段在剩余容量内最大化
//!    保留价值(O(n·W),滚动双行 dp + 决策矩阵回溯);
//! 4. **摘要接缝**(`SummaryBackend` trait):落选高价值段经
//!    [`ExtractiveSummary`] 抽取式摘要(纯 Rust,无模型依赖——否决的是本地
//!    压缩**模型**,抽取式文本合纯 Rust 红线);`ApiSummary`(主 API 异步
//!    摘要)接缝预留,待隐藏摘要通道就绪后接线(ADR-081 诚实标注)。
//!
//! # RL 预留
//! [`CurationPolicy`] trait 为策略接缝,`RuleCurationPolicy` 规则实现为首参;
//! MemAgent/MemAct 的学习策略未来替换规则表而不改调用方。

use serde::{Deserialize, Serialize};

use crate::types::{ChatMessage, ChatRole};

// ============================================================
// 配置与策略档
// ============================================================

/// 策展配置 — 权重/预算/摘要参数(进 `TuiConfig.curation`,serde 四源可配)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurationConfig {
    /// 新近性权重 α(越靠后价值越高)
    pub alpha: f32,
    /// 引用计数权重 β(@mention 数)
    pub beta: f32,
    /// 钉选权重 γ(User 轮次固有保护)
    pub gamma: f32,
    /// token 成本惩罚 δ(越长越倾向驱逐)
    pub delta: f32,
    /// 总预算(token 估算单位,策略档乘数作用于本值)
    pub budget_tokens: usize,
    /// Recent 保护窗口(轮次;一轮 ≈ 2 条消息)
    pub recent_turns: usize,
    /// 落选段进入摘要流的价值阈值(低于则直接丢弃)
    pub summary_value_threshold: f32,
    /// 抽取式摘要:每条消息取首 N 字符
    pub summary_per_msg_chars: usize,
    /// 抽取式摘要:总量封顶(字符)
    pub summary_total_chars: usize,
}

impl Default for CurationConfig {
    fn default() -> Self {
        Self {
            // 初值标定:新近性主导,引用与钉选加成,长度轻惩罚
            alpha: 1.0,
            beta: 0.3,
            gamma: 0.5,
            delta: 0.2,
            budget_tokens: 4096,
            recent_turns: 4,
            summary_value_threshold: 0.1,
            summary_per_msg_chars: 80,
            summary_total_chars: 600,
        }
    }
}

/// 压缩策略档 — `/compact [--policy]` 参数
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompactPolicy {
    /// 激进:预算 ×0.5
    Aggressive,
    /// 均衡(缺省):预算 ×0.75
    Balanced,
    /// 保守:预算 ×1.0
    Conservative,
}

impl CompactPolicy {
    /// 预算乘数
    pub fn multiplier(self) -> f32 {
        match self {
            Self::Aggressive => 0.5,
            Self::Balanced => 0.75,
            Self::Conservative => 1.0,
        }
    }

    /// 从命令参数解析;未识别返回 None(执行层诚实反馈用法)
    pub fn from_arg(arg: &str) -> Option<Self> {
        match arg.trim().to_ascii_lowercase().as_str() {
            "aggressive" => Some(Self::Aggressive),
            "balanced" => Some(Self::Balanced),
            "conservative" => Some(Self::Conservative),
            _ => None,
        }
    }
}

/// 解析 `/compact` 完整参数串:支持 `--policy <档>` 与裸档位词;
/// 空参 = 缺省 Balanced;无法识别 = None
pub fn parse_compact_args(args: &str) -> Option<CompactPolicy> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Some(CompactPolicy::Balanced);
    }
    let mut tokens = trimmed.split_whitespace();
    let first = tokens.next().unwrap_or("");
    if first == "--policy" {
        let value = tokens.next().unwrap_or("");
        return CompactPolicy::from_arg(value);
    }
    // 裸档位词(容错:额外 token 视为无效)
    if tokens.next().is_some() {
        return None;
    }
    CompactPolicy::from_arg(first)
}

/// 策展请求 — app → 管道命令信道的载荷(Concord W9 T9.3)
#[derive(Debug, Clone)]
pub struct CompactRequest {
    /// 策略档(预算乘数)
    pub policy: CompactPolicy,
    /// 策展配置(权重/预算/摘要参数,取自 TuiConfig.curation)
    pub cfg: CurationConfig,
}

// ============================================================
// T9.1 分类与打分
// ============================================================

/// 五段分类(方案 §5.3;System 本波预留恒空)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment {
    /// 系统消息(预留:当前 TUI 会话无 system 消息源)
    System,
    /// 钉选段(User 轮次,不可驱逐)
    Pinned,
    /// 近况段(末 k 轮保护窗口)
    Recent,
    /// 候选段(Assistant 历史,参与背包取舍)
    Evictable,
}

/// 打分结果 — 单条消息的分类 + 价值 + token 估算
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredMessage {
    /// 原历史下标
    pub index: usize,
    /// 分段归属
    pub segment: Segment,
    /// 价值密度(可为负;背包侧 clamp ≥0)
    pub value: f32,
    /// token 估算(字符数 / 4,最小 1)
    pub tokens: usize,
}

/// token 估算:字符数 / 4 向上取整,最小 1(空消息亦占 1)
pub fn estimate_tokens(content: &str) -> usize {
    content.chars().count().div_ceil(4).max(1)
}

/// 价值密度(方案 §5.3 公式,f32 全程不转 f64)
///
/// `value = α·recency + β·ref_count + γ·pinned − δ·token_cost`
/// - recency = (index+1)/total,线性新近性;
/// - ref_count = 消息内 `@` 字符计数(启发式引用度);
/// - pinned = Pinned 段记 1,其余 0;
/// - token_cost = tokens/100(长度惩罚归一)。
pub fn value_density(
    index: usize,
    total: usize,
    msg: &ChatMessage,
    segment: Segment,
    cfg: &CurationConfig,
) -> f32 {
    let recency = if total == 0 {
        0.0f32
    } else {
        (index + 1) as f32 / total as f32
    };
    let ref_count = msg.content.bytes().filter(|&b| b == b'@').count() as f32;
    let pinned = if segment == Segment::Pinned {
        1.0f32
    } else {
        0.0f32
    };
    let token_cost = estimate_tokens(&msg.content) as f32 / 100.0;
    cfg.alpha * recency + cfg.beta * ref_count + cfg.gamma * pinned - cfg.delta * token_cost
}

/// 五段分类 + 逐条打分(纯函数)
///
/// # 规则(诚实映射,ADR-081)
/// - `User` 角色 → `Pinned`(用户轮次不可驱逐);
/// - 末 `recent_turns × 2` 条窗口内的 Assistant → `Recent`;
/// - 其余 Assistant → `Evictable`(候选);
/// - `System` 预留恒空(当前无消息源)。
pub fn classify(messages: &[ChatMessage], cfg: &CurationConfig) -> Vec<ScoredMessage> {
    let total = messages.len();
    // Recent 窗口起点:末 2k 条(k = recent_turns);saturating 防窗口越界
    let recent_start = total.saturating_sub(cfg.recent_turns.saturating_mul(2));
    messages
        .iter()
        .enumerate()
        .map(|(index, msg)| {
            let segment = if msg.role == ChatRole::User {
                Segment::Pinned
            } else if index >= recent_start {
                Segment::Recent
            } else {
                Segment::Evictable
            };
            let value = value_density(index, total, msg, segment, cfg);
            let tokens = estimate_tokens(&msg.content);
            ScoredMessage {
                index,
                segment,
                value,
                tokens,
            }
        })
        .collect()
}

// ============================================================
// T9.2 背包求解与策略接缝
// ============================================================

/// 0-1 背包(滚动双行 dp + 决策矩阵回溯,O(n·W))
///
/// # 参数
/// - `weights`:各物品重量(token 估算)
/// - `values`:各物品价值(非负整数,×1000 缩放后的 f32)
/// - `capacity`:容量上限
///
/// # 返回
/// 与输入等长的选择向量;容量为 0 或全超重时全 false。
fn knapsack_01(weights: &[usize], values: &[u64], capacity: usize) -> Vec<bool> {
    let n = weights.len();
    let mut sel = vec![false; n];
    if n == 0 || capacity == 0 {
        return sel;
    }
    let mut prev = vec![0u64; capacity + 1];
    // 决策矩阵:take[i][c] = 物品 i 在容量 c 下是否被选中(回溯用)
    let mut take: Vec<Vec<bool>> = Vec::with_capacity(n);
    for i in 0..n {
        let mut cur = prev.clone();
        let mut took = vec![false; capacity + 1];
        let w = weights[i];
        if w <= capacity {
            for c in w..=capacity {
                let cand = prev[c - w].saturating_add(values[i]);
                if cand > cur[c] {
                    cur[c] = cand;
                    took[c] = true;
                }
            }
        }
        take.push(took);
        prev = cur;
    }
    // 回溯选择集
    let mut c = capacity;
    for i in (0..n).rev() {
        if take[i][c] {
            sel[i] = true;
            c -= weights[i];
        }
    }
    sel
}

/// 策展报告 — 回传 app 供状态栏与 /context 展示(入快照,serde)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactReport {
    /// 策展前消息数
    pub before_messages: usize,
    /// 策展后消息数(含摘要消息)
    pub after_messages: usize,
    /// 策展前 token 估算总量
    pub before_tokens: usize,
    /// 策展后 token 估算总量
    pub after_tokens: usize,
    /// 保留价值比 = (保留+摘要输入)价值 / 全量价值(clamp ≥0,f32)
    pub retained_value_ratio: f32,
    /// 本次策略档
    pub policy: CompactPolicy,
    /// 被驱逐消息数(不含进入摘要流者)
    pub evicted_count: usize,
    /// 进入摘要流的消息数
    pub summarized_count: usize,
    /// 报告序号(管道侧递增,app 检测新报告)
    pub seq: u64,
}

/// 策展计划 — 策略执行产物
#[derive(Debug, Clone, PartialEq)]
pub struct CurationPlan {
    /// 新历史(保留段原序 + 摘要消息插入 Recent 窗口前)
    pub new_messages: Vec<ChatMessage>,
    /// 被保留消息的原下标(不含插入的摘要消息;测试/遥测用,
    /// 内容匹配在重复文本下不可靠,故显式暴露下标集)
    pub kept_indices: Vec<usize>,
    /// 报告
    pub report: CompactReport,
}

/// 策展策略接缝(RL 预留:学习策略未来替换规则实现)
pub trait CurationPolicy {
    /// 对消息历史执行策展,产出新历史与报告
    fn curate(
        &self,
        messages: &[ChatMessage],
        cfg: &CurationConfig,
        policy: CompactPolicy,
    ) -> CurationPlan;
}

/// 规则式策展策略(首参实现:可解释、可测试、零训练依赖)
#[derive(Debug, Clone, Copy, Default)]
pub struct RuleCurationPolicy;

impl CurationPolicy for RuleCurationPolicy {
    fn curate(
        &self,
        messages: &[ChatMessage],
        cfg: &CurationConfig,
        policy: CompactPolicy,
    ) -> CurationPlan {
        let scored = classify(messages, cfg);
        let budget = ((cfg.budget_tokens as f32) * policy.multiplier()) as usize;

        // 强制保留段(Pinned + Recent)先扣预算
        let forced_tokens: usize = scored
            .iter()
            .filter(|s| matches!(s.segment, Segment::Pinned | Segment::Recent))
            .map(|s| s.tokens)
            .sum();
        let capacity = budget.saturating_sub(forced_tokens);

        // 候选段参与背包
        let candidates: Vec<&ScoredMessage> = scored
            .iter()
            .filter(|s| s.segment == Segment::Evictable)
            .collect();
        let weights: Vec<usize> = candidates.iter().map(|s| s.tokens).collect();
        // 价值 ×1000 整数化(负值 clamp 0:无保留价值者天然不入包)
        let values: Vec<u64> = candidates
            .iter()
            .map(|s| (s.value.max(0.0) * 1000.0) as u64)
            .collect();
        let selected = knapsack_01(&weights, &values, capacity);

        // 保留下标 = 强制段 + 入包候选,按原序重建
        let mut kept_flags = vec![false; messages.len()];
        for s in scored
            .iter()
            .filter(|s| matches!(s.segment, Segment::Pinned | Segment::Recent))
        {
            kept_flags[s.index] = true;
        }
        let mut summary_input: Vec<usize> = Vec::new();
        let mut evicted_count = 0usize;
        for (i, s) in candidates.iter().enumerate() {
            if selected[i] {
                kept_flags[s.index] = true;
            } else if s.value >= cfg.summary_value_threshold {
                summary_input.push(s.index);
            } else {
                evicted_count += 1;
            }
        }

        // 摘要消息(落选高价值段 → 抽取式)
        let summary_message = build_summary_message(messages, &summary_input, cfg);

        // 新历史:保留段原序,摘要消息插入 Recent 窗口起点前
        let mut new_messages: Vec<ChatMessage> = messages
            .iter()
            .enumerate()
            .filter(|(i, _)| kept_flags[*i])
            .map(|(_, m)| m.clone())
            .collect();
        if let Some(summary) = summary_message {
            // Recent 窗口前的保留段数量 = 插入位置
            let recent_start = messages
                .len()
                .saturating_sub(cfg.recent_turns.saturating_mul(2));
            let insert_at = kept_flags[..recent_start.min(kept_flags.len())]
                .iter()
                .filter(|&&k| k)
                .count();
            new_messages.insert(insert_at.min(new_messages.len()), summary);
        }

        let report = build_report(
            messages,
            &scored,
            &new_messages,
            &kept_flags,
            &summary_input,
            evicted_count,
            policy,
        );
        let kept_indices: Vec<usize> = kept_flags
            .iter()
            .enumerate()
            .filter(|(_, &k)| k)
            .map(|(i, _)| i)
            .collect();
        CurationPlan {
            new_messages,
            kept_indices,
            report,
        }
    }
}

/// 构建策展报告(保留价值比 = 保留+摘要输入价值 / 全量价值)
fn build_report(
    messages: &[ChatMessage],
    scored: &[ScoredMessage],
    new_messages: &[ChatMessage],
    kept_flags: &[bool],
    summary_input: &[usize],
    evicted_count: usize,
    policy: CompactPolicy,
) -> CompactReport {
    // 全量与保留价值均 clamp ≥0(负价值不拉低比值,只影响入包决策)
    let clamp = |v: f32| v.max(0.0);
    let total_value: f32 = scored.iter().map(|s| clamp(s.value)).sum();
    let mut retained_value: f32 = scored
        .iter()
        .filter(|s| kept_flags[s.index])
        .map(|s| clamp(s.value))
        .sum();
    retained_value += summary_input
        .iter()
        .filter_map(|&i| scored.get(i))
        .map(|s| clamp(s.value))
        .sum::<f32>();
    let ratio = if total_value > 0.0 {
        (retained_value / total_value).min(1.0)
    } else {
        1.0
    };
    let before_tokens: usize = messages.iter().map(|m| estimate_tokens(&m.content)).sum();
    let after_tokens: usize = new_messages
        .iter()
        .map(|m| estimate_tokens(&m.content))
        .sum();
    CompactReport {
        before_messages: messages.len(),
        after_messages: new_messages.len(),
        before_tokens,
        after_tokens,
        retained_value_ratio: ratio,
        policy,
        evicted_count,
        summarized_count: summary_input.len(),
        seq: 0, // 管道侧赋值
    }
}

// ============================================================
// T9.4 摘要接缝 — SummaryBackend / ExtractiveSummary
// ============================================================

/// 摘要后端接缝(ADR-081:主 API 异步摘要待隐藏通道就绪;
/// 当前 orchestrator 仅消费 TuiChatSubmitted 且会显示为用户轮次,
/// 不伪造通道,抽取式先行)
pub trait SummaryBackend {
    /// 将落选段文本摘要为一段正文;空输入返回 None
    fn summarize(&self, inputs: &[&str], cfg: &CurationConfig) -> Option<String>;
}

/// 抽取式摘要(纯 Rust,无模型依赖):逐条取首 N 字符合并,总量封顶
#[derive(Debug, Clone, Copy, Default)]
pub struct ExtractiveSummary;

impl SummaryBackend for ExtractiveSummary {
    fn summarize(&self, inputs: &[&str], cfg: &CurationConfig) -> Option<String> {
        if inputs.is_empty() {
            return None;
        }
        let mut out = String::new();
        for s in inputs {
            let total_chars = s.chars().count();
            let head: String = s.chars().take(cfg.summary_per_msg_chars).collect();
            out.push_str("- ");
            out.push_str(&head);
            if total_chars > cfg.summary_per_msg_chars {
                out.push('…');
            }
            out.push('\n');
            // 总量封顶:截断并标记,防止摘要本身成为新的上下文负担
            if out.chars().count() > cfg.summary_total_chars {
                let keep: String = out.chars().take(cfg.summary_total_chars).collect();
                out = keep;
                out.push('…');
                break;
            }
        }
        Some(out)
    }
}

/// 组装摘要 assistant 消息(插入新历史;无摘要输入则 None)
fn build_summary_message(
    messages: &[ChatMessage],
    summary_input: &[usize],
    cfg: &CurationConfig,
) -> Option<ChatMessage> {
    let inputs: Vec<&str> = summary_input
        .iter()
        .filter_map(|&i| messages.get(i))
        .map(|m| m.content.as_str())
        .collect();
    let backend = ExtractiveSummary;
    let body = backend.summarize(&inputs, cfg)?;
    Some(ChatMessage {
        role: ChatRole::Assistant,
        content: format!("[上下文策展摘要]\n{body}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(s: &str) -> ChatMessage {
        ChatMessage {
            role: ChatRole::User,
            content: s.into(),
        }
    }

    fn asst(s: &str) -> ChatMessage {
        ChatMessage {
            role: ChatRole::Assistant,
            content: s.into(),
        }
    }

    /// 构造交替轮次历史:n 轮(user + assistant)
    fn history(n: usize) -> Vec<ChatMessage> {
        let mut v = Vec::with_capacity(n * 2);
        for i in 0..n {
            v.push(user(&format!("question {i}")));
            v.push(asst(&format!("answer {i} with some content")));
        }
        v
    }

    // ── T9.1 分类与打分 ──────────────────────────────────

    #[test]
    fn classify_empty_history() {
        let cfg = CurationConfig::default();
        assert!(classify(&[], &cfg).is_empty());
    }

    #[test]
    fn classify_all_user_messages_are_pinned() {
        let cfg = CurationConfig::default();
        let msgs = vec![user("a"), user("b"), user("c")];
        let scored = classify(&msgs, &cfg);
        assert!(scored.iter().all(|s| s.segment == Segment::Pinned));
    }

    #[test]
    fn classify_recent_window_and_evictable() {
        let cfg = CurationConfig {
            recent_turns: 1, // 窗口 = 末 2 条
            ..Default::default()
        };
        let msgs = history(5); // 10 条
        let scored = classify(&msgs, &cfg);
        // 偶数下标(user)全 Pinned
        for s in &scored {
            if s.index % 2 == 0 {
                assert_eq!(s.segment, Segment::Pinned);
            }
        }
        // 末 2 条窗口(下标 8,9):8 为 user(Pinned),9 为 Recent
        assert_eq!(scored[9].segment, Segment::Recent);
        // 早期 assistant(下标 1,3,5)为候选
        assert_eq!(scored[1].segment, Segment::Evictable);
        assert_eq!(scored[5].segment, Segment::Evictable);
    }

    #[test]
    fn classify_recent_window_larger_than_history() {
        // 窗口越界:全部落入保护范围,无候选
        let cfg = CurationConfig {
            recent_turns: 100,
            ..Default::default()
        };
        let msgs = history(3);
        let scored = classify(&msgs, &cfg);
        assert!(scored
            .iter()
            .all(|s| s.segment == Segment::Pinned || s.segment == Segment::Recent));
    }

    #[test]
    fn value_density_components() {
        let cfg = CurationConfig {
            alpha: 1.0,
            beta: 1.0,
            gamma: 1.0,
            delta: 1.0,
            ..Default::default()
        };
        let msg = asst("@alice see this");
        let v = value_density(1, 2, &msg, Segment::Evictable, &cfg);
        // recency = 2/2 = 1.0;ref = 1;pinned = 0;
        // tokens = 15 chars/4 = 4 → cost 0.04
        let expected = 1.0f32 + 1.0 + 0.0 - 0.04;
        assert!((v - expected).abs() < 1e-6, "got {v}, want {expected}");
    }

    #[test]
    fn estimate_tokens_minimum_one() {
        assert_eq!(estimate_tokens(""), 1, "空消息最小 1");
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2, "向上取整");
    }

    // ── T9.2 背包与策略 ──────────────────────────────────

    #[test]
    fn knapsack_small_instance_exact() {
        // 容量 10:物品 (w=6,v=10) (w=5,v=8) (w=5,v=8) → 选后两者 v=16
        let sel = knapsack_01(&[6, 5, 5], &[10, 8, 8], 10);
        assert_eq!(sel, vec![false, true, true]);
    }

    #[test]
    fn knapsack_zero_capacity_selects_nothing() {
        let sel = knapsack_01(&[1, 2], &[5, 5], 0);
        assert_eq!(sel, vec![false, false]);
    }

    #[test]
    fn knapsack_overweight_item_skipped() {
        let sel = knapsack_01(&[20, 3], &[100, 10], 10);
        assert_eq!(sel, vec![false, true]);
    }

    #[test]
    fn curate_preserves_user_and_recent() {
        let cfg = CurationConfig {
            budget_tokens: 100_000, // 大预算:候选全保留
            ..Default::default()
        };
        let msgs = history(6);
        let plan = RuleCurationPolicy.curate(&msgs, &cfg, CompactPolicy::Balanced);
        // 大预算下零驱逐(摘要消息不插入:无落选段)
        assert_eq!(plan.new_messages.len(), msgs.len());
        assert_eq!(plan.report.evicted_count, 0);
        assert_eq!(plan.report.retained_value_ratio, 1.0);
    }

    #[test]
    fn curate_tight_budget_evicts_low_value_first() {
        let cfg = CurationConfig {
            budget_tokens: 40, // 紧预算
            recent_turns: 1,
            alpha: 1.0,
            beta: 0.0,
            gamma: 0.5,
            delta: 0.2,
            ..Default::default()
        };
        let msgs = history(6); // 12 条
        let plan = RuleCurationPolicy.curate(&msgs, &cfg, CompactPolicy::Conservative);
        // user 消息全部保留(Pinned 语义)
        let user_kept = plan
            .new_messages
            .iter()
            .filter(|m| m.role == ChatRole::User)
            .count();
        assert_eq!(user_kept, 6, "用户轮次不可驱逐");
        assert!(plan.report.after_messages < msgs.len(), "紧预算应发生驱逐");
        // 保留价值比合法域
        assert!((0.0..=1.0).contains(&plan.report.retained_value_ratio));
    }

    #[test]
    fn policy_multiplier_scales_budget() {
        assert_eq!(CompactPolicy::Aggressive.multiplier(), 0.5);
        assert_eq!(CompactPolicy::Balanced.multiplier(), 0.75);
        assert_eq!(CompactPolicy::Conservative.multiplier(), 1.0);
    }

    #[test]
    fn parse_compact_args_variants() {
        assert_eq!(parse_compact_args(""), Some(CompactPolicy::Balanced));
        assert_eq!(
            parse_compact_args("--policy aggressive"),
            Some(CompactPolicy::Aggressive)
        );
        assert_eq!(
            parse_compact_args("conservative"),
            Some(CompactPolicy::Conservative)
        );
        assert_eq!(parse_compact_args("--policy turbo"), None);
        assert_eq!(
            parse_compact_args("balanced extra"),
            None,
            "多余 token 非法"
        );
        assert_eq!(parse_compact_args("--policy"), None, "缺值非法");
    }

    // ── 验收验证集:保留价值 ≥0.85(默认权重)──────────────

    #[test]
    fn validation_corpus_retains_at_least_85_percent() {
        // 固定语料:60 轮交替历史 + 变化长度(模拟真实会话)
        let mut msgs = Vec::new();
        for i in 0..60 {
            msgs.push(user(&format!("task request {i}")));
            let filler = "x".repeat(40 + (i % 5) * 20);
            msgs.push(asst(&format!("detailed response {i}: {filler}")));
        }
        let cfg = CurationConfig::default(); // budget 4096, balanced ×0.75
        let plan = RuleCurationPolicy.curate(&msgs, &cfg, CompactPolicy::Balanced);
        assert!(
            plan.report.retained_value_ratio >= 0.85,
            "保留价值比 {} 低于 0.85 验收线(R13 触发条件)",
            plan.report.retained_value_ratio
        );
        assert!(
            plan.report.after_tokens <= cfg.budget_tokens + 200,
            "策展后 token {} 应大致落入预算 {} 内(摘要与 Pinned 段允许少量溢出)",
            plan.report.after_tokens,
            cfg.budget_tokens
        );
    }

    // ── T9.4 摘要接缝 ────────────────────────────────────

    #[test]
    fn extractive_summary_empty_input() {
        let cfg = CurationConfig::default();
        assert_eq!(ExtractiveSummary.summarize(&[], &cfg), None);
    }

    #[test]
    fn extractive_summary_per_message_cap_and_truncation() {
        let cfg = CurationConfig {
            summary_per_msg_chars: 10,
            summary_total_chars: 1000,
            ..Default::default()
        };
        let long = "y".repeat(50);
        let out = ExtractiveSummary
            .summarize(&[&long], &cfg)
            .expect("非空输入");
        assert!(out.contains("yyyyyyyyyy…"), "超长条目应截断标记");
        assert!(!out.contains(&"y".repeat(11)), "不得超出单条上限");
    }

    #[test]
    fn extractive_summary_total_cap() {
        let cfg = CurationConfig {
            summary_per_msg_chars: 80,
            summary_total_chars: 30,
            ..Default::default()
        };
        let inputs: Vec<String> = (0..5)
            .map(|i| format!("message number {i} content"))
            .collect();
        let refs: Vec<&str> = inputs.iter().map(|s| s.as_str()).collect();
        let out = ExtractiveSummary.summarize(&refs, &cfg).expect("非空输入");
        assert!(
            out.chars().count() <= 32,
            "总量封顶后应 ≈ summary_total_chars(含截断标记)"
        );
    }

    #[test]
    fn curate_inserts_summary_message_for_valuable_evicted() {
        // 构造:紧预算 + 高价值落选段(多处 @ 提升价值过阈值)
        let cfg = CurationConfig {
            budget_tokens: 30,
            recent_turns: 1,
            alpha: 1.0,
            beta: 1.0,
            gamma: 0.5,
            delta: 0.1,
            summary_value_threshold: 0.05,
            ..Default::default()
        };
        let mut msgs = Vec::new();
        for i in 0..8 {
            msgs.push(user(&format!("q{i}")));
            msgs.push(asst(&format!("@reviewer result {i} @owner follow up")));
        }
        let plan = RuleCurationPolicy.curate(&msgs, &cfg, CompactPolicy::Conservative);
        assert!(plan.report.summarized_count > 0, "应有段落进入摘要流");
        let has_summary = plan
            .new_messages
            .iter()
            .any(|m| m.content.starts_with("[上下文策展摘要]"));
        assert!(has_summary, "摘要消息应插入新历史");
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    fn arb_messages(max_len: usize) -> impl Strategy<Value = Vec<ChatMessage>> {
        proptest::collection::vec(
            (any::<bool>(), "[a-z@ ]{0,40}").prop_map(|(is_user, text)| ChatMessage {
                role: if is_user {
                    ChatRole::User
                } else {
                    ChatRole::Assistant
                },
                content: text,
            }),
            0..max_len,
        )
    }

    proptest! {
        /// 不变量①:策展后候选段保留部分的 token 总量 ≤ 背包剩余容量
        /// (强制段不受背包约束,单独断言候选段;经 kept_indices 精确判定)
        #[test]
        fn candidate_retention_within_budget(msgs in arb_messages(40)) {
            let cfg = CurationConfig { budget_tokens: 64, recent_turns: 2, ..Default::default() };
            let scored = classify(&msgs, &cfg);
            let plan = RuleCurationPolicy.curate(&msgs, &cfg, CompactPolicy::Conservative);
            let forced: usize = scored.iter()
                .filter(|s| matches!(s.segment, Segment::Pinned | Segment::Recent))
                .map(|s| s.tokens).sum();
            let capacity = cfg.budget_tokens.saturating_sub(forced);
            let kept_candidate_tokens: usize = scored.iter()
                .filter(|s| s.segment == Segment::Evictable && plan.kept_indices.contains(&s.index))
                .map(|s| s.tokens).sum();
            prop_assert!(
                kept_candidate_tokens <= capacity,
                "候选保留 {} 超剩余容量 {}", kept_candidate_tokens, capacity
            );
        }

        /// 不变量②:预算单调性 — 预算 ↑ ⇒ 保留价值非减
        #[test]
        fn retained_value_monotonic_in_budget(msgs in arb_messages(30)) {
            let small = CurationConfig { budget_tokens: 32, ..Default::default() };
            let large = CurationConfig { budget_tokens: 4096, ..Default::default() };
            let r_small = RuleCurationPolicy.curate(&msgs, &small, CompactPolicy::Conservative);
            let r_large = RuleCurationPolicy.curate(&msgs, &large, CompactPolicy::Conservative);
            prop_assert!(
                r_large.report.retained_value_ratio + 1e-6
                    >= r_small.report.retained_value_ratio,
                "大预算保留比 {} 不应低于小预算 {}",
                r_large.report.retained_value_ratio,
                r_small.report.retained_value_ratio
            );
        }

        /// 不变量③:分类全覆盖零丢失 — 每条消息恰属一段(System 预留恒空),
        /// 且候选 = 保留 + 摘要输入 + 驱逐 三类互斥覆盖
        #[test]
        fn classification_covers_all(msgs in arb_messages(30)) {
            let cfg = CurationConfig::default();
            let scored = classify(&msgs, &cfg);
            prop_assert_eq!(scored.len(), msgs.len());
            for (i, s) in scored.iter().enumerate() {
                prop_assert_eq!(s.index, i);
                prop_assert!(s.segment != Segment::System, "当前无 System 消息源(预留段)");
            }
            let plan = RuleCurationPolicy.curate(&msgs, &cfg, CompactPolicy::Balanced);
            // 候选守恒:保留候选 + 摘要输入 + 驱逐 == 候选总数
            let candidates = scored.iter().filter(|s| s.segment == Segment::Evictable).count();
            let kept_candidates = scored.iter()
                .filter(|s| s.segment == Segment::Evictable && plan.kept_indices.contains(&s.index))
                .count();
            prop_assert_eq!(
                kept_candidates + plan.report.summarized_count + plan.report.evicted_count,
                candidates,
                "候选段三类分流必须无丢失无重复"
            );
            // 新历史长度 = 保留数 + 摘要消息(0/1)
            let summary_extra = if plan.new_messages.iter().any(|m| m.content.starts_with("[上下文策展摘要]")) { 1 } else { 0 };
            prop_assert_eq!(plan.new_messages.len(), plan.kept_indices.len() + summary_extra);
        }
    }
}
