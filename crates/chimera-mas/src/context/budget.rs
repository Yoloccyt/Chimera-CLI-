//! TokenBudget Token 预算管理 — 原子并发安全的上下文预算跟踪
//!
//! 架构层归属: L9 Quest(chimera-mas 内部子模块)
//! 核心职责: 跟踪 Agent 上下文 Token 使用量,80% 阈值发布告警,100% 阈值拒绝写入
//!
//! ## ADR-026 决策 2: AgentContextOverflow 走 mpsc 旁路通道
//!
//! `NexusEvent::AgentContextOverflow` 的 `severity()` 返回 `Normal`(同步函数不依赖
//! 运行时值),但语义上是告警,发布者应通过 Critical 通道发送以确保投递
//! (见 `event-bus/src/types.rs:1771-1775` 注释)。本实现使用
//! `EventBus::publish_critical_blocking()` 走 broadcast + mpsc 双通道,
//! 符合 §6.2 红线"Critical 安全事件用 mpsc channel 确保送达"的精神。
//!
//! ## 阈值规则
//!
//! - **80% 阈值**: 首次跨越时发布 `AgentContextOverflow` 告警,触发上下文压缩
//!   (使用 `AtomicBool` 标志位避免事件风暴,`reset` 清除标志允许重新告警)
//! - **100% 阈值**: 拒绝新 Token 写入,返回 `MasError::TokenBudgetExceeded`
//!
//! ## 并发安全
//!
//! - `current_tokens` 用 `AtomicUsize`,支持多线程并发 `add`/`consume`
//! - `add` 使用 `compare_exchange_weak` CAS 循环实现"检查并设置"原子语义
//!   (简单 `fetch_add` 无法满足"超限则不修改"的原子性要求)
//! - `overflow_published` 用 `AtomicBool`,确保多线程首次跨越阈值只发布一次事件
//!
//! ## 红线对齐
//!
//! - §4.1: u64 大数百分比计算用 f64 中间值(非 f32,避免精度膨胀)
//! - §4.4 反模式 6: f32 禁止隐式转 f64 比较,全程用 f64
//! - §6.2: 80% 阈值告警通过 mpsc 旁路通道确保投递
//! - `#![forbid(unsafe_code)]`: AtomicUsize/AtomicBool 是 safe 的,可用

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use event_bus::{EventBus, EventMetadata, NexusEvent};

use crate::error::{MasError, Result};

/// 80% 告警阈值(0.0-1.0)
///
/// WHY 用 f64 而非 f32:§4.4 反模式 6,f32 转 f64 精度膨胀导致误判
/// (如 0.8f32 as f64 > 0.8)。全程用 f64 计算避免精度问题。
const WARNING_THRESHOLD: f64 = 0.8;

/// Token 预算管理器 — 跟踪 Agent 上下文 Token 使用量(线程安全)
///
/// 使用 `AtomicUsize` 实现无锁并发计数,支持多线程并发 `add`/`consume`。
/// 80% 阈值首次跨越时发布 `NexusEvent::AgentContextOverflow` 告警,
/// 100% 阈值拒绝写入并返回 `MasError::TokenBudgetExceeded`。
///
/// ## 字段说明
///
/// - `agent_id`: 所属 Agent ID(用于事件发布与错误诊断)
/// - `max_tokens`: 最大 Token 预算(1M 等效 = 128K 实际 + 8× 稀疏)
/// - `current_tokens`: 当前已用 Token 数(AtomicUsize,线程安全)
/// - `event_bus`: 事件总线(发布 AgentContextOverflow 告警)
/// - `overflow_published`: 80% 阈值告警已发布标志(AtomicBool,避免事件风暴)
///
/// ## Clone 语义
///
/// Clone 创建独立副本(AtomicUsize/AtomicBool 各自独立计数),
/// 不共享状态。若需共享计数,调用方应使用 `Arc<TokenBudget>`。
///
/// ## 示例
///
/// ```no_run
/// use chimera_mas::prelude::*;
/// use event_bus::EventBus;
///
/// let budget = TokenBudget::new("agent-1", 1_000_000, EventBus::new());
/// let _ = budget.add(500_000); // 增加 500K Token 使用量
/// assert_eq!(budget.current(), 500_000);
/// assert_eq!(budget.remaining(), 500_000);
/// ```
pub struct TokenBudget {
    /// 所属 Agent ID
    pub agent_id: String,
    /// 最大 Token 预算
    pub max_tokens: usize,
    /// 当前已用 Token 数(AtomicUsize,线程安全)
    current_tokens: AtomicUsize,
    /// 事件总线(发布 AgentContextOverflow 告警)
    event_bus: EventBus,
    /// 80% 阈值告警已发布标志(AtomicBool,避免事件风暴)
    ///
    /// WHY AtomicBool:多线程并发 add 跨越阈值时,只有一个线程 CAS 成功并发布事件,
    /// 其他线程 CAS 失败跳过发布。reset 时清除为 false,允许再次告警。
    overflow_published: AtomicBool,
}

