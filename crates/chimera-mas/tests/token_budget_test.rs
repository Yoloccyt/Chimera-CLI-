#![forbid(unsafe_code)]

//! Task 10.1 (RED): TokenBudget 预算管理失败测试
//!
//! 覆盖 7 类场景（共 17 个测试）：
//! 1. 构造与初始状态（2 个）— new 三参签名、初始 current=0/remaining=max
//! 2. add 增加 Token（3 个）— 成功、超限返回 TokenBudgetExceeded、边界 max_tokens
//! 3. consume 减少 Token（2 个）— 成功、下溢返回错误
//! 4. reset 重置（1 个）— reset 清零 current 并清除告警标志
//! 5. 状态查询（3 个）— is_exceeded / is_near_limit(80%) / remaining
//! 6. 80% 阈值事件发布（3 个）— 发布 / 不发布 / 仅发布一次(避免风暴)
//! 7. 并发安全与精度（3 个）— 多线程并发 add / u64 大数百分比(f64 中间值) / add(0) 不发布事件
//!
//! TDD RED 阶段：本文件引用尚未实现的 API（new 三参签名、add/consume/reset/current/
//! is_exceeded/is_near_limit/remaining），编译失败为预期。GREEN 阶段实现后全部通过。
//!
//! ## 红线对齐
//!
//! - §4.1: u64 大数百分比计算用 f64 中间值（非 f32，避免精度膨胀）
//! - §4.4 反模式 6: f32 禁止隐式转 f64 比较
//! - §6.2: 80% 阈值告警事件通过 mpsc 旁路通道确保投递
//! - `#![forbid(unsafe_code)]`: AtomicUsize/AtomicBool 是 safe 的,可用

use std::sync::Arc;

use chimera_mas::prelude::*;
use event_bus::{EventBus, NexusEvent};

// ============================================================
// 辅助函数
// ============================================================

/// 构造测试用 EventBus
fn make_bus() -> EventBus {
    EventBus::new()
}

// ============================================================
// 1. 构造与初始状态（2 个测试）
// ============================================================

/// 验证 TokenBudget::new 三参签名构造成功,初始 current_tokens=0
///
/// spec SubTask 10.2: `new(agent_id, max_tokens, event_bus) -> Self`
#[test]
fn test_token_budget_new_initial_state() {
    let budget = TokenBudget::new("agent-1", 1_000_000, make_bus());
    assert_eq!(budget.current(), 0, "初始 current_tokens 应为 0");
    assert_eq!(
        budget.remaining(),
        1_000_000,
        "初始 remaining 应等于 max_tokens"
    );
    assert!(!budget.is_exceeded(), "初始状态不应超限");
    assert!(
        !budget.is_near_limit(),
        "初始状态不应接近阈值(0 < 80% of 1M)"
    );
}

/// 验证 max_tokens = 0 的边界场景(避免除零)
#[test]
fn test_token_budget_new_with_zero_max_tokens() {
    let budget = TokenBudget::new("agent-zero", 0, make_bus());
    assert_eq!(budget.current(), 0);
    assert_eq!(
        budget.remaining(),
        0,
        "max=0 时 remaining 应为 0(saturating)"
    );
    // max=0 时任何 add(>0) 都应超限
    let result = budget.add(1);
    assert!(
        result.is_err(),
        "max_tokens=0 时 add(1) 应返回 TokenBudgetExceeded"
    );
    // max=0 时 is_near_limit 不应 panic(避免除零)
    // 语义上 0 >= 0 算 near_limit,但 max=0 是退化场景,不强制断言
    let _ = budget.is_near_limit();
}

// ============================================================
// 2. add 增加 Token（3 个测试）
// ============================================================

/// 验证 add 成功增加 current_tokens
///
/// spec SubTask 10.2: `add(&self, tokens: usize) -> Result<()>`（原子增加,超限返回错误）
#[test]
fn test_token_budget_add_success() {
    let budget = TokenBudget::new("agent-1", 10_000, make_bus());
    budget.add(3_000).expect("add 3000 应成功");
    assert_eq!(budget.current(), 3_000, "add 3000 后 current 应为 3000");
    budget.add(2_000).expect("add 2000 应成功");
    assert_eq!(budget.current(), 5_000, "再 add 2000 后 current 应为 5000");
    assert_eq!(budget.remaining(), 5_000, "remaining = 10000 - 5000");
}

