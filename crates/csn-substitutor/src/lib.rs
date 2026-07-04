//! 能力替代网络(CSN)— 能力降级链,在缺失时自动寻找替代实现
//!
//! 对应架构层:L10 Interface
//! 对应创新点:CSN(Capability Substitution Network)
//! 设计来源:MCP Mesh 量子网格的容错降级机制 + ADR-023
//!
//! ## 核心机制
//! - 维护能力语义向量注册表(`SubstitutionCandidateRegistry`),100 能力 × 50 维 in-memory
//! - 能力不可达时,基于余弦相似度寻找 Top-K 替代候选(`select_nth_unstable` O(n))
//! - 多级降级链(`DegradationChain`)支持 ≥ 3 级降级,逐级回退
//! - 通过 EventBus 发布 `CsnSubstitutionTriggered`、订阅 `McpMeshTransactionCompleted`
//!
//! ## 依赖方向
//! L10 → L1 单向依赖:仅依赖 `event-bus` + `nexus-core`(均 L1),
//! 禁止依赖 L2-L9 任何 crate(§2.2 依赖铁律)。
//!
//! ## 快速示例
//! ```no_run
//! use csn_substitutor::{CsnSubstitutor, CsnConfig, CapabilityDescriptor};
//!
//! # async fn run() {
//! let substitutor = CsnSubstitutor::new(CsnConfig::default());
//! let cap = CapabilityDescriptor::new("cap-1", vec![1.0; 50]);
//! substitutor.register_capability(cap).unwrap();
//! let candidates = substitutor.find_substitutes("cap-1", 5);
//! assert!(candidates.len() <= 5);
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod config;
pub mod degradation_chain;
pub mod error;
pub mod similarity;
pub mod substitutor;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use config::CsnConfig;
pub use degradation_chain::DegradationChain;
pub use error::CsnError;
pub use substitutor::{SubstitutionCandidateRegistry, SubstitutionRegistryStats};
pub use types::{CapabilityDescriptor, CapabilityMetadata, SubstitutionCandidate};

use dashmap::DashMap;
use std::sync::Arc;
use tracing::warn;

use event_bus::{EventBus, EventMetadata, NexusEvent};

/// 能力替代网络核心组件 — 能力注册、替代查询与降级链管理
///
/// 持有并发安全的能力注册表(`Arc<SubstitutionCandidateRegistry>`)、
/// 降级链表(`DashMap<String, DegradationChain>`)与可选的 EventBus。
///
/// ## 线程安全
/// - `SubstitutionCandidateRegistry` 基于 `DashMap`(分片锁),`&self` 调用安全
/// - `DegradationChain` 存储在 `DashMap` 中,按 chain_id 分片
/// - `EventBus` 是 `Clone`(Arc 引用计数)
///
/// ## 事件契约
/// - 发布:`CsnSubstitutionTriggered`(替代触发时)
/// - 订阅:`McpMeshTransactionCompleted`(MCP 事务失败时触发降级)
pub struct CsnSubstitutor {
    /// 替代候选注册表(Arc 共享,后台订阅任务可 clone)
    registry: Arc<SubstitutionCandidateRegistry>,
    /// 降级链集合(Arc 共享,后台订阅任务修改同一实例)
    ///
    /// WHY `Arc<DashMap>`:start_degradation_listener 后台任务需推进
    /// 降级链,必须共享同一 DashMap 实例。若用 clone 会创建独立副本,
    /// 后台修改不会反映到原始 substitutor。
    chains: Arc<DashMap<String, DegradationChain>>,
    /// 配置
    config: CsnConfig,
    /// 可选事件总线(替代触发时发布事件)
    event_bus: Option<EventBus>,
}

impl CsnSubstitutor {
    /// 创建替代器(无 EventBus,不发布事件)
    pub fn new(config: CsnConfig) -> Self {
        let registry = Arc::new(SubstitutionCandidateRegistry::new(config.registry_capacity));
        Self {
            registry,
            chains: Arc::new(DashMap::new()),
            config,
            event_bus: None,
        }
    }

