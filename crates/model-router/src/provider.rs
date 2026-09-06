//! 模型 provider 开放注册与热更 — 能力 seam(capability seam)
//!
//! 对应架构:L1 Core,是 model-router 对外暴露的 provider 抽象边界。
//! 对应计划:WI-06 模型 provider 开放注册与热更。
//!
//! # 核心职责
//! - `ModelProvider` trait:抽象任意底层模型后端(OpenAI / Anthropic / 本地 / 自定义),
//!   通过 `id()` / `capabilities()` / `complete()` / `health()` 四类能力点与路由层解耦。
//! - `ProviderSpec`:TOML 注册表条目(provider_id / endpoint / model_map / caps),
//!   派生 `serde::Deserialize`,可直接由 `toml`(或 `serde_json`)反序列化获得。
//! - `ProviderRegistry`:基于 `arc_swap::ArcSwap` 的 RCU(Read-Copy-Update)热更注册表,
//!   `reload_from_specs()` 原子换表,配置错误时**拒载且不回滚好表**。
//!
//! # 设计要点
//! - **读快照语义**:`get()` 通过 `load_full()` 取到 `Arc<HashMap>` 快照,立即 clone 出
//!   目标 provider 的 `Arc` 并释放表引用,因此**读路径不持 guard**,满足红线「不持锁跨
//!   await / ArcSwap load guard 只在同步块用」。
//! - **原子换表**:`store(Arc)` 单指令替换指针,写入者不需要等待读者;热更后即读即见,
//!   生效延迟远低于 1s。旧表在无读者引用后被自动释放。
//! - **拒载不回滚**:`reload_from_specs()` 先完整构建新表;任意一个 spec 解析/工厂失败
//!   → 整体返回 `Err` 且**不执行 store**,已存在的旧表原样保留。

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::RouterError;

// ===========================================================================
// 能力规格 & 辅助类型
// ===========================================================================

/// 注意力模式 — provider 作用的序列注意力机制
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum AttentionMode {
    /// 全量注意力(注意力随序列长度线性增长)
    #[default]
    Full,
    /// 局部窗口注意力(固定窗口,长序列下常数内存)
    Local,
    /// 稀疏注意力(仅查询关键 token,配 Ω-Sparse 稀疏掩码)
    Sparse,
}

/// Provider 能力规格 — 描述底层模型支持的能力集
///
/// 路由层据此判断某 provider 是否能承接某类任务(视觉 / 工具 / 流式 / 注意力范围)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCaps {
    /// 上下文窗口大小(token 数)
    pub context: usize,
    /// 是否支持视觉输入
    pub vision: bool,
    /// 是否支持工具/函数调用
    pub tools: bool,
    /// 是否支持流式输出
    pub streaming: bool,
    /// 是否支持推理努力度(thought effort)调节
    pub effort: bool,
    /// 序列注意力模式
    pub attention_mode: AttentionMode,
}

impl Default for ProviderCaps {
    fn default() -> Self {
        ProviderCaps {
            context: 8192,
            vision: false,
            tools: false,
            streaming: true,
            effort: false,
            attention_mode: AttentionMode::Full,
        }
    }
}

/// 补全请求 — `ModelProvider::complete` 的入参
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionReq {
    /// 目标 model_map 解析出的下游模型名(与注册表 model_key 对应)
    pub model: String,
    /// 输入提示词
    pub prompt: String,
    /// 输出 token 上限(None = 由 provider 默认)
    pub max_tokens: Option<usize>,
    /// 是否请求流式输出
    pub streaming: bool,
}

/// 补全结果 — `ModelProvider::complete` 的返回值(单批最小返回单元)
///
/// 为保持能力 seam 最小化,统一返回结构化结果;流式场景由上层基于 `streaming`
/// 逐段调用并聚合 `chunks`,此处仅承载单段文本与元信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionResult {
    /// 提供此响应的 provider id
    pub provider_id: String,
    /// 实际使用的下游模型名
    pub model: String,
    /// 生成的文本片段
    pub text: String,
    /// 是否为最后一段(true 表示流式结束/非流式单响应)
    pub done: bool,
}

/// Provider 健康状态 — `ModelProvider::health` 的返回值
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    /// provider id
    pub provider_id: String,
    /// 是否健康可用
    pub healthy: bool,
    /// 单次健康探测延迟(毫秒,0 表示探测未发生)
    pub latency_ms: u64,
}

// ===========================================================================
// ModelProvider trait — 能力 seam
// ===========================================================================

