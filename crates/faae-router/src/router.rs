//! FaaE 语义路由器 — Function-as-Expert 的核心路由实现
//!
//! 对应架构层:L6 Router
//! 对应创新点:FaaE(Function-as-Expert)
//!
//! # 核心职责
//! - **语义路由**:基于 CLV 与专家向量的余弦相似度,从候选工具集精筛 Top-K
//! - **EDSB 均衡**:路由后通过熵均衡器概率性重分配到次优工具
//! - **专家注册/注销**:动态管理工具专家注册表
//! - **事件发布**:路由/注册/注销均发布对应事件到 EventBus
//!
//! # 路由流程
//! 1. 接收 KVBSR 粗筛的候选工具集(Top-3 块的工具并集)
//! 2. 对每个候选工具,计算 CLV 与 expert_vector 的余弦相似度
//! 3. 使用 `select_nth_unstable_by` 部分排序选 Top-K(O(n))
//! 4. 若启用均衡,调用 EDSB 概率性重分配
//! 5. 更新被路由工具的 usage_count 和 last_used_at
//! 6. 发布 ExpertRouted 事件
//!
//! # 架构红线
//! - 所有跨层通信走 EventBus(§2.2 依赖铁律)
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect() 在非测试代码
//! - 所有 async fn 满足 Send 约束
//! - 持锁状态下不可 await,避免死锁

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use event_bus::{EventBus, EventMetadata, NexusEvent};
use tokio::sync::RwLock;
use tracing::warn;

// ws2-c1 红线 #8:公共 Top-K 收敛工具(替代 sort_by 全排)
use nexus_contracts::util::xts_top_k_by;

use crate::config::FaaeConfig;
use crate::edsb::EdsbBalancer;
use crate::error::FaaeError;
use crate::types::{ExpertProfile, ExpertScoring, RoutingResult, ToolId};

// ============================================================================
// 仅测试可见:`route()` 步骤 2 真实产出的快照探针
//
// WHY 需要这个旁路:只断言 `collect_scoring_snapshot` 自身,锁住的是**builder**。
// 有人把拷贝循环重新内联回 `route()`、把 builder 原地留着不删时,那样的测试依旧
// 全绿 —— 与 ADR-197 A6 要防的"锁错对象"是同一类假绿。要让"零拷贝"真正锁在
// **接线**上,只能观测 `route()` 的产物,而不是测试自己调 builder 的产物。
//
// WHY `tokio::task_local!` 而非 `thread_local!`:探针要跟随的是**一次 `route()`
// 调用所在的异步任务**,而非 OS 线程(一次 route 可能跨 worker 迁移)。另外
// clippy 1.97 的 `missing_const_for_thread_local` 对 `thread_local!` 宏展开内部
// 误报 "can be made const"(即使已用 Rust 1.88+ 推荐的 `const {}` 块写法),
// 本仓已实测并以 crate 级 allow 绕过(`crates/chimera-tui/src/i18n/mod.rs:25`)——
// 不为一个测试旁路向整个 crate 借一条永久 lint 豁免。
// ============================================================================
// WHY 存 `Arc<Mutex<..>>` 而非裸 `RefCell`:task_local 的值是**按值移入** future 的,
// 测试要在 scope 之外读回它,必须自带一份可共享的句柄;而句柄要能跨 `.await`
// 安全存在,故用 `Mutex` 而非 `RefCell`(后者非 `Sync`,`Arc<RefCell<_>>` 会被
// clippy `arc_with_non_send_sync` 拒)。
#[cfg(test)]
tokio::task_local! {
    static ROUTE_SNAPSHOT_PROBE: Arc<std::sync::Mutex<Option<Vec<Arc<ExpertScoring>>>>>;
}

/// 收集候选专家的评分快照 —— 借用注册表里已预计算好的不可变三元组
///
/// # WHY 只取 `Arc<ExpertScoring>` 而不现读三个字段
/// 旧实现对每个候选做 `tool_id.clone()` + `expert_vector.clone()`(64 维 `Vec<f32>`
/// 堆分配 + 整段 memcpy),12_000 专家实测 ~12.4ms —— 约为同规模并行评分本体
/// (~2.1ms)的 6 倍,快照而非计算成了瓶颈。改为取注册期就建好的 `Arc` 共享三元组后,
/// 每个候选只剩一次引用计数自增,即**每候选零分配、零深拷贝**(与候选数无关的
/// 两次分配仍在:外层 `Vec::with_capacity(n)` 与 `route()` 里的 `Arc::new`)。
///
/// # 锁纪律（不变）
/// 读锁仅在取 `Arc` 的瞬间持有,随迭代结束立即释放;不跨 await,不进 rayon 闭包。
async fn collect_scoring_snapshot(
    candidate_arcs: &[Arc<RwLock<ExpertProfile>>],
) -> Vec<Arc<ExpertScoring>> {
    let mut snapshot = Vec::with_capacity(candidate_arcs.len());
    for profile_arc in candidate_arcs {
        let profile = profile_arc.read().await;
        // 承重不变式复核(m4):`scoring` 必须**仍与** profile 的活字段同源。若有人
        // 在注册之后改写 `tool_id` / `priority` / `expert_vector`,本函数就会把
        // 陈旧三元组送进评分 —— 不 panic、不报错,只是静默路由到错误的工具。
        //
        // WHY 这里不比 brief 建议的 `Arc::ptr_eq(&scoring, &scoring)`:下一步就是
        // `Arc::clone(&profile.scoring)`,自己跟自己的克隆比地址恒为真,零判别力。
        // 有判别力的是"预计算副本 == 当前活字段",下面三条正是这个等式。
        //
        // WHY 不逐元素比 `expert_vector`:那是每候选 O(64) 的比较,12_000 候选的
        // hot loop 在 debug 构建下不可接受。此处比 O(1)/O(短串) 的 tool_id +
        // priority 与 O(1) 的向量长度,足以覆盖"换成了另一份三元组"这一最可能的
        // 破坏形式;全字段逐元素相等由测试
        // `snapshot_shares_precomputed_scoring` 在测试构建中锁住。release 下
        // `debug_assert!` 整体编译掉,零成本。
        debug_assert_eq!(
            profile.scoring.tool_id, profile.tool_id,
            "ExpertScoring 已与 ExpertProfile.tool_id 失去同源 —— \
             预计算副本陈旧化,评分会路由到错误的工具(见 types.rs 不变式说明)"
        );
        debug_assert_eq!(
            profile.scoring.priority, profile.priority,
            "ExpertScoring 已与 ExpertProfile.priority 失去同源 —— \
             预计算副本陈旧化,评分权重与画像不符"
        );
        debug_assert_eq!(
            profile.scoring.expert_vector.len(),
            profile.expert_vector.len(),
            "ExpertScoring.expert_vector 与 ExpertProfile.expert_vector 维度不符 —— \
             预计算副本陈旧化,评分会在错误的子空间上做余弦"
        );
        snapshot.push(Arc::clone(&profile.scoring));
    }
    // 仅测试构建:把 `route()` 真实产出的快照交给旁路探针观测(见 ROUTE_SNAPSHOT_PROBE)。
    // Arc clone 是每候选 O(1),且只存在于 test profile,不进生产二进制。
    // `try_with` 在无 scope 的调用(如直接单测 builder)下返回 Err;锁中毒同样忽略 ——
    // 探针是纯观测旁路,它的缺席或异常都不得影响生产行为(故此处不 `expect`)。
    #[cfg(test)]
    let _ = ROUTE_SNAPSHOT_PROBE.try_with(|probe| {
        if let Ok(mut slot) = probe.lock() {
            *slot = Some(snapshot.clone());
        }
    });
    snapshot
}

