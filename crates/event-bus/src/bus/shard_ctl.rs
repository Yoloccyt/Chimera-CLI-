//! 分片控制面 — 分片生命周期与路由裁决（ADR-314 W6 / BC-01 按职责拆分）
//!
//! 拆分自原 `bus.rs`（生产面 2060 行 → 五职责子模块）。
//!
//! WHY 独立成模块：分片是**灰度开关 + 有序关闭**的复合语义，历史上与发布面、
//! 订阅面、统计面混在同一文件里，导致「谁负责关闭 worker」「谁负责回退
//! broadcast」两个高风险问题必须跨 2000 行才能读完。本模块把分片的
//! 四个动作收敛到一屏：
//!
//! 1. **开关**：`without_sharding`（宿主屏蔽 leaf opt-in）/ `try_enable_sharding`
//!    （幂等 opt-in + 降级可见）/ `enable_sharding`（显式强制启用）；
//! 2. **关闭**：`shutdown_sharding`（五步关闭协议，见该方法 rustdoc）；
//! 3. **路由裁决**：`try_shard_publish`（片满回退 broadcast，事件永不丢失）；
//! 4. **观测**：`sharding_enabled` / `shadow_stats`（影子双跑前哨五元组）。
//!
//! 红线（拆分不得改变）：分片键派生仍同时提 `type_name` + `fnv1a`；
//! `DEFAULT_SHARD_COUNT == 64`、`SHARD_CAPACITY == 256` 不变；
//! Critical 永不进分片（`LANE_FORBIDDEN_SHARD` 清单为准）。
//!
//! `impl EventBus` 的方法可跨文件定义（同一 crate 内），故本模块只承载
//! 方法体，类型定义仍在 `super`（`bus/mod.rs`）——外部路径
//! `event_bus::bus::EventBus` 逐字不变。

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::error::EventBusError;
use crate::shard::{ShadowStats, ShardedEventBus, SHARD_CAPACITY};
use crate::types::NexusEvent;

use super::{EventBus, ShardWorkerGuard};

impl EventBus {
    // ============================================================
    // P1-T12 分片灰度开关 — 默认不启用,零回归
    // ============================================================

    /// 关闭本总线（及其所有 Clone 副本）的 leaf 分片 opt-in（性能重构 D1）。
    ///
    /// one-shot 命令的组合根在构造 gqep/scc 之前调用，使后续任何
    /// `try_enable_sharding` 早退，避免为 64 个常驻 worker 付费。不影响：
    /// - 显式 `enable_sharding(n)`（测试/宿主强制）；
    /// - `CHIMERA_BUS_SHARDS` 显式运维覆盖（见 `try_enable_sharding`）。
    ///
    /// 返回 `self` 以支持 `let bus = EventBus::new().without_sharding();`。
    #[must_use = "返回应用开关后的总线，丢弃返回值等于没关闭"]
    pub fn without_sharding(self) -> Self {
        // sharding_allowed 为 Arc<AtomicBool>，store 经 &self 即可，无需 `mut self`。
        self.sharding_allowed.store(false, Ordering::Relaxed);
        self
    }

