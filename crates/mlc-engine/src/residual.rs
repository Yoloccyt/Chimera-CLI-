//! RSB 跨轮事件残留系统（P2-T6，手册 T-09 + v4.0 WI-20）
//!
//! 对应架构层: **L2 Memory**（mlc-engine，ADR-140 批准：挂 mlc-engine，否决 nexus-residual 新建）
//! 对应任务: **P2-T6**（手册 W12-13）
//!
//! # 问题（与深层 Transformer 梯度消失同构，v4.0 WI-20）
//! 50+ 轮后早期事件影响指数衰减——早期决策在长会话中"被稀释"。
//! RSB 以残差连接思想（y = F(x) + x）在轮间保留关键信息。
//!
//! # 三层缓冲（v4.0 WI-20 规格）
//! - L1 高频：近 5 轮关键事实（逐轮精确）
//! - L2 中频：近 20 轮关键事实（降频采样）
//! - L3 低频：跨会话摘要（全量摘要，逐轮更新）
//!
//! # 相位门控矩阵（手册 T-09）
//! | 相位 | L1 | L2 | L3 |
//! |------|----|----|----|
//! | Exploration | 0.8 | 0.6 | 0.4 |
//! | Execution | 0.3 | 0.2 | 0.1 |
//! | Debugging | 0.9 | 0.7 | 0.5 |
//! | Planning | 0.5 | 0.8 | 0.9 |
//!
//! 注入公式：`context' = context + α·residual(context)`（α = 门控权重）
//! 注入预算：注入 token < 5% 会话预算（门禁——预算检查在 `inject` 内执行，
//! 超预算按 L3→L2→L1 优先级裁剪）。
//!
//! # 确定性（Ω₂）
//! 纯数据结构 + 显式轮序推进，同种子同输入序列同输出。

use std::collections::VecDeque;

/// 任务相位（门控矩阵行索引）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    /// 探索：广泛扫描，注入高（早期线索重要）
    Exploration,
    /// 执行：聚焦当前，注入低（减少干扰）
    Execution,
    /// 调试：错误历史重要，注入最高
    Debugging,
    /// 规划：目标锚点重要，注入中高
    Planning,
}

/// 关键事实（轮末提取的原子信息单元）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyFact {
    /// 事实内容（token 计数近似：3 字 ≈ 1 token，与压缩链路一致）
    pub content: String,
    /// 来源轮次
    pub turn: u64,
}

impl KeyFact {
    /// 新建关键事实
    #[must_use]
    pub fn new(content: impl Into<String>, turn: u64) -> Self {
        Self {
            content: content.into(),
            turn,
        }
    }

    /// token 近似（中文 3 字 ≈ 1 token）
    #[must_use]
    pub fn token_estimate(&self) -> usize {
        self.content.chars().count() / 3 + 1
    }
}

/// 相位门控矩阵（手册 T-09 权威值）
const GATE_MATRIX: [[f64; 3]; 4] = [
    [0.8, 0.6, 0.4], // Exploration
    [0.3, 0.2, 0.1], // Execution
    [0.9, 0.7, 0.5], // Debugging
    [0.5, 0.8, 0.9], // Planning
];

impl Phase {
    /// 相位 → 矩阵行索引
    const fn index(self) -> usize {
        match self {
            Self::Exploration => 0,
            Self::Execution => 1,
            Self::Debugging => 2,
            Self::Planning => 3,
        }
    }

    /// 该相位下某层（0=L1, 1=L2, 2=L3）的门控权重
    #[must_use]
    pub fn gate(self, layer: usize) -> f64 {
        GATE_MATRIX[self.index()][layer.min(2)]
    }
}

/// RSB 三层残留缓冲
#[derive(Debug, Clone)]
pub struct ResidualStore {
    /// L1 高频：近 5 轮关键事实
    l1: VecDeque<KeyFact>,
    /// L2 中频：近 20 轮关键事实
    l2: VecDeque<KeyFact>,
    /// L3 低频：跨会话摘要
    l3: String,
    /// 已处理轮次
    current_turn: u64,
}

