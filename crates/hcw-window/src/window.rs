//! HcwWindow 主结构 — 分层上下文窗口的核心实现
//!
//! 对应架构层:L2 Memory
//! 对应创新点:HCW(Hierarchical Context Window,分层上下文窗口)
//!
//! # 核心职责
//! - 管理 4K/32K/128K/1M 四级窗口的上下文条目
//! - 按复杂度自动选择窗口层级(WindowSelector)
//! - 窗口溢出时自动升级 tier(降级链:L0→L1→L2→L3)
//! - 应用 OSA context_mask 稀疏化(仅加载活跃文件上下文)
//! - 发布 ContextWindowSwitched/ContextCompressed 事件
//! - 订阅 OmniSparseMasksComputed 事件(记录 mask_hash 与 sparsity)
//!
//! # V1 违规修正
//! 原架构:OSA(L6)直接 import HCW(L2)→ 向上依赖违规
//! 修正后:OSA 发布 OmniSparseMasksComputed 事件,HCW 订阅消费
//! HCW 不持有 OSA 的引用,仅通过 EventBus 接收掩码信息
//!
//! # 线程安全
//! - `state: Arc<RwLock<HcwState>>`:内部状态受 RwLock 保护,支持并发读写
//! - `event_bus: EventBus`:基于 Arc,Clone 廉价
//! - `config: HcwConfig`:只读配置,无需锁保护
//! - 所有 async fn 满足 Send + 'static 约束,可被 tokio::spawn
//!
//! # 架构红线
//! - 所有跨层通信走 EventBus(§2.2 依赖铁律)
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect()
//! - 持锁状态下不可 await(避免死锁,先 drop guard 再 await)

use std::sync::Arc;

use chrono::Utc;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::compressor::ContextCompressor;
use crate::error::HcwError;
use crate::selector::WindowSelector;
use crate::types::HcwConfig;
use crate::types::{CompressionReport, ContextEntry, HcwState, WindowTier};

/// select_window 锁内决策的结果(锁外据此发布事件)
///
/// WHY:写锁内完成状态更新后,需在锁外发布事件(避免持锁 await 死锁)。
/// 此 enum 封装锁内决策结果,锁外 match 据此发布对应事件。
enum SelectOutcome {
    /// 直接切换(升级或容量足够,无需压缩)
    Switched {
        from: WindowTier,
        to: WindowTier,
        reason: String,
    },
    /// 压缩后切换(降级且需压缩)
    Compressed {
        from: WindowTier,
        to: WindowTier,
        reason: String,
        original_size: usize,
        compressed_size: usize,
    },
    /// 压缩后仍超容量,保持当前 tier(无需发布事件)
    Rejected { current: WindowTier },
}

/// HcwWindow — 分层上下文窗口主结构
///
/// 管理 4K/32K/128K/1M 四级窗口,自动选择层级、压缩与稀疏化。
/// 可跨 async 任务共享(Send + Sync),所有 async fn 满足 Send + 'static 约束。
///
/// # 快速示例
/// ```no_run
/// use hcw_window::{HcwWindow, HcwConfig, ContextEntry, WindowTier};
/// use event_bus::EventBus;
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let bus = EventBus::new();
/// let window = HcwWindow::with_default_config(bus)?;
///
/// let entry = ContextEntry::new("e-1", "file-1", "内容", 100);
/// window.insert(entry).await?;
///
/// let tier = window.select_window(0.6).await?; // 选择 L2 窗口
/// assert_eq!(tier, WindowTier::L2);
/// # Ok(())
/// # }
/// ```
pub struct HcwWindow {
    /// 内部状态(当前层级、条目列表、最近掩码信息),受 RwLock 保护
    state: Arc<RwLock<HcwState>>,
    /// 事件总线(基于 Arc,Clone 廉价)
    event_bus: EventBus,
    /// 只读配置(四级容量、压缩阈值)
    config: HcwConfig,
}

impl HcwWindow {
    /// 创建 HcwWindow,使用指定配置与 EventBus
    ///
    /// 配置在创建时校验,非法配置返回 `HcwError::InvalidConfig`
    pub fn new(config: HcwConfig, event_bus: EventBus) -> Result<Self, HcwError> {
        config.validate()?;
        let initial_tier = WindowTier::L0;
        Ok(Self {
            state: Arc::new(RwLock::new(HcwState::new(initial_tier))),
            event_bus,
            config,
        })
    }

    /// 创建 HcwWindow,使用默认配置与指定 EventBus
    pub fn with_default_config(event_bus: EventBus) -> Result<Self, HcwError> {
        Self::new(HcwConfig::default(), event_bus)
    }

    /// 获取配置引用(只读)
    pub fn config(&self) -> &HcwConfig {
        &self.config
    }

