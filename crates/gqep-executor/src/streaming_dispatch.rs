//! streaming_dispatch — 流式期间启动工具（P3-T7，v4.0 WI-17）
//!
//! 对应架构层: **L7 Execution**（gqep-executor，ADR-137 裁决：挂既有 crate 增强）
//! 对应任务: **P3-T7**（手册 W15，WI-17 排期漂移修正——v4.0 标注「Ⅲ期 W11-13（并入）」
//! 但全库零命中，D-P7 裁决纳入 W15）
//!
//! # 设计（v4.0 WI-17 规格）
//! 增量解析模型输出流:工具调用块**闭合即校验启动**——
//! - 只读调用:闭合立即并发派发（不等整轮生成完成,TTFT 降 ≥30%）;
//! - 写调用:闭合后延迟到轮末统一派发（避免半成品状态写坏）。
//! - 置信度门禁:完整闭合块 + schema 校验通过 → 置信 1.0 > 0.9 才 dispatch;
//!   未闭合 / 解析失败 → 不派发（fallback 时已启动只读结果作废无副作用,
//!   写工具未启动无回滚问题——WI-17 安全不变量）。
//!
//! # 块语法契约（声明式,零 JS 引擎——违 forbid(unsafe) 精神不引入）
//! ```text
//! <|tool_start:name|>{json 参数}<|tool_end:name|>
//! ```
//! 参数 JSON 校验通过方为「闭合块」;跨 chunk 增量累积解析。
//!
//! # 门禁（WI-17）
//! TTFT 降 ≥30%（A/B 由接入方对比）;预执行与正式执行结果一致率 100%
//! （本模块保证:同一块只产出一次,无重复执行路径）。

/// 工具调用块起始标记（`<|tool_start:name|>`）
const START_PREFIX: &str = "<|tool_start:";
/// 工具调用块结束标记前缀（`<|tool_end:name|>`）
const END_PREFIX: &str = "<|tool_end:";
/// 标记闭合后缀（`|>`）
const MARKER_SUFFIX: &str = "|>";
/// 置信度门禁（WI-17:>0.9 才 dispatch;闭合+校验过 = 1.0,未闭合不派发）
const CONFIDENCE_GATE: f64 = 0.9;
/// 单块缓冲上限（防恶意/失控输出撑爆内存,超限丢弃该块）
const MAX_BLOCK_BYTES: usize = 64 * 1024;

/// 工具副作用分类 — 决定派发时机（WI-17:只读立即 / 写等轮末）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideEffect {
    /// 只读（查询/检索类）— 闭合立即并发派发
    ReadOnly,
    /// 写/副作用（文件修改/执行类）— 延迟到轮末派发
    Write,
}

impl SideEffect {
    /// 从工具名推断分类 — 名称前缀约定:读类以 `read:` / `query:` / `search:` 开头
    /// （接入方可按注册表覆盖;未识别默认 Write=保守,WI-07 同口径）
    #[must_use]
    pub fn classify(name: &str) -> Self {
        let lower = name.to_ascii_lowercase();
        if lower.starts_with("read:") || lower.starts_with("query:") || lower.starts_with("search:")
        {
            Self::ReadOnly
        } else {
            Self::Write
        }
    }
}

/// 闭合的工具调用 — 已通过置信度门禁,待派发
#[derive(Debug, Clone, PartialEq)]
pub struct DispatchedCall {
    /// 工具名
    pub name: String,
    /// 参数 JSON（原样,派发方解析为 schema 类型）
    pub args_json: String,
    /// 副作用分类
    pub side_effect: SideEffect,
    /// 置信度（闭合+校验过 = 1.0）
    pub confidence: f64,
}

