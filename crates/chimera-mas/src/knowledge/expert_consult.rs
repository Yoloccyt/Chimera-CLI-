//! ExpertConsultant — 专家旗舰咨询(Task 18 §18.3-18.4,§18.8)
//!
//! 架构层归属:L9 Quest(chimera-mas knowledge 子模块)
//! 核心职责:为 Agent 提供专家旗舰模型咨询能力,带 SLA 超时与并发信号量控制,
//! 防止旗舰模型过载(上限 = CPU 核数 × 2)。
//!
//! ## SLA 紧急度映射(§18.3)
//!
//! | ConsultUrgency | 超时上限 | 适用场景 |
//! |----------------|---------|---------|
//! | Critical | 5s | 阻断 Quest 的关键决策 |
//! | High | 15s | 影响任务进度的咨询 |
//! | Medium | 30s | 常规咨询 |
//! | Low | 60s | 非紧急咨询 |
//!
//! ## 并发控制(§18.4)
//!
//! `tokio::sync::Semaphore` 限制并发咨询数 ≤ CPU 核数 × 2,防止旗舰模型过载。
//! 超出并发上限的请求会等待(`acquire().await`),不会立即失败。
//!
//! ## 超时处理(§6.2 红线)
//!
//! 超时返回 `MasError::ExpertUnavailable { reason: "timeout after Xs" }`,
//! 并发布 `NexusEvent::AgentTaskFailed`(Critical,走 mpsc 旁路投递)。
//!
//! ## 关键约束
//!
//! - `tokio::broadcast` 先 subscribe 再 spawn(§4.4 反模式 3)
//! - Critical 安全事件用 mpsc(AgentTaskFailed 走 publish_critical,§6.2 红线)
//! - 不引入 unwrap/expect(§4.1)

use std::sync::Arc;

use event_bus::{ConsultUrgency, EventBus, EventMetadata, NexusEvent};
use tokio::sync::Semaphore;
use tokio::time::{timeout, Duration};

use crate::error::{MasError, Result};

/// 专家咨询 SLA 配置 — 按 ConsultUrgency 映射超时秒数(§18.3)
///
/// WHY 独立常量而非散落在代码中:SLA 是合同性约束,集中声明便于
/// 审计追溯与未来调整(参考 ADR-026 决策 4 不引入 sqlite-vec)。
#[derive(Debug, Clone, Copy)]
pub struct ConsultSla {
    /// Critical 级超时(秒,默认 5)
    pub critical_s: u64,
    /// High 级超时(秒,默认 15)
    pub high_s: u64,
    /// Medium 级超时(秒,默认 30)
    pub medium_s: u64,
    /// Low 级超时(秒,默认 60)
    pub low_s: u64,
}

impl Default for ConsultSla {
    fn default() -> Self {
        Self {
            critical_s: 5,
            high_s: 15,
            medium_s: 30,
            low_s: 60,
        }
    }
}

impl ConsultSla {
    /// 按紧急度获取超时秒数
    pub fn timeout_s(&self, urgency: ConsultUrgency) -> u64 {
        match urgency {
            ConsultUrgency::Critical => self.critical_s,
            ConsultUrgency::High => self.high_s,
            ConsultUrgency::Medium => self.medium_s,
            ConsultUrgency::Low => self.low_s,
        }
    }
}

/// 专家旗舰咨询器 — 封装专家咨询的 SLA + 信号量 + 超时处理
///
/// ## 设计要点
///
/// - **信号量并发控制**:`max_concurrent = CPU 核数 × 2`,防止旗舰模型过载(§18.4)
/// - **SLA 超时**:按 ConsultUrgency 自动选择 5s/15s/30s/60s,超时发 AgentTaskFailed
/// - **EventBus 复用**:不新建 AgentMessageBus,通过 AgentConsultRequested/Responded 通信
pub struct ExpertConsultant {
    /// 事件总线(复用,不新建 AgentMessageBus,ADR-026 决策 2)
    bus: EventBus,
    /// 并发信号量(上限 = CPU 核数 × 2)
    semaphore: Arc<Semaphore>,
    /// 并发上限(Semaphore 初始许可数,§18.4)
    /// WHY 独立存储:tokio::sync::Semaphore 不暴露 max_permits 查询方法,
    /// 为支持 `max_concurrent()` 查询,需独立存储
    max_concurrent: usize,
    /// SLA 配置
    sla: ConsultSla,
    /// 默认超时秒数(用于构造时指定,consult 调用时按 urgency 覆盖)
    default_timeout_s: u64,
}

impl ExpertConsultant {
    /// 创建专家咨询器
    ///
    /// ## 参数
    /// - `bus`:事件总线(复用)
    /// - `max_concurrent`:并发上限(默认 = CPU 核数 × 2)
    /// - `timeout_s`:默认超时秒数(用于 ConsultUrgency::Low 兜底)
    pub fn new(bus: EventBus, max_concurrent: usize, timeout_s: u64) -> Self {
        Self {
            bus,
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            max_concurrent,
            sla: ConsultSla::default(),
            default_timeout_s: timeout_s,
        }
    }