    /// 获取事件总线引用(用于测试与调试)
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// 插入上下文条目
    ///
    /// 流程:
    /// 1. 若存在待应用的 OSA context_mask(由 listener 存入),先自动应用稀疏化
    /// 2. 将条目追加到 entries
    /// 3. 计算总大小,若超过当前 tier 实际加载容量,触发溢出处理
    /// 4. 溢出处理:逐级升级 tier(L0→L1→L2→L3),保留全部条目
    /// 5. 若 L3 仍溢出,压缩到 L3 实际容量(128K)
    /// 6. 升级时发布 ContextWindowSwitched 事件,压缩时发布 ContextCompressed 事件
    ///
    /// WHY:升级时保留全部条目(不压缩),符合"保留全部上下文"要求;
    /// 仅 L3(最高 tier)溢出时才压缩(丢弃低重要性条目)
    pub async fn insert(&self, entry: ContextEntry) -> Result<(), HcwError> {
        // SubTask 17.1:惰性兜底 — 若 listener 尚未消费 pending_context_mask,在此消费
        self.apply_pending_mask_if_any().await?;

        // 1. 追加条目
        // SubTask 19.5:用 push_entry 替代 entries.push,同步维护 entries_index
        {
            let mut state = self.state.write().await;
            state.push_entry(entry);
        }

        // 2. 检查是否溢出,若溢出逐级升级
        self.handle_overflow().await
    }

    /// 按 ID 获取条目(更新访问次数与时间,返回深拷贝)
    ///
    /// 找到后返回条目克隆,并递增 access_count、更新 last_accessed_at(LRU 语义)。
    /// 不存在返回 None。
    ///
    /// WHY(M-02):get 在热路径上 clone ContextEntry(含 content String 深拷贝)。
    /// 保持此签名(API 兼容),但内部 state.get_mut 用 Arc::make_mut(CoW):
    /// 无外部 Arc 引用时零分配,有外部引用时 clone 一份(保证外部 Arc 不被修改)。
    /// 调用方需零拷贝时用 get_arc/get_ref(不递增 access_count)。
    pub async fn get(&self, id: &str) -> Result<Option<ContextEntry>, HcwError> {
        let mut state = self.state.write().await;
        // SubTask 19.5:用 state.get_mut(id) O(1) 索引查找替代 iter().find() O(n) 扫描
        // WHY(M-01/M-02):state.get_mut 内部 Arc::make_mut(CoW),无外部引用时零分配
        if let Some(entry) = state.get_mut(id) {
            entry.increment_access();
            return Ok(Some(entry.clone()));
        }
        Ok(None)
    }

    /// 按 ID 获取条目(返回 `Arc<ContextEntry>`,共享所有权,真零拷贝)
    ///
    /// 与 get() 的区别:
    /// - get() 返回 ContextEntry 克隆(深拷贝 content String,递增 access_count)
    /// - get_arc() 返回 `Arc::clone(&entries[idx])`(O(1) 引用计数,真零拷贝,
    ///   不递增 access_count)
    ///
    /// WHY(M-01 修复):原实现 `Arc::new(entry.clone())` 等价于先深拷贝再包 Arc,
    /// 多消费者场景下 content String 被反复深拷贝。
    /// 修复后用读锁 + get_ref 获取 &Arc,Arc::clone 返回共享 Arc,
    /// 返回的 Arc 与内部存储共享同一引用(Arc::ptr_eq 为 true)。
    ///
    /// 不递增 access_count:保证 Arc::ptr_eq 为 true(递增会触发 Arc::make_mut CoW,
    /// 破坏 Arc 共享)。调用方需 LRU 语义时用 get()。
    pub async fn get_arc(&self, id: &str) -> Result<Option<Arc<ContextEntry>>, HcwError> {
        let state = self.state.read().await;
        // WHY(M-01):读锁 + get_ref,Arc::clone 零拷贝(仅引用计数 +1)
        Ok(state.get_ref(id).map(Arc::clone))
    }

    /// 按 ID 获取条目(返回 `Arc<ContextEntry>`,引用语义,零拷贝)
    ///
    /// WHY(M-02 新增):与 get_arc 行为相同(读锁 + Arc::clone,零拷贝),
    /// 语义强调"引用访问"(不深拷贝 content)。适用于性能敏感场景
    /// (如 PVL 并行验证同一上下文,多消费者共享同一 Arc)。
    ///
    /// 与 get() 的区别:get() 深拷贝 + 递增 access_count(LRU 语义),
    /// get_ref() 零拷贝 + 不递增 access_count(纯读)。
    pub async fn get_ref(&self, id: &str) -> Result<Option<Arc<ContextEntry>>, HcwError> {
        let state = self.state.read().await;
        // WHY(M-02):同 get_arc,读锁 + Arc::clone,零拷贝引用访问
        Ok(state.get_ref(id).map(Arc::clone))
    }

