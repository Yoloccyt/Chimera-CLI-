//! 发布面 — 事件的投递路径（ADR-314 W6 / BC-01 按职责拆分）
//!
//! 拆分自原 `bus.rs`（生产面 2060 行 → 五职责子模块）。
//!
//! WHY 独立成模块：发布是总线最热的路径，也是**语义最易漂**的一类
//! ——历史上 9 条近重复发布路径（`publish` / `publish_blocking` /
//! `publish_batch` / `publish_batch_blocking` / `publish_critical` /
//! `publish_critical_blocking` / `publish_claim` / `publish_certify` /
//! `publish_declaim`）并存，投递语义（走 broadcast 还是 Critical mpsc、
//! 是否分片、是否降采样）散布在多个方法里。把它们收进一个模块后，
//! "这条事件最终投到哪儿"变成一个可在一屏内读完的问题。
//!
//! # 发布路径族(ADR-314 W6 / BC-02 收敛后)
//!
//! | 族 | 方法 | 路由 |
//! |---|---|---|
//! | 自动路由 | `publish` / `publish_blocking` / `publish_batch` | 按 `is_critical_mpsc_event` 清单自动决定是否走 mpsc 旁路 |
//! | 显式 Critical | `publish_critical` / `publish_critical_blocking` | **无条件**走 mpsc 旁路(清单外的 Critical 事件用) |
//! | 废弃中 | `publish_batch_blocking` / `publish_claim` / `publish_declaim` | `#[deprecated]`,计划版本窗后物理删除 |
//!
//! `impl EventBus` 的方法可跨文件定义（同一 crate 内），故本模块只承载
//! 方法体，类型定义仍在 `super`（`bus/mod.rs`）——外部路径
//! `event_bus::bus::EventBus` 逐字不变。

use std::time::Instant;

use tracing::warn;

use crate::error::EventBusError;
use crate::shard::{event_lane, Lane};
use crate::types::{EventSeverity, NexusEvent};

use super::{is_critical_mpsc_event, EventBus};

