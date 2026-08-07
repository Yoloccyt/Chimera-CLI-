//! 流式输出治理 — early stop 控制器(数据面消费侧护栏)
//!
//! # 为什么 early stop 不进 event-bus(ADR-065 决策 4)
//! 归一后的 `StreamEvent` 是进程内数据面类型,per-token delta 经 bounded
//! mpsc(256)直连调用方,不广播进 event-bus(broadcast 1024 容量承载不了
//! per-token 流,Lagged 丢弃会破坏 TUI 体验)。early stop 是数据面消费侧
//! 的预算护栏,必须与被治理的流同侧——跨总线往返一次决策会引入不可接受
//! 的延迟,且"停止后冻结消费"天然是消费侧局部状态。
//!
//! # 为什么用字符/4 估算
//! 流式过程中没有真实 token 化器(本地无词表,逐 delta 调厂商 tokenize 不
//! 现实),中英文混排下 1 token ≈ 4 字节是业界通用近似——与
//! `adapters::estimate_cost` 的字符/4 口径一致(`len()` 字节数)。估算只用于
//! 预算护栏的粗粒度预检,厂商真实 output_tokens 到达(`StreamEvent::Usage`)
//! 后以真实值覆盖。
//!
//! # 与 negotiate_budget 的联动
//! `negotiate_budget` 产出 `OutputBudget { max_output_tokens, .. }`(来自模型
//! 容量 `capabilities.max_output`),本控制器以该值为硬上限:`new(max_output_tokens)`。
//! 语义区别:厂商侧 max_tokens 是软上限(厂商可能超发或提前 return),本
//! 控制器是网关侧硬护栏——超限即 `Stop::BudgetExceeded`,提前终止阻止后续
//! token 消费,未消费的部分自然不计成本。
//!
//! # 完成度感知早停(ADR-072 决策 ⑥)
//! early stop 上限只省 TTFT 与下游处理,不省已生成 token(厂商按生成计费);
//! 语义完成即停是唯一能"真省输出 token"的流式手段——结构化输出(JSON/
//! 代码围栏/Markdown)在语义上已完成时主动停止消费,阻止厂商继续生成。
//! 仅当请求显式声明 `output_format` 时启用(正确性优先,FreeText 不启用)。

use crate::sse::StreamEvent;
use nexus_contracts::affinity::{FinishReason, OutputFormat};

/// 单事件停止决策
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopDecision {
    /// 继续消费流
    Continue,
    /// 停止消费流
    Stop {
        /// 停止原因
        reason: StopReason,
        /// 停止时已消费输出 token 数(估算或厂商真实值)
        consumed: u64,
    },
}

/// 停止原因
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// 厂商自然结束(流终止)
    Finished(FinishReason),
    /// 达到输出预算上限(max_output_tokens),提前终止
    BudgetExceeded,
    /// 语义完成(结构化输出已完整,阻止厂商继续生成,ADR-072 决策 ⑥)
    ///
    /// 仅 `output_format` 显式声明(Json/CodeFence/Markdown)时可能触发;
    /// FreeText 永不触发(正确性优先,防误判截断)。
    SemanticComplete,
}

/// 流式输出治理 early stop 控制器
#[derive(Debug)]
pub struct EarlyStopController {
    /// 输出预算硬上限(来自 negotiate_budget 的 max_output_tokens)
    max_output_tokens: u32,
    /// 字符/4 启发式估算累计(TextDelta + ThinkingDelta;与 estimate_cost 同口径)
    estimated_consumed: u64,
    /// 厂商真实 output_tokens(Usage 回填;None = 未回填,用估算)
    last_reported_output: Option<u64>,
    /// 已触发停止决策(幂等记忆;None = 未停止)
    stop_state: Option<StopDecision>,
    /// 完成度检测器(None = 未启用,FreeText 或未声明 output_format)
    completion: Option<CompletionDetector>,
}

impl EarlyStopController {
    /// 创建控制器,max_output_tokens 来自 negotiate_budget 的 OutputBudget
    pub fn new(max_output_tokens: u32) -> Self {
        Self::with_completion(max_output_tokens, OutputFormat::FreeText)
    }