    /// 按 ID 移除条目
    ///
    /// WHY(M-01/M-02):state.remove 返回 `Arc<ContextEntry>`(零拷贝移交所有权),
    /// HcwWindow::remove 保持 API 兼容返回 ContextEntry,用 `(*arc).clone()` 解包。
    /// remove 非热路径(不如 get 频繁),clone 开销可接受。
    /// 若 Arc 为唯一引用(无外部 get_arc/get_ref 持有),可用 Arc::try_unwrap 零拷贝。
    pub async fn remove(&self, id: &str) -> Result<Option<ContextEntry>, HcwError> {
        let mut state = self.state.write().await;
        Ok(state.remove(id).map(|arc| (*arc).clone()))
    }

    /// 按复杂度选择窗口层级(可能触发压缩或层级切换)
    ///
    /// 流程:
    /// 1. WindowSelector 按复杂度选择目标 tier
    /// 2. 若目标 tier == 当前 tier,无操作
    /// 3. 若目标 tier > 当前 tier(升级),直接切换(更大容量)
    /// 4. 若目标 tier < 当前 tier(降级):
    ///    - 总大小 ≤ 目标容量:直接切换
    ///    - 总大小 > 目标容量:压缩到目标容量,压缩成功则切换,失败则保持当前 tier
    /// 5. 切换时发布 ContextWindowSwitched 事件,压缩时发布 ContextCompressed 事件
    ///
    /// 返回最终层级(可能与目标 tier 不同,若降级失败保持当前 tier)
    ///
    /// # 并发安全(SubTask 12.7 修复)
    /// WHY:原实现采用"读锁→释放→锁外 compress→写锁覆盖 entries"模式,
    /// compress 期间其他线程的 insert 会被写锁覆盖,导致数据丢失(P0 竞态)。
    /// 修复方案 A:全程持有写锁,在锁内调用 compress(纯同步函数,不 await,
    /// 不会死锁),事件发布在锁外(避免持锁 await)。compress 期间阻塞所有
    /// 读写,但 compress 是纯 CPU 计算(无 I/O),耗时短,可接受。
    pub async fn select_window(&self, complexity: f32) -> Result<WindowTier, HcwError> {
        // SubTask 17.1:惰性兜底 — 若 listener 尚未消费 pending_context_mask,在此消费
        self.apply_pending_mask_if_any().await?;

        let target_tier = WindowSelector::select(complexity);

        // 全程持有写锁:读取→压缩→更新 原子化,消除竞态窗口
        let outcome = {
            let mut state = self.state.write().await;
            let current_tier = state.current_tier;

            if target_tier == current_tier {
                return Ok(target_tier);
            }

            let total_size = state.total_size();
            let target_capacity = self.config.effective_capacity_for(target_tier);

            if target_tier > current_tier || total_size <= target_capacity {
                // 升级或容量足够:直接切换(不压缩)
                state.current_tier = target_tier;
                SelectOutcome::Switched {
                    from: current_tier,
                    to: target_tier,
                    reason: format!("complexity={complexity}"),
                }
            } else {
                // 降级且需压缩:在写锁内调用 compress(纯同步函数,不 await)
                let target_size =
                    ((target_capacity as f32) * self.config.compression_threshold) as usize;
                let target_size = target_size.max(1);
                // SubTask 19.4:传 &state.entries 借用,消除 state.entries.clone() 全量 clone
                // compress 内部仅 clone 保留的 Top-N 条目(通常 ≤ 100)
                let report = ContextCompressor::compress(
                    &self.config,
                    &state.entries,
                    target_size,
                    None,
                    Utc::now(),
                );

                if report.algorithm == "none" {
                    // 无需压缩(原始大小 ≤ target_size):直接切换,不替换 entries
                    // WHY:compress 在无需压缩时返回空 retained_entries + algorithm == "none",
                    // 调用方据此跳过 state.entries 替换,避免无谓的全量 clone 与赋值
                    state.current_tier = target_tier;
                    SelectOutcome::Switched {
                        from: current_tier,
                        to: target_tier,
                        reason: format!("complexity={complexity} (no compression needed)"),
                    }
                } else if report.compressed_size <= target_capacity
                    && !report.retained_entries.is_empty()
                {
                    // 压缩成功,切换到目标 tier
                    let original_size = report.original_size;
                    let compressed_size = report.compressed_size;
                    state.entries = report.retained_entries;
                    // SubTask 19.5:entries 全量替换后索引失效,重建索引
                    state.rebuild_index();
                    state.current_tier = target_tier;
                    SelectOutcome::Compressed {
                        from: current_tier,
                        to: target_tier,
                        reason: format!("complexity={complexity} (compressed)"),
                        original_size,
                        compressed_size,
                    }
                } else {
                    // 压缩后仍超过目标容量,拒绝降级,保持当前 tier
                    warn!(
                        from = current_tier.as_str(),
                        target = target_tier.as_str(),
                        compressed_size = report.compressed_size,
                        target_capacity,
                        "降级失败:压缩后仍超过目标容量,保持当前层级"
                    );
                    SelectOutcome::Rejected {
                        current: current_tier,
                    }
                }
            }
        };
        // 写锁已释放(state guard 离开作用域)

        // 锁外发布事件(避免持锁 await 导致死锁)
        match outcome {
            SelectOutcome::Switched { from, to, reason } => {
                self.publish_window_switched(from, to, reason).await?;
                Ok(to)
            }
            SelectOutcome::Compressed {
                from,
                to,
                reason,
                original_size,
                compressed_size,
            } => {
                // 先发布压缩事件,再发布切换事件(顺序:压缩完成 → 窗口切换)
                self.publish_compressed(original_size, compressed_size)
                    .await?;
                self.publish_window_switched(from, to, reason).await?;
                Ok(to)
            }
            SelectOutcome::Rejected { current } => Ok(current),
        }
    }