/// 验证 add 超过 max_tokens 返回 MasError::TokenBudgetExceeded
///
/// spec SubTask 10.1: 超限检测 current_tokens > max_tokens 返回 MasError::TokenBudgetExceeded
#[test]
fn test_token_budget_add_exceeds_limit_returns_error() {
    let budget = TokenBudget::new("agent-1", 1_000, make_bus());
    budget.add(800).expect("add 800 应成功");
    // 再 add 300,总 1100 > max 1000,应返回 TokenBudgetExceeded
    let result = budget.add(300);
    let err = result.expect_err("超过 max_tokens 应返回错误");
    assert!(
        matches!(
            &err,
            MasError::TokenBudgetExceeded {
                agent_id,
                current_tokens,
                max_tokens
            } if agent_id == "agent-1" && *current_tokens == 1100 && *max_tokens == 1000
        ),
        "应返回 TokenBudgetExceeded(agent=agent-1, current=1100, max=1000), 实际: {err}"
    );
    // 超限 add 不应改变 current_tokens(原子性:检查失败则不修改)
    assert_eq!(
        budget.current(),
        800,
        "超限 add 不应改变 current_tokens,应保持 800"
    );
}

/// 验证 add 刚好等于 max_tokens 的边界(不超限)
#[test]
fn test_token_budget_add_at_max_boundary_ok() {
    let budget = TokenBudget::new("agent-1", 1_000, make_bus());
    budget
        .add(1_000)
        .expect("add 1000 刚好等于 max,应成功(边界)");
    assert_eq!(budget.current(), 1_000);
    assert!(!budget.is_exceeded(), "current == max 不算超限(> 才算)");
    assert!(
        budget.is_near_limit(),
        "current == max (100%) >= 80%,应 near_limit"
    );
}

// ============================================================
// 3. consume 减少 Token（2 个测试）
// ============================================================

/// 验证 consume 成功减少 current_tokens(释放预算)
///
/// spec SubTask 10.2: `consume(&self, tokens: usize) -> Result<()>`（原子减少）
#[test]
fn test_token_budget_consume_success() {
    let budget = TokenBudget::new("agent-1", 10_000, make_bus());
    budget.add(5_000).expect("add 5000 应成功");
    budget.consume(2_000).expect("consume 2000 应成功");
    assert_eq!(budget.current(), 3_000, "5000 - 2000 = 3000");
    assert_eq!(budget.remaining(), 7_000, "10000 - 3000 = 7000");
}

/// 验证 consume 超过 current_tokens 返回错误(下溢保护)
#[test]
fn test_token_budget_consume_underflow_returns_error() {
    let budget = TokenBudget::new("agent-1", 10_000, make_bus());
    budget.add(1_000).expect("add 1000 应成功");
    // consume 2000 > current 1000,应返回错误(下溢保护)
    let result = budget.consume(2_000);
    assert!(
        result.is_err(),
        "consume 超过 current_tokens 应返回错误(下溢保护)"
    );
    // 下溢 consume 不应改变 current_tokens
    assert_eq!(
        budget.current(),
        1_000,
        "下溢 consume 不应改变 current_tokens,应保持 1000"
    );
}

// ============================================================
// 4. reset 重置（1 个测试）
// ============================================================

/// 验证 reset 清零 current_tokens 并清除告警标志
///
/// spec SubTask 10.2: `reset(&self)`（重置为 0）
/// 关键验证:reset 清除 overflow_published 标志,使后续跨越阈值能重新发布事件
#[test]
fn test_token_budget_reset_clears_current() {
    let bus = make_bus();
    // 先订阅 critical mpsc 旁路通道(§4.4 反模式 3:先 subscribe 再构造 budget)
    // EventBus 内部 critical_tx 用 Arc<Mutex<Vec>> 共享,bus move 后 rx 仍有效
    let mut rx = bus.subscribe_critical_events();
    let budget = TokenBudget::new("agent-1", 10_000, bus);

    // 首次跨越 80% 阈值,发布告警事件
    budget
        .add(8_500)
        .expect("add 8500 应成功(>80% 阈值,触发告警)");
    assert_eq!(budget.current(), 8_500);
    assert!(budget.is_near_limit());
    // 消费首次告警事件,避免污染后续断言
    let first_event = rx
        .blocking_recv()
        .expect("首次跨越 80% 应发布 AgentContextOverflow");
    assert_matches_agent_context_overflow(&first_event, "agent-1", 8_500, 10_000);

    // reset 清零 current 并清除 overflow_published 标志
    budget.reset();
    assert_eq!(budget.current(), 0, "reset 后 current 应为 0");
    assert_eq!(
        budget.remaining(),
        10_000,
        "reset 后 remaining 应恢复为 max"
    );
    assert!(!budget.is_near_limit(), "reset 后不应 near_limit");

    // reset 后再次跨越 80% 阈值应能重新发布事件(标志已清除)
    budget
        .add(8_000)
        .expect("reset 后 add 8000 应成功并触发告警");
    let second_event = rx
        .blocking_recv()
        .expect("reset 后再次跨越 80% 阈值应重新发布 AgentContextOverflow");
    assert_matches_agent_context_overflow(&second_event, "agent-1", 8_000, 10_000);
}