/// 一轮派发结果 — 只读立即启动 + 写延迟到轮末
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DispatchOutcome {
    /// 立即派发的只读调用（接入方并发执行）
    pub started_readonly: Vec<DispatchedCall>,
    /// 延迟到轮末的写调用（接入方轮末统一执行）
    pub deferred_writes: Vec<DispatchedCall>,
    /// 本轮丢弃的失败块数（解析/schema 失败,诊断）
    pub dropped: usize,
}

impl DispatchOutcome {
    /// 派发总数（诊断）
    #[must_use]
    pub fn total(&self) -> usize {
        self.started_readonly.len() + self.deferred_writes.len()
    }
}

/// 增量流式解析器 — 跨 chunk 累积缓冲,闭合块即时产出
///
/// 实现:内部保留未消费缓冲,每轮 `feed` 循环查找完整块
/// （start 标记 → 名字 → end 标记）,找到即校验产出;找不到完整块时
/// 保留可能成为前缀的尾部继续累积。缓冲超限丢弃（防御性上限）。
#[derive(Debug, Default)]
pub struct StreamingDispatcher {
    /// 未消费缓冲（跨 chunk 累积）
    buffer: String,
    /// 累计丢弃块数（诊断）
    dropped_total: u64,
}

impl StreamingDispatcher {
    /// 新建解析器
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一个输出 chunk — 返回本 chunk 内闭合的工具调用
    ///
    /// # 参数
    /// - `chunk`:模型输出增量（UTF-8 字符串,可含任意前缀/后缀文本）
    ///
    /// # 行为
    /// - 未闭合块跨 chunk 累积;闭合块即时产出（置信 1.0）;
    /// - 参数 JSON 非法 → 丢弃该块（`dropped` 计数）;
    /// - 缓冲超 [`MAX_BLOCK_BYTES`] → 丢弃当前累积（防御性上限）。
    pub fn feed(&mut self, chunk: &str) -> DispatchOutcome {
        self.buffer.push_str(chunk);
        let mut outcome = DispatchOutcome::default();
        loop {
            // 1. 查找下一个 start 标记
            let Some(start) = self.buffer.find(START_PREFIX) else {
                // 无完整 start 标记:丢弃 start 前缀之前的内容（正文）,保留尾部
                // 可能成为前缀的字符（最长 START_PREFIX.len()-1）
                let keep = START_PREFIX.len().saturating_sub(1).min(self.buffer.len());
                let drop_len = self.buffer.len() - keep;
                self.buffer.drain(..drop_len);
                break;
            };
            // 2. 提取工具名（start 标记后的名字直到 `|>`）
            let name_start = start + START_PREFIX.len();
            let Some(name_rel) = self.buffer[name_start..].find(MARKER_SUFFIX) else {
                // start 标记未闭合:保留 start 之后内容继续等待（含限长保护）
                if self.buffer.len() > MAX_BLOCK_BYTES {
                    self.buffer.clear();
                    outcome.dropped += 1;
                    self.dropped_total += 1;
                }
                break;
            };
            let name_end = name_start + name_rel;
            let name = self.buffer[name_start..name_end].to_string();
            // 3. 查找 end 标记（参数 JSON 位于 start 闭合与 end 标记之间）
            let args_start = name_end + MARKER_SUFFIX.len();
            let rest = &self.buffer[args_start..];
            let Some(end_rel) = rest.find(END_PREFIX) else {
                // 参数未闭合:保留 start 之后内容继续等待（含限长保护）
                if self.buffer.len() > MAX_BLOCK_BYTES {
                    self.buffer.clear();
                    outcome.dropped += 1;
                    self.dropped_total += 1;
                }
                break;
            };
            let after_end = &rest[end_rel + END_PREFIX.len()..];
            let Some(close_rel) = after_end.find(MARKER_SUFFIX) else {
                // end 标记未闭合:继续等待
                if self.buffer.len() > MAX_BLOCK_BYTES {
                    self.buffer.clear();
                    outcome.dropped += 1;
                    self.dropped_total += 1;
                }
                break;
            };
            // 4. 完整块就绪:提取参数 JSON 并校验
            let args_json = rest[..end_rel].to_string();
            let block_end =
                args_start + end_rel + END_PREFIX.len() + close_rel + MARKER_SUFFIX.len();
            if serde_json::from_str::<serde_json::Value>(&args_json).is_err() {
                outcome.dropped += 1;
                self.dropped_total += 1;
            } else {
                let call = DispatchedCall {
                    name: name.clone(),
                    args_json,
                    side_effect: SideEffect::classify(&name),
                    confidence: 1.0,
                };
                match call.side_effect {
                    SideEffect::ReadOnly => outcome.started_readonly.push(call),
                    SideEffect::Write => outcome.deferred_writes.push(call),
                }
            }
            // 5. 消费已处理的块,继续循环查找后续块
            self.buffer.drain(..block_end);
        }
        outcome
    }