    /// 创建替代器并绑定 EventBus
    ///
    /// 绑定后,`trigger_substitution` 成功会发布 `CsnSubstitutionTriggered` 事件,
    /// 调用 `start_degradation_listener` 可订阅 `McpMeshTransactionCompleted`。
    pub fn with_event_bus(config: CsnConfig, bus: EventBus) -> Self {
        let registry = Arc::new(SubstitutionCandidateRegistry::new(config.registry_capacity));
        Self {
            registry,
            chains: Arc::new(DashMap::new()),
            config,
            event_bus: Some(bus),
        }
    }

    /// 获取能力注册表引用
    pub fn registry(&self) -> &SubstitutionCandidateRegistry {
        &self.registry
    }

    /// 获取配置引用
    pub fn config(&self) -> &CsnConfig {
        &self.config
    }

    /// 注册能力描述符 — 若 capability_id 已存在则覆盖
    ///
    /// # 错误
    /// - `InvalidCapability`:语义向量维度与 `vector_dimension` 不匹配
    /// - `RegistryFull`:注册表已满(且 key 不存在)
    pub fn register_capability(&self, cap: CapabilityDescriptor) -> Result<(), CsnError> {
        self.registry.register(cap)
    }

    /// 查找替代候选 — 基于余弦相似度选 Top-K
    ///
    /// 从注册表中查找与 `capability_id` 语义最相似的 K 个候选(排除自身)。
    /// 使用 `select_nth_unstable` 实现 O(n) Top-K 选择(降序)。
    ///
    /// # 返回
    /// 按 `similarity_score` 降序排列的 Top-K 候选列表;若 `capability_id`
    /// 未注册,返回空 Vec。
    pub fn find_substitutes(
        &self,
        capability_id: &str,
        top_k: usize,
    ) -> Vec<SubstitutionCandidate> {
        self.registry.find_substitutes(capability_id, top_k)
    }

    /// 触发能力替代 — 查找最优替代并推进降级链
    ///
    /// # 流程
    /// 1. 查找 Top-1 替代候选
    /// 2. 若存在降级链,推进到下一级;否则创建新降级链
    /// 3. 发布 `CsnSubstitutionTriggered` 事件(若绑定 EventBus)
    ///
    /// # 错误
    /// - `NoSubstituteFound`:无可用替代候选
    pub async fn trigger_substitution(
        &self,
        original_id: &str,
    ) -> Result<SubstitutionCandidate, CsnError> {
        let candidates = self.registry.find_substitutes(original_id, 1);
        let candidate =
            candidates
                .into_iter()
                .next()
                .ok_or_else(|| CsnError::NoSubstituteFound {
                    capability_id: original_id.to_string(),
                })?;

        // 推进降级链:若已存在则以 original_id 为 chain_id 推进,否则创建新链
        let level = self.advance_or_create_chain(original_id)?;

        // 发布事件(best-effort,失败仅记录日志)
        self.publish_substitution(original_id, &candidate, level)
            .await;

        Ok(candidate)
    }

    /// 显式推进降级链到下一级
    ///
    /// # 错误
    /// - `ChainNotFound`:指定 chain_id 的降级链不存在
    /// - `ChainExhausted`:已到达降级链末端,无法继续
    pub fn advance_degradation(&self, chain_id: &str) -> Result<(), CsnError> {
        let mut chain = self
            .chains
            .get_mut(chain_id)
            .ok_or_else(|| CsnError::ChainNotFound {
                chain_id: chain_id.to_string(),
            })?;
        chain.next_level()
    }

    /// 获取降级链当前层级(若存在)
    pub fn degradation_level(&self, chain_id: &str) -> Option<usize> {
        self.chains.get(chain_id).map(|c| c.current_level())
    }

    /// 获取当前降级链总数(监控指标)
    pub fn chain_count(&self) -> usize {
        self.chains.len()
    }