impl TokenBudget {
    /// 创建新的 Token 预算管理器
    ///
    /// ## 参数
    /// - `agent_id`: 所属 Agent ID
    /// - `max_tokens`: 最大 Token 预算(1M 等效 = 128K 实际 + 8× 稀疏)
    /// - `event_bus`: 事件总线(用于发布 AgentContextOverflow 告警)
    ///
    /// ## 示例
    ///
    /// ```no_run
    /// use chimera_mas::prelude::*;
    /// use event_bus::EventBus;
    ///
    /// let budget = TokenBudget::new("agent-1", 1_048_576, EventBus::new());
    /// assert_eq!(budget.current(), 0);
    /// assert_eq!(budget.remaining(), 1_048_576);
    /// ```
    pub fn new(agent_id: impl Into<String>, max_tokens: usize, event_bus: EventBus) -> Self {
        Self {
            agent_id: agent_id.into(),
            max_tokens,
            current_tokens: AtomicUsize::new(0),
            event_bus,
            overflow_published: AtomicBool::new(false),
        }
    }

    /// 获取当前已用 Token 数(原子读取)
    ///
    /// 使用 `Acquire` 顺序确保后续操作看到最新值。
    pub fn current(&self) -> usize {
        self.current_tokens.load(Ordering::Acquire)
    }

    /// 增加 Token 使用量(原子操作,超限返回错误)
    ///
    /// 使用 `compare_exchange_weak` CAS 循环实现"检查并设置"原子语义:
    /// 简单 `fetch_add` 无法满足"超限则不修改"的原子性要求(多线程可能同时
    /// 通过检查再增加,导致超限)。CAS 循环确保检查与修改作为一个不可分割的步骤。
    ///
    /// 成功增加后,检查 80% 阈值,首次跨越时发布 `AgentContextOverflow` 告警。
    ///
    /// ## 参数
    /// - `tokens`: 要增加的 Token 数
    ///
    /// ## 返回
    /// - `Ok(())`: 增加成功
    /// - `Err(MasError::TokenBudgetExceeded)`: 超出 max_tokens,current_tokens 不变
    ///
    /// ## 错误处理
    ///
    /// 超限时返回错误,current_tokens 保持不变(原子性:检查失败则不修改)。
    /// `current_tokens` 字段记录尝试达到的总量(含被拒绝的 tokens),便于错误诊断。
    pub fn add(&self, tokens: usize) -> Result<()> {
        // CAS 循环:确保"检查超限 + 更新 current"的原子性
        let mut current = self.current_tokens.load(Ordering::Acquire);
        loop {
            // checked_add 处理 usize 加法溢出(极大 tokens 场景)
            let new_total = current.checked_add(tokens).ok_or_else(|| {
                MasError::TokenBudgetExceeded {
                    agent_id: self.agent_id.clone(),
                    // 溢出时用 usize::MAX 表示尝试达到的总量(诊断用)
                    current_tokens: usize::MAX,
                    max_tokens: self.max_tokens,
                }
            })?;
            if new_total > self.max_tokens {
                return Err(MasError::TokenBudgetExceeded {
                    agent_id: self.agent_id.clone(),
                    // current_tokens 记录尝试达到的总量(含被拒绝部分),便于诊断
                    current_tokens: new_total,
                    max_tokens: self.max_tokens,
                });
            }
            // compare_exchange_weak:Spurious failure 可能发生,用 loop 重试
            match self.current_tokens.compare_exchange_weak(
                current,
                new_total,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => {
                    // 其他线程修改了 current,用实际值重试
                    current = actual;
                }
            }
        }
        // 成功增加后检查 80% 阈值并发布告警
        self.maybe_publish_overflow();
        Ok(())
    }

