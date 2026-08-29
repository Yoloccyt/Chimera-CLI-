//! CSC 四级渐进压缩链 + from 模式保前缀(v4.0 WI-12 / ADR-119 / 手册 §10.5)
//!
//! 对应任务:P2-T4(T-03 CSC + v4.0 WI-12)
//! 对应架构层:L2 Memory(挂 hcw-window 增强,ADR-139 批准,否决新建 nexus-compress)
//!
//! # 四级渐进链(渐进增强,信息悬崖消除)
//! `Snip`(规则去重截断)→ `Microcompact`(签名化,本任务降级为去重+截断的可逆标记)
//! → `Collapse`(语义聚类合并,同域条目合并)→ `Autocompact`(轻模型摘要,
//! 本任务降级为规则式摘要:按重要性分桶 + 最高分保留,零 LLM 调用)
//!
//! # 阈值表(ADR-119,手册 §10.5 level_for 骨架)
//! `ratio = tokens / budget`:
//! - `ratio ≥ 1.3` → `Snip`(超预算最严重,先做廉价规则去重截断)
//! - `1.15 ≤ ratio < 1.3` → `Microcompact`
//! - `1.0 ≤ ratio < 1.15` → `Collapse`
//! - `ratio < 1.0` → `Autocompact`(骨架兜底分支;管线入口在 `ratio ≤ 1.0` 时
//!   直接判定「无需压缩」提前返回,故该分支仅对直接调用 `level_for` 可见)
//!
//! # 管线语义
//! - **每级后检查预算,够即停**:从 `level_for(ratio)` 起始级沿链序逐级渐进,
//!   任一级达到预算即停止(不一次到位 → 信息悬崖消除,ADR-119 决策 10)。
//! - **分组截断重试 ≤3**:仍超预算则升级到更激进一级,最多重试 3 次
//!   (Claude 三级降级链模式);仍超则接受链末端(最激进)一级的结果。
//! - **from 模式保前缀**:`compress` 复用原前缀 + 尾追加压缩指令,前缀逐字节
//!   不变(缓存前缀不失效)。
//! - **thinking 全程保留**:`split_thinking` → 压缩 body → `rejoin` 原样回填
//!   (T-02,见 `crate::preserve`)。
//! - **Collapse 复用共享索引**:语义聚类直接读 `SharedSemanticIndex`(IndexShare,
//!   避免重复计算),合并结果写回索引供跨层复用。
//!
//! # 降级差异声明(本任务范围)
//! - `Microcompact` 的「签名化」:完整签名化应把内容替换为可重建签名;本任务以
//!   「去重 + 截断 + 可逆标记(记录丢弃条目 id)」替代,保留条目内容不变,
//!   丢弃集合可据标记重建(降级为工程可用实现,注释说明与手册语义差异)。
//! - `Autocompact` 的「轻模型摘要」:手册要求轻量模型语义摘要;本任务以
//!   「重要性分桶 + 最高分保留」的规则式摘要替代,零 LLM 调用(红线),注释说明差异。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use nexus_core::CLV;

use crate::compressor::ContextCompressor;
use crate::preserve::{rejoin, split_thinking, ConversationContext};
use crate::semantic_index::{SemanticDomain, SemanticEntry, SharedSemanticIndex};
use crate::types::{ContextEntry, HcwConfig};

/// 四级压缩级别 — 渐进增强链(轻 → 激进)
///
/// WHY 链序 = 信息保真度降序:Snip 仅移除重复/低分条目(保真最高),
/// Autocompact 只留高分桶(保真最低);逐级升级避免一步到位的信息悬崖。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompressionLevel {
    /// 规则去重截断:移除重复内容条目,仍超预算则按重要性截断
    Snip,
    /// 签名化(降级实现):去重 + 截断 + 可逆标记(记录丢弃 id)
    Microcompact,
    /// 语义聚类合并:同域(file_id)条目合并(读共享索引复用历史结果)
    Collapse,
    /// 规则式摘要(降级实现):按重要性分桶,只保留最高分桶
    Autocompact,
}

impl CompressionLevel {
    /// 级别名(日志与压缩指令)
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Snip => "Snip",
            Self::Microcompact => "Microcompact",
            Self::Collapse => "Collapse",
            Self::Autocompact => "Autocompact",
        }
    }

    /// 链序(轻 → 激进):Snip < Microcompact < Collapse < Autocompact
    #[must_use]
    pub fn order(&self) -> u8 {
        match self {
            Self::Snip => 0,
            Self::Microcompact => 1,
            Self::Collapse => 2,
            Self::Autocompact => 3,
        }
    }

    /// 链中下一级(更激进);`Autocompact` 为链末端返回 None
    #[must_use]
    pub fn next(&self) -> Option<Self> {
        match self {
            Self::Snip => Some(Self::Microcompact),
            Self::Microcompact => Some(Self::Collapse),
            Self::Collapse => Some(Self::Autocompact),
            Self::Autocompact => None,
        }
    }
}

/// ADR-119 阈值表 — 按 `ratio = tokens / budget` 选择起始压缩级别
///
/// WHY 骨架逐字遵循手册 §10.5(1.3/1.15/1.0 三档):
/// 超 1.3 用 Snip(廉价规则去重截断起步)、超 1.15 用 Microcompact、
/// 超 1.0 用 Collapse、其余(≤1.0,含 NaN)用 Autocompact 兜底。
/// 管线入口在 `ratio ≤ 1.0` 时提前判定「无需压缩」,此分支仅供直接调用
/// 与骨架对齐(见模块注释)。
#[must_use]
pub fn level_for(ratio: f64) -> CompressionLevel {
    match ratio {
        r if r >= 1.3 => CompressionLevel::Snip,
        r if r >= 1.15 => CompressionLevel::Microcompact,
        r if r >= 1.0 => CompressionLevel::Collapse,
        // NaN 等异常比值走兜底(防御性;管线入口不会以 ratio<1 进入压缩)
        _ => CompressionLevel::Autocompact,
    }
}

/// 分组截断重试上限 — 仍超预算时最多升级重试 3 次(Claude 三级降级链模式)
///
/// WHY 3:起始级 + 3 次重试 = 覆盖完整四级链(4 次尝试);超过即接受链末端结果。
pub const MAX_RETRIES: usize = 3;

/// 重要性分桶阈值 — Autocompact 规则式摘要的分桶边界
///
/// WHY 0.6/0.3:重要性评分 ∈ [0,1](recency/frequency/relevance 归一化),
/// ≥0.6 为强相关(高桶),<0.3 为弱相关(低桶);纯规则、确定性、零 LLM。
pub(crate) const HIGH_SCORE_BUCKET: f32 = 0.6;
#[allow(dead_code)] // 分桶预留:低分桶阈值(与 HIGH_SCORE_BUCKET 对偶,诊断可视化用)
pub(crate) const LOW_SCORE_BUCKET: f32 = 0.3;