    /// 应用 OSA context_mask 稀疏化(仅保留活跃文件的条目)
    ///
    /// 流程:
    /// 1. 记录原始大小与条目数
    /// 2. retain_by_file_ids 仅保留 file_id 在 active_file_ids 中的条目
    /// 3. 计算压缩报告(原始/压缩后大小、保留/丢弃条目数)
    /// 4. 若有丢弃,发布 ContextCompressed 事件
    ///
    /// WHY:V1 违规修正 — HCW 订阅 OmniSparseMasksComputed 事件后,
    /// 通过此方法应用 context_mask,仅加载活跃文件上下文,其余稀疏化跳过,
    /// 验证 1M 等效不通过暴力加载(架构红线)
    ///
    /// # 参数
    /// - `active_file_ids`:活跃文件 ID 列表(OSA context_mask.active_ids)
    pub async fn apply_sparse_mask(
        &self,
        active_file_ids: Vec<String>,
    ) -> Result<CompressionReport, HcwError> {
        // SubTask 17.1:委托给自由函数,使 listener(spawned task 无 &self)可复用同一逻辑
        apply_sparse_mask_to_state(&self.state, &self.event_bus, active_file_ids).await
    }

    /// 获取当前窗口层级
    pub async fn current_tier(&self) -> WindowTier {
        self.state.read().await.current_tier
    }

    /// 获取当前总 Token 大小
    pub async fn current_size(&self) -> usize {
        self.state.read().await.total_size()
    }

    /// 获取当前条目数
    pub async fn entry_count(&self) -> usize {
        self.state.read().await.entries.len()
    }

    /// 获取最近接收的 OSA 掩码哈希(用于测试与监控)
    pub async fn last_mask_hash(&self) -> Option<String> {
        self.state.read().await.last_mask_hash.clone()
    }

    /// 获取最近接收的 OSA 稀疏度(用于测试与监控)
    pub async fn last_sparsity(&self) -> Option<f32> {
        self.state.read().await.last_sparsity
    }

    /// 消费并应用 pending_context_mask(若存在)
    ///
    /// WHY(SubTask 17.1):insert/select_window 的惰性兜底入口。
    /// 若 listener 已立即应用过则 pending 为 None,此方法无操作;
    /// 若 listener 尚未消费(如事件刚到达),在此消费,确保稀疏化不遗漏。
    async fn apply_pending_mask_if_any(&self) -> Result<(), HcwError> {
        apply_pending_mask(&self.state, &self.event_bus).await
    }