/// FaaE 语义路由器 — 工具即专家的语义化路由调度
///
/// # 线程安全
/// - `expert_registry`: `Arc<RwLock<HashMap<...>>>`,读多写少场景,RwLock 允许并发读
/// - 内层 `Arc<RwLock<ExpertProfile>>`:每个专家画像独立锁,支持并发访问不同专家
/// - `event_bus`: Clone 廉价(基于 Arc)
/// - `edsb`: `Arc<EdsbBalancer>`,无状态均衡器,以 Arc 共享给后台衰减循环
///   (避免 `Arc::new(self.edsb.clone())` 创建独立副本,违反 §4.4 #5 Arc 共享 mutate 红线;
///   虽 EdsbBalancer 当前无 mutate 状态,但 `spawn_decay_loop(self: Arc<Self>)` 的签名
///   表明设计意图就是 Arc 共享,统一以 Arc 持有避免未来引入内部状态时的隐性分裂)
///
/// # 并发设计
/// - `route`:获取 registry 读锁 → clone 候选 Arc → 释放锁 → 锁外计算相似度
/// - `register_expert`/`unregister_expert`:获取 registry 写锁,原子更新
/// - `spawn_decay_loop`:通过 `Arc::clone` 共享 edsb 与 registry,后台异步衰减
///
/// # 示例
/// ```no_run
/// use faae_router::{FaaeRouter, ExpertProfile};
/// use event_bus::EventBus;
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let bus = EventBus::new();
/// let router = FaaeRouter::new(bus);
///
/// let profile = ExpertProfile::new("tool-1", vec![0.5; 64], vec!["code".into()], 0.8);
/// router.register_expert(profile).await;
///
/// let clv = vec![0.5; 64];
/// let candidates = vec!["tool-1".into()];
/// let result = router.route(&clv, &candidates).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct FaaeRouter {
    /// 专家注册表(读多写少,用 RwLock;Arc 用于共享给衰减循环)
    expert_registry: Arc<RwLock<HashMap<ToolId, Arc<RwLock<ExpertProfile>>>>>,
    /// FaaE 配置
    config: FaaeConfig,
    /// 事件总线(发布 ExpertRouted/ExpertRegistered 等事件)
    event_bus: EventBus,
    /// EDSB 熵均衡器(Arc 共享,后台衰减循环通过 `Arc::clone` 持有同一引用)
    edsb: Arc<EdsbBalancer>,
}

impl FaaeRouter {
    /// 创建路由器,使用默认配置
    pub fn new(event_bus: EventBus) -> Self {
        Self::with_config(event_bus, FaaeConfig::default())
    }

    /// 创建路由器,使用自定义配置
    pub fn with_config(event_bus: EventBus, config: FaaeConfig) -> Self {
        // WHY: edsb 用 Arc 包装,确保 spawn_decay_loop 通过 `Arc::clone(&self.edsb)`
        // 共享同一实例,而非 `Arc::new(self.edsb.clone())` 创建独立副本(§4.4 #5 红线)。
        let edsb = Arc::new(EdsbBalancer::new(config.clone(), event_bus.clone()));
        Self {
            expert_registry: Arc::new(RwLock::new(HashMap::new())),
            config,
            event_bus,
            edsb,
        }
    }

    /// 获取配置引用
    pub fn config(&self) -> &FaaeConfig {
        &self.config
    }

    /// 获取事件总线引用
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// 获取 EDSB 均衡器引用
    ///
    /// WHY: 返回 `&EdsbBalancer` 而非 `&Arc<EdsbBalancer>`,保持对外 API 不变。
    /// 内部 `Arc::clone(&self.edsb)` 仍可由 `spawn_decay_loop` 直接调用。
    pub fn edsb(&self) -> &EdsbBalancer {
        &self.edsb
    }

    /// 获取当前注册的专家数量(异步,需读锁)
    pub async fn expert_count(&self) -> usize {
        self.expert_registry.read().await.len()
    }

