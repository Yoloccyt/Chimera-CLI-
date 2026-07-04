# 维度 B:并发安全审计报告

## 1. 执行摘要

- **审计日期**: 2026-06-28
- **审计范围**: `d:\Chimera CLI\crates` 下 34 个 crates 的并发模式
- **扫描文件数**: 全量 `.rs` 文件(含 src/ 与 tests/)
- **总体评价**: **良好(需改进)**
  - 项目铁律"锁 guard 不跨 await 点"在绝大多数 crate 中被严格遵守,值得肯定
  - 仍存在 **faae-router** 一处系统性违规,需立即修复
  - **csn-substitutor** 存在一处 DashMap TOCTOU 竞态
  - 多处 fire-and-forget `tokio::spawn` 未管理 JoinHandle,任务生命周期不可控
- **问题数量**: Critical 4 / Major 2 / Minor 3

### 关键结论

| 维度 | 结论 |
|------|------|
| 持锁跨 await | **faae-router 存在 4 处违规**(router.rs + edsb.rs),其余 crate 全部合规 |
| broadcast subscribe 时机 | 全部合规(mcp-mesh / efficiency-monitor / ssra-fusion / csn-substitutor / hcw-window / sesa-router 等均在 spawn 之前同步 subscribe) |
| DashMap 与 async 交互 | **csn-substitutor substitutor.rs 存在 TOCTOU**;其余 DashMap 使用规范(efficiency-monitor alerts.rs 主动快照规避死锁) |
| mpsc channel 配对 | 良好(pvl-layer / gea-activator 测试覆盖 drop(tx) 配对) |
| async fn Send + 'static | 抽查 hcw-window / mlc-engine / cmt-tiering / event-bus 均在文档注释中明确保证 |
| 死锁风险 | **faae-router edsb.rs 存在嵌套读锁链**,理论上当前不会死锁(同序获取),但持锁跨 await 已是事实违规 |

---

## 2. 持锁跨 await 扫描

### 2.1 扫描方法

使用 Grep 全量搜索以下模式,然后读取上下文确认是否跨 await:

- `\.lock\(\)\.await` — `tokio::sync::Mutex` 跨 await
- `\.write\(\)\.await` — `tokio::sync::RwLock` 写锁跨 await
- `\.read\(\)\.await` — `tokio::sync::RwLock` 读锁跨 await
- `entry\(\)\.or_insert` — DashMap 写锁跨 await

注:`std::sync::Mutex`/`RwLock` 的 `.lock()` / `.read()` / `.write()` 不带 `.await`,不会跨挂起点,不在违规范围。

### 2.2 扫描结果概览

- `\.lock\(\)\.await`: **0 处**(项目无 `tokio::sync::Mutex` 跨 await 用法)
- `\.write\(\)\.await`: 集中在 `hcw-window\src\window.rs` 与 `faae-router\src\router.rs`
- `\.read\(\)\.await`: 集中在 `hcw-window\src\window.rs`、`faae-router\src\router.rs`、`faae-router\src\edsb.rs`

### 2.3 合规样例(值得肯定)

**hcw-window\src\window.rs:210-316**(`select_window`):

```rust
let outcome = {
    let mut state = self.state.write().await;   // 持锁
    // ... 同步 compress,不 await ...
    state.current_tier = target_tier;
};                                              // ← 写锁在此释放
// 锁外发布事件(避免持锁 await 导致死锁)
match outcome {
    SelectOutcome::Switched { from, to, reason } => {
        self.publish_window_switched(from, to, reason).await?;  // ← await 在锁外
    }
    ...
}
```

代码注释明确标注"全程持有写锁:读取→压缩→更新 原子化,消除竞态窗口"与"锁外发布事件(避免持锁 await 导致死锁)",完全符合项目铁律。

**hcw-window\src\window.rs:510-550**(`compress_to_capacity`)、**window.rs:617-672**(`apply_sparse_mask_to_state`):同样采用"持锁 → 同步操作 → 释放锁 → await 发布事件"模式,合规。

**hcw-window\src\window.rs:394-441**(`spawn_mask_listener`):

```rust
// 1. 写锁内:更新掩码信息 + 存储 pending_context_mask
{
    let mut guard = state.write().await;
    guard.last_mask_hash = Some(mask_hash);
    // ...
}                                               // ← 写锁在此释放
// 2. 锁已释放 — 调用 apply_pending_mask 消费并应用
if let Err(e) = apply_pending_mask(&state, &event_bus).await { ... }
```

注释明确"必须先释放写锁再调用 apply_sparse_mask(获取独立写锁),否则 RwLock 写锁重入导致死锁",合规。