    /// 默认并发上限 = CPU 核数 × 2(§18.4,防旗舰模型过载)
    ///
    /// WHY ×2 而非 ×1:CPU 核数 × 2 是 Tokio 阻塞线程池常用上界,
    /// 平衡并发吞吐与旗舰模型推理队列深度(参考 Tokio doc:blocking pool 默认 512)。
    /// 对咨询场景而言,2× 核数已足够让旗舰模型饱和,超过此值会导致推理排队恶化 SLA。
    pub fn default_max_concurrent() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get() * 2)
            .unwrap_or(8)
    }

    /// 获取信号量当前可用许可数(供测试与监控查询)
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// 获取并发上限(信号量总数)
    pub fn max_concurrent(&self) -> usize {
        self.max_concurrent
    }

    /// 向专家发起咨询 — 经 AgentConsultRequested 事件 + 等待 AgentConsultResponded
    ///
    /// ## 参数
    /// - `expert_id`:被咨询专家 Agent ID
    /// - `urgency`:咨询紧急度(决定 SLA 超时)
    ///
    /// ## 返回
    /// - `Ok(String)`:专家回答内容
    /// - `Err(MasError::ExpertUnavailable)`:专家超时 / 未响应
    ///
    /// ## 错误处理
    ///
    /// 超时发布 `NexusEvent::AgentTaskFailed`(Critical,走 mpsc,§6.2 红线),
    /// 由 Parliament 进行补救决策(重试 / 降级 / 转交其他专家)。
    pub async fn consult(&self, expert_id: &str, urgency: ConsultUrgency) -> Result<String> {
        // 1. 按 urgency 选择 SLA 超时(§18.3)
        let sla_timeout_s = self.sla.timeout_s(urgency);
        let _ = self.default_timeout_s; // 兜底字段保留,Low 级用 sla.low_s

        // 2. 信号量 acquire(§18.4,防止旗舰模型过载)
        //    WHY 不用 acquire_owned():需要保持 self.semaphore 引用计数,
        //    permit 在 await 完成后自动 drop 释放
        let _permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| MasError::Internal(format!("Semaphore acquire failed: {e}")))?;

        // 3. 构造 AgentConsultRequested 事件
        let request_event = NexusEvent::AgentConsultRequested {
            metadata: EventMetadata::new("chimera-mas/expert-consultant"),
            from: "chimera-mas".to_string(),
            to: expert_id.to_string(),
            question: "expert consultation".to_string(),
            context: String::new(), // 上下文由调用方在外层注入,这里保持空(脱敏后)
            urgency,
        };

        // 4. 先订阅再 publish(§4.4 反模式 3:broadcast 不缓存历史)
        //    WHY 先 subscribe:publish 后事件立即广播,若订阅在 publish 之后,
        //    AgentConsultResponded 事件会被静默丢失(Week 6 SSRA 教训)
        let mut rx = self.bus.subscribe();

        // 5. publish AgentConsultRequested
        self.bus.publish(request_event).await.map_err(|e| {
            MasError::Internal(format!("Publish AgentConsultRequested failed: {e}"))
        })?;

        // 6. 等待 AgentConsultResponded 事件,带 SLA 超时
        let sla_duration = Duration::from_secs(sla_timeout_s);
        let result = timeout(sla_duration, async {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if let NexusEvent::AgentConsultResponded { to, answer, .. } = &event {
                            if to == "chimera-mas" {
                                return Ok(answer.clone());
                            }
                        }
                    }
                    Err(_e) => {
                        // broadcast 通道错误(LagPaused / Closed),返回 Internal
                        return Err(MasError::Internal(
                            "EventBus subscribe channel closed".to_string(),
                        ));
                    }
                }
            }
        })
        .await;

        // 7. 处理超时与结果
        match result {
            Ok(inner_result) => inner_result,
            Err(_elapsed) => {
                // SLA 超时:发布 AgentTaskFailed Critical 事件(§6.2 红线,走 mpsc)
                // WHY await:publish_critical 是 async 方法,需 await 确保事件实际发布到
                // mpsc 旁路通道(§4.4 反模式 7:Critical 事件不可 fire-and-forget)
                let _ = self
                    .bus
                    .publish_critical(NexusEvent::AgentTaskFailed {
                        metadata: EventMetadata::new("chimera-mas/expert-consultant"),
                        from: expert_id.to_string(),
                        to: "chimera-mas".to_string(),
                        task_id: format!("consult-{expert_id}"),
                        error: format!(
                            "Expert consultation timeout after {sla_timeout_s}s (urgency={urgency:?})"
                        ),
                        retry_count: 0,
                    })
                    .await;
                Err(MasError::ExpertUnavailable {
                    expert_id: expert_id.to_string(),
                    reason: format!("timeout after {sla_timeout_s}s"),
                })
            }
        }
    }
}