    /// FaaE 语义路由 — 从候选工具集精筛 Top-K
    ///
    /// # 路由流程
    /// 1. 获取 registry 读锁,收集候选工具的 Arc 引用
    /// 2. 锁外计算 CLV 与各候选 expert_vector 的余弦相似度
    /// 3. 使用 `select_nth_unstable_by` 部分排序选 Top-K
    /// 4. 若启用均衡,调用 EDSB 概率性重分配
    /// 5. 更新被路由工具的 usage_count 和 last_used_at
    /// 6. 发布 ExpertRouted 事件
    ///
    /// # 参数
    /// - `clv`:上下文潜在向量(512 维,内部截取前 64 维与 expert_vector 对齐)
    /// - `candidate_tools`:KVBSR 粗筛的候选工具 ID 列表
    ///
    /// # 错误
    /// - `RoutingFailed`:候选集为空或无已注册的候选工具
    pub async fn route(
        &self,
        clv: &[f32],
        candidate_tools: &[ToolId],
    ) -> Result<RoutingResult, FaaeError> {
        if candidate_tools.is_empty() {
            return Err(FaaeError::RoutingFailed {
                reason: "候选工具集为空".into(),
            });
        }

        // 1. 获取读锁,收集候选工具的 Arc 引用(锁内仅收集引用,减少锁持有时间)
        let candidate_arcs: Vec<Arc<RwLock<ExpertProfile>>> = {
            let registry = self.expert_registry.read().await;
            candidate_tools
                .iter()
                .filter_map(|tid| registry.get(tid).cloned())
                .collect()
        };

        if candidate_arcs.is_empty() {
            return Err(FaaeError::RoutingFailed {
                reason: "候选工具集中无已注册的专家".into(),
            });
        }

        // 2. 锁外计算相似度(ComputeBridge 批量评分,快照分离,见 parallel 模块)
        //
        // P1-T14: 原实现循环内每候选持 tokio 读锁 await + 计算,不满足 rayon
        // 闭包契约（闭包内禁 await/持锁,红线 §7.5.3 纪律⑥）。改造为快照分离:
        //   a) 主线程异步读锁收集评分快照,读锁随收集结束立即释放（无锁跨 await）;
        //   b) 纯计算评分经 ComputeBridge 路由:Inline（n < Generic 阈值,串行,
        //      与注入前行为逐位一致）或 Rayon（spawn_compute_batch 批量并行）。
        //      `FaaeConfig::parallel_expert_scoring` + `CHIMERA_NO_PARALLEL_FAAE`
        //      env 任一关闭 → 强制串行回退。
        // 注:原 scored 第三元素 Arc<RwLock<ExpertProfile>> 为死数据（步骤 5 起
        // 按 final_tool 重新查 registry）,已移除,简化并行化。
        let scoring_snapshot = collect_scoring_snapshot(&candidate_arcs).await;
        // Arc 包一层只为让 rayon 闭包能以 `'static` 捕获容器(O(1),与候选数无关)
        let scoring_snapshot = Arc::new(scoring_snapshot);

        // 快照里存什么、不存什么(易误读处,故写明):
        //   - 存:每个候选的 `Arc<ExpertScoring>` 引用 —— 一次引用计数自增,
        //     **不拷贝 tool_id / expert_vector / priority**。这三项由
        //     `ExpertProfile::new` 预计算一次并以 Arc 共享,构造后不变
        //     (见 types.rs `ExpertScoring` 的不变式说明)。
        //   - 不存:可变的 usage_count / last_used_at,它们本就不参与评分。
        //   - 仍有一处拷贝:CLV 每批拷贝一次(进并行闭包),与候选数无关。
        // 零拷贝语义的回归锁见 `snapshot_shares_precomputed_scoring`
        // （该测试观测的是本函数产出的快照,而非 builder 自身）。
        //
        // WHY 走 `score_experts_scoring_batch_offloaded` 而非直接调
        // `score_experts_scoring_batch`:后者是同步的(内层为 ComputeBridge 的
        // rayon `pool.scope`,不是 Future),在 12000 专家规模下会独占 tokio
        // worker 数毫秒,期间该 worker 无法 poll 其它 task / 驱动 IO ——
        // 这里是请求热路径。offloaded 层在多线程 runtime 上经 `block_in_place`
        // 让出 worker(计算本体仍在 ComputeBridge 专用 rayon 池内,不新建 CPU
        // 预算,§7.5.3 纪律④「桥接唯一」),current_thread 上直调不 panic。
        // 两分支的完整理由见 `parallel::score_experts_batch_offloaded` 文档。
        let scored = crate::parallel::score_experts_scoring_batch_offloaded(
            clv,
            &scoring_snapshot,
            self.config.parallel_expert_scoring,
        )?;

        // 3. 部分排序选 Top-K(O(n),公共 xts_top_k_by,红线 #8 ws2-c1)
        // 平局确定性:分数降序 + 原始下标升序 —— 与"全排后截断"的稳定语义等价
        // (select_nth 分区会扰动相等元素相对顺序,必须在比较器内以原始下标消歧)。
        let k = self.config.top_k.min(scored.len());
        let mut scored_idx: Vec<(usize, Arc<ExpertScoring>, f32)> = scored
            .into_iter()
            .enumerate()
            .map(|(i, (s, score))| (i, s, score))
            .collect();
        xts_top_k_by(&mut scored_idx, k, |a, b| {
            b.2.partial_cmp(&a.2)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });

        // 4. 构建候选列表(Top-K 工具 ID + 分数)
        // WHY 只在此处 clone ToolId:评分结果携带的是 `Arc<ExpertScoring>`,
        // 故 n 次 ToolId(String)堆分配降为 Top-K(k)次 —— 见 parallel 模块文档。
        let candidates: Vec<(ToolId, f32)> = scored_idx[..k]
            .iter()
            .map(|(_, s, score)| (s.tool_id.clone(), *score))
            .collect();

        let routed_tool = candidates[0].0.clone();
        let confidence = candidates[0].1;

        // 5. EDSB 均衡(若启用)
        let final_tool = if self.config.balance_enabled {
            // WHY: 克隆 HashMap 获取快照后立即释放 registry 读锁,再调用 balance,
            // 避免 balance 内部的多次 await(compute_entropy / estimate / publish)
            // 跨 registry 锁,导致 register/unregister 写锁被阻塞(B-Crit-1)。
            // HashMap 内是 Arc<RwLock<ExpertProfile>>,Clone 仅增加引用计数,代价低。
            let registry_snapshot: HashMap<ToolId, Arc<RwLock<ExpertProfile>>> = {
                let registry = self.expert_registry.read().await;
                registry.clone()
            };
            self.edsb
                .balance(&registry_snapshot, &routed_tool, &candidates)
                .await
                .unwrap_or(routed_tool.clone())
        } else {
            routed_tool.clone()
        };

        // 6. 更新被路由工具的 usage_count 和 last_used_at
        // WHY: 消除三重嵌套锁(registry 读锁 → profile 读锁 → last_used_at 写锁),
        // 改为顺序获取快照 + 锁外 await(B-Crit-2 修复):
        //   1. registry 读锁内仅 clone profile_arc,立即释放(消除 registry 锁跨 await)
        //   2. profile 读锁内原子 fetch_add(同步)+ clone last_used_at Arc,立即释放
        //   3. last_used_at 写锁单独获取(锁外 await),更新时间戳
        // last_used_at 是 Arc<RwLock<Instant>>,可 clone 后锁外访问,消除嵌套锁死锁风险。
        let profile_arc_opt: Option<Arc<RwLock<ExpertProfile>>> = {
            let registry = self.expert_registry.read().await;
            registry.get(&final_tool).cloned()
        };
        if let Some(profile_arc) = profile_arc_opt {
            // 原子更新 usage_count(同步)+ clone last_used_at Arc(同步,廉价)
            let last_used_arc = {
                let profile = profile_arc.read().await;
                profile.usage_count.fetch_add(1, Ordering::Relaxed);
                profile.last_used_at.clone()
            }; // profile 读锁在此释放
               // 锁外获取 last_used_at 写锁(不嵌套 profile 读锁,消除 await 跨嵌套锁)
            let mut last_used = last_used_arc.write().await;
            *last_used = Instant::now();
        }

        // 7. 发布 ExpertRouted 事件
        let event = NexusEvent::ExpertRouted {
            metadata: EventMetadata::new("faae-router"),
            routed_tool: final_tool.to_string(),
            confidence,
        };
        if let Err(e) = self.event_bus.publish(event).await {
            warn!(error = %e, "ExpertRouted 事件发布失败(不影响路由结果)");
        }

        Ok(RoutingResult {
            routed_tool: final_tool,
            confidence,
            candidates,
        })
    }