    /// 启动 OmniSparseMasksComputed 事件监听任务
    ///
    /// spawn 一个 tokio task 持续接收 OmniSparseMasksComputed 事件,
    /// 收到后:
    /// 1. 在写锁内更新 last_mask_hash、last_sparsity,并将 context_mask 存入 pending_context_mask
    /// 2. 释放写锁(避免锁重入死锁)
    /// 3. 调用 apply_pending_mask 消费 pending_context_mask 并应用稀疏化
    ///
    /// WHY(SubTask 17.1):闭合 OSA→HCW 事件驱动稀疏化链路。
    /// 原实现仅记录日志,不应用 context_mask,导致稀疏化链路未闭环。
    /// 现改为:listener 收到事件后立即应用稀疏化(释放锁后调用 apply_sparse_mask),
    /// insert/select_window 作为惰性兜底(处理 listener 应用期间新到达的事件)。
    ///
    /// # 锁重入规避
    /// listener 先在写锁内存储 pending_context_mask,释放锁后调用 apply_pending_mask
    /// (apply_pending_mask 内部 take pending → 释放锁 → apply_sparse_mask 获取独立写锁)。
    /// 若在持锁状态下调用 apply_sparse_mask,会导致 RwLock 写锁重入死锁。
    ///
    /// # 返回
    /// JoinHandle,调用方可用于等待或取消监听
    pub fn spawn_mask_listener(&self) -> tokio::task::JoinHandle<()> {
        let state = self.state.clone();
        let event_bus = self.event_bus.clone();
        let mut rx = self.event_bus.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if let NexusEvent::OmniSparseMasksComputed {
                            mask_hash,
                            sparsity,
                            context_mask,
                            ..
                        } = event
                        {
                            // 1. 写锁内:更新掩码信息 + 存储 pending_context_mask
                            {
                                let mut guard = state.write().await;
                                guard.last_mask_hash = Some(mask_hash);
                                guard.last_sparsity = Some(sparsity);
                                // 仅在 context_mask 非空时存储,避免空 Vec 误触发稀疏化
                                if !context_mask.is_empty() {
                                    guard.pending_context_mask = Some(context_mask);
                                }
                                debug!(
                                    "收到 OmniSparseMasksComputed 事件,已存储 pending_context_mask"
                                );
                            }
                            // 2. 锁已释放 — 调用 apply_pending_mask 消费并应用
                            // WHY:必须先释放写锁再调用 apply_sparse_mask(获取独立写锁),
                            // 否则 RwLock 写锁重入导致死锁
                            if let Err(e) = apply_pending_mask(&state, &event_bus).await {
                                warn!(error = %e, "listener 自动应用 pending_context_mask 失败");
                            }
                        }
                    }
                    Err(event_bus::EventBusError::ChannelClosed) => {
                        info!("EventBus 通道已关闭,停止监听 OmniSparseMasksComputed 事件");
                        break;
                    }
                    Err(e) => {
                        warn!(error = %e, "EventBus 接收错误,继续监听");
                        continue;
                    }
                }
            }
        })
    }

    // ============================================================
    // 内部辅助方法
    // ============================================================

    /// 处理窗口溢出:逐级升级 tier,直到容量足够或达 L3
    ///
    /// 策略:
    /// - L0/L1/L2 溢出 → 升级到更高 tier(保留全部条目)
    /// - L3 溢出 → 压缩到 L3 实际加载容量(128K)
    async fn handle_overflow(&self) -> Result<(), HcwError> {
        loop {
            let (current_tier, total_size) = {
                let state = self.state.read().await;
                (state.current_tier, state.total_size())
            };
            let capacity = self.config.effective_capacity_for(current_tier);

            if total_size <= capacity {
                return Ok(());
            }

            // 溢出处理
            if current_tier == WindowTier::L3 {
                // L3 已是最高 tier,压缩到 L3 实际容量
                self.compress_to_capacity(capacity).await?;
                return Ok(());
            }

            // 升级到更高 tier
            let next_tier = current_tier.upgrade().ok_or_else(|| {
                HcwError::WindowOverflow(format!("{current_tier:?} 无法升级(已达最高层级)"))
            })?;

            self.switch_tier(
                current_tier,
                next_tier,
                format!("{current_tier:?} capacity exceeded ({total_size} > {capacity})"),
            )
            .await?;
        }
    }

    /// 切换窗口层级(内部方法,不检查容量)
    ///
    /// 更新 state.current_tier 并发布 ContextWindowSwitched 事件。
    async fn switch_tier(
        &self,
        from: WindowTier,
        to: WindowTier,
        reason: String,
    ) -> Result<(), HcwError> {
        {
            let mut state = self.state.write().await;
            state.current_tier = to;
        }
        self.publish_window_switched(from, to, reason).await
    }

    /// 压缩当前条目到指定容量
    ///
    /// 用于 L3 溢出场景:压缩到 L3 实际加载容量(128K)。
    /// 压缩后若仍超过容量,返回 WindowOverflow 错误。
    ///
    /// # 并发安全(SubTask 12.7 修复)
    /// WHY:原实现采用"读锁→释放→锁外 compress→写锁覆盖 entries"模式,
    /// 与 select_window 同类竞态:compress 期间其他线程的 insert 会被覆盖。
    /// 修复:全程持有写锁,锁内完成 compress + entries 更新,锁外发布事件。
    async fn compress_to_capacity(&self, capacity: usize) -> Result<(), HcwError> {
        let target_size = ((capacity as f32) * self.config.compression_threshold) as usize;
        let target_size = target_size.max(1);

        // 全程持有写锁:compress + entries 更新原子化,消除竞态
        let (original_size, compressed_size) = {
            let mut state = self.state.write().await;
            // SubTask 19.4:传 &state.entries 借用,消除 state.entries.clone() 全量 clone
            let report = ContextCompressor::compress(
                &self.config,
                &state.entries,
                target_size,
                None,
                Utc::now(),
            );

            if report.algorithm == "none" {
                // 无需压缩:entries 不变,直接返回当前大小
                // WHY:compress 在无需压缩时返回空 retained_entries + algorithm == "none",
                // 跳过 state.entries 替换,避免无谓的全量 clone 与赋值
                let size = report.original_size;
                (size, size)
            } else if report.compressed_size > capacity {
                // 压缩后仍超过容量(单条目超过容量),返回错误
                return Err(HcwError::WindowOverflow(format!(
                    "L3 压缩后 {} 仍超过容量 {}",
                    report.compressed_size, capacity
                )));
            } else {
                state.entries = report.retained_entries;
                // SubTask 19.5:entries 全量替换后索引失效,重建索引
                state.rebuild_index();
                (report.original_size, report.compressed_size)
            }
        };
        // 写锁已释放

        // 锁外发布 ContextCompressed 事件
        self.publish_compressed(original_size, compressed_size)
            .await
    }

    /// 发布 ContextWindowSwitched 事件
    async fn publish_window_switched(
        &self,
        from: WindowTier,
        to: WindowTier,
        reason: String,
    ) -> Result<(), HcwError> {
        let event = NexusEvent::ContextWindowSwitched {
            metadata: EventMetadata::new("hcw-window"),
            from_tier: from.as_str().to_string(),
            to_tier: to.as_str().to_string(),
            reason,
        };
        self.event_bus.publish(event).await?;
        debug!(
            from = from.as_str(),
            to = to.as_str(),
            "ContextWindowSwitched 事件已发布"
        );
        Ok(())
    }

    /// 发布 ContextCompressed 事件
    ///
    /// 事件 payload 的 ratio = compressed/original ∈ [0, 1](与 CompressionReport.compression_ratio 方向相反)
    async fn publish_compressed(
        &self,
        original_size: usize,
        compressed_size: usize,
    ) -> Result<(), HcwError> {
        let event_ratio = if original_size == 0 {
            1.0
        } else {
            compressed_size as f32 / original_size as f32
        };
        let event = NexusEvent::ContextCompressed {
            metadata: EventMetadata::new("hcw-window"),
            original_size: original_size as u64,
            compressed_size: compressed_size as u64,
            ratio: event_ratio,
        };
        self.event_bus.publish(event).await?;
        debug!(
            original_size,
            compressed_size, "ContextCompressed 事件已发布"
        );
        Ok(())
    }
}

