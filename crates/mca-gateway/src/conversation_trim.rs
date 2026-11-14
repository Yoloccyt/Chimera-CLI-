//! conversation 预算裁剪 — R4 上下文预算与动态裁剪(ADR-069 Token 效率优化)
//!
//! 对应架构层: L10 Interface(mca-gateway)
//!
//! # 与 hcw-window::trim_to_budget 的关系
//! 同源语义:预算驱动 + 重要性排序。hcw 版操作 `ContextEntry`(file_id/
//! access_count/时间戳),其语义与 `AffinityMessage`(角色/内容块)不匹配,
//! 故在 mca-gateway 内实现针对性 conversation 裁剪纯函数,避免
//! `ContextEntry` 类型泄漏到 L10。
//!
//! # 重要性评分
//! - 工具返回(Tool 消息):最高权重,模型下一步决策的依据,恒保留
//! - 用户输入(User):次高,承载最新意图
//! - 历史(Assistant/System):最低,裁剪优先
//! - 时近性:最后一条消息恒保留(保尾段);同权重内新消息优先保留
//!
//! # Top-K 实现(红线)
//! 预算约束的 Top-K 用 `select_nth_unstable_by`(O(n) 部分排序),
//! 禁止 `sort_by` 全排序。
//!
//! # token 估算口径
//! 字符/4 启发式,与 `adapters::estimate_cost` 同口径;但本模块**逐条
//! floor 后求和**,使裁剪累加与预算断言口径一致(sum(floor) ≤ floor(sum),
//! 若只对总量 floor,裁剪后总量可能反超预算,见 trim 测试)。
//!
//! 更精确地说,唯一真值源是 [`crate::token_estimate::estimate_text`]
//! (**UTF-8 字节宽/4 floor**):[`estimate_tokens`] / [`estimate_message_tokens`]
//! 都是它的逐条求和包装。该函数同时是 R12 W1-2 起注入给装配内核的估算器
//! (见 [`assembly_token_estimate`])——一个函数,两处账本。
//!
//! # R12 W1-2:估算口径与装配内核统一(单一真值源)
//! 装配内核(`hcw_window::assemble`)原先把 `字符数/4 向上取整、最小 1` 硬编码在
//! 函数体里,与本模块的 `字节宽/4 floor` 是**两套单位**(中文上差 3 倍)。R12 W1-2
//! 给内核加了可注入估算器入口 `assemble_context_with`,本模块随即注入
//! [`assembly_token_estimate`] ⇒ **内核算出的 token 数现在就是本模块口径**。
//!
//! 但仍**不**启用内核驱逐(`max_tokens` 保持 `usize::MAX`,理由见 [`assemble_dedup`]):
//! 口径统一只是启用驱逐的**第一道**前置;第二道缺口是"输入文本表示"不同
//! (内核看到的是去重键用的扁平化文本,不是消息块原文),需单独裁决 + 影子期。
//!
//! # R12 W1-1:装配内核接线(默认关闭)
//! R9 起的「权威装配内核」`hcw_window::assemble::assemble_context`(两档去重 +
//! 单一预算账本 + 驱逐)原本生产调用点 = 0;本模块是该内核在 L10 生产路径上的
//! 第一个接线点(mca-gateway 旧路径**完全无去重**,只有权重桶 + 预算裁剪,
//! 正是内核去重能力的空白面)。
//!
//! - 开关:`CHIMERA_ASSEMBLY_WIRED`(未设 / `off` / `0` ⇒ 旧路径,输出**逐字节
//!   相同**;`on` / `1` ⇒ 先装配去重再裁剪)。解析见 [`parse_assembly_wiring`]。
//! - 分派点只在本模块内部,`trim_to_budget` 签名与调用点不变(零改 adapters)。
//! - 预算仍由**本模块唯一账本**([`trim_to_budget_legacy`])裁决;装配侧预算
//!   有意置为非约束值,WHY 见 [`assemble_dedup`]。
//! - 装配失败/异常一律 fail-open 回旧路径 + warn(纯优化面,不是安全面:
//!   去重失败的正确行为是"少省一点 token",不是"少发一条消息")。

use std::sync::OnceLock;

use event_bus::EventBus;
use hcw_window::assemble::{
    assemble_context_with, AssemblyDiagnostics, ContextItem, ContextRole, ContextSource,
};
use nexus_contracts::affinity::{
    AffinityMessage, AffinityRequest, ContentBlock, MessageRole, ModelAffinitySpec,
};
use nexus_contracts::spec::harness_dimensions::ContextAssemblyContract;
use osa_coordinator::{ComplexityBand, OmniSparseCoordinator};

/// 预算利用率 — 上下文窗口 × 复杂度利用率 × 本比率(默认 0.6)
///
/// WHY 0.6: 预留输出 token 与峰值波动余量,避免输入占满窗口挤掉输出。
const DEFAULT_BUDGET_RATIO: f32 = 0.6;

/// 小窗口模型预算比率 — context_window ≤ 32K 时用 0.7
///
/// WHY 0.7: 小窗口模型(16K/32K)输出空间本就紧张,比大窗口多预留 10%
/// 窗口空间给输出,避免输入裁剪过激导致模型"挤牙膏"式输出。
const SMALL_WINDOW_BUDGET_RATIO: f32 = 0.7;

/// 小窗口判定阈值 — context_window ≤ 此值视为小窗口模型
const SMALL_WINDOW_THRESHOLD: u32 = 32768;

/// 压缩阈值 — 单条历史消息 token 估算超过此值才触发 sidecar 压缩(≈4K)
pub const COMPRESS_THRESHOLD_TOKENS: u32 = 4096;

/// 压缩目标比率 — LLMLingua-2 target_ratio(0.5 = 压缩一半)
pub const COMPRESS_TARGET_RATIO: f32 = 0.5;

/// 复杂度粗判的消息数阈值 — 有工具且消息 ≥ 此值判为 Complex 档
const COMPLEX_MESSAGE_THRESHOLD: usize = 30;

/// 预算计算器单例 — `compute_token_budget` 不依赖 OSA 运行时状态
/// (纯函数),进程内懒构造一次即可;独立 EventBus 仅作构造参数,
/// 不参与任何事件发布(预算面是纯计算)。
static BUDGET_COORDINATOR: OnceLock<OmniSparseCoordinator> = OnceLock::new();

