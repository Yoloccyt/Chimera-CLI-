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

use crate::sse::StreamEvent;
use nexus_contracts::affinity::FinishReason;

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
}

impl EarlyStopController {
    /// 创建控制器,max_output_tokens 来自 negotiate_budget 的 OutputBudget
    pub fn new(max_output_tokens: u32) -> Self {
        Self {
            max_output_tokens,
            estimated_consumed: 0,
            last_reported_output: None,
            stop_state: None,
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
                // 与 estimate_cost 一致的字符/4 启发式:len() 为字节数,
                // 中英文混排 1 token ≈ 4 字节;真实值由 Usage 回填覆盖
                self.estimated_consumed = self
                    .estimated_consumed
                    .saturating_add((text.len() / 4) as u64);
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
}