### 2.4 违规项清单(Critical)

#### B-Crit-1: faae-router\src\router.rs:196-200 — 持读锁跨 `edsb.balance().await`

```rust
// 5. EDSB 均衡(若启用)
let final_tool = if self.config.balance_enabled {
    let registry = self.expert_registry.read().await;           // ← 持读锁
    self.edsb
        .balance(&registry, &routed_tool, &candidates)
        .await                                                    // ← 持锁跨 await!
        .unwrap_or(routed_tool.clone())
} else {
    routed_tool.clone()
};
```

**问题**:`registry`(`RwLockReadGuard`)在 `balance().await` 期间一直持有。`balance` 是 async fn,内部含 await 点。`expert_registry` 是高频读路径,持锁阻塞其他写者(register/unregister/decay)将导致:
- 路由路径与 `spawn_decay_loop` 后台任务互相阻塞
- `register_expert` / `unregister_expert` 写锁等待,反向饿死路由

**修复建议**:克隆 `registry` 快照(或仅克隆 balance 需要的字段)后释放读锁,再调用 `balance`:

```rust
let final_tool = if self.config.balance_enabled {
    // 快照:仅取 balance 需要的 (ToolId, Arc<RwLock<ExpertProfile>>)
    let snapshot: Vec<_> = {
        let registry = self.expert_registry.read().await;
        registry.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    };                                                            // ← 读锁释放
    self.edsb.balance(&snapshot, &routed_tool, &candidates).await
        .unwrap_or(routed_tool.clone())
} else { ... };
```

#### B-Crit-2: faae-router\src\router.rs:207-213 — 嵌套锁 + 持锁跨 await

```rust
// 6. 更新被路由工具的 usage_count 和 last_used_at
{
    let registry = self.expert_registry.read().await;            // ← 第一把锁(读)
    if let Some(profile_arc) = registry.get(&final_tool) {
        let profile = profile_arc.read().await;                  // ← 第二把锁(嵌套读)
        profile.usage_count.fetch_add(1, Ordering::Relaxed);
        let mut last_used = profile.last_used_at.write().await;  // ← 第三把锁(嵌套写,跨 await)
        *last_used = Instant::now();
    }
}
```

**问题**:同时持有 `expert_registry` 读锁 + `ExpertProfile` 读锁 + `last_used_at` 写锁。`last_used_at.write().await` 是 await 点,虽然 `Instant::now()` 同步,但三重嵌套锁大幅增加死锁与超时风险。若另一处以相反顺序(先 `last_used_at.write()` 再 `expert_registry.read()`)获取,即构成 AB-BA 死锁。

**修复建议**:缩小锁粒度,registry 仅用于查找 Arc,取出后立即释放;profile 字段更新改为同步原子操作:

```rust
let profile_arc = {
    let registry = self.expert_registry.read().await;
    registry.get(&final_tool).cloned()                          // ← clone Arc,廉价
};                                                               // ← 读锁释放
if let Some(profile_arc) = profile_arc {
    profile_arc.usage_count.fetch_add(1, Ordering::Relaxed);    // 原子,无需锁
    if let Ok(mut last_used) = profile_arc.last_used_at.try_write() {
        *last_used = Instant::now();
    }                                                            // ← try_write 规避阻塞
}
```

#### B-Crit-3: faae-router\src\edsb.rs:284-304 — `decay_usage_counts` 嵌套读锁跨 await

```rust
pub async fn decay_usage_counts(
    &self,
    profiles: &HashMap<ToolId, Arc<RwLock<ExpertProfile>>>,
) {
    for profile_arc in profiles.values() {
        let profile = profile_arc.read().await;                  // ← 持 profile 读锁
        let raw_count = profile.get_usage_count();
        if raw_count == 0 { continue; }
        let last_used = *profile.last_used_at.read().await;      // ← 持锁跨 await(嵌套)
        let delta_secs = now.duration_since(last_used).as_secs_f64();
        let decay_factor = (-delta_secs / tau).exp();
        let decayed_count = (raw_count as f64 * decay_factor).round() as u64;
        profile.set_usage_count(decayed_count);                  // 原子 store,无需持锁
    }
}
```

**问题**:`profile.read().await` 持锁期间再 `last_used_at.read().await`,持锁跨 await。`get_usage_count` / `set_usage_count` 本身是原子操作,不需要读锁保护;仅 `last_used_at` 需要锁。

**修复建议**:去掉外层 `profile.read().await`,直接原子读计数 + 仅对 `last_used_at` 加锁:

