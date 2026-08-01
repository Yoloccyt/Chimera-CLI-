//! 能力替代网络(CSN)— 能力降级链,在缺失时自动寻找替代实现
//!
//! 对应架构层:L10 Interface
//! 对应创新点:CSN(Capability Substitution Network)
//! 设计来源:MCP Mesh 量子网格的容错降级机制 + ADR-023 + ADR-062(v2.9.0-omega 重设计)
//!
//! ## 核心机制
//! - 维护能力语义向量注册表(`SubstitutionCandidateRegistry`),100 能力 × 50 维 in-memory
//! - 能力不可达时,基于余弦相似度寻找 Top-K 替代候选(`select_nth_unstable` O(n))
//! - 多级降级链(`DegradationChain`)支持 ≥ 3 级降级,逐级回退
//! - 通过 EventBus 发布 `CsnSubstitutionTriggered`、订阅 `McpMeshTransactionCompleted`
//!
//! ## v2.9.0-omega 重设计(ADR-062)
//! - 降级链 level N 与候选选择关联:level N 返回 Top-(N+1) 的第 N+1 个候选
//! - `default_degradation_levels: Vec<Vec<String>>`,每级可显式指定候选 ID
//! - MCP 失败推进根据 `capability_id` 字段精准推进(避免误伤其他链)
//! - chains TTL 清理 + ChainExhausted 自动移除
//! - `similarity_threshold` 过滤低相似度候选
//! - `register_capability` 校验向量维度
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

/// MCA 降级链通道亲和 — CapabilitySet 加权距离 + 配额耗尽降级选择(ADR-068)
pub mod channel_affinity;
pub mod config;
pub mod degradation_chain;
pub mod error;
pub mod similarity;
pub mod substitutor;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use channel_affinity::{capability_distance, select_substitute};
pub use config::CsnConfig;
pub use degradation_chain::DegradationChain;
pub use error::CsnError;
pub use substitutor::{SubstitutionCandidateRegistry, SubstitutionRegistryStats};
pub use types::{CapabilityDescriptor, CapabilityMetadata, SubstitutionCandidate};

use dashmap::DashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

use event_bus::{EventBus, EventMetadata, NexusEvent};

use crate::similarity::cosine_similarity;

