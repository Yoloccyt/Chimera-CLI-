//! data::pace_gate — PaceGate 认知节奏闸门(Concord W8 T8.1,ADR-080)
//!
//! 对应架构层:L10 Interface
//!
//! # 职责
//! 流式输出的"完整性 + 节奏"双闸门,替代单一维度的 NewlineGate:
//! - **完整性闸**(继承):内嵌 [`super::newline_gate::NewlineGate`],行/fence
//!   块边界切分规则与 v1.0 完全一致;
//! - **节奏闸**(新增):完整行按内容类别配速提交——Code 快速通道
//!   (≤8ms/行)、Prose 阅读配速(~40ms/行)、空行(Status)即时;
//!   学理依据:UIST 2025 认知负载感知双流(Xiao & Yang)——慢流匹配阅读、
//!   快流匹配跳读,降本不损体验。
//!
//! # 关键规则(方案 §5.2)
//! - **首行直通**:`first_line_passed` 之前任何完整行零延迟提交,
//!   保住 TTFB ≤100ms 不变量(R12 熔断线);
//! - **积压排空**:待提交行 > `backlog_drain_threshold`(默认 64)时
//!   批量全放行,防积压雪崩(SEC-6 有界缓冲);
//! - **显式时钟**:所有时间经 `now: Instant` 参数注入,组件纯确定,
//!   测试用合成时钟,生产由调用方传 `Instant::now()`。
//!
//! # 内容守恒不变量(proptest 守护)
//! 任意切分的 chunk 序列与任意单调时间推进,
//! `feed/drain_due 输出拼接 + flush 冲刷 == 原始流`。

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::newline_gate::NewlineGate;

/// 内容类别 — 决定行级配速档
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentClass {
    /// 代码(fence 块内与围栏标记行):快速通道 ≤8ms/行
    Code,
    /// 散文(普通文本):阅读配速 ~40ms/行
    Prose,
    /// 状态行(空行):即时提交(零视觉权重)
    Status,
}

/// 配速档 — `/pace` 命令三档
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum PaceMode {
    /// 阅读配速(默认):Code ≤8ms/行,Prose ~40ms/行
    #[default]
    Reading,
    /// 快速通道:所有类别零配速(完整即提交,节奏闸退化)
    Fast,
    /// 关闭:恢复 v1.0 行闸门语义(完整性一维)
    Off,
}

impl PaceMode {
    /// 从命令参数解析;未识别返回 None(执行层诚实反馈)
    pub fn from_arg(arg: &str) -> Option<Self> {
        match arg.trim().to_ascii_lowercase().as_str() {
            "fast" => Some(Self::Fast),
            "reading" => Some(Self::Reading),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    /// 档位标识(状态栏反馈与 i18n 键后缀)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Reading => "reading",
            Self::Fast => "fast",
            Self::Off => "off",
        }
    }

    /// 编码为 u8(跨线程 AtomicU8 槽位传递,DataPipeline pace_slot)
    pub fn as_u8(self) -> u8 {
        match self {
            Self::Reading => 0,
            Self::Fast => 1,
            Self::Off => 2,
        }
    }

    /// 从 u8 解码;未知值回退默认 Reading(保守降级)
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Fast,
            2 => Self::Off,
            _ => Self::Reading,
        }
    }
}

/// 提交单元 — 一条配速后的完整行(携带行尾 `\n` 或 flush 残段)
#[derive(Debug, Clone, PartialEq)]
pub struct CommitUnit {
    /// 行文本(守恒:全部单元拼接 + flush == 原始流)
    pub text: String,
    /// 内容类别(遥测/渲染调优用;提交行为不受其影响)
    pub class: ContentClass,
}

// WHY Clone/PartialEq:ChatSync 派生 Clone/PartialEq 需要(Instant 均实现)
/// 待提交队列条目:提交单元 + 到期时刻
#[derive(Debug, Clone, PartialEq)]
struct PacedUnit {
    unit: CommitUnit,
    /// 到期时刻(≤ now 时可提交)
    ready_at: Instant,
}

/// Code 快速通道间隔(方案 §5.2 配速表初值)
pub const CODE_INTERVAL_MS: u64 = 8;
/// Prose 阅读配速间隔(~250 词/分阅读速度)
pub const PROSE_INTERVAL_MS: u64 = 40;
/// 积压排空阈值默认值(待提交行数上限,超限批量放行)
pub const DEFAULT_DRAIN_THRESHOLD: usize = 64;