    /// 注册工具专家 — 将工具添加到专家注册表
    ///
    /// # 并发一致性
    /// 获取写锁后原子插入,确保并发注册不会丢失。
    /// 若 tool_id 已存在,覆盖旧画像(更新语义向量/能力标签等)。
    ///
    /// # 参数
    /// - `profile`:专家画像(含 tool_id、expert_vector、capability_tags 等)
    pub async fn register_expert(&self, profile: ExpertProfile) {
        let tool_id = profile.tool_id.clone();

        // 原子插入(写锁内完成)
        {
            let mut registry = self.expert_registry.write().await;
            registry.insert(tool_id.clone(), Arc::new(RwLock::new(profile)));
        }

        // 锁外发布事件(避免持锁期间 await)
        let event = NexusEvent::ExpertRegistered {
            metadata: EventMetadata::new("faae-router"),
            tool_id: tool_id.to_string(),
        };
        if let Err(e) = self.event_bus.publish(event).await {
            warn!(error = %e, "ExpertRegistered 事件发布失败");
        }
    }

    /// 注销工具专家 — 从专家注册表移除指定工具
    ///
    /// # 参数
    /// - `tool_id`:要注销的工具 ID
    ///
    /// # 错误
    /// - `ExpertNotFound`:指定 tool_id 未注册
    pub async fn unregister_expert(&self, tool_id: &ToolId) -> Result<(), FaaeError> {
        // 原子移除(写锁内完成)
        let removed = {
            let mut registry = self.expert_registry.write().await;
            registry.remove(tool_id).is_some()
        };

        if !removed {
            return Err(FaaeError::ExpertNotFound {
                tool_id: tool_id.to_string(),
            });
        }

        // 锁外发布事件
        let event = NexusEvent::ExpertUnregistered {
            metadata: EventMetadata::new("faae-router"),
            tool_id: tool_id.to_string(),
        };
        if let Err(e) = self.event_bus.publish(event).await {
            warn!(error = %e, "ExpertUnregistered 事件发布失败");
        }

        Ok(())
    }

    /// 启动后台衰减循环 — 定期对使用计数应用指数衰减
    ///
    /// 在独立 tokio 任务中运行,每 `config.decay_interval_secs` 秒执行一次 `decay_usage_counts`。
    /// 需要在 tokio 运行时上下文中调用。
    ///
    /// WHY: 使用 `Arc::clone(&self.edsb)` 而非 `Arc::new(self.edsb.clone())`,
    /// 共享同一 `EdsbBalancer` 实例的引用计数,而非创建独立副本(§4.4 #5 红线)。
    /// 即便 EdsbBalancer 当前无 mutate 状态,统一以 Arc 持有可避免未来引入
    /// 内部状态时(如统计计数器、缓存)后台循环看不到 router 状态变更的隐性 bug。
    ///
    /// 返回 `JoinHandle` 供调用方管理任务生命周期(B-Min-3 修复,§4.4 #7:
    /// 关键路径必须管理 JoinHandle)。忽略返回值也是安全的 — 任务独立运行直至进程退出。
    pub fn spawn_decay_loop(&self) -> tokio::task::JoinHandle<()> {
        let edsb = Arc::clone(&self.edsb);
        let registry = self.expert_registry.clone();
        edsb.spawn_decay_loop(registry)
    }

