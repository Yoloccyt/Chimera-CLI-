//! Composer 历史导航器(Concord W6 T6.2,方案 §7.2 Composer 历史 ↑↓)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! 主流 Agent CLI composer 均支持 ↑↓ 回溯输入历史(Claude Code/Codex 共识)。
//! 导航器为纯函数状态机:历史条目队列 + 导航位置 + 草稿暂存,与 TuiState
//! 解耦(TuiState 持有数据,本模块提供语义操作),便于单测与 proptest 守护。
//!
//! # 语义约定
//! - `commit`:空串忽略;与队尾连续重复去重;容量上限 `pop_front` O(1) 淘汰;
//! - `prev`:首次回溯保存当前草稿,返回更旧条目;到顶保持(不越界);
//! - `forward`:向新前进;回底时恢复进入历史前的草稿;
//! - `reset`:提交后复位导航位置(草稿清空)。

use std::collections::VecDeque;

/// 历史容量上限(防无限增长;100 条足够会话回溯,内存占用可忽略)
pub const HISTORY_CAPACITY: usize = 100;

/// Composer 历史导航器
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ComposerHistory {
    /// 历史条目(队尾最新)
    pub entries: VecDeque<String>,
    /// 当前导航位置(None = 未处于回溯态,编辑的是实时草稿)
    pub pos: Option<usize>,
    /// 进入回溯前的草稿(回底时恢复)
    pub draft: String,
}

impl ComposerHistory {
    /// 创建空导航器
    pub fn new() -> Self {
        Self::default()
    }

    /// 从既有历史构造(持久化恢复用)
    pub fn from_entries(entries: VecDeque<String>) -> Self {
        Self {
            entries,
            pos: None,
            draft: String::new(),
        }
    }