// WHY Clone/PartialEq:ChatSync 派生同名 trait 需要(全字段均支持)
/// 认知节奏闸门 — 完整性闸(NewlineGate)+ 节奏闸(类别令牌排程)
#[derive(Debug, Clone, PartialEq)]
pub struct PaceGate {
    /// 完整性闸(v1.0 组件内嵌复用,行/fence 规则零改动)
    integr: NewlineGate,
    /// 当前配速档
    mode: PaceMode,
    /// 首行是否已直通(此后才启用节奏闸,TTFB 保护)
    first_line_passed: bool,
    /// 积压排空阈值
    drain_threshold: usize,
    /// 待提交队列(有界:drain 强制排空保证 ≤ threshold+单批)
    pending: VecDeque<PacedUnit>,
    /// 围栏态自持跟踪:NewlineGate 对 fence 块是**延迟整块提交**,
    /// 提交时刻其 in_fence 已闭合,无法逐行反映;故本闸自持翻转态
    /// (围栏标记行成对出现,逐行 XOR;flush/reset 复位)。
    fence_open: bool,
    /// 各类别下一次可排程时刻(令牌排程:next = max(now, next + interval))
    next_slot_code: Option<Instant>,
    next_slot_prose: Option<Instant>,
}

impl Default for PaceGate {
    fn default() -> Self {
        Self::new()
    }
}

impl PaceGate {
    /// 创建 Reading 档闸门(默认配速表与排空阈值)
    pub fn new() -> Self {
        Self {
            integr: NewlineGate::new(),
            mode: PaceMode::Reading,
            first_line_passed: false,
            drain_threshold: DEFAULT_DRAIN_THRESHOLD,
            pending: VecDeque::new(),
            fence_open: false,
            next_slot_code: None,
            next_slot_prose: None,
        }
    }

    /// 自定义排空阈值(测试与调参用)
    pub fn with_drain_threshold(mut self, threshold: usize) -> Self {
        self.drain_threshold = threshold;
        self
    }

    /// 当前配速档
    pub fn mode(&self) -> PaceMode {
        self.mode
    }

    /// 切换配速档(运行时经 `/pace` 命令)
    ///
    /// 已在队中的待提交单元保留其原到期时刻(不追溯重排,避免
    /// 内容乱序),由后续 drain_due 自然释放;新行按新档配速。
    pub fn set_mode(&mut self, mode: PaceMode) {
        self.mode = mode;
    }

    /// 当前积压行数(待提交队列长度)
    pub fn backlog(&self) -> usize {
        self.pending.len()
    }

    /// 喂入一个增量 chunk,返回本次立即可提交的单元
    ///
    /// 流程:完整性闸切分 → 逐行分类 → 首行直通 / Off 直通 / Fast 直通,
    /// 否则排入配速队列(到期后由 `drain_due` 取出);积压超限先排空。
    pub fn feed(&mut self, chunk: &str, now: Instant) -> Vec<CommitUnit> {
        let lines = self.integr.feed(chunk);
        if lines.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<CommitUnit> = Vec::new();
        for line in lines {
            let class = self.classify_and_track(&line);
            let unit = CommitUnit { text: line, class };
            // 直通三通道:首行(TTFB)/ Off(行闸门语义)/ Fast(零配速)
            if !self.first_line_passed || self.mode != PaceMode::Reading {
                self.first_line_passed = true;
                out.push(unit);
                continue;
            }
            self.schedule(unit, now);
        }
        // 积压排空:超限批量放行(防雪崩,SEC-6)
        if self.pending.len() > self.drain_threshold {
            out.extend(self.pending.drain(..).map(|p| p.unit));
        }
        out
    }

    /// 取出已到期的待提交单元(O(队首连续到期段长度))
    pub fn drain_due(&mut self, now: Instant) -> Vec<CommitUnit> {
        let mut out = Vec::new();
        while let Some(front) = self.pending.front() {
            if front.ready_at <= now {
                out.push(self.pending.pop_front().expect("front checked").unit);
            } else {
                break;
            }
        }
        out
    }