```rust
for profile_arc in profiles.values() {
    let raw_count = profile_arc.get_usage_count();              // 原子读,无需锁
    if raw_count == 0 { continue; }
    let last_used = *profile_arc.last_used_at.read().await;     // 仅这一把锁
    let delta_secs = now.duration_since(last_used).as_secs_f64();
    let decayed_count = (raw_count as f64 * (-delta_secs / tau).exp()).round() as u64;
    profile_arc.set_usage_count(decayed_count);                 // 原子写,无需锁
}
```

#### B-Crit-4: faae-router\src\edsb.rs:319-326 — `spawn_decay_loop` 持读锁跨 `decay_usage_counts().await`

```rust
pub fn spawn_decay_loop(
    self: Arc<Self>,
    profiles: Arc<RwLock<HashMap<ToolId, Arc<RwLock<ExpertProfile>>>>>,
) {
    tokio::spawn(async move {
        let interval = Duration::from_secs(DECAY_INTERVAL_SECS);
        loop {
            tokio::time::sleep(interval).await;
            let registry = profiles.read().await;                // ← 持外层读锁
            self.decay_usage_counts(&registry).await;            // ← 持锁跨 await!内部还有 await
        }
    });
}
```

**问题**:`profiles.read().await` 期间调用 `decay_usage_counts(&registry).await`,而 `decay_usage_counts` 内部还有 `profile_arc.read().await` 与 `last_used_at.read().await` 多个 await 点。整条 await 链都持有外层读锁,导致:
- `register_expert` / `unregister_expert` 写锁被阻塞 5 分钟周期触发时可能瞬间卡死
- 与 B-Crit-1 路由路径读锁互相阻塞,高并发下延迟不可控

**修复建议**:克隆 registry 快照后释放读锁:

```rust
loop {
    tokio::time::sleep(interval).await;
    let snapshot: Vec<Arc<RwLock<ExpertProfile>>> = {
        let registry = profiles.read().await;
        registry.values().cloned().collect()
    };                                                           // ← 读锁释放
    // 后续 decay 仅对 Vec<Arc<...>> 操作,不持有外层锁
    self.decay_usage_counts_snapshot(&snapshot).await;
}
```

---

## 3. tokio::spawn 孤儿任务扫描

### 3.1 扫描结果

全量扫描 `tokio::spawn` 调用,共发现约 90+ 处(含测试与生产代码)。生产代码中 spawn 主要分布在:

| crate | 文件:行 | 用途 | JoinHandle 管理 |
|-------|---------|------|----------------|
| faae-router | edsb.rs:319 | 衰减循环 | **未返回,fire-and-forget** |
| kvbsr-router | router.rs:379 | 自动重平衡 | **未保存,fire-and-forget** |
| efficiency-monitor | lib.rs:226 | 事件订阅循环 | **未返回(返回 Result<()>),fire-and-forget** |
| csn-substitutor | lib.rs:237 | 降级链推进 | 返回 `Option<JoinHandle>` ✓ |
| hcw-window | window.rs:398 | 掩码监听 | 返回 `JoinHandle` ✓ |
| mcp-mesh | mesh.rs:329 | 事件订阅 | 返回 `Option<JoinHandle>` ✓ |
| ssra-fusion | engine.rs:187 | 防御性适配 | 返回 `Option<JoinHandle>` ✓ |
| sesa-router | activation.rs:297 | 共识订阅 | 返回 `Option<JoinHandle>` ✓ |
| parliament | ahirt.rs:531 | 周期探测 | 返回 `JoinHandle` ✓ |
| decb-governor | governor.rs:507 | 溢出监控 | 返回 `JoinHandle` ✓ |

### 3.2 违规项

#### B-Min-1: kvbsr-router\src\router.rs:379-383 — fire-and-forget spawn 未管理 handle

```rust
let router = self.clone();
tokio::spawn(async move {
    if let Err(e) = router.auto_rebalance().await {
        warn!(error = %e, "自动重平衡失败");
    }
});
```

**问题**:JoinHandle 被立即丢弃。任务 panic 会被 tokio 默认打印到 stderr 但无法被上层感知;任务取消只能依赖程序退出。注释虽说明"异步触发重平衡(不阻塞当前路由响应)",但缺少 handle 管理意味着:
- 无法在 shutdown 时优雅停止重平衡任务
- 无法观测任务是否仍在运行
- 任务 panic 后无自动重启机制

**修复建议**:将 handle 存入 `Arc<Mutex<Vec<JoinHandle>>>` 或返回给调用方;或采用 `tokio::task::Builder`(若使用 tokio 1.27+)为任务命名便于监控。

#### B-Min-2: efficiency-monitor\src\lib.rs:226 — `start_event_subscriber` 吞掉 JoinHandle