    /// 优雅关闭分片：唤醒全部 worker 排空各自队列，await 其退出，再释放槽位强引用。
    ///
    /// WHY 公开：长驻宿主（TUI/serve）退出前可显式调用，保证已入片但未汇入的
    /// `Unordered` 事件不丢。不调用亦可：[`ShardWorkerGuard`] 会在最后一个
    /// `EventBus` clone drop 时 abort worker（硬退出，进程 teardown 语义）。
    /// 幂等：重复调用因槽位/句柄已被 take 而为 no-op。
    ///
    /// # 关闭协议（P1-1 顺序不可换）
    /// 1. `request_drain()` → worker drain-to-empty；
    /// 2. **先把 `shard_bus` 置 None** → 此后 `try_shard_publish` 直接回退 broadcast，
    ///    不再有「worker 已退出却仍入片」的路径；
    /// 3. await worker 退出；
    /// 4. 等待在飞发布者释放 Arc（`strong_count` 收敛到 1）—— 收敛后无任何
    ///    「已 load_full 但未 push」的在飞事件；
    /// 5. 残留队列兜底补投 broadcast 并归还信用（覆盖 empty 检查与 break 之间的极端窗口）。
    pub async fn shutdown_sharding(&self) {
        // 1) 取快照；未启用/已关闭 → 幂等返回
        let Some(sb) = self.shard_bus.load_full() else {
            return;
        };
        // 2) 置 draining + 唤醒（notify_one 存 permit，避免 worker 尚未 await 时丢失唤醒）
        sb.request_drain();
        // 3) 先切断新入片（顺序关键，见 rustdoc §关闭协议）
        self.shard_bus.store(None);
        // 4) 取走句柄并 await（worker 见 draining 且队列空后自行 break）
        if let Some(guard) = self.shard_guard.get() {
            for handle in guard.take_handles() {
                // 忽略 JoinError：被 abort 或 panic 均视为已回收，不应因此中断关闭流程
                let _ = handle.await;
            }
        }
        // 5) 等待在飞 publisher 完成 try_push（worker 已退出，额外强引用只来自它们）
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(100);
        while Arc::strong_count(&sb) > 1 && std::time::Instant::now() < deadline {
            tokio::task::yield_now().await;
        }
        // 6) 残余兜底：极端竞态窗口（push 落在 worker 最后一次 empty 检查之后）
        let residual = sb.drain_residual(&self.sender);
        if residual > 0 {
            self.credit_flow.release_many(residual);
            tracing::warn!(
                residual,
                "sharding shutdown 兜底补投残余事件（极端竞态窗口）"
            );
        }
    }

    /// 解析 `CHIMERA_BUS_SHARDS` 覆盖值（严格：仅 `1..=256` 的整数）。
    ///
    /// WHY 抽纯函数：可无环境变量竞态地锁定「非法值 = 未设置」语义；
    /// 同时避免只判存在性导致误配置绕过宿主策略。
    ///
    /// WHY `pub(super)`：解析语义由 `bus::tests` 的严格性用例直接断言
    /// （`env_shard_override_parsing_is_strict`），故需对同模块树可见；
    /// 但不外泄到 crate 外（仍是 bus 模块内部实现细节）。
    pub(super) fn resolve_shard_override(raw: Option<&str>) -> Option<usize> {
        raw.and_then(|v| v.parse::<usize>().ok())
            .filter(|n| (1..=256).contains(n))
    }