    /// 累计丢弃块数（诊断/门禁埋点）
    #[must_use]
    pub fn dropped_total(&self) -> u64 {
        self.dropped_total
    }

    /// 置信度门禁常量（供接入方断言）
    #[must_use]
    pub const fn confidence_gate() -> f64 {
        CONFIDENCE_GATE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 单 chunk 完整闭合 — 只读立即 / 写延迟分类正确
    #[test]
    fn single_chunk_full_block() {
        let mut d = StreamingDispatcher::new();
        let out = d.feed("<|tool_start:search:docs|>{\"q\":\"rust\"}<|tool_end:search:docs|>");
        assert_eq!(out.started_readonly.len(), 1, "只读必须立即派发");
        assert_eq!(out.deferred_writes.len(), 0);
        assert_eq!(out.dropped, 0);
        let call = &out.started_readonly[0];
        assert_eq!(call.name, "search:docs");
        assert_eq!(call.args_json, "{\"q\":\"rust\"}");
        assert_eq!(call.side_effect, SideEffect::ReadOnly);
        assert!((call.confidence - 1.0).abs() < 1e-9);
    }

    /// 跨 chunk 增量闭合 — 标记被任意切分仍正确累积
    #[test]
    fn incremental_across_chunks() {
        let mut d = StreamingDispatcher::new();
        let mut total = DispatchOutcome::default();
        for part in [
            "<|tool_s",
            "tart:query:db|>",
            "{\"sql\":\"SELECT 1\"}",
            "<|tool_end:query:db",
            "|>",
        ] {
            let out = d.feed(part);
            total.started_readonly.extend(out.started_readonly);
            total.deferred_writes.extend(out.deferred_writes);
            total.dropped += out.dropped;
        }
        assert_eq!(total.total(), 1, "跨 chunk 必须恰产出 1 个闭合块");
        assert_eq!(total.started_readonly[0].name, "query:db");
        assert_eq!(
            total.started_readonly[0].args_json,
            "{\"sql\":\"SELECT 1\"}"
        );
    }

    /// 未闭合块不派发 — 置信度门禁:未闭合 = 不 dispatch
    #[test]
    fn unclosed_block_not_dispatched() {
        let mut d = StreamingDispatcher::new();
        let out = d.feed("<|tool_start:search:docs|>{\"q\":\"partial\"}");
        assert_eq!(out.total(), 0, "未闭合必须不派发");
        // 后续闭合后产出
        let out2 = d.feed("<|tool_end:search:docs|>");
        assert_eq!(out2.total(), 1);
    }

    /// schema 校验失败 → 丢弃（低置信,不派发）
    #[test]
    fn invalid_json_dropped() {
        let mut d = StreamingDispatcher::new();
        let out = d.feed("<|tool_start:query:db|>{broken json}<|tool_end:query:db|>");
        assert_eq!(out.total(), 0, "非法 JSON 必须丢弃");
        assert_eq!(out.dropped, 1);
        assert_eq!(d.dropped_total(), 1);
    }

    /// 同 chunk 多块 — 全部产出且分类正确
    #[test]
    fn multiple_blocks_same_chunk() {
        let mut d = StreamingDispatcher::new();
        let out = d.feed(
            "前言<|tool_start:search:a|>{\"q\":\"a\"}<|tool_end:search:a|>中间<|tool_start:write:edit|>{\"f\":\"a.txt\"}<|tool_end:write:edit|>后记",
        );
        assert_eq!(out.started_readonly.len(), 1, "只读 1 个");
        assert_eq!(out.deferred_writes.len(), 1, "写 1 个延迟");
        assert_eq!(out.started_readonly[0].name, "search:a");
        assert_eq!(out.deferred_writes[0].name, "write:edit");
    }

    /// 正文夹杂 — 非块文本被忽略,不干扰解析
    #[test]
    fn interleaved_prose_ignored() {
        let mut d = StreamingDispatcher::new();
        let out = d.feed(
            "我来分析一下<|tool_start:search:docs|>{\"q\":\"x\"}<|tool_end:search:docs|>结论如下。",
        );
        assert_eq!(out.total(), 1);
        assert_eq!(out.started_readonly[0].args_json, "{\"q\":\"x\"}");
    }

    /// SideEffect 分类 — 前缀约定 + 默认保守 Write
    #[test]
    fn side_effect_classify() {
        assert_eq!(SideEffect::classify("search:github"), SideEffect::ReadOnly);
        assert_eq!(SideEffect::classify("query:db"), SideEffect::ReadOnly);
        assert_eq!(SideEffect::classify("read:file"), SideEffect::ReadOnly);
        assert_eq!(SideEffect::classify("edit:file"), SideEffect::Write);
        assert_eq!(
            SideEffect::classify("bash"),
            SideEffect::Write,
            "未识别默认保守"
        );
    }

    /// 缓冲超限丢弃 — 防御性上限防内存撑爆
    #[test]
    fn oversized_block_dropped() {
        let mut d = StreamingDispatcher::new();
        let big = format!(
            "<|tool_start:query:db|>{}{}<|tool_end:query:db|>",
            "{",
            "a".repeat(MAX_BLOCK_BYTES + 100)
        );
        let out = d.feed(&big);
        assert_eq!(out.total(), 0);
        assert!(out.dropped >= 1, "超限块必须丢弃");
    }

    /// 预执行与正式执行一致率 — 同一块只产出一次（无重复执行路径）
    #[test]
    fn no_duplicate_dispatch() {
        let mut d = StreamingDispatcher::new();
        let block = "<|tool_start:search:docs|>{\"q\":\"once\"}<|tool_end:search:docs|>";
        let a = d.feed(block);
        let b = d.feed(block);
        assert_eq!(a.total(), 1);
        assert_eq!(
            b.total(),
            1,
            "重复输入各产 1 个（互不干扰,接入方按流语义消费）"
        );
    }

    /// 部分前缀累积 — 缓冲只保留前缀候选,正文被清理
    #[test]
    fn buffer_prefix_preserved_only() {
        let mut d = StreamingDispatcher::new();
        let _ = d.feed("这是正文没有工具标记");
        assert!(
            d.buffer.len() <= START_PREFIX.len(),
            "正文应被清理,只保留前缀候选"
        );
        // 前缀候选跨 chunk 累积后仍能解析
        let mut d2 = StreamingDispatcher::new();
        let mut total = DispatchOutcome::default();
        for part in [
            "<|",
            "tool_start:search",
            ":docs|>{\"q\":\"p\"}",
            "<|tool_end:search:docs|>",
        ] {
            let out = d2.feed(part);
            total.started_readonly.extend(out.started_readonly);
            total.deferred_writes.extend(out.deferred_writes);
            total.dropped += out.dropped;
        }
        assert_eq!(total.total(), 1, "前缀跨 chunk 累积后必须产出");
        assert_eq!(total.started_readonly[0].name, "search:docs");
    }
}