    /// 获取专家注册表的共享引用(用于 EDSB 直接访问)
    ///
    /// WHY:允许外部直接访问注册表进行熵计算或衰减,
    /// 避免重复实现遍历逻辑
    pub fn registry(&self) -> Arc<RwLock<HashMap<ToolId, Arc<RwLock<ExpertProfile>>>>> {
        self.expert_registry.clone()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;

    /// 构造测试用专家画像
    fn make_profile(name: &str, vector: Vec<f32>, priority: f32) -> ExpertProfile {
        ExpertProfile::new(name, vector, vec!["test".into()], priority)
    }

    #[tokio::test]
    async fn test_register_and_unregister() {
        let bus = EventBus::new();
        let router = FaaeRouter::new(bus);

        let profile = make_profile("tool-1", vec![0.5; 64], 0.8);
        router.register_expert(profile).await;
        assert_eq!(router.expert_count().await, 1);

        router
            .unregister_expert(&ToolId::new("tool-1"))
            .await
            .unwrap();
        assert_eq!(router.expert_count().await, 0);
    }

    #[tokio::test]
    async fn test_unregister_not_found() {
        let bus = EventBus::new();
        let router = FaaeRouter::new(bus);

        let result = router.unregister_expert(&ToolId::new("nonexistent")).await;
        assert!(matches!(result, Err(FaaeError::ExpertNotFound { .. })));
    }

    #[tokio::test]
    async fn test_route_empty_candidates() {
        let bus = EventBus::new();
        let router = FaaeRouter::new(bus);

        let clv = vec![0.5; 64];
        let result = router.route(&clv, &[]).await;
        assert!(matches!(result, Err(FaaeError::RoutingFailed { .. })));
    }

    #[tokio::test]
    async fn test_route_no_registered_candidates() {
        let bus = EventBus::new();
        let router = FaaeRouter::new(bus);

        let clv = vec![0.5; 64];
        let candidates = vec![ToolId::new("unregistered")];
        let result = router.route(&clv, &candidates).await;
        assert!(matches!(result, Err(FaaeError::RoutingFailed { .. })));
    }

    #[tokio::test]
    async fn test_route_single_candidate() {
        let bus = EventBus::new();
        let router = FaaeRouter::new(bus);

        let profile = make_profile("tool-1", vec![1.0; 64], 1.0);
        router.register_expert(profile).await;

        let clv = vec![1.0; 64];
        let candidates = vec![ToolId::new("tool-1")];
        let result = router.route(&clv, &candidates).await.unwrap();

        assert_eq!(result.routed_tool.as_str(), "tool-1");
        assert!((result.confidence - 1.0).abs() < 1e-5);
        assert_eq!(result.candidates.len(), 1);
    }

    #[tokio::test]
    async fn test_route_top_k_selection() {
        let bus = EventBus::new();
        let router = FaaeRouter::new(bus);

        // 3 个工具,向量不同,确保 Top-K 排序正确
        let mut v1 = vec![0.0; 64];
        v1[0] = 1.0; // 与 CLV 最相似
        router.register_expert(make_profile("t1", v1, 1.0)).await;

        let mut v2 = vec![0.0; 64];
        v2[1] = 1.0; // 与 CLV 次相似
        router.register_expert(make_profile("t2", v2, 1.0)).await;

        let mut v3 = vec![0.0; 64];
        v3[2] = 1.0; // 与 CLV 最不相似
        router.register_expert(make_profile("t3", v3, 1.0)).await;

        let mut clv = vec![0.0; 64];
        clv[0] = 1.0; // 匹配 t1
        let candidates = vec![ToolId::new("t1"), ToolId::new("t2"), ToolId::new("t3")];
        let result = router.route(&clv, &candidates).await.unwrap();

        // Top-1 应为 t1
        assert_eq!(result.routed_tool.as_str(), "t1");
        // 候选按相似度降序
        assert_eq!(result.candidates[0].0.as_str(), "t1");
        assert_eq!(result.candidates[1].0.as_str(), "t2");
        assert_eq!(result.candidates[2].0.as_str(), "t3");
        // 相似度递减
        assert!(result.candidates[0].1 >= result.candidates[1].1);
        assert!(result.candidates[1].1 >= result.candidates[2].1);
    }

    #[tokio::test]
    async fn test_route_updates_usage_count() {
        let bus = EventBus::new();
        let router = FaaeRouter::new(bus);

        let profile = make_profile("tool-1", vec![1.0; 64], 1.0);
        router.register_expert(profile).await;

        let clv = vec![1.0; 64];
        let candidates = vec![ToolId::new("tool-1")];

        // 路由 3 次
        for _ in 0..3 {
            router.route(&clv, &candidates).await.unwrap();
        }

        // 验证 usage_count 递增
        let registry = router.expert_registry.read().await;
        let profile = registry.get(&ToolId::new("tool-1")).unwrap().read().await;
        assert_eq!(profile.get_usage_count(), 3);
    }

    #[tokio::test]
    async fn test_route_with_priority_weighting() {
        let bus = EventBus::new();
        let router = FaaeRouter::new(bus);

        // 两个向量相同的工具,但优先级不同
        router
            .register_expert(make_profile("low-priority", vec![1.0; 64], 0.5))
            .await;
        router
            .register_expert(make_profile("high-priority", vec![1.0; 64], 1.0))
            .await;

        let clv = vec![1.0; 64];
        let candidates = vec![ToolId::new("low-priority"), ToolId::new("high-priority")];
        let result = router.route(&clv, &candidates).await.unwrap();

        // 高优先级工具应被选中(weighted_score = sim × priority)
        assert_eq!(result.routed_tool.as_str(), "high-priority");
    }

    #[tokio::test]
    async fn test_route_top_k_limit() {
        let bus = EventBus::new();
        let config = FaaeConfig::default().with_top_k(2);
        let router = FaaeRouter::with_config(bus, config);

        // 注册 5 个工具
        for i in 0..5 {
            let mut v = vec![0.0; 64];
            v[i] = 1.0;
            router
                .register_expert(make_profile(&format!("t{i}"), v, 1.0))
                .await;
        }

        let clv = vec![1.0; 64];
        let candidates: Vec<ToolId> = (0..5).map(|i| ToolId::new(format!("t{i}"))).collect();
        let result = router.route(&clv, &candidates).await.unwrap();

        // Top-K = 2,候选列表长度应为 2
        assert_eq!(result.candidates.len(), 2);
    }

    #[tokio::test]
    async fn test_concurrent_register() {
        let bus = EventBus::new();
        let router = Arc::new(FaaeRouter::new(bus));

        let mut handles = Vec::new();
        for i in 0..10 {
            let router_clone = router.clone();
            handles.push(tokio::spawn(async move {
                let profile = make_profile(&format!("tool-{i}"), vec![0.5; 64], 0.8);
                router_clone.register_expert(profile).await;
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        assert_eq!(router.expert_count().await, 10);
    }

    /// SubTask 10.4:验证 spawn_decay_loop 通过共享 Arc 修改原 router 持有的状态。
    ///
    /// WHY 此测试存在:§4.4 反模式 5 红线要求 `spawn_decay_loop` 用 `Arc::clone(&self.edsb)`
    /// 而非 `Arc::new(self.edsb.clone())`,确保后台任务与原 router 共享同一引用。
    /// 由于 `EdsbBalancer` 本身无 mutate 状态,decay_loop 实际修改的是通过共享
    /// `Arc<RwLock<HashMap>>` 传入的 `expert_registry` 中各 profile 的 `usage_count`。
    /// 本测试通过验证 `usage_count` 衰减,间接证明共享 Arc 路径正确工作
    /// (若用独立副本,registry 仍是共享的,但若未来 edsb 引入 mutate 状态就会分裂)。
    #[tokio::test]
    async fn test_decay_loop_modifies_shared_edsb() {
        use std::time::{Duration, Instant};

        // 配置:1s 衰减周期 + τ=1s,让测试在 3s 内完成且衰减效果明显
        // (Δt=10s, τ=1s → 衰减因子 exp(-10) ≈ 4.5e-5 → 100 × 4.5e-5 ≈ 0)
        let config = FaaeConfig::default()
            .with_decay_interval_secs(1)
            .with_decay_tau(1.0);
        let bus = EventBus::new();
        let router = FaaeRouter::with_config(bus, config);

        // 注册一个工具:usage_count=100,last_used_at 设为 10s 前
        let profile = ExpertProfile::with_usage_count("tool-1", vec![1.0; 64], vec![], 1.0, 100);
        {
            let mut last_used = profile.last_used_at.write().await;
            *last_used = Instant::now() - Duration::from_secs(10);
        }
        router.register_expert(profile).await;

        // 启动后台 decay_loop(通过 Arc::clone 共享 edsb 与 registry)
        let handle = router.spawn_decay_loop();

        // 等待 2 个周期(2s),确保 decay_loop 至少执行一次
        tokio::time::sleep(Duration::from_secs(2)).await;

        // 验证 router 持有的 registry 中 usage_count 已衰减
        let registry = router.registry();
        let profile = registry
            .read()
            .await
            .get(&ToolId::new("tool-1"))
            .unwrap()
            .clone();
        let count_after = profile.read().await.get_usage_count();

        // 衰减因子 exp(-10/1) ≈ 4.5e-5,100 × 4.5e-5 ≈ 0.0045,round 后 = 0
        assert!(
            count_after < 100,
            "decay_loop 应已通过共享 Arc 修改 registry,实际 count_after={count_after}"
        );

        // 清理:停止后台任务避免影响其他测试
        handle.abort();
    }

    // ============================================================
    // 步骤 2 runtime flavor 分支 —— 批评分让出 tokio worker
    //
    // WHY 放在 router.rs:被测分支就在本文件 `route()` 内,端到端经 route()
    // 才能触发;直接调 `parallel::score_experts_batch` 不经过该分支,锁不住回归。
    // ============================================================

    /// 大规模专家数 —— ≥ `TaskKind::Generic` 阈值 10_000 → 必走 rayon 批量分支
    const BULK_EXPERTS: usize = 12_000;

    /// 确定性批量专家画像（固定种子 LCG）
    fn make_bulk_profiles(n: usize) -> Vec<ExpertProfile> {
        (0..n)
            .map(|i| {
                let mut state = (i as u64)
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let mut next = move || {
                    state = state
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    (state >> 33) as usize
                };
                ExpertProfile::new(
                    format!("tool-{i}"),
                    (0..64).map(|_| (next() % 1000) as f32 / 1000.0).collect(),
                    vec!["bulk".into()],
                    (next() % 100) as f32 / 100.0 + 0.01,
                )
            })
            .collect()
    }

    /// 构造已装载 `n` 个专家的 router
    ///
    /// WHY 直写 registry 而非 `register_expert`:12_000 次 `ExpertRegistered`
    /// 广播会淹没批评分耗时、污染 T2 的测量窗口;`registry()` 是 crate 公开
    /// API（bench 同一用法），装配结果与 `register_expert` 等价。
    ///
    /// WHY 关 `balance_enabled`:EDSB 重分配是概率性的，会让 `routed_tool` 不可
    /// 复现；本组测试只关心评分分支，`candidates` 不受均衡影响。
    async fn router_with_bulk_experts(n: usize, parallel_scoring: bool) -> FaaeRouter {
        let config = FaaeConfig::default()
            .with_parallel_expert_scoring(parallel_scoring)
            .with_balance_enabled(false);
        let router = FaaeRouter::with_config(EventBus::new(), config);
        let registry = router.registry();
        {
            let mut registry = registry.write().await;
            for profile in make_bulk_profiles(n) {
                registry.insert(profile.tool_id.clone(), Arc::new(RwLock::new(profile)));
            }
        }
        router
    }

    /// 批量候选 ID 列表 —— 与 `make_bulk_profiles` 的命名规则一致
    fn bulk_candidates(n: usize) -> Vec<ToolId> {
        (0..n).map(|i| ToolId::new(format!("tool-{i}"))).collect()
    }

    /// 固定种子 CLV(64 维确定性)
    fn bulk_clv() -> Vec<f32> {
        (0..64).map(|i| ((i * 13) % 1000) as f32 / 1000.0).collect()
    }

    /// 跑一次 `route()`,并取回它步骤 2 **真实构造**的快照
    ///
    /// WHY 必须整体包在 `ROUTE_SNAPSHOT_PROBE.scope` 里:探针按 tokio 任务作用域
    /// 存放(`collect_scoring_snapshot` 用 `try_with` 写**当前任务**的槽),故写入
    /// 方与读出方必须落在同一次 scope 内。探针槽用 `Arc` 是因为 task_local 的值
    /// 须满足 `'static`。
    #[cfg(test)]
    async fn route_capturing_snapshot(
        router: &FaaeRouter,
        clv: &[f32],
        candidates: &[ToolId],
    ) -> (RoutingResult, Vec<Arc<ExpertScoring>>) {
        let probe: Arc<std::sync::Mutex<Option<Vec<Arc<ExpertScoring>>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let routed = ROUTE_SNAPSHOT_PROBE
            .scope(Arc::clone(&probe), router.route(clv, candidates))
            .await
            .expect("批量路由应成功");
        let snapshot = probe
            .lock()
            .expect("探针锁不应中毒")
            .take()
            .expect("route() 步骤 2 必须留下快照探针记录");
        (routed, snapshot)
    }

    /// 零拷贝性质 —— `route()` 的快照只共享预计算副本,不复制评分数据
    ///
    /// # 本测试锁的是**接线**,不是 builder
    /// 被断言的快照取自 `route()` 步骤 2 的**真实产物**(经 `#[cfg(test)]` 旁路探针
    /// 观测),不是测试自己调 `collect_scoring_snapshot` 的结果。只测 builder 的话,
    /// 有人把拷贝循环重新内联进 `route()`、builder 原地留着不删时测试照样全绿 ——
    /// 这正是 ADR-197 A6 那一类"锁错对象"的假绿。
    ///
    /// # 为何 `Arc::ptr_eq` 能证明"共享"而非"复制"
    /// 快照元素若经任何一环深拷贝,必然落在**新的分配**上,与 profile 上的
    /// `scoring` 不同址,`Arc::ptr_eq` 即为 false;只有真的共享同一块
    /// `ExpertScoring` 才会为 true。再取第二次 `route()` 的快照复核跨调用稳定共享。
    ///
    /// # 为何这锁得住回归
    /// 任何人把 `route()` 步骤 2 改回 `.tool_id.clone()` / `.expert_vector.clone()`
    /// 现场构造,本测试立即失败 —— 因为断言对象是 `route()` 的产物,不是 helper 的。
    #[tokio::test]
    async fn snapshot_shares_precomputed_scoring() {
        const N: usize = 512;
        let router = router_with_bulk_experts(N, true).await;
        let clv = bulk_clv();
        let candidates = bulk_candidates(N);

        // 走端到端真实路径:route() 步骤 2 构造的快照会被探针记录
        let (routed, snapshot) = route_capturing_snapshot(&router, &clv, &candidates).await;
        assert_eq!(snapshot.len(), N, "route() 快照长度必须等于候选数");
        assert_eq!(routed.candidates.len(), 8, "Top-K 默认 8");

        // 与 profile 上的预计算副本同源 —— 零深拷贝的硬判据。
        // 锁纪律:读锁内只取 Arc 引用随即释放,锁外再读 profile(同 route() 步骤 1/6)。
        let registry = router.registry();
        let profile_arcs: Vec<Arc<RwLock<ExpertProfile>>> = {
            let guard = registry.read().await;
            snapshot
                .iter()
                .map(|entry| guard.get(&entry.tool_id).cloned())
                .collect::<Option<Vec<_>>>()
                .expect("快照元素必须全部对应注册表里已注册的专家")
        };
        for (i, (entry, profile_arc)) in snapshot.iter().zip(profile_arcs.iter()).enumerate() {
            let profile = profile_arc.read().await;
            assert!(
                Arc::ptr_eq(entry, &profile.scoring),
                "route() 快照[{i}] 必须是 ExpertProfile 预计算的同一份(而非临时新建)"
            );
            assert_eq!(entry.tool_id, profile.tool_id);
            assert_eq!(entry.priority, profile.priority);
            assert_eq!(entry.expert_vector, profile.expert_vector);
        }

        // 跨调用稳定共享:第二次 route() 仍取到同一批分配,不得每次重建
        let (_, second) = route_capturing_snapshot(&router, &clv, &candidates).await;
        assert_eq!(second.len(), N, "二次 route() 快照长度必须等于候选数");
        for (i, (a, b)) in snapshot.iter().zip(second.iter()).enumerate() {
            assert!(
                Arc::ptr_eq(a, b),
                "快照[{i}] 两次 route() 必须共享同一份 ExpertScoring,不得各拷一份"
            );
        }
    }

    /// T1 —— current_thread 上评分结果逐元素不变（分支回归锁）
    ///
    /// `#[tokio::test]` 默认 current_thread:① 若把 flavor 分支退化为裸
    /// `block_in_place`,此处与本模块既有全部路由测试会直接 panic;
    /// ② 本测试锁定「让出逻辑不得改变评分结果」——同一批 12_000 专家在并行
    /// 配置与串行配置两个 router 上,`route()` 的 Top-K 候选必须逐元素一致
    /// (ToolId + f32 全等),`routed_tool` 亦同。
    #[tokio::test]
    async fn t1_current_thread_scores_are_invariant() {
        let parallel_router = router_with_bulk_experts(BULK_EXPERTS, true).await;
        let serial_router = router_with_bulk_experts(BULK_EXPERTS, false).await;
        let clv = bulk_clv();
        let candidates = bulk_candidates(BULK_EXPERTS);

        let parallel = parallel_router
            .route(&clv, &candidates)
            .await
            .expect("并行路由应成功");
        let serial = serial_router
            .route(&clv, &candidates)
            .await
            .expect("串行路由应成功");

        assert_eq!(parallel.candidates.len(), 8, "Top-K 默认 8");
        assert_eq!(parallel.candidates.len(), serial.candidates.len());
        for (i, (p, s)) in parallel
            .candidates
            .iter()
            .zip(serial.candidates.iter())
            .enumerate()
        {
            assert_eq!(p.0, s.0, "Top-K[{i}] ToolId 必须一致");
            assert_eq!(p.1, s.1, "Top-K[{i}] 评分必须与串行逐元素一致");
        }
        assert_eq!(
            parallel.routed_tool, serial.routed_tool,
            "routed_tool 必须一致"
        );
    }

    /// T2 —— multi_thread 上批评分不饿死 worker
    ///
    /// # 为何必须 `tokio::spawn` 出去跑批
    /// `#[tokio::test]` 的测试体经 `Runtime::block_on` 在**调用线程**上驱动,
    /// 并不占用 runtime worker —— 实测在测试体里直接同步调批时,worker 上的
    /// 自旋 ticker 照常以 ~7 万次/秒推进(与是否让出无关),即测试体根本阻塞
    /// 不到任何 worker,本断言天生无效。必须把批 `spawn` 成真正的 runtime 任务,
    /// 由 worker 轮询它,同步批才会真的占住 worker。
    ///
    /// # 为何测 wrapper 而非 `route()`
    /// `route()` 全程还含快照收集(自身让出 1 tick),而被测的批窗口仅 ~2.1ms
    /// —— 信噪比 ~1:1,把窗口包在 `route()` 外测无法区分「worker 被饿死」与
    /// 「快照阶段让出过」。`score_experts_scoring_batch_offloaded` 正是
    /// `route()` 步骤 2 实际调用的实现,故直接测它即等价于测该分支;
    /// T1 另行锁住 `route()` 端到端接线。
    ///
    /// # 注:输入载体已随 `route()` 一并切到预计算路径
    /// 本测试原先测 `ScoreInput` 版 wrapper;`route()` 改用共享的
    /// `ExpertScoring` 后,被测对象必须跟着换,否则测的是一个生产上
    /// 已无人调用的分支。`ScoreInput` 版 wrapper 的 flavor 分支由
    /// `parallel::tests::test_precomputed_path_matches_score_input_path`
    /// 在无 tokio 上下文中复核(直调等价),两条 wrapper 共用同一
    /// `on_multi_thread_runtime` 判据与同一 `block_in_place` 包装。
    ///
    /// # 为何用 yield 自旋 ticker 而非 `tokio::time::interval`
    /// 本机(Windows)系统定时器分辨率 ~15.6ms,`interval(1ms)` 实际每 ~15ms
    /// 才 fire 一次 —— 实测纯 `std::thread::sleep(50ms)` 硬阻塞窗口内 ticker
    /// 也只推进 4 次。**仪表分辨率低于被测的 ~2ms 批窗口,测不出差别。**
    /// 自旋 ticker 不依赖定时器:worker 空闲时高速推进,被同步批占住时完全冻结。
    ///
    /// # `worker_threads = 1` 而非 2
    /// 2 个 worker 时 ticker 落在**另一个**空闲 worker 上,无论批调用是否让出
    /// 都会推进 —— 判别力为零。1 个 worker 时 ticker 与批任务必在同一 worker
    /// 上,只有 `block_in_place` 交接出的替补 worker 能让 ticker 继续跑。
    /// flavor 仍是 `MultiThread`(flavor 由调度器类型决定,与 worker 数无关),
    /// 正是本分支的目标分支。
    ///
    /// # 断言口径(为何不是裸 `delta >= 1`)
    /// 取「批调用前后 tick 计数差」而非最终值:最终值会被批调用之后的
    /// await 淹没,使测试空过。但裸 `delta >= 1` 同样空过 —— 实测「读 `before`
    /// → 批任务真正被 worker 轮询」之间仍有自旋泄漏,不让出时也能拿到 3~6 tick。
    /// 故改为**自校准速率比**:先量 worker 空闲时的自旋速率作基线,再要求批窗口
    /// 速率 ≥ 基线的 10%。实测让出时为基线的 63%~89%,不让出时仅 1.6%~2.8%,
    /// 阈值两侧各有 >8× 余量,且不随核数/主频漂移。
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn t2_multi_thread_batch_does_not_starve_worker() {
        use std::sync::atomic::AtomicU64;

        let clv = bulk_clv();
        // 走 `route()` 步骤 2 实际调用的那条路径（预计算 `ExpertScoring` + 让出包装）,
        // 而非同族的 `ScoreInput` 版本 —— 见本测试文档「为何测这条路径」。
        let inputs = Arc::new(
            make_bulk_profiles(BULK_EXPERTS)
                .into_iter()
                .map(|p| Arc::clone(&p.scoring))
                .collect::<Vec<Arc<crate::types::ExpertScoring>>>(),
        );

        let ticks = Arc::new(AtomicU64::new(0));
        let ticker = {
            let ticks = Arc::clone(&ticks);
            tokio::spawn(async move {
                loop {
                    // 自旋让出:无定时器依赖,worker 被占住时完全冻结
                    tokio::task::yield_now().await;
                    ticks.fetch_add(1, Ordering::Relaxed);
                }
            })
        };

        // 预热自旋 + 量「worker 空闲」基线速率,作为机器无关的判别标尺
        for _ in 0..10_000 {
            tokio::task::yield_now().await;
        }
        assert!(
            ticks.load(Ordering::Relaxed) > 0,
            "ticker 未存活,本测试失去判别力"
        );
        let free_t0 = Instant::now();
        let free_start = ticks.load(Ordering::Relaxed);
        for _ in 0..200_000 {
            tokio::task::yield_now().await;
        }
        let free_rate =
            (ticks.load(Ordering::Relaxed) - free_start) as f64 / free_t0.elapsed().as_secs_f64();

        let before = ticks.load(Ordering::Relaxed);
        // 批必须 spawn 成 runtime 任务,才会真的占住 worker(见文档首节)
        let task = tokio::spawn({
            let clv = clv.clone();
            let inputs = Arc::clone(&inputs);
            async move {
                let t0 = Instant::now();
                let scored =
                    crate::parallel::score_experts_scoring_batch_offloaded(&clv, &inputs, true)
                        .expect("多线程批量评分应成功");
                (t0.elapsed(), scored.len())
            }
        });
        let (window, n) = task.await.expect("批任务应正常完成");
        let after = ticks.load(Ordering::Relaxed);
        ticker.abort();

        let delta = after - before;
        let batch_rate = delta as f64 / window.as_secs_f64();
        println!(
            "[T2] 批调用窗口 = {window:?}, 窗口内 tick delta = {delta}, \
             批窗口速率 = {batch_rate:.0}/s, 空闲基线速率 = {free_rate:.0}/s"
        );
        assert_eq!(n, BULK_EXPERTS, "结果数必须等于输入数");
        // WHY 断言速率占比而非 `delta >= 1`:绝对 tick 数随机器速度漂移。
        // 实测本机让出时批窗口速率 ≈ 空闲基线的 100%(worker 交接给替补),
        // 不让出时仅 ~2%(worker 被同步批占死,自旋几乎冻结)—— 取 10% 阈值,
        // 两侧各有 >8× 余量,且不依赖具体核数/主频。
        assert!(
            batch_rate >= free_rate * 0.1,
            "批调用窗口 {window:?} 内自旋速率 {batch_rate:.0}/s 仅为空闲基线 \
             {free_rate:.0}/s 的 10% 以下 —— tokio worker 被同步批评分饿死 \
             (multi_thread 上应经 block_in_place 让出)"
        );

        // route() 端到端接线:同一 runtime 上仍须走通且结果正确
        let router = router_with_bulk_experts(BULK_EXPERTS, true).await;
        let result = router
            .route(&clv, &bulk_candidates(BULK_EXPERTS))
            .await
            .expect("多线程路由应成功");
        assert_eq!(result.candidates.len(), 8, "Top-K 默认 8");
    }
}