/// 估算会话总 token — 委托 token_estimate 显式化估算(ADR-070)
///
/// 字节宽分类权重(1B:0.25/2B:0.5/3B:0.75/4B:1.0)与既有字节/4 等价,
/// 逐条 floor 后求和——裁剪累加与预算断言口径一致(sum(floor) ≤ floor(sum))。
pub fn estimate_tokens(messages: &[AffinityMessage]) -> u32 {
    crate::token_estimate::estimate_messages(messages)
}

/// 估算单条消息 token — 内容块文本字节宽加权(与 estimate_cost 同口径)
pub fn estimate_message_tokens(message: &AffinityMessage) -> u32 {
    crate::token_estimate::estimate_message(message)
}

/// 会话 token 预算 — osa-coordinator `compute_token_budget` 接线
///
/// complexity 粗判:无工具 = Simple(纯对话);有工具按消息量分档
/// (≥ COMPLEX_MESSAGE_THRESHOLD 判 Complex,否则 Regular)。
/// base_context_window = spec.capabilities.context_window;budget_ratio 按窗口大小
/// 分档:≤32K 用 SMALL_WINDOW_BUDGET_RATIO(0.7),>32K 用 DEFAULT_BUDGET_RATIO(0.6)。
pub fn conversation_budget(spec: &ModelAffinitySpec, request: &AffinityRequest) -> u32 {
    let complexity = if request.tools.is_empty() {
        ComplexityBand::Simple
    } else if request.messages.len() >= COMPLEX_MESSAGE_THRESHOLD {
        ComplexityBand::Complex
    } else {
        ComplexityBand::Regular
    };
    let budget_ratio = if spec.capabilities.context_window <= SMALL_WINDOW_THRESHOLD {
        SMALL_WINDOW_BUDGET_RATIO
    } else {
        DEFAULT_BUDGET_RATIO
    };
    let coordinator =
        BUDGET_COORDINATOR.get_or_init(|| OmniSparseCoordinator::new(EventBus::new()));
    coordinator.compute_token_budget(complexity, spec.capabilities.context_window, budget_ratio)
}

/// 预算驱动的 conversation 裁剪 — 超预算才裁剪(纯函数,不修改入参)
///
/// **本函数是接线前的原始实现,保留为两条用途**:
/// 1. `CHIMERA_ASSEMBLY_WIRED` 关闭(缺省)时它就是生产路径 —— 保证关开关
///    输出与原实现**逐字节相同**;
/// 2. 开启时它是预算**唯一**裁决者(装配只负责去重),同时充当等价性对照的
///    金标准(见 `tests/assembly_wiring.rs::wire_off_equals_legacy`)。
///
/// WHY 保留而不是直接改写:任何"接线"都可能悄悄偏移旧行为,而旧行为已被
/// 大量上游测试(adapters / token_efficiency_integration / 本模块单测)按
/// 具体条数与顺序断言。留一份原样可调用的实现,等于把"等价"变成可执行断言
/// 而不是评审时的口头保证。
///
/// 规则:
/// 1. 空会话或 token ≤ 预算 → 原样返回
/// 2. 恒保留:工具返回消息 + 最后一条消息(时近性保尾段)
/// 3. 候选按 (角色权重, 序号) 评分,权重主键(工具 3 > 用户 2 > 历史 1)
/// 4. 超预算时按权重桶贪心保留,桶内用 `select_nth_unstable_by` 选
///    最近的 K 条(Top-K O(n) 红线);低权重桶全部丢弃
/// 5. 保留集按原始顺序重组
pub fn trim_to_budget_legacy(
    messages: Vec<AffinityMessage>,
    budget_tokens: u32,
) -> Vec<AffinityMessage> {
    // 空会话或预算充足 → 原样返回(不裁剪)
    if messages.is_empty() || estimate_tokens(&messages) <= budget_tokens {
        return messages;
    }

    // 恒保留:工具返回消息 + 最后一条消息(时近性保尾段)
    let last_idx = messages.len() - 1;
    let mut keep: Vec<bool> = messages
        .iter()
        .map(|m| m.role == MessageRole::Tool)
        .collect();
    keep[last_idx] = true;

    // 极端兜底:保留集自身已 ≥ 预算(工具+最新消息超预算)——语义优先,
    // 宁可超预算也不丢工具返回与最新输入(此时继续裁剪会破坏角色语义)。
    let keep_tokens: u32 = messages
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(m, _)| estimate_message_tokens(m))
        .sum();
    if keep_tokens >= budget_tokens {
        return messages
            .into_iter()
            .zip(keep)
            .filter(|(_, k)| *k)
            .map(|(m, _)| m)
            .collect();
    }
    let mut remaining = budget_tokens - keep_tokens;

    // 候选按权重桶贪心保留:高权重桶整桶保留,首个放不下的桶内
    // 用 select_nth_unstable_by 选最近的 K 条,更低权重桶全部丢弃。
    let mut selected: Vec<usize> = Vec::new();
    for weight in (1..=2).rev() {
        // 候选仅 User(2)/Assistant+System(1),Tool 已在 keep 恒保留
        let bucket: Vec<(usize, u32)> = messages
            .iter()
            .enumerate()
            .filter(|(i, m)| !keep[*i] && role_weight(m.role) == weight)
            .map(|(i, m)| (i, estimate_message_tokens(m)))
            .collect();
        if bucket.is_empty() {
            continue;
        }
        let bucket_tokens: u32 = bucket.iter().map(|(_, t)| t).sum();
        if remaining >= bucket_tokens {
            selected.extend(bucket.iter().map(|(i, _)| *i));
            remaining -= bucket_tokens;
        } else {
            // 桶内新消息优先:从最新往前贪心确定 K(最近的 K 条放得下预算)
            let mut acc = 0u32;
            let mut k = 0usize;
            for (_, t) in bucket.iter().rev() {
                if acc + t > remaining {
                    break;
                }
                acc += t;
                k += 1;
            }
            if k > 0 {
                // Top-K 红线:select_nth_unstable_by 部分排序 O(n),禁止 sort_by
                let mut top = bucket;
                top.select_nth_unstable_by(k - 1, |a, c| c.0.cmp(&a.0));
                for (i, _) in top[..k].iter() {
                    selected.push(*i);
                }
            }
            break;
        }
    }

    for i in selected {
        keep[i] = true;
    }
    messages
        .into_iter()
        .enumerate()
        .filter(|(i, _)| keep[*i])
        .map(|(_, m)| m)
        .collect()
}

