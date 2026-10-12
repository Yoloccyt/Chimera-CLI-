# ADR-201: model_view 事件类型分类学 — `turn.submit` 投影时映射 + AppServer 上下文组装消费

- **状态**: ✅ **Accepted（2026-10-07，所有者认证）**；来源 = L0-L3 融合打磨方案 W4-0 T0-3（U-4 裁决「出 ADR 并接线」；承 v3.1 §5-A2 开放项与 v3.2 WI-G11）；施工登记 = W4 批 D WI-G11
- **决策者**: E01（架构）；证据面 = 2026-10-07 现测（`session-store` / `nexus-app-server` 只读勘察）
- **取号证据**: 同 ADR-200（全域双通道 grep `ADR-20[01]` 无实体占用）
- **承袭**: ADR-141（session-store 新建）/ ADR-108/109（CBMR 微批 + k-way 回放）/ ADR-184（wire 契约约束）/ ADR-194 D3（写入敏感扫描）/ v4.0 WI-18（「model-visible means logged」不变量）
- **不触碰**: append-only 段文件权威源；`SENSITIVE_MARKERS` 9 项；`NexusEvent` 变体集（152 锁定）；crate 数（39 锁定）

---

## 1. 背景与判据

### 1.1 现状（2026-10-07 现测）

- `session-store::model_view::to_model_view`（白名单投影 + 敏感剔除，纯函数）已实现 + 测试（7 项），但**生产零消费者**；`crates/session-store/src/lib.rs:48-52` 自述：「`turn.submit` 不在 `MODEL_VISIBLE_TYPES` 白名单内，接线须先对齐事件类型分类学，属消费者侧语义决策」。
- `nexus-app-server::persist_turn`（`server.rs:367-391`）写入事件类型 `"turn.submit"`（payload = turn_id JSON）；白名单现有 5 类为**模型对话域命名**：`model.request` / `model.response` / `tool.request` / `tool.result` / `state.update`。
- 语义张力：`turn.submit` 是**用户回合提交**的编排域类型名，语义上与「模型请求侧输入」等价——两套命名轴需要一次归一裁决，否则「model-visible means logged」不变量无法在生产强制。

### 1.2 三选项评测

| 选项 | 内容 | 判定 |
|---|---|---|
| ① 白名单升格 | `"turn.submit"` 直接加入 `MODEL_VISIBLE_TYPES`（5→6） | ❌ 否决：白名单从此承载两套命名体系（编排域 + 对话域），分类学污染；`turn.*` 家族未来扩展将继续侵蚀白名单 |
| ② 写入侧类型改写 | `persist_turn` 改写为 `"model.request"` | ❌ 否决：append-only 段文件将出现同一语义两种类型名（存量 `turn.submit` 与新增量并存）；改动落盘协议与既有测试/审计口径 |
| ③ 投影时映射 | 存储保持 `"turn.submit"` 原样；`to_model_view` 出口按**显式映射表**归一化输出 | ✅ 采纳（D1） |

判据核心：**append-only 权威源不动**（存储层保持事件原样），语义映射收敛在唯一出口（单一职责 + 唯一出口纪律；本仓曾有 publish 双份实现静默分叉的反向先例）。

---

## 2. 决策

### D1 `turn.submit` 归类 = 模型可见（映射为 `model.request`），映射位置 = 投影时映射（唯一出口）

- `session-store::model_view` 新增显式常量 `MODEL_VISIBLE_MAPPING`（初始 1 条：`("turn.submit", "model.request")`）+ 测试锁；未来新增映射走同表评审。
- `to_model_view` 语义修订：`canonical = mapping.get(type).unwrap_or(type)` → 白名单判定 → 投影输出 `event_type` = **canonical**（映射后规范类型）。既有 5 类的 identity 行为零变化（含既有 7 项测试不动）。
- **不变量保持**：投影只读已落盘事件（`offset` = 段文件 seq 镜像）；append-only 段文件为权威源不动；`SENSITIVE_MARKERS` 9 项不回退；敏感剔除优先于映射（含敏感载荷的 `turn.submit` → 投影 `payload = None`）。

### D2 首个生产消费方 = `AppServer::model_visible_context`

- `nexus-app-server` 新增 `pub async fn model_visible_context(&self, thread_id: &ThreadId) -> Result<Vec<ModelVisibleEvent>, ServerError>`：
  - store 未配置 → `StoreNotConfigured`（与 `persist_turn` 同语义）；经 `CbmrWriter::read_events(session_id, None)`（现有读路径，`run_blocking`/spawn_blocking）读回已落盘事件 → `filter_map(to_model_view)`。
  - 消费语义 = **模型上下文组装源**：宿主（`chimera serve` / `acp`）拼装模型请求上下文时必须取此出口，使「logged ⇒ 不泄漏 / 已泄漏面必已记录」在生产路径可强制；被剔除的敏感载荷仍存储（审计需要），仅不在模型视图出现。
- 接线可达性（现测）：AppServer 由 `chimera-cli::composition::build_app_server`（`composition.rs:249-252`）+ `serve` / `acp` 命令生产装配（WI-01），消费方法进入生产可达面。

### D3 反模式与回滚

