//! GQEP 聚集器 — 并发异步操作的聚集汇聚核心
//!
//! 基于 `FuturesUnordered` 流式处理(对应 A.2 设计决策),
//! 集成 QEEP `OrphanDetector` 实现零孤儿调用保证。
//!
//! # 设计决策
//! - **FuturesUnordered vs join_all**:FuturesUnordered 支持流式处理
//!   (完成一个处理一个),内存占用更低,首个完成可立即处理。
//!   `join_all` 在 1000 个 Future 同时聚集时内存峰值高。
//! - **QEEP 集成**:每个 future 经 `QeepProtocol::entangle` 包裹,
//!   利用 `OrphanGuard` 在 future drop 时检测孤儿调用。
//!   WHY entangle:QEEP 内部维护 `pending_calls` 与 `OrphanDetector`,
//!   future 正常完成标记 `completed=true`,异常 drop 则报告孤儿。

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use event_bus::{EventBus, EventMetadata, NexusEvent};
use futures::stream::{FuturesUnordered, StreamExt};
use qeep_protocol::{QeepError as QeepErr, QeepProtocol};
use tracing::warn;

use crate::config::GqepConfig;
use crate::error::GqepError;
use crate::types::{GatherCollected, GatherResult, GqepFuture};

/// `collect_bounded` 的采集统计（crate 私有）
///
/// WHY 结构体而非 4/6 元组：本批新增 `abandoned` / `not_started` 后，
/// 多值返回靠位置区分会读错且编译器兜不住（两个都是 u32）；
/// 命名字段同时戒掉「记账公式在两处各写一遍」的双源风险。
struct GatherStats<T> {
    /// 成功数
    succeeded: u32,
    /// 成功值（完成序）
    values: Vec<T>,
    /// 失败数（含单操作超时）
    failed: u32,
    /// 逐条错误（全局超时时额外含一条 `GlobalTimedOut`）
    errors: Vec<GqepError>,
    /// 已入流（被 poll 过）但被全局 deadline 放弃——有 `OrphanGuard` 归因
    abandoned: u32,
    /// 从未入流、从未被 poll——**没有** `OrphanGuard`，只能靠本字段显式归因
    not_started: u32,
}

/// GQEP 执行器 — 并发异步操作的聚集汇聚核心
///
/// 基于 `FuturesUnordered` 流式处理,集成 QEEP `OrphanDetector`
/// 实现零孤儿调用保证。
///
/// # 线程安全
/// `GqepExecutor` 内部所有字段均为线程安全(`EventBus` 与 `QeepProtocol`
/// 均基于 `Arc` 实现 `Clone` 廉价)。但 `gather` 方法为 `&self`,
/// 保证多次聚集调用共享同一 `QeepProtocol` 实例(统一孤儿检测)。
pub struct GqepExecutor {
    /// 执行配置(超时、并发度、原子性开关)
    pub(crate) config: GqepConfig,
    /// 事件总线,用于发布聚集完成/超时/孤儿事件
    pub(crate) event_bus: EventBus,
    /// QEEP 协议实例,提供孤儿调用检测(entangle 包裹 + OrphanGuard)
    pub(crate) qeep: QeepProtocol,
}

impl GqepExecutor {
    /// 创建新的 GQEP 执行器
    ///
    /// # 参数
    /// - `config`:执行配置(超时、并发度、原子性)
    /// - `event_bus`:事件总线,用于发布 `GatherCompleted`/`OrphanCallDetected` 事件
    ///
    /// # 内部初始化
    /// 创建 `QeepProtocol` 实例,默认超时取自 `config.default_timeout_ms`。
    /// 该超时作用于 `entangle` 包裹的每个 future(单操作超时)。
    pub fn new(config: GqepConfig, event_bus: EventBus) -> Self {
        // P1-T12 接入(灰度验证「公共 API 零感知」):
        // GatherCompleted/OperationTimedOut 等高频非 Critical 事件走分片扇出
        // (worker 汇入既有 broadcast,订阅者 API 零变化);无 tokio runtime 时
        // enable_sharding 返回 Err 即降级单流(零回归)
        // A3/ADR-197:启用结果可见化（内部按级别记日志），不再静默忽略 Err
        event_bus.try_enable_sharding("gqep-executor");
        let default_timeout = std::time::Duration::from_millis(config.default_timeout_ms);
        let qeep = QeepProtocol::new(default_timeout);
        Self {
            config,
            event_bus,
            qeep,
        }
    }

