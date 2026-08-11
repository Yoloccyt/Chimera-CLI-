//! NewlineGate 行闸门 — 流式输出的行级提交闸门(Concord W3 · T3.1)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! 借鉴 Codex newline-gated streaming 实证:流式增量按**完整行**提交给渲染层,
//! 不完整行暂存,避免半行文本在 diff 渲染中反复闪烁;markdown 渲染也以完整行
//! 为稳态单元。闸门为纯函数状态机,置于 ChatSync 累积层(v3 渲染引擎上游,
//! 渲染层零改动),保住 diff<100µs 阈值。
//!
//! # fence 块规则
//! ``` 围栏内的行放宽为**块级提交**:进入 fence 后行暂存,直至围栏闭合行到达
//! 才整块提交,避免代码块半块闪烁。fence 状态以"当前是否处于未闭合围栏内"
//! 跟踪;围栏标记行 = 以 ``` 起始的完整行。
//!
//! # 内容守恒不变量(proptest 守护)
//! 任意切分的 chunk 序列,`feed 输出拼接 + flush 冲刷 == 原始流`。
//! 提交行携带行尾 `\n`;flush 冲刷的残段不含未消费的 `\n` 之外的丢弃。

use std::fmt;

/// fence 围栏标记前缀(简化:行首三个反引号;不处理缩进变体)
const FENCE_MARKER: &str = "```";

/// 行闸门 — 按 `\n` 切出完整行,fence 块内整块提交
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewlineGate {
    /// 未提交的残段(可能含未闭合 fence 块的累积行)
    pending: String,
    /// 当前是否处于未闭合 fence 围栏内
    in_fence: bool,
}

impl fmt::Display for NewlineGate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "NewlineGate(pending_len={}, in_fence={})",
            self.pending.len(),
            self.in_fence
        )
    }
}

impl NewlineGate {
    /// 创建空闸门
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前残段(未提交内容,含 fence 块累积)
    pub fn pending(&self) -> &str {
        &self.pending
    }

    /// 是否处于未闭合 fence 围栏内
    pub fn in_fence(&self) -> bool {
        self.in_fence
    }

    /// 喂入一个增量 chunk,返回本次可提交的完整行(各行携带行尾 `\n`)
    ///
    /// # 规则
    /// - 围栏外:完整行立即提交;遇 fence 起始行则进入围栏态(该行转入块缓冲)
    /// - 围栏内:行累积于 pending,直至围栏闭合行到达后整块提交
    /// - 不完整行(无 `\n`)始终留存 pending
    pub fn feed(&mut self, chunk: &str) -> Vec<String> {
        self.pending.push_str(chunk);
        self.drain()
    }

    /// 流结束冲刷:返回全部剩余内容(可能含未闭合 fence 块),闸门复位
    ///
    /// WHY 整段返回:残段可能含多个未提交行(fence 未闭合场景),冲刷时
    /// 不再拆分——渲染层收到的是一整块尾部内容,守恒不变量不受影响。
    pub fn flush(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            self.in_fence = false;
            return None;
        }
        let rest = std::mem::take(&mut self.pending);
        self.in_fence = false;
        Some(rest)
    }

    /// 从 pending 中榨取可提交内容(核心循环)
    fn drain(&mut self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        loop {
            if self.in_fence {
                // 围栏内:寻找闭合围栏行;找到则整块提交,未找到则全部暂存
                match Self::find_fence_close(&self.pending) {
                    Some(end) => {
                        let block: String = self.pending[..end].to_string();
                        self.pending = self.pending[end..].to_string();
                        self.in_fence = false;
                        // 块按行拆分提交(各行含行尾 \n;块末必为完整行)
                        out.extend(Self::split_lines_keep_newline(&block));
                    }
                    None => break,
                }
            } else {
                // 围栏外:逐个完整行提交;遇 fence 起始行转入围栏态
                match self.pending.find('\n') {
                    Some(pos) => {
                        let line = &self.pending[..=pos];
                        if Self::is_fence_marker(line) {
                            // fence 起始行不单独提交,转入围栏态由块级提交接管
                            self.in_fence = true;
                            continue;
                        }
                        out.push(line.to_string());
                        self.pending = self.pending[pos + 1..].to_string();
                    }
                    None => break,
                }
            }
        }
        out
    }

    /// 判断一行是否为 fence 围栏标记(行首 ``` 且为完整行)
    fn is_fence_marker(line: &str) -> bool {
        // line 含行尾 \n;去掉行尾后判断前缀
        let body = line.strip_suffix('\n').unwrap_or(line);
        body.starts_with(FENCE_MARKER)
    }

    /// 在 pending 中寻找围栏闭合位置:返回闭合围栏行结束处的字节偏移
    ///
    /// 前提:pending 首行为围栏起始行(drain 的围栏态入口保证)。
    /// 从第二个完整行起逐行扫描,首个 fence 标记行即闭合行。
    fn find_fence_close(pending: &str) -> Option<usize> {
        let mut offset = 0usize;
        let mut first = true;
        for (line_with_nl, _) in split_inclusive_iter(pending) {
            offset += line_with_nl.len();
            if first {
                first = false; // 跳过起始行本身
                continue;
            }
            if Self::is_fence_marker(line_with_nl) {
                return Some(offset);
            }
        }
        None
    }

    /// 按行拆分且保留行尾 `\n`(块提交用)
    fn split_lines_keep_newline(block: &str) -> Vec<String> {
        split_inclusive_iter(block)
            .map(|(s, _)| s.to_string())
            .collect()
    }
}