/// 单事件发布的共享实现(**宏形式**)—— `publish` 与 `publish_blocking` 的唯一路径
///
/// # WHY 必须是宏,不能改回函数(实测结论,勿优化)
/// 2026-09-11 基准实测:把本实现提取为**函数**后无法让两条路径同时零成本 ——
/// 同步路径 `publish_blocking` 用普通 `#[inline]` 无效(**+5.1%**, p < 0.05),
/// 换 `#[inline(always)]` 可恢复(p = 0.15,与原始持平);
/// 但**异步路径 `publish` 救不回来**:`#[inline(always)]` 对 async fn 状态机内部的
/// 调用不生效,`publish_batch_vs_serial/serial/5` 退化 **+6.4%**(p = 0.00)。
/// 宏展开是文本替换,两条路径生成的机器码与原始内联实现逐字等价,同时源码保持单一
/// (消除 88 行复制粘贴的**静默分叉**风险:改一处漏一处,两条路径行为不再等价)。
///
/// # 为什么用 `$self` / `$event` 作参数
/// 宏体含 `return Ok(())`(分片命中提前返回),展开后必须落在**调用方函数**内 ——
/// 故不能抽成闭包或函数(会改变 `return` 的目标或引入调用边界)。
/// `$event` 以 `expr` 匹配,由调用方按值传入。
///
/// # 发布序列(顺序敏感,勿随意调整)
/// 吞吐计数 → Critical 车道计数 → CBF 信用扣减(仅「分片启用 + Unordered」)
/// → 分片路由(命中提前返回;片满回退并归还信用)→ 日志埋点
/// → Critical 保底送达(空订阅者)或 mpsc 旁路投递(有订阅者)
/// → 背压水位告警 → broadcast 发送 + lag 检测
///
/// # SubTask 17.2:Critical 事件无订阅者告警(B-a 后语义更新)
/// 当 `subscriber_count == 0` 且事件为 Critical 级时,**投递到保底 sink**
/// (默认 `LogCriticalSink` 结构化 `error!` 日志)并递增
/// `critical_no_subscriber_count`。WHY:CheckpointSaved/ConsensusReached 等
/// 关键事件丢失会导致系统状态不一致(如 Quest 无法恢复),B-a 之前仅 warn
/// 后放弃(事件无落点);现保底 sink 提供不可绕过的最低落点,同时保留计数
/// 供运维观测接线覆盖率。Normal 级静默丢弃,避免日志噪声。
///
/// # §6.2 红线双通道(2026-06-29)
/// Critical 安全/治理告警事件(is_critical_mpsc_event 清单,当前 18 类;第三轮
/// T1.3 纠漂——旧值 14 未计入 v2.3 的 CertifyIssued/FailureRegistered)
/// 额外走 mpsc 旁路通道,确保在 broadcast Lagged 场景下
/// 仍能被 `subscribe_critical_events` 订阅者接收。旁路通道未初始化时
/// (无 Critical 订阅者)投递到保底 sink(B-a,M0;原为 warn 告警后放弃)。
///
/// # P1-T12 分片路由(灰度,默认关闭)
/// 启用分片后,`Unordered` 车道事件入 64 片之一(worker 汇入既有 broadcast,
/// 订阅者 API 零变化);`Critical`/`OrderSensitive` 恒走单流路径
/// (红线:Critical 永不进分片,顺序敏感保序)。
macro_rules! dispatch_one_impl {
    ($self:ident, $event:expr) => {{
        // 遮蔽为可变:P1-T12 分片回退分支需重新赋值事件所有权
        // (Err(ev) => event = ev),保证回退事件继续走既有单流路径,
        // 事件永不因分片而丢失(漏发率恒 0)。
        let mut event = $event;
        // 第三轮 T1.2(F-P2):车道判定单次求值。旧形态在信用分支(原 :421)与
        // 分片路由分支(原 :434)各调一次 `event_lane(&event)` —— 第二次结果
        // 被 `matches!` 即时丢弃,而 OrderSensitive 车道会先分配一个
        // `SessionKey`(session_id/correlation_id/rule_id 的 String clone),
        // 等于热路径每发布白做 0-2 次堆分配。提升为 `lane` 单值后:
        //   - event_lane 是纯函数(只读事件,无副作用),两次求值同值 ⇒ 行为等价;
        //   - `Lane` 为 owned 类型不借事件,`&event` 借位在求值结束即释放(NLL),
        //     :438 try_shard_publish(event) 的所有权移动不受影响;
        //   - severity() 保留原位(语义独立的廉价虚调用,不因本改动一并合并判定)。
        let lane = event_lane(&event);
        // §16.5 吞吐量计数(Phase 10 Wave 6):发布入口递增
        $self.published_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // P1-T12 前哨:Critical 车道计数(无条件统计 —— 分片启用与否均递增,
        // 供 T13 影子双跑观测 Critical 车道流量;Critical 永不进分片)
        if event.severity() == EventSeverity::Critical {
            $self.critical_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        // P1-T11 CBF 信用流接入 —— 方案 A(评审 Issue 2 修复):信用扣减仅在
        // 「分片启用 + Unordered」路径发生(信用流仅作分片背压载体):
        // - 未启用分片:不扣信用(broadcast 路径本有 Lagged 保护,无需信用);
        // - 启用分片 + OrderSensitive/Critical:不扣信用(直投单流无人归还,
        //   扣减即泄漏;Critical 另有豁免红线:Critical 背压 = 死锁源,推演 9);
        // - 启用分片 + Unordered:try_acquire(1) 扣信用(扣减失败静默 ——
        //   池空时事件仍可入片,信用仅作片满重试的裁决信号);
        // - shed 计数统一由 try_shard_publish 回退点完成(单一计数,防双重)。
        let mut credit_deducted = false;
        if event.severity() != EventSeverity::Critical
            && $self.shard_bus.load().is_some()
            // `&lane` 借用匹配:两次 matches! 若按值会第一次就 move 掉 lane
            && matches!(&lane, &Lane::Unordered)
        {
            credit_deducted = $self.credit_flow.try_acquire(1).is_ok();
        }
        // P1-T12 分片路由(灰度开关;F-a 后快速路径 = 一次 ArcSwap load ~5ns,
        // 默认关闭时仅此一次原子读 —— 单一真值源,无双表达分叉风险)
        // - `Unordered` → 入分片(worker 攒批汇入 broadcast,订阅者 API 零变化);
        // - `Critical`/`OrderSensitive` → 走原单流(红线:Critical 永不进分片;
        //   顺序敏感通道保持单流,不分片,E8-4)。
        // WHY log_publish 不覆盖分片路径:分片是发布端并行化灰度增强,其可观测性
        // 由 shadow_stats()/bus_shard_depth 指标承载;log_publish 埋点保留在单流
        // 路径(Critical/OrderSensitive/回退事件),避免双记与热路径额外采样。
        if $self.shard_bus.load().is_some()
            && matches!(&lane, &Lane::Unordered)
        {
            match $self.try_shard_publish(event) {
                Ok(()) => return Ok(()),
                // 片满且信用耗尽回退:重新赋值事件所有权,继续走既有
                // broadcast 单流路径 —— 事件永不因分片而丢失(漏发率恒 0)。
                // 同时归还入口扣的 1 信用(事件不经分片、无 worker 归还,
                // 信用不可滞留 —— 否则「片满回退」成为无人归还的泄漏路径)。
                Err(ev) => {
                    if credit_deducted {
                        $self.credit_flow.release(1);
                    }
                    event = ev;
                }
            }
        }
        // WHY 在方法入口测量 start:log_publish 在 send 之前调用(event 所有权
        // 尚未 move),elapsed 主要覆盖 receiver_count() 调用(接近零),
        // 但保留测量点以便未来将 log/指标采集移到 send 之后时仍准确。
        let start = Instant::now();
        let subscriber_count = $self.sender.receiver_count();

        // 记录发布日志(若已启用日志埋点)
        if let Some(logger) = &$self.logger {
            logger.log_publish(&event, subscriber_count, start.elapsed());
        }

        // SubTask 17.2:Critical 事件无订阅者告警
        // WHY:关键事件(CheckpointSaved/ConsensusReached/SlowConsumerDropped)丢失
        // 会导致系统状态不一致,无订阅者时必须告警;Normal 级静默丢弃避免日志噪声
        if subscriber_count == 0 && event.severity() == EventSeverity::Critical {
            warn!(
                event_type = event.type_name(),
                "Critical 事件无订阅者,事件将被丢弃"
            );
        }

        // §6.2 红线双通道:Critical 安全/治理告警事件(is_critical_mpsc_event 清单)额外走 mpsc 旁路
        // WHY 先 mpsc 后 broadcast:旁路走的是**有界**通道(CRITICAL_CHANNEL_CAPACITY)且用
        // try_send,故本步「不可能阻塞」——这是把它放前面的真正理由:不因 Critical 旁路
        // 的背压拖住主发布路径。语义边界(勿读成「无条件必达」):
        //   - 有空位 ⇒ 必达;满载(Full)⇒ 丢弃本条并递增 critical_dropped_count 使丢弃可见,
        //     Sender 保留(临时满载,下次可能成功);receiver 已 drop(Closed)⇒ 移除该 Sender
        //   - 无任何活跃订阅者 ⇒ 不走「warn + 放弃」,而落 fallback sink 保底(见 B-a 分支)
        // broadcast 仍照发以保证向后兼容(旁路不减 broadcast,两通道是并集不是二选一)。
        if is_critical_mpsc_event(&event) {
            $self.send_critical_mpsc(&event);
        }

        // A3 背压监控:发送前检查 broadcast 缓冲区占用率
        // WHY sender.len() 是 AtomicUsize load,开销极低(~1ns),不影响 publish 吞吐
        // 阈值 3/4:提前告警,留给运维反应缓冲;满容量时才告警已太晚(事件即将被丢弃)
        let queued = $self.sender.len();
        let threshold = $self.capacity * 3 / 4;
        if queued > threshold {
            $self.backpressure_warning_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!(
                queued,
                threshold,
                capacity = $self.capacity,
                "broadcast 通道背压告警:缓冲区占用超过 75%"
            );
        }

        // E-2(ADR-191 D2)压力下降采样:非降采样策略/Critical 恒放行(上方早退),
        // 锁存期内被降采样的事件递增计数后跳过广播(返回 Ok——主动降级非错误,
        // published_total 已在入口计数,降级量由 downsampled_total 单独可见)
        if !$self.ds_admit(event.severity(), queued) {
            $self.ds.downsampled.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok(());
        }

        // broadcast::Sender::send 返回 Ok(receiver_count) 表示有多少接收者收到了消息。
        // 若 receiver_count < subscriber_count(发送前采样),说明有慢消费者 lag
        // (其内部缓冲区已满,send 跳过了它)。Err(SendError) 表示无接收者。
        // WHY 零成本:send 返回值本身是 usize,仅需一次整数比较 + 原子 load,
        // 不影响 publish 热路径性能(~1ns 额外开销)。
        let expected = subscriber_count;
        match $self.sender.send(event) {
            Ok(receivers) if receivers < expected => {
                warn!(
                    expected_subscribers = expected,
                    actual_receivers = receivers,
                    lagged_count = expected - receivers,
                    "broadcast 发送者 lag 检测:部分订阅者缓冲区已满,事件被跳过"
                );
            }
            Ok(_) => {}  // 所有订阅者正常接收
            Err(_) => {} // 无接收者,已在上方 Critical 告警路径处理
        }
        Ok(())
    }};
}

impl EventBus {
    /// 发布事件到所有订阅者
    ///
    /// 若无订阅者,事件被丢弃但不视为错误(返回 Ok(()))。
    /// 慢消费者导致的丢弃由接收端的 `recv()` 以 `SlowConsumerDropped` 错误暴露。
    ///
    /// # Critical 双通道(§6.2 红线)
    /// Critical 安全/治理事件(`is_critical_mpsc_event` 清单内)额外走有界 mpsc 旁路,
    /// broadcast 仍照发以保证向后兼容(两通道是并集不是二选一)。
    ///
    /// # 分片路由
    /// 分片启用且事件属 `Unordered` 车道时入分片(worker 攒批汇入 broadcast);
    /// 片满且无信用时回退 broadcast(事件永不丢失,漏发率恒 0)。
    ///
    /// WHY async 签名:内部纯同步 send,保留 async 以保证 API 稳定性 ——
    /// 未来引入跨进程投递(MCP Mesh)或异步序列化时无需破坏各依赖方的调用签名。
    #[allow(clippy::unused_async)]
    pub async fn publish(&self, event: NexusEvent) -> Result<(), EventBusError> {
        dispatch_one_impl!(self, event)
    }

    /// 同步发布 — 用于不便 await 的场景(如 Drop 实现)
    ///
    /// WHY:某些回调场景无法 await,提供同步版本避免阻塞
    ///
    /// # SubTask 17.2:Critical 事件无订阅者告警
    /// 与 `publish` 保持一致:无订阅者且 Critical 级时记录 `warn` 日志。
    ///
    /// # §6.2 红线双通道(2026-06-29)
    /// 与 `publish` 一致:Critical 安全/治理告警事件(is_critical_mpsc_event 清单)额外走 mpsc 旁路通道。
    pub fn publish_blocking(&self, event: NexusEvent) -> Result<(), EventBusError> {
        dispatch_one_impl!(self, event)
    }

    /// 批量发布多个事件到所有订阅者(摊销固定采样开销)
    ///
    /// WHY(M1-T1.3):`publish` 每次调用重复 `receiver_count()` / `sender.len()`
    /// 背压采样等固定开销;当调用方需连续发布 N 条事件(如议会 N 个 VoteCast)时,
    /// `publish_batch` 将这些采样摊销为一次,减少 N-1 次重复固定开销。
    ///
    /// # 语义一致性(与 `publish` 完全对齐)
    /// 逐条保留:Critical 无订阅者告警、Critical 安全事件 mpsc 旁路双通道(is_critical_mpsc_event 清单)、
    /// broadcast lag 检测。仅 `receiver_count` 与背压采样摊销为一次。
    ///
    /// # 空 Vec 早退
    /// `events` 为空时直接返回 Ok(())(避免无意义采样)。
    ///
    /// WHY async 签名:与 `publish` 一致,内部纯同步 send,保留 async 以稳定 API。
    #[allow(clippy::unused_async)]
    pub async fn publish_batch(&self, events: Vec<NexusEvent>) -> Result<(), EventBusError> {
        self.dispatch_batch(events);
        Ok(())
    }

    /// 同步批量发布 — [`publish_batch`](Self::publish_batch) 的同步版本
    ///
    /// 供不便 await 的场景使用(如 sync 方法内批量发布)。
    ///
    /// # 废弃说明(BC-02,ADR-314 W6)
    ///
    /// 计划退役,改用 [`publish_batch`](Self::publish_batch)(async 语义;
    /// 两者共用同一投递内核 `dispatch_batch`,投递行为逐字等价)。
    /// 退役判据：全仓 grep 调用点归零 + 一个版本窗(≥2 个 CI 周期 + 1 次发布)
    /// 后物理删除。**本次仅打标记,不改行为**(ADR-222 D6 原子纪律:标记与删除
    /// 必须分属不同 commit)。
    #[deprecated(
        since = "2.29.0-omega",
        note = "改用 `publish_batch`(共用同一投递内核,投递行为等价);计划在版本窗后物理删除"
    )]
    pub fn publish_batch_blocking(&self, events: Vec<NexusEvent>) -> Result<(), EventBusError> {
        self.dispatch_batch(events);
        Ok(())
    }

    /// 批量发布内部实现(async/blocking 共用,内部纯同步 send)
    ///
    /// 摊销点:`receiver_count()` 与 `sender.len()` 背压采样各仅一次;
    /// 逐条事件的 send / mpsc 旁路 / Critical 判定为固有开销,无法摊销。
    pub(super) fn dispatch_batch(&self, events: Vec<NexusEvent>) {
        if events.is_empty() {
            return; // 空批次早退,避免无意义采样
        }
        let start = Instant::now();
        // 摊销点 1:receiver_count 仅采样一次(原子 load)
        let subscriber_count = self.sender.receiver_count();
        // 摊销点 2:背压检查仅一次(sender.len 原子 load + 阈值比较)
        let queued = self.sender.len();
        // 降采样闸门是否启用(策略 = DownsampleUnderPressure 时为 Some)。
        // WHY 需要这个开关:降采样策略的语义是「按**当前**压力决定放行」,
        // 故闸门必须拿到**新鲜**的占用水位 —— 而批前一次性采样恒为 0
        // (此时一条都还没发出去)⇒ 闸门永不触发 = 策略被完全绕过。
        // 默认策略(全仓唯一生产配置)下 ds_admit 直接早退,无需逐条重采,
        // 故按此开关只在策略启用时逐条重采,保留默认路径的摊销收益。
        let ds_gate_active = self.ds.params.is_some();
        let threshold = self.capacity * 3 / 4;
        if queued > threshold {
            self.backpressure_warning_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!(
                queued,
                threshold,
                capacity = self.capacity,
                "broadcast 通道背压告警:缓冲区占用超过 75%(批量发布)"
            );
        }
        // 逐条发布:与 publish 语义一致,仅采样已摊销到循环外
        for event in events {
            // 遮蔽为可变:分片回退分支需重新赋值事件所有权(Err(ev) => event = ev),
            // 保证回退事件继续走本循环既有单流逻辑(事件不丢弃,漏发率恒 0)。
            let mut event = event;
            // ADR-314 W6(BC-01 发布路径收敛):批量路径与单发路径对齐**入口计数** ——
            // `publish` 在入口递增 published_total / critical_total,批量路径此前漏计。
            // WHY 必须对齐:① ADR-191 §16.5 把 published_total 定义为「发布入口计数」,
            // 是 L1 吞吐量指标(§8.3 观测面)的唯一输入 —— publish_batch 同样是发布入口,
            // 漏计会让吞吐量**低报**(唯一生产批量发布方是 parliament 的辩论批量投递);
            // ② critical_total 是影子双跑前哨的 Critical 车道计数,漏计会让前哨失真。
            // 位置对齐 publish 的顺序:计数在**信用扣减与分片路由之前**(入片事件同样计数)。
            self.published_total
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if event.severity() == EventSeverity::Critical {
                self.critical_total
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            // 第三轮 T1.2 补丁（F-P2 · 批量路径同病）：车道判定单次求值。
            // 批量循环不走 `dispatch_one_impl` 宏（手写并行副本），旧形态在信用
            // 分支（原 :1014）与分片路由分支（原 :1027）各调一次 `event_lane(&event)`——
            // OrderSensitive 车道会先分配一个 `SessionKey`（session_id/correlation_id/
            // rule_id 的 String clone），等于每次批量发布白做 0-2 次堆分配。提升为
            // `lane` 单值后：event_lane 是纯函数（只读事件），两次求值同值⇒行为等价；
            // `Lane` 为 owned 不借事件，:1028 try_shard_publish(event) 的移动不受影响（NLL）。
            let lane = event_lane(&event);
            // P1-T11 CBF 信用流：逐条判定（与 publish 一致，方案 A：仅「分片启用 +
            // Unordered」扣信用，扣减失败静默；OrderSensitive/Critical/未启用
            // 分片不扣 —— 见 publish 注释；shed 由 try_shard_publish 统一计数）
            let mut credit_deducted = false;
            if event.severity() != EventSeverity::Critical
                && self.shard_bus.load().is_some()
                && matches!(&lane, &Lane::Unordered)
            {
                credit_deducted = self.credit_flow.try_acquire(1).is_ok();
            }
            // P1-T12 分片路由(逐条判定,与 publish 一致):
            // Unordered 事件入分片,worker 汇入 broadcast(订阅者 API 零变化);
            // Critical/OrderSensitive 恒走本循环原单流(红线)
            // WHY 在信用处理之后路由:与 publish 信用账目完全一致 —— 入分片事件
            // 的 1 信用由本循环扣除(条件与下方路由一致:F-a 后由 shard_bus.load
            // 判启用 + Unordered),worker 汇入时 release_many 归还(信用守恒
            // 不变量见 try_shard_publish;若先路由后扣信用,worker 会归还未扣过
            // 的信用,破坏守恒语义;若扣信用条件宽于路由条件,则 OrderSensitive/
            // 未入片事件扣而无人归还)
            if self.shard_bus.load().is_some() && matches!(&lane, &Lane::Unordered) {
                match self.try_shard_publish(event) {
                    Ok(()) => continue,
                    // 回退语义与 publish 一致:重新赋值所有权,继续走既有单流路径
                    // (批量路径同样保证事件不丢弃,漏发率恒 0);同时归还入口扣的
                    // 1 信用(回退路径无 worker 归还,信用不可滞留)。
                    Err(ev) => {
                        if credit_deducted {
                            self.credit_flow.release(1);
                        }
                        event = ev;
                    }
                }
            }
            if let Some(logger) = &self.logger {
                logger.log_publish(&event, subscriber_count, start.elapsed());
            }
            if subscriber_count == 0 && event.severity() == EventSeverity::Critical {
                warn!(
                    event_type = event.type_name(),
                    "Critical 事件无订阅者,事件将被丢弃(批量发布)"
                );
            }
            // §6.2 红线双通道:Critical 安全事件(is_critical_mpsc_event 清单)额外走 mpsc 旁路(逐条判定,不破坏语义)
            if is_critical_mpsc_event(&event) {
                self.send_critical_mpsc(&event);
            }
            // ADR-314 W6(BC-01 发布路径收敛):批量路径补齐**降采样闸门** ——
            // publish 的单发路径经 ds_admit 判定,批量路径此前完全跳过 ⇒
            // 压力区内批量发布会绕过「每 every 条放行 1 条」策略,使两条路径不等价
            // (且降采样计数downsampled_total 漏记,运维看不见降级量)。
            // 位置对齐 publish 的顺序:闸门在 mpsc 旁路之后、broadcast 发送之前。
            //
            // WHY 逐条重采占用:降采样策略按「当前压力」决定放行,批前采样恒为 0
            // ⇒ 闸门永不触发(策略被完全绕过)。默认策略下 ds_gate_active=false,
            // 直接复用批前采样值(零额外开销,保留摊销收益)。
            let queued_for_ds = if ds_gate_active {
                self.sender.len()
            } else {
                queued
            };
            if !self.ds_admit(event.severity(), queued_for_ds) {
                self.ds.downsampled.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                continue;
            }
            match self.sender.send(event) {
                Ok(receivers) if receivers < subscriber_count => {
                    warn!(
                        expected_subscribers = subscriber_count,
                        actual_receivers = receivers,
                        lagged_count = subscriber_count - receivers,
                        "broadcast 发送者 lag 检测:部分订阅者缓冲区已满,事件被跳过(批量发布)"
                    );
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }
    }

    /// 显式发布 Critical 事件到双通道(broadcast + mpsc 旁路)
    ///
    /// 调用方明确知道事件为 Critical 时使用此方法,语义清晰。
    /// 内部行为与 [`publish`](Self::publish) 对 is_critical_mpsc_event 清单事件的处理一致,
    /// 但不依赖 `is_critical_mpsc_event` 判定,直接走 mpsc 旁路(适用于
    /// 调用方自定义的 Critical 事件,如未来扩展的 AsaIntervention Block 级)。
    ///
    /// WHY 提供 explicit API:与 `publish_critical_blocking` 配对,
    /// 供 async 上下文调用方使用(如 spawn_overflow_monitor 中的 async 任务)。
    #[allow(clippy::unused_async)]
    pub async fn publish_critical(&self, event: NexusEvent) -> Result<(), EventBusError> {
        // 先走 mpsc 旁路确保 Critical 订阅者必收,再走 broadcast 保持向后兼容
        self.send_critical_mpsc(&event);
        let _ = self.sender.send(event);
        Ok(())
    }

    /// v2.3 (b.1)②:裁决权分离的语义发布 API —— Claim(宣布)走普通广播。
    ///
    /// WHY 独立 API:为「Certify 白名单治理门」提供可静态检索的调用面 token。
    /// 本方法不做运行时 ACL(agent 越权认证由静态门拦截,与 `publish_critical`
    /// 现状一致 —— 编译期写隔离不可得,见 §(b.1) 修正②)。
    ///
    /// # 废弃说明(BC-02,ADR-314 W6)
    ///
    /// 本方法体逐字为 `self.publish(event).await`,与 [`publish`](Self::publish)
    /// **语义完全等价** ⇒ 计划退役,调用方改用 `publish`。
    /// 退役判据：全仓 grep 调用点归零 + 一个版本窗后物理删除。
    ///
    /// 注:签发面的静态可检索性**不依赖本方法**——`NexusEvent::ClaimIssued {`
    /// 的构造点仍是 `check_event_ownership.py` 的判据(见 `sched_claim.rs`
    /// 的 `publish_claim_issued` 注释),迁移到 `publish` 不丢治理可观测性。
    #[deprecated(
        since = "2.29.0-omega",
        note = "本方法等价于 `publish`(方法体逐字相同);改用 `publish`"
    )]
    pub async fn publish_claim(&self, event: NexusEvent) -> Result<(), EventBusError> {
        self.publish(event).await
    }

    /// v2.3 (b.1)②:验证器签发 Certify —— 走 Critical 双通道(必达,验证器语义独占)。
    ///
    /// # 为什么保留(BC-02 偏离裁决,ADR-314 W6)
    ///
    /// 方案 §5.4 把本方法列入 BC-02 退役清单,但 §5.2.1 同时要求「保留
    /// (`certify_claim_ref` 是公开契约的一部分)」——两处冲突。实测以**保留**为准:
    ///
    /// 1. `scripts/check_certify_whitelist.py` 以 `\.publish_certify\s*\(` 作为
    ///    「Certify 生产签发面」的**静态可检索 token**(该门是裁决权外置的落地形态);
    ///    废弃或改名(迁到 `publish_critical`)都会让该门的真实命中数归零 ⇒
    ///    治理门失去判别力(门仍绿,但已不测任何东西)。
    /// 2. 本仓唯一的生产签发点在 `crates/parliament/src/debate/mod.rs`,
    ///    是白名单验证器 crate 自身,调用面本就受门约束。
    ///
    /// ⇒ 本方法**不废弃**;未来若治理门改为按「`NexusEvent::CertifyIssued {`
    /// 构造点」检索,方可与 `publish_critical` 合并(须同批改门 + 迁调用点)。
    pub async fn publish_certify(&self, event: NexusEvent) -> Result<(), EventBusError> {
        self.publish_critical(event).await
    }

    /// v2.3 (b.1)②:agent 撤回此前 Claim(广播级,审计完整性)。
    ///
    /// # 废弃说明(BC-02,ADR-314 W6)
    ///
    /// 本方法体逐字为 `self.publish(event).await`,与 [`publish`](Self::publish)
    /// **语义完全等价** ⇒ 计划退役,调用方改用 `publish`。
    /// 退役判据：全仓 grep 调用点归零 + 一个版本窗后物理删除。
    #[deprecated(
        since = "2.29.0-omega",
        note = "本方法等价于 `publish`(方法体逐字相同);改用 `publish`"
    )]
    pub async fn publish_declaim(&self, event: NexusEvent) -> Result<(), EventBusError> {
        self.publish(event).await
    }

    /// 显式同步发布 Critical 事件到双通道(broadcast + mpsc 旁路)
    ///
    /// 同步版本,供不便 await 的场景使用(如 sync 方法内调用)。
    /// 内部行为与 [`publish_blocking`](Self::publish_blocking) 对 is_critical_mpsc_event
    /// 清单事件的处理一致,但不依赖 `is_critical_mpsc_event` 判定。
    ///
    /// # 为什么保留(BC-02 偏离裁决,ADR-314 W6)
    ///
    /// 方案 §5.4 把本方法列入 BC-02 退役清单(替代路径写作 `publish_blocking`),
    /// 实测**该替代路径不等价**,故不废弃:
    ///
    /// 1. **无条件 vs 条件旁路**:本方法**无条件**走 mpsc 旁路;
    ///    [`publish_blocking`](Self::publish_blocking) 仅对 `is_critical_mpsc_event`
    ///    清单内事件走旁路(当前 18 类)。清单外的 Critical 事件(生产实例:
    ///    `NexusEvent::AgentContextOverflow`,chimera-mas 的预算越界告警)
    ///    改用 `publish_blocking` 会**丢失 mpsc 旁路投递保证**——
    ///    该保证正是调用点选择本方法的理由(见 `chimera-mas/src/context/budget.rs`)。
    /// 2. **sync/async 对称**:`publish_critical` 是 async 显式 Critical 通道,
    ///    本方法是其 sync 孪生;废弃 sync 侧会让 sync 上下文(无法 await)失去
    ///    显式 Critical 投递能力。
    ///
    /// ⇒ 本方法**不废弃**;它与 `publish_critical` 构成「显式 Critical 路由」族,
    /// 与「自动路由」族(`publish`/`publish_blocking`/`publish_batch`)互补。
    pub fn publish_critical_blocking(&self, event: NexusEvent) -> Result<(), EventBusError> {
        self.send_critical_mpsc(&event);
        let _ = self.sender.send(event);
        Ok(())
    }

    // ============================================================
    // NOTE(B5/ADR-312): publish_membrane / publish_membrane_blocking 方法对已随
    // membrane 子系统退役（零生产调用方,仅 bench 自引用）;恢复路径 = git 历史
    // 恢复 membrane.rs + 本方法对。
    // ============================================================
}