    /// 创建控制器并启用完成度检测(ADR-072 决策 ⑥)
    ///
    /// `format` 非 FreeText 时启用语义完成早停:结构化输出检测到完整后
    /// 触发 `StopReason::SemanticComplete`(阻止厂商继续生成,省输出 token)。
    pub fn with_completion(max_output_tokens: u32, format: OutputFormat) -> Self {
        Self {
            max_output_tokens,
            estimated_consumed: 0,
            last_reported_output: None,
            stop_state: None,
            completion: (format != OutputFormat::FreeText).then(|| CompletionDetector::new(format)),
        }
    }

    /// 消费单个流事件,产出停止决策
    pub fn on_event(&mut self, event: &StreamEvent) -> StopDecision {
        // 已停止后幂等:冻结决策,不再消费后续事件(阻止后续 token 泄漏)
        if let Some(stop) = self.stop_state {
            return stop;
        }
        let decision = match event {
            StreamEvent::Done(reason) => StopDecision::Stop {
                reason: StopReason::Finished(*reason),
                consumed: self.consumed_tokens(),
            },
            StreamEvent::TextDelta(text) | StreamEvent::ThinkingDelta(text) => {
                // 与 estimate_cost 一致的字节宽启发式(ADR-070 显式化):
                // 中英文混排 1 token ≈ 4 字节;真实值由 Usage 回填覆盖
                self.estimated_consumed = self
                    .estimated_consumed
                    .saturating_add(u64::from(crate::token_estimate::estimate_text(text)));
                // 完成度检测(ADR-072):语义完成优先于预算上限触发
                // (预算护栏是兜底,完成即停才是输出 token 治理的主手段)
                if let Some(detector) = &mut self.completion {
                    if detector.on_delta(text) {
                        let decision = StopDecision::Stop {
                            reason: StopReason::SemanticComplete,
                            consumed: self.consumed_tokens(),
                        };
                        self.stop_state = Some(decision);
                        return decision;
                    }
                }
                if self.consumed_tokens() > u64::from(self.max_output_tokens) {
                    StopDecision::Stop {
                        reason: StopReason::BudgetExceeded,
                        consumed: self.consumed_tokens(),
                    }
                } else {
                    StopDecision::Continue
                }
            }
            // 厂商真实计量:覆盖估算(含思考;thinking_tokens 单列时已计入
            // output,不重复累加)。output_tokens=0 是首帧输入侧计量
            // (Anthropic message_start),回填 0 会清空估算,故仅回填 >0 值
            StreamEvent::Usage(usage) => {
                if usage.output_tokens > 0 {
                    self.last_reported_output = Some(usage.output_tokens);
                }
                StopDecision::Continue
            }
            // 工具调用帧不是输出 token 成本主体;Unknown 容错(P3)继续
            StreamEvent::ToolCallStart { .. }
            | StreamEvent::ToolCallDelta { .. }
            | StreamEvent::ToolCallEnd { .. }
            | StreamEvent::Unknown(_) => StopDecision::Continue,
        };
        if !matches!(decision, StopDecision::Continue) {
            self.stop_state = Some(decision);
        }
        decision
    }

    /// 已消费输出 token(厂商真实值优先,否则字符/4 估算)
    pub fn consumed_tokens(&self) -> u64 {
        self.last_reported_output.unwrap_or(self.estimated_consumed)
    }

    /// 是否已触发停止
    pub fn should_stop(&self) -> bool {
        self.stop_state.is_some()
    }
}

/// 完成度检测器 — 结构化输出的语义完成检测(ADR-072 决策 ⑥)
///
/// 流式累计文本,按 `output_format` 判定语义完成:
/// - `Json`: 括号平衡(嵌套 {}[]) + 引号配对 + 末字符为 `}`/`]`
/// - `CodeFence`: 代码围栏(```)成对闭合 + 内容非空
/// - `Markdown`: 围栏闭合 + 末尾段落结束(空行) + 内容充足
/// - 通用: 4-gram 重复检测(模型冗长循环/复读机)→ 冗余抑制
///
/// # 保守性(正确性优先)
/// - FreeText 永不启用(由控制器保证)
/// - Json 要求括号完全平衡**且**末字符为闭合符——模型在完整 JSON 后
///   继续输出说明文本时末字符非 `}`/`]`,不会误触发
/// - 检测节流: 仅当累计增量 ≥ 50 字符时才执行重复检测(流式高频
///   delta 下避免 O(200²) 逐 delta 扫描)
#[derive(Debug)]
pub struct CompletionDetector {
    /// 输出格式(Json/CodeFence/Markdown)
    format: OutputFormat,
    /// 流式累计文本(有界:仅保留尾部,防长流内存膨胀)
    buffer: String,
    /// 上次重复检测时的 buffer 长度(节流标记)
    last_check_len: usize,
    /// 是否已判定完成(幂等)
    complete: bool,
}