/// 单级产出的可逆标记 — Microcompact 签名化降级实现的「可逆性」载体
///
/// 记录某级压缩丢弃的条目 id,配合保留内容可重建丢弃集合
/// (与完整签名化的差异见模块注释)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelMarker {
    /// 产生该标记的级别
    pub level: CompressionLevel,
    /// 标记内容,如 `dropped=e-2,e-3`
    pub note: String,
}

/// 四级渐进链压缩报告
#[derive(Debug, Clone, PartialEq)]
pub struct ChainReport {
    /// 压缩前总 token
    pub original_tokens: usize,
    /// 压缩后总 token
    pub compressed_tokens: usize,
    /// 预算
    pub budget: usize,
    /// ratio = original / budget(预算为 0 时按 1 计算分母)
    pub ratio: f64,
    /// 实际执行的起始级别(None = 无需压缩)
    pub level: Option<CompressionLevel>,
    /// 升级重试次数(≤ MAX_RETRIES)
    pub retries: usize,
    /// 各级的可逆标记(如 Microcompact 的 dropped 列表)
    pub markers: Vec<LevelMarker>,
    /// 压缩后保留条目(预算不足时保底单条,见 ContextCompressor fallback)
    pub retained_entries: Vec<Arc<ContextEntry>>,
}

impl ChainReport {
    /// 压缩比 = original / compressed(compressed=0 取 f64::MAX,防除零)
    #[must_use]
    pub fn compression_ratio(&self) -> f64 {
        if self.compressed_tokens > 0 {
            self.original_tokens as f64 / self.compressed_tokens as f64
        } else {
            f64::MAX
        }
    }

    /// token 降幅百分比 = (original - compressed) / original × 100
    ///
    /// 门禁口径:合成 100 轮会话 token_reduction_pct ≥ 40%。
    #[must_use]
    pub fn token_reduction_pct(&self) -> f64 {
        if self.original_tokens == 0 {
            0.0
        } else {
            (self.original_tokens - self.compressed_tokens) as f64 / self.original_tokens as f64
                * 100.0
        }
    }
}

/// Microcompact 级输出 — 条目 + 可逆标记
#[derive(Debug, Clone, PartialEq)]
pub struct MicrocompactOutcome {
    /// 压缩后条目(保留者内容逐字节不变)
    pub entries: Vec<Arc<ContextEntry>>,
    /// 被丢弃条目的 id 集合(可逆标记:可据标记重建丢弃集合)
    pub dropped_ids: Vec<String>,
}

/// from 模式完整输出 — 前缀逐字节不变 + thinking 原样 + 正文压缩后
#[derive(Debug, Clone, PartialEq)]
pub struct CscOutput {
    /// 压缩后的完整上下文(prefix/thinking 与原输入逐字节一致)
    pub context: ConversationContext,
    /// 尾追加压缩指令(如 `[csc:level=Snip,ratio=1.50,...]`;无需压缩时为空)
    pub directive: String,
    /// 四级链压缩报告
    pub report: ChainReport,
}

/// CSC 四级渐进压缩管线 — 无内部可变状态,`Send + Sync`
///
/// 持有共享语义索引(IndexShare:Collapse 级读/写索引,跨层复用)与
/// `HcwConfig`(评分权重 + 并行开关,复用 Phase 1 T14 并行注入)。
pub struct CompressionPipeline {
    config: HcwConfig,
    index: Arc<SharedSemanticIndex>,
}

impl CompressionPipeline {
    /// 默认配置 + 新建共享索引
    #[must_use]
    pub fn new(config: HcwConfig) -> Self {
        Self {
            config,
            index: Arc::new(SharedSemanticIndex::new()),
        }
    }

    /// 指定配置与共享索引(跨层共享:mlc-engine P2-T6 消费同一索引)
    #[must_use]
    pub fn with_index(config: HcwConfig, index: Arc<SharedSemanticIndex>) -> Self {
        Self { config, index }
    }

    /// 指定配置 + 共享索引 + 事件总线（P5-T2 扩量批次 2,D-Q6 向后兼容扩展）
    ///
    /// # P5-T2 分片扩量（ADR-153 Go 续批）
    /// ContextWindowSwitched/ContextCompressed 高频非 Critical 事件→ 分片扇出;
    /// 幂等 + 无 runtime 时 Err 降级回单流（零回归,P1-T12 先例）。
    /// 既有 `new`/`with_index` 签名不变（零破坏性变更,mlc P2-T6 先例）。
    #[must_use]
    pub fn with_event_bus(
        config: HcwConfig,
        index: Arc<SharedSemanticIndex>,
        event_bus: &event_bus::EventBus,
    ) -> Self {
        let _ = event_bus.enable_sharding(event_bus::DEFAULT_SHARD_COUNT);
        Self { config, index }
    }

    /// 共享索引引用(供调用方跨层共享/消费)
    #[must_use]
    pub fn index(&self) -> Arc<SharedSemanticIndex> {
        Arc::clone(&self.index)
    }

    // ============================================================
    // 四级各自语义(公开 API,供单独调用与测试)
    // ============================================================

    /// Snip — 规则去重截断:移除内容逐字节相同的条目(保留首见者),
    /// 仍超预算则按重要性评分截断(复用 `ContextCompressor` 的 importance-top-n,
    /// 含 Phase 1 T14 并行评分,不重复造轮)
    ///
    /// WHY 去重保留首见者:内容相同 → 信息等价,首见者保持插入顺序确定性(Ω₂)。
    pub fn snip(
        &self,
        entries: &[Arc<ContextEntry>],
        budget: usize,
        task_clv: Option<&CLV>,
        now: DateTime<Utc>,
    ) -> Vec<Arc<ContextEntry>> {
        let deduped = dedup_by_content(entries);
        if within_budget(&deduped, budget) {
            return deduped;
        }
        // 截断:既有 importance-top-n(贪心保留至预算,保底单条,避免空上下文)
        let report = ContextCompressor::compress(&self.config, &deduped, budget, task_clv, now);
        report.retained_entries
    }