```rust
pub fn start_event_subscriber(&self) -> Result<(), MonitorError> {
    // ...
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        while let Ok(event) = rx.recv().await { ... }
    });                                                          // ← handle 丢弃
    Ok(())
}
```

**问题**:函数签名返回 `Result<(), MonitorError>`,丢失了 JoinHandle。调用方无法等待订阅任务结束、无法取消任务。相比之下,同项目的 `csn-substitutor::start_degradation_listener` 与 `mcp-mesh::start_event_subscriber` 都返回 `Option<JoinHandle>`,模式不一致。

**修复建议**:改为返回 `Result<tokio::task::JoinHandle<()>, MonitorError>`,与项目其他 start_* 函数对齐。

---

## 4. DashMap 写锁与 async 交互

### 4.1 合规样例(值得肯定)

**efficiency-monitor\src\alerts.rs:83-117**(`AlertRuleEngine::check`):

```rust
// 先收集所有规则快照,避免在循环中持有 DashMap 的借用
// WHY:DashMap::iter() 返回 RefMulti,持有 shard lock;
// 若在循环内向 last_triggered 插入(可能命中同一 shard),会死锁。
let rules_snapshot: Vec<AlertRule> = self.rules.iter().map(|r| r.clone()).collect();

for rule in rules_snapshot {                                     // ← 快照遍历,无锁
    // ...
    self.last_triggered.insert(rule.rule_id.clone(), now);       // ← 仅此处加锁,且不与 iter 冲突
}
```

注释明确说明"避免持有 DashMap 的引用导致死锁",主动快照规避死锁,合规。

**efficiency-monitor\src\collectors.rs:67-72**:

```rust
pub fn record_event(&self, event: &NexusEvent) {
    let type_name = event.type_name();
    // entry().or_insert().modify() 保证原子性
    let mut entry = self.event_counts.entry(type_name).or_insert(0);
    *entry += 1;
    drop(entry);                                                 // ← 显式释放 shard lock
    // ...
}
```

`entry().or_insert()` 原子操作 + 显式 drop 释放锁,合规。

**mlc-engine\src\engine.rs:64-74 + 352-360**:使用 `DashMap<MemoryId, ()>` 的 `entry().or_insert(())` 实现条目级迁移锁,消除 TOCTOU 窗口。注释明确"第一个线程 `entry().or_insert(())` 原子性获取锁,后续同一 MemoryId 的迁移会阻塞在 `entry()` 上",合规。

### 4.2 违规项

#### B-Maj-1: csn-substitutor\src\substitutor.rs:89-118 — `register` 存在 TOCTOU 竞态

```rust
pub fn register(&self, cap: CapabilityDescriptor) -> Result<(), CsnError> {
    // ...
    let key = cap.capability_id.clone();

    // 已存在则覆盖(更新能力描述符)                       ← check
    if self.capabilities.contains_key(&key) {
        self.capabilities.insert(key, cap);                    // ← act
        return Ok(());
    }

    // 新增:检查容量上限                                  ← check
    if self.capabilities.len() >= self.capacity {
        return Err(CsnError::RegistryFull { capacity: self.capacity });
    }

    self.capabilities.insert(key, cap);                         // ← act
    Ok(())
}
```

**问题**:两个 check-then-act 之间没有锁保护。并发场景下:
- 线程 A: `contains_key(K)` 返回 false
- 线程 B: `contains_key(K)` 返回 false
- 线程 A: `len() < capacity` 通过
- 线程 B: `len() < capacity` 通过
- 线程 A: `insert(K, V1)`
- 线程 B: `insert(K, V2)` — 覆盖 V1,且容量计数可能短暂超限

虽然 DashMap `insert` 本身原子,但"检查容量 → 插入"组合操作不是原子的,实际容量可能短暂超过 `self.capacity`。

**修复建议**:使用 `entry().or_insert()` 原子化 check-then-act:

```rust
use dashmap::mapref::entry::Entry;

match self.capabilities.entry(key.clone()) {
    Entry::Occupied(mut o) => { o.insert(cap); Ok(()) }         // 已存在,覆盖
    Entry::Vacant(v) => {
        if self.capabilities.len() >= self.capacity {
            return Err(CsnError::RegistryFull { capacity: self.capacity });
        }
        v.insert(cap);
        Ok(())
    }
}
```

注:`len()` 检查仍存在轻微窗口(entry 持有分片锁时其他分片可能 insert),但 `entry` API 在同一分片锁内完成"判断存在 + 插入",比当前 contains_key + insert 双锁调用安全得多。

### 4.3 csn-substitutor\src\lib.rs:229-232 — Arc::clone 正确使用(合规,值得肯定)