    /// 减少 Token 使用量(原子操作,下溢返回错误)
    ///
    /// 语义:释放已用的 Token 预算(如上下文块被清理、缓存被驱逐)。
    ///
    /// WHY 命名为 consume 而非 release:遵循 spec Task 10.2 方法签名,
    /// spec 明确要求 consume 为"原子减少"。虽然语义上 consume 通常指
    /// "消耗"(增加使用量),但本实现遵循 spec 字面要求。
    ///
    /// ## 参数
    /// - `tokens`: 要减少的 Token 数
    ///
    /// ## 返回
    /// - `Ok(())`: 减少成功
    /// - `Err(MasError::Internal)`: 下溢(tokens > current_tokens),current_tokens 不变
    ///
    /// ## 错误处理
    ///
    /// 下溢时返回错误,current_tokens 保持不变(原子性:检查失败则不修改)。
    /// 避免使用 `saturating_sub` 静默截断,因为下溢通常意味着调用方逻辑错误
    /// (如释放了未分配的预算),应显式报错而非静默忽略。
    pub fn consume(&self, tokens: usize) -> Result<()> {
        // CAS 循环:确保"检查下溢 + 更新 current"的原子性
        let mut current = self.current_tokens.load(Ordering::Acquire);
        loop {
            if tokens > current {
                return Err(MasError::Internal(format!(
                    "TokenBudget consume underflow: agent={}, trying to consume {} but only {} available",
                    self.agent_id, tokens, current
                )));
            }
            let new_total = current - tokens;
            match self.current_tokens.compare_exchange_weak(
                current,
                new_total,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => {
                    current = actual;
                }
            }
        }
        Ok(())
    }

    /// 重置当前 Token 使用量为 0,并清除告警标志
    ///
    /// 语义:Agent 上下文被完全清空(如 Agent 重启、上下文归档后释放)。
    /// 清除 `overflow_published` 标志,使后续跨越 80% 阈值能重新发布告警。
    pub fn reset(&self) {
        self.current_tokens.store(0, Ordering::Release);
        self.overflow_published.store(false, Ordering::Release);
    }

    /// 检查是否超出最大预算(current_tokens > max_tokens)
    ///
    /// 注意:由于 `add` 会拒绝超限写入,正常使用下此方法永远返回 false。
    /// 保留此方法用于防御性检查(如外部直接修改 current_tokens 的未来扩展场景)。
    pub fn is_exceeded(&self) -> bool {
        self.current() > self.max_tokens
    }

    /// 检查是否接近阈值(current_tokens >= max_tokens * 80%)
    ///
    /// 使用 f64 中间值计算阈值(§4.1 规范:u64 大数百分比用 f64,
    /// §4.4 反模式 6:f32 禁止隐式转 f64 比较,避免精度膨胀)。
    ///
    /// ## 边界场景
    ///
    /// - `max_tokens == 0`: 返回 false(退化场景,避免除零与误告警)
    /// - `current_tokens == max_tokens`: 返回 true(100% >= 80%)
    pub fn is_near_limit(&self) -> bool {
        if self.max_tokens == 0 {
            return false;
        }
        // u64 大数百分比用 f64 中间值(§4.1 规范)
        let threshold = (self.max_tokens as f64 * WARNING_THRESHOLD) as usize;
        self.current() >= threshold
    }

    /// 获取剩余可用 Token 数(max_tokens - current_tokens)
    ///
    /// 使用 `saturating_sub` 避免下溢(虽然 `add` 拒绝超限,但防御性编程)。
    pub fn remaining(&self) -> usize {
        self.max_tokens.saturating_sub(self.current())
    }

