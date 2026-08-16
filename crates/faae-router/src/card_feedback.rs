//! 经验卡片反馈闭环 — ExperienceCardBus → OperatorRouter（W4，ADR-084 决策 2）
//!
//! 对应架构层: **L6 Router**（faae-router 子模块）
//! 对应设计源: 规范 §16.4 跨层事件表的 **零新增变体实现**——L7 未来的
//! `OperatorExecuted` 事件语义（算子执行反馈）由已落地的 ExperienceCardBus
//! 数据面承载（卡片天然携带 operator + score + execution_status，正是
//! `record_result` 的全部输入）。
//!
//! # 设计约束
//!
//! - **ADR-084 决策 2**: 零新增 NexusEvent 变体;L7 卡片生成器落地后
//!   本闭环自动接通真实反馈（前向兼容）
//! - **双流消费**: 中分流（0.5 < score ≤ 0.8，broadcast）+ 高分流
//!   （score > 0.8，Critical mpsc 确保送达）——高分执行反馈不因广播
//!   容量丢失;`select!` 公平轮转
//! - **§4.4 红线 1（禁止持锁跨 await）**: recv 完成后才取锁，锁内仅
//!   同步 `record_result`（短临界区）
//! - **§4.4 红线 3（先 subscribe 再 spawn）**: broadcast 仅投递给发布时
//!   已存在的 receiver——订阅在 spawn 之前同步完成
//! - **task_type 映射**: `card.method_family` 作为任务类型族键（方法族
//!   是算子路由的天然任务分类;`operator-routing` 伪卡片标记不会被
//!   发布到总线，无自回环风险）
//! - **Lagged 容错**: broadcast 滞后（容量 1024 溢出）仅记日志继续，
//!   通道关闭才终止循环

use std::sync::{Arc, Mutex};

use event_bus::ExperienceCardBus;
use nexus_contracts::ExperienceCard;

use crate::operator_router::OperatorRouter;

/// 路由器共享句柄 — 供反馈闭环与调用方并发共享
pub type SharedOperatorRouter = Arc<Mutex<OperatorRouter>>;

/// 短临界区应用卡片反馈（锁内同步，不跨 await）
fn apply_card(router: &SharedOperatorRouter, card: &ExperienceCard) {
    if let Ok(mut router) = router.lock() {
        router.record_result(
            &card.method_family,
            card.operator,
            card.score,
            card.execution_status,
        );
    }
}

/// 启动经验卡片反馈闭环（后台 tokio task）
///
/// 订阅 ExperienceCardBus 双流（中分 broadcast + 高分 Critical mpsc），
/// 逐卡片驱动 `OperatorRouter::record_result`——L6 算子路由的统计随
/// 真实执行反馈在线更新。
///
/// 返回 `JoinHandle` 供调用者管理任务生命周期（装配期调用一次）。
///
/// # 示例
///
/// ```no_run
/// use std::sync::{Arc, Mutex};
/// use event_bus::ExperienceCardBus;
/// use faae_router::OperatorRouter;
/// use faae_router::card_feedback::spawn_card_feedback_loop;
/// use nexus_contracts::OperatorSelectionStrategy;
///
/// # async fn demo() {
/// let card_bus = ExperienceCardBus::new();
/// let router = Arc::new(Mutex::new(
///     OperatorRouter::new(OperatorSelectionStrategy::ThreeFactor),
/// ));
/// let handle = spawn_card_feedback_loop(&card_bus, router.clone());
/// # }
/// ```
pub fn spawn_card_feedback_loop(
    card_bus: &ExperienceCardBus,
    router: SharedOperatorRouter,
) -> tokio::task::JoinHandle<()> {
    // 红线 §4.4-3: subscribe 必须在 spawn 之前同步调用（双流同步订阅）
    let mut rx_normal = card_bus.subscribe();
    let mut rx_critical = card_bus.subscribe_critical();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                result = rx_normal.recv() => {
                    match result {
                        Ok(card) => apply_card(&router, &card),
                        // Lagged(容量溢出) → 继续消费后续;Closed → 退出
                        Err(err) if matches!(
                            err,
                            tokio::sync::broadcast::error::RecvError::Lagged(_)
                        ) => {
                            tracing::debug!(error = %err, "反馈闭环广播滞后,继续");
                        }
                        Err(_) => break,
                    }
                }
                result = rx_critical.recv() => {
                    match result {
                        // unbounded mpsc: Some(卡片) → 应用;None = 发送端全部丢弃
                        Some(card) => apply_card(&router, &card),
                        None => break,
                    }
                }
            }
        }
    })
}