```rust
// WHY Arc::clone 而非 Arc::new(self.chains.clone()):
// 必须共享同一 DashMap 实例,否则后台任务推进降级链的修改不会
// 反映到原始 substitutor(Week 7 Task 2.5 关键 bug 修复)
let chains = Arc::clone(&self.chains);
```

注释明确说明使用 `Arc::clone` 共享同一 DashMap 实例,符合项目历史教训。

---

## 5. broadcast subscribe 时机

### 5.1 扫描结果

全量扫描 `\.subscribe\(\)` 调用(约 90+ 处),核验 subscribe 是否在 `tokio::spawn` 之前同步调用。

### 5.2 合规样例(全部合规)

**mcp-mesh\src\mesh.rs:320-352**(`start_event_subscriber`):

```rust
pub fn start_event_subscriber(&self) -> Option<JoinHandle<()>> {
    let bus = self.event_bus.clone()?;
    // 关键:在 spawn 之前同步订阅,确保不遗漏后续事件
    // WHY: tokio::broadcast 仅投递给发布时已存在的 receiver;
    // 若在 spawn 的 async block 内 subscribe,后台任务调度时机不确定,
    // 可能晚于 publish 导致事件静默丢失(broadcast 不缓存历史给新订阅者)
    let mut rx = bus.subscribe();                               // ← 同步 subscribe
    Some(tokio::spawn(async move {                              // ← 之后 spawn
        while let Ok(event) = rx.recv().await { ... }
    }))
}
```

注释明确引用"Week 6 教训:broadcast 时序",合规。

**efficiency-monitor\src\lib.rs:211-237**、**ssra-fusion\src\fusion\engine.rs:178-202**、**csn-substitutor\src\lib.rs:227-251**、**hcw-window\src\window.rs:394-441**、**sesa-router\src\activation.rs:294-310**:全部采用"subscribe 在 spawn 之前同步调用"模式,合规。

### 5.3 违规项

**无违规**。所有生产代码的 `bus.subscribe()` 调用均在 `tokio::spawn` 之前同步执行。

---

## 6. mpsc channel 配对

### 6.1 扫描结果

mpsc channel 主要用于:
- **pvl-layer**:Producer → Verifier 操作流(`mpsc::channel::<Operation>`)
- **pvl-layer**:Verifier → Producer 反馈流(`mpsc::channel::<FeedbackMessage>`)
- **gea-activator**:测试中的 op_rx / fb_tx(同 pvl-layer 模式)
- **gqep-executor**:FuturesUnordered 流式处理(非 mpsc)

### 6.2 合规样例

**pvl-layer\src\verifier.rs:311-315**(测试代码,体现正确配对模式):

```rust
op_tx.send(make_operation("valid")).await.unwrap();
drop(op_tx);                                                    // ← 显式 drop 发送端
let mut rx = op_rx;
let result = verifier.run(&mut rx, &feedback_tx).await;
assert!(result.is_ok(), "通道关闭应正常返回 Ok");
```

测试代码显式 `drop(op_tx)` 触发 `rx.recv()` 返回 `None`,使 `verifier.run` 正常退出,符合"recv() 返回 None 仅当所有 Sender drop"的项目教训。

**pvl-layer\src\producer.rs:67-76**(`Producer` 结构体文档):

```rust
/// # 背压控制
/// 通道容量由 `PvlConfig::channel_capacity` 控制(默认 128)。
/// 通道满时 `tx.send().await` 会阻塞,形成自然背压,
/// 避免 Producer 淹没 Verifier(对应架构红线:1M Token 暴力加载)
```

文档明确说明背压控制机制,合规。

### 6.3 违规项

**无违规**。mpsc channel 使用规范,显式 `drop(tx)` 模式在测试中正确体现。

---

## 7. async fn 约束(Send + 'static + 'async)

### 7.1 抽查结果

| crate | 文件:行 | 文档声明 | 实际验证 |
|-------|---------|---------|---------|
| event-bus | bus.rs:9 | "所有 async fn 满足 Send 约束,可被 tokio::spawn" | ✓ publish 用 `#[allow(clippy::unused_async)]` 标注,内部同步 |
| hcw-window | window.rs:23 | "所有 async fn 满足 Send + 'static 约束,可被 tokio::spawn" | ✓ spawn_mask_listener 的闭包仅捕获 Arc + EventBus(Clone 廉价) |
| mlc-engine | engine.rs:20 | "所有 async fn 满足 `Send + 'static` 约束,可被 tokio::spawn" | ✓ migrate 闭包捕获 Arc<Self> |
| cmt-tiering | coordinator.rs:57 | "所有 async fn 满足 `Send + 'static` 约束" | ✓ WarmTier/ColdTier 用 spawn_blocking 包装 SQLite |
| decb-governor | governor.rs:523-524 | "state(MutexGuard) 非 Send,不能跨 await 持有;用内层块限制其作用域" | ✓ 明确用 `{}` 块限制 std::sync::Mutex guard |