    /// 聚集执行多个异步操作
    ///
    /// 使用 `FuturesUnordered` 流式处理,每个 future 经 QEEP `entangle` 包裹。
    /// 聚集完成后发布 `GatherCompleted` 事件,并检查孤儿调用报告。
    ///
    /// # 双层超时防护
    /// - **单操作超时**:`entangle` 内部用 `tokio::time::timeout` 包裹每个 future
    ///   (阈值取自 `config.default_timeout_ms`),超时返回 `OperationTimeout`。
    /// - **全局超时**:整个采集循环用 `tokio::time::timeout_at` 包裹
    ///   (阈值取自 `config.gather_deadline_ms`),超时追加 `GlobalTimedOut` 错误并
    ///   发布 `GatherTimedOut` 事件。`gather_deadline_ms = 0` 时禁用(向后兼容)。
    ///   WHY 全局超时:大规模 gather 时单操作超时累积可能导致整体执行时间失控,
    ///   全局 deadline 为整批操作兜底(对应架构红线"所有异步操作必须有 GQEP
    ///   聚集/超时处理")。WHY `timeout_at`:并发上界下多批必须共享**同一个绝对
    ///   时刻**,逐批重置相对超时会把总墙钟放大成 `deadline × 批数`(ADR-202 I4)。
    ///
    /// # 并发上界(ADR-202 Option B)与**契约变更**(Annex C-6)
    /// 入参 `futures` **不再保证全部被执行**：实际在飞数受
    /// [`GqepConfig::max_concurrency`] 约束（钳制到 `[1, MAX_GATHER_FANOUT]`），
    /// 超出的部分排在 backlog 里等前一个完成才入流。若全局 deadline 先触发,
    /// 留在 backlog 的元素**从未被 poll**，因而**不会触发 QEEP `OrphanGuard`**
    /// （注册发生在 async 体首次执行）——它们不产生孤儿事件,只计入返回值的
    /// `not_started` 与 `GatherTimedOut` 事件字段。
    ///
    /// WHY 调用方必须读这条：`not_started` 兼有**重试幂等判据**——它表示「根本没跑」,
    /// 与 `abandoned`（已执行但被放弃）不可合并；副作用型/计费型调用方只有区分
    /// 两者才能安全重试（合并则重试可能重复副作用或漏掉未做的工）。
    /// 需要「全部必执行」的调用方应保证 `gather_deadline_ms = 0`（不限时）或把
    /// `max_concurrency` 设为不小于入参数量。
    ///
    /// # 流程
    /// 1. 记录开始时间
    /// 2. 每个 future 经 `entangle` 包裹(实现孤儿检测 + 单操作超时)
    /// 3. 按上界分批入流，`collect_bounded` 流式处理 + 单一绝对 deadline 包裹
    /// 4. 四段守恒记账（`succeeded + failed + abandoned + not_started == total`）
    /// 5. 检查 `orphan_reports`,发布 `OrphanCallDetected` 事件(Critical)
    /// 6. 发布 `GatherCompleted` 事件
    ///
    /// # 参数
    /// - `futures`:待聚集的异步操作列表（见上「契约变更」：不保证全部被执行）
    ///
    /// # 返回
    /// 聚集结果统计（总数/成功数/失败数/放弃数/未启动数/延迟/错误列表）。
    ///
    /// 全局超时时，未完成的操作**不计入 `failed`**而是分成 `abandoned`（已入流
    /// 被 poll、但被全局 deadline 放弃）与 `not_started`（从未入流），同时
    /// `errors` 追加一个 `GlobalTimedOut`（区分于单操作超时，故此时
    /// `errors.len() == failed + 1`）。
    pub async fn gather(&self, futures: Vec<GqepFuture<String>>) -> GatherResult {
        self.gather_generic(futures).await.stats
    }