    /// Microcompact — 签名化的降级实现:去重 + 截断 + 可逆标记
    ///
    /// # 与「完整签名化」的差异(WHY 降级)
    /// 完整签名化将内容替换为可重建签名(内容不保留原文);本任务以
    /// 「保留条目内容逐字节不变 + 记录丢弃 id 集合」替代 —— 内容不变,
    /// 丢弃集合可据 `dropped_ids` 重建,是签名化的可逆工程近似。
    pub fn microcompact(
        &self,
        entries: &[Arc<ContextEntry>],
        budget: usize,
        task_clv: Option<&CLV>,
        now: DateTime<Utc>,
    ) -> MicrocompactOutcome {
        // 记录去重丢弃的 id(与 dedup 相同的首见语义)
        let mut dropped_ids: Vec<String> = Vec::new();
        let mut seen: HashSet<Arc<str>> = HashSet::new();
        for e in entries {
            if !seen.insert(Arc::clone(&e.content)) {
                dropped_ids.push(e.id.clone());
            }
        }
        let deduped = dedup_by_content(entries);
        if !within_budget(&deduped, budget) {
            // 仍超预算 → 截断,记录截断丢弃的 id
            let report = ContextCompressor::compress(&self.config, &deduped, budget, task_clv, now);
            let kept: HashSet<&str> = report
                .retained_entries
                .iter()
                .map(|e| e.id.as_str())
                .collect();
            for e in &deduped {
                if !kept.contains(e.id.as_str()) {
                    dropped_ids.push(e.id.clone());
                }
            }
            return MicrocompactOutcome {
                entries: report.retained_entries,
                dropped_ids,
            };
        }
        MicrocompactOutcome {
            entries: deduped,
            dropped_ids,
        }
    }

    /// Collapse — 语义聚类合并:同域(file_id)条目合并为一个条目
    ///
    /// # IndexShare 语义(直接读索引,避免重复计算)
    /// - 合并前先查 `SharedSemanticIndex::Symbol` 域:命中则以索引中的历史合并
    ///   结果为基线(内容 seed + token 基线),仅合并新增内容 —— 历史合并计算
    ///   被复用,不重复拼接。
    /// - 未命中则全量合并,并写回索引(跨层共享:后续 Collapse / mlc-engine
    ///   P2-T6 直接复用)。
    ///
    /// # 单调性保证
    /// 合并 token = 唯一内容 token 之和,并对当前组总和取 min(任意输入下
    /// 压缩后 token ≤ 压缩前,proptest 锁定)。
    ///
    /// # 索引语义边界(WHY 注释)
    /// 索引按「会话追加式」语义设计(压缩请求复用原前缀,正文只增不改);
    /// 索引键建议按会话命名空间隔离(生产接线由 P2-T6 负责),跨会话复用
    /// 需调用方自行清理索引。
    pub fn collapse(&self, entries: &[Arc<ContextEntry>]) -> Vec<Arc<ContextEntry>> {
        // 按 file_id 分组,保持首见顺序(确定性:HashMap 不参与输出顺序)
        let mut order: Vec<&str> = Vec::new();
        let mut groups: HashMap<&str, Vec<&Arc<ContextEntry>>> = HashMap::new();
        for e in entries {
            match groups.get_mut(e.file_id.as_str()) {
                Some(list) => list.push(e),
                None => {
                    groups.insert(e.file_id.as_str(), vec![e]);
                    order.push(e.file_id.as_str());
                }
            }
        }
        let mut out: Vec<Arc<ContextEntry>> = Vec::with_capacity(order.len());
        for file_id in order {
            if let Some(group) = groups.get(file_id) {
                let merged = self.merge_group(file_id, group);
                out.push(Arc::new(merged));
            }
        }
        out
    }

    /// Autocompact — 规则式摘要(降级实现):按重要性分桶,只保留最高分桶
    ///
    /// # 与「轻模型摘要」的差异(WHY 降级)
    /// 手册要求轻量模型语义摘要(生成式);本任务以「重要性分桶 + 最高分保留」
    /// 的规则式摘要替代,零 LLM 调用(红线),确定性(Ω₂)。
    ///
    /// 分桶:score ≥ 0.6 高桶 / [0.3, 0.6) 中桶 / < 0.3 低桶;
    /// 高桶非空 → 只保留高桶;高桶空 → 保留最高分单条(避免空上下文)。
    pub fn autocompact(
        &self,
        entries: &[Arc<ContextEntry>],
        budget: usize,
        task_clv: Option<&CLV>,
        now: DateTime<Utc>,
    ) -> Vec<Arc<ContextEntry>> {
        if within_budget(entries, budget) {
            return entries.to_vec();
        }
        let scores = score_all(&self.config, entries, task_clv, now);
        let mut best: Option<(&Arc<ContextEntry>, f32)> = None;
        let mut high_bucket: Vec<Arc<ContextEntry>> = Vec::new();
        for (e, s) in entries.iter().zip(&scores) {
            // NaN 防御:NaN 比较恒 false → 不进高桶;best 选择用 partial_cmp 保序
            if *s >= HIGH_SCORE_BUCKET {
                high_bucket.push(Arc::clone(e));
            }
            if best.is_none_or(|(_, bs)| (*s).partial_cmp(&bs) == Some(std::cmp::Ordering::Greater))
            {
                best = Some((e, *s));
            }
        }
        if !high_bucket.is_empty() {
            return high_bucket;
        }
        // 高桶空:保留最高分单条(规则式摘要的最低保底,避免空上下文)
        match best {
            Some((e, _)) => vec![Arc::clone(e)],
            None => Vec::new(),
        }
    }

    // ============================================================
    // 四级渐进链
    // ============================================================

    /// 四级渐进链入口 — 压缩正文(thinking 由调用方经 `compress` 保护)
    ///
    /// # 流程(每级后检查预算,够即停;渐进增强,信息悬崖消除)
    /// 1. `ratio ≤ 1.0` 或空输入 → 无需压缩(level = None,retries = 0)
    /// 2. 起始级 = `level_for(ratio)`,逐级应用,每级后查预算
    /// 3. 仍超预算 → 升级到链序下一级(更激进),最多重试 `MAX_RETRIES` 次
    /// 4. 重试耗尽仍超 → 接受链末端(最激进)一级的结果(「接受最低级结果」,
    ///    即降级链末端的最终产出;此时调用方可触发窗口升级兜底)
    ///
    /// # 确定性(Ω₂)
    /// 纯规则 + 时间/随机经参数注入(`now` 由调用方固定),同输入同结果。
    pub fn compress_body(
        &self,
        entries: &[Arc<ContextEntry>],
        budget: usize,
        task_clv: Option<&CLV>,
        now: DateTime<Utc>,
    ) -> ChainReport {
        let original_tokens = total_tokens(entries);
        // 预算已满足(含空输入):无需压缩,原样返回(level=None 供调用方识别)
        if entries.is_empty() || original_tokens <= budget {
            return ChainReport {
                original_tokens,
                compressed_tokens: original_tokens,
                budget,
                ratio: ratio_of(original_tokens, budget),
                level: None,
                retries: 0,
                markers: Vec::new(),
                retained_entries: entries.to_vec(),
            };
        }
        let ratio = ratio_of(original_tokens, budget);
        let start = level_for(ratio);
        let mut current: Vec<Arc<ContextEntry>> = entries.to_vec();
        let mut markers: Vec<LevelMarker> = Vec::new();
        let mut retries = 0usize;
        let mut accepted: Option<CompressionLevel> = None;
        let mut lvl = Some(start);
        while let Some(level) = lvl {
            // 应用当前级(渐进:输入为上一级输出,信息保真逐级下降)
            let (reduced, marker_note) = self.apply_level(level, &current, budget, task_clv, now);
            current = reduced;
            accepted = Some(level);
            if let Some(note) = marker_note {
                markers.push(LevelMarker { level, note });
            }
            // 够即停:预算满足则立即停止(渐进增强的核心:不一次到位)
            if within_budget(&current, budget) {
                break;
            }
            // 分组截断重试 ≤3:仍超预算则升级到更激进一级
            if retries >= MAX_RETRIES {
                break;
            }
            retries += 1;
            lvl = level.next();
        }
        let compressed_tokens = total_tokens(&current);
        ChainReport {
            original_tokens,
            compressed_tokens,
            budget,
            ratio,
            level: accepted,
            retries,
            markers,
            retained_entries: current,
        }
    }