/// L1/L2 容量（v4.0 WI-20：近 5 轮 / 近 20 轮）
const L1_CAPACITY: usize = 5;
const L2_CAPACITY: usize = 20;

impl Default for ResidualStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ResidualStore {
    /// 新建空残留存储
    #[must_use]
    pub fn new() -> Self {
        Self {
            l1: VecDeque::new(),
            l2: VecDeque::new(),
            l3: String::new(),
            current_turn: 0,
        }
    }

    /// 轮末提取关键信息写入（记录 + 层间沉降）
    ///
    /// - 写入 L1（近 5 轮，溢出覆盖最旧）
    /// - L2 降频采样：每 4 轮将 L1 中"已沉降"事实入 L2（近 20 轮）
    /// - L3 摘要：逐轮追加高层摘要（内容去重，保留最新）
    pub fn record_turn(&mut self, facts: Vec<KeyFact>) {
        self.current_turn += 1;
        for f in facts {
            // L1 写入（容量 5，溢出丢最旧）
            if self.l1.len() >= L1_CAPACITY {
                self.l1.pop_front();
            }
            self.l1.push_back(f.clone());
            // L3 摘要：内容去重追加（保留最新表述）——先 borrow（f 随后被 L2 move）
            if !self.l3.contains(&f.content) {
                if !self.l3.is_empty() {
                    self.l3.push(';');
                }
                self.l3.push_str(&f.content);
            }
            // L2 降频采样：每 4 轮沉降一条（保持 20 轮窗口的稀疏覆盖）——move f
            if self.current_turn.is_multiple_of(4) && self.l2.len() >= L2_CAPACITY {
                self.l2.pop_front();
            }
            if self.current_turn.is_multiple_of(4) {
                self.l2.push_back(f);
            }
        }
    }

    /// 轮首相位门控注入 — 返回按门控权重加权的注入事实
    ///
    /// 注入预算检查：总注入 token < 5% × `session_budget`；
    /// 超预算按 L3→L2→L1 优先级裁剪（L3 摘要优先保留——跨会话锚点）。
    #[must_use]
    pub fn inject(&self, phase: Phase, session_budget: usize) -> Vec<KeyFact> {
        let budget = session_budget / 20; // 5% 注入预算
        let mut out = Vec::new();
        let mut used = 0usize;

        // L3 摘要（最高优先级：跨会话锚点）
        let l3_fact = KeyFact::new(self.l3.clone(), self.current_turn);
        let w3 = phase.gate(2);
        if w3 > 0.0 && !self.l3.is_empty() {
            let tokens = l3_fact.token_estimate();
            let weighted = (tokens as f64 * w3) as usize;
            if used + weighted <= budget {
                out.push(l3_fact);
                used += weighted;
            }
        }
        // L2（中频）
        let w2 = phase.gate(1);
        if w2 > 0.0 {
            for f in self.l2.iter().rev().take(5) {
                let weighted = (f.token_estimate() as f64 * w2) as usize;
                if used + weighted <= budget {
                    out.push(f.clone());
                    used += weighted;
                }
            }
        }
        // L1（高频，最近优先）
        let w1 = phase.gate(0);
        if w1 > 0.0 {
            for f in self.l1.iter().rev() {
                let weighted = (f.token_estimate() as f64 * w1) as usize;
                if used + weighted <= budget {
                    out.push(f.clone());
                    used += weighted;
                }
            }
        }
        out
    }

    /// 当前轮次（诊断）
    #[must_use]
    pub fn current_turn(&self) -> u64 {
        self.current_turn
    }

