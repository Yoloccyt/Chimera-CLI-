//! CHTC 桥接器 — 整合协议转换、适配器分发与 EventBus 集成
//!
//! 对应架构:L10 Interface → L1 EventBus(跨层解耦)
//!
//! # 架构铁律 §2.2
//! CHTC 位于 L10,不直接调用下层路由/执行组件。工具调用到达后,
//! 通过 EventBus 发布 `ChtcToolCallReceived` 事件,下层(L6/L7)订阅消费。
//! 这是 L10→下层通信的唯一合法路径。

use crate::config::ChtcConfig;
use crate::error::ChtcError;
use crate::protocol::ProtocolConverter;
use crate::registry::IdeAdapterRegistry;
use crate::types::{IdeSource, ToolCallResult, UnifiedToolCall};
use event_bus::{EventBus, EventMetadata, NexusEvent};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// receive 入口载荷大小上限(1MB)——防止恶意 IDE 注入超大 JSON 耗尽内存
const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

/// receive 入口 JSON 嵌套深度上限(32 层)——防止深层嵌套触发解析栈溢出
const MAX_PAYLOAD_DEPTH: usize = 32;

/// CHTC 桥接器 — 跨 IDE 工具调用的统一入口
pub struct ChtcBridge {
    /// 桥接配置(支持的 IDE、超时、并发上限)
    config: ChtcConfig,
    /// 协议转换器(无状态)
    converter: ProtocolConverter,
    /// 可选事件总线,用于向下层广播工具调用事件
    event_bus: Option<EventBus>,
    /// 并发限流信号量——限制同时在途的 execute 调用数
    semaphore: Arc<Semaphore>,
    /// IDE 适配器注册中心——支持运行时注册新 IDE
    registry: Arc<IdeAdapterRegistry>,
}

impl ChtcBridge {
    /// 创建桥接器(不接入 EventBus,仅做协议转换与本地执行)
    pub fn new(config: ChtcConfig) -> Self {
        let max_concurrent = config.max_concurrent_calls;
        Self {
            config,
            converter: ProtocolConverter::new(),
            event_bus: None,
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            registry: Arc::new(IdeAdapterRegistry::new()),
        }
    }

    /// 创建桥接器并接入 EventBus,工具调用将广播给下层订阅者
    pub fn with_event_bus(config: ChtcConfig, bus: EventBus) -> Self {
        let max_concurrent = config.max_concurrent_calls;
        Self {
            config,
            converter: ProtocolConverter::new(),
            event_bus: Some(bus),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            registry: Arc::new(IdeAdapterRegistry::new()),
        }
    }

    /// 获取配置引用
    pub fn config(&self) -> &ChtcConfig {
        &self.config
    }

    /// 获取 IDE 适配器注册中心引用(支持运行时注册新 IDE)
    pub fn registry(&self) -> &Arc<IdeAdapterRegistry> {
        &self.registry
    }

    /// 接收原生工具调用,归一化为 UnifiedToolCall 并广播事件
    ///
    /// 步骤:
    /// 1. 校验 ide_source 是否受支持
    /// 2. 校验 raw_call 大小(≤1MB)与 JSON 嵌套深度(≤32 层)
    /// 3. 协议转换为 UnifiedToolCall
    /// 4. 通过 EventBus 发布 `ChtcToolCallReceived`(若已接入)
    ///
    /// WHY 用 `publish_blocking`:`receive` 是同步方法(适配 IDE 同步回调),
    /// 无法 await;`publish_blocking` 内部为 broadcast::send,不阻塞。
    pub fn receive(
        &self,
        raw_call: serde_json::Value,
        ide_source: IdeSource,
    ) -> Result<UnifiedToolCall, ChtcError> {
        if !self.config.is_supported(&ide_source) {
            return Err(ChtcError::UnsupportedIde {
                ide: ide_source.as_str().into(),
            });
        }

        // 系统边界校验:载荷大小(防止超大 JSON 耗尽内存)
        let payload_size = serde_json::to_vec(&raw_call).map(|b| b.len()).unwrap_or(0);
        if payload_size > MAX_PAYLOAD_BYTES {
            return Err(ChtcError::PayloadTooLarge {
                size: payload_size,
                limit: MAX_PAYLOAD_BYTES,
            });
        }

        // 系统边界校验:JSON 嵌套深度(防止深层嵌套栈溢出)
        let depth = json_depth(&raw_call);
        if depth > MAX_PAYLOAD_DEPTH {
            return Err(ChtcError::PayloadDepthExceeded {
                depth,
                limit: MAX_PAYLOAD_DEPTH,
            });
        }

        let call = self.converter.receive(raw_call, ide_source)?;
        if let Some(bus) = &self.event_bus {
            let parameters_hash = sha256_hex(&call.parameters);
            let event = NexusEvent::ChtcToolCallReceived {
                metadata: EventMetadata::new("chtc-bridge"),
                call_id: call.call_id.clone(),
                tool_id: call.tool_id.clone(),
                ide_source: call.ide_source.as_str().to_string(),
                parameters_hash,
            };
            bus.publish_blocking(event)
                .map_err(|e| ChtcError::ProtocolError {
                    reason: format!("event publish: {e}"),
                })?;
        }
        Ok(call)
    }