### 7.2 违规项

**无违规**。抽查的 crate 均在文档注释中明确 Send + 'static 约束,且 `decb-governor` 主动用 `{}` 块规避 `std::sync::MutexGuard` 非 Send 的问题(避免 future 非 Send 导致 spawn 失败),体现深度理解。

---

## 8. 竞态条件识别

### 8.1 已识别竞态

#### B-Maj-1(重复引用,详见 §4.2):csn-substitutor\src\substitutor.rs:89-118

`register` 函数的 `contains_key` + `insert` 与 `len()` + `insert` 两处 check-then-act 无锁保护,并发场景下可能:
- 同一 key 被两个线程同时判定为"不存在",然后都执行 insert(后者覆盖前者,容量计数短暂超限)
- 容量检查通过后另一线程已插入,导致实际容量超过 `self.capacity`

### 8.2 已正确处理的竞态(值得肯定)

**hcw-window\src\window.rs:204-316**(`select_window`):

```rust
/// # 并发安全(SubTask 12.7 修复)
/// WHY:原实现采用"读锁→释放→锁外 compress→写锁覆盖 entries"模式,
/// compress 期间其他线程的 insert 会被写锁覆盖,导致数据丢失(P0 竞态)。
/// 修复方案 A:全程持有写锁,在锁内调用 compress(纯同步函数,不 await,
/// 不会死锁),事件发布在锁外(避免持锁 await)。
```

注释明确标注"P0 竞态"修复历史,采用"全程持锁 + 锁内同步 + 锁外 await"模式,合规。

**kvbsr-router\src\router.rs:276-329**(`route_impl`):

```rust
/// WHY 单一锁消除竞态:原设计 `route` 先读 `blocks` 锁再读 `tool_vectors` 锁,
/// 两次读锁之间可能被 `auto_rebalance` 更新 `blocks` 但未更新 `tool_vectors`,
/// 导致 `route` 读到"新 blocks + 旧 tool_vectors",可能路由到已删除的工具。
/// 合并后,`route` 一次性读取 blocks 快照,tool_vectors 从 DashMap 无锁读,
/// 且 auto_rebalance 不修改 tool_vectors,确保一致性。
```

注释明确说明"两次读锁之间的竞态"修复方案,采用单一读锁 + 锁外无锁读 DashMap,合规。

**cmt-tiering\src\hot.rs:48-54 + 120-145**:

```rust
/// WHY Mutex<()>:DashMap 的分片锁无法保证 `len()` 检查与 `insert()` 之间的
/// 原子性。并发插入时多个线程可能同时通过容量检查,导致超容。
/// 用 `Mutex<()>` 作为粗粒度临界区保护,简单可靠且不引入新依赖。
```

HotTier 用 `Mutex<()>` 保护"检查容量 → 驱逐 → 插入"临界区,正是 csn-substitutor 缺失的模式。建议 csn-substitutor 借鉴此设计。

### 8.3 原子操作内存序

抽查 `Ordering` 使用:
- `Relaxed`:用于计数器(`fetch_add`、`fetch_sub`),无跨线程顺序要求,合规
- `Acquire/Release`:未发现使用,项目无强制内存序场景
- `SeqCst`:未发现使用

整体合规,无内存序错误。

---

## 9. 死锁风险评估

### 9.1 已识别死锁风险

#### B-Crit-2/B-Crit-3(重复引用):faae-router 嵌套锁链

`router.rs:207-213` 持有 `expert_registry.read()` + `profile_arc.read()` + `last_used_at.write()` 三重嵌套锁。虽然当前代码中所有路径都以相同顺序获取(registry → profile → last_used_at),理论上不会 AB-BA 死锁,但:
1. 持锁跨 await 已是事实违规(见 §2.4)
2. 嵌套深度增加未来重构风险(若有人添加反向获取路径即死锁)
3. `last_used_at` 写锁在 `profile` 读锁内获取,若 `spawn_decay_loop` 同时持有 `profile.read()` 并尝试 `last_used_at.read()`,可能与本路径的 `last_used_at.write()` 互锁

### 9.2 已规避的死锁(值得肯定)

**hcw-window\src\window.rs:386-441**(`spawn_mask_listener`):