    /// 聚集执行多个异步操作,并保留成功操作的返回值(泛型版,M12 / ADR-185 D3)
    ///
    /// 与 [`gather`](Self::gather) 共享同一泛型内核,行为契约完全一致:
    /// 相同的 QEEP entangle 孤儿检测、相同的双层超时防护、相同的
    /// **并发上界与「入参可能完全不执行」契约**（见 [`gather`](Self::gather)
    /// 的「契约变更」段；`values.len()` 等于 `stats.succeeded` 而非入参个数）、
    /// 相同的 `GatherCompleted` 事件发布;唯一区别是本方法把成功操作的 `T` 值
    /// 随统计一并返回,供调用方按自身 tag 还原顺序(见 `GatherCollected`)。
    ///
    /// WHY 泛型化而非 String 特判:chimera-cli doctor 探针并行化需要回传
    /// `HealthCheck` 值;泛型 `<T: Send + 'static>` 由 `GqepFuture<T>` 天然承载,
    /// 零额外 trait 约束。既有 `gather` 收敛为 `.stats` 薄包装,签名与语义零变化。
    ///
    /// # 参数
    /// - `futures`:待聚集的异步操作列表(`Vec<GqepFuture<T>>`)
    ///
    /// # 返回
    /// 带值聚集结果 [`GatherCollected`]——`stats` 为聚集统计(与 `gather` 同语义),
    /// `values` 为成功操作返回值集合(完成序,不保证输入序)
    pub async fn gather_collected<T: Send + 'static>(
        &self,
        futures: Vec<GqepFuture<T>>,
    ) -> GatherCollected<T> {
        self.gather_generic(futures).await
    }

    /// 泛型聚集内核(`gather` / `gather_collected` 共用,M12)
    ///
    /// 流程与原子性保证与原 `gather` 逐行等价(仅 `String` 特化提升为 `<T>`):
    /// 1. 每个 future 经 `entangle` 包裹(孤儿检测 + 单操作超时)
    /// 2. `collect_with_deadline` 流式处理 + 全局 deadline 包裹
    /// 3. 检查 `orphan_reports`,发布 `OrphanCallDetected` 事件(Critical)
    /// 4. 发布 `GatherCompleted` 事件
    async fn gather_generic<T: Send + 'static>(
        &self,
        futures: Vec<GqepFuture<T>>,
    ) -> GatherCollected<T> {
        let total = futures.len() as u32;
        let start = Instant::now();
        // 并发上界首次成为真实约束（ADR-202 Option B）：bound 经
        // `GqepConfig::effective_bound` 钳制到 [1, MAX_GATHER_FANOUT]，
        // 故 `0` 等价串行、`10^9` 被截到静态帽，两者都不会退化为「无限制」。
        let bound = self.config.effective_bound();

        // 将每个 future 经 QEEP entangle 包裹。
        // WHY entangle:利用 OrphanGuard 在 future drop 时检测孤儿调用,
        // 同时 entangle 内部用 tokio::time::timeout 提供单操作超时保护。
        //
        // WHY 先全部包装再分波入流（而非包装与入流合并）：包装本身**零副作用**——
        // QEEP 注册与 OrphanGuard 创建都发生在 async 体**首次 poll**
        // （`qeep-protocol/src/protocol.rs:283-284` 源码自述），故预包装不会
        // 产生在飞额度；真正决定并发的是何时把元素 push 进 stream。
        let qeep = self.qeep.clone();
        let wrapped: Vec<GqepFuture<T>> = futures
            .into_iter()
            .map(move |future| -> GqepFuture<T> {
                let qeep = qeep.clone();
                // 将 GqepFuture 转换为 entangle 要求的 Future<Output=Result<T, QeepError>>
                // 内部做 GqepError <-> QeepError 错误映射(entangle 要求 QeepError)
                Box::pin(async move {
                    let mapped: Pin<Box<dyn Future<Output = Result<T, QeepErr>> + Send>> =
                        Box::pin(async move { future.await.map_err(map_gqep_to_qeep) });
                    // entangle 提供孤儿检测 + 单操作超时
                    qeep.entangle(mapped).await.map_err(map_qeep_to_gqep)
                })
            })
            .collect();

        // 分批入流：首波至多 bound 个，其余留在 backlog 队尾等完成一个补一个。
        // WHY 载体始终是 `FuturesUnordered`（不换成 buffer_unordered）：
        // 后者未拉取的源项**从未被 poll**，在全局超时被 drop 时**无任何孤儿归因**，
        // 直接把可观测的放弃面换成静默丢失（破 UNLEARNABLE #3）——ADR-202 因此否决。
        // 本实现只改**入流节奏**，不改数据结构，故不违 `agents.md` 「并发收集用
        // FuturesUnordered」红线（ADR-202 §6 裁决）。
        let mut backlog: VecDeque<GqepFuture<T>> = wrapped.into_iter().collect();
        // WHY 不加 `mut`：`FuturesUnordered::push` 取 `&self`（内部无锁队列），
        // 只有下游消费它的地方才需要可变借用。
        let stream: FuturesUnordered<GqepFuture<T>> = FuturesUnordered::new();
        while stream.len() < bound {
            let Some(next) = backlog.pop_front() else {
                break;
            };
            stream.push(next);
        }

        // 流式收集结果,应用全局 gather deadline(双层超时的外层)
        // WHY 提取为独立方法:将"全局超时包裹"与"统计"职责分离,
        // 保持 gather 主方法简洁(架构红线:单函数 ≤200 行)
        //
        // 记账单点：四段守恒（I5）全部由 `collect_bounded` 算出，此处**只搬运**。
        // WHY 不在此重算：公式写两处就成双源，一旦漂移（例如漏了补波后的
        // backlog 变化）守恒会**假绿**——断测只比两边自洽，察不到公式分叉。
        let GatherStats {
            succeeded,
            values,
            failed,
            errors,
            abandoned,
            not_started,
        } = self
            .collect_bounded(stream, &mut backlog, start, total)
            .await;

        let latency_ms = start.elapsed().as_secs_f32() * 1000.0;

        // SubTask 24.5:检查孤儿调用并发布 Critical 事件
        // WHY 在 gather 中检查:孤儿可能由 future 内部 spawn 子任务且不 await 引起,
        // QEEP OrphanGuard 会在子任务 drop 时报告。gather 聚集后统一发布事件。
        // NOTE:全局超时丢弃的未完成 future 也会触发 OrphanGuard(预期行为:
        // 被放弃的 future 本质即孤儿调用,系统应感知并告警)。
        self.publish_orphan_events().await;

        let result = GatherResult {
            total,
            succeeded,
            failed,
            abandoned,
            not_started,
            latency_ms,
            errors,
        };

        // 发布聚集完成事件
        let event = NexusEvent::GatherCompleted {
            metadata: EventMetadata::new("gqep-executor"),
            total: result.total,
            succeeded: result.succeeded,
            failed: result.failed,
            latency_ms: result.latency_ms,
        };
        if let Err(e) = self.event_bus.publish(event).await {
            warn!(error = %e, "发布聚集完成事件失败");
        }

        GatherCollected {
            stats: result,
            values,
        }
    }

    /// 流式收集 `FuturesUnordered` 结果,并应用全局 gather deadline
    ///
    /// 这是双层超时的外层:用 `tokio::time::timeout` 包裹整个 `stream.next()` 循环。
    /// 与 `entangle` 内部的单操作超时互补:
    /// - 单操作超时保护单个 future(超时返回 `OperationTimeout`,该 future 计入 failed)
    /// - 全局超时保护整个 gather 流程(超时后未完成 future 全部放弃,返回 `GlobalTimedOut`)
    ///
    /// # 全局超时触发时的副作用
    /// `tokio::time::timeout` 超时会 drop 内部的 `collect_all` future,进而 drop
    /// `FuturesUnordered` 及其中所有未完成的 entangle future。由于 `entangle` 仅在
    /// 其内部超时/完成时才把 `OrphanGuard.completed` 置 true,被外层 drop 的 entangle
    /// future 仍处于 `completed=false`,会触发 `OrphanGuard::drop` 报告孤儿。这是
    /// **预期行为**:被全局超时放弃的 future 本质上就是孤儿调用(结果未被收集),
    /// 系统应通过 `OrphanCallDetected` 事件感知并告警(由 `publish_orphan_events` 广播)。
    ///
    /// # 借用细节(反模式规避)
    /// `collect_all` 以可变引用捕获计数器。`tokio::time::timeout(deadline, collect_all).await`
    /// 产生的 `Timeout` 临时量在 Edition 2021 下会存活到所在 match 语句结束,导致 match
    /// 分支无法 move 计数器。故先 `let outcome = ...;` 绑定,使临时量在分号处 drop、
    /// 释放借用,再 match outcome(对应 §4.4 async 反模式经验)。
    ///
    /// # 参数
    /// - `stream`:首波已入流的 `FuturesUnordered`（个数 ≤ bound）
    /// - `backlog`:尚未入流的尾部。本方法按 FIFO 从队首补波；结束后其
    ///   `len()` 即为**从未 poll** 的操作数（`not_started`）。
    /// - `start`:gather 开始时刻(用于计算 elapsed_ms)
    /// - `total`:总操作数(用于计算被放弃数,发布事件)
    ///
    /// # 返回
    /// [`GatherStats`]：四段守恒的全部计数 + 成功值 + 错误列表。
    ///
    /// WHY `backlog` 以 `&mut` 借入而非移入：`VecDeque` 一旦被 move 进采集闭包，
    /// 超时 drop 后调用方就读不到剩余量了；借用使 `not_started` 的读取与
    /// 归因责任能集中在本函数**单一记账点**。
    async fn collect_bounded<T: Send + 'static>(
        &self,
        mut stream: FuturesUnordered<GqepFuture<T>>,
        backlog: &mut VecDeque<GqepFuture<T>>,
        start: Instant,
        total: u32,
    ) -> GatherStats<T> {
        let mut succeeded: u32 = 0;
        let mut values: Vec<T> = Vec::new();
        let mut failed: u32 = 0;
        let mut errors: Vec<GqepError> = Vec::new();

        let deadline_ms = self.config.gather_deadline_ms;

        // 采集 + 补波：每收到一个结果就从 backlog 取一个补进 stream，
        // 故 stream 长度恒 ≤ bound（I6 在飞峰值的执行侧保证）。
        // WHY 把 drain 逻辑写**一份**而不是 0 分支 / deadline 分支各一份：
        // 两份副本一旦漏改其中一份就会静默分叉（本函数两分支只差一层 timeout 包裹）。
        let collect = async {
            while let Some(result) = stream.next().await {
                match result {
                    Ok(v) => {
                        succeeded += 1;
                        values.push(v);
                    }
                    Err(e) => {
                        failed += 1;
                        errors.push(e);
                    }
                }
                // 补波：固定 FIFO 波序，禁止按结果价值重排（否则引入不确定性 / R4 尾部饥饿）
                if let Some(next) = backlog.pop_front() {
                    stream.push(next);
                }
            }
        };

        // deadline_ms=0 表示禁用全局超时(向后兼容,行为与无全局超时一致)
        // WHY 0=禁用:与 with_timeout 的 timeout_ms=0 语义保持一致。
        // 限流下的语义：上界只**延后**不**拒绝**（I7），故全部操作终将执行。
        let mut timed_out = false;
        if deadline_ms == 0 {
            collect.await;
        } else {
            // I4：所有批次共享同一个绝对时刻，而不是每批一个相对 duration。
            // WHY `timeout_at` 而非 `timeout(relative)`：多批场景下若逐批重置，
            // 总墙钟会变成 `deadline × 批数`——即 ADR-202 §7 R1 要防的放大路径。
            let deadline_at = tokio::time::Instant::now() + Duration::from_millis(deadline_ms);
            // 先绑定 outcome:使 Timeout 临时量在分号处 drop,释放对计数器与 backlog 的借用,
            // 否则下方无法读 backlog.len()、也无法 move succeeded/failed/errors
            // (Edition 2021 临时量生命周期)
            let outcome = tokio::time::timeout_at(deadline_at, collect).await;
            timed_out = outcome.is_err();
        }

        // 单一记账点（I5 四段守恒，红线级）：
        // - `not_started` = backlog 剩余——它们从未 poll、**无** OrphanGuard、不会有孤儿事件，
        //   本字段是唯一显式归因出口（没有它就是静默消失，违 UNLEARNABLE #3）。
        // - `abandoned` = 已入流但未完成 = (total - not_started) - succeeded - failed。
        let not_started = backlog.len() as u32;
        let abandoned = (total - not_started).saturating_sub(succeeded + failed);

        if timed_out {
            // 全局超时:记录 GlobalTimedOut(区分单操作超时,供调用者决策)
            let elapsed_ms = start.elapsed().as_millis() as u64;

            // 发布 GatherTimedOut 事件(可观测性:供 efficiency-monitor 记录全局超时指标)
            // WHY 事件同时带 abandoned 与 not_started：两者语义不可合并（前者重试可能
            // 重复副作用，后者可安全重试），监控面需要分开归因。
            let event = NexusEvent::GatherTimedOut {
                metadata: EventMetadata::new("gqep-executor"),
                deadline_ms,
                elapsed_ms,
                total,
                abandoned,
                not_started,
            };
            if let Err(e) = self.event_bus.publish(event).await {
                warn!(error = %e, "发布 gather 全局超时事件失败");
            }

            // Phase 7 D-6: 全局超时真实计数点
            crate::record_global_timeout();

            errors.push(GqepError::GlobalTimedOut {
                deadline_ms,
                elapsed_ms,
            });
        }

        GatherStats {
            succeeded,
            values,
            failed,
            errors,
            abandoned,
            not_started,
        }
    }

    /// 发布所有已检测到的孤儿调用事件
    ///
    /// 从 QEEP `OrphanDetector` 获取孤儿报告,逐个发布 `OrphanCallDetected` 事件。
    /// 事件为 Critical 级别(对应 Claude Code 尸检 5.4% 孤儿调用教训),
    /// 发布失败仅记录 warn 日志,不阻塞聚集流程。
    pub(crate) async fn publish_orphan_events(&self) {
        let orphan_reports = self.qeep.orphan_reports();
        for report in &orphan_reports {
            let event = NexusEvent::OrphanCallDetected {
                metadata: EventMetadata::new("gqep-executor"),
                operation_id: report.call_id.0.to_string(),
                spawn_location: report.reason.clone(),
            };
            if let Err(e) = self.event_bus.publish(event).await {
                warn!(error = %e, "发布孤儿调用事件失败");
            }
        }
    }

    /// 获取当前待处理(pending)操作数量
    ///
    /// 反映当前正在执行(未完成)的纠缠调用数。
    pub fn pending_count(&self) -> usize {
        self.qeep.pending_count()
    }

    /// 获取已完成操作数量(成功 + 失败 + 超时,不含孤儿)
    pub fn completed_count(&self) -> usize {
        self.qeep.completed_count()
    }

    /// 获取孤儿调用数量
    ///
    /// 对应 Claude Code 尸检 5.4% 孤儿调用指标,用于监控告警。
    pub fn orphan_count(&self) -> usize {
        self.qeep.orphan_count()
    }

    /// 创建纠缠后台任务(委托 QEEP entangle_spawn)
    ///
    /// WHY 公开此方法:集成测试需通过 entangle_spawn + abort 验证孤儿检测,
    /// 内部字段 qeep 为 pub(crate),外部测试无法直接访问
    pub fn entangle_spawn<F, T>(&self, future: F) -> tokio::task::JoinHandle<Result<T, QeepErr>>
    where
        F: std::future::Future<Output = Result<T, QeepErr>> + Send + 'static,
        T: Send + 'static,
    {
        self.qeep.entangle_spawn(future)
    }

    /// 获取配置引用
    pub fn config(&self) -> &GqepConfig {
        &self.config
    }
}

