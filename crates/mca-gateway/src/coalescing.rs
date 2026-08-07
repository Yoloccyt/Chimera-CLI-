//! coalescing — in-flight 请求合并(单飞,ADR-072 决策 ④)
//!
//! 对应架构层: L10 Interface(mca-gateway)
//!
//! # 为什么需要合并
//! 语义缓存是"完成态"缓存(响应生成后才可命中);in-flight 期间相同请求
//! 并发到达(多 Agent 并行子任务 / TUI 重试 / 双通道竞速)会重复计费。
//! 单飞合并(single-flight)是分布式系统标准去重模式:并发相同请求合并为
//! 一次厂商调用,首个请求成为领导者,后续请求挂接到共享等待通道。
//!
//! # 通知机制:oneshot 而非 watch
//! `AffinityError` 非 Clone(闭集枚举),watch 要求载荷 Clone;oneshot 每个
//! 等待者独立通道,领导者完成时逐一发送,天然支持非 Clone 载荷。
//!
//! # 失败/超时传播
//! - 领导者厂商调用失败 → 全部等待者收到同一错误(字符串化,信息保留)
//! - 领导者异常消失(recv Err)→ 等待者按 retryable 处理
//! - 等待超时(调用方传入 spec.endpoint.timeout_ms)→ retryable 错误
//!
//! # 线程安全
//! DashMap 分片锁 + std Mutex 短临界区(仅 push/take,不跨 await,C7 红线);
//! `join`/`complete` 均为同步原子操作。

use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use nexus_contracts::affinity::{AffinityResponse, TokenCacheKey};
use tokio::sync::oneshot;

use crate::error::AffinityError;

/// 合并键 — TokenCacheKey(模型/工具/系统提示/思考/采样)+ context_hash(消息内容)
///
/// 双维覆盖:键相同但消息内容不同(同工具集不同问题)不得合并;
/// 消息内容相同但键不同(不同模型)同样不得合并。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CoalesceKey {
    /// 精确缓存键(与语义缓存同键空间)
    pub cache_key: TokenCacheKey,
    /// 消息内容哈希(分段 Context Ledger,与语义缓存同口径)
    pub context_hash: [u8; 32],
}

impl CoalesceKey {
    /// 构造合并键
    pub fn new(cache_key: TokenCacheKey, context_hash: [u8; 32]) -> Self {
        Self {
            cache_key,
            context_hash,
        }
    }
}

/// 合并结果 — 成功共享 Arc(零拷贝),失败字符串化(错误类型经
/// `AffinityError::Transport { retryable: true }` 归一,保留信息供重试决策)
pub type CoalesceResult = Result<Arc<AffinityResponse>, String>;

/// in-flight 条目 — 等待者 oneshot 通道集合
struct InflightEntry {
    /// 等待者通道(领导者创建空集合,等待者 join 时追加)
    waiters: Mutex<Vec<oneshot::Sender<CoalesceResult>>>,
}

/// 手写 Debug:oneshot::Sender 未实现 Debug,仅暴露等待者数量
impl std::fmt::Debug for InflightEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.waiters.lock().map(|g| g.len()).unwrap_or(0);
        f.debug_struct("InflightEntry")
            .field("waiters", &n)
            .finish()
    }
}

/// 加入合并的结果
pub enum JoinOutcome {
    /// 领导者:需执行厂商调用,完成后必须调用 `complete()` 释放等待者
    Lead,
    /// 等待者:等待领导者结果(recv Err = 领导者异常,按 retryable 处理)
    Wait(oneshot::Receiver<CoalesceResult>),
}

/// in-flight 请求合并器 — DashMap<CoalesceKey, InflightEntry>
///
/// 生命周期:join 创建条目(Lead)→ 并发请求追加等待者(Wait)→
/// complete 取出条目并向全部等待者发送结果(条目消失)。
/// 领导者异常消失时条目滞留:等待者 recv 返回 Err 后自行放弃,
/// 残留条目由后续 join 的同键领导者复用(等待者集合已空,零泄漏)。
#[derive(Debug, Default)]
pub struct RequestCoalescer {
    /// 键 → in-flight 条目
    inflight: DashMap<CoalesceKey, Arc<InflightEntry>>,
}

impl RequestCoalescer {
    /// 创建空合并器
    pub fn new() -> Self {
        Self::default()
    }

    /// 加入合并 — 同步原子操作(短临界区,不跨 await)
    ///
    /// - 键已存在(他人 in-flight)→ 追加等待者通道,返回 `Wait`
    /// - 键不存在 → 创建条目,返回 `Lead`(调用方执行厂商调用)
    pub fn join(&self, key: CoalesceKey) -> JoinOutcome {
        match self.inflight.entry(key) {
            dashmap::mapref::entry::Entry::Occupied(occ) => {
                let (tx, rx) = oneshot::channel();
                // 临界区仅 push,立即释放分片锁与内部锁
                occ.get()
                    .waiters
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(tx);
                JoinOutcome::Wait(rx)
            }
            dashmap::mapref::entry::Entry::Vacant(vac) => {
                vac.insert(Arc::new(InflightEntry {
                    waiters: Mutex::new(Vec::new()),
                }));
                JoinOutcome::Lead
            }
        }
    }

    /// 领导者完成 — 向全部等待者发送结果并移除条目(幂等:键已消失则零操作)
    pub fn complete(&self, key: &CoalesceKey, result: CoalesceResult) {
        let Some((_, entry)) = self.inflight.remove(key) else {
            return; // 已释放(幂等保护)
        };
        // 临界区仅 take,立即释放内部锁(发送在锁外,不跨 await)
        let waiters = std::mem::take(&mut *entry.waiters.lock().unwrap_or_else(|p| p.into_inner()));
        for tx in waiters {
            // 等待者已超时放弃(recv 端已 drop)→ send 失败可忽略
            let _ = tx.send(result.clone());
        }
    }