// ============================================================
// R12 W1-1:装配内核接线(默认关闭)
// ============================================================

/// 装配接线开关的环境变量名
pub const ASSEMBLY_WIRED_ENV: &str = "CHIMERA_ASSEMBLY_WIRED";

/// 装配接线开关 —— `CHIMERA_ASSEMBLY_WIRED` 的解析结果
///
/// WHY 独立枚举而非裸 `bool`:调用方(含测试)需要"显式给定开关"的纯函数入口
/// [`trim_to_budget_with`],而布尔字面量在调用点读不出语义(`true` 是"开装配"
/// 还是"开裁剪"?);枚举把分派意图写进类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssemblyWiring {
    /// 关闭(缺省)—— 只走 R4 纯裁剪,输出与接线前逐字节相同
    #[default]
    Off,
    /// 开启 —— 先经装配内核去重,再由 R4 裁剪裁决预算
    On,
}

/// 解析 `CHIMERA_ASSEMBLY_WIRED`(表驱动纯函数,不读环境)
///
/// | 输入 | 输出 | 附加行为 |
/// |------|------|----------|
/// | 未设 / 空串(含纯空白) | `Off` | 无(空串视同未设,不 warn) |
/// | `on` / `1` / `true` / `yes`(ASCII 大小写不敏感) | `On` | 无 |
/// | `off` / `0` / `false` / `no`(ASCII 大小写不敏感) | `Off` | 无 |
/// | 其他任意值 | `Off` | warn |
///
/// WHY 非法值 warn 后回退 `Off` 而非拒绝启动/回退 `On`:
/// - 回退 `On` 会让一个拼错的开关静默改变生产行为(且是尚未验证的路径);
/// - 拒绝启动则把"纯优化面"的错配放大成可用性事故;
/// - 本函数保持纯函数形态(入参 `Option<&str>`),使非法值/缺省值的分支
///   可在不改进程环境的前提下被直接断言(进程环境是全局可变的,测试内
///   `set_var` 会与同进程并行测试互相污染)。
pub fn parse_assembly_wiring(raw: Option<&str>) -> AssemblyWiring {
    let Some(raw) = raw else {
        return AssemblyWiring::Off;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return AssemblyWiring::Off;
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "on" | "1" | "true" | "yes" => AssemblyWiring::On,
        "off" | "0" | "false" | "no" => AssemblyWiring::Off,
        _ => {
            tracing::warn!(
                env = ASSEMBLY_WIRED_ENV,
                value = %raw,
                "CHIMERA_ASSEMBLY_WIRED 非法(应为 on/1 或 off/0),回退关闭"
            );
            AssemblyWiring::Off
        }
    }
}

/// 进程级接线开关 —— 环境读取一次后缓存
///
/// WHY `OnceLock` 而非每次 `std::env::var`:本函数位于 `trim_to_budget` 的
/// 请求热路径上,本项目红线要求环境开关"启动期一次读取、禁热路径运行时开关"
/// (与 `hcw_window::assemble::normalized_dedup_enabled` 的
/// `CHIMERA_DEDUP_NORMALIZE`、mlc 的 `hnsw_tier_enabled_by_env` 同款纪律)。
///
/// 代价(已知边界,非缺陷):进程内不可热切换 —— 翻转开关需重启进程。
/// 逃生通道可用性优先于热切换:回滚 = 重启并回到缺省(未设)即可。
pub fn assembly_wiring() -> AssemblyWiring {
    static WIRING: OnceLock<AssemblyWiring> = OnceLock::new();
    *WIRING.get_or_init(|| {
        // 非 UTF-8 值 ⇒ `Err` ⇒ `None` ⇒ Off(与"非法值回退关闭"同向)
        let raw = std::env::var(ASSEMBLY_WIRED_ENV).ok();
        parse_assembly_wiring(raw.as_deref())
    })
}

/// 预算驱动的 conversation 裁剪(对外入口)—— 按环境开关分派
///
/// 签名与语义与接线前一致,故既有调用点(`adapters::prepare_effective_request`
/// 等)零改动;分派完全封装在本模块内。
pub fn trim_to_budget(messages: Vec<AffinityMessage>, budget_tokens: u32) -> Vec<AffinityMessage> {
    trim_to_budget_with(messages, budget_tokens, assembly_wiring())
}

/// 显式开关版的裁剪核心(纯函数,不读环境变量)
///
/// # 参数
/// - `messages`:会话消息(所有权转移,便于关闭态零拷贝直通旧路径)
/// - `budget_tokens`:token 预算,口径 = 本模块 `estimate_tokens`(字节宽加权、
///   逐条 floor);**与装配内核内部口径不同**,见 [`assemble_dedup`]
/// - `wiring`:开关。`Off` ⇒ 与 [`trim_to_budget_legacy`] 完全一致
///
/// # 返回
/// 裁剪后的消息序列。`wiring == On` 时先经装配内核去重,再由旧路径裁决预算。
///
/// # 失败语义(fail-open)
/// 装配阶段任何异常(空结果 / `seq` 越界)都回退旧路径并 warn。WHY 不是
/// fail-closed:去重是**纯优化面** —— 失败的正确后果是"这一轮少省些 token",
/// 而不是"少发一条消息给模型"。安全面(fail-closed)与本面的取舍方向相反,
/// 不适用同一条纪律。
pub fn trim_to_budget_with(
    messages: Vec<AffinityMessage>,
    budget_tokens: u32,
    wiring: AssemblyWiring,
) -> Vec<AffinityMessage> {
    let legacy = |msgs: Vec<AffinityMessage>| trim_to_budget_legacy(msgs, budget_tokens);
    // 空会话不是故障:旧路径本就返回空,不必先跑一遍内核再命中空结果告警
    if wiring == AssemblyWiring::Off || messages.is_empty() {
        return legacy(messages);
    }
    match assemble_dedup(&messages) {
        Ok(deduped) => legacy(deduped),
        Err(reason) => {
            tracing::warn!(
                reason,
                before = messages.len(),
                "装配内核接线异常,回退旧裁剪路径(fail-open)"
            );
            legacy(messages)
        }
    }
}

