//! 统一上下文装配内核（R9 / 上下文装配权威）
//!
//! # 职责
//! 为所有上下文入口提供**唯一**的去重与预算账本：
//! - 两档去重：精确档（`role + source + content` 三元组逐字相同）+ 规范化档
//!   （空白折叠 + ASCII 大小写归一后相同，R11 W4a；`CHIMERA_DEDUP_NORMALIZE=off`
//!   回精确档）；
//! - 单一预算 = `min(contract.max_tokens, tier_capacity)`；
//! - 确定性驱逐：可驱逐段 → 近期段 → 钉选段，同级按价值密度升序、`seq` 降序淘汰；
//! - 输出保持输入相对顺序（`seq` 单调不减是调用方前置条件，装配不改写）。
//!
//! # 为什么放在 L2 而不是 L0
//! L0 只承载契约（[`ContextAssemblyContract`]）。把执行内核放 L2 可避免新增 L0
//! 公开面触发契约棘轮门（G-105/G-107/G-115/G-116 等），依赖方向 L2 → L0 合法。
//!
//! # 零差异保证
//! 当输入不含（精确或规范化）重复且预算未成为约束时，输出 `items` 与输入
//! **逐字段等价**（顺序与内容均不变）。这是「功能不削减」的硬约束，由单测
//! 与 property test 锁定。规范化档开启时，仅空白/ASCII 大小写差异的条目会
//! 被折叠——属冗余治理语义（ADR-295 D7），非行为回归。
//!
//! # 不做的事
//! - 不调用模型、不做摘要、不触发记忆合成（`on_demand_synthesis` 仅作为诊断字段透出）；
//! - 不修改调用方的 `seq`，不做隐式排序（避免改变既有输出字节）。
//!
//! # R12 W1-2：token 估算可注入（单一真值源）
//!
//! [`assemble_context`] 内部所有 token 计量（预算累计、驱逐扣减、`used_tokens`
//! 统计）走的是**一个可注入的估算器**：精确入口
//! [`assemble_context_with`] 接受 `&dyn Fn(&str) -> usize`，而 [`assemble_context`]
//! 只是「以默认估算器 [`estimate_tokens`] 委托」的薄壳（签名与行为逐字不变）。
//!
//! ## WHY 用注入而不是把 `estimate_tokens` 改成调用方的公式
//!
//! 这是本波次最容易被"顺手优化"掉的一处，故把理由写死在这里：
//!
//! 1. **改常量 = 改内核语义**。`estimate_tokens`（字符数/4 向上取整、最小 1）不是
//!    内核的私有细节，而是它与 TUI 策展器 `data::curator::estimate_tokens` 共享的
//!    口径。把它换成"UTF-8 字节宽/4 floor"，会让既有 17 处 `assembly_contract`
//!    断言（含 proptest 的 `no_duplicates ⇒ identity` 与驱逐条数断言）全部换一次
//!    语义基座 —— 那不是修 bug，是**重新定标**，必须走独立裁决 + 影子期。
//! 2. **改常量会反转依赖方向**。内核在 L2，`mca-gateway` 在 L10。若内核直接采用
//!    mca 的计量（字节宽/4 floor），等于让 L2 的一处纯函数语义取决于 L10 的账本
//!    偏好 —— 依赖铁律里最贵的那种耦合（下游口径向上渗透进内核默认值）。注入则
//!    把选择权留在调用点：默认值仍属于 L2，L10 只是**自己**选择用哪把尺子。
//! 3. **改常量无法容纳第三个调用方**。TUI 与 mca 的口径本就不必相同（前者对齐
//!    字符/4，后者显式化字节宽并做 EWMA 校准）。一个常量只能服务一个口径，注入面
//!    可服务 N 个；而"两个口径变成一份共享常量"正是双账本的来源。
//!
//! ## 「单一账本」如何被结构（而非约定）保证
//!
//! 函数体内**只有一个**估算调用点：`per_item_tokens: Vec<usize>`。预算累计
//! （`total_tokens`）是它的求和，驱逐扣减读 `per_item_tokens[index]`，
//! `used_tokens` 就是驱逐后的累计值。三者共用同一份向量、同一把尺子，
//! 于是"预算用 A 口径、统计用 B 口径"的半统一**在本函数里不可表达**：
//! 任何想把某处改回 [`estimate_tokens`] 的改动，要么让该处重新变成第二个调用点
//! （`tests/estimator_injection.rs::all_estimations_use_injected` 立刻红），
//! 要么就绕不开那条唯一的 `estimate` 引用。
//!
//! 已知边界（本波次**不**改变的事）：`effective_budget`（`min(max_tokens,
//! tier_capacity)`）与 `density`（角色/来源权重 + 时近性 + 钉选加成）**都不做
//! token 计量** —— 前者只做整数取小，后者是"价值"而非"价值/长度"密度。二者因此
//! 不存在需要注入的估算点（`density` 的命名易被误读为 `tokens/长度`，实则不含
//! token 项，见 `tests/estimator_injection.rs` 的口径固化断言）。

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
    /// 被去重的条数（精确档 + 规范化档合计）
    pub deduped: usize,
    /// 其中经规范化判据（空白折叠 + ASCII 大小写归一）折叠的条数（R11 W4a）
    ///
    /// 精确档计数 = `deduped - deduped_normalized`。`#[serde(default)]`：诊断
    /// 快照非持久契约，旧载荷反序列化兼容为防御性默认。
    #[serde(default)]
    pub deduped_normalized: usize,
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