    /// 启用分片并把结果显式化（ADR-197 A3）
    ///
    /// WHY 统一入口而不是各 crate 各自 `let _ =`：全仓 10 处调用点分布在 9 个 crate，
    /// 静默忽略 Err 会让“本次运行为何没分片”成为不可归因的性能差异（审计 S-4）。
    /// 本方法与 [`enable_sharding`](Self::enable_sharding) 的降级方向完全相同
    /// （Err 不阻断、回落单流），只把结果按类别写进日志并返回是否可用。
    ///
    /// # 幂等与噪声控制
    /// - `ShardingAlreadyEnabled` 属正常幂等（多实例/重复构造）→ `debug!`，返回 `true`；
    /// - `ShardingRequiresRuntime` 属环境缺位 → 首次 `warn!`，后续同类降为 `debug!`
    ///   （避免多实例构造刷屏）；其余错误同理。
    ///
    /// 本方法的核心价值是**副作用（日志可见）**，返回值仅供调用方自行决定是否
    /// 追加记录，故不加 `#[must_use]`（避免调用点重新写回 `let _ =`）。
    pub fn try_enable_sharding(&self, component: &str) -> bool {
        // D1：组合根经 `without_sharding()` 屏蔽 leaf opt-in 时，此处早退（不 spawn
        // worker），除非运维经 `CHIMERA_BUS_SHARDS` 显式覆盖（A/B 与故障排查逃生）。
        // 显式 `enable_sharding(n)` 不受本开关影响（测试/宿主强制仍可用）。
        //
        // P2-5：**先严格解析再判策略**。旧实现只判 `var_os(..).is_none()`，
        // `CHIMERA_BUS_SHARDS=abc` 会绕过宿主策略且 `parse` 失败回退 64 分片，
        // 把 one-shot（Off）命令推到 Auto 档。非法值按「未设置」处理并告警一次。
        let raw_env = std::env::var("CHIMERA_BUS_SHARDS").ok();
        let env_shards = Self::resolve_shard_override(raw_env.as_deref());
        if raw_env.is_some() && env_shards.is_none() {
            static ENV_WARNED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !ENV_WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                tracing::warn!(
                    component,
                    value = %raw_env.as_deref().unwrap_or_default(),
                    "CHIMERA_BUS_SHARDS 非法（需 1..=256 的整数），按未设置处理"
                );
            }
        }
        if !self.sharding_allowed.load(Ordering::Relaxed) && env_shards.is_none() {
            tracing::debug!(
                component,
                "分片被宿主策略关闭(without_sharding)，跳过 opt-in"
            );
            return false;
        }
        // T3.4：默认分片数不变（64）；`CHIMERA_BUS_SHARDS` 仅作为调优开关，
        // 合法范围 1..=256（0 会在 ShardedEventBus::new 内回退 1，但这里显式拒绝，
        // 避免“设 0 却悄悄变单片”的误配置）。
        let shards = env_shards.unwrap_or(crate::shard::DEFAULT_SHARD_COUNT);
        match self.enable_sharding(shards) {
            Ok(()) => {
                tracing::debug!(component, shards, "事件分片已启用");
                true
            }
            Err(EventBusError::ShardingAlreadyEnabled) => {
                // P2-4：诚实返回——重复 enable 且分片仍在 → true；但
                // `shutdown_sharding` 后 `shard_bus=None` 而 guard 已 set（无法重启用），
                // 此时必须返回 false，不得谎报「分片可用」。
                let enabled = self.sharding_enabled();
                tracing::debug!(component, enabled, "事件分片已处于启用态（幂等）");
                enabled
            }
            Err(e) => {
                static WARNED_ONCE: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                let first = WARNED_ONCE
                    .compare_exchange(
                        false,
                        true,
                        std::sync::atomic::Ordering::SeqCst,
                        std::sync::atomic::Ordering::Relaxed,
                    )
                    .is_ok();
                if first {
                    tracing::warn!(
                        component,
                        error = %e,
                        "事件分片未启用，回落单流（行为不变，仅影响吞吐量与尾延）；后续同类降级降为 debug"
                    );
                } else {
                    tracing::debug!(component, error = %e, "事件分片未启用，回落单流");
                }
                false
            }
        }
    }

    /// 启用分片(灰度开关,非 feature flag;手册 §8.5 / v4.0 WI-15)
    ///
    /// 启用后:非 Critical 且非顺序敏感的 `Unordered` 事件按 FNV-1a 哈希
    /// 入 64 片之一,worker 攒批汇入既有 broadcast(订阅者 API 零变化);
    /// `Critical` 与 `OrderSensitive` 事件恒走既有单流(红线)。
    ///
    /// # 默认不启用(零回归)
    /// `EventBus::new()` 行为与 v2.27.1 完全一致(不调用本方法即不分片);
    /// 全量既有测试零回归是本任务最终门禁。
    ///
    /// # 灰度安全
    /// - 无 tokio runtime 上下文(如纯同步测试线程)时返回
    ///   [`ShardingRequiresRuntime`](crate::error::EventBusError::ShardingRequiresRuntime),
    ///   调用方 `let _ =` 忽略即自动降级回单流(分片失败零风险);
    /// - 重复调用返回 [`ShardingAlreadyEnabled`](crate::error::EventBusError::ShardingAlreadyEnabled)
    ///   (幂等保护;多个 crate 各自调用时仅首个生效,其余忽略即可)。
    ///
    /// # 参数
    /// - `n_shards`:分片数(建议 [`crate::shard::DEFAULT_SHARD_COUNT`]=64;0 时回退 1 片,防取模除零)
    ///
    /// # 返回
    /// `Ok(())` 启用成功;`Err(ShardingAlreadyEnabled)` 已启用;
    /// `Err(ShardingRequiresRuntime)` 无 runtime(调用方应忽略降级)。
    pub fn enable_sharding(&self, n_shards: usize) -> Result<(), EventBusError> {
        // 幂等保护(快速路径):已启用则拒绝重复初始化。ArcSwapOption::load 是
        // 无锁读(~5ns),锁内复查兜底任何并发交错,调用方 `let _ =` 忽略 Err
        // 即安全降级
        if self.shard_bus.load().is_some() {
            return Err(EventBusError::ShardingAlreadyEnabled);
        }
        // 灰度安全:worker 是 tokio 异步任务,必须持有 runtime 上下文才能 spawn;
        // 无 runtime(同步线程)时返回 Err,调用方忽略即降级回单流(零风险)
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| EventBusError::ShardingRequiresRuntime)?;
        // enable 串行化门闩(DCL 写端):ArcSwapOption 的 load/store 原子无锁,
        // 但"检查-构造-spawn-注册"四步需要串行化保证至多一组 worker
        // (评审 Issue 1:并发双 enable 不得重复 spawn,空转 worker = 资源泄漏)。
        //
        // WHY 门闩而非 arc-swap compare_and_swap(F-a 实测记录,2026-09-12):
        // 孤立实验证实 compare_and_swap 在比较成功路径上会为 new 值留下一个
        // **永久性额外强引用**(CAS 后计数 = 存储 1 + 额外 1,drop Guard 后不
        // 消失),破坏"分片总线恰被总线与 worker 各持一份"的可观测不变量
        // (test_enable_sharding_concurrent_no_duplicate_spawn 66≠65 红灯)。
        // 门闩方案下 store 只存一份,引用计数确定性恢复,行为与 v2.27.1 完全一致。
        let _gate = match self.enable_lock.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        // 锁内复查(权威判定):并发交错下另一线程可能已完成初始化
        if self.shard_bus.load().is_some() {
            return Err(EventBusError::ShardingAlreadyEnabled);
        }
        // 构造分片总线并 spawn 每片 worker(worker 持有 broadcast sender + 信用流,
        // 汇入后按 ADR-125 批提交归还信用 —— 信用流自动平衡闭环);
        // spawn 后注册(原版顺序):发布路径经 shard_bus.load() 只在注册后可见
        // 分片,不存在"已注册未 spawn"窗口,行为与 v2.27.1 完全一致。
        let sb = Arc::new(ShardedEventBus::new(n_shards, SHARD_CAPACITY));
        // D2:spawn_workers 返回句柄;先把 guard 落到 OnceLock(仅首个 enable 成功),
        // 再注册 shard_bus。set 失败(shutdown 后重复 enable)→ 丢弃刚 spawn 的 worker
        // (guard 随返回值 drop 触发 abort,避免脱离托管的泄漏),拒绝重复启用。
        let handles = sb.spawn_workers(self.sender.clone(), Arc::clone(&self.credit_flow), &handle);
        if self
            .shard_guard
            .set(ShardWorkerGuard::new(handles))
            .is_err()
        {
            return Err(EventBusError::ShardingAlreadyEnabled);
        }
        self.shard_bus.store(Some(sb));
        Ok(())
    }

    /// 查询分片是否已启用(灰度开关状态)
    ///
    /// # 返回
    /// `true` 表示 `enable_sharding` 已成功调用(Unordered 事件走分片扇出)。
    #[must_use = "分片启用状态是灰度观测项,忽略返回值无意义"]
    pub fn sharding_enabled(&self) -> bool {
        self.shard_bus.load().is_some()
    }

    /// 影子双跑前哨统计(P1-T12;T13 漏发率=0 硬门禁的采集输入)
    ///
    /// 漏发率口径:`sharded_total`(入分片数)vs `merged_total`(worker 汇入
    /// broadcast 数),正常场景两者相等 = 漏发率 0。`shed_total` 复用 T11
    /// `credit_shed_total`(分片满 + 无信用回退 broadcast 的次数,事件不丢弃)。
    ///
    /// # 返回
    /// [`ShadowStats`]:published_total / sharded_total / merged_total /
    /// shed_total / critical_total 五元组(Relaxed 观测,单调累计)。
    #[must_use = "前哨统计是 T13 双跑门禁输入,忽略返回值无意义"]
    pub fn shadow_stats(&self) -> ShadowStats {
        // F-a:ArcSwapOption 读端(Guard 借用解引用,无锁;统计是观测面,
        // 不再需要原 Mutex 版的 poison 恢复分支 —— ArcSwap 无锁不可中毒)
        let (sharded_total, merged_total, merge_closed_total) = match self.shard_bus.load().as_ref()
        {
            Some(sb) => (
                sb.sharded_total(),
                sb.merged_total(),
                sb.merge_closed_total(),
            ),
            None => (0, 0, 0),
        };
        ShadowStats {
            published_total: self.published_total.load(Ordering::Relaxed),
            sharded_total,
            merged_total,
            shed_total: self.credit_shed_total.load(Ordering::Relaxed),
            critical_total: self.critical_total.load(Ordering::Relaxed),
            merge_closed_total,
        }
    }

    /// 尝试将 Unordered 事件入分片 — CBF 信用流裁决背压(手册 §8.5)
    ///
    /// # 返回
    /// - `Ok(())`:事件已入分片(worker 将汇入 broadcast,本事件发布完成);
    /// - `Err(event)`:未入片(片满且无信用/重试失败),调用方回退既有
    ///   broadcast 路径直接投递 —— **事件不丢弃,漏发率恒 0**(前哨硬门禁前置)。
    ///
    /// # 信用账目(不变量:事件信用守恒,方案 A)
    /// - 事件的 1 信用由 publish 入口在入片前扣减(仅分片启用 + Unordered),
    ///   入片成功后由 worker 汇入时 `release_many(batch_len)` 归还
    ///   (ADR-125 批提交);
    /// - 片满重试借用的 1 信用**立即归还**(无论重试成败),仅作为裁决信号,
    ///   不改变事件信用账目 —— 避免重试事件被 worker 重复归还造成泄漏;
    /// - 回退 broadcast 时调用方归还入口扣的 1 信用(本方法返回 `Err` 后
    ///   事件不经分片、无 worker 归还,信用不可滞留 —— 见 publish 回退分支)。
    /// - shed 计数(credit_shed_total)仅在本方法回退点累计:片满 + 无信用,
    ///   语义 = 分片路径的 shed(回退 broadcast 次数,事件不丢弃)。
    // WHY allow(result_large_err):Err 携带事件所有权回退 broadcast（避免热路径 clone）,
    // 有意设计而非疏漏（P1-T12 接口契约,shard::try_push 同语义）
    #[allow(clippy::result_large_err)]
    pub(super) fn try_shard_publish(&self, event: NexusEvent) -> Result<(), NexusEvent> {
        // F-a:ArcSwapOption 无锁读(load ~5ns),clone 出 Arc<ShardedEventBus>
        // 后在锁外操作(不跨 await,§4.4 红线 1 合规;ArcSwap 无锁不可中毒,
        // 原 Mutex 版的 poison 恢复分支随之消失)
        let sb = match self.shard_bus.load().as_ref().cloned() {
            Some(sb) => sb,
            None => return Err(event),
        };
        match sb.try_push(event) {
            Ok(()) => Ok(()),
            Err(event) => {
                // 片满 → CBF 信用流裁决:
                // - 有信用:重试入片一次(借用信用立即归还,不改变事件账目);
                // - 无信用:shed 计数(复用 T11 credit_shed_total 口径:
                //   「因背压未走最优通道,回退既有 broadcast」)并回退。
                if self.credit_flow.try_acquire(1).is_ok() {
                    let result = sb.try_push(event);
                    self.credit_flow.release(1);
                    result
                } else {
                    self.credit_shed_total.fetch_add(1, Ordering::Relaxed);
                    Err(event)
                }
            }
        }
    }
}