/// 注入给装配内核的估算器 —— **本模块口径**(UTF-8 字节宽/4 floor)
///
/// # WHY 是一个具名函数而不是闭包字面量
/// 1. 它是"一个函数,两处账本"的可指认实体:裁剪侧 [`estimate_tokens`] 与内核侧
///    预算/`used_tokens` 都归结到 [`crate::token_estimate::estimate_text`] 同一个
///    实现上,评审时能一眼看出**没有第二把尺子**;
/// 2. 装配内核的注入面是 `&dyn Fn(&str) -> usize`,而 `estimate_text` 返回 `u32`
///    —— 这里做唯一一次 `as usize` 扩宽(`u32 → usize` 在 32/64 位目标上均无损,
///    不会截断),而不是在调用点写一个看不出意图的转换;
/// 3. 可被 `#[cfg(test)]` 单测直接调用,断言"内核 `used_tokens` == Σ 本函数(内核看
///    到的 content)"(见本文件 `mod tests::kernel_used_tokens_are_in_mca_units`)。
///
/// # 参数语义(这一步的边界,必须写清)
/// `content` 是**装配内核看到的字符串** —— 在本路径上即 [`dedup_content`] 产出的
/// 去重键扁平化文本(带 `标记 + 字节长度 + ':'` 前缀),**不是**消息块原文。故
/// "口径统一"指的是**计量函数一致**(字节宽/4 floor),不指"和
/// [`estimate_tokens`]`(messages)` 逐 token 相等"(后者按块原文逐块 floor 求和)。
/// 该差异是启用内核驱逐前必须处理的第二道缺口,已知且已登记。
pub(crate) fn assembly_token_estimate(content: &str) -> usize {
    crate::token_estimate::estimate_text(content) as usize
}

/// 装配内核去重闸门 —— 消息 → `ContextItem` → `assemble_context_with` → 回取原消息
///
/// # 参数
/// `messages`:原始会话消息(只读)
///
/// # 返回
/// 去重后的消息序列(**原消息的克隆**,非重新构造)与内核诊断;异常返回 `Err(原因)`。
///
/// # WHY 装配侧预算**仍**置 `usize::MAX`(R12 W1-2 后依然如此)
/// 两个口径原本系统性不同:
/// - 内核旧默认 `assemble::estimate_tokens` = 字符数/4 **向上取整、最小 1**;
/// - 本模块 [`crate::token_estimate::estimate_text`] = UTF-8 **字节宽**/4 **floor**。
///   中文/emoji 上 3 字节宽 = 0.75 token/字符 vs 0.25 token/字符(**1/3 低估**)。
///
/// **R12 W1-2 已消除这一半差异**:内核现在通过 `assemble_context_with` 接收
/// [`assembly_token_estimate`],其内的预算累计/驱逐扣减/`used_tokens` 全部按
/// **本模块单位**计量 ⇒ **口径已统一,启用内核驱逐的第一道前置已就绪**。
///
/// 但本波次**仍然不**启用内核驱逐,理由是三个未解问题(任一都足以否决):
/// 1. **表示层缺口**:内核看到的是 [`dedup_content`] 的扁平化文本(带标记与前缀),
///    不是消息块原文 ⇒ 即便尺子相同,量出来的总和 ≠ [`estimate_tokens`]`(messages)`。
///    直接喂真实预算等于在"另一份文本"上做驱逐决策,偏差方向随块结构而变。
/// 2. **双层账本**:内核驱逐与随后 [`trim_to_budget_legacy`] 的裁剪叠加,同一条
///    消息会被两套优先级各裁一次(过度裁剪)。
/// 3. **优先级方向冲突**:内核按 `source_value_factor(Tool)=0.3` 把工具输出当最低
///    价值最先淘汰,与旧路径"工具返回消息恒保留"的不变量**相反**。
///
/// 故本波次的架构仍是**单一账本**:
/// - 内核只做它不可替代的那件事 —— 两档去重(精确 + 规范化);
/// - 预算驱逐的**唯一**裁决权留在 [`trim_to_budget_legacy`](本模块口径);
/// - 传给内核的 `max_tokens` 因此是**有意非约束**的(驱逐段永不触发,`evicted` 恒 0)。
///
/// 代价(已知边界):`diagnostics.evicted` 恒 0;`used_tokens` 现在虽是**本模块单位**
/// (口径已统一),但因表示层缺口,仍只作日志参考,不得用于告警或阈值。
///
/// # WHY 回取走 `seq` 而不是重建消息
/// `ContextItem.content` 是**去重键用的扁平化文本**(见 [`dedup_content`]),
/// 重建会丢掉 `Thinking`/`ToolUse` 块的签名与结构 —— 那是 P4 状态守恒的一等
/// 公民(Anthropic 回传校验会直接 400)。故 `seq` 承载原始下标,回取时按
/// `seq` 取回**原消息的克隆**,装配结果只决定"保留哪几条、按什么顺序",
/// 绝不改写消息内容。这也使"输出是输入的子序列"成为结构性保证而非约定。
fn assemble_dedup(messages: &[AffinityMessage]) -> Result<Vec<AffinityMessage>, &'static str> {
    assemble_dedup_detailed(messages).map(|(kept, _diagnostics)| kept)
}