/// 默认 token 估算器：字符数 / 4 向上取整，最小 1
///
/// WHY 与 TUI 策展器 `data::curator::estimate_tokens` 同公式：统一预算账本要求
/// 两处口径一致，否则同一消息在两个入口计费不同。
///
/// # 本函数的**值**是冻结的（R12 W1-2）
/// 它是 [`assemble_context`] 的默认估算器，被既有 17 处 `assembly_contract`
/// 断言（去重条数、驱逐条数、`budget_tokens`、proptest 的"无重复 ⇒ 逐字段等价"）
/// 与 mca 侧接线对照测试一起按**具体数字**锁定。要换成别的口径（例如 mca 的
/// 字节宽/4 floor）请走 [`assemble_context_with`] 在**调用点**注入 ——
/// 不要改这里的公式，那会一次性重定标全部既有断言（理由详见模块文档
/// 「R12 W1-2」节）。
pub fn estimate_tokens(content: &str) -> usize {
    content.chars().count().div_ceil(4).max(1)
}

/// 装配上下文（纯函数，确定性）—— 默认估算器入口
///
/// **签名与行为完全不变**（R12 W1-2 的硬约束）：本函数是
/// `assemble_context_with(items, contract, tier_capacity, &estimate_tokens)`
/// 的薄壳，一字不差的委托 —— 不存在"新老两条实现"，故不存在二者漂移的可能。
///
/// 需要别的计量口径（如 `mca-gateway` 的 UTF-8 字节宽/4）请用
/// [`assemble_context_with`]；本函数保留给"默认口径即可"的调用方。
///
/// # 契约
/// 1. 空输入 ⇒ 空输出，不 panic；
/// 2. 两档去重，均保留最先出现者：精确键 = `(role, source, content)` 逐字
///    相同；规范化键 = 同三元组经空白折叠 + ASCII 大小写归一后相同
///    （`CHIMERA_DEDUP_NORMALIZE=off` 关闭规范化档）；
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
    assemble_context_with(items, contract, tier_capacity, &estimate_tokens)
}