    /// 执行工具调用 — 根据 ide_source 选择适配器,加 timeout 与并发限流
    ///
    /// # 并发限流
    /// 使用 `tokio::sync::Semaphore` 限制同时在途的 execute 调用数
    /// (上限 = `config.max_concurrent_calls`,默认 32)。
    ///
    /// # 超时保护
    /// 使用 `tokio::select! { biased; delay, future }` 包装适配器执行,超时返回 `CallTimeout`。
    ///
    /// WHY 用 `biased` select 而非 `tokio::time::timeout`:
    /// `tokio::time::timeout` 内部先 poll future 再 poll delay。当适配器是 sync(mock 阶段)
    /// 且 inner 含 `sleep(0)` 时,sleep(0) 的 wake 让 future 在第二次 poll 中先完成,
    /// delay 永远不会被检查,timeout 无法触发。
    ///
    /// `biased` + delay 优先确保每次 poll 先检查超时:若 delay 已 Ready(超时已到),
    /// 直接返回 `CallTimeout`,不再 poll future。这在 future 与 delay 同时 Ready 时
    /// 优先返回超时错误——对于"操作耗时过长"的语义更准确。
    ///
    /// WHY inner 用 `sleep(Duration::ZERO)` 而非 `yield_now`:
    /// `yield_now` 的 wake 走 task queue(直接调度),绕过 timer driver;
    /// 而 delay 的 `Sleep` 的 wake 走 timer driver。两者不在同一批次处理,
    /// 导致 yield_now 的 wake 先被处理,future 先完成,delay 永远无法触发。
    /// `sleep(Duration::ZERO)` 的 wake 也走 timer driver,与 delay 在同一批次处理。
    /// 当 `call_timeout_ms=0` 时,两个 timer 的 deadline 都是 now,同时 fire,
    /// biased 确保 delay 先被检查 → CallTimeout。
    /// 未来 v3.x MCP Mesh 真实 async adapter 的 IO await 自然提供 yield 点。
    pub async fn execute(&self, call: &UnifiedToolCall) -> Result<ToolCallResult, ChtcError> {
        // 1. 通过 registry 查找适配器(支持运行时注册新 IDE)
        let adapter = self
            .registry
            .create(call.ide_source.as_str())
            .ok_or_else(|| ChtcError::UnsupportedIde {
                ide: call.ide_source.as_str().into(),
            })?;

        // WS-4A: adapter 状态变更点 — 适配器经 registry 解析成功(上线)后发布
        // ChtcAdapterStatus 事件,供 L10 TUI Chtc 面板观察各 IDE 适配器兼容状态。
        // publish_blocking 为同步调用(无 tokio 依赖),发布失败仅告警优雅降级。
        if let Some(bus) = &self.event_bus {
            let adapter_type = call.ide_source.as_str().to_string();
            let status_event = NexusEvent::ChtcAdapterStatus {
                metadata: EventMetadata::new("chtc-bridge"),
                adapter_id: adapter_type.clone(),
                adapter_type,
                compatibility_score: 100,
                recent_requests: vec![(call.tool_id.clone(), 1u32)],
                is_online: true,
            };
            if let Err(e) = bus.publish_blocking(status_event) {
                tracing::warn!(error = %e, "ChtcAdapterStatus 事件发布失败");
            }
        }

        // 2. 获取并发许可(_permit 持有至函数返回,自动释放;下划线前缀抑制 unused warning)
        let _permit =
            self.semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| ChtcError::ProtocolError {
                    reason: "信号量已关闭".into(),
                })?;

        let timeout_ms = self.config.call_timeout_ms;