/// [`assemble_dedup`] 的诊断可见版 —— 同一条实现,额外把内核诊断交回调用方
///
/// WHY 拆出这一层而不是让 `assemble_dedup` 直接返回元组:生产调用点
/// ([`trim_to_budget_with`])只关心"保留哪几条",多一个字段会让调用点出现
/// 未使用的绑定;而测试需要 `diagnostics.used_tokens` 来证明"内核算的是本模块
/// 单位"(见 `mod tests`)。
fn assemble_dedup_detailed(
    messages: &[AffinityMessage],
) -> Result<(Vec<AffinityMessage>, AssemblyDiagnostics), &'static str> {
    // === 参与面闸门（R12 W1-1 订正，ADR-298 D2）===
    //
    // **只有 `Tool` 消息参与去重**；`User` / `Assistant` / `System` 恒保留。
    //
    // WHY 不能对对话轮次去重：重复的对话消息是**不同事件**，不是同一件事的副本。
    // 用户连说两次「继续」= 两次发言（模型应看到两次输入），折叠掉就是**静默少一次
    // 发言**；而 `CHIMERA_DEDUP_NORMALIZE` 默认开启，连"仅大小写/空白不同"的两条
    // 不同轮次也会被折成一条 —— 那是把语义差异当噪声删。
    //
    // 工具消息则相反：同一段工具输出被多轮重复注入，是同一个产物的副本，去重是净收益
    // （上下文装配内核 R9 的去重本就是为这类"注入项"设计的）。
    //
    // 该闸门让"去重"与"压缩"两个目的解耦：本路径只做前者，且只在其语义成立的面上做。
    let items: Vec<ContextItem> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| is_dedup_participant(message.role))
        .map(|(index, message)| ContextItem {
            role: context_role(message.role),
            source: context_source(message.role),
            content: dedup_content(message),
            pinned: is_pinned(message.role, index, messages.len()),
            // seq = 原始下标(唯一且非递减),作为回取的唯一句柄
            seq: index as u64,
        })
        .collect();

    let mut contract = ContextAssemblyContract::default_contract();
    // 有意非约束:预算唯一账本在本模块,理由见本函数文档
    contract.max_tokens = usize::MAX;

    // 全是对话轮次 = 没有可去重的候选（正常情形，不是故障）：原样放行，
    // 不起内核、不告警，也不构造一份恒等的空装配。
    if items.is_empty() {
        return Ok((messages.to_vec(), no_participant_diagnostics(&contract)));
    }

    // tier_capacity = 0 ⇒ 内核视为"未配置",只用 contract.max_tokens
    //
    // R12 W1-2:注入本模块估算器 `assembly_token_estimate`(字节宽/4 floor)。
    // 内核内的预算累计 / 驱逐扣减 / `used_tokens` 全部只经它 ⇒ 与
    // `estimate_message_tokens` 同一把尺子。`max_tokens = usize::MAX` 仍使驱逐
    // 不触发(三条例由见本函数文档),故这一步是**口径统一**而非启用驱逐。
    let assembled = assemble_context_with(items, &contract, 0, &assembly_token_estimate);

    if assembled.items.is_empty() {
        return Err("装配结果为 0 条(不得用空结果替换非空输入)");
    }

    // 逐条决定去留：非参与者恒 true，参与者默认 false，被内核留下者置 true
    let mut keep = vec![true; messages.len()];
    for (index, message) in messages.iter().enumerate() {
        if is_dedup_participant(message.role) {
            keep[index] = false;
        }
    }
    for item in &assembled.items {
        let index = usize::try_from(item.seq).map_err(|_| "seq 超出 usize 域")?;
        let Some(slot) = keep.get_mut(index) else {
            // 内核契约:seq 为调用方提供且不得改写。越界即契约被破坏 ⇒ 回退
            return Err("seq 越界(内核不得改写 seq)");
        };
        *slot = true;
    }

    let before = messages.len();
    let out: Vec<AffinityMessage> = messages
        .iter()
        .zip(keep.iter())
        .filter(|(_, kept)| **kept)
        .map(|(message, _)| message.clone())
        .collect();
    tracing::debug!(
        before,
        after = out.len(),
        deduped = assembled.diagnostics.deduped,
        deduped_normalized = assembled.diagnostics.deduped_normalized,
        "上下文装配内核已去重 CHIMERA_ASSEMBLY_WIRED=on"
    );
    Ok((out, assembled.diagnostics))
}

/// 无去重参与者时的诊断快照 —— 与内核「空输入」分支同形
///
/// WHY 需要:该快路径不起内核(没有可去重的候选),但返回值类型要求一份诊断。
/// 字段取内核空输入分支的语义值(`budget_tokens` = 契约预算、其余 0),使
/// "没起内核"与"起了内核但输入为空"在诊断面上不可区分 —— 两者对调用方确实是
/// 同一件事。
fn no_participant_diagnostics(contract: &ContextAssemblyContract) -> AssemblyDiagnostics {
    AssemblyDiagnostics {
        deduped: 0,
        deduped_normalized: 0,
        evicted: 0,
        budget_tokens: contract.max_tokens,
        used_tokens: 0,
        synthesis_requested: contract.on_demand_synthesis,
    }
}

/// 该角色是否参与去重 —— **只允许工具消息**
///
/// 语义判据而非性能判据：对话轮次的重复是不同事件，工具输出块的重复是同一产物的副本。
/// 详见 [`assemble_dedup`] 的闸门说明。
fn is_dedup_participant(role: MessageRole) -> bool {
    role == MessageRole::Tool
}

/// 角色 → 装配面角色(一一对应,无信息损失)
fn context_role(role: MessageRole) -> ContextRole {
    match role {
        MessageRole::System => ContextRole::System,
        MessageRole::User => ContextRole::User,
        MessageRole::Assistant => ContextRole::Assistant,
        MessageRole::Tool => ContextRole::Tool,
    }
}

/// 角色 → 装配面来源(退化映射)
///
/// WHY 退化:L0 `AffinityMessage` **没有 provenance 字段**(L0 零依赖铁律禁止
/// `serde_json::Value`,内容块仅 4 类),因此"这条消息来自记忆检索还是知识
/// 检索"在 L10 侧不可知。只能让 `source` 跟随 `role`,`ContextSource::{Memory,
/// Retrieval}` 在本路径**不可达**。后果:去重键的 source 维度在此路径上不增加
/// 判别力(role 已覆盖同一区分);这是接线面的真实信息损失,已列入报告,
/// 不靠"名字嗅探"猜测补齐(原则 P8:代码零厂商/场景字符串)。
fn context_source(role: MessageRole) -> ContextSource {
    match role {
        MessageRole::System => ContextSource::System,
        MessageRole::User => ContextSource::User,
        MessageRole::Assistant => ContextSource::Assistant,
        MessageRole::Tool => ContextSource::Tool,
    }
}