```rust
/// # 锁重入规避
/// listener 先在写锁内存储 pending_context_mask,释放锁后调用 apply_pending_mask
/// (apply_pending_mask 内部 take pending → 释放锁 → apply_sparse_mask 获取独立写锁)。
/// 若在持锁状态下调用 apply_sparse_mask,会导致 RwLock 写锁重入死锁。
```

注释明确说明"RwLock 写锁重入死锁"规避方案,采用"take pending → 释放锁 → 获取独立写锁"两段锁模式,合规。

**decb-governor\src\governor.rs:523-556**(`spawn_overflow_monitor`):

```rust
// state(MutexGuard) 非 Send,不能跨 await 持有;用内层块限制其作用域,
// 锁在块结束自动释放,await 在块外执行(避免 future 非 Send 导致 spawn 失败)
let (old_tier, new_tier) = {
    let mut state = match tier_state.lock() { ... };
    // ... 同步操作 ...
    (old, suggested_tier)
};                                                              // ← state 在此自动 drop
// 后续 await 在锁外
```

注释明确说明 `MutexGuard` 非 Send 问题,用 `{}` 块限制作用域,合规。

### 9.3 持锁等待 channel 消息 / 持锁等待 task 完成

全量扫描未发现"持锁 → recv().await"或"持锁 → join().await"模式。所有 channel recv 与 task join 均在锁外执行。

---

## 10. 问题清单

| ID | 严重程度 | 问题 | 代码位置 | 修复建议 |
|----|---------|------|---------|---------|
| B-Crit-1 | Critical | 持读锁跨 `edsb.balance().await`,阻塞 register/unregister/decay 写路径 | `crates\faae-router\src\router.rs:196-200` | 克隆 registry 快照后释放读锁,再调用 balance |
| B-Crit-2 | Critical | 三重嵌套锁 + 持锁跨 `last_used_at.write().await`,死锁风险 | `crates\faae-router\src\router.rs:207-213` | 缩小锁粒度:registry 仅查 Arc 后释放;profile 字段改原子操作 |
| B-Crit-3 | Critical | `decay_usage_counts` 嵌套读锁跨 `last_used_at.read().await` | `crates\faae-router\src\edsb.rs:284-304` | 去掉外层 profile.read(),仅对 last_used_at 加锁;计数用原子操作 |
| B-Crit-4 | Critical | `spawn_decay_loop` 持外层读锁跨 `decay_usage_counts().await`(内部多 await) | `crates\faae-router\src\edsb.rs:319-326` | 克隆 registry 快照为 Vec<Arc<...>> 后释放读锁,再 decay |
| B-Maj-1 | Major | `register` 的 `contains_key`+`insert` 与 `len()`+`insert` 两处 TOCTOU 竞态 | `crates\csn-substitutor\src\substitutor.rs:89-118` | 改用 `DashMap::entry().or_insert()` 原子化 check-then-act |
| B-Min-1 | Minor | `tokio::spawn` 自动重平衡未保存 JoinHandle,任务生命周期不可控 | `crates\kvbsr-router\src\router.rs:379-383` | 返回 handle 或存入 `Arc<Mutex<Vec<JoinHandle>>>` 供 shutdown |
| B-Min-2 | Minor | `start_event_subscriber` 吞掉 JoinHandle(签名返回 `Result<()>` 而非 handle) | `crates\efficiency-monitor\src\lib.rs:226` | 改返回 `Result<JoinHandle<()>, MonitorError>`,与项目其他 start_* 对齐 |
| B-Min-3 | Minor | `spawn_decay_loop` 未返回 JoinHandle,fire-and-forget | `crates\faae-router\src\edsb.rs:315-327` | 改为返回 `JoinHandle<()>`,与 `spawn_overflow_monitor` 等同模式 |

---

## 11. 长期主义建议

### 11.1 立即修复(本周内)

1. **faae-router 全链路重构**:B-Crit-1 ~ B-Crit-4 同源于一个设计反模式——"持锁跨 await 访问嵌套 RwLock"。建议一次性重构 `route` / `register_expert` / `unregister_expert` / `decay_usage_counts` / `spawn_decay_loop`,统一采用"快照 → 释放锁 → await"模式。参考 `hcw-window\src\window.rs` 的合规模式。

2. **csn-substitutor register 原子化**:B-Maj-1 修复成本低(改用 `entry()` API),收益高(消除竞态),应立即修复。参考 `mlc-engine\src\engine.rs:352-360` 的 `migration_locks` 模式或 `cmt-tiering\src\hot.rs:120-145` 的 `Mutex<()>` 临界区模式。

### 11.2 中期改进(本月内)