    /// 完整管线入口 — from 模式保前缀 + thinking 全程保留(T-02)
    ///
    /// 流程:`split_thinking`(剥离 thinking)→ `compress_body`(压缩正文)→
    /// `rejoin`(前缀逐字节不变 + thinking 原样回填)→ 尾追加压缩指令。
    ///
    /// # 保证
    /// - `output.context.prefix` 与原前缀逐字节一致(静态层 token 序列不变,
    ///   缓存前缀不失效,v4.0 WI-12)
    /// - `output.context.thinking` 与输入逐字节一致(门禁:thinking 链完整率 100%)
    /// - `output.directive` 为尾追加压缩指令(无需压缩时为空串)
    pub fn compress(
        &self,
        ctx: &ConversationContext,
        budget: usize,
        task_clv: Option<&CLV>,
        now: DateTime<Utc>,
    ) -> CscOutput {
        let (thinking, body, prefix) = split_thinking(ctx);
        let report = self.compress_body(&body, budget, task_clv, now);
        let context = rejoin(prefix, report.retained_entries.clone(), thinking);
        CscOutput {
            context,
            directive: build_directive(&report),
            report,
        }
    }

    /// 单级分发 — 返回(压缩后条目, 可逆标记)
    fn apply_level(
        &self,
        level: CompressionLevel,
        entries: &[Arc<ContextEntry>],
        budget: usize,
        task_clv: Option<&CLV>,
        now: DateTime<Utc>,
    ) -> (Vec<Arc<ContextEntry>>, Option<String>) {
        match level {
            CompressionLevel::Snip => (self.snip(entries, budget, task_clv, now), None),
            CompressionLevel::Microcompact => {
                let out = self.microcompact(entries, budget, task_clv, now);
                // 可逆标记:记录被丢弃 id 集合(签名化降级实现的可逆性载体)
                let note = if out.dropped_ids.is_empty() {
                    None
                } else {
                    Some(format!("dropped={}", out.dropped_ids.join(",")))
                };
                (out.entries, note)
            }
            CompressionLevel::Collapse => (self.collapse(entries), None),
            CompressionLevel::Autocompact => {
                (self.autocompact(entries, budget, task_clv, now), None)
            }
        }
    }

    /// 合并单一 file_id 组(语义聚类合并的核心)
    ///
    /// IndexShare:先读索引复用历史合并结果(seed),再合并新增唯一内容;
    /// 合并结果写回索引(跨层共享)。
    fn merge_group(&self, file_id: &str, group: &[&Arc<ContextEntry>]) -> ContextEntry {
        // 读索引(IndexShare:直接读已共享的合并结果,避免重复计算历史内容)
        let indexed = self.index.lookup(SemanticDomain::Symbol, file_id);
        let mut seen: HashSet<Arc<str>> = HashSet::new();
        let mut merged_content = String::new();
        let mut merged_token = 0usize;
        if let Some(idx) = indexed {
            // 命中:先检查输入是否全部被历史合并内容覆盖(会话追加式语义)
            // WHY 内容级子串包含:索引只存合并拼接串,无法还原原始内容集合;
            // 全部覆盖 = 无新增 → 直接复用索引结果(幂等,第二轮与第一轮逐字段一致)
            let all_covered = group
                .iter()
                .all(|e| idx.payload.contains(e.content.as_ref()));
            if all_covered {
                let latest = group
                    .iter()
                    .map(|e| e.last_accessed_at)
                    .max()
                    .unwrap_or(DateTime::<Utc>::MIN_UTC);
                return ContextEntry {
                    id: format!("merge:{file_id}"),
                    file_id: file_id.to_string(),
                    content: Arc::from(idx.payload.as_str()),
                    token_size: idx.meta as usize,
                    access_count: group.iter().map(|e| e.access_count).sum(),
                    last_accessed_at: latest,
                    created_at: latest,
                    clv: group.first().and_then(|e| e.clv.clone()),
                };
            }
            // 有新增内容:以历史合并结果为基线继续合并(历史内容 ⊆ 当前内容)
            seen.insert(Arc::from(idx.payload.as_str()));
            merged_content = idx.payload.clone();
            merged_token = idx.meta as usize;
        }
        let mut group_sum = 0usize;
        let mut access_sum = 0u32;
        let mut latest_at: Option<DateTime<Utc>> = None;
        for e in group {
            group_sum += e.token_size;
            access_sum = access_sum.saturating_add(e.access_count);
            // 时间取组内最新(确定性,不依赖时钟)
            if latest_at.is_none_or(|m| e.last_accessed_at > m) {
                latest_at = Some(e.last_accessed_at);
            }
            // 内容级去重:相同内容只计一次(合并 token ≤ 组总和 → 单调性)
            if seen.insert(Arc::clone(&e.content)) {
                merged_content.push_str(&e.content);
                merged_token = merged_token.saturating_add(e.token_size);
            }
        }
        // token 上限钳制:任意输入(含索引 seed 来自不同内容集)下保证
        // 合并 token ≤ 组总和 → 压缩后 token ≤ 压缩前(proptest 不变量)
        merged_token = merged_token.min(group_sum);
        let latest = latest_at.unwrap_or(DateTime::<Utc>::MIN_UTC);
        let merged = ContextEntry {
            id: format!("merge:{file_id}"),
            file_id: file_id.to_string(),
            content: Arc::from(merged_content),
            token_size: merged_token,
            access_count: access_sum,
            last_accessed_at: latest,
            created_at: latest,
            clv: group.first().and_then(|e| e.clv.clone()),
        };
        // 写回索引(IndexShare:跨层共享本次合并结果,后续直接复用)
        self.index.insert(
            SemanticDomain::Symbol,
            SemanticEntry::new(
                file_id,
                SemanticDomain::Symbol,
                merged.content.to_string(),
                merged_token as u64,
            ),
        );
        merged
    }
}