impl CompletionDetector {
    /// 创建检测器(format 为 FreeText 时恒不触发,控制器已过滤)
    pub fn new(format: OutputFormat) -> Self {
        Self {
            format,
            buffer: String::new(),
            last_check_len: 0,
            complete: false,
        }
    }

    /// 追加流式 delta,返回是否语义完成(完成即幂等恒 true)
    pub fn on_delta(&mut self, text: &str) -> bool {
        if self.complete {
            return true;
        }
        // 有界缓冲:仅保留尾部 4096 字符(完成判定只需尾部结构)
        self.buffer.push_str(text);
        if self.buffer.len() > 8192 {
            self.buffer = self.buffer[self.buffer.len() - 4096..].to_string();
        }
        let structural = match self.format {
            OutputFormat::Json => json_complete(&self.buffer),
            OutputFormat::CodeFence => code_fence_complete(&self.buffer),
            OutputFormat::Markdown => markdown_complete(&self.buffer),
            OutputFormat::FreeText => false,
        };
        // 冗余抑制(节流):增量 ≥ 50 字符才扫描重复 4-gram
        let repetitive =
            self.buffer.len() - self.last_check_len >= 50 && repetitive_tail(&self.buffer);
        if structural || repetitive {
            self.last_check_len = self.buffer.len();
            self.complete = true;
            true
        } else {
            self.last_check_len = self.buffer.len();
            false
        }
    }

    /// 当前累计文本(诊断用)
    pub fn buffer(&self) -> &str {
        &self.buffer
    }
}

/// JSON 完成判定 — 括号平衡 + 引号配对 + 末字符为闭合符
///
/// 括号不匹配(截断的 JSON)恒返回 false(绝不误判为完成)。
fn json_complete(buffer: &str) -> bool {
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for ch in buffer.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' | '[' => stack.push(ch),
            '}' => {
                if stack.pop() != Some('{') {
                    return false;
                }
            }
            ']' if stack.pop() != Some('[') => return false,
            _ => {}
        }
    }
    stack.is_empty() && !in_string && buffer.trim_end().ends_with(['}', ']'])
}

/// 代码围栏完成判定 — ``` 成对闭合且内容非空
fn code_fence_complete(buffer: &str) -> bool {
    let fences = buffer.matches("```").count();
    fences >= 2 && fences.is_multiple_of(2) && buffer.trim().len() > 3
}

/// Markdown 完成判定 — 围栏闭合 + 末尾段落结束(空行) + 内容充足
///
/// 保守实现:未闭合围栏恒未完成;末尾无空行(段落进行中)未完成;
/// 内容 < 80 字符(标题/列表未展开)未完成。
fn markdown_complete(buffer: &str) -> bool {
    let fences = buffer.matches("```").count();
    if !fences.is_multiple_of(2) {
        return false;
    }
    let trimmed = buffer.trim_end();
    let ends_with_blank = buffer.len() > trimmed.len();
    trimmed.chars().count() >= 80 && ends_with_blank
}