// ============================================================
// SubTask 17.1:自由函数 — 供 listener(spawned task 无 &self)复用
// ============================================================

/// 应用 OSA context_mask 稀疏化的核心逻辑(自由函数版本)
///
/// WHY(SubTask 17.1):listener 是 spawned task,仅持有 `Arc<RwLock<HcwState>>`
/// 与 `EventBus` 的克隆,无 `&HcwWindow` 引用。将 `apply_sparse_mask` 的核心逻辑
/// 提取为自由函数,使 listener 与 HcwWindow::apply_sparse_mask 方法共用同一实现,
/// 避免代码重复。
///
/// # 参数
/// - `state`:HCW 内部状态(受 RwLock 保护)
/// - `event_bus`:事件总线(用于发布 ContextCompressed 事件)
/// - `active_file_ids`:活跃文件 ID 列表(OSA context_mask)
async fn apply_sparse_mask_to_state(
    state: &Arc<RwLock<HcwState>>,
    event_bus: &EventBus,
    active_file_ids: Vec<String>,
) -> Result<CompressionReport, HcwError> {
    let (original_size, original_count, removed_count, compressed_size, retained_count) = {
        let mut guard = state.write().await;
        let original_size = guard.total_size();
        let original_count = guard.entries.len();
        let removed = guard.retain_by_file_ids(&active_file_ids);
        let compressed_size = guard.total_size();
        let retained_count = guard.entries.len();
        (
            original_size,
            original_count,
            removed,
            compressed_size,
            retained_count,
        )
    };

    let dropped_count = removed_count;
    // 压缩比 = original / compressed(>1.0,越大压缩越多)
    // WHY(SubTask 14.6):compressed_size == 0(全部稀疏化)时返回 f32::MAX,
    // 非 f32::INFINITY(serde_json 序列化 INFINITY 会输出 null,导致反序列化失败)
    let compression_ratio = if compressed_size > 0 {
        original_size as f32 / compressed_size as f32
    } else if original_size == 0 {
        1.0
    } else {
        // 压缩后为 0(全部稀疏化),compression_ratio 设为 f32::MAX 表示完全稀疏
        f32::MAX
    };

    let report = CompressionReport {
        original_size,
        compressed_size,
        compression_ratio,
        original_count,
        retained_count,
        dropped_count,
        retained_entries: Vec::new(), // 不克隆,避免内存开销
        algorithm: "sparse-mask".into(),
    };

    debug!(
        original_size,
        compressed_size, dropped_count, "OSA 掩码稀疏化完成"
    );

    // 若有丢弃,发布 ContextCompressed 事件
    if dropped_count > 0 {
        publish_compressed_event(event_bus, original_size, compressed_size).await?;
    }

    Ok(report)
}