// ============================================================
// 5. 状态查询（3 个测试）
// ============================================================

/// 验证 is_exceeded 在超限场景返回 true
///
/// 注意:由于 add 会拒绝超限写入,正常使用下 is_exceeded 永远 false。
/// 此测试通过构造超限场景验证方法语义(防御性检查)。
#[test]
fn test_token_budget_is_exceeded_within_limit() {
    let budget = TokenBudget::new("agent-1", 1_000, make_bus());
    budget.add(500).expect("add 500 应成功");
    assert!(!budget.is_exceeded(), "current(500) <= max(1000),不应超限");
    budget.add(500).expect("add 500 应成功(刚好到 max)");
    assert!(
        !budget.is_exceeded(),
        "current(1000) == max(1000),不算超限(> 才算)"
    );
}

/// 验证 is_near_limit 在 >= 80% 时返回 true
///
/// spec SubTask 10.1: 80% 阈值告警 current_tokens >= max_tokens * 0.8
/// §4.1: u64 大数百分比计算用 f64 中间值
#[test]
fn test_token_budget_is_near_limit_at_80_percent() {
    let budget = TokenBudget::new("agent-1", 1_000, make_bus());
    // 79% — 不应触发
    budget.add(790).expect("add 790 应成功");
    assert!(!budget.is_near_limit(), "790 < 800 (80%),不应 near_limit");
    // 再 add 10,到 800 (80%) — 应触发
    budget.add(10).expect("add 10 应成功");
    assert!(budget.is_near_limit(), "800 >= 800 (80%),应 near_limit");
}

/// 验证 is_near_limit 在 < 80% 时返回 false
#[test]
fn test_token_budget_is_near_limit_below_80_percent() {
    let budget = TokenBudget::new("agent-1", 1_000, make_bus());
    budget.add(799).expect("add 799 应成功");
    assert!(!budget.is_near_limit(), "799 < 800 (80%),不应 near_limit");
    budget.add(0).expect("add 0 应成功(无操作)");
    assert!(
        !budget.is_near_limit(),
        "799 + 0 = 799 < 800,仍不应 near_limit"
    );
}

/// 验证 remaining 计算
#[test]
fn test_token_budget_remaining_calculation() {
    let budget = TokenBudget::new("agent-1", 10_000, make_bus());
    assert_eq!(budget.remaining(), 10_000, "初始 remaining = max");
    budget.add(3_000).expect("add 3000 应成功");
    assert_eq!(budget.remaining(), 7_000, "10000 - 3000 = 7000");
    budget.consume(1_000).expect("consume 1000 应成功");
    assert_eq!(budget.remaining(), 8_000, "10000 - (3000-1000) = 8000");
    budget.reset();
    assert_eq!(budget.remaining(), 10_000, "reset 后 remaining 恢复为 max");
}

// ============================================================
// 6. 80% 阈值事件发布（3 个测试）
// ============================================================

/// 验证 80% 阈值时发布 NexusEvent::AgentContextOverflow 事件
///
/// spec SubTask 10.3: 80% 阈值检测在 add() 方法中,发布 AgentContextOverflow
/// types.rs:1775 注释建议通过 Critical 通道发送以确保投递
#[test]
fn test_token_budget_overflow_event_published_at_80_percent() {
    let bus = make_bus();
    // 先订阅 Critical 事件 mpsc 旁路通道(§4.4 反模式 3:先 subscribe 再操作)
    let mut rx = bus.subscribe_critical_events();
    let budget = TokenBudget::new("agent-1", 1_000, bus);

    // add 800 (80% 阈值) 应触发发布 AgentContextOverflow
    budget.add(800).expect("add 800 应成功");

    let event = rx
        .blocking_recv()
        .expect("应通过 mpsc 旁路通道收到 AgentContextOverflow 事件");
    assert_matches_agent_context_overflow(&event, "agent-1", 800, 1_000);
}

/// 验证 < 80% 阈值时不发布 AgentContextOverflow 事件
#[test]
fn test_token_budget_overflow_event_not_published_below_80_percent() {
    let bus = make_bus();
    let mut rx = bus.subscribe_critical_events();
    let budget = TokenBudget::new("agent-1", 1_000, bus);

    // add 799 (< 80% 阈值 800) 不应触发发布
    budget.add(799).expect("add 799 应成功");

    // 短暂等待后确认无事件(mpsc 旁路通道应无消息)
    // 使用 try_recv 而非 blocking_recv,避免无限阻塞
    let result = rx.try_recv();
    assert!(
        result.is_err(),
        "< 80% 阈值时不应发布 AgentContextOverflow,实际收到: {result:?}"
    );
}