/// 底层模型 provider 抽象 — 所有后端适配器的统一接口
///
/// `Send + Sync` 保证可跨 tokio 任务共享;`async_trait` 允许 trait 内声明 `async fn`。
/// 实现方负责映射到真实下游 API;本 crate 不绑定任何具体厂商。
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// provider 唯一标识(与注册表 key 一致)
    fn id(&self) -> &str;

    /// 本 provider 的能力规格
    fn capabilities(&self) -> ProviderCaps;

    /// 执行一次补全请求(响应批;流式由上层逐段聚合)
    async fn complete(&self, req: &CompletionReq) -> Result<CompletionResult, RouterError>;

    /// 健康探测
    async fn health(&self) -> Health;

    // === 便捷能力查询(基于 capabilities 的快照,非 async,同步即答) ===

    /// 上下文窗口大小
    fn context_size(&self) -> usize {
        self.capabilities().context
    }
    /// 是否支持流式
    fn supports_streaming(&self) -> bool {
        self.capabilities().streaming
    }
    /// 是否支持视觉
    fn supports_vision(&self) -> bool {
        self.capabilities().vision
    }
    /// 是否支持工具
    fn supports_tools(&self) -> bool {
        self.capabilities().tools
    }
}

// ===========================================================================
// ProviderSpec — TOML 注册表条目
// ===========================================================================

/// Provider 注册表条目 — 一份可序列化(TOML/JSON)的 provider 描述
///
/// 由 `reload_from_specs` 配合适配器工厂解析为具体 `ModelProvider`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSpec {
    /// provider 唯一标识(必填)
    pub provider_id: String,
    /// 下游服务端点(如 `https://api.openai.com/v1`)
    #[serde(default)]
    pub endpoint: String,
    /// 模型映射:本系统模型 key → 下游模型名(如 `{"lite": "gpt-4o-mini"}`)
    #[serde(default)]
    pub model_map: HashMap<String, String>,
    /// 能力规格
    #[serde(default)]
    pub caps: ProviderCaps,
}

// ===========================================================================
// ProviderRegistry — 基于 ArcSwap 的 RCU 热更注册表
// ===========================================================================

/// Provider 注册表 — 持有全部已注册 provider 的并发安全快照
///
/// 内部: `ArcSwap<HashMap<String, Arc<dyn ModelProvider>>>`
/// — 表的读/写均为 O(1) 指针操作,写路径(RCU)不需等待读者、不持 guard。
///
/// # Copy 语义
/// `ProviderRegistry` 不含内部可变借用即可 `Clone`(仅 clone ArcSwap 引用),
/// 各句柄共享同一份注册表,跨任务自由传递。
#[derive(Clone)]
pub struct ProviderRegistry {
    table: Arc<ArcSwap<HashMap<String, Arc<dyn ModelProvider>>>>,
}

