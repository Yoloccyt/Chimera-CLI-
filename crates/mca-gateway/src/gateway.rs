//! McaGateway — 通道注册表与网关门面
//!
//! M0 骨架职责:spec 注册/查找/注销(通道注册表)。后续里程碑逐步挂载:
//! - PR-8: transport(HTTP 客户端池)+ VendorAdapter 装配
//! - M1: session 状态守恒 + health 健康探针
//! - M2: capability 能力协商引擎
//!
//! # 读写分离设计(ADR-065 决策 4 附属)
//! - **spec 快照**: `ArcSwap<HashMap>` RCU 原语——路由热路径读 ~5ns,
//!   spec 热更新(affinity.d 重载)是低频写,整表替换语义天然匹配
//! - **健康状态**(M1 挂载): `DashMap<RouteKey, HealthState>` 高频原子写,
//!   与 spec 快照分离,避免健康探针写入打扰 spec 读热点
//!
//! # 锁纪律(C7 红线)
//! ArcSwap `load()` 返回 Guard 仅在同步块内使用,禁止持 Guard 跨 `.await`;
//! 本模块所有公开方法均为同步方法,天然合规。

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use nexus_contracts::affinity::ModelAffinitySpec;

/// 网关配置 — M0 骨架仅承载注册表容量提示,后续里程碑扩展
///
/// WHY 独立 config 类型而非裸参数: 对齐 §4.2 模块组织模式(config 独立),
/// M1 将追加 transport 超时/重试/熔断参数,提前锚定扩展位。
#[derive(Debug, Clone)]
pub struct McaGatewayConfig {
    /// 通道注册表初始容量提示(七厂商 × 平均 2-3 模型 ≈ 20)
    pub registry_capacity_hint: usize,
}

impl Default for McaGatewayConfig {
    fn default() -> Self {
        Self {
            registry_capacity_hint: 20,
        }
    }
}

/// 多通道亲和网关 — 通道注册/查找门面
///
/// Clone 廉价(内部 Arc 共享),可在任务间自由传递(对齐 EventBus 惯例)。
#[derive(Clone)]
pub struct McaGateway {
    /// spec 注册表快照 — key 为路由键 `provider/model`
    ///
    /// WHY `Arc<ModelAffinitySpec>` 值: 查找返回克隆 Arc(refcount)而非
    /// 深拷贝 spec(含 Vec/Box<str> 多字段),路由热路径零深拷贝。
    specs: Arc<ArcSwap<HashMap<String, Arc<ModelAffinitySpec>>>>,
    /// 配置(M0 仅容量提示;保留供 M1 transport 装配读取)
    config: McaGatewayConfig,
}

impl McaGateway {
    /// 创建空注册表的网关
    pub fn new(config: McaGatewayConfig) -> Self {
        Self {
            specs: Arc::new(ArcSwap::from_pointee(HashMap::with_capacity(
                config.registry_capacity_hint,
            ))),
            config,
        }
    }

    /// 注册(或覆盖)一张模型亲和描述符,返回其路由键
    ///
    /// WHY RCU 整表替换: spec 注册是低频操作(启动加载 + 热更新),
    /// 复制整表 O(n)(n≈20)可忽略;换来读侧无锁 ~5ns(`load` 原子指针)。
    pub fn register_spec(&self, spec: ModelAffinitySpec) -> String {
        let key = spec.route_key();
        let spec = Arc::new(spec);
        self.specs.rcu(|current| {
            let mut next: HashMap<String, Arc<ModelAffinitySpec>> = (**current).clone();
            next.insert(key.clone(), Arc::clone(&spec));
            next
        });
        tracing::debug!(route_key = %key, "mca spec registered");
        key
    }

    /// 按路由键查找描述符(路由热路径,无锁读)
    pub fn lookup_spec(&self, route_key: &str) -> Option<Arc<ModelAffinitySpec>> {
        self.specs.load().get(route_key).cloned()
    }