/// 逐行迭代(保留行尾):返回 (含行尾的行切片, 行内容)
///
/// WHY 手写迭代器:str::split_inclusive 在 stable Rust 可用,此处直接复用;
/// 返回元组中第二项目前与第一项相同,保留接口以便未来区分(如剥离行尾)。
fn split_inclusive_iter(s: &str) -> impl Iterator<Item = (&str, &str)> {
    s.split_inclusive('\n').map(|line| (line, line))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn feed_all(gate: &mut NewlineGate, chunks: &[&str]) -> Vec<String> {
        let mut out = Vec::new();
        for c in chunks {
            out.extend(gate.feed(c));
        }
        out
    }

    // === 基础行为 ===

    #[test]
    fn single_complete_line_commits() {
        let mut g = NewlineGate::new();
        assert_eq!(g.feed("hello\n"), vec!["hello\n"]);
        assert!(g.pending().is_empty());
    }

    #[test]
    fn incomplete_line_holds_until_continued() {
        let mut g = NewlineGate::new();
        assert!(g.feed("hel").is_empty());
        assert_eq!(g.pending(), "hel");
        assert_eq!(g.feed("lo\n"), vec!["hello\n"]);
        assert!(g.pending().is_empty());
    }

    #[test]
    fn multiple_lines_and_tail_fragment() {
        let mut g = NewlineGate::new();
        let out = g.feed("a\nb\nc");
        assert_eq!(out, vec!["a\n", "b\n"]);
        assert_eq!(g.pending(), "c");
        assert_eq!(g.flush(), Some("c".to_string()));
        assert!(g.pending().is_empty());
    }

    #[test]
    fn empty_and_newline_only_inputs() {
        let mut g = NewlineGate::new();
        assert!(g.feed("").is_empty());
        assert_eq!(g.feed("\n"), vec!["\n"]);
        assert_eq!(g.feed("\n\n"), vec!["\n", "\n"]);
        assert_eq!(g.flush(), None, "空残段 flush 应返回 None");
    }

    #[test]
    fn cjk_content_preserved() {
        let mut g = NewlineGate::new();
        let out = g.feed("你好,世界\n第二行");
        assert_eq!(out, vec!["你好,世界\n"]);
        assert_eq!(g.flush(), Some("第二行".to_string()));
    }

    // === fence 块行为 ===

    #[test]
    fn fence_block_commits_as_whole() {
        let mut g = NewlineGate::new();
        // 进入 fence 后行暂存
        assert!(g.feed("```\ncode1\n").is_empty());
        assert!(g.in_fence());
        // 闭合后整块提交(含围栏行)
        let out = g.feed("code2\n```\n");
        assert_eq!(out, vec!["```\n", "code1\n", "code2\n", "```\n"]);
        assert!(!g.in_fence());
    }

    #[test]
    fn unclosed_fence_flushes_on_end() {
        let mut g = NewlineGate::new();
        assert!(g.feed("```\npartial").is_empty());
        // 流结束:未闭合 fence 整段冲刷(不丢内容)
        assert_eq!(g.flush(), Some("```\npartial".to_string()));
        assert!(!g.in_fence());
    }

    #[test]
    fn text_after_closed_fence_continues_normally() {
        let mut g = NewlineGate::new();
        let out = g.feed("```\nx\n```\nafter\n");
        assert_eq!(out, vec!["```\n", "x\n", "```\n", "after\n"]);
    }

    /// ttfb 断言式守护(Concord W3 T3.5):首行产出延迟 <1ms
    ///
    /// WHY 单测内断言:基准仅报告分布,断言式用例在 debug 全量回归中
    /// 即时拦截量级回退(debug 开销下 1ms 预算仍宽裕,回退到 10ms+ 必拦)。
    #[test]
    fn ttfb_first_line_under_1ms() {
        let mut g = NewlineGate::new();
        let line = "a]".repeat(200) + "\n"; // 400 字符长行
        let start = std::time::Instant::now();
        let out = g.feed(&line);
        let elapsed = start.elapsed();
        assert_eq!(out.len(), 1, "首个完整行应立即提交");
        assert!(
            elapsed < std::time::Duration::from_millis(1),
            "ttfb 超预算: {elapsed:?}"
        );
    }

    // === proptest 守恒不变量 ===

    proptest! {
        /// 核心守恒:任意文本任意切分,feed 输出拼接 + flush == 原始流
        #[test]
        fn content_conservation_under_arbitrary_chunking(
            content in "[a-z`\n ]{0,200}",
            cuts in proptest::collection::vec(0usize..200, 0..30),
        ) {
            // 依 cuts 切分 content 为 chunk 序列
            let mut points: Vec<usize> = cuts.into_iter()
                .filter(|&p| p <= content.len())
                .collect();
            points.sort_unstable();
            points.dedup();
            let mut chunks: Vec<&str> = Vec::new();
            let mut prev = 0usize;
            for p in &points {
                // 切点必须落在 char 边界
                if content.is_char_boundary(*p) {
                    chunks.push(&content[prev..*p]);
                    prev = *p;
                }
            }
            chunks.push(&content[prev..]);

            let mut g = NewlineGate::new();
            let mut reconstructed = String::new();
            for line in feed_all(&mut g, &chunks) {
                reconstructed.push_str(&line);
            }
            if let Some(rest) = g.flush() {
                reconstructed.push_str(&rest);
            }
            prop_assert_eq!(&reconstructed, &content, "内容守恒被破坏");
        }

        /// 行数上界:提交行数 ≤ 原始换行数 + 1(flush 残段)
        #[test]
        fn committed_line_count_bounded(content in "[a-z`\n ]{0,200}") {
            let mut g = NewlineGate::new();
            let lines = feed_all(&mut g, &[content.as_str()]);
            let newlines = content.chars().filter(|&c| c == '\n').count();
            prop_assert!(lines.len() <= newlines + 1);
        }
    }
}