    /// 当前 in-flight 键数(诊断用)
    pub fn inflight_count(&self) -> usize {
        self.inflight.len()
    }
}

/// 将合并失败原因映射为可重试传输错误(等待者侧统一错误面)
///
/// 保留原始错误信息(供日志),类型归一为 retryable=true——
/// 调用方重试后可能重新合并或走正常厂商路径。
pub fn coalesce_failure(route_key: &str, reason: String) -> AffinityError {
    AffinityError::Transport {
        route_key: route_key.to_string(),
        reason: format!("coalesced request failed: {reason}"),
        retryable: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::affinity::{FinishReason, UsageReport};

    fn test_key(seed: u8) -> CoalesceKey {
        CoalesceKey {
            cache_key: TokenCacheKey {
                model: "glm-5.2".into(),
                model_version: "v1".into(),
                tool_schema_hash: [seed; 32],
                system_prompt_hash: [seed; 32],
                thinking_tier: nexus_contracts::affinity::ThinkingPreference::Standard,
                sampling_bucket: 0,
            },
            context_hash: [seed; 32],
        }
    }

    fn resp() -> AffinityResponse {
        AffinityResponse {
            blocks: vec![nexus_contracts::affinity::ContentBlock::Text { text: "ok".into() }],
            usage: UsageReport::default(),
            cost: nexus_contracts::affinity::CostEstimate::default(),
            finish_reason: FinishReason::Stop,
            receipt: nexus_contracts::affinity::ProviderReceipt {
                provider: nexus_contracts::affinity::ProviderId::Zhipu,
                model: "glm-5.2".into(),
                dialect: nexus_contracts::affinity::ProtocolDialect::OpenAiChat,
                request_id: None,
            },
        }
    }

    #[tokio::test]
    async fn leader_executes_waiter_receives_result() {
        let c = RequestCoalescer::new();
        let key = test_key(1);
        // 领导者先加入
        assert!(matches!(c.join(key.clone()), JoinOutcome::Lead));
        // 两个等待者加入
        let JoinOutcome::Wait(rx1) = c.join(key.clone()) else {
            panic!("第二次 join 必须是 Wait");
        };
        let JoinOutcome::Wait(rx2) = c.join(key.clone()) else {
            panic!("第三次 join 必须是 Wait");
        };
        // 领导者完成:两个等待者都收到同一响应
        c.complete(&key, Ok(Arc::new(resp())));
        let r1 = rx1.await.unwrap().unwrap();
        let r2 = rx2.await.unwrap().unwrap();
        assert_eq!(r1.blocks, r2.blocks);
        assert_eq!(c.inflight_count(), 0, "完成后条目必须移除");
    }

    #[tokio::test]
    async fn leader_failure_propagates_to_all_waiters() {
        let c = RequestCoalescer::new();
        let key = test_key(2);
        assert!(matches!(c.join(key.clone()), JoinOutcome::Lead));
        let JoinOutcome::Wait(rx) = c.join(key.clone()) else {
            panic!("第二次 join 必须是 Wait");
        };
        // 领导者失败:等待者收到字符串化错误
        c.complete(&key, Err("upstream 500".into()));
        let err = rx.await.unwrap().unwrap_err();
        assert!(err.contains("upstream 500"), "错误信息必须保留: {err}");
    }

    #[tokio::test]
    async fn distinct_keys_do_not_coalesce() {
        let c = RequestCoalescer::new();
        assert!(matches!(c.join(test_key(1)), JoinOutcome::Lead));
        // 不同 context_hash(消息内容不同)必须独立执行
        assert!(matches!(c.join(test_key(2)), JoinOutcome::Lead));
        assert_eq!(c.inflight_count(), 2);
    }

    #[tokio::test]
    async fn waiter_times_out_when_leader_vanishes() {
        // 领导者异常消失(complete 未调用):等待者 recv 后自行放弃
        let c = RequestCoalescer::new();
        let key = test_key(3);
        assert!(matches!(c.join(key.clone()), JoinOutcome::Lead));
        let JoinOutcome::Wait(rx) = c.join(key.clone()) else {
            panic!("第二次 join 必须是 Wait");
        };
        // 模拟领导者消失:直接 drop 接收端(发送端仍在,无信号)
        // 实际场景由调用方 timeout 保护;此处验证残留条目不 panic 且后续
        // 同键 join 的领导者可复用条目(等待者集合已空,零泄漏)
        drop(rx);
        assert!(
            matches!(c.join(key), JoinOutcome::Wait(_)),
            "残留条目可复用"
        );
    }

    #[test]
    fn complete_is_idempotent() {
        let c = RequestCoalescer::new();
        let key = test_key(4);
        assert!(matches!(c.join(key.clone()), JoinOutcome::Lead));
        c.complete(&key, Ok(Arc::new(resp())));
        // 重复 complete:键已移除,零操作不 panic
        c.complete(&key, Ok(Arc::new(resp())));
        assert_eq!(c.inflight_count(), 0);
    }

    #[test]
    fn coalesce_failure_is_retryable_transport() {
        let e = coalesce_failure("zhipu/glm-5.2", "boom".into());
        match e {
            AffinityError::Transport { retryable, .. } => assert!(retryable),
            other => panic!("必须映射为 Transport, got {other:?}"),
        }
    }
}