    /// 注销描述符(通道下线/spec 卡片移除),返回是否存在
    pub fn unregister_spec(&self, route_key: &str) -> bool {
        let mut removed = false;
        self.specs.rcu(|current| {
            let mut next: HashMap<String, Arc<ModelAffinitySpec>> = (**current).clone();
            removed = next.remove(route_key).is_some();
            next
        });
        if removed {
            tracing::debug!(route_key = %route_key, "mca spec unregistered");
        }
        removed
    }

    /// 当前注册的全部路由键(诊断/TUI 通道面板用)
    pub fn route_keys(&self) -> Vec<String> {
        self.specs.load().keys().cloned().collect()
    }

    /// 已注册通道数
    pub fn spec_count(&self) -> usize {
        self.specs.load().len()
    }

    /// 网关配置只读访问
    pub fn config(&self) -> &McaGatewayConfig {
        &self.config
    }
}

impl std::fmt::Debug for McaGateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McaGateway")
            .field("spec_count", &self.spec_count())
            .field("config", &self.config)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::affinity::{ProtocolDialect, ProviderId};

    fn make_gateway() -> McaGateway {
        McaGateway::new(McaGatewayConfig::default())
    }

    fn make_spec(provider: ProviderId, model: &str) -> ModelAffinitySpec {
        ModelAffinitySpec::minimal(provider, model, ProtocolDialect::OpenAiChat)
    }

    #[test]
    fn register_and_lookup() {
        let gw = make_gateway();
        let key = gw.register_spec(make_spec(ProviderId::Zhipu, "glm-5.2"));
        assert_eq!(key, "zhipu/glm-5.2");
        let found = gw.lookup_spec(&key).expect("registered spec must be found");
        assert_eq!(found.model.as_ref(), "glm-5.2");
        assert_eq!(gw.spec_count(), 1);
    }

    #[test]
    fn lookup_missing_returns_none() {
        let gw = make_gateway();
        assert!(gw.lookup_spec("zhipu/not-registered").is_none());
    }

    #[test]
    fn register_overwrites_same_route_key() {
        // 同路由键重复注册 = spec 热更新(affinity.d 重载语义)
        let gw = make_gateway();
        let mut updated = make_spec(ProviderId::Zhipu, "glm-5.2");
        gw.register_spec(updated.clone());
        updated.capabilities.tool_calling = true;
        gw.register_spec(updated);
        assert_eq!(gw.spec_count(), 1);
        let found = gw.lookup_spec("zhipu/glm-5.2").unwrap();
        assert!(found.capabilities.tool_calling, "热更新后应读到新能力集");
    }

    #[test]
    fn unregister_removes_spec() {
        let gw = make_gateway();
        let key = gw.register_spec(make_spec(ProviderId::DeepSeek, "deepseek-v4-flash"));
        assert!(gw.unregister_spec(&key));
        assert!(!gw.unregister_spec(&key), "重复注销返回 false");
        assert!(gw.lookup_spec(&key).is_none());
    }

    #[test]
    fn clone_shares_registry() {
        // WHY: 网关 Clone 语义必须是共享(Arc)而非快照,否则多持有方
        // 看到的注册表会分叉(csn-substitutor 教训: Arc::new(x.clone()) 独立副本)
        let gw = make_gateway();
        let gw2 = gw.clone();
        gw.register_spec(make_spec(ProviderId::MiniMax, "MiniMax-M3"));
        assert_eq!(gw2.spec_count(), 1, "克隆体必须看到同一注册表");
    }

    #[test]
    fn route_keys_lists_all() {
        let gw = make_gateway();
        gw.register_spec(make_spec(ProviderId::Zhipu, "glm-5.2"));
        gw.register_spec(make_spec(ProviderId::StepFun, "step-3.5-flash-2603"));
        let mut keys = gw.route_keys();
        keys.sort();
        assert_eq!(keys, vec!["step_fun/step-3.5-flash-2603", "zhipu/glm-5.2"]);
    }
}