/// 能力替代网络核心组件 — 能力注册、替代查询与降级链管理
///
/// 持有并发安全的能力注册表(`Arc<SubstitutionCandidateRegistry>`)、
/// 降级链表(`DashMap<String, DegradationChain>`)、事务映射
/// (`DashMap<String, String>` 用于 transaction_id → chain_id)与可选的 EventBus。
///
/// ## 线程安全
/// - `SubstitutionCandidateRegistry` 基于 `DashMap`(分片锁),`&self` 调用安全
/// - `DegradationChain` 存储在 `DashMap` 中,按 chain_id 分片
/// - `transaction_chain_map` 同样基于 `DashMap`
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
    /// transaction_id → chain_id 映射(Arc 共享,后台订阅任务读取)
    ///
    /// WHY:SubTask 0.5.8 — MCP 失败事件可能携带 transaction_id 但未携带
    /// capability_id,通过此映射反查关联的降级链,实现精准推进。
    transaction_chain_map: Arc<DashMap<String, String>>,
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
            transaction_chain_map: Arc::new(DashMap::new()),
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
            transaction_chain_map: Arc::new(DashMap::new()),
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
    /// - `InvalidCapability`:语义向量维度与 `vector_dimension` 不匹配,或 capability_id 为空
    /// - `RegistryFull`:注册表已满(且 key 不存在)
    ///
    /// # v2.9.0-omega 新增(SubTask 0.5.7)
    /// 校验 `semantic_vector.len() == config.vector_dimension`,不一致返回 `InvalidCapability`。
    /// WHY:维度不一致会导致余弦相似度计算取 min 长度,产生错误的相似度分数,
    /// 在配置层提前拦截而非运行时静默错误。
    pub fn register_capability(&self, cap: CapabilityDescriptor) -> Result<(), CsnError> {
        // SubTask 0.5.7: 向量维度校验
        if cap.semantic_vector.len() != self.config.vector_dimension {
            return Err(CsnError::InvalidCapability {
                reason: format!(
                    "语义向量维度不匹配:期望 {},实际 {}",
                    self.config.vector_dimension,
                    cap.semantic_vector.len()
                ),
            });
        }
        self.registry.register(cap)
    }

    /// 查找替代候选 — 基于余弦相似度选 Top-K,并过滤低于阈值者
    ///
    /// 从注册表中查找与 `capability_id` 语义最相似的 K 个候选(排除自身)。
    /// 使用 `select_nth_unstable` 实现 O(n) Top-K 选择(降序)。
    ///
    /// # v2.9.0-omega 新增(SubTask 0.5.6)
    /// 过滤相似度低于 `config.similarity_threshold` 的候选,避免选择语义不相关的替代。
    ///
    /// # 返回
    /// 按 `similarity_score` 降序排列的 Top-K 候选列表(已过滤低相似度);
    /// 若 `capability_id` 未注册,返回空 Vec。
    pub fn find_substitutes(
        &self,
        capability_id: &str,
        top_k: usize,
    ) -> Vec<SubstitutionCandidate> {
        let mut candidates = self.registry.find_substitutes(capability_id, top_k);
        // SubTask 0.5.6: 过滤低于 similarity_threshold 的候选
        // WHY f32 直接比较:与 similarity_score 类型一致,避免 §4.4 #6 f32→f64 精度膨胀
        let threshold = self.config.similarity_threshold;
        candidates.retain(|c| c.similarity_score >= threshold);
        candidates
    }

    /// 触发能力替代 — 查找最优替代并推进降级链
    ///
    /// # v2.9.0-omega 重设计(SubTask 0.5.3,ADR-062)
    /// 根据降级链 level N 选择候选:
    /// - 若 `default_degradation_levels[N]` 非空 → 按显式 ID 列表查找
    /// - 否则 → `find_substitutes(original_id, N+1)` 返回 Top-(N+1) 的第 N+1 个候选
    ///
    /// # 流程
    /// 1. 推进降级链或创建新链(获取当前 level N)
    /// 2. 根据 level N 选择候选(Top-(N+1) 或显式 ID)
    /// 3. 发布 `CsnSubstitutionTriggered` 事件(若绑定 EventBus)
    ///
    /// # 错误
    /// - `NoSubstituteFound`:无可用替代候选
    /// - `ChainExhausted`:降级链已耗尽(需调用方决策:重置或放弃)
    pub async fn trigger_substitution(
        &self,
        original_id: &str,
    ) -> Result<SubstitutionCandidate, CsnError> {
        // 1. 推进降级链或创建新链(获取当前 level N)
        let level = self.advance_or_create_chain(original_id)?;
        let level_idx = level as usize;

        // 2. 根据 level N 选择候选
        let candidate = self.select_candidate_for_level(original_id, level_idx)?;

        // 3. 发布事件(best-effort,失败仅记录日志)
        self.publish_substitution(original_id, &candidate, level)
            .await;

        Ok(candidate)
    }

    /// 根据 level 选择候选 — Top-(N+1) 模式或显式 ID 列表
    ///
    /// # 选择逻辑
    /// - 若 `default_degradation_levels[level_idx]` 非空 → 显式 ID 列表查找
    /// - 否则 → Top-(N+1) 模式:level 0 → Top-1 第 1 个,level 1 → Top-2 第 2 个,...
    ///
    /// # 候选不足处理
    /// Top-(N+1) 模式下,若候选数 < N+1,返回最后一个可用候选(避免降级链提前终止)。
    fn select_candidate_for_level(
        &self,
        original_id: &str,
        level_idx: usize,
    ) -> Result<SubstitutionCandidate, CsnError> {
        // 检查 default_degradation_levels[level_idx]
        if let Some(explicit_ids) = self.config.default_degradation_levels.get(level_idx) {
            if !explicit_ids.is_empty() {
                // 显式 ID 列表:查找第一个在注册表中存在的候选
                return self.find_explicit_substitute(original_id, explicit_ids);
            }
        }
        // Top-(N+1) 模式:level 0 → Top-1, level 1 → Top-2, ...
        let top_k = level_idx + 1;
        let mut candidates = self.find_substitutes(original_id, top_k);
        // 返回第 (N+1) 个候选(Top-(N+1) 中的最后一个)
        // WHY:每次推进应返回不同的候选,Top-(N+1) 的第 N+1 个是新候选
        if candidates.len() >= top_k {
            Ok(candidates.remove(top_k - 1))
        } else if !candidates.is_empty() {
            // 候选不足:返回最后一个可用候选(避免降级链提前终止)
            Ok(candidates.pop().expect("已检查非空"))
        } else {
            Err(CsnError::NoSubstituteFound {
                capability_id: original_id.to_string(),
            })
        }
    }

    /// 按显式 ID 列表查找替代候选
    ///
    /// 遍历 `explicit_ids`,返回第一个在注册表中存在且不是 `original_id` 的候选。
    /// 相似度通过与 `original_id` 的语义向量计算得出。
    fn find_explicit_substitute(
        &self,
        original_id: &str,
        explicit_ids: &[String],
    ) -> Result<SubstitutionCandidate, CsnError> {
        // 获取目标向量(若 original_id 未注册,相似度计算会返回 0.0)
        let target_vector = self
            .registry
            .get(original_id)
            .map(|cap| cap.semantic_vector);

        for id in explicit_ids {
            if id == original_id {
                continue; // 跳过自身
            }
            if let Some(cap) = self.registry.get(id) {
                let score = match &target_vector {
                    Some(target) => cosine_similarity(target, &cap.semantic_vector),
                    // 原始能力未注册时相似度为 0.0(仍可作显式替代)
                    None => 0.0,
                };
                return Ok(SubstitutionCandidate {
                    candidate_id: id.clone(),
                    similarity_score: score,
                    // 显式指定的候选 tier=0(primary)
                    tier: 0,
                });
            }
        }
        Err(CsnError::NoSubstituteFound {
            capability_id: original_id.to_string(),
        })
    }

    /// 显式推进降级链到下一级
    ///
    /// # v2.9.0-omega 新增(SubTask 0.5.9)
    /// 推进到末端返回 `ChainExhausted` 时,自动移除该降级链(避免内存泄漏)。
    ///
    /// # 错误
    /// - `ChainNotFound`:指定 chain_id 的降级链不存在
    /// - `ChainExhausted`:已到达降级链末端,无法继续推进(链将被自动移除)
    pub fn advance_degradation(&self, chain_id: &str) -> Result<(), CsnError> {
        // 持有 get_mut 引用时调用 next_level,然后释放引用
        let result = {
            let mut chain =
                self.chains
                    .get_mut(chain_id)
                    .ok_or_else(|| CsnError::ChainNotFound {
                        chain_id: chain_id.to_string(),
                    })?;
            chain.next_level()
        };
        // ChainExhausted 时自动移除链(SubTask 0.5.9)
        // WHY:降级链耗尽后无继续推进意义,自动移除避免内存泄漏;
        // 调用方收到 ChainExhausted 错误后可决策:重置(reset_chain)或放弃
        if matches!(result, Err(CsnError::ChainExhausted { .. })) {
            self.chains.remove(chain_id);
        }
        result
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

    /// 记录 transaction_id → chain_id(通常 = capability_id)映射
    ///
    /// 用于 MCP 失败事件携带 transaction_id 但未携带 capability_id 时,
    /// 反查关联的降级链(SubTask 0.5.8)。
    ///
    /// # 参数
    /// - `transaction_id`:MCP Mesh 事务 ID
    /// - `capability_id`:关联的能力 ID(作为 chain_id)
    pub fn record_transaction(&self, transaction_id: &str, capability_id: &str) {
        self.transaction_chain_map
            .insert(transaction_id.to_string(), capability_id.to_string());
    }

    /// 清理过期的降级链(TTL > 指定时长)
    ///
    /// 返回被清理的降级链数量。
    ///
    /// # 用途
    /// 供调用方按需清理长期未推进的降级链,避免内存泄漏(SubTask 0.5.9)。
    /// 建议调用方周期性调用(如每小时一次),而非依赖后台任务。
    ///
    /// WHY 不启动后台任务:遵循 §4.4 #7 fire-and-forget 评估框架,
    /// TTL 清理是幂等操作,但显式调用更可控(避免后台任务 panic 影响数据一致性)。
    pub fn cleanup_chains(&self, ttl: Duration) -> usize {
        let mut to_remove: Vec<String> = Vec::new();
        for chain in self.chains.iter() {
            if chain.is_expired(ttl) {
                to_remove.push(chain.chain_id.clone());
            }
        }
        let removed = to_remove.len();
        for chain_id in to_remove {
            self.chains.remove(&chain_id);
        }
        removed
    }

    /// 启动 MCP Mesh 事务完成事件订阅任务(后台 tokio task)
    ///
    /// 订阅 `McpMeshTransactionCompleted` 事件,事务失败时(success=false)
    /// 触发对应能力的降级链推进。
    ///
    /// # v2.9.0-omega 重设计(SubTask 0.5.8)
    /// 根据 `capability_id` 字段精准推进:
    /// 1. 优先用 `capability_id`(若 Some)作为 chain_id 推进
    /// 2. 否则用 `transaction_id` → chain_id 映射(`record_transaction` 记录)
    /// 3. 都无 → 推进所有链(向后兼容,保持 v2.8.0 行为)
    ///
    /// ChainExhausted 时自动移除该链(SubTask 0.5.9)。
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
        let tx_map = Arc::clone(&self.transaction_chain_map);

        // 在 spawn 之前同步订阅,确保不丢失后续事件
        let mut rx = bus.subscribe();

        Some(tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                if let NexusEvent::McpMeshTransactionCompleted {
                    success: false,
                    transaction_id,
                    capability_id,
                    ..
                } = event
                {
                    // 确定要推进的 chain_id:
                    // 1. 优先用 capability_id(若 Some)
                    // 2. 否则用 transaction_id → chain_id 映射
                    // 3. 都无 → None(回退到推进所有链)
                    let target_chain_id = capability_id
                        .or_else(|| tx_map.get(&transaction_id).map(|r| r.value().clone()));

                    if let Some(chain_id) = target_chain_id {
                        // 精准推进:只推进该 chain_id 的链
                        let result = {
                            let mut chain = match chains.get_mut(&chain_id) {
                                Some(c) => c,
                                None => continue,
                            };
                            chain.next_level()
                        };
                        if matches!(result, Err(CsnError::ChainExhausted { .. })) {
                            // ChainExhausted → 自动移除(SubTask 0.5.9)
                            warn!(chain_id = %chain_id, "降级链已耗尽,自动移除");
                            chains.remove(&chain_id);
                        }
                    } else {
                        // 向后兼容:capability_id 为 None 且无映射时,推进所有链
                        // WHY 保留原行为:旧版 mcp-mesh 不填充 capability_id,
                        // 保持兼容避免破坏现有部署
                        let mut to_remove: Vec<String> = Vec::new();
                        for mut chain in chains.iter_mut() {
                            if chain.next_level().is_err() {
                                to_remove.push(chain.chain_id.clone());
                            }
                        }
                        for cid in to_remove {
                            warn!(chain_id = %cid, "降级链已耗尽,自动移除");
                            chains.remove(&cid);
                        }
                    }
                }
            }
        }))
    }

    /// 推进现有降级链或创建新链
    ///
    /// 返回当前降级层级(0=primary substitute, 1=secondary substitute, ...)
    ///
    /// WHY levels 用 "level-{i}" 标识:DegradationChain.levels 仅跟踪深度,
    /// 具体候选选择由 `select_candidate_for_level` 根据
    /// `config.default_degradation_levels[level]` 决定(空 Vec → Top-(N+1),
    /// 非空 Vec → 显式 ID 列表)。
    fn advance_or_create_chain(&self, original_id: &str) -> Result<u32, CsnError> {
        // 若已有降级链,推进;否则创建新链
        if let Some(mut chain) = self.chains.get_mut(original_id) {
            // 已存在:推进到下一级(若已耗尽则返回 ChainExhausted 错误)
            chain.next_level()?;
            return Ok(chain.current_level() as u32);
        }

        // 创建新降级链:levels 来自 config.default_degradation_levels 的深度
        let levels: Vec<String> = (0..self.config.default_degradation_levels.len())
            .map(|i| format!("level-{i}"))
            .collect();
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

    // === SubTask 0.5.7: register_capability 维度校验 ===

    #[test]
    fn test_register_capability_dimension_mismatch() {
        let sub = CsnSubstitutor::new(CsnConfig::default());
        // vector_dimension=50,传入 49 维向量应失败
        let cap = make_descriptor("cap-1", vec![1.0; 49]);
        let result = sub.register_capability(cap);
        assert!(
            matches!(result, Err(CsnError::InvalidCapability { .. })),
            "维度不匹配应返回 InvalidCapability"
        );
    }

    #[test]
    fn test_register_capability_dimension_match_ok() {
        let sub = CsnSubstitutor::new(CsnConfig::default());
        let cap = make_descriptor("cap-1", vec![1.0; 50]);
        assert!(sub.register_capability(cap).is_ok());
    }

    // === SubTask 0.5.6: find_substitutes similarity_threshold 过滤 ===

    #[test]
    fn test_find_substitutes_filters_below_threshold() {
        // 构造:cap-1 与 cap-2 相似度 ~0.99(>0.5),cap-1 与 cap-3 相似度 ~0.0(<0.5)
        let mut v1 = vec![0.0_f32; 50];
        v1[0] = 1.0;
        let mut v2 = vec![0.0_f32; 50];
        v2[0] = 0.99; // 与 v1 极相似
        v2[1] = 0.1;
        let mut v3 = vec![0.0_f32; 50];
        v3[1] = 1.0; // 与 v1 正交(相似度 0)

        let sub = make_substitutor_with_caps(vec![("cap-1", v1), ("cap-2", v2), ("cap-3", v3)]);
        let candidates = sub.find_substitutes("cap-1", 5);
        // cap-3 相似度 < 0.5,应被过滤
        assert_eq!(candidates.len(), 1, "cap-3 应被 threshold=0.5 过滤");
        assert_eq!(candidates[0].candidate_id, "cap-2");
    }

    #[test]
    fn test_find_substitutes_threshold_zero_disables_filter() {
        // threshold=0.0 禁用过滤
        let config = CsnConfig {
            similarity_threshold: 0.0,
            ..CsnConfig::default()
        };
        let sub = CsnSubstitutor::new(config);
        let mut v1 = vec![0.0_f32; 50];
        v1[0] = 1.0;
        let mut v2 = vec![0.0_f32; 50];
        v2[1] = 1.0; // 正交,相似度 0
        sub.register_capability(make_descriptor("cap-1", v1))
            .unwrap();
        sub.register_capability(make_descriptor("cap-2", v2))
            .unwrap();

        let candidates = sub.find_substitutes("cap-1", 5);
        // threshold=0.0 时 cap-2(相似度 0)应保留
        assert_eq!(candidates.len(), 1, "threshold=0.0 不应过滤");
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

    // === SubTask 0.5.3: trigger_substitution 重设计(Top-(N+1) 模式)===

    #[tokio::test]
    async fn test_trigger_substitution_top_n_plus_1_progression() {
        // 注册 4 个能力:cap-1 是原始,cap-2/3/4 是候选(相似度递减)
        // WHY 不用全 1.0/0.99/0.9/0.8 标量倍数:余弦相似度对方向敏感,
        // 标量倍数向量方向相同(余弦=1.0),无法区分 Top-1/2/3。
        // 改用前缀 1.0 + 尾部 0.0 的模式,确保方向不同:
        // - v1 = [1.0; 50]            (全 1.0,参考向量)
        // - v2 = [1.0; 40] + [0.0;10] (40/50 重叠,余弦 ≈ 0.894)
        // - v3 = [1.0; 30] + [0.0;20] (30/50 重叠,余弦 ≈ 0.775)
        // - v4 = [1.0; 20] + [0.0;30] (20/50 重叠,余弦 ≈ 0.632)
        // 所有相似度 > 0.5(默认阈值),可被 find_substitutes 返回
        let v1 = vec![1.0; 50];
        let mut v2 = vec![1.0; 40];
        v2.extend(vec![0.0; 10]);
        let mut v3 = vec![1.0; 30];
        v3.extend(vec![0.0; 20]);
        let mut v4 = vec![1.0; 20];
        v4.extend(vec![0.0; 30]);
        let sub = make_substitutor_with_caps(vec![
            ("cap-1", v1),
            ("cap-2", v2),
            ("cap-3", v3),
            ("cap-4", v4),
        ]);

        // level 0:Top-1 的第 1 个 → cap-2(最高相似度)
        let c0 = sub.trigger_substitution("cap-1").await.expect("level 0");
        assert_eq!(c0.candidate_id, "cap-2", "level 0 应返回 Top-1 (cap-2)");

        // level 1:Top-2 的第 2 个 → cap-3(次高相似度)
        let c1 = sub.trigger_substitution("cap-1").await.expect("level 1");
        assert_eq!(
            c1.candidate_id, "cap-3",
            "level 1 应返回 Top-2 的第 2 个 (cap-3)"
        );

        // level 2:Top-3 的第 3 个 → cap-4(最低相似度)
        let c2 = sub.trigger_substitution("cap-1").await.expect("level 2");
        assert_eq!(
            c2.candidate_id, "cap-4",
            "level 2 应返回 Top-3 的第 3 个 (cap-4)"
        );
    }

    #[tokio::test]
    async fn test_trigger_substitution_explicit_ids() {
        // 显式 ID 列表模式
        let config = CsnConfig {
            default_degradation_levels: vec![
                vec!["explicit-sub-1".into()],
                vec!["explicit-sub-2".into()],
                vec![],
            ],
            ..CsnConfig::default()
        };
        let v1 = vec![1.0; 50];
        let v2 = vec![0.5; 50]; // 显式候选 1
        let v3 = vec![0.6; 50]; // 显式候选 2
        let sub = CsnSubstitutor::new(config);
        sub.register_capability(make_descriptor("cap-1", v1))
            .unwrap();
        sub.register_capability(make_descriptor("explicit-sub-1", v2))
            .unwrap();
        sub.register_capability(make_descriptor("explicit-sub-2", v3))
            .unwrap();

        // level 0:显式 ID ["explicit-sub-1"] → explicit-sub-1
        let c0 = sub.trigger_substitution("cap-1").await.expect("level 0");
        assert_eq!(c0.candidate_id, "explicit-sub-1");

        // level 1:显式 ID ["explicit-sub-2"] → explicit-sub-2
        let c1 = sub.trigger_substitution("cap-1").await.expect("level 1");
        assert_eq!(c1.candidate_id, "explicit-sub-2");
    }

    #[tokio::test]
    async fn test_trigger_substitution_chain_exhausted_error() {
        // 默认 3 级降级,触发 4 次应返回 ChainExhausted
        let v1 = vec![1.0; 50];
        let v2 = vec![0.9; 50];
        let sub = make_substitutor_with_caps(vec![("cap-1", v1), ("cap-2", v2)]);

        // level 0/1/2 成功
        sub.trigger_substitution("cap-1").await.unwrap();
        sub.trigger_substitution("cap-1").await.unwrap();
        sub.trigger_substitution("cap-1").await.unwrap();

        // 第 4 次:ChainExhausted(降级链已耗尽)
        let result = sub.trigger_substitution("cap-1").await;
        assert!(
            matches!(result, Err(CsnError::ChainExhausted { .. })),
            "第 4 次触发应返回 ChainExhausted"
        );
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

    // === SubTask 0.5.9: ChainExhausted 自动移除 ===

    #[tokio::test]
    async fn test_advance_degradation_exhausted_removes_chain() {
        let v1 = vec![1.0; 50];
        let v2 = vec![0.9; 50];
        let sub = make_substitutor_with_caps(vec![("cap-1", v1), ("cap-2", v2)]);

        // 创建降级链
        sub.trigger_substitution("cap-1").await.unwrap();
        // 推进到末端(默认 3 级:level 0 → 1 → 2)
        sub.advance_degradation("cap-1").expect("推进到 level 1");
        sub.advance_degradation("cap-1").expect("推进到 level 2");

        // 已耗尽,应返回错误 + 自动移除
        let result = sub.advance_degradation("cap-1");
        assert!(
            matches!(result, Err(CsnError::ChainExhausted { .. })),
            "末端推进应返回 ChainExhausted"
        );
        // ChainExhausted 后链应被自动移除
        assert_eq!(
            sub.degradation_level("cap-1"),
            None,
            "ChainExhausted 后链应被自动移除"
        );
        assert_eq!(sub.chain_count(), 0, "ChainExhausted 后链应被自动移除");
    }

    // === SubTask 0.5.9: cleanup_chains TTL ===

    #[tokio::test]
    async fn test_cleanup_chains_removes_expired() {
        let v1 = vec![1.0; 50];
        let v2 = vec![0.9; 50];
        let sub = make_substitutor_with_caps(vec![("cap-1", v1), ("cap-2", v2)]);

        // 创建降级链
        sub.trigger_substitution("cap-1").await.unwrap();
        assert_eq!(sub.chain_count(), 1);

        // TTL=1ns,所有 chain 都已过期(因为创建后立即检查)
        // WHY 1ns:创建到 cleanup 之间必然 > 1ns,确保 is_expired 返回 true
        let removed = sub.cleanup_chains(Duration::from_nanos(1));
        assert_eq!(removed, 1, "应清理 1 条过期降级链");
        assert_eq!(sub.chain_count(), 0, "清理后应无降级链");
    }

    #[tokio::test]
    async fn test_cleanup_chains_keeps_fresh() {
        let v1 = vec![1.0; 50];
        let v2 = vec![0.9; 50];
        let sub = make_substitutor_with_caps(vec![("cap-1", v1), ("cap-2", v2)]);

        sub.trigger_substitution("cap-1").await.unwrap();
        // TTL=1小时,刚创建的 chain 不应被清理
        let removed = sub.cleanup_chains(Duration::from_secs(3600));
        assert_eq!(removed, 0, "刚创建的 chain 不应被清理");
        assert_eq!(sub.chain_count(), 1, "chain 应保留");
    }

    // === SubTask 0.5.8: record_transaction + MCP 精准推进 ===

    #[test]
    fn test_record_transaction_creates_mapping() {
        let sub = CsnSubstitutor::new(CsnConfig::default());
        sub.record_transaction("tx-1", "cap-1");
        // 验证映射存在(通过内部字段访问)
        assert_eq!(
            sub.transaction_chain_map
                .get("tx-1")
                .map(|r| r.value().clone()),
            Some("cap-1".to_string())
        );
    }

    // === SubTask 0.5.3 + 0.5.8: 显式 ID 与 transaction_id 协同 ===

    #[tokio::test]
    async fn test_mcp_failure_advances_specific_chain_via_capability_id() {
        // capability_id 字段填充时,只推进对应 chain
        let bus = EventBus::new();
        let sub = CsnSubstitutor::with_event_bus(CsnConfig::default(), bus.clone());
        let v1 = vec![1.0; 50];
        let v2 = vec![0.9; 50];
        let v3 = vec![0.8; 50];
        sub.register_capability(make_descriptor("cap-1", v1))
            .unwrap();
        sub.register_capability(make_descriptor("cap-2", v2))
            .unwrap();
        sub.register_capability(make_descriptor("cap-3", v3))
            .unwrap();

        // 创建两条降级链
        sub.trigger_substitution("cap-1").await.unwrap();
        sub.trigger_substitution("cap-2").await.unwrap();
        let cap1_level_before = sub.degradation_level("cap-1").unwrap();
        let cap2_level_before = sub.degradation_level("cap-2").unwrap();

        let handle = sub.start_degradation_listener().expect("应启动订阅");

        // 发布失败事件,capability_id = "cap-1"
        bus.publish(NexusEvent::McpMeshTransactionCompleted {
            metadata: EventMetadata::new("mcp-mesh"),
            transaction_id: "tx-1".into(),
            participant_count: 3,
            latency_ms: 100,
            success: false,
            capability_id: Some("cap-1".into()),
        })
        .await
        .expect("发布失败");

        tokio::time::sleep(Duration::from_millis(150)).await;

        // cap-1 应被推进
        let cap1_level_after = sub.degradation_level("cap-1");
        assert!(cap1_level_after.is_some(), "cap-1 链应仍存在(未耗尽)");
        assert!(
            cap1_level_after.unwrap() > cap1_level_before,
            "cap-1 链应被推进"
        );

        // cap-2 不应被推进(精准推进)
        let cap2_level_after = sub.degradation_level("cap-2").unwrap();
        assert_eq!(
            cap2_level_after, cap2_level_before,
            "cap-2 链不应被推进(精准推进只影响 cap-1)"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn test_mcp_failure_advances_via_transaction_id_mapping() {
        // capability_id 为 None,但有 transaction_id → chain_id 映射
        let bus = EventBus::new();
        let sub = CsnSubstitutor::with_event_bus(CsnConfig::default(), bus.clone());
        let v1 = vec![1.0; 50];
        let v2 = vec![0.9; 50];
        sub.register_capability(make_descriptor("cap-1", v1))
            .unwrap();
        sub.register_capability(make_descriptor("cap-2", v2))
            .unwrap();

        sub.trigger_substitution("cap-1").await.unwrap();
        let level_before = sub.degradation_level("cap-1").unwrap();

        // 记录 transaction_id → chain_id 映射
        sub.record_transaction("tx-123", "cap-1");

        let handle = sub.start_degradation_listener().expect("应启动订阅");

        // 发布失败事件,capability_id = None,但 transaction_id 有映射
        bus.publish(NexusEvent::McpMeshTransactionCompleted {
            metadata: EventMetadata::new("mcp-mesh"),
            transaction_id: "tx-123".into(),
            participant_count: 3,
            latency_ms: 100,
            success: false,
            capability_id: None,
        })
        .await
        .expect("发布失败");

        tokio::time::sleep(Duration::from_millis(150)).await;

        let level_after = sub.degradation_level("cap-1").unwrap();
        assert!(
            level_after > level_before,
            "通过 transaction_id 映射应推进 cap-1 链"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn test_mcp_failure_no_capability_no_mapping_advances_all() {
        // 向后兼容:capability_id 为 None 且无映射时,推进所有链
        let bus = EventBus::new();
        let sub = CsnSubstitutor::with_event_bus(CsnConfig::default(), bus.clone());
        let v1 = vec![1.0; 50];
        let v2 = vec![0.9; 50];
        sub.register_capability(make_descriptor("cap-1", v1))
            .unwrap();
        sub.register_capability(make_descriptor("cap-2", v2))
            .unwrap();

        sub.trigger_substitution("cap-1").await.unwrap();
        sub.trigger_substitution("cap-2").await.unwrap();
        let cap1_before = sub.degradation_level("cap-1").unwrap();
        let cap2_before = sub.degradation_level("cap-2").unwrap();

        let handle = sub.start_degradation_listener().expect("应启动订阅");

        // 发布失败事件,capability_id = None,无映射
        bus.publish(NexusEvent::McpMeshTransactionCompleted {
            metadata: EventMetadata::new("mcp-mesh"),
            transaction_id: "tx-unknown".into(),
            participant_count: 3,
            latency_ms: 100,
            success: false,
            capability_id: None,
        })
        .await
        .expect("发布失败");

        tokio::time::sleep(Duration::from_millis(150)).await;

        // 两条链都应被推进(向后兼容)
        let cap1_after = sub.degradation_level("cap-1").unwrap();
        let cap2_after = sub.degradation_level("cap-2").unwrap();
        assert!(cap1_after > cap1_before, "向后兼容:cap-1 链应被推进");
        assert!(cap2_after > cap2_before, "向后兼容:cap-2 链应被推进");

        handle.abort();
    }
}
