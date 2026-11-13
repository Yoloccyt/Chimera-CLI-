//! 统一上下文装配内核（R9 / 上下文装配权威）
//!
//! # 职责
//! 为所有上下文入口提供**唯一**的去重与预算账本：
//! - 精确重复去重（`role + source + content` 三元组逐字相同）；
//! - 单一预算 = `min(contract.max_tokens, tier_capacity)`；
//! - 确定性驱逐：可驱逐段 → 近期段 → 钉选段，同级按价值密度升序、`seq` 降序淘汰；
//! - 输出保持输入相对顺序（`seq` 单调不减是调用方前置条件，装配不改写）。
//!
//! # 为什么放在 L2 而不是 L0
//! L0 只承载契约（[`ContextAssemblyContract`]）。把执行内核放 L2 可避免新增 L0
//! 公开面触发契约棘轮门（G-105/G-107/G-115/G-116 等），依赖方向 L2 → L0 合法。
//!
//! # 零差异保证
//! 当输入不含精确重复且预算未成为约束时，输出 `items` 与输入**逐字段等价**
//! （顺序与内容均不变）。这是「功能不削减」的硬约束，由单测与 property test 锁定。
//!
//! # 不做的事
//! - 不调用模型、不做摘要、不触发记忆合成（`on_demand_synthesis` 仅作为诊断字段透出）；
//! - 不修改调用方的 `seq`，不做隐式排序（避免改变既有输出字节）。

use std::collections::HashMap;

use nexus_contracts::spec::harness_dimensions::ContextAssemblyContract;
use serde::{Deserialize, Serialize};

/// 近期段保护窗口（条目数）
///
/// WHY 4：与 TUI 策展器 `CurationConfig::recent_turns` 的缺省值 4 对齐；
/// 装配内核按条目而非轮次计数，避免引入对话轮次解析。
pub const RECENT_WINDOW_ITEMS: usize = 4;

/// 上下文角色（装配面最小角色集）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContextRole {
    /// 系统指令
    System,
    /// 用户输入
    User,
    /// 助手输出
    Assistant,
    /// 工具结果
    Tool,
}

/// 上下文来源（用于去重键与价值密度）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContextSource {
    /// 系统提示
    System,
    /// 用户消息
    User,
    /// 助手消息
    Assistant,
    /// 记忆检索
    Memory,
    /// 工具输出
    Tool,
    /// 知识检索
    Retrieval,
}

/// 单条上下文候选
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextItem {
    /// 角色
    pub role: ContextRole,
    /// 来源
    pub source: ContextSource,
    /// 文本内容
    pub content: String,
    /// 是否钉选（最强保护：仅在预算仍超限时才被淘汰）
    pub pinned: bool,
    /// 稳定序（调用方提供；装配不改写，只要求输入非递减）
    pub seq: u64,
}

/// 装配诊断快照（有限取值域，无高基数标签）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssemblyDiagnostics {
    /// 被去重的条数
    pub deduped: usize,
    /// 被预算驱逐的条数
    pub evicted: usize,
    /// 生效预算（token）
    pub budget_tokens: usize,
    /// 装配后 token 估算
    pub used_tokens: usize,
    /// 契约是否要求按需记忆合成（仅透出，不在本内核执行）
    pub synthesis_requested: bool,
}

/// 装配结果
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssembledContext {
    /// 装配后的条目（保持输入相对顺序）
    pub items: Vec<ContextItem>,
    /// 诊断
    pub diagnostics: AssemblyDiagnostics,
}

/// token 估算：字符数 / 4 向上取整，最小 1
///
/// WHY 与 TUI 策展器 `data::curator::estimate_tokens` 同公式：统一预算账本要求
/// 两处口径一致，否则同一消息在两个入口计费不同。
pub fn estimate_tokens(content: &str) -> usize {
    content.chars().count().div_ceil(4).max(1)
}