/// 计算 ratio = tokens / budget(预算为 0 时按 1 计算分母,防 inf)
///
/// WHY budget=0 语义为「尽量压缩」,ratio 仅用于选起始级(≥1.3 → Snip),
/// 分母钳 1 不影响选级方向(0 预算必然超预算)。
#[must_use]
pub(crate) fn ratio_of(tokens: usize, budget: usize) -> f64 {
    tokens as f64 / budget.max(1) as f64
}

/// 计算全部条目的重要性评分 — 复用 Phase 1 T14 并行注入(不重复造轮)
///
/// WHY 走 `crate::parallel::score_entries`:并行开启且 n ≥ 阈值时经
/// ComputeBridge(Rayon)段间并行,否则串行;两种路径逐元素一致(既有断言锁定)。
#[must_use]
fn score_all(
    config: &HcwConfig,
    entries: &[Arc<ContextEntry>],
    task_clv: Option<&CLV>,
    now: DateTime<Utc>,
) -> Vec<f32> {
    let weights = config.selector_policy.weights();
    let max_access_count = entries
        .iter()
        .map(|e| e.access_count)
        .max()
        .unwrap_or(0)
        .max(1) as f32;
    let oldest = entries
        .iter()
        .map(|e| e.last_accessed_at)
        .min()
        .unwrap_or(now);
    let newest = entries
        .iter()
        .map(|e| e.last_accessed_at)
        .max()
        .unwrap_or(now);
    let time_span_ms = (newest - oldest).num_milliseconds().max(1) as f32;
    crate::parallel::score_entries(
        entries,
        weights,
        task_clv,
        now,
        max_access_count,
        time_span_ms,
        config.parallel_compress,
    )
}

/// 内容级去重 — 逐字节相同的内容只保留首见条目(插入顺序保持,确定性 Ω₂)
#[must_use]
fn dedup_by_content(entries: &[Arc<ContextEntry>]) -> Vec<Arc<ContextEntry>> {
    let mut seen: HashSet<Arc<str>> = HashSet::new();
    let mut out: Vec<Arc<ContextEntry>> = Vec::with_capacity(entries.len());
    for e in entries {
        if seen.insert(Arc::clone(&e.content)) {
            out.push(Arc::clone(e));
        }
    }
    out
}

/// 总 token 数
#[must_use]
pub(crate) fn total_tokens(entries: &[Arc<ContextEntry>]) -> usize {
    entries.iter().map(|e| e.token_size).sum()
}

/// 预算检查 — 压缩后 token 总量 ≤ budget 即视为达标(够即停判定)
#[must_use]
pub(crate) fn within_budget(entries: &[Arc<ContextEntry>], budget: usize) -> bool {
    total_tokens(entries) <= budget
}