- 否决「审计投影与上下文组装各实现一份」：唯一出口原则；两处实现必随时间漂移（本仓双源漂移三例前科）。
- 回滚：删映射表 1 行 + 移除消费方法 = 回到现行为（白名单 5 类 + 零消费者）；存储面零改动（append-only 不动）。

---

## 3. 后果

- **正面**：model-visible 不变量首次有生产出口；`turn.submit` 语义归类获正式裁决；审计出口与模型组装出口共用同一投影（分类学单源）。
- **代价**：`ModelVisibleEvent.event_type` 对 `turn.submit` 输出为 `model.request`（原始类型名仅存于段文件 / 审计出口；如需保真回看走存储面）。
- **性能**：读路径 = spawn_blocking + 纯函数投影，非热路径。

---

## 4. 验收（批 D WI-G11 适用）

| # | 判据 | 验证方式 |
|---|---|---|
| V1 | 映射语义 | `to_model_view`：`turn.submit` → 输出 `model.request`；既有 5 类 identity；非白名单 / 非映射类型仍 `None`（测试锁） |
| V2 | 消费方集成 | 集成测试：`persist_turn` 双写后 `model_visible_context` 输出 = 白名单交集（`turn.submit` 在其中）；`persist_turn` 写入类型与映射表一致性测试 |
| V3 | 不回退 | session-store 既有 61 测试 + nexus-app-server 测试全绿；`SENSITIVE_MARKERS` 9 项断言不动 |
| V4 | 敏感面 | 含 secret 载荷的 `turn.submit` → 投影 `payload = None`（剔除优先于映射） |

---

## 5. 剩余开放项

1. 映射表后续扩展（如 `turn.complete` 等）逐项独立评审（本 ADR 不预授权）。
2. 「审计查询面」消费（`to_model_view` 的第二个消费者）按真实需求另立。
3. `model_view` 对中文 / 混淆键名的敏感覆盖 = 启发式边界（见 `model_view.rs:44-47` 文档），维持现状。

---

- **证据锚点**: `crates/session-store/src/model_view.rs:36-58`（白名单 / 敏感标记）· `:88-105`（`to_model_view`）· `crates/session-store/src/lib.rs:48-52`（遗留开放项原文）· `crates/nexus-app-server/src/server.rs:356-391`（`persist_turn`）· `:291-303`（`with_session_store`）· `crates/session-store/src/writer.rs:386-394`（`read_events`）· `crates/chimera-cli/src/composition.rs:249-252`（组合根装配）

---

## 6. 追加事实注记（2026-10-10 / v3.5 自查；只加注，不改写上方决策原文）

> 触发：全栈终局总账 v3.5 的 Key Change 6 / C-1（P-4）复验。下列四项均为现取事实（A 级）。

1. **D2 的「首个生产消费方」仅在代码 / API 层成立，现行生产链上该消费方不可达**：
   组合根 `chimera-cli/src/composition.rs:251` 走 `AppServer::with_backend(ctx.server_config, backend)`
   ⇒ `store` 字段为 `None`（`nexus-app-server/src/server.rs:283`），于是 `persist_turn` 与
   `model_visible_context` 均直接返回 `ServerError::StoreNotConfigured`。`AppServer::with_session_store`
   （定义在 `server.rs:293`）的两个调用点在 `:920` 与 `:977`，而 `#[cfg(test)]` 区自 `:634` 开始
   ⇒ **全仓无生产态双写**。因此 §4 V2（集成测试双写后输出白名单交集）为真，而 D2 中「宿主拼装
   模型上下文时必须取此出口」在现行生产路径上**暂无可执行载体**（本 ADR 不因此失效，但不得被读成“已投产”）。
2. **不改 CRATE-CONTRACT 机器字段**：`CONSUMERS` 由**非 optional** 依赖边派生、`MATURITY` 由 crate 内
   自陈标签派生（口径见 `scripts/crate_contract.py:8-10`）⇒ 手写改值会被 G-59 拒。**语义变更需新 ADR**。
3. **对上游计划文本的一处否证**：v3.5 Spec W6-3 将收口径 ②（降为 `optional` + 表述改“诊断面”）描述为
   「一个提交闭台账债、零运行期风险」——实测**不成立**：`nexus-app-server` 内 `session_store` 引用共
   **17 处**（均在 `server.rs`），降 optional 需 cfg-gate 生产 API 面（`AppServer` 字段与
   `with_session_store` / `replay_since` 签名），并同时删 `scripts/dep_edge_freeze.txt` 的
   `nexus-app-server|session-store` 行（减方向合法；该文件头注已预写“when it is made optional”）。
4. **按证据把 P-4 拆为两层执行（不是“选个更小的做完”）**：（a）**不需 ADR、本轮已做**＝表述订正：
   `crates/session-store/src/lib.rs` 头部新增「装配态再订正（2026-10-10）」段（原 2026-10-06 注里的
   「replay 已接入」与同文件“生产未启用双写”订正段共存而未被覆盖，属同文件内口径不自洽）；
   总账 §4 行维持 **DORMANT**。（b）**需 ADR**＝依赖降 optional + CONSUMERS/MATURITY 语义变更，
   并须先定义 N-4 存储不可用时的降级姿态（现仅 `Err`，无降级）。