/// GqepError → QeepError 映射
///
/// WHY:QEEP `entangle` 要求 future 输出 `Result<T, QeepError>`,
/// 需将 GQEP 错误转换为 QEEP 错误以接入纠缠协议。
/// 映射保留语义(超时→超时,孤儿→孤儿),其他错误映射到 `SerializationError`
/// (复用 QEEP 的通用错误变体,携带原始原因字符串)。
pub(crate) fn map_gqep_to_qeep(e: GqepError) -> QeepErr {
    match e {
        GqepError::OperationTimeout { .. } => QeepErr::Timeout,
        GqepError::OrphanCallDetected { .. } => QeepErr::Orphaned,
        GqepError::OperationFailed { reason, .. } => QeepErr::SerializationError(reason),
        GqepError::BatchAtomicFailure { reason, .. } => QeepErr::SerializationError(reason),
        // WHY 此分支不应被触达:GlobalTimedOut 是 gather 级错误,在 collect_with_deadline
        // 中直接生成并入 GatherResult.errors,不会经 entangle 流转。此映射仅为保持
        // match 穷尽性(新增错误变体必须显式处理,防止遗漏)。
        GqepError::GlobalTimedOut { deadline_ms, .. } => {
            QeepErr::SerializationError(format!("global gather timeout: deadline_ms={deadline_ms}"))
        }
    }
}