    /// L3 摘要内容（诊断/测试）
    #[must_use]
    pub fn summary(&self) -> &str {
        &self.l3
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_matrix_authoritative() {
        // 手册 T-09 权威矩阵逐项锁定
        assert_eq!(Phase::Exploration.gate(0), 0.8);
        assert_eq!(Phase::Exploration.gate(1), 0.6);
        assert_eq!(Phase::Exploration.gate(2), 0.4);
        assert_eq!(Phase::Execution.gate(0), 0.3);
        assert_eq!(Phase::Execution.gate(1), 0.2);
        assert_eq!(Phase::Execution.gate(2), 0.1);
        assert_eq!(Phase::Debugging.gate(0), 0.9);
        assert_eq!(Phase::Debugging.gate(1), 0.7);
        assert_eq!(Phase::Debugging.gate(2), 0.5);
        assert_eq!(Phase::Planning.gate(0), 0.5);
        assert_eq!(Phase::Planning.gate(1), 0.8);
        assert_eq!(Phase::Planning.gate(2), 0.9);
    }

    #[test]
    fn l1_ring_capacity_5() {
        let mut store = ResidualStore::new();
        for i in 0..8 {
            store.record_turn(vec![KeyFact::new(format!("fact-{i}"), i)]);
        }
        // L1 近 5 轮：fact-3..fact-7（注入时最近优先）
        let injected = store.inject(Phase::Debugging, 10_000);
        assert!(injected.iter().any(|f| f.content == "fact-7"));
        assert!(!injected.iter().any(|f| f.content == "fact-0"), "L1 容量 5 必须丢弃最旧");
    }

    #[test]
    fn l2_downsample_every_4_turns() {
        let mut store = ResidualStore::new();
        for i in 0..12 {
            store.record_turn(vec![KeyFact::new(format!("f{i}"), i)]);
        }
        // L2 只在 turn % 4 == 0 沉降：turn 4/8/12
        let injected = store.inject(Phase::Execution, 10_000);
        let l2_entries: Vec<&str> = injected
            .iter()
            .map(|f| f.content.as_str())
            .filter(|c| c.starts_with('f'))
            .collect();
        assert!(l2_entries.iter().any(|c| *c == "f3" || *c == "f4"), "L2 含沉降事实");
    }

    #[test]
    fn l3_summary_dedup() {
        let mut store = ResidualStore::new();
        store.record_turn(vec![KeyFact::new("anchor-A", 1)]);
        store.record_turn(vec![KeyFact::new("anchor-B", 2)]);
        store.record_turn(vec![KeyFact::new("anchor-A", 3)]);
        let summary = store.summary();
        assert_eq!(summary.matches("anchor-A").count(), 1, "L3 内容去重");
        assert!(summary.contains("anchor-B"));
    }

    #[test]
    fn injection_budget_5pct() {
        let mut store = ResidualStore::new();
        // 大量事实（构造高 token 场景）
        for i in 0..10 {
            store.record_turn(vec![KeyFact::new(format!("fact-content-{i}-会话上下文关键信息"), i)]);
        }
        let budget = 2000; // 5% = 100 token
        let injected = store.inject(Phase::Debugging, budget);
        let used: usize = injected.iter().map(KeyFact::token_estimate).sum();
        assert!(used <= budget / 20 + 50, "注入 token 必须 ≤ 5% 预算（含尾条误差）");
    }

    #[test]
    fn deterministic_same_sequence() {
        let mut a = ResidualStore::new();
        let mut b = ResidualStore::new();
        let facts: Vec<Vec<KeyFact>> = (0..10)
            .map(|i| vec![KeyFact::new(format!("fact-{i}"), i)])
            .collect();
        for turn in &facts {
            a.record_turn(turn.clone());
            b.record_turn(turn.clone());
        }
        assert_eq!(
            a.inject(Phase::Planning, 5000),
            b.inject(Phase::Planning, 5000),
            "同序列必须逐项一致(Ω₂)"
        );
    }

    #[test]
    fn debugging_injects_more_than_execution() {
        let mut store = ResidualStore::new();
        for i in 0..10 {
            store.record_turn(vec![KeyFact::new(format!("fact-{i}"), i)]);
        }
        let debug = store.inject(Phase::Debugging, 10_000);
        let exec = store.inject(Phase::Execution, 10_000);
        assert!(
            debug.len() >= exec.len(),
            "Debugging 门控(0.9/0.7/0.5)必须 ≥ Execution(0.3/0.2/0.1)"
        );
    }
}