    /// 提交一条输入入史并复位导航
    ///
    /// # 语义
    /// - 空串(trim 后)忽略,不入史;
    /// - 与队尾完全相同时去重(连续重复提交不堆叠);
    /// - 超容量时淘汰最旧(`pop_front` O(1))。
    pub fn commit(&mut self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            self.reset();
            return;
        }
        let duplicate = self
            .entries
            .back()
            .map(|last| last == trimmed)
            .unwrap_or(false);
        if !duplicate {
            if self.entries.len() >= HISTORY_CAPACITY {
                self.entries.pop_front();
            }
            self.entries.push_back(trimmed.to_string());
        }
        self.reset();
    }

    /// 回溯上一条:返回应填入 composer 的文本
    ///
    /// # 参数
    /// - `current`:composer 当前缓冲(首次回溯时作为草稿保存)
    ///
    /// # 返回
    /// Some(历史条目) 或 None(无历史可回溯)。
    pub fn prev(&mut self, current: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        match self.pos {
            None => {
                // 首次回溯:保存草稿,定位到最新条目
                self.draft = current.to_string();
                let idx = self.entries.len() - 1;
                self.pos = Some(idx);
                Some(self.entries[idx].clone())
            }
            Some(0) => {
                // 已到最旧:保持,不越界
                Some(self.entries[0].clone())
            }
            Some(idx) => {
                let next_idx = idx - 1;
                self.pos = Some(next_idx);
                Some(self.entries[next_idx].clone())
            }
        }
    }

    /// 前进一条:返回应填入 composer 的文本
    ///
    /// WHY 命名 forward 而非 next:避免与 std::iter::Iterator::next
    /// 语义混淆(clippy::should_implement_trait)。
    ///
    /// # 返回
    /// Some(更新条目或恢复的草稿) 或 None(未处于回溯态,无操作)。
    pub fn forward(&mut self) -> Option<String> {
        let idx = self.pos?;
        if idx + 1 >= self.entries.len() {
            // 回底:恢复草稿,退出回溯态
            self.pos = None;
            Some(std::mem::take(&mut self.draft))
        } else {
            let next_idx = idx + 1;
            self.pos = Some(next_idx);
            Some(self.entries[next_idx].clone())
        }
    }

    /// 复位导航状态(提交后调用)
    pub fn reset(&mut self) {
        self.pos = None;
        self.draft.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn commit_stores_and_dedups_consecutive() {
        let mut h = ComposerHistory::new();
        h.commit("a");
        h.commit("a"); // 连续重复去重
        h.commit("b");
        h.commit("  "); // 空白忽略
        let v: Vec<&str> = h.entries.iter().map(|s| s.as_str()).collect();
        assert_eq!(v, vec!["a", "b"]);
    }

    #[test]
    fn commit_capacity_evicts_oldest() {
        let mut h = ComposerHistory::new();
        for i in 0..(HISTORY_CAPACITY + 5) {
            h.commit(&format!("cmd-{i}"));
        }
        assert_eq!(h.entries.len(), HISTORY_CAPACITY);
        assert_eq!(h.entries.front().unwrap(), "cmd-5", "最旧 5 条应被淘汰");
        assert_eq!(
            h.entries.back().unwrap(),
            &format!("cmd-{}", HISTORY_CAPACITY + 4)
        );
    }

    #[test]
    fn prev_next_navigation_with_draft_restore() {
        let mut h = ComposerHistory::new();
        h.commit("one");
        h.commit("two");
        // 带草稿进入回溯
        assert_eq!(h.prev("draft-text"), Some("two".to_string()));
        assert_eq!(h.prev("ignored"), Some("one".to_string()));
        // 到顶保持
        assert_eq!(h.prev("ignored"), Some("one".to_string()));
        // 前进回 two
        assert_eq!(h.forward(), Some("two".to_string()));
        // 回底恢复草稿
        assert_eq!(h.forward(), Some("draft-text".to_string()));
        assert_eq!(h.pos, None, "回底应退出回溯态");
        // 未回溯时 forward 无操作
        assert_eq!(h.forward(), None);
    }

    #[test]
    fn prev_on_empty_history_is_none() {
        let mut h = ComposerHistory::new();
        assert_eq!(h.prev("anything"), None);
    }

    #[test]
    fn commit_resets_navigation() {
        let mut h = ComposerHistory::new();
        h.commit("a");
        h.prev("");
        assert!(h.pos.is_some());
        h.commit("b");
        assert_eq!(h.pos, None, "提交后应复位导航");
        assert!(h.draft.is_empty());
    }

    proptest! {
        /// 不变量:任意 commit/prev/next 交错序列后——
        /// ① entries 无连续重复;② 长度 ≤ 容量;③ prev 不越界(pos ≤ len-1)
        #[test]
        fn navigation_invariants(
            commits in proptest::collection::vec("[a-c]{0,4}", 0..40),
            nav_seed in proptest::collection::vec(any::<bool>(), 0..60),
        ) {
            let mut h = ComposerHistory::new();
            let mut it = nav_seed.iter();
            for c in &commits {
                h.commit(c);
                // 随机穿插导航
                for _ in 0..2 {
                    match it.next() {
                        Some(true) => { h.prev("d"); }
                        Some(false) => { h.forward(); }
                        None => break,
                    }
                }
            }
            // ② 容量界
            prop_assert!(h.entries.len() <= HISTORY_CAPACITY);
            // ① 无连续重复
            for w in h.entries.as_slices().0.windows(2) {
                prop_assert_ne!(&w[0], &w[1]);
            }
            for w in h.entries.as_slices().1.windows(2) {
                prop_assert_ne!(&w[0], &w[1]);
            }
            // 跨切片边界的连续对(仅当两切片均非空)
            if let (Some(last_a), Some(first_b)) =
                (h.entries.as_slices().0.last(), h.entries.as_slices().1.first())
            {
                prop_assert_ne!(last_a, first_b);
            }
            // ③ pos 界
            if let Some(p) = h.pos {
                prop_assert!(p < h.entries.len());
            }
        }

        /// prev 到底后连续 prev 恒返回最旧条目(不越界)
        #[test]
        fn prev_bottom_stays(n in 1usize..10) {
            let mut h = ComposerHistory::new();
            for i in 0..n {
                h.commit(&format!("c{i}"));
            }
            for _ in 0..(n + 5) {
                let got = h.prev("x").expect("有历史应返回");
                if got == "c0" { break; }
            }
            // 再继续 prev 应一直是 c0
            for _ in 0..3 {
                prop_assert_eq!(h.prev("x"), Some("c0".to_string()));
            }
        }
    }
}
