//! LSCT 任务感知策略装配 — cmt-tiering → lsct-tiering 生产依赖边
//!
//! 对应架构层:L3 Storage(同层互引,§2.2 依赖铁律)
//! 对应任务:ADR-160 冻结孤岛偿还(M10,island-batch-promotion-a 岛 4/4)
//!
//! # 模块职责(WHY 本模块存在)
//! LSCT 订阅闭环的 CMT 消费腿(`CmtCoordinator::spawn_lsct_subscriber`)
//! 自 L3 深度优化起已存在,但生产依赖图上没有任何 `lsct_tiering` 实例,
//! `LsctTierSwitched` 事件零生产者 —— "任务负载 → 策略 → 事件 → 实际迁移"
//! 四段链路缺第一段。本模块把 LSCT 策略层生产地装配进 CMT:
//!
//! ```text
//! QuestCreated 事件 ──► reconcile 种子(注册 CMT 已有能力)
//!                    └► LsctCoordinator::handle_quest_created(真实 lsct 调用)
//!                         └► LsctTierSwitched 事件
//!                              └► spawn_lsct_subscriber(既有)执行真实迁移
//! ```
//!
//! # 依赖方向
//! - `cmt-tiering → lsct-tiering`:L3 → L3 同层互引,合法(§2.2)
//! - 反向事件流走 EventBus(`LsctTierSwitched`),不构成依赖边
//! - WHY 无依赖环:lsct 已自持 `Tier` 类型(M10 解除原 cmt 类型复用),
//!   两类型语义 1:1,由本模块的双向转换函数守护契约
//!
//! # 零行为变化保证
//! 全部为 additive API:既有方法(`insert`/`get`/`spawn_lsct_subscriber` 等)
//! 语义不变;策略腿仅在上层显式调用 `spawn_lsct_policy` 后运行。

use std::sync::Arc;

use event_bus::NexusEvent;
use lsct_tiering::{LsctConfig, LsctCoordinator};
use tracing::{debug, info, warn};

use crate::types::Tier;
use crate::CmtCoordinator;

/// cmt `Tier` → lsct `Tier`(语义 1:1 映射)
///
/// WHY 独立转换函数而非 From trait 散点调用:层级契约是跨 crate 的
/// 隐式接口(事件 payload 字符串的载体),集中一处便于审计与测试覆盖
/// (见 `tests/lsct_policy.rs::test_tier_conversion_roundtrip`)。
pub fn to_lsct_tier(tier: Tier) -> lsct_tiering::Tier {
    match tier {
        Tier::Hot => lsct_tiering::Tier::Hot,
        Tier::Warm => lsct_tiering::Tier::Warm,
        Tier::Cold => lsct_tiering::Tier::Cold,
        Tier::Ice => lsct_tiering::Tier::Ice,
    }
}

/// lsct `Tier` → cmt `Tier`(语义 1:1 映射,`to_lsct_tier` 的逆)
pub fn from_lsct_tier(tier: lsct_tiering::Tier) -> Tier {
    match tier {
        lsct_tiering::Tier::Hot => Tier::Hot,
        lsct_tiering::Tier::Warm => Tier::Warm,
        lsct_tiering::Tier::Cold => Tier::Cold,
        lsct_tiering::Tier::Ice => Tier::Ice,
    }
}

/// LSCT 策略装配句柄 — 持有策略协调器实例与后台决策任务
///
/// 由 [`CmtCoordinator::spawn_lsct_policy`] 返回。调用方应持有该句柄
/// 直至 CMT 生命周期结束;`abort` 仅停止决策腿,已执行的迁移不回滚。
pub struct LsctPolicyHandle {
    /// LSCT 策略协调器(真实生产实例,可通过本句柄观测策略状态)
    coordinator: Arc<LsctCoordinator>,
    /// 决策腿后台任务订阅循环(bus 关闭时自然结束)
    join_handle: tokio::task::JoinHandle<()>,
}

impl LsctPolicyHandle {
    /// 获取 LSCT 策略协调器引用(策略状态观测/测试断言入口)
    pub fn coordinator(&self) -> &Arc<LsctCoordinator> {
        &self.coordinator
    }

    /// 中止决策腿(优雅停机;既有迁移结果不回滚)
    pub fn abort(self) {
        self.join_handle.abort();
    }
}