    /// 流结束冲刷:队列全放行 + 完整性闸残段(与 v1.0 flush 语义一致)
    ///
    /// 返回 (队列冲刷单元, 残段文本)。残段不配速——流已终结,
    /// 守恒优先于节奏。
    pub fn flush(&mut self) -> (Vec<CommitUnit>, Option<String>) {
        let queued: Vec<CommitUnit> = self.pending.drain(..).map(|p| p.unit).collect();
        let rest = self.integr.flush();
        self.first_line_passed = false;
        self.fence_open = false;
        self.next_slot_code = None;
        self.next_slot_prose = None;
        (queued, rest)
    }

    /// 新一轮交互复位:丢弃残段与队列(与 ChatSync 的 TuiChatSubmitted 语义一致)
    pub fn reset(&mut self) {
        self.integr.flush();
        self.pending.clear();
        self.first_line_passed = false;
        self.fence_open = false;
        self.next_slot_code = None;
        self.next_slot_prose = None;
    }

    /// 行分类并推进围栏态:围栏内/围栏标记 → Code;空行 → Status;其余 Prose
    ///
    /// 围栏标记行成对出现(开/闭),逐行 XOR 翻转 `fence_open`;
    /// 标记行本身也属代码(它是 fence 块的一部分)。
    fn classify_and_track(&mut self, line: &str) -> ContentClass {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let is_marker = body.starts_with("```");
        let class = if self.fence_open || is_marker {
            ContentClass::Code
        } else if body.trim().is_empty() {
            ContentClass::Status
        } else {
            ContentClass::Prose
        };
        if is_marker {
            self.fence_open = !self.fence_open;
        }
        class
    }