/// 冗余检测 — 尾部 200 字符内任一 4-gram 出现 ≥ 3 次(复读机/冗长循环)
///
/// WHY floor_char_boundary:UTF-8 字节索引切分必须落在字符边界,
/// 中文多字节场景下 `saturating_sub` 可能落在字符中间(panic 风险)。
fn repetitive_tail(buffer: &str) -> bool {
    let start = buffer.len().saturating_sub(200);
    let tail = &buffer[buffer.floor_char_boundary(start)..];
    let chars: Vec<char> = tail.chars().collect();
    if chars.len() < 8 {
        return false;
    }
    for w in chars.windows(4) {
        let pat: String = w.iter().collect();
        if tail.matches(&pat).count() >= 3 {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::affinity::{FinishReason, UsageReport};

    fn usage(output_tokens: u64) -> UsageReport {
        UsageReport {
            input_tokens: 0,
            output_tokens,
            cache_hit_tokens: 0,
            thinking_tokens: None,
        }
    }

    // ---------- 自然结束流 ----------

    #[test]
    fn natural_finish_on_done() {
        let mut c = EarlyStopController::new(100);
        assert_eq!(
            c.on_event(&StreamEvent::TextDelta("你好世界".into())),
            StopDecision::Continue
        );
        assert!(!c.should_stop());
        let decision = c.on_event(&StreamEvent::Done(FinishReason::Stop));
        assert_eq!(
            decision,
            StopDecision::Stop {
                reason: StopReason::Finished(FinishReason::Stop),
                consumed: 3, // 你好世界 = 12 字节 / 4
            }
        );
        assert!(c.should_stop());
    }

    #[test]
    fn finished_after_budget_stop_keeps_early_decision() {
        // 预算已超时即使后续到达 Done,也必须保持 BudgetExceeded(幂等冻结)
        let mut c = EarlyStopController::new(2);
        c.on_event(&StreamEvent::TextDelta("hello world".into())); // 11/4 = 2,边界内
        c.on_event(&StreamEvent::TextDelta("hello world".into())); // 4 > 2 → 停止
        let decision = c.on_event(&StreamEvent::Done(FinishReason::Stop));
        assert_eq!(
            decision,
            StopDecision::Stop {
                reason: StopReason::BudgetExceeded,
                consumed: 4,
            }
        );
    }

    // ---------- 提前终止与幂等 ----------

    #[test]
    fn budget_exceeded_stops_and_idempotent() {
        let mut c = EarlyStopController::new(2);
        assert_eq!(
            c.on_event(&StreamEvent::TextDelta("hello".into())), // 5/4 = 1
            StopDecision::Continue
        );
        let decision = c.on_event(&StreamEvent::TextDelta("world!".into())); // 6/4 = 1 → 2 == 2 边界内
        assert_eq!(decision, StopDecision::Continue);
        let stop = c.on_event(&StreamEvent::TextDelta("!!!!".into())); // 4/4 = 1 → 3 > 2
        assert_eq!(
            stop,
            StopDecision::Stop {
                reason: StopReason::BudgetExceeded,
                consumed: 3,
            }
        );
        assert!(c.should_stop());
        // 幂等:停止后再次调用返回同一决策,不继续累计
        let again = c.on_event(&StreamEvent::TextDelta("xxxxxxxx".into()));
        assert_eq!(again, stop);
        assert_eq!(c.consumed_tokens(), 3);
    }

    // ---------- 预算边界 ----------

    #[test]
    fn budget_boundary_equal_continues() {
        // 恰好等于 max_output_tokens:预算内继续
        let mut c = EarlyStopController::new(2);
        let decision = c.on_event(&StreamEvent::TextDelta("hello world".into())); // 11/4 = 2 == 2
        assert_eq!(decision, StopDecision::Continue);
        assert!(!c.should_stop());
    }

    #[test]
    fn budget_boundary_exceeded_stops() {
        // 超过 max_output_tokens 一个 delta 即停止
        let mut c = EarlyStopController::new(2);
        c.on_event(&StreamEvent::TextDelta("hello world".into())); // 2 == 2
        let decision = c.on_event(&StreamEvent::TextDelta("aaaa".into())); // 3 > 2
        assert_eq!(
            decision,
            StopDecision::Stop {
                reason: StopReason::BudgetExceeded,
                consumed: 3,
            }
        );
    }

    // ---------- 工具调用帧不触发停止 ----------

    #[test]
    fn tool_call_frames_do_not_trigger_stop() {
        let mut c = EarlyStopController::new(100);
        assert_eq!(
            c.on_event(&StreamEvent::ToolCallStart {
                index: 0,
                id: "c1".into(),
                name: "read_file".into()
            }),
            StopDecision::Continue
        );
        assert_eq!(
            c.on_event(&StreamEvent::ToolCallDelta {
                index: 0,
                args_fragment: "{\"path\"".into()
            }),
            StopDecision::Continue
        );
        assert_eq!(
            c.on_event(&StreamEvent::ToolCallEnd { index: 0 }),
            StopDecision::Continue
        );
        assert_eq!(
            c.on_event(&StreamEvent::TextDelta("hi".into())),
            StopDecision::Continue
        );
        assert!(!c.should_stop());
    }

    // ---------- Usage 覆盖 ----------

    #[test]
    fn usage_overrides_estimate() {
        let mut c = EarlyStopController::new(100);
        c.on_event(&StreamEvent::TextDelta("hello world".into())); // 估算 2
        assert_eq!(
            c.on_event(&StreamEvent::Usage(usage(57))),
            StopDecision::Continue
        );
        assert_eq!(c.consumed_tokens(), 57);
        assert!(!c.should_stop());
    }

    #[test]
    fn usage_zero_keeps_estimate() {
        // Anthropic message_start 帧携带输入侧 usage(output_tokens=0),
        // 回填 0 会清空估算,必须保留估算值
        let mut c = EarlyStopController::new(100);
        c.on_event(&StreamEvent::TextDelta("hello world".into())); // 估算 2
        c.on_event(&StreamEvent::Usage(usage(0)));
        assert_eq!(c.consumed_tokens(), 2);
    }

    #[test]
    fn budget_stop_after_usage_reveals_over_budget() {
        // Usage 本身不触发停止(计量帧);真实值越过上限后,下一个内容事件停止
        let mut c = EarlyStopController::new(5);
        c.on_event(&StreamEvent::TextDelta("hello".into())); // 估算 1
        assert_eq!(
            c.on_event(&StreamEvent::Usage(usage(100))),
            StopDecision::Continue
        );
        let decision = c.on_event(&StreamEvent::TextDelta("x".into())); // 0 增量,但真实值 100 > 5
        assert_eq!(
            decision,
            StopDecision::Stop {
                reason: StopReason::BudgetExceeded,
                consumed: 100,
            }
        );
    }

    // ---------- 无终止信号 ----------

    #[test]
    fn no_termination_all_continue() {
        let mut c = EarlyStopController::new(100);
        for _ in 0..5 {
            assert_eq!(
                c.on_event(&StreamEvent::TextDelta("hello".into())), // 5/4 = 1
                StopDecision::Continue
            );
        }
        assert_eq!(c.consumed_tokens(), 5);
        assert!(!c.should_stop());
    }

    // ---------- 字符/4 估算正确性 ----------

    #[test]
    fn char_four_heuristic() {
        let mut c = EarlyStopController::new(100);
        c.on_event(&StreamEvent::TextDelta("hello world".into())); // 11/4 = 2
        c.on_event(&StreamEvent::ThinkingDelta("你好".into())); // 6/4 = 1(字节口径)
        assert_eq!(c.consumed_tokens(), 3);
    }

    #[test]
    fn thinking_delta_counts_toward_budget() {
        let mut c = EarlyStopController::new(1);
        assert_eq!(
            c.on_event(&StreamEvent::ThinkingDelta("abcd".into())), // 4/4 = 1 == 1,边界内
            StopDecision::Continue
        );
        let decision = c.on_event(&StreamEvent::TextDelta("efgh".into())); // 2 > 1
        assert_eq!(
            decision,
            StopDecision::Stop {
                reason: StopReason::BudgetExceeded,
                consumed: 2,
            }
        );
    }

    // ---------- Unknown 容错(P3) ----------

    #[test]
    fn unknown_event_continues() {
        let mut c = EarlyStopController::new(100);
        assert_eq!(
            c.on_event(&StreamEvent::Unknown("unparseable".into())),
            StopDecision::Continue
        );
        assert!(!c.should_stop());
    }

    // ============================================================
    // 完成度感知早停(ADR-072 决策 ⑥)
    // ============================================================

    #[test]
    fn json_completion_triggers_on_balanced_document() {
        let mut c = EarlyStopController::with_completion(10_000, OutputFormat::Json);
        // 流式分段注入完整 JSON
        let chunks = [
            r#"{"name":"#,
            r#""quicksort","#,
            r#""complexity":"O(n log n)"}"#,
        ];
        for ch in chunks {
            let d = c.on_event(&StreamEvent::TextDelta(ch.into()));
            if matches!(d, StopDecision::Stop { .. }) {
                assert_eq!(
                    d,
                    StopDecision::Stop {
                        reason: StopReason::SemanticComplete,
                        consumed: 11, // 3 个 delta 的字节/4 估算累计(9+12+26)/4
                    },
                    "完整 JSON 必须触发 SemanticComplete"
                );
                assert!(c.should_stop());
                return;
            }
        }
        panic!("完整 JSON 未触发完成");
    }

    #[test]
    fn json_unclosed_never_triggers() {
        // 未闭合括号:截断的 JSON 绝不误判为完成
        let mut d = CompletionDetector::new(OutputFormat::Json);
        assert!(!d.on_delta(r#"{"name":"quicksort","#));
        // 注:完整 JSON 触发后幂等冻结(声明 Json 即承诺结构化输出,
        // 首个平衡点即停;若模型尾随文本则被截断——已文档化的权衡,
        // 由 output_format 显式声明 + S9 ASA 冻结兜底)
        let mut d2 = CompletionDetector::new(OutputFormat::Json);
        assert!(d2.on_delta(r#"{"a":1}"#), "完整 JSON 必须触发");
        assert!(d2.on_delta("更多"), "触发后幂等恒 true");
    }

    #[test]
    fn code_fence_completion() {
        let mut d = CompletionDetector::new(OutputFormat::CodeFence);
        d.on_delta("```rust\nfn main() { println!(\"hi\"); }\n");
        assert!(d.on_delta("```"), "围栏成对闭合必须触发完成");
        // 未闭合围栏不触发
        let mut d2 = CompletionDetector::new(OutputFormat::CodeFence);
        d2.on_delta("```rust\nfn main() {}");
        assert!(!d2.on_delta("\n"), "未闭合围栏不得触发");
    }

    #[test]
    fn markdown_completion_requires_blank_line() {
        let mut d = CompletionDetector::new(OutputFormat::Markdown);
        d.on_delta(
            "# 标题\n\n这是正文内容,长度需要超过八十个字符才能满足内容充足的门槛,继续写一些文字来凑足长度,这里再补充一些描述性的内容以确保字符数量达标,最后还要加上一句收尾的话让段落显得更完整一些。",
        );
        assert!(d.on_delta("\n"), "内容充足 + 末尾空行必须触发");
        // 无空行(段落进行中)不触发
        let mut d2 = CompletionDetector::new(OutputFormat::Markdown);
        d2.on_delta("# 标题\n正在写正文没有结束");
        assert!(!d2.on_delta("继续"), "段落进行中不得触发");
    }

    #[test]
    fn repetitive_output_suppressed() {
        // 复读机模式:同一 4-gram 大量重复 → 冗余抑制触发
        let mut d = CompletionDetector::new(OutputFormat::FreeText);
        // FreeText 无结构检测;冗余检测是通用层,仍生效
        let mut chunk = String::new();
        for _ in 0..20 {
            chunk.push_str("重复重复重复重复");
        }
        assert!(d.on_delta(&chunk), "大量重复必须触发冗余抑制");
    }

    #[test]
    fn free_text_controller_never_triggers_semantic() {
        // FreeText(默认):即使内容完整也不触发 SemanticComplete(正确性优先)
        let mut c = EarlyStopController::new(10_000);
        assert_eq!(
            c.on_event(&StreamEvent::TextDelta(r#"{"a":1}"#.into())),
            StopDecision::Continue,
            "FreeText 必须继续消费(不启用完成检测)"
        );
    }

    #[test]
    fn semantic_complete_idempotent_after_trigger() {
        // 触发后冻结:后续事件返回同一决策,不重复累计
        let mut c = EarlyStopController::with_completion(10_000, OutputFormat::Json);
        c.on_event(&StreamEvent::TextDelta(r#"{"a":1}"#.into()));
        let first = c.on_event(&StreamEvent::TextDelta(" ".into()));
        assert!(matches!(first, StopDecision::Stop { .. }));
        let again = c.on_event(&StreamEvent::TextDelta("more".into()));
        assert_eq!(again, first, "触发后必须幂等冻结");
    }
}