    /// 检查 80% 阈值并发布 AgentContextOverflow 告警(内部方法)
    ///
    /// 设计要点:
    /// 1. 读取当前 current_tokens
    /// 2. 计算 80% 阈值(用 f64 中间值,§4.1 规范)
    /// 3. 若 current >= threshold,用 CAS 设置 overflow_published = true
    ///    - 成功:首次跨越阈值,发布事件
    ///    - 失败:已发布过告警,跳过(避免事件风暴)
    /// 4. 用 `publish_critical_blocking` 确保 mpsc 旁路投递
    ///    (types.rs:1775 注释建议通过 Critical 通道发送)
    ///
    /// WHY 用 AtomicBool CAS 而非 Mutex:无锁,无 await,符合 §4.4 反模式 1
    /// (禁止持锁跨 .await,此处为 sync 无 await,但 CAS 更轻量)。
    fn maybe_publish_overflow(&self) {
        // max=0 退化场景,不发布告警(避免 0 >= 0 误判)
        if self.max_tokens == 0 {
            return;
        }
        let current = self.current_tokens.load(Ordering::Acquire);
        // u64 大数百分比用 f64 中间值(§4.1 规范,避免 f32 精度膨胀 §4.4 反模式 6)
        let threshold = (self.max_tokens as f64 * WARNING_THRESHOLD) as usize;
        if current < threshold {
            return;
        }
        // 首次跨越阈值时发布,避免事件风暴(AtomicBool CAS)
        // compare_exchange 返回 Ok 表示原值为 false 且已成功设置为 true
        if self
            .overflow_published
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let event = NexusEvent::AgentContextOverflow {
                metadata: EventMetadata::new("chimera-mas"),
                agent_id: self.agent_id.clone(),
                current_tokens: current,
                max_tokens: self.max_tokens,
            };
            // 用 publish_critical_blocking 确保 mpsc 旁路投递(types.rs:1775 注释建议)
            // WHY 忽略返回值:发布失败不应阻塞 add 操作,告警丢失由订阅者侧处理
            let _ = self.event_bus.publish_critical_blocking(event);
        }
    }
}

impl Clone for TokenBudget {
    /// 手动实现 Clone:创建独立副本(AtomicUsize/AtomicBool 各自独立计数)
    ///
    /// WHY 手动实现:AtomicUsize/AtomicBool 在 stable Rust 不实现 Clone
    /// (仅在 nightly `atomic_from_mut` feature 下可用)。手动 Clone 语义为
    /// "读取当前值并创建新原子变量",与派生 Clone 语义一致(值语义复制)。
    ///
    /// Clone 后两个 TokenBudget 独立计数,修改一个不影响另一个。
    /// 若需共享计数,调用方应使用 `Arc<TokenBudget>`。
    fn clone(&self) -> Self {
        Self {
            agent_id: self.agent_id.clone(),
            max_tokens: self.max_tokens,
            current_tokens: AtomicUsize::new(self.current_tokens.load(Ordering::Acquire)),
            event_bus: self.event_bus.clone(),
            overflow_published: AtomicBool::new(self.overflow_published.load(Ordering::Acquire)),
        }
    }
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self::new("default", 1_000_000, EventBus::new())
    }
}