/// 构建尾追加压缩指令(如 `[csc:level=Snip,ratio=1.50,original=1000,compressed=600,retries=0]`)
///
/// WHY 指令在尾部(不动前缀):缓存前缀不失效;指令携带级别/降幅/重试/可逆标记,
/// 供下游(会话存储/调试)还原压缩信息。
#[must_use]
fn build_directive(report: &ChainReport) -> String {
    let Some(level) = report.level else {
        return String::new();
    };
    let mut d = format!(
        "[csc:level={},ratio={:.2},original={},compressed={},retries={}",
        level.as_str(),
        report.ratio,
        report.original_tokens,
        report.compressed_tokens,
        report.retries
    );
    for m in &report.markers {
        d.push(',');
        d.push_str(&m.note);
    }
    d.push(']');
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preserve::ThinkingBlock;
    use proptest::prelude::*;

    /// 确定性测试基元:固定 now + 构造条目(时间/权重全部注入,Ω₂)
    fn fixed_now() -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp_millis(1_700_000_000_000)
            .unwrap_or(DateTime::<Utc>::MIN_UTC)
    }

    /// 构造条目:可指定内容/域/访问频次,last_accessed_at 全部对齐到固定 now
    fn make_entry(
        id: &str,
        file_id: &str,
        content: &str,
        token: usize,
        access: u32,
        age_ms: i64,
    ) -> Arc<ContextEntry> {
        let mut e = ContextEntry::new(id, file_id, content, token);
        e.access_count = access;
        e.last_accessed_at = fixed_now() - chrono::Duration::milliseconds(age_ms);
        Arc::new(e)
    }

    /// 快速构造(默认 0 频次/最新)
    fn entry(id: &str, file_id: &str, content: &str, token: usize) -> Arc<ContextEntry> {
        make_entry(id, file_id, content, token, 0, 0)
    }

    fn pipeline() -> CompressionPipeline {
        CompressionPipeline::new(HcwConfig::default())
    }

    fn ids(entries: &[Arc<ContextEntry>]) -> Vec<&str> {
        entries.iter().map(|e| e.id.as_str()).collect()
    }

    // ============================================================
    // 阈值表(1.3 / 1.15 / 1.0 边界邻域)
    // ============================================================

    #[test]
    fn test_level_for_boundaries() {
        // 上档边界:≥ 1.3 → Snip
        assert_eq!(level_for(1.3), CompressionLevel::Snip);
        assert_eq!(level_for(2.0), CompressionLevel::Snip);
        // (1.15, 1.3) → Microcompact
        assert_eq!(level_for(1.29), CompressionLevel::Microcompact);
        assert_eq!(level_for(1.2), CompressionLevel::Microcompact);
        // 1.15 下档边界 → Microcompact(≥ 1.15)
        assert_eq!(level_for(1.15), CompressionLevel::Microcompact);
        // (1.0, 1.15) → Collapse
        assert_eq!(level_for(1.149), CompressionLevel::Collapse);
        assert_eq!(level_for(1.05), CompressionLevel::Collapse);
        // 1.0 边界 → Collapse(≥ 1.0)
        assert_eq!(level_for(1.0), CompressionLevel::Collapse);
        // < 1.0 → Autocompact(骨架兜底;管线入口在 ≤1.0 提前返回)
        assert_eq!(level_for(0.99), CompressionLevel::Autocompact);
        assert_eq!(level_for(0.5), CompressionLevel::Autocompact);
        // NaN 防御 → Autocompact(_ 兜底分支)
        assert_eq!(level_for(f64::NAN), CompressionLevel::Autocompact);
    }

    #[test]
    fn test_level_chain_order() {
        // 链序:Snip → Microcompact → Collapse → Autocompact → None
        assert_eq!(
            CompressionLevel::Snip.next(),
            Some(CompressionLevel::Microcompact)
        );
        assert_eq!(
            CompressionLevel::Microcompact.next(),
            Some(CompressionLevel::Collapse)
        );
        assert_eq!(
            CompressionLevel::Collapse.next(),
            Some(CompressionLevel::Autocompact)
        );
        assert_eq!(CompressionLevel::Autocompact.next(), None);
        assert!(CompressionLevel::Snip.order() < CompressionLevel::Autocompact.order());
        assert_eq!(CompressionLevel::Snip.as_str(), "Snip");
        assert_eq!(CompressionLevel::Autocompact.as_str(), "Autocompact");
    }

    // ============================================================
    // 预算检查够即停
    // ============================================================

    #[test]
    fn test_no_compression_when_within_budget() {
        // ratio ≤ 1.0:无需压缩,level=None,条目原样
        let entries = vec![entry("e-1", "f-1", "A", 50), entry("e-2", "f-2", "B", 50)];
        let report = pipeline().compress_body(&entries, 100, None, fixed_now());
        assert_eq!(report.level, None);
        assert_eq!(report.retries, 0);
        assert_eq!(report.original_tokens, report.compressed_tokens);
        assert_eq!(ids(&report.retained_entries), vec!["e-1", "e-2"]);
    }

    #[test]
    fn test_budget_check_stops_early_low_ratio() {
        // 低 ratio(1.05 ∈ Collapse 档)且 Collapse 即达预算 → 不触发高等级:
        // 同域重复内容合并后达标,Autocompact 不得出现(渐进增强:够即停)
        let entries = vec![
            entry("e-1", "f-1", "A", 100),
            entry("e-2", "f-1", "A", 100), // 与 e-1 同内容同域 → 合并去重
            entry("e-3", "f-2", "B", 100),
        ];
        let report = pipeline().compress_body(&entries, 285, None, fixed_now());
        assert_eq!(
            report.level,
            Some(CompressionLevel::Collapse),
            "低 ratio 应从 Collapse 起步"
        );
        assert_eq!(report.retries, 0, "达标即停,不应重试");
        assert!(report.compressed_tokens <= 285, "压缩后必须满足预算");
        // 不触发高等级:Autocompact 未被执行(无其标记,级别停在 Collapse)
        assert!(report.markers.is_empty());
        // 合并语义:同域条目合并为一条(merge:f-1)
        assert_eq!(
            ids(&report.retained_entries),
            vec!["merge:f-1", "merge:f-2"]
        );
    }

    #[test]
    fn test_chain_escalates_to_autocompact_when_budget_unmet() {
        // 超预算场景(所有条目 > 预算):四级全部无法达标 → 链末端 Autocompact,
        // 重试计数 = MAX_RETRIES(≤3)
        let entries: Vec<Arc<ContextEntry>> = (0..5)
            .map(|i| {
                make_entry(
                    &format!("e-{i}"),
                    &format!("f-{i}"),
                    &format!("C-{i}"),
                    100,
                    i as u32,
                    i as i64,
                )
            })
            .collect();
        let report = pipeline().compress_body(&entries, 50, None, fixed_now());
        assert_eq!(
            report.level,
            Some(CompressionLevel::Autocompact),
            "应渐进到链末端"
        );
        assert_eq!(
            report.retries, MAX_RETRIES,
            "重试必须 ≤3(此处 3 次重试耗尽)"
        );
        // 接受链末端(最激进)结果:至少保底 1 条(不空上下文)
        assert!(!report.retained_entries.is_empty());
        assert!(report.compressed_tokens <= report.original_tokens);
    }

    // ============================================================
    // 四级各自语义
    // ============================================================

    #[test]
    fn test_snip_dedup_removes_duplicate_content() {
        // Snip 语义:重复行(内容逐字节相同)移除,保留内容唯一
        let entries = vec![
            entry("e-0", "f-0", "A", 100),
            entry("e-1", "f-1", "A", 100), // 重复内容
            entry("e-2", "f-2", "B", 100),
            entry("e-3", "f-3", "C", 100),
            entry("e-4", "f-4", "D", 100),
        ];
        // ratio = 500/200 = 2.5 ≥ 1.3 → Snip 起步
        let report = pipeline().compress_body(&entries, 200, None, fixed_now());
        assert_eq!(report.level, Some(CompressionLevel::Snip));
        assert!(report.retained_entries.len() < 5, "去重后条目数必须减少");
        // 内容唯一性
        let mut contents: HashSet<&str> = HashSet::new();
        for e in &report.retained_entries {
            assert!(
                contents.insert(&e.content),
                "保留内容不得重复(重复行已移除)"
            );
        }
        assert!(report.compressed_tokens <= 200, "去重 + 截断后必须满足预算");
    }

    #[test]
    fn test_microcompact_content_unchanged_and_reversible_marker() {
        // Microcompact 语义:保留条目内容逐字节不变 + 可逆标记记录丢弃 id
        let entries = vec![
            entry("e-0", "f-0", "C0", 100),
            entry("e-1", "f-1", "C1", 100),
            entry("e-2", "f-2", "C1", 100), // 重复内容 → 被去重丢弃
            entry("e-3", "f-3", "C3", 100),
            entry("e-4", "f-4", "C4", 100),
        ];
        // ratio = 500/400 = 1.25 ∈ [1.15, 1.3) → Microcompact 起步
        let report = pipeline().compress_body(&entries, 400, None, fixed_now());
        assert_eq!(report.level, Some(CompressionLevel::Microcompact));
        // 可逆标记:标记了被丢弃的 id
        let marker = report
            .markers
            .iter()
            .find(|m| m.level == CompressionLevel::Microcompact)
            .expect("Microcompact 必须产出可逆标记");
        assert!(
            marker.note.contains("e-2"),
            "重复内容 e-2 应被标记为丢弃: {}",
            marker.note
        );
        // 保留条目内容逐字节不变(签名化降级:内容不变,可逆性在标记)
        for e in &report.retained_entries {
            let original = entries
                .iter()
                .find(|o| o.id == e.id)
                .expect("保留条目必须来自原集合");
            assert_eq!(
                e.content.as_bytes(),
                original.content.as_bytes(),
                "保留条目内容必须逐字节不变"
            );
        }
        // 可重建性:保留集合 + 标记丢弃集合 = 原集合(丢弃集可重建)
        let dropped: Vec<&str> = entries
            .iter()
            .map(|e| e.id.as_str())
            .filter(|id| !report.retained_entries.iter().any(|r| r.id == *id))
            .collect();
        for id in dropped {
            assert!(
                marker.note.contains(id),
                "丢弃 id {id} 必须出现在可逆标记中"
            );
        }
        assert!(report.compressed_tokens <= 400);
    }

    #[test]
    fn test_collapse_merges_same_domain_entries() {
        // Collapse 语义:同域(file_id)条目合并为一条,合并 token ≤ 组总和
        let entries = vec![
            entry("e-1", "f-1", "A", 100),
            entry("e-2", "f-1", "A", 100), // 同域 + 同内容 → 合并去重
            entry("e-3", "f-1", "B", 50),  // 同域新增内容 → 并入
            entry("e-4", "f-2", "C", 100), // 独立域
        ];
        let out = pipeline().collapse(&entries);
        assert_eq!(out.len(), 2, "两个域 → 两条合并条目");
        assert_eq!(out[0].id, "merge:f-1");
        assert_eq!(out[0].file_id, "f-1");
        // 合并 token = 唯一内容 token 之和(A=100 + B=50)= 150 ≤ 组总和 250
        assert_eq!(out[0].token_size, 150);
        assert_eq!(out[1].id, "merge:f-2");
        assert_eq!(out[1].token_size, 100);
    }

    #[test]
    fn test_collapse_reuses_index_second_round_idempotent() {
        // IndexShare:第一轮写索引,第二轮直接读索引复用 → 结果幂等一致
        let entries = vec![
            entry("e-1", "f-1", "A", 100),
            entry("e-2", "f-1", "A", 100),
            entry("e-3", "f-1", "B", 50),
        ];
        let p = pipeline();
        let first = p.collapse(&entries);
        // 索引已写入(跨层共享)
        let indexed = p.index().lookup(SemanticDomain::Symbol, "f-1");
        assert!(indexed.is_some(), "合并结果必须写回共享索引");
        let second = p.collapse(&entries);
        assert_eq!(
            first, second,
            "第二轮(索引命中路径)结果必须与第一轮一致(幂等)"
        );
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].token_size, 150);
    }

    #[test]
    fn test_autocompact_keeps_highest_score_bucket() {
        // Autocompact 语义:规则式摘要 — 按重要性分桶,只保留最高分桶
        // 频次主导评分(全部同时间无 CLV → recency/relevance 相同):
        // access=100 → score 0.85(高桶);其余 → < 0.6(低桶)
        let _entries: Vec<Arc<ContextEntry>> = (0..6)
            .map(|i| {
                make_entry(
                    &format!("e-{i}"),
                    &format!("f-{i}"),
                    &format!("K-{i}"),
                    100,
                    i * 5,
                    0,
                )
            })
            .collect();
        // 最高频条目 access = 5*5 = 25 → freq=1 → score = 0.4 + 0.3 + 0.15 = 0.85
        // 其余 freq ≤ 0.8 → score ≤ 0.4 + 0.24 + 0.15 = 0.79 —— 全部 ≥ 0.6?
        // 校正:让低频条目显著低于阈值(用 access 1..=5 而非 0,5,10..)
        let entries = vec![
            make_entry("e-lo-0", "f-0", "K0", 100, 0, 0),
            make_entry("e-lo-1", "f-1", "K1", 100, 1, 0),
            make_entry("e-lo-2", "f-2", "K2", 100, 2, 0),
            make_entry("e-lo-3", "f-3", "K3", 100, 3, 0),
            make_entry("e-lo-4", "f-4", "K4", 100, 4, 0),
            make_entry("e-hi", "f-5", "K5", 100, 1000, 0),
        ];
        let out = pipeline().autocompact(&entries, 50, None, fixed_now());
        // 高桶 = {e-hi}(score = 0.4 + 0.3×1 + 0.15 = 0.85 ≥ 0.6;其余 < 0.6)
        assert_eq!(ids(&out), vec!["e-hi"], "规则式摘要应只保留最高分桶");
    }

    #[test]
    fn test_autocompact_best_fallback_when_no_high_bucket() {
        // 无高桶时保留最高分单条(避免空上下文)
        let _entries: Vec<Arc<ContextEntry>> = (0..3)
            .map(|i| {
                make_entry(
                    &format!("e-{i}"),
                    &format!("f-{i}"),
                    &format!("L-{i}"),
                    100,
                    1,
                    0,
                )
            })
            .collect();
        // 全部 access=1 → score = 0.4 + 0.3×1/1 + 0.15 = 0.85 ≥ 0.6 —— 全部高桶!
        // 改用 access=0(全部低分):score = 0.4 + 0 + 0.15 = 0.55 < 0.6
        let entries = vec![
            make_entry("e-0", "f-0", "L0", 100, 0, 0),
            make_entry("e-1", "f-1", "L1", 100, 0, 0),
            make_entry("e-2", "f-2", "L2", 100, 0, 0),
        ];
        let out = pipeline().autocompact(&entries, 50, None, fixed_now());
        assert_eq!(out.len(), 1, "无高桶应保底最高分单条");
        assert_eq!(ids(&out), vec!["e-0"], "平分时保底首条(确定性)");
    }

    // ============================================================
    // from 模式保前缀 + thinking 完整率 100%
    // ============================================================

    #[test]
    fn test_from_mode_prefix_unchanged_after_compression() {
        // from 模式:压缩后前缀逐字节不变(静态层 token 序列一致 → 缓存前缀不失效)
        let entries: Vec<Arc<ContextEntry>> = (0..10)
            .map(|i| {
                make_entry(
                    &format!("e-{i}"),
                    &format!("f-{}", i % 2),
                    &format!("M-{i}"),
                    100,
                    i as u32,
                    i as i64,
                )
            })
            .collect();
        let prefix = "system: 你是 Chimera 助手,遵循下列约定:\n1. 不编造事实\n2. 引用需给出处\n";
        let ctx = ConversationContext::new(prefix, entries, Vec::new());
        let out = pipeline().compress(&ctx, 500, None, fixed_now());
        // 前缀逐字节一致
        assert_eq!(
            out.context.prefix.as_bytes(),
            prefix.as_bytes(),
            "from 模式:压缩后前缀必须逐字节不变"
        );
        assert_eq!(out.context.prefix, ctx.prefix);
        // 尾追加压缩指令(非空:确实发生了压缩)
        assert!(!out.directive.is_empty(), "发生压缩时必须产出尾追加指令");
        assert!(
            out.directive.starts_with("[csc:"),
            "指令格式: {}",
            out.directive
        );
        assert!(out.directive.ends_with(']'));
        assert!(out.directive.contains("original=1000"));
        assert!(out.directive.contains("compressed="));
    }

    #[test]
    fn test_no_compression_yields_empty_directive() {
        // 无需压缩时指令为空串(缓存前缀不失效 + 无多余尾注)
        let entries = vec![entry("e-1", "f-1", "A", 50)];
        let ctx = ConversationContext::new("prefix", entries, Vec::new());
        let out = pipeline().compress(&ctx, 100, None, fixed_now());
        assert_eq!(out.directive, "");
        assert_eq!(out.report.level, None);
        assert_eq!(out.context.prefix.as_bytes(), "prefix".as_bytes());
    }

    #[test]
    fn test_thinking_chain_integrity_100pct_through_compression() {
        // 门禁:thinking 链完整率 100% — 压缩全程逐字节一致(含真实压缩路径)
        let entries: Vec<Arc<ContextEntry>> = (0..10)
            .map(|i| {
                make_entry(
                    &format!("e-{i}"),
                    &format!("f-{i}"),
                    &format!("T-{i}"),
                    100,
                    i as u32,
                    i as i64,
                )
            })
            .collect();
        let thinking = vec![
            ThinkingBlock::new(1, "第一轮推理:识别目标是压缩上下文\n"),
            ThinkingBlock::new(2, "第二轮推理:评估阈值 1.3/1.15/1.0"),
            ThinkingBlock::new(3, "第三轮推理:选择 Snip 起步\n后续链式引用此结论"),
        ];
        let ctx = ConversationContext::new("prefix", entries, thinking.clone());
        let out = pipeline().compress(&ctx, 400, None, fixed_now());
        // 确实发生了压缩(否则测试无意义)
        assert!(out.report.level.is_some());
        assert!(out.report.compressed_tokens < out.report.original_tokens);
        // thinking 链完整率 100%:块数、链序、字节全部一致
        assert_eq!(out.context.thinking.len(), thinking.len());
        for (a, b) in out.context.thinking.iter().zip(thinking.iter()) {
            assert_eq!(a.id, b.id, "thinking 链序必须一致");
            assert_eq!(
                a.as_bytes(),
                b.as_bytes(),
                "thinking 块必须逐字节一致(压缩不触碰推理痕迹)"
            );
        }
        // 前缀同样逐字节不变
        assert_eq!(out.context.prefix.as_bytes(), "prefix".as_bytes());
    }

    // ============================================================
    // 确定性(Ω₂:同输入同结果)
    // ============================================================

    #[test]
    fn test_deterministic_same_input_same_output() {
        // 确定性:同输入(含固定 now)两次压缩结果逐项一致
        let entries: Vec<Arc<ContextEntry>> = (0..12)
            .map(|i| {
                make_entry(
                    &format!("e-{i}"),
                    &format!("f-{}", i % 3),
                    &format!("D-{}", i % 4),
                    100,
                    i as u32,
                    i as i64,
                )
            })
            .collect();
        let p = pipeline();
        let r1 = p.compress_body(&entries, 400, None, fixed_now());
        let r2 = p.compress_body(&entries, 400, None, fixed_now());
        assert_eq!(r1.level, r2.level);
        assert_eq!(r1.retries, r2.retries);
        assert_eq!(r1.compressed_tokens, r2.compressed_tokens);
        assert_eq!(
            ids(&r1.retained_entries),
            ids(&r2.retained_entries),
            "保留条目序列必须一致"
        );
        assert_eq!(r1.markers, r2.markers);
    }

    // ============================================================
    // proptest:任意上下文 → thinking 完整 + token 不增
    // ============================================================

    proptest! {
        #[test]
        fn prop_thinking_intact_and_tokens_non_increasing(
            n in 0usize..20,
            token_mod in 1usize..50,
            budget in 0usize..1000,
            dup_rate in 0usize..3,
        ) {
            // 构造任意上下文(内容部分重复 + 部分同域 + 频次/时间各异)
            let now = fixed_now();
            let mut entries: Vec<Arc<ContextEntry>> = Vec::with_capacity(n);
            for i in 0..n {
                let content = if i % (dup_rate + 1) == 0 {
                    format!("common-{}-{}", dup_rate, i % 3) // 部分重复内容
                } else {
                    format!("round-{i}-{}", token_mod)
                };
                let mut e = ContextEntry::new(
                    format!("e-{i}"),
                    format!("f-{}", i % 2),
                    content,
                    1 + (i * 7) % token_mod,
                );
                e.access_count = (i % 5) as u32;
                e.last_accessed_at = now - chrono::Duration::milliseconds(i as i64);
                entries.push(Arc::new(e));
            }
            let p = pipeline();
            let report = p.compress_body(&entries, budget, None, now);
            // 不变量 1:token 不增(压缩后 token ≤ 压缩前)
            prop_assert!(
                report.compressed_tokens <= report.original_tokens,
                "token 不增: {}(原) → {}(压缩), budget={}",
                report.original_tokens, report.compressed_tokens, budget
            );
            // 不变量 2:空输入不产生条目
            if n == 0 {
                prop_assert!(report.retained_entries.is_empty());
            }
            // 不变量 3:重试 ≤ MAX_RETRIES
            prop_assert!(report.retries <= MAX_RETRIES);

            // 不变量 4:thinking 链完整(完整管线 compress 路径逐字节一致)
            let thinking = vec![
                ThinkingBlock::new(1, "推理锚点 A\n"),
                ThinkingBlock::new(2, "推理锚点 B:链式引用"),
                ThinkingBlock::new(3, "推理锚点 C"),
            ];
            let ctx = ConversationContext::new("static-prefix", entries.clone(), thinking.clone());
            let out = p.compress(&ctx, budget, None, now);
            prop_assert_eq!(out.context.thinking.len(), thinking.len());
            for (a, b) in out.context.thinking.iter().zip(thinking.iter()) {
                prop_assert_eq!(a.id, b.id);
                prop_assert_eq!(a.as_bytes(), b.as_bytes(), "thinking 块必须逐字节一致");
            }
            // 不变量 5:from 模式前缀逐字节不变
            prop_assert_eq!(out.context.prefix.as_bytes(), "static-prefix".as_bytes());
            // 不变量 6:指令格式(发生压缩时)
            if report.level.is_some() {
                prop_assert!(out.directive.starts_with("[csc:"));
                prop_assert!(out.directive.ends_with(']'));
            } else {
                prop_assert_eq!(out.directive, "");
            }
        }
    }

    #[test]
    fn test_report_metrics() {
        // 报告指标口径:压缩比 / token 降幅百分比
        let entries = vec![entry("e-1", "f-1", "A", 100), entry("e-2", "f-2", "B", 100)];
        let report = pipeline().compress_body(&entries, 100, None, fixed_now());
        assert_eq!(report.compression_ratio(), 2.0);
        assert!((report.token_reduction_pct() - 50.0).abs() < 1e-6);
        // 零压缩:ratio = 1.0,降幅 0%
        let ok = pipeline().compress_body(&entries, 500, None, fixed_now());
        assert_eq!(ok.compression_ratio(), 1.0);
        assert_eq!(ok.token_reduction_pct(), 0.0);
    }
}