/// 装配上下文（纯函数，确定性）
///
/// # 契约
/// 1. 空输入 ⇒ 空输出，不 panic；
/// 2. 去重键 = `(role, source, content)` 逐字相同；保留最先出现者；
/// 3. 预算 = `min(contract.max_tokens, tier_capacity)`；`tier_capacity == 0` 视为未配置，
///    只用 `contract.max_tokens`；
/// 4. 驱逐顺序：可驱逐段（非钉选且非末 [`RECENT_WINDOW_ITEMS`] 条）→ 近期段 → 钉选段；
///    同级按价值密度升序、`seq` 降序（先淘汰价值低且更晚出现者）；
/// 5. 输出保持输入相对顺序；不改写 `seq`。
///
/// # 诊断
/// `diagnostics.deduped` 统计去重条数；`evicted` 统计预算驱逐条数；
/// `used_tokens` 为输出实际 token 估算。
pub fn assemble_context(
    items: Vec<ContextItem>,
    contract: &ContextAssemblyContract,
    tier_capacity: usize,
) -> AssembledContext {
    if items.is_empty() {
        return AssembledContext {
            items: Vec::new(),
            diagnostics: AssemblyDiagnostics {
                deduped: 0,
                evicted: 0,
                budget_tokens: effective_budget(contract, tier_capacity),
                used_tokens: 0,
                synthesis_requested: contract.on_demand_synthesis,
            },
        };
    }

    // ---- 1) 精确重复去重（哈希分桶 + 内容逐字复核，避免哈希碰撞误删）----
    let mut buckets: HashMap<(u8, u8, u64), Vec<usize>> = HashMap::new();
    let mut keep = vec![true; items.len()];
    for (index, item) in items.iter().enumerate() {
        let key = (
            role_code(item.role),
            source_code(item.source),
            fnv1a_64(item.content.as_bytes()),
        );
        let bucket = buckets.entry(key).or_default();
        if bucket
            .iter()
            .any(|&prev| items[prev].content == item.content)
        {
            keep[index] = false;
        }
        bucket.push(index);
    }
    let deduped: Vec<ContextItem> = items
        .into_iter()
        .zip(keep.iter())
        .filter(|(_, &k)| k)
        .map(|(item, _)| item)
        .collect();
    let deduped_count = keep.iter().filter(|&&k| !k).count();

    // ---- 2) 预算与驱逐 ----
    let budget = effective_budget(contract, tier_capacity);
    let total_tokens: usize = deduped.iter().map(|i| estimate_tokens(&i.content)).sum();

    let mut retained_flags = vec![true; deduped.len()];
    let mut used = total_tokens;
    let mut evicted = 0usize;
    if total_tokens > budget {
        // 计算保留优先级：group 越小越先被淘汰；密度越低越先被淘汰；seq 越大越先被淘汰。
        let recent_start = deduped.len().saturating_sub(RECENT_WINDOW_ITEMS);
        let mut order: Vec<usize> = (0..deduped.len()).collect();
        order.sort_by(|&a, &b| {
            let ga = protection_group(&deduped[a], a, recent_start);
            let gb = protection_group(&deduped[b], b, recent_start);
            ga.cmp(&gb)
                .then_with(|| {
                    let da = density(&deduped[a], a, deduped.len());
                    let db = density(&deduped[b], b, deduped.len());
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| deduped[b].seq.cmp(&deduped[a].seq))
        });
        for &index in &order {
            if used <= budget {
                break;
            }
            if retained_flags[index] {
                retained_flags[index] = false;
                used = used.saturating_sub(estimate_tokens(&deduped[index].content));
                evicted += 1;
            }
        }
    }

    let assembled: Vec<ContextItem> = deduped
        .into_iter()
        .zip(retained_flags.iter())
        .filter(|(_, &k)| k)
        .map(|(item, _)| item)
        .collect();

    // R9: 诊断面（有限取值域，无高基数标签；默认日志级别下零输出）
    tracing::debug!(
        deduped = deduped_count,
        evicted,
        budget_tokens = budget,
        used_tokens = used,
        "context assembly diagnostics"
    );

    AssembledContext {
        items: assembled,
        diagnostics: AssemblyDiagnostics {
            deduped: deduped_count,
            evicted,
            budget_tokens: budget,
            used_tokens: used,
            synthesis_requested: contract.on_demand_synthesis,
        },
    }
}

/// 生效预算 = `min(contract.max_tokens, tier_capacity)`；tier 未配置（0）时只用契约值
fn effective_budget(contract: &ContextAssemblyContract, tier_capacity: usize) -> usize {
    if tier_capacity == 0 {
        contract.max_tokens
    } else {
        contract.max_tokens.min(tier_capacity)
    }
}

/// 保护分组：0 = 可驱逐，1 = 近期段，2 = 钉选段（越大越晚被淘汰）
fn protection_group(item: &ContextItem, index: usize, recent_start: usize) -> u8 {
    if item.pinned {
        2
    } else if index >= recent_start {
        1
    } else {
        0
    }
}

/// 来源价值系数 — 单一真值源（R9b WP-D 归一：compressor w4 与本模块 density 共用）
///
/// WHY 提取:原为 `density` 私有 match 表;w4（ADR-293）的压缩评分需要同一套
/// 来源价值语义（Tool 降权 = 蓝图 §5.4「tool-response 类条目降权」的
/// AKV Rolling Drop 内容层转译），两处各写一份 match 会产生真值源分叉漂移。
/// 数值沿用吸收成果（另一会话 R9 W2 波次）的既有标定。
pub fn source_value_factor(source: ContextSource) -> f32 {
    match source {
        ContextSource::System => 1.0_f32,
        ContextSource::User => 1.0_f32,
        ContextSource::Assistant => 0.5_f32,
        ContextSource::Memory => 0.8_f32,
        ContextSource::Tool => 0.3_f32,
        ContextSource::Retrieval => 0.7_f32,
    }
}

/// 价值密度（全部 f32，禁止中途转 f64 —— 项目红线 §4.4 #6）
fn density(item: &ContextItem, index: usize, total: usize) -> f32 {
    let role_weight = match item.role {
        ContextRole::System => 2.0_f32,
        ContextRole::User => 1.5_f32,
        ContextRole::Assistant => 1.0_f32,
        ContextRole::Tool => 0.5_f32,
    };
    // WP-D:经 source_value_factor 单点（语义与数值逐位一致，纯提取重构）
    let source_weight = source_value_factor(item.source);
    let recency = if total == 0 {
        0.0_f32
    } else {
        (index + 1) as f32 / total as f32
    };
    let pinned_bonus = if item.pinned { 3.0_f32 } else { 0.0_f32 };
    role_weight + source_weight + recency + pinned_bonus
}

fn role_code(role: ContextRole) -> u8 {
    match role {
        ContextRole::System => 0,
        ContextRole::User => 1,
        ContextRole::Assistant => 2,
        ContextRole::Tool => 3,
    }
}

fn source_code(source: ContextSource) -> u8 {
    match source {
        ContextSource::System => 0,
        ContextSource::User => 1,
        ContextSource::Assistant => 2,
        ContextSource::Memory => 3,
        ContextSource::Tool => 4,
        ContextSource::Retrieval => 5,
    }
}

/// FNV-1a 64 位（稳定跨进程/跨运行，仅用于分桶；碰撞由内容逐字复核兜底）
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