/// 消费 pending_context_mask 并应用稀疏化(若存在)
///
/// WHY(SubTask 17.1):listener 与 insert/select_window 共用的消费入口。
/// 流程:
/// 1. 写锁内 take pending_context_mask(取走,设为 None),释放锁
/// 2. 若 pending 为空或 None,直接返回(无操作)
/// 3. 调用 apply_sparse_mask_to_state 应用稀疏化(获取独立写锁)
///
/// # 锁重入规避
/// take 操作在写锁内完成并释放锁,随后 apply_sparse_mask_to_state 获取独立写锁。
/// 两段写锁不重叠,避免 RwLock 写锁重入死锁。
async fn apply_pending_mask(
    state: &Arc<RwLock<HcwState>>,
    event_bus: &EventBus,
) -> Result<(), HcwError> {
    // 1. 写锁内:take pending_context_mask(取走并设为 None)
    let active_file_ids = {
        let mut guard = state.write().await;
        guard.pending_context_mask.take()
    };
    // 写锁已释放

    // 2. 若无 pending mask,直接返回
    let active_file_ids = match active_file_ids {
        Some(m) if !m.is_empty() => m,
        _ => return Ok(()),
    };

    // 3. 应用稀疏化(获取独立写锁,无锁重入)
    apply_sparse_mask_to_state(state, event_bus, active_file_ids).await?;
    Ok(())
}