    /// 重置降级链到初始层级
    ///
    /// # 错误
    /// - `ChainNotFound`:指定 chain_id 的降级链不存在
    pub fn reset_chain(&self, chain_id: &str) -> Result<(), CsnError> {
        let mut chain = self
            .chains
            .get_mut(chain_id)
            .ok_or_else(|| CsnError::ChainNotFound {
                chain_id: chain_id.to_string(),
            })?;
        chain.reset();
        Ok(())
    }

    /// 启动 MCP Mesh 事务完成事件订阅任务(后台 tokio task)
    ///
    /// 订阅 `McpMeshTransactionCompleted` 事件,事务失败时(success=false)
    /// 触发对应能力的降级链推进。
    ///
    /// # 注意
    /// - 必须在 tokio runtime 上下文中调用
    /// - **订阅在 spawn 之前同步调用**(Week 6 教训 #9):
    ///   broadcast 不缓存历史消息,若在 async block 内订阅会因调度时机
    ///   不确定导致事件静默丢失
    ///
    /// 返回 `JoinHandle` 供调用者管理任务生命周期。
    /// 若未绑定 EventBus,返回 `None`。
    pub fn start_degradation_listener(&self) -> Option<tokio::task::JoinHandle<()>> {
        let bus = self.event_bus.clone()?;
        // WHY Arc::clone 而非 Arc::new(self.chains.clone()):
        // 必须共享同一 DashMap 实例,否则后台任务推进降级链的修改不会
        // 反映到原始 substitutor(Week 7 Task 2.5 关键 bug 修复)
        let chains = Arc::clone(&self.chains);

        // 在 spawn 之前同步订阅,确保不丢失后续事件
        let mut rx = bus.subscribe();

        Some(tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                if let NexusEvent::McpMeshTransactionCompleted { success: false, .. } = event {
                    // 事务失败:推进所有现有降级链
                    // WHY:事务失败可能是能力不可达导致,推进降级链触发下一级替代
                    for mut chain in chains.iter_mut() {
                        if chain.next_level().is_err() {
                            // 降级链已耗尽,记录日志后跳过
                            warn!(chain_id = chain.chain_id, "降级链已耗尽,无法继续推进");
                        }
                    }
                }
            }
        }))
    }

    /// 推进现有降级链或创建新链
    ///
    /// 返回当前降级层级(0=primary, 1=secondary, ...)
    ///
    /// WHY levels 不含 original_id:降级链的 levels 代表"替代路径",
    /// level 0 即为首次降级(primary substitute)。原始能力不可达时
    /// 才触发替代,因此首次调用即处于 level 0(已降级到 primary)。
    fn advance_or_create_chain(&self, original_id: &str) -> Result<u32, CsnError> {
        // 若已有降级链,推进;否则创建新链
        if let Some(mut chain) = self.chains.get_mut(original_id) {
            // 已存在:推进到下一级(若已耗尽则保持当前层级)
            let _ = chain.next_level();
            return Ok(chain.current_level() as u32);
        }

        // 创建新降级链:levels = [primary, secondary, tertiary](来自 config)
        let levels = self.config.default_degradation_levels.clone();
        let chain = DegradationChain::new(original_id.to_string(), levels);
        let level = chain.current_level() as u32;
        self.chains.insert(original_id.to_string(), chain);
        Ok(level)
    }

    /// 发布替代触发事件(best-effort,失败仅记录日志)
    async fn publish_substitution(
        &self,
        original_id: &str,
        candidate: &SubstitutionCandidate,
        degradation_level: u32,
    ) {
        if let Some(bus) = &self.event_bus {
            let event = NexusEvent::CsnSubstitutionTriggered {
                metadata: EventMetadata::new("csn-substitutor"),
                original_capability_id: original_id.to_string(),
                substitute_id: candidate.candidate_id.clone(),
                similarity_score: candidate.similarity_score,
                degradation_level,
            };
            if let Err(e) = bus.publish(event).await {
                warn!(error = %e, "CSN 替代触发事件发布失败");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === 辅助函数 ===

    fn make_descriptor(id: &str, vector: Vec<f32>) -> CapabilityDescriptor {
        CapabilityDescriptor::new(id, vector)
    }

    fn make_substitutor_with_caps(caps: Vec<(&str, Vec<f32>)>) -> CsnSubstitutor {
        let config = CsnConfig::default();
        let sub = CsnSubstitutor::new(config);
        for (id, v) in caps {
            sub.register_capability(make_descriptor(id, v))
                .expect("注册失败");
        }
        sub
    }

    // === 1. new/with_event_bus 创建 ===

    #[test]
    fn test_new_creates_empty_substitutor() {
        let sub = CsnSubstitutor::new(CsnConfig::default());
        assert_eq!(sub.registry().len(), 0);
        assert!(sub.chains.is_empty());
        assert!(sub.event_bus.is_none());
    }

    #[test]
    fn test_with_event_bus_binds_bus() {
        let bus = EventBus::new();
        let sub = CsnSubstitutor::with_event_bus(CsnConfig::default(), bus);
        assert!(sub.event_bus.is_some());
    }

    // === 2. register_capability 与 find_substitutes ===

    #[test]
    fn test_register_and_find_substitutes() {
        let v1 = vec![1.0; 50];
        let v2 = vec![0.99; 50]; // 与 v1 极相似
        let sub = make_substitutor_with_caps(vec![("cap-1", v1), ("cap-2", v2)]);

        let candidates = sub.find_substitutes("cap-1", 5);
        assert_eq!(candidates.len(), 1, "仅 cap-2 是候选(排除自身)");
        assert_eq!(candidates[0].candidate_id, "cap-2");
        assert!(candidates[0].similarity_score > 0.99);
    }

    #[test]
    fn test_find_substitutes_unregistered_returns_empty() {
        let sub = CsnSubstitutor::new(CsnConfig::default());
        let candidates = sub.find_substitutes("missing", 5);
        assert!(candidates.is_empty(), "未注册能力应返回空候选列表");
    }

    // === 3. trigger_substitution 全流程 ===

    #[tokio::test]
    async fn test_trigger_substitution_returns_candidate() {
        let v1 = vec![1.0; 50];
        let v2 = vec![0.9; 50];
        let sub = make_substitutor_with_caps(vec![("cap-1", v1), ("cap-2", v2)]);

        let candidate = sub.trigger_substitution("cap-1").await.expect("应找到替代");
        assert_eq!(candidate.candidate_id, "cap-2");
        assert!(candidate.similarity_score > 0.0);
    }

    #[tokio::test]
    async fn test_trigger_substitution_no_candidate_returns_error() {
        let sub = CsnSubstitutor::new(CsnConfig::default());
        let result = sub.trigger_substitution("missing").await;
        assert!(result.is_err());
        assert!(matches!(result, Err(CsnError::NoSubstituteFound { .. })));
    }

    // === 4. 降级链管理 ===

    #[tokio::test]
    async fn test_trigger_substitution_creates_chain() {
        let v1 = vec![1.0; 50];
        let v2 = vec![0.9; 50];
        let sub = make_substitutor_with_caps(vec![("cap-1", v1), ("cap-2", v2)]);

        sub.trigger_substitution("cap-1").await.unwrap();
        assert_eq!(sub.chains.len(), 1, "应创建 1 条降级链");
        assert!(sub.degradation_level("cap-1").is_some());
    }

    #[test]
    fn test_advance_degradation_chain_not_found() {
        let sub = CsnSubstitutor::new(CsnConfig::default());
        let result = sub.advance_degradation("missing");
        assert!(matches!(result, Err(CsnError::ChainNotFound { .. })));
    }

    #[test]
    fn test_reset_chain_not_found() {
        let sub = CsnSubstitutor::new(CsnConfig::default());
        let result = sub.reset_chain("missing");
        assert!(matches!(result, Err(CsnError::ChainNotFound { .. })));
    }
}