impl ProviderRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            table: Arc::new(ArcSwap::from_pointee(HashMap::new())),
        }
    }

    /// 以原子方式整体替换注册表(热更核心原语)
    ///
    /// 传入的新表即刻对所有读者可见;旧表在无引用后自动释放。
    #[inline]
    fn store(&self, table: HashMap<String, Arc<dyn ModelProvider>>) {
        self.table.store(Arc::new(table));
    }

    /// 从 spec 列表热更注册表 — 原子换表,失败拒载且不回滚旧表
    ///
    /// `factory(&ProviderSpec) -> Result<Arc<dyn ModelProvider>, RouterError>`
    /// 由调用方提供,负责把一份 spec 解析为具体 provider 适配器(如 OpenAI 客户端、
    /// 本地推理、测试用 Fake 等)。本注册表不内置任何厂商适配器,保持 seam 纯正。
    ///
    /// # 语义
    /// - 先完整构建新表;任一 spec 非法(空 id / 重复 id / factory 失败)即整体失败
    ///   → 返回 `Err`,且**不执行 store**,已存在的旧表原样保留。
    /// - 成功 → 单次 `store` 原子切换,热更即时生效(<1s)。
    pub fn reload_from_specs<F>(
        &self,
        specs: Vec<ProviderSpec>,
        factory: F,
    ) -> Result<usize, RouterError>
    where
        F: Fn(&ProviderSpec) -> Result<Arc<dyn ModelProvider>, RouterError>,
    {
        let mut next = HashMap::with_capacity(specs.len() * 2);
        for spec in specs {
            let id = spec.provider_id.trim();
            if id.is_empty() {
                return Err(RouterError::ConfigError(
                    "provider spec has empty provider_id".into(),
                ));
            }
            // 同表内重复 id:视为配置错误,整体拒载
            if next.contains_key(id) {
                return Err(RouterError::ConfigError(format!(
                    "duplicate provider_id in specs: {id}"
                )));
            }
            let provider: Arc<dyn ModelProvider> = factory(&spec)?;
            if provider.id() != id {
                return Err(RouterError::ConfigError(format!(
                    "factory provider id '{}' != spec provider_id '{id}'",
                    provider.id()
                )));
            }
            next.insert(id.to_string(), provider);
        }
        self.store(next);
        Ok(self.count())
    }

    /// 直接以一组现成 provider 原子换表(便捷入口,不经过 spec 解析)
    pub fn replace(&self, providers: Vec<Arc<dyn ModelProvider>>) -> usize {
        let mut next = HashMap::with_capacity(providers.len() * 2);
        for p in providers {
            next.insert(p.id().to_string(), p);
        }
        self.store(next);
        self.count()
    }

    /// 读取指定 provider — 返回 owned `Arc<dyn ModelProvider>`(不持 guard)
    ///
    /// 通过 `load_full()` 取表快照(owned Arc),clone 出目标 provider 后表引用即刻释放,
    /// 返回的 `Arc` 可安全用于后续 async 调用,不跨 await 持任何 guard(Send 友好)。
    pub fn get(&self, provider_id: &str) -> Option<Arc<dyn ModelProvider>> {
        let table = self.table.load_full();
        table.get(provider_id).cloned()
    }

    /// 是否已注册指定 provider
    pub fn contains(&self, provider_id: &str) -> bool {
        let table = self.table.load();
        table.contains_key(provider_id)
    }

    /// 当前已注册 provider 数量
    pub fn count(&self) -> usize {
        let table = self.table.load();
        table.len()
    }

    /// 列出全部 provider id(无序)
    pub fn ids(&self) -> Vec<String> {
        let table = self.table.load();
        table.keys().cloned().collect()
    }

    /// 列出全部 provider id + 能力规格快照
    pub fn capabilities_map(&self) -> HashMap<String, ProviderCaps> {
        let table = self.table.load_full();
        table
            .iter()
            .map(|(k, v)| (k.clone(), v.capabilities()))
            .collect()
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Fake provider(无网络依赖,测试专用) ---

    struct FakeProvider {
        id: String,
        caps: ProviderCaps,
        healthy: bool,
        reply: String,
    }

    impl FakeProvider {
        fn new(id: &str, caps: ProviderCaps) -> Self {
            FakeProvider {
                id: id.into(),
                caps,
                healthy: true,
                reply: format!("[faked reply from {id}]"),
            }
        }
    }

    #[async_trait]
    impl ModelProvider for FakeProvider {
        fn id(&self) -> &str {
            &self.id
        }
        fn capabilities(&self) -> ProviderCaps {
            self.caps.clone()
        }
        async fn complete(&self, req: &CompletionReq) -> Result<CompletionResult, RouterError> {
            Ok(CompletionResult {
                provider_id: self.id.clone(),
                model: req.model.clone(),
                text: self.reply.clone(),
                done: true,
            })
        }
        async fn health(&self) -> Health {
            Health {
                provider_id: self.id.clone(),
                healthy: self.healthy,
                latency_ms: 5,
            }
        }
    }

    fn spec(pid: &str, ctx: usize, vision: bool) -> ProviderSpec {
        ProviderSpec {
            provider_id: pid.into(),
            endpoint: "https://stub.local/v1".into(),
            model_map: [(pid.to_string(), "stub-model".to_string())]
                .into_iter()
                .collect(),
            caps: ProviderCaps {
                context: ctx,
                vision,
                ..ProviderCaps::default()
            },
        }
    }

    fn ok_factory(spec: &ProviderSpec) -> Result<Arc<dyn ModelProvider>, RouterError> {
        Ok(Arc::new(FakeProvider::new(
            &spec.provider_id,
            spec.caps.clone(),
        )))
    }

    #[tokio::test]
    async fn test_complete_and_health_roundtrip() {
        let reg = ProviderRegistry::new();
        reg.reload_from_specs(vec![spec("p1", 16384, true)], ok_factory)
            .unwrap();
        let p = reg.get("p1").expect("p1 registered");
        let caps = p.capabilities();
        assert_eq!(caps.context, 16384);
        assert!(caps.vision);
        assert!(caps.streaming);
        let res = p
            .complete(&CompletionReq {
                model: "stub-model".into(),
                prompt: "hi".into(),
                max_tokens: Some(16),
                streaming: false,
            })
            .await
            .unwrap();
        assert!(res.done);
        assert!(res.text.contains("p1"));
        let h = p.health().await;
        assert!(h.healthy);
        assert_eq!(h.provider_id, "p1");
    }

    #[test]
    fn test_hot_reload_visibility_change() {
        let reg = ProviderRegistry::new();
        reg.reload_from_specs(vec![spec("alpha", 8192, false)], ok_factory)
            .unwrap();
        assert!(reg.contains("alpha"));

        // 热更:替换为 beta,alpha 立即不可见(原子换表,即时生效)
        reg.reload_from_specs(vec![spec("beta", 32768, true)], ok_factory)
            .unwrap();
        assert!(!reg.contains("alpha"));
        assert!(reg.contains("beta"));
        assert_eq!(reg.count(), 1);
    }

    #[test]
    fn test_error_spec_does_not_destroy_good_table() {
        let reg = ProviderRegistry::new();
        reg.reload_from_specs(vec![spec("stable", 8192, false)], ok_factory)
            .unwrap();

        // 一个坏 spec(空 provider_id)夹在两个好 spec 中间 → 整体拒载
        let mut bad = spec("bad", 4096, false);
        bad.provider_id = "   ".into();
        let result = reg.reload_from_specs(
            vec![spec("new1", 8192, false), bad, spec("new2", 8192, false)],
            ok_factory,
        );
        assert!(matches!(result, Err(RouterError::ConfigError(_))));

        // 旧表原样保留,未被污染
        assert!(reg.contains("stable"));
        assert!(!reg.contains("new1"));
        assert!(!reg.contains("new2"));
        assert_eq!(reg.count(), 1);
    }

    #[test]
    fn test_factory_error_rejects_without_rolling_back() {
        let reg = ProviderRegistry::new();
        reg.reload_from_specs(vec![spec("good", 8192, false)], ok_factory)
            .unwrap();

        // factory 对 "bad" 返回 Err → 整体拒载
        let result = reg.reload_from_specs(
            vec![spec("good", 8192, false), spec("bad", 8192, false)],
            |s| {
                if s.provider_id == "bad" {
                    Err(RouterError::ConfigError("adapter unavailable".into()))
                } else {
                    ok_factory(s)
                }
            },
        );
        assert!(matches!(result, Err(RouterError::ConfigError(_))));
        assert!(reg.contains("good"));
        assert!(!reg.contains("bad"));
    }

    #[test]
    fn test_duplicate_spec_rejected() {
        let reg = ProviderRegistry::new();
        let result = reg.reload_from_specs(
            vec![spec("dup", 8192, false), spec("dup", 8192, false)],
            ok_factory,
        );
        assert!(matches!(result, Err(RouterError::ConfigError(_))));
        assert_eq!(reg.count(), 0);
    }

    #[test]
    fn test_capabilities_map_and_ids() {
        let reg = ProviderRegistry::new();
        reg.reload_from_specs(
            vec![spec("a", 4096, false), spec("b", 65536, true)],
            ok_factory,
        )
        .unwrap();
        let caps = reg.capabilities_map();
        assert_eq!(caps.len(), 2);
        assert_eq!(caps["b"].context, 65536);
        assert!(caps["b"].vision);
        assert!(!caps["a"].vision);
        let mut ids = reg.ids();
        ids.sort();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn test_capability_helper_methods() {
        let reg = ProviderRegistry::new();
        reg.reload_from_specs(vec![spec("v", 32768, true)], ok_factory)
            .unwrap();
        let p = reg.get("v").unwrap();
        assert!(p.supports_vision());
        assert!(p.supports_streaming());
        assert_eq!(p.context_size(), 32768);
    }

    #[test]
    fn test_spec_serde_roundtrip() {
        let s = spec("s1", 16384, false);
        let json = serde_json::to_string(&s).unwrap();
        let de: ProviderSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(de, s);
        assert_eq!(de.provider_id, "s1");
        assert_eq!(de.caps.context, 16384);
    }

    #[test]
    fn test_attention_mode_serde_and_default() {
        assert_eq!(AttentionMode::default(), AttentionMode::Full);
        let json = serde_json::to_string(&AttentionMode::Sparse).unwrap();
        assert_eq!(json, "\"sparse\"");
        let de: AttentionMode = serde_json::from_str("\"local\"").unwrap();
        assert_eq!(de, AttentionMode::Local);
    }

    #[test]
    fn test_replace_direct() {
        let reg = ProviderRegistry::new();
        reg.replace(vec![Arc::new(FakeProvider::new(
            "d1",
            ProviderCaps::default(),
        ))]);
        assert!(reg.contains("d1"));
        assert_eq!(reg.count(), 1);
        let p = reg.get("d1").unwrap();
        assert_eq!(p.id(), "d1");
    }
}