3. **JoinHandle 管理标准化**:项目内 `start_*` 函数返回类型不一致(有 `Option<JoinHandle>`、`JoinHandle`、`Result<()>`、`Result<JoinHandle>` 四种)。建议统一为 `Result<JoinHandle<()>>` 或 `Option<JoinHandle<()>>`,并在调用方集中管理(如 `Arc<Mutex<Vec<JoinHandle>>>` 或 `tokio::task::JoinSet`)。Week 7 MCP Mesh 集成阶段需统一此模式。

4. **引入 clippy::await_holding_lock lint**:在 workspace 级 `Cargo.toml` 或 `clippy.toml` 启用 `await_holding_lock` lint,将持锁跨 await 检测前置到编译期。Rust 1.56+ 已内置此 lint(默认 warn),建议升级为 deny:

   ```toml
   # .cargo/config.toml 或 workspace 级
   [lints.clippy]
   await_holding_lock = "deny"
   ```

### 11.3 长期架构(本季度内)

5. **faae-router 锁结构重新设计**:当前 `Arc<RwLock<HashMap<ToolId, Arc<RwLock<ExpertProfile>>>>` 双层 RwLock + 内层 `last_used_at: RwLock<Instant>` 三层锁结构过于复杂。建议:
   - 外层改 `DashMap<ToolId, Arc<ExpertProfile>>`(分片锁,无锁读)
   - `ExpertProfile` 内部计数用 `AtomicU64`(无锁)
   - `last_used_at` 改 `AtomicU64`(Instant 序列化为 u64 纳秒)
   - 仅在需要"读-改-写"复合操作时用 `DashMap::entry()` 临界区

   此重构可彻底消除 B-Crit-1 ~ B-Crit-4 的根因,与 `cmt-tiering\src\hot.rs` 的 DashMap + AtomicU64 模式一致。

6. **并发测试覆盖率提升**:项目 `tests/concurrent.rs` 已覆盖基础并发场景(10 个 spawn 并发 insert/get)。建议补充:
   - faae-router:并发 route + register + decay 的三向竞态测试
   - csn-substitutor:并发 register 同一 key 的容量超限测试
   - hcw-window:并发 select_window + insert + apply_sparse_mask 的写锁竞争测试

   参考现有 `decb-governor\tests\concurrent.rs`(8 个并发测试场景)与 `parliament\tests\concurrent.rs`(6 个场景)的覆盖深度。

### 11.4 值得肯定的设计模式

项目在以下方面体现了对并发安全的深度理解,应作为团队最佳实践沉淀:

| 模式 | 典型实现 | 推广价值 |
|------|---------|---------|
| 持锁 → 同步操作 → 释放锁 → await | hcw-window select_window / compress_to_capacity | ⭐⭐⭐ 全项目统一 |
| `bus.subscribe()` 在 spawn 之前同步调用 | mcp-mesh / efficiency-monitor / ssra-fusion / csn-substitutor / hcw-window | ⭐⭐⭐ 全项目统一 |
| DashMap `iter()` 后快照遍历,避免持锁 | efficiency-monitor alerts.rs | ⭐⭐⭐ |
| `DashMap::entry().or_insert(())` 实现条目级锁 | mlc-engine migration_locks | ⭐⭐⭐ |
| `Mutex<()>` 临界区保护 check-then-act | cmt-tiering hot.rs | ⭐⭐ |
| `std::sync::Mutex` guard 用 `{}` 块限制作用域 | decb-governor spawn_overflow_monitor | ⭐⭐⭐ |
| `spawn_blocking` 包装 SQLite I/O | cmt-tiering warm.rs / cold.rs | ⭐⭐⭐ |
| 显式 `drop(tx)` 触发 recv 返回 None | pvl-layer verifier.rs 测试 | ⭐⭐ |
| `Arc::clone` 共享同一 DashMap(而非 clone 副本) | csn-substitutor lib.rs | ⭐⭐⭐(项目历史教训) |

---

## 12. 审计边界与免责声明

- 本审计基于静态代码分析,未运行时插桩验证
- 报告中"合规"指代码模式符合项目铁律,不代表运行时无死锁(需动态测试验证)
- `tests/` 目录下的测试代码未纳入违规统计(测试代码允许 fire-and-forget spawn)
- 建议结合 `cargo test --workspace` 与 `cargo clippy --workspace -- -D warnings` 验证修复效果
- 建议在 Week 7 MCP Mesh 集成阶段引入 `loom` 竞态测试框架,对 faae-router 与 csn-substitutor 进行模型化并发测试

---

**审计人**:并发安全审计子代理(GLM-5.2)
**审计日期**: 2026-06-28
**下次复审**: 修复 B-Crit-1 ~ B-Crit-4 后立即复审