/// 钉选判定 —— 与旧路径的"恒保留"集合对齐
///
/// 旧路径恒保留**工具返回消息**与**最后一条消息**(时近性保尾段),故此处
/// 同集合置 `pinned`。`System` 故意**不**钉选:旧路径把 System 与 Assistant
/// 同列 `role_weight == 1`(最先被裁),若在此钉选会让两条路径的保护集合
/// 分叉。注:本波次装配侧驱逐不触发,`pinned` 目前只作为语义声明与后续
/// 接管驱逐时的前置事实。
fn is_pinned(role: MessageRole, index: usize, len: usize) -> bool {
    role == MessageRole::Tool || index + 1 == len
}

/// 消息 → 去重键内容(扁平化,仅用于去重判定)
///
/// WHY 逐字段 `标记 + 字节长度 + ':' + 原文`:内核精确去重键 =
/// `(role, source, content)`,若用"块文本直接拼接",同角色下
/// `Text{"ok"}` 与 `ToolResult{content:"ok"}` 会被判为同一条 ⇒ 误删一条
/// **不同**的消息;若只用 `\n` 分隔,`[Text("a"),Text("b")]` 与
/// `[Text("a\nb")]` 又会撞成同一键。长度前置(Netstring 式)使字段自定界、
/// 整体对"块序列"单射,把假同键面压到零 —— 与项目「宁漏勿错」纪律一致
/// (少合并 = 不误删)。
///
/// 该文本**只作去重键**,不用于重建消息(重建走 `seq` 回取原消息),故不会被
/// 写回请求体,也不参与本模块的 token 估算。
fn dedup_content(message: &AffinityMessage) -> String {
    let mut out = String::new();
    for block in &message.blocks {
        match block {
            ContentBlock::Text { text } => push_key_field(&mut out, 't', text),
            // signature 一并入键(不参与内容语义,但区分"不同轮次"的同文思考块;
            // 遗漏它会让"同文不同签名"的两次思考被判为同一条 ⇒ 误删)
            ContentBlock::Thinking {
                thinking,
                signature,
            } => {
                push_key_field(&mut out, 'k', thinking);
                push_key_field(&mut out, 's', signature.as_deref().unwrap_or(""));
            }
            ContentBlock::ToolUse {
                id,
                name,
                input_json,
            } => {
                push_key_field(&mut out, 'u', id);
                push_key_field(&mut out, 'n', name);
                push_key_field(&mut out, 'i', input_json);
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                push_key_field(&mut out, 'r', tool_use_id);
                push_key_field(&mut out, 'e', if *is_error { "1" } else { "0" });
                push_key_field(&mut out, 'c', content);
            }
        }
    }
    out
}

/// 追加一个自定界字段:`<标记><字节长度>:<原文>`
fn push_key_field(out: &mut String, tag: char, value: &str) {
    out.push(tag);
    out.push_str(&value.len().to_string());
    out.push(':');
    out.push_str(value);
}

/// 选最长的可压缩历史消息 — 排除 System/Tool 与最后一条
///
/// 返回消息 index;无候选(全为 System/Tool/最后一条)返回 None。
pub fn longest_compressible_message(messages: &[AffinityMessage]) -> Option<usize> {
    let last = messages.len().saturating_sub(1);
    messages
        .iter()
        .enumerate()
        .filter(|(i, m)| *i != last && m.role != MessageRole::System && m.role != MessageRole::Tool)
        .max_by_key(|(_, m)| estimate_message_tokens(m))
        .map(|(i, _)| i)
}