    /// 排程一个单元:按类别推进令牌槽位,计算到期时刻入队
    fn schedule(&mut self, unit: CommitUnit, now: Instant) {
        // Status(空行)零视觉权重:即时到期,不占任何槽位
        if unit.class == ContentClass::Status {
            self.pending.push_back(PacedUnit {
                unit,
                ready_at: now,
            });
            return;
        }
        let interval = match unit.class {
            ContentClass::Code => Duration::from_millis(CODE_INTERVAL_MS),
            ContentClass::Prose => Duration::from_millis(PROSE_INTERVAL_MS),
            ContentClass::Status => unreachable!("Status 已提前处理"),
        };
        // 令牌排程:同类别行按间隔串行,跨类别独立槽位互不阻塞
        let slot = match unit.class {
            ContentClass::Prose => &mut self.next_slot_prose,
            ContentClass::Code => &mut self.next_slot_code,
            ContentClass::Status => unreachable!("Status 已提前处理"),
        };
        let ready_at = match *slot {
            Some(prev) => prev.max(now) + interval,
            None => now + interval,
        };
        *slot = Some(ready_at);
        self.pending.push_back(PacedUnit { unit, ready_at });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn first_line_passes_through_without_delay() {
        let mut g = PaceGate::new();
        let now = t0();
        let out = g.feed("hello\nworld\n", now);
        assert_eq!(out.len(), 1, "仅首行直通;第二行进入节奏闸");
        assert_eq!(out[0].text, "hello\n");
        assert_eq!(out[0].class, ContentClass::Prose);
        assert_eq!(g.backlog(), 1, "第二行应被配速扣留");
        let due = g.drain_due(now + Duration::from_millis(40));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].text, "world\n");
    }

    #[test]
    fn prose_lines_are_paced_after_first() {
        let mut g = PaceGate::new();
        let now = t0();
        g.feed("first\n", now);
        let out = g.feed("second\n", now);
        assert!(out.is_empty(), "第二行应被节奏闸扣留(40ms 未到期)");
        assert_eq!(g.backlog(), 1);
        let due = g.drain_due(now + Duration::from_millis(40));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].text, "second\n");
    }

    #[test]
    fn code_lines_use_fast_lane() {
        let mut g = PaceGate::new();
        let now = t0();
        // 首行直通后,fence 块闭合整块提交,块内行类别 Code(8ms 间隔)
        g.feed("intro\n", now);
        g.feed("```\ncode_line\n```\n", now);
        assert_eq!(g.backlog(), 3, "围栏三行应入配速队列");
        // 三行 Code 按 8ms 串行排程(8/16/24ms),+24ms 全部到期
        let due = g.drain_due(now + Duration::from_millis(24));
        assert_eq!(due.len(), 3, "+24ms 应全部到期");
        assert!(
            due.iter().any(|u| u.text == "```\n"),
            "围栏行应按 Code 配速到期"
        );
        let code_line = due.iter().find(|u| u.text == "code_line\n");
        assert!(
            code_line.is_some_and(|u| u.class == ContentClass::Code),
            "围栏内行应分类为 Code(自持围栏态)"
        );
    }

    #[test]
    fn empty_lines_are_status_instant() {
        let mut g = PaceGate::new();
        let now = t0();
        g.feed("first\n", now);
        let out = g.feed("\n", now);
        // 空行(Status)排入队列但到期时刻 = now,drain_due 立即可取
        assert!(out.is_empty() || out[0].class == ContentClass::Status);
        let due = g.drain_due(now);
        assert!(
            due.iter().any(|u| u.class == ContentClass::Status),
            "Status 行应即时到期"
        );
    }

    #[test]
    fn off_mode_restores_newline_semantics() {
        let mut g = PaceGate::new();
        g.set_mode(PaceMode::Off);
        let now = t0();
        let out = g.feed("a\nb\nc\n", now);
        assert_eq!(out.len(), 3, "Off 档:完整即提交,无节奏扣留");
        assert_eq!(g.backlog(), 0);
    }

    #[test]
    fn fast_mode_zero_pacing() {
        let mut g = PaceGate::new();
        let now = t0();
        g.feed("first\n", now);
        g.set_mode(PaceMode::Fast);
        let out = g.feed("x\ny\n", now);
        assert_eq!(out.len(), 2, "Fast 档:零配速立即提交");
    }

    #[test]
    fn backlog_drain_prevents_avalanche() {
        let mut g = PaceGate::new().with_drain_threshold(3);
        let now = t0();
        g.feed("first\n", now);
        // 一次灌入 5 行(> 阈值 3):积压排空全放行
        let out = g.feed("l1\nl2\nl3\nl4\nl5\n", now);
        assert!(out.len() >= 4, "积压超限应批量排空,实测放行 {}", out.len());
        assert!(g.backlog() <= 3, "排空后积压应回到阈值内");
    }

    #[test]
    fn flush_drains_queue_and_residual() {
        let mut g = PaceGate::new();
        let now = t0();
        g.feed("first\n", now);
        g.feed("pending_line\n", now); // 被扣留
        g.feed("tail_no_newline", now); // 残段
        let (queued, rest) = g.flush();
        assert_eq!(queued.len(), 1, "队列单元应全放行");
        assert_eq!(queued[0].text, "pending_line\n");
        assert_eq!(rest.as_deref(), Some("tail_no_newline"), "残段守恒冲刷");
    }

    #[test]
    fn reset_clears_state_for_new_turn() {
        let mut g = PaceGate::new();
        let now = t0();
        g.feed("a\nb\n", now);
        g.feed("c\n", now);
        g.reset();
        assert_eq!(g.backlog(), 0);
        // 复位后首行重新直通(TTFB 语义逐轮生效)
        let out = g.feed("new_turn\n", now);
        assert_eq!(out.len(), 1, "复位后首行应直通");
    }

    #[test]
    fn pace_mode_arg_parsing() {
        assert_eq!(PaceMode::from_arg("fast"), Some(PaceMode::Fast));
        assert_eq!(PaceMode::from_arg(" Reading "), Some(PaceMode::Reading));
        assert_eq!(PaceMode::from_arg("OFF"), Some(PaceMode::Off));
        assert_eq!(PaceMode::from_arg("turbo"), None, "未识别档位返回 None");
    }

    #[test]
    fn pace_mode_u8_roundtrip() {
        for mode in [PaceMode::Reading, PaceMode::Fast, PaceMode::Off] {
            assert_eq!(PaceMode::from_u8(mode.as_u8()), mode);
        }
        assert_eq!(PaceMode::from_u8(99), PaceMode::Reading, "未知值回退默认");
    }

    #[test]
    fn conservation_simple_sequence() {
        let mut g = PaceGate::new();
        let mut now = t0();
        let input = "alpha\nbeta\ngamma\n";
        let mut got = String::new();
        for chunk in ["alpha\n", "beta\n", "gamma\n"] {
            for u in g.feed(chunk, now) {
                got.push_str(&u.text);
            }
            now += Duration::from_millis(50);
            for u in g.drain_due(now) {
                got.push_str(&u.text);
            }
        }
        let (queued, rest) = g.flush();
        for u in queued {
            got.push_str(&u.text);
        }
        if let Some(r) = rest {
            got.push_str(&r);
        }
        assert_eq!(got, input, "内容守恒:输出拼接 == 输入流");
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    fn t0() -> Instant {
        Instant::now()
    }

    /// 任意 chunk 切分 + 任意单调时间推进下的守恒与不变量
    fn run_gate(chunks: &[String], time_steps_ms: &[u64], mode: PaceMode) -> (String, usize) {
        let mut g = PaceGate::new();
        g.set_mode(mode);
        let mut now = t0();
        let mut got = String::new();
        let mut max_backlog = 0usize;
        for (i, chunk) in chunks.iter().enumerate() {
            for u in g.feed(chunk, now) {
                got.push_str(&u.text);
            }
            max_backlog = max_backlog.max(g.backlog());
            // 时间推进(取模保证有限步长,模拟真实节奏抖动)
            let step = time_steps_ms.get(i).copied().unwrap_or(10) % 100;
            now += Duration::from_millis(step);
            for u in g.drain_due(now) {
                got.push_str(&u.text);
            }
            max_backlog = max_backlog.max(g.backlog());
        }
        // 充分长时间后冲刷:一切皆到期
        now += Duration::from_secs(60);
        for u in g.drain_due(now) {
            got.push_str(&u.text);
        }
        let (queued, rest) = g.flush();
        for u in queued {
            got.push_str(&u.text);
        }
        if let Some(r) = rest {
            got.push_str(&r);
        }
        (got, max_backlog)
    }

    proptest! {
        /// 不变量①(守恒):任意切分 + 任意节奏下,输出拼接 == 输入流
        #[test]
        fn conservation_under_any_chunking(
            chunks in proptest::collection::vec("[a-z \n`]{0,40}", 0..30),
            steps in proptest::collection::vec(any::<u64>(), 0..30),
        ) {
            let input: String = chunks.concat();
            let (got, _) = run_gate(&chunks, &steps, PaceMode::Reading);
            prop_assert_eq!(got, input);
        }

        /// 不变量②(TTFB):首个完整行在第一次 feed 即返回(零延迟)
        #[test]
        fn first_line_zero_latency(
            prefix in "[a-z]{1,10}",
            suffix in "[a-z]{0,10}",
        ) {
            let mut g = PaceGate::new();
            let now = t0();
            let out = g.feed(&format!("{prefix}\n{suffix}"), now);
            prop_assert!(!out.is_empty(), "首行必须在首次 feed 直通");
            prop_assert_eq!(&out[0].text, &format!("{prefix}\n"));
        }

        /// 不变量③(有界):Reading 档积压峰值 ≤ 阈值 + 单次 feed 行数
        /// (drain 在超限同批内强制排空,无界增长不可能)
        #[test]
        fn backlog_bounded(
            chunks in proptest::collection::vec("[a-z]{1,6}\\n", 1..80),
            steps in proptest::collection::vec(0u64..3, 1..80),
        ) {
            // steps 取 0-2ms(远小于配速间隔)制造最坏积压场景
            let (_, max_backlog) = run_gate(&chunks, &steps, PaceMode::Reading);
            // 阈值 64;单次 feed 一行,故峰值 ≤ 65(超限当批即排空)
            prop_assert!(
                max_backlog <= DEFAULT_DRAIN_THRESHOLD + 1,
                "积压峰值 {} 超有界上限",
                max_backlog
            );
        }

        /// 不变量④(Off/Fast 档零扣留):任意输入下队列为空
        #[test]
        fn off_and_fast_never_hold(
            chunks in proptest::collection::vec("[a-z\n]{0,30}", 0..20),
            mode_idx in 0usize..2,
        ) {
            let mode = if mode_idx == 0 { PaceMode::Off } else { PaceMode::Fast };
            let mut g = PaceGate::new();
            g.set_mode(mode);
            let now = t0();
            for c in &chunks {
                g.feed(c, now);
                prop_assert_eq!(g.backlog(), 0, "{:?} 档不得扣留", mode);
            }
        }
    }
}