/// 发布 ContextCompressed 事件(自由函数版本)
///
/// 事件 payload 的 ratio = compressed/original ∈ [0, 1]
/// (与 CompressionReport.compression_ratio 方向相反)
async fn publish_compressed_event(
    event_bus: &EventBus,
    original_size: usize,
    compressed_size: usize,
) -> Result<(), HcwError> {
    let event_ratio = if original_size == 0 {
        1.0
    } else {
        compressed_size as f32 / original_size as f32
    };
    let event = NexusEvent::ContextCompressed {
        metadata: EventMetadata::new("hcw-window"),
        original_size: original_size as u64,
        compressed_size: compressed_size as u64,
        ratio: event_ratio,
    };
    event_bus.publish(event).await?;
    debug!(
        original_size,
        compressed_size, "ContextCompressed 事件已发布"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_entry(id: &str, token_size: usize) -> ContextEntry {
        ContextEntry::new(
            id,
            format!("file-{id}"),
            format!("content-{id}"),
            token_size,
        )
    }

    #[tokio::test]
    async fn test_new_validates_config() {
        let bus = EventBus::new();
        let invalid_config = HcwConfig::default().with_l0_capacity(0);
        let result = HcwWindow::new(invalid_config, bus);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_with_default_config() {
        let bus = EventBus::new();
        let window = HcwWindow::with_default_config(bus).unwrap();
        assert_eq!(window.current_tier().await, WindowTier::L0);
        assert_eq!(window.current_size().await, 0);
    }

    #[tokio::test]
    async fn test_insert_and_get() {
        let bus = EventBus::new();
        let window = HcwWindow::with_default_config(bus).unwrap();
        window.insert(make_entry("e-1", 100)).await.unwrap();
        let entry = window.get("e-1").await.unwrap().unwrap();
        assert_eq!(entry.id, "e-1");
        assert_eq!(entry.access_count, 1); // get 递增访问次数
    }

    #[tokio::test]
    async fn test_get_nonexistent_returns_none() {
        let bus = EventBus::new();
        let window = HcwWindow::with_default_config(bus).unwrap();
        let result = window.get("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_insert_overflow_upgrades_tier() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let window = HcwWindow::with_default_config(bus).unwrap();

        // 插入超过 L0 容量(4096)的条目,应升级到 L1
        window.insert(make_entry("e-big", 5000)).await.unwrap();

        // 应收到 ContextWindowSwitched 事件(L0 → L1)
        let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
        match event {
            NexusEvent::ContextWindowSwitched {
                from_tier, to_tier, ..
            } => {
                assert_eq!(from_tier, "L0");
                assert_eq!(to_tier, "L1");
            }
            other => panic!("期望 ContextWindowSwitched,收到 {other:?}"),
        }

        assert_eq!(window.current_tier().await, WindowTier::L1);
    }

    #[tokio::test]
    async fn test_insert_overflow_chain_l0_to_l3() {
        let bus = EventBus::new();
        let window = HcwWindow::with_default_config(bus).unwrap();

        // 插入 30 个 5000 token 条目 = 150K,超过 L0(4K)/L1(32K)/L2(128K),应升级到 L3
        // L3 实际容量 = 1M / 8 = 128K,150K > 128K,应压缩到  128K
        // WHY:使用多个小条目而非单个大条目,确保压缩可成功(Top-N 保留)
        for i in 0..30 {
            window
                .insert(make_entry(&format!("e-{i}"), 5000))
                .await
                .unwrap();
        }

        // 最终应在 L3,且压缩后总大小  L3 实际容量(131_072)
        assert_eq!(window.current_tier().await, WindowTier::L3);
        assert!(window.current_size().await <= 131_072);
    }

    #[tokio::test]
    async fn test_select_window_upgrade() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let window = HcwWindow::with_default_config(bus).unwrap();

        // 复杂度 0.6 → L2
        let tier = window.select_window(0.6).await.unwrap();
        assert_eq!(tier, WindowTier::L2);

        // 应收到 ContextWindowSwitched 事件
        let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
        assert!(matches!(event, NexusEvent::ContextWindowSwitched { .. }));
    }

    #[tokio::test]
    async fn test_select_window_downgrade_with_compression() {
        let bus = EventBus::new();
        let window = HcwWindow::with_default_config(bus).unwrap();

        // 先升级到 L3 并插入大量条目
        window.select_window(0.9).await.unwrap(); // L3
        for i in 0..50 {
            window
                .insert(make_entry(&format!("e-{i}"), 1000))
                .await
                .unwrap();
        }
        assert_eq!(window.current_tier().await, WindowTier::L3);
        assert_eq!(window.current_size().await, 50_000);

        // 降级到 L0(复杂度 0.1),应压缩到 L0 容量(4096)
        let tier = window.select_window(0.1).await.unwrap();
        // 50K > L0 容量 4K,压缩后应 ≤ 4K,降级成功
        assert_eq!(tier, WindowTier::L0);
        assert!(window.current_size().await <= 4096);
    }

    #[tokio::test]
    async fn test_apply_sparse_mask() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let window = HcwWindow::with_default_config(bus).unwrap();

        // 插入 3 个条目,分属 3 个文件
        let mut e1 = make_entry("e-1", 100);
        e1.file_id = "file-1".into();
        let mut e2 = make_entry("e-2", 200);
        e2.file_id = "file-2".into();
        let mut e3 = make_entry("e-3", 300);
        e3.file_id = "file-3".into();
        window.insert(e1).await.unwrap();
        window.insert(e2).await.unwrap();
        window.insert(e3).await.unwrap();

        // 应用掩码:仅保留 file-1 和 file-3
        let report = window
            .apply_sparse_mask(vec!["file-1".into(), "file-3".into()])
            .await
            .unwrap();

        assert_eq!(report.original_size, 600);
        assert_eq!(report.compressed_size, 400); // 100 + 300
        assert_eq!(report.dropped_count, 1);
        assert_eq!(report.retained_count, 2);
        assert_eq!(window.entry_count().await, 2);

        // 应收到 ContextCompressed 事件
        let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
        assert!(matches!(event, NexusEvent::ContextCompressed { .. }));
    }

    #[tokio::test]
    async fn test_context_compressed_event_payload() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let window = HcwWindow::with_default_config(bus).unwrap();

        // 插入大量条目触发 L0 → L1 升级(不压缩)
        for i in 0..10 {
            window
                .insert(make_entry(&format!("e-{i}"), 1000))
                .await
                .unwrap();
        }
        // 消耗升级事件
        let _ = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();

        // 降级到 L0 触发压缩
        window.select_window(0.1).await.unwrap();

        // 应收到 ContextCompressed 事件
        let mut found_compressed = false;
        for _ in 0..5 {
            let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
            if let NexusEvent::ContextCompressed {
                original_size,
                compressed_size,
                ratio,
                ..
            } = event
            {
                assert!(original_size > 0);
                assert!(compressed_size <= original_size);
                assert!((0.0..=1.0).contains(&ratio));
                found_compressed = true;
                break;
            }
        }
        assert!(found_compressed, "应收到 ContextCompressed 事件");
    }

    #[tokio::test]
    async fn test_spawn_mask_listener_receives_event() {
        let bus = EventBus::new();
        let window = HcwWindow::with_default_config(bus.clone()).unwrap();

        // 启动监听任务
        let handle = window.spawn_mask_listener();

        // 发布 OmniSparseMasksComputed 事件
        let event = NexusEvent::OmniSparseMasksComputed {
            metadata: EventMetadata::new("osa-coordinator"),
            mask_hash: "abc123".into(),
            sparsity: 0.875,
            context_mask: vec!["file-0".into()],
        };
        bus.publish(event).await.unwrap();

        // 等待监听任务处理
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 验证 state 已更新
        assert_eq!(window.last_mask_hash().await, Some("abc123".into()));
        assert!((window.last_sparsity().await.unwrap() - 0.875).abs() < 1e-6);

        handle.abort();
    }

    #[tokio::test]
    async fn test_remove_entry() {
        let bus = EventBus::new();
        let window = HcwWindow::with_default_config(bus).unwrap();
        window.insert(make_entry("e-1", 100)).await.unwrap();
        let removed = window.remove("e-1").await.unwrap();
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().id, "e-1");
        assert_eq!(window.entry_count().await, 0);
    }
}