        // 3. biased select:delay 优先,确保超时检查先于 future poll
        let delay = tokio::time::sleep(Duration::from_millis(timeout_ms));
        tokio::pin!(delay);

        tokio::select! {
            biased;
            _ = &mut delay => Err(ChtcError::CallTimeout {
                call_id: call.call_id.clone(),
                timeout_ms,
            }),
            res = async {
                // sleep(0) 让 inner 首次 poll 返回 Pending,且 wake 走 timer driver
                // (与 delay 同批次),使 biased 能在 timer fire 后优先检查 delay
                tokio::time::sleep(Duration::ZERO).await;
                adapter.execute(call)
            } => res,
        }
    }
}

/// 计算 JSON Value 的 SHA256 十六进制摘要
///
/// WHY:事件 payload 仅携带参数哈希(而非完整参数),避免大对象
/// 经 EventBus 传播;消费者据哈希去重或拉取具体参数。
fn sha256_hex(value: &serde_json::Value) -> String {
    // serde_json::Value 序列化几乎不会失败;失败时哈希空字节,仍是稳定摘要
    let bytes = match serde_json::to_vec(value) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "sha256_hex 序列化失败,使用空字节兜底");
            Vec::new()
        }
    };
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    hex::encode(hasher.finalize())
}

/// 计算 JSON Value 的最大嵌套深度
///
/// 标量(字符串/数字/布尔/null)深度为 1;对象/数组深度 = max(子元素深度) + 1。
/// 用于 receive 入口的深度限制校验,防止恶意深层嵌套触发栈溢出。
fn json_depth(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Object(map) => map.values().map(json_depth).max().unwrap_or(0) + 1,
        serde_json::Value::Array(arr) => arr.iter().map(json_depth).max().unwrap_or(0) + 1,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_vscode_raw() -> serde_json::Value {
        serde_json::json!({ "command": "editor.open", "args": { "file": "/x" } })
    }

    #[test]
    fn test_bridge_receive_without_event_bus() {
        let bridge = ChtcBridge::new(ChtcConfig::default());
        let call = bridge
            .receive(sample_vscode_raw(), IdeSource::vscode())
            .expect("转换失败");
        assert_eq!(call.tool_id, "editor.open");
        assert!(!call.call_id.is_empty());
    }

    #[test]
    fn test_bridge_receive_publishes_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let bridge = ChtcBridge::with_event_bus(ChtcConfig::default(), bus);

        let call = bridge
            .receive(sample_vscode_raw(), IdeSource::vscode())
            .expect("转换失败");

        // 验证事件已发布
        let event = rx.try_recv().expect("接收失败");
        let event = event.expect("应有事件");
        match event {
            NexusEvent::ChtcToolCallReceived {
                call_id,
                tool_id,
                ide_source,
                parameters_hash,
                ..
            } => {
                assert_eq!(call_id, call.call_id);
                assert_eq!(tool_id, "editor.open");
                assert_eq!(ide_source, "vscode");
                assert!(!parameters_hash.is_empty());
            }
            other => panic!("期望 ChtcToolCallReceived, 实际: {other:?}"),
        }
    }

    #[test]
    fn test_bridge_receive_unsupported_ide() {
        // 构造仅支持 VSCode 的配置
        let cfg = ChtcConfig {
            supported_ides: vec![IdeSource::vscode()],
            ..Default::default()
        };
        let bridge = ChtcBridge::new(cfg);
        let err = bridge
            .receive(
                serde_json::json!({ "action": "x", "data": {} }),
                IdeSource::zed(),
            )
            .unwrap_err();
        assert!(matches!(err, ChtcError::UnsupportedIde { .. }));
    }

    #[tokio::test]
    async fn test_bridge_execute_vscode_success() {
        let bridge = ChtcBridge::new(ChtcConfig::default());
        let call = bridge
            .receive(sample_vscode_raw(), IdeSource::vscode())
            .unwrap();
        let result = bridge.execute(&call).await.expect("执行失败");
        assert!(result.success);
        assert_eq!(result.result["ide"], "vscode");
    }

    #[tokio::test]
    async fn test_bridge_execute_intellij_returns_success() {
        let bridge = ChtcBridge::new(ChtcConfig::default());
        let call = bridge
            .receive(
                serde_json::json!({ "action": "a", "params": {} }),
                IdeSource::intellij(),
            )
            .unwrap();
        let result = bridge
            .execute(&call)
            .await
            .expect("IntelliJ execute 应成功");
        assert!(result.success);
        assert_eq!(result.result["ide"], "intellij");
        assert!(result.error.is_none());
    }

    #[test]
    fn test_bridge_event_metadata_source() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let bridge = ChtcBridge::with_event_bus(ChtcConfig::default(), bus);
        let _ = bridge
            .receive(sample_vscode_raw(), IdeSource::vscode())
            .unwrap();
        let event = rx.try_recv().unwrap().unwrap();
        assert_eq!(event.metadata().source, "chtc-bridge");
    }

    // === SubTask 0.6.5: receive 入口大小与深度限制测试 ===

    #[test]
    fn test_bridge_receive_rejects_oversized_payload() {
        let bridge = ChtcBridge::new(ChtcConfig::default());
        // 构造超过 1MB 的 payload(大字符串)
        let big_string = "x".repeat(MAX_PAYLOAD_BYTES + 1);
        let raw = serde_json::json!({ "command": "c", "args": { "data": big_string } });
        let err = bridge.receive(raw, IdeSource::vscode()).unwrap_err();
        assert!(
            matches!(err, ChtcError::PayloadTooLarge { .. }),
            "期望 PayloadTooLarge, 实际: {err:?}"
        );
    }

    #[test]
    fn test_bridge_receive_accepts_payload_at_size_limit() {
        let bridge = ChtcBridge::new(ChtcConfig::default());
        // 构造接近但未超过 1MB 的 payload
        // 序列化后含 JSON 结构开销,字符串本身略小于 1MB
        let safe_string = "x".repeat(MAX_PAYLOAD_BYTES - 100);
        let raw = serde_json::json!({ "command": "c", "args": { "data": safe_string } });
        let result = bridge.receive(raw, IdeSource::vscode());
        assert!(result.is_ok(), "接近上限的 payload 应被接受: {result:?}");
    }

    #[test]
    fn test_bridge_receive_rejects_excessive_depth() {
        let bridge = ChtcBridge::new(ChtcConfig::default());
        // 构造深度超过 32 的嵌套对象
        let mut deep = serde_json::json!({ "leaf": true });
        for _ in 0..(MAX_PAYLOAD_DEPTH + 5) {
            deep = serde_json::json!({ "nested": deep });
        }
        let raw = serde_json::json!({ "command": "c", "args": deep });
        let err = bridge.receive(raw, IdeSource::vscode()).unwrap_err();
        assert!(
            matches!(err, ChtcError::PayloadDepthExceeded { .. }),
            "期望 PayloadDepthExceeded, 实际: {err:?}"
        );
    }

    #[test]
    fn test_bridge_receive_accepts_depth_at_limit() {
        let bridge = ChtcBridge::new(ChtcConfig::default());
        // 构造深度等于 32 的嵌套对象(应被接受)
        // 深度计算:标量=1,每层嵌套 +1
        let mut nested = serde_json::json!("leaf");
        for _ in 0..(MAX_PAYLOAD_DEPTH - 2) {
            nested = serde_json::json!({ "n": nested });
        }
        let raw = serde_json::json!({ "command": "c", "args": nested });
        let result = bridge.receive(raw, IdeSource::vscode());
        assert!(
            result.is_ok(),
            "深度在限制内的 payload 应被接受: {result:?}"
        );
    }

    // === SubTask 0.6.6: execute timeout 与并发限流测试 ===

    #[tokio::test]
    async fn test_bridge_execute_timeout_returns_call_timeout_error() {
        // call_timeout_ms=0 + biased select(delay 优先)+ sleep(0) inner:
        //
        // 执行流程:
        // 1. 首次 poll:delay 分支先 poll → Sleep(0ms) 注册 timer(deadline=now)→ Pending
        //    inner 分支后 poll → sleep(0) 注册 timer(deadline=now)→ Pending
        // 2. timer driver 运行:两个 Sleep(0ms) 均 expired(deadline=now ≤ current_time)
        //    → 两个 timer 同时 fire → delay 和 inner 均 woken
        // 3. 第二次 poll:delay 分支先 poll(biased)→ timer 已 fire → Ready → CallTimeout!
        //
        // 关键:inner 用 sleep(0) 而非 yield_now,确保 wake 走 timer driver(与 delay 同批次),
        // 而非走 task queue(会绕过 timer driver 导致 inner 先完成)。
        let config = ChtcConfig {
            call_timeout_ms: 0,
            ..Default::default()
        };
        let bridge = ChtcBridge::new(config);
        let call = bridge
            .receive(
                serde_json::json!({ "command": "x", "args": {} }),
                IdeSource::vscode(),
            )
            .unwrap();
        let result = bridge.execute(&call).await;
        assert!(
            matches!(result, Err(ChtcError::CallTimeout { timeout_ms: 0, .. })),
            "期望 CallTimeout, 实际: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_bridge_execute_blocks_when_semaphore_exhausted() {
        // 配置 max_concurrent_calls=0,semaphore 无 permit,acquire 永久阻塞
        // 用 tokio::time::timeout 验证 execute 在短时间内未完成(阻塞)
        let config = ChtcConfig {
            max_concurrent_calls: 0,
            call_timeout_ms: 10_000,
            ..Default::default()
        };
        let bridge = ChtcBridge::new(config);
        let call = bridge
            .receive(
                serde_json::json!({ "command": "x", "args": {} }),
                IdeSource::vscode(),
            )
            .unwrap();
        // 50ms 内 execute 应仍在阻塞(等待 permit),返回 Err(Elapsed)
        let result = tokio::time::timeout(Duration::from_millis(50), bridge.execute(&call)).await;
        assert!(
            result.is_err(),
            "execute 应在 semaphore 耗尽时阻塞,实际已完成: {result:?}"
        );
    }

    // === SubTask 0.6.7: IdeAdapterRegistry 集成测试 ===

    #[tokio::test]
    async fn test_bridge_execute_via_registry_registered_ide() {
        // 验证 bridge.execute 通过 registry 查找适配器
        let bridge = ChtcBridge::new(ChtcConfig::default());
        let call = bridge
            .receive(
                serde_json::json!({ "command": "x", "args": {} }),
                IdeSource::vscode(),
            )
            .unwrap();
        let result = bridge.execute(&call).await.expect("执行失败");
        assert!(result.success);
        assert_eq!(result.result["ide"], "vscode");
    }

    #[test]
    fn test_bridge_registry_default_contains_five_builtin_ides() {
        let bridge = ChtcBridge::new(ChtcConfig::default());
        let registry = bridge.registry();
        let list = registry.list();
        assert_eq!(list.len(), 5);
        assert!(list.contains(&"vscode"));
        assert!(list.contains(&"zed"));
    }

    // === json_depth 单元测试 ===

    #[test]
    fn test_json_depth_scalar() {
        assert_eq!(json_depth(&serde_json::json!(42)), 1);
        assert_eq!(json_depth(&serde_json::json!("str")), 1);
        assert_eq!(json_depth(&serde_json::json!(true)), 1);
        assert_eq!(json_depth(&serde_json::Value::Null), 1);
    }

    #[test]
    fn test_json_depth_flat_collection() {
        assert_eq!(json_depth(&serde_json::json!([1, 2, 3])), 2);
        assert_eq!(json_depth(&serde_json::json!({ "a": 1, "b": 2 })), 2);
    }

    #[test]
    fn test_json_depth_nested() {
        // {"a": {"b": [1]}} → depth 4(标量1 → [1]2 → {"b":...}3 → {"a":...}4)
        let v = serde_json::json!({ "a": { "b": [1] } });
        assert_eq!(json_depth(&v), 4);
    }

    // ============================================================
    // WS-4A: ChtcAdapterStatus 幽灵事件生产者(adapter 状态变更点)
    // ============================================================

    #[tokio::test]
    async fn test_bridge_execute_publishes_adapter_status() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let bridge = ChtcBridge::with_event_bus(ChtcConfig::default(), bus);

        let call = bridge
            .receive(sample_vscode_raw(), IdeSource::vscode())
            .expect("转换失败");
        bridge.execute(&call).await.expect("执行失败");

        // 先消费 receive 发布的 ChtcToolCallReceived,再断言收到 ChtcAdapterStatus
        let _ = rx.try_recv();
        let mut found = false;
        while let Ok(Some(event)) = rx.try_recv() {
            if let NexusEvent::ChtcAdapterStatus {
                metadata,
                adapter_id,
                adapter_type,
                compatibility_score,
                recent_requests,
                is_online,
                ..
            } = event
            {
                assert_eq!(metadata.source, "chtc-bridge");
                assert_eq!(adapter_id, "vscode");
                assert_eq!(adapter_type, "vscode");
                assert_eq!(compatibility_score, 100);
                assert!(!recent_requests.is_empty());
                assert!(is_online);
                found = true;
                break;
            }
        }
        assert!(found, "应收到 ChtcAdapterStatus 事件");
    }
}