impl CmtCoordinator {
    /// 装配 LSCT 任务感知策略层(ADR-160 孤岛偿还:lsct-tiering 生产边)
    ///
    /// 创建 `LsctCoordinator` 并接入共享 EventBus,后台任务订阅
    /// `QuestCreated` 事件:每个新 Quest 触发一次"重种子 + 策略 tick",
    /// 生成的 `LsctTierSwitched` 由既有 `spawn_lsct_subscriber` 消费执行
    /// 真实层级迁移。**两方法需同时装配,闭环才成立**:
    ///
    /// ```no_run
    /// # use std::sync::Arc;
    /// # use cmt_tiering::CmtCoordinator;
    /// # async fn run(coord: Arc<CmtCoordinator>) {
    /// let _sub = coord.spawn_lsct_subscriber();  // 执行腿(既有)
    /// let _policy = coord.spawn_lsct_policy(Default::default()); // 决策腿(本方法)
    /// # }
    /// ```
    ///
    /// # 参数
    /// - `config`:LSCT 策略配置(升降温阈值;`LsctConfig::default()` 即可)
    ///
    /// # 返回
    /// [`LsctPolicyHandle`]:持有策略实例与后台任务。调用方可 `abort` 停机。
    ///
    /// # 红线
    /// - **先 subscribe 再 spawn**(W8 教训):broadcast 不缓存历史事件,
    ///   订阅必须在 `tokio::spawn` 之前同步建立,否则事件静默丢失
    /// - **Lagged 告警继续**:慢消费者丢弃部分 Quest 事件不中断策略腿
    /// - **Closed 退出**:总线关闭时任务自然结束
    pub fn spawn_lsct_policy(self: &Arc<Self>, config: LsctConfig) -> LsctPolicyHandle {
        // 与 CMT 共享同一 EventBus:LSCT 发布的 LsctTierSwitched 直接送达
        // CMT 自身订阅者,事件流不走跨 crate 调用(§2.2 跨层通信唯一通道)
        let coordinator = Arc::new(LsctCoordinator::with_event_bus(
            config,
            self.event_bus().clone(),
        ));

        // W8 红线:先 subscribe 再 spawn(见方法级 doc)
        let mut rx = self.event_bus().subscribe();

        let cmt = Arc::clone(self);
        let lsct = Arc::clone(&coordinator);
        let join_handle = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(NexusEvent::QuestCreated { title, .. }) => {
                        // 1) 重种子:把 CMT 已有能力注册进 LSCT(增量,幂等 —
                        //    已注册能力由 lsct 侧 assignments 去重)
                        match cmt.reconcile_lsct_assignments(&lsct).await {
                            Ok(registered) => {
                                debug!(quest_title = title, registered, "LSCT 策略重种子完成")
                            }
                            Err(e) => warn!(
                                error = %e,
                                quest_title = title,
                                "LSCT 重种子失败,仅对已注册能力出策略"
                            ),
                        }
                        // 2) 策略 tick + 批量应用(真实调用 lsct API;
                        //    单能力失败仅 warn 不中断,lsct 侧既有语义)
                        if let Err(e) = lsct.handle_quest_created(&title).await {
                            warn!(error = %e, quest_title = title, "LSCT 策略 tick 失败");
                        } else {
                            info!(quest_title = title, "LSCT 策略 tick 完成");
                        }
                    }
                    Ok(_) => {} // 非 QuestCreated 事件忽略(策略腿单一职责)
                    Err(event_bus::EventBusError::SlowConsumerDropped { lag, .. }) => {
                        warn!(lag, "LSCT 策略腿 Lagged,部分 Quest 事件丢失");
                    }
                    Err(_) => break, // 总线关闭,退出循环
                }
            }
        });

        LsctPolicyHandle {
            coordinator,
            join_handle,
        }
    }

    /// 把 CMT 四层已有能力增量注册进 LSCT 策略层(reconcile)
    ///
    /// 每次 `QuestCreated` 前执行:新插入 CMT 的能力在此进入 LSCT 的
    /// assignment 映射,LSCT tick 才会对其出策略。已注册能力由
    /// `LsctCoordinator::get_tier` 去重(幂等,重复注册为零副作用跳过)。
    ///
    /// # 返回
    /// 本次新注册的能力数。
    async fn reconcile_lsct_assignments(
        &self,
        lsct: &LsctCoordinator,
    ) -> Result<usize, crate::error::CmtError> {
        let mut registered = 0usize;
        // 四层全量扫描:容量有限(Hot 256/Warm 4096/Cold 65536,Ice 归档),
        // 单次 Quest 触发一次全层 list 的成本可接受(O(总条目数) 克隆)
        for tier in [Tier::Hot, Tier::Warm, Tier::Cold, Tier::Ice] {
            for entry in self.list(tier).await? {
                if lsct.get_tier(entry.id.as_str()).is_none() {
                    lsct.register_capability(entry.id.as_str(), to_lsct_tier(tier));
                    registered += 1;
                }
            }
        }
        Ok(registered)
    }
}