impl fmt::Debug for TokenBudget {
    /// 手动实现 Debug,避免依赖 EventBus 的 Debug 实现(EventBus 未派生 Debug)
    ///
    /// WHY 手动实现:与 AgentContext(manager.rs:168)保持一致风格,
    /// 避免泄露 EventBus 内部状态(broadcast::Sender / mpsc Vec 等)。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenBudget")
            .field("agent_id", &self.agent_id)
            .field("max_tokens", &self.max_tokens)
            .field(
                "current_tokens",
                &self.current_tokens.load(Ordering::Relaxed),
            )
            .field(
                "overflow_published",
                &self.overflow_published.load(Ordering::Relaxed),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 辅助:创建测试用 EventBus
    fn make_bus() -> EventBus {
        EventBus::new()
    }

    #[test]
    fn test_token_budget_new_initial_state() {
        let budget = TokenBudget::new("agent-1", 1_000_000, make_bus());
        assert_eq!(budget.current(), 0);
        assert_eq!(budget.remaining(), 1_000_000);
        assert!(!budget.is_exceeded());
        assert!(!budget.is_near_limit());
    }

    #[test]
    fn test_token_budget_add_success() {
        let budget = TokenBudget::new("agent-1", 10_000, make_bus());
        budget.add(3_000).unwrap();
        assert_eq!(budget.current(), 3_000);
        budget.add(2_000).unwrap();
        assert_eq!(budget.current(), 5_000);
        assert_eq!(budget.remaining(), 5_000);
    }

    #[test]
    fn test_token_budget_add_exceeds_limit_returns_error() {
        let budget = TokenBudget::new("agent-1", 1_000, make_bus());
        budget.add(800).unwrap();
        let result = budget.add(300);
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            MasError::TokenBudgetExceeded {
                agent_id,
                current_tokens,
                max_tokens,
            } if agent_id == "agent-1" && current_tokens == 1100 && max_tokens == 1000
        ));
        // 超限 add 不应改变 current_tokens(原子性)
        assert_eq!(budget.current(), 800);
    }

    #[test]
    fn test_token_budget_add_at_max_boundary_ok() {
        let budget = TokenBudget::new("agent-1", 1_000, make_bus());
        budget.add(1_000).unwrap();
        assert_eq!(budget.current(), 1_000);
        assert!(!budget.is_exceeded());
        assert!(budget.is_near_limit());
    }

    #[test]
    fn test_token_budget_consume_success() {
        let budget = TokenBudget::new("agent-1", 10_000, make_bus());
        budget.add(5_000).unwrap();
        budget.consume(2_000).unwrap();
        assert_eq!(budget.current(), 3_000);
        assert_eq!(budget.remaining(), 7_000);
    }

    #[test]
    fn test_token_budget_consume_underflow_returns_error() {
        let budget = TokenBudget::new("agent-1", 10_000, make_bus());
        budget.add(1_000).unwrap();
        let result = budget.consume(2_000);
        assert!(result.is_err());
        assert_eq!(budget.current(), 1_000);
    }

    #[test]
    fn test_token_budget_reset_clears_current() {
        let budget = TokenBudget::new("agent-1", 10_000, make_bus());
        budget.add(8_500).unwrap();
        assert_eq!(budget.current(), 8_500);
        budget.reset();
        assert_eq!(budget.current(), 0);
        assert_eq!(budget.remaining(), 10_000);
    }

    #[test]
    fn test_token_budget_is_near_limit_at_80_percent() {
        let budget = TokenBudget::new("agent-1", 1_000, make_bus());
        budget.add(790).unwrap();
        assert!(!budget.is_near_limit());
        budget.add(10).unwrap();
        assert!(budget.is_near_limit());
    }

    #[test]
    fn test_token_budget_concurrent_add_thread_safe() {
        use std::sync::Arc;
        use std::thread;

        let budget = Arc::new(TokenBudget::new("agent-1", 100_000, make_bus()));
        let mut handles = vec![];
        for _ in 0..10 {
            let b = Arc::clone(&budget);
            handles.push(thread::spawn(move || {
                b.add(1_000).unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(budget.current(), 10_000);
    }

    #[test]
    fn test_token_budget_u64_large_number_percentage() {
        let large_max = 1_000_000_000usize;
        let budget = TokenBudget::new("agent-big", large_max, make_bus());
        budget.add(800_000_000).unwrap();
        assert!(budget.is_near_limit());
        assert_eq!(budget.remaining(), 200_000_000);
    }

    #[test]
    fn test_token_budget_debug_format() {
        let budget = TokenBudget::new("agent-1", 1_000, make_bus());
        budget.add(500).unwrap();
        let debug_str = format!("{budget:?}");
        assert!(debug_str.contains("agent-1"));
        assert!(debug_str.contains("1000"));
        assert!(debug_str.contains("500"));
    }

    #[test]
    fn test_token_budget_clone_independent_state() {
        let budget = TokenBudget::new("agent-1", 10_000, make_bus());
        budget.add(5_000).unwrap();
        let cloned = budget.clone();
        // Clone 创建独立副本,修改 clone 不影响原对象
        cloned.add(3_000).unwrap();
        assert_eq!(budget.current(), 5_000, "原对象 current 不应改变");
        assert_eq!(cloned.current(), 8_000, "clone current 应为 8000");
    }

    #[test]
    fn test_token_budget_default() {
        let budget = TokenBudget::default();
        assert_eq!(budget.agent_id, "default");
        assert_eq!(budget.max_tokens, 1_000_000);
        assert_eq!(budget.current(), 0);
    }
}