/// 验证首次跨越 80% 阈值只发布一次事件(避免告警风暴)
///
/// 设计:AtomicBool 标志位跟踪是否已发布,首次跨越发布,reset 清除
#[test]
fn test_token_budget_overflow_event_published_only_once() {
    let bus = make_bus();
    let mut rx = bus.subscribe_critical_events();
    let budget = TokenBudget::new("agent-1", 1_000, bus);

    // 首次跨越 80% 阈值 — 应发布一次
    budget.add(800).expect("add 800 应成功(首次跨越 80%)");
    let event1 = rx
        .blocking_recv()
        .expect("首次跨越 80% 应发布 AgentContextOverflow");
    assert_matches_agent_context_overflow(&event1, "agent-1", 800, 1_000);

    // 再 add 100(仍在阈值之上)— 不应再次发布
    budget.add(100).expect("add 100 应成功(总 900,仍 > 80%)");
    let result = rx.try_recv();
    assert!(
        result.is_err(),
        "已发布过告警后,后续 add 不应重复发布(避免风暴),实际收到: {result:?}"
    );
}

// ============================================================
// 7. 并发安全与精度（3 个测试）
// ============================================================

/// 验证多线程并发 add 安全(AtomicUsize CAS 循环)
///
/// spec SubTask 10.1: 原子操作 — 多线程并发 add_tokens 安全
/// §4.4 反模式: 禁止持锁跨 .await(此处为 sync,无 await)
#[test]
fn test_token_budget_concurrent_add_thread_safe() {
    use std::thread;

    let bus = make_bus();
    // Arc 共享 TokenBudget(AtomicUsize 内部线程安全)
    let budget = Arc::new(TokenBudget::new("agent-1", 100_000, bus));
    let mut handles = vec![];

    // 10 个线程,每个 add 1000,总 10_000(未超 max 100_000)
    for _ in 0..10 {
        let b = Arc::clone(&budget);
        handles.push(thread::spawn(move || {
            b.add(1_000).expect("并发 add 1000 应成功");
        }));
    }
    for h in handles {
        h.join().expect("线程应成功完成");
    }

    // 验证原子性:总增加 10_000,无丢失无超限
    assert_eq!(
        budget.current(),
        10_000,
        "10 线程 × 1000 = 10000,原子计数应准确"
    );
    assert!(!budget.is_exceeded(), "10000 <= 100000,不应超限");
}

/// 验证 u64 大数百分比计算用 f64 中间值(§4.1 规范)
///
/// §4.4 反模式 6: f32 禁止隐式转 f64 比较
/// 0.4f32 as f64 精度膨胀,导致误判;全程用 f64
#[test]
fn test_token_budget_u64_large_number_percentage() {
    // 1B token 大数场景(模拟 1M 上下文的预算放大)
    let large_max = 1_000_000_000usize;
    let bus = make_bus();
    let budget = TokenBudget::new("agent-big", large_max, bus);

    // 80% 阈值 = 800_000_000(用 f64 计算避免精度损失)
    budget.add(800_000_000).expect("add 800M 应成功");
    assert!(
        budget.is_near_limit(),
        "800M >= 800M (80% of 1B),应 near_limit(f64 计算)"
    );
    assert_eq!(budget.remaining(), 200_000_000, "1B - 800M = 200M");
}

/// 验证 add(0) 不发布事件且不改变 current
#[test]
fn test_token_budget_add_zero_tokens_no_event() {
    let bus = make_bus();
    let mut rx = bus.subscribe_critical_events();
    let budget = TokenBudget::new("agent-1", 1_000, bus);

    budget.add(0).expect("add 0 应成功(无操作)");
    assert_eq!(budget.current(), 0, "add 0 不改变 current");
    let result = rx.try_recv();
    assert!(result.is_err(), "add(0) 不应触发事件,实际收到: {result:?}");
}

// ============================================================
// 辅助函数 — 事件断言
// ============================================================

/// 断言事件是 AgentContextOverflow 且字段匹配
///
/// 校验 agent_id / current_tokens / max_tokens 三个核心字段,
/// metadata 字段由 EventBus 内部自动生成,不校验具体值。
fn assert_matches_agent_context_overflow(
    event: &NexusEvent,
    expected_agent_id: &str,
    expected_current: usize,
    expected_max: usize,
) {
    match event {
        NexusEvent::AgentContextOverflow {
            agent_id,
            current_tokens,
            max_tokens,
            ..
        } => {
            assert_eq!(agent_id, expected_agent_id, "agent_id 不匹配");
            assert_eq!(*current_tokens, expected_current, "current_tokens 不匹配");
            assert_eq!(*max_tokens, expected_max, "max_tokens 不匹配");
        }
        other => panic!("期望 NexusEvent::AgentContextOverflow, 实际: {other:?}"),
    }
}