/// 装配上下文（纯函数，确定性）—— **可注入估算器**入口（R12 W1-2）
///
/// 与 [`assemble_context`] 的唯一差别：token 计量用调用方给的 `estimate`，
/// 而不是内核默认的 [`estimate_tokens`]。其余（去重、预算取小、驱逐顺序、
/// 输出相对顺序、诊断字段语义）逐字相同 —— 两者共用一个函数体。
///
/// # 参数
/// - `estimate`: `content -> token 数` 的纯函数。**内核内的唯一估算真值源**：
///   预算累计、驱逐扣减、`used_tokens` 全部只经它（见"单一账本"小节）。
/// - 其余参数同 [`assemble_context`]。
///
/// # 调用方须知的边界
/// 估算器看到的是 [`ContextItem::content`] —— 即调用方交给内核的那份**字符串
/// 表示**，不是它自己的领域对象（例如 `mca-gateway` 传的是去重键用的扁平化
/// 文本）。故"同一把尺子"保证的是**计量函数一致**，不保证"和调用方按块算出的
/// 总量逐 token 相等"。启用内核驱逐前必须把"表示层"也对齐（详见报告
/// `docs/reports/r12-w1-2-estimator.md` 的已知边界）。
///
/// # 单一账本（结构保证，非约定）
/// 函数体内只有一个估算调用点（`per_item_tokens`），预算累计与 `used_tokens`
/// 都读它 ⇒ "预算用 A 口径、统计用 B 口径"在此不可表达。注入恒定值估算器时
/// `used_tokens == 保留条数 × 值` 恒成立（`tests/estimator_injection.rs`）。
///
/// # 契约 / 诊断
/// 与 [`assemble_context`] 完全一致（除计量口径由 `estimate` 决定外）。
pub fn assemble_context_with(
    items: Vec<ContextItem>,
    contract: &ContextAssemblyContract,
    tier_capacity: usize,
    estimate: &dyn Fn(&str) -> usize,
) -> AssembledContext {
    if items.is_empty() {
        return AssembledContext {
            items: Vec::new(),
            diagnostics: AssemblyDiagnostics {
                deduped: 0,
                deduped_normalized: 0,
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

    // ---- 1b) 规范化近重复去重（R11 W4a / ADR-295 D7）----
    // 键 = (role, source, 规范化内容哈希)；复核 = 规范化逐词等价（空白折叠 +
    // ASCII 大小写归一）。捕获面：工具输出/回显的空白与大小写漂移——真实
    // 冗余的主要形态。保守方向：Unicode 大小写/变体**不折叠** ⇒ 少合并 =
    // 不误删（宁漏勿错的审计纪律与精确档一致）。保留最先出现者（与精确档
    // 同语义）。逃生：`CHIMERA_DEDUP_NORMALIZE=off` 回精确档（启动期一次读取，
    // 禁热路径运行时开关红线）。
    let (deduped, deduped_normalized) = if normalized_dedup_enabled() {
        let mut nbuckets: HashMap<(u8, u8, u64), Vec<usize>> = HashMap::new();
        let mut nkeep = vec![true; deduped.len()];
        for (index, item) in deduped.iter().enumerate() {
            let key = (
                role_code(item.role),
                source_code(item.source),
                normalized_fnv1a_64(&item.content),
            );
            let bucket = nbuckets.entry(key).or_default();
            if bucket
                .iter()
                .any(|&prev| normalized_eq(&deduped[prev].content, &item.content))
            {
                nkeep[index] = false;
            }
            bucket.push(index);
        }
        let folded = nkeep.iter().filter(|&&k| !k).count();
        let filtered: Vec<ContextItem> = deduped
            .into_iter()
            .zip(nkeep.iter())
            .filter(|(_, &k)| k)
            .map(|(item, _)| item)
            .collect();
        (filtered, folded)
    } else {
        (deduped, 0)
    };
    let deduped_count = deduped_count + deduped_normalized;

    // ---- 2) 预算与驱逐 ----
    let budget = effective_budget(contract, tier_capacity);
    // **本函数唯一的估算调用点**（R12 W1-2 单一账本的结构支点）：
    // 每条（去重后的）条目精确调用注入估算器一次；预算累计 `total_tokens`
    // 是这份向量的求和，驱逐扣减读 `per_item_tokens[index]`，`used_tokens`
    // 是驱逐后的累计值。三者共用同一份数字 ⇒ 不存在"第二个估算点"，
    // 也就无法出现"预算用 A 口径、统计用 B 口径"的半统一。
    //
    // WHY 缓存进 `Vec` 而不是两处各自调用 `estimate(&content)`：
    // ① 使调用次数与条目数解耦（驱逐循环里再估一次是 O(驱逐数) 次多余调用）；
    // ② 让"某一处漏改"无法隐藏在同一份数字里（只有这一份可减，另一处若改回
    //    默认估算器就会与它分叉 ⇒ 被 `tests/estimator_injection.rs` 的
    //    `all_estimations_use_injected` 直接打红，而不是靠读者肉眼比对）。
    // 代价:一次 O(n) 的 usize 分配（n = 去重后条目数，装配面本来就是 O(n)）。
    let per_item_tokens: Vec<usize> = deduped.iter().map(|item| estimate(&item.content)).collect();
    let total_tokens: usize = per_item_tokens.iter().copied().sum();

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
                // 读同一份 `per_item_tokens`：与预算累计同源同口径（见上）
                used = used.saturating_sub(per_item_tokens[index]);
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
            deduped_normalized,
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

/// FNV 单字节推进（规范化哈希复用同一素数/偏移基）
fn fnv_byte(hash: u64, byte: u8) -> u64 {
    (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
}

/// 规范化内容哈希（R11 W4a）：空白序列折叠为单空格 + ASCII 大小写归一后 FNV。
///
/// WHY 流式哈希而非先物化规范化字符串：省一次堆分配（装配面每条目一次，
/// 预算驱逐前就要算）；词间分隔符显式哈希进流，保证 `"a  b"` 与 `"a b"` 同键。
fn normalized_fnv1a_64(content: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut first_word = true;
    for word in content.split_whitespace() {
        if !first_word {
            hash = fnv_byte(hash, b' ');
        }
        first_word = false;
        for &byte in word.as_bytes() {
            hash = fnv_byte(hash, byte.to_ascii_lowercase());
        }
    }
    hash
}

/// 规范化逐词等价复核（与 [`normalized_fnv1a_64`] 同判据：空白折叠 + ASCII
/// 大小写归一）——哈希碰撞兜底，与精确档「逐字复核」同纪律。
fn normalized_eq(a: &str, b: &str) -> bool {
    let mut aw = a.split_whitespace();
    let mut bw = b.split_whitespace();
    loop {
        match (aw.next(), bw.next()) {
            (None, None) => return true,
            (Some(x), Some(y)) => {
                if !x.eq_ignore_ascii_case(y) {
                    return false;
                }
            }
            _ => return false,
        }
    }
}

/// 规范化去重开关（启动期一次读取；`CHIMERA_DEDUP_NORMALIZE=off` 回精确档）
///
/// WHY OnceLock：禁热路径运行时开关红线（与 mlc `hnsw_tier_enabled_by_env`
/// 同款）；默认开启（W4a 是冗余治理的主体交付）。
fn normalized_dedup_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("CHIMERA_DEDUP_NORMALIZE")
            .map(|v| !v.eq_ignore_ascii_case("off"))
            .unwrap_or(true)
    })
}