/// 角色 → 裁剪权重(数值越大越优先保留)
fn role_weight(role: MessageRole) -> u32 {
    match role {
        MessageRole::Tool => 3,
        MessageRole::User => 2,
        // Assistant 与 System 同属历史消息,裁剪优先
        MessageRole::Assistant | MessageRole::System => 1,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use nexus_contracts::affinity::ContentBlock;
    use nexus_contracts::affinity::{
        AffinityOverrides, OutputFormat, ProtocolDialect, ProviderId, SamplingParams,
        ThinkingPreference, ToolDecl,
    };

    fn msg(role: MessageRole, text: &str) -> AffinityMessage {
        AffinityMessage {
            role,
            blocks: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    fn request(tools: Vec<ToolDecl>, msg_count: usize) -> AffinityRequest {
        AffinityRequest {
            intent_id: "intent-trim".into(),
            messages: (0..msg_count)
                .map(|i| msg(MessageRole::User, &format!("turn {i}")))
                .collect(),
            tools,
            thinking_pref: ThinkingPreference::Fast,
            budget_hint_micro: None,
            overrides: AffinityOverrides::default(),
            sampling: SamplingParams::default(),
            output_format: OutputFormat::default(),
        }
    }

    // ============================================================
    // 3.1 失败测试:裁剪语义
    // ============================================================

    #[test]
    fn trim_reduces_under_budget_keeping_tool_results() {
        // 超预算 conversation:8 条长 User 历史 + 短工具返回 + 最后一条
        let mut messages = Vec::new();
        for i in 0..8 {
            messages.push(msg(
                MessageRole::User,
                &format!("history message number {i} {}", "x".repeat(200)),
            ));
        }
        messages.push(msg(MessageRole::Tool, "tool result ok"));
        messages.push(msg(MessageRole::Assistant, "last response"));
        let budget = 200;
        let trimmed = trim_to_budget(messages.clone(), budget);
        assert!(
            estimate_tokens(&trimmed) <= budget,
            "裁剪后 token 必须 ≤ 预算, got {}",
            estimate_tokens(&trimmed)
        );
        assert!(
            trimmed.iter().any(|m| m.role == MessageRole::Tool),
            "工具返回消息必须恒保留"
        );
        assert_eq!(
            trimmed.last().map(|m| m.blocks.clone()),
            messages.last().map(|m| m.blocks.clone()),
            "最后一条消息必须保留(时近性)"
        );
    }

    #[test]
    fn trim_keeps_original_when_within_budget() {
        let messages = vec![
            msg(MessageRole::User, "hi"),
            msg(MessageRole::Assistant, "hello"),
        ];
        let trimmed = trim_to_budget(messages.clone(), 1000);
        assert_eq!(trimmed, messages, "预算充足时不得裁剪");
    }

    #[test]
    fn trim_empty_conversation_is_noop() {
        let trimmed = trim_to_budget(Vec::new(), 100);
        assert!(trimmed.is_empty(), "空会话不 panic 且返回空");
    }

    #[test]
    fn trim_zero_budget_keeps_recent_message() {
        let messages = vec![
            msg(MessageRole::Assistant, "old assistant turn"),
            msg(MessageRole::User, "new user turn"),
        ];
        let trimmed = trim_to_budget(messages.clone(), 0);
        assert!(!trimmed.is_empty(), "预算零也必须兜底保留最近消息");
        assert!(
            trimmed.iter().any(|m| m.blocks == messages[1].blocks),
            "最新用户输入必须兜底保留"
        );
    }

    #[test]
    fn trim_preserves_original_relative_order() {
        // 工具返回在历史中段;裁剪后保留集按原序重组,工具恒在最后一条之前
        let messages = vec![
            msg(
                MessageRole::User,
                &format!("first user {}", "x".repeat(100)),
            ),
            msg(
                MessageRole::Assistant,
                &format!("assistant turn {}", "x".repeat(100)),
            ),
            msg(MessageRole::Tool, "tool result"),
            msg(
                MessageRole::User,
                &format!("second user {}", "x".repeat(100)),
            ),
            msg(MessageRole::Assistant, "latest assistant"),
        ];
        let trimmed = trim_to_budget(messages.clone(), 50);
        let roles: Vec<MessageRole> = trimmed.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            vec![MessageRole::Tool, MessageRole::User, MessageRole::Assistant,],
            "保留集按原序重组,工具恒在最新消息之前, roles = {roles:?}"
        );
    }

    // ============================================================
    // 3.2 预算接线:conversation_budget 复杂度分档
    // ============================================================

    #[test]
    fn conversation_budget_scales_with_complexity() {
        let mut spec = ModelAffinitySpec::minimal(
            ProviderId::DeepSeek,
            "deepseek-v4-flash",
            ProtocolDialect::OpenAiChat,
        );
        spec.capabilities.context_window = 128_000;
        let tool = ToolDecl {
            name: "search".into(),
            description: "web search".into(),
            parameters_schema: "{}".into(),
        };
        let simple = conversation_budget(&spec, &request(Vec::new(), 3));
        let regular = conversation_budget(&spec, &request(vec![tool.clone()], 3));
        let complex = conversation_budget(&spec, &request(vec![tool], 30));
        // 128K × 利用率 × 0.6:Simple=0.25 / Regular=0.5 / Complex=0.75
        assert!(
            (19_000..25_000).contains(&simple),
            "Simple ≈ 128K×0.25×0.6 = 19200, got {simple}"
        );
        assert!(
            simple < regular && regular < complex,
            "复杂度越高预算越高, {simple} < {regular} < {complex}"
        );
    }

    // ============================================================
    // 3.3 窗口大小分档:小窗口(≤32K)用 0.7,大窗口(>32K)用 0.6
    // ============================================================

    fn spec_with_window(window: u32) -> ModelAffinitySpec {
        let mut spec = ModelAffinitySpec::minimal(
            ProviderId::DeepSeek,
            "test-model",
            ProtocolDialect::OpenAiChat,
        );
        spec.capabilities.context_window = window;
        spec
    }

    #[test]
    fn small_window_models_use_higher_budget_ratio() {
        // 16K 窗口:Simple 档 = 16384 × 0.25 × 0.7 = 2867.2 → 2867
        let spec = spec_with_window(16384);
        let simple = conversation_budget(&spec, &request(Vec::new(), 3));
        // 允许 ±1 浮点误差
        assert!(
            (2866..=2868).contains(&simple),
            "16K Simple 预期 ≈ 16384×0.25×0.7=2867, got {simple}"
        );

        // 32K 窗口:Regular 档 = 32768 × 0.5 × 0.7 = 11468.8 → 11468
        let spec = spec_with_window(32768);
        let tool = ToolDecl {
            name: "search".into(),
            description: "web search".into(),
            parameters_schema: "{}".into(),
        };
        let regular = conversation_budget(&spec, &request(vec![tool], 5));
        assert!(
            (11467..=11470).contains(&regular),
            "32K Regular 预期 ≈ 32768×0.5×0.7=11468, got {regular}"
        );
    }

    #[test]
    fn large_window_models_use_default_budget_ratio() {
        // 64K 窗口:Simple 档 = 65536 × 0.25 × 0.6 = 9830.4 → 9830
        let spec = spec_with_window(65536);
        let simple = conversation_budget(&spec, &request(Vec::new(), 3));
        assert!(
            (9829..=9831).contains(&simple),
            "64K Simple 预期 ≈ 65536×0.25×0.6=9830, got {simple}"
        );

        // 128K 窗口:Simple 档 = 131072 × 0.25 × 0.6 = 19660.8 → 19660
        let spec = spec_with_window(131072);
        let simple = conversation_budget(&spec, &request(Vec::new(), 3));
        assert!(
            (19659..=19662).contains(&simple),
            "128K Simple 预期 ≈ 131072×0.25×0.6=19660, got {simple}"
        );

        // 1M 窗口:Simple 档 = 1048576 × 0.25 × 0.6 = 157286.4 → 157286
        let spec = spec_with_window(1_048_576);
        let simple = conversation_budget(&spec, &request(Vec::new(), 3));
        assert!(
            (157285..=157287).contains(&simple),
            "1M Simple 预期 ≈ 1048576×0.25×0.6=157286, got {simple}"
        );
    }

    // ============================================================
    // token 估算与压缩辅助
    // ============================================================

    #[test]
    fn estimate_tokens_uses_char4_heuristic() {
        assert_eq!(estimate_tokens(&[msg(MessageRole::User, "abcd")]), 1);
        assert_eq!(estimate_tokens(&[msg(MessageRole::User, "abc")]), 0);
        // 逐条 floor 求和:3 条 5 字符 = 3×1 = 3,而非 floor(15/4)=3
        let three = vec![
            msg(MessageRole::User, "12345"),
            msg(MessageRole::User, "abcde"),
            msg(MessageRole::User, "ABCDE"),
        ];
        assert_eq!(estimate_tokens(&three), 3);
    }

    #[test]
    fn longest_compressible_excludes_system_tool_and_latest() {
        let messages = vec![
            msg(MessageRole::System, "sys prompt"),
            msg(MessageRole::User, "short"),
            msg(MessageRole::Tool, "tool result"),
            msg(MessageRole::Assistant, &"a".repeat(400)),
            msg(MessageRole::User, "latest user turn"),
        ];
        assert_eq!(
            longest_compressible_message(&messages),
            Some(3),
            "最长可压缩历史消息为 index 3(Assistant)"
        );
        // 仅 System + 最后一条 → 无候选
        let only_latest = vec![msg(MessageRole::System, "s"), msg(MessageRole::User, "u")];
        assert_eq!(longest_compressible_message(&only_latest), None);
    }

    #[test]
    fn role_weight_ordering() {
        assert!(role_weight(MessageRole::Tool) > role_weight(MessageRole::User));
        assert!(role_weight(MessageRole::User) > role_weight(MessageRole::Assistant));
        assert_eq!(
            role_weight(MessageRole::Assistant),
            role_weight(MessageRole::System)
        );
    }

    // ============================================================
    // R12 W1-2:注入给装配内核的估算器 = 本模块口径(单一真值源)
    // ============================================================

    /// 注入器必须**就是** `token_estimate::estimate_text`,不是复制品
    ///
    /// WHY 断言"恒等"而不是逐值对照:复制公式（把 4 写成字面量）也能让逐值对照
    /// 通过，但下次改标定就会分叉。这里锁定的是"同一实现"这一事实。
    #[test]
    fn assembly_estimator_is_byte_width_floor() {
        // 字节宽/4 floor（mca 口径）：ASCII 4 字节/4=1、3 字节/4=0
        assert_eq!(assembly_token_estimate("abcd"), 1);
        assert_eq!(assembly_token_estimate("abc"), 0);
        // 中文 3 字节宽：4 字符 = 12 字节/4 = 3（内核旧默认口径同一个串只给 1）
        assert_eq!(assembly_token_estimate("你好世界"), 3);
        assert_eq!(assembly_token_estimate(""), 0);
        for text in ["", "a", "abc", "abcd", "你好世界", "修复bug", "你好，世界"] {
            assert_eq!(
                assembly_token_estimate(text),
                usize::try_from(crate::token_estimate::estimate_text(text))
                    .expect("u32 → usize 在 32 位以上目标均无损"),
                "{text:?} 必须与 token_estimate::estimate_text 同值(同一实现)"
            );
        }
    }

    /// **关键**:内核算出的 `used_tokens` 是本模块单位(字节宽/4 floor)
    ///
    /// 断言口径 = `Σ assembly_token_estimate(内核看到的那份 content)`。
    /// WHY 不写成 `estimate_tokens(&messages)`:内核看到的是去重键用的**扁平化
    /// 文本**(`dedup_content`,带 `标记+字节长度+':'` 前缀),不是消息块原文,
    /// 两者必然不相等 —— 那不是缺陷,是"表示层仍未对齐"这个已知缺口
    /// (报告 `docs/reports/r12-w1-2-estimator.md` 的已知边界 §1)。
    #[test]
    fn kernel_used_tokens_are_in_mca_units() {
        let messages = vec![
            msg(MessageRole::Tool, "你好世界"),
            msg(MessageRole::Tool, "工具返回结果"),
            msg(MessageRole::Tool, "tool ok"),
            msg(MessageRole::User, "继续"),
        ];
        let (kept, diagnostics) = assemble_dedup_detailed(&messages).expect("装配必须成功");
        assert_eq!(kept, messages, "无重复 ⇒ 输出与输入逐条相同(含非参与者)");

        // 参与去重的条目 = 内核看到的条目
        let participants: Vec<&AffinityMessage> = messages
            .iter()
            .filter(|m| is_dedup_participant(m.role))
            .collect();
        assert!(
            !participants.is_empty(),
            "本用例必须有去重参与者,否则内核未启动"
        );

        let expected: usize = participants
            .iter()
            .map(|m| assembly_token_estimate(&dedup_content(m)))
            .sum();
        assert_eq!(
            diagnostics.used_tokens, expected,
            "内核 used_tokens 必须恰等于 Σ 本模块口径(内核看到的那份 content)"
        );

        // 判别力:内核旧默认口径(字符/4 向上取整)在同一语料上必然给出不同读数
        let legacy_units: usize = participants
            .iter()
            .map(|m| hcw_window::assemble::estimate_tokens(&dedup_content(m)))
            .sum();
        assert_ne!(
            diagnostics.used_tokens, legacy_units,
            "中文语料上两套口径必须给出不同读数,否则本用例证明不了注入生效"
        );

        // 本波次不启用驱逐:预算 = usize::MAX ⇒ evicted 恒 0
        assert_eq!(diagnostics.evicted, 0, "max_tokens=usize::MAX ⇒ 驱逐不触发");
        assert_eq!(diagnostics.budget_tokens, usize::MAX);
    }

    /// 无去重参与者(全为对话轮次)⇒ 不起内核,诊断取内核空输入分支同形值
    #[test]
    fn no_participant_fast_path_reports_empty_diagnostics() {
        let messages = vec![
            msg(MessageRole::User, "继续"),
            msg(MessageRole::Assistant, "好的"),
        ];
        let (kept, diagnostics) = assemble_dedup_detailed(&messages).expect("空参与面不是故障");
        assert_eq!(kept, messages, "无参与者 ⇒ 原样放行");
        assert_eq!(diagnostics.deduped, 0);
        assert_eq!(diagnostics.evicted, 0);
        assert_eq!(diagnostics.used_tokens, 0);
        assert_eq!(diagnostics.budget_tokens, usize::MAX);
    }
}