/// QeepError → GqepError 映射
///
/// WHY:`entangle` 返回 `Result<T, QeepError>`,需转回 GQEP 错误以保持 API 一致性。
/// QEEP 的 `Timeout`/`Orphaned` 语义明确,直接映射;
/// 其他错误映射到 `OperationFailed`(携带 QEEP 错误描述)。
pub(crate) fn map_qeep_to_gqep(e: QeepErr) -> GqepError {
    match e {
        QeepErr::Timeout => GqepError::OperationTimeout {
            operation_id: String::new(),
            timeout_ms: 0,
        },
        QeepErr::Orphaned => GqepError::OrphanCallDetected {
            operation_id: String::new(),
            spawn_location: String::new(),
        },
        QeepErr::Cancelled => GqepError::OperationFailed {
            operation_id: String::new(),
            reason: "调用被取消".to_string(),
        },
        QeepErr::AlreadyCompleted => GqepError::OperationFailed {
            operation_id: String::new(),
            reason: "调用已完成,不能重复操作".to_string(),
        },
        QeepErr::AckMissing => GqepError::OperationFailed {
            operation_id: String::new(),
            reason: "缺少确认(Ack)".to_string(),
        },
        QeepErr::ReceiptMissing => GqepError::OperationFailed {
            operation_id: String::new(),
            reason: "缺少回执(Receipt)".to_string(),
        },
        QeepErr::SerializationError(s) => GqepError::OperationFailed {
            operation_id: String::new(),
            reason: s,
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;
    use std::time::Duration;

    /// 创建立即成功的 future
    fn make_success_future(value: &str) -> GqepFuture<String> {
        let value = value.to_string();
        Box::pin(async move { Ok(value) })
    }

    /// 创建立即失败的 future
    fn make_failure_future(reason: &str) -> GqepFuture<String> {
        let reason = reason.to_string();
        Box::pin(async move {
            Err(GqepError::OperationFailed {
                operation_id: String::new(),
                reason,
            })
        })
    }

    #[tokio::test]
    async fn test_gather_all_success() {
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let futures = vec![
            make_success_future("a"),
            make_success_future("b"),
            make_success_future("c"),
        ];
        let result = executor.gather(futures).await;

        assert_eq!(result.total, 3);
        assert_eq!(result.succeeded, 3);
        assert_eq!(result.failed, 0);
        assert!(result.errors.is_empty());
        assert!(result.is_all_success());
        assert_eq!(executor.orphan_count(), 0);
    }

    #[tokio::test]
    async fn test_gather_partial_failure() {
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let futures = vec![
            make_success_future("a"),
            make_failure_future("fail-1"),
            make_success_future("c"),
        ];
        let result = executor.gather(futures).await;

        assert_eq!(result.total, 3);
        assert_eq!(result.succeeded, 2);
        assert_eq!(result.failed, 1);
        assert_eq!(result.errors.len(), 1);
        assert!(!result.is_all_success());
        assert_eq!(executor.orphan_count(), 0);
    }

    #[tokio::test]
    async fn test_gather_all_failure() {
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let futures = vec![make_failure_future("fail-1"), make_failure_future("fail-2")];
        let result = executor.gather(futures).await;

        assert_eq!(result.total, 2);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 2);
        assert_eq!(result.errors.len(), 2);
        assert_eq!(executor.orphan_count(), 0);
    }

    #[tokio::test]
    async fn test_gather_empty() {
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let result = executor.gather(vec![]).await;

        assert_eq!(result.total, 0);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 0);
    }

    #[tokio::test]
    async fn test_gather_publishes_completed_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let executor = GqepExecutor::new(GqepConfig::default(), bus.clone());

        let futures = vec![make_success_future("a"), make_success_future("b")];
        let _ = executor.gather(futures).await;

        // 应收到 GatherCompleted 事件
        let event = rx.recv_timeout(Duration::from_millis(100)).await;
        assert!(event.is_ok(), "应收到事件");
        let event = event.unwrap();
        assert!(
            matches!(
                event,
                NexusEvent::GatherCompleted {
                    total: 2,
                    succeeded: 2,
                    failed: 0,
                    ..
                }
            ),
            "应为 GatherCompleted 事件,实际: {:?}",
            event
        );
    }

    #[tokio::test]
    async fn test_gather_with_slow_operations() {
        // 验证 FuturesUnordered 流式处理:慢操作不阻塞快操作的结果收集
        let executor = GqepExecutor::new(
            GqepConfig {
                default_timeout_ms: 5000,
                ..Default::default()
            },
            EventBus::new(),
        );

        let futures = vec![
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok("slow".to_string())
            }) as GqepFuture<String>,
            Box::pin(async { Ok("fast".to_string()) }) as GqepFuture<String>,
        ];

        let start = Instant::now();
        let result = executor.gather(futures).await;
        let elapsed = start.elapsed();

        assert_eq!(result.succeeded, 2);
        // 流式处理:快操作立即完成,慢操作 50ms,总耗时约 50ms(并发)
        assert!(
            elapsed < Duration::from_millis(200),
            "并发聚集应快于串行,实际耗时: {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn test_orphan_detection_via_entangle_spawn_abort() {
        // SubTask 24.5:验证孤儿调用检测
        // 方法:用 entangle_spawn 创建后台任务,abort 后触发 OrphanGuard drop
        let bus = EventBus::new();
        let executor = GqepExecutor::new(GqepConfig::default(), bus);

        // 创建一个长时间运行的纠缠调用,然后 abort
        let handle = executor.qeep.entangle_spawn(async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok("done".to_string())
        });

        // 等待任务启动(被 poll,OrphanGuard 已创建)
        tokio::time::sleep(Duration::from_millis(50)).await;

        // abort 任务,触发 future drop,OrphanGuard 检测到未完成
        handle.abort();
        // 等待 abort 生效与 Drop 执行
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 应检测到孤儿调用
        assert!(
            executor.orphan_count() > 0,
            "abort 后应检测到孤儿调用,实际: {}",
            executor.orphan_count()
        );
    }

    #[tokio::test]
    async fn test_orphan_event_published_on_gather() {
        // SubTask 24.5:验证孤儿调用事件发布
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let executor = GqepExecutor::new(GqepConfig::default(), bus.clone());

        // 创建孤儿:entangle_spawn + abort
        let handle = executor.qeep.entangle_spawn(async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok("done".to_string())
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(executor.orphan_count() > 0, "应已检测到孤儿");

        // 调用 gather,触发孤儿事件发布
        let _ = executor
            .gather(vec![Box::pin(async { Ok("ok".to_string()) })])
            .await;

        // 接收事件:应有 OrphanCallDetected(Critical)和 GatherCompleted
        let mut found_orphan = false;
        for _ in 0..5 {
            match rx.recv_timeout(Duration::from_millis(100)).await {
                Ok(NexusEvent::OrphanCallDetected { .. }) => {
                    found_orphan = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert!(found_orphan, "应发布 OrphanCallDetected 事件(Critical)");
    }

    #[tokio::test]
    async fn test_no_orphan_on_normal_completion() {
        // 验证正常完成的操作不会产生孤儿
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let futures = vec![
            make_success_future("a"),
            make_success_future("b"),
            make_failure_future("c"),
        ];
        let _ = executor.gather(futures).await;

        assert_eq!(
            executor.orphan_count(),
            0,
            "正常完成(含失败)的操作不应产生孤儿"
        );
    }

    #[test]
    fn test_error_mapping_roundtrip() {
        // 验证 GqepError <-> QeepError 映射
        let gqep_err = GqepError::OperationTimeout {
            operation_id: "op-1".into(),
            timeout_ms: 1000,
        };
        let qeep_err = map_gqep_to_qeep(gqep_err);
        assert!(matches!(qeep_err, QeepErr::Timeout));

        let gqep_back = map_qeep_to_gqep(qeep_err);
        assert!(matches!(gqep_back, GqepError::OperationTimeout { .. }));
    }

    #[tokio::test]
    async fn test_pending_and_completed_count() {
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        assert_eq!(executor.pending_count(), 0);
        assert_eq!(executor.completed_count(), 0);

        let futures = vec![make_success_future("a"), make_success_future("b")];
        let _ = executor.gather(futures).await;

        // 聚集完成后:pending=0, completed=2
        assert_eq!(executor.pending_count(), 0);
        assert_eq!(executor.completed_count(), 2);
    }

    // ============================================================
    // gather_collected<T>:泛型带值聚集(M12 / ADR-185 D3)
    // ============================================================

    /// 创建携带任意值的成功 future(泛型 gather_collected 测试用)
    fn make_value_future<T: Send + 'static>(value: T) -> GqepFuture<T> {
        Box::pin(async move { Ok(value) })
    }

    #[tokio::test]
    async fn test_gather_collected_returns_all_values() {
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let futures: Vec<GqepFuture<u64>> = vec![
            make_value_future(10u64),
            make_value_future(20u64),
            make_value_future(30u64),
        ];
        let outcome = executor.gather_collected(futures).await;

        assert_eq!(outcome.stats.total, 3);
        assert_eq!(outcome.stats.succeeded, 3);
        assert_eq!(outcome.stats.failed, 0);
        assert!(outcome.stats.is_all_success());
        // WHY 排序后比较:FuturesUnordered 为流式完成序,不保证输入序;
        // 值完整性用集合语义断言(顺序由调用方按自身 tag 还原,见 doctor 桥接)
        let mut values = outcome.values.clone();
        values.sort_unstable();
        assert_eq!(values, vec![10u64, 20u64, 30u64]);
    }

    #[tokio::test]
    async fn test_gather_collected_struct_values() {
        // 泛型价值证明:非 String 的 struct 值可完整穿透聚集层
        #[derive(Debug, Clone, PartialEq)]
        struct Probe {
            name: &'static str,
            elapsed_ms: u64,
        }
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let futures: Vec<GqepFuture<Probe>> = vec![
            make_value_future(Probe {
                name: "config_file",
                elapsed_ms: 1,
            }),
            make_value_future(Probe {
                name: "event_bus",
                elapsed_ms: 2,
            }),
        ];
        let outcome = executor.gather_collected(futures).await;

        assert_eq!(outcome.stats.succeeded, 2);
        assert_eq!(outcome.values.len(), 2);
        let mut names: Vec<&str> = outcome.values.iter().map(|p| p.name).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["config_file", "event_bus"]);
    }

    #[tokio::test]
    async fn test_gather_collected_excludes_failed_values() {
        // 失败/超时操作不计入 values,但计入 stats.failed(与 GatherResult 语义一致)
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let futures: Vec<GqepFuture<u64>> = vec![
            make_value_future(1u64),
            Box::pin(async {
                Err(GqepError::OperationFailed {
                    operation_id: String::new(),
                    reason: "boom".to_string(),
                })
            }),
            make_value_future(3u64),
        ];
        let outcome = executor.gather_collected(futures).await;

        assert_eq!(outcome.stats.total, 3);
        assert_eq!(outcome.stats.succeeded, 2);
        assert_eq!(outcome.stats.failed, 1);
        assert_eq!(outcome.stats.errors.len(), 1);
        let mut values = outcome.values.clone();
        values.sort_unstable();
        assert_eq!(values, vec![1u64, 3u64]);
    }

    #[tokio::test]
    async fn test_gather_collected_publishes_completed_event() {
        // 泛型路径与 stats-only 路径同事件契约:GatherCompleted(total, succeeded, failed, latency)
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let executor = GqepExecutor::new(GqepConfig::default(), bus.clone());

        let futures: Vec<GqepFuture<u8>> = vec![make_value_future(7u8)];
        let _ = executor.gather_collected(futures).await;

        let event = rx.recv_timeout(Duration::from_millis(100)).await;
        assert!(
            matches!(
                event,
                Ok(NexusEvent::GatherCompleted {
                    total: 1,
                    succeeded: 1,
                    failed: 0,
                    ..
                })
            ),
            "泛型 gather 也应发布 GatherCompleted,实际: {:?}",
            event
        );
    }

    #[tokio::test]
    async fn test_gather_collected_empty() {
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let outcome: GatherCollected<u64> = executor.gather_collected(vec![]).await;
        assert_eq!(outcome.stats.total, 0);
        assert!(outcome.values.is_empty());
    }

    #[tokio::test]
    async fn test_gather_still_stats_only_after_generic_refactor() {
        // 回归守护:泛型化重构后 gather 签名/语义零变化(返回值不含个体值)
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let futures = vec![
            make_success_future("a"),
            make_success_future("b"),
            make_failure_future("c"),
        ];
        let result = executor.gather(futures).await;
        assert_eq!(result.total, 3);
        assert_eq!(result.succeeded, 2);
        assert_eq!(result.failed, 1);
    }
}
