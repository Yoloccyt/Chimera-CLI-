# ADR-132: CausalGraph 归因管道（CausalGraph × W5 因果归因联动）

> 日期：2026-08-27（补建落档，D-P2 裁决）｜ 状态：实施先行待批（同 ADR-135 先例）
> 关联：ADR-033（VectorClock 豁免圈）/ ADR-153（Phase 4 治理）/ Ch12 W23「CausalGraph×W5 归因联动，任何 diff 可在 5s 窗口内归因到事件链」

## 1. 背景与问题

手册 Ch12 W23 行引用本 ADR，但文件此前缺失（Phase 1-3 未落档）。代码侧因果基座已存在：

- `event-bus::causal::VectorClock`（Lamport 时空时钟，BTreeMap 有序，偏序/并发判定，**71 测试**，P2-W7.2.1 落地）
- `event-bus::causal::CausalBuffer`（乱序缓冲重排，P2-W7.2.2）
- `event-bus::shadow_diff::ShadowDiffRecorder`（P4-T4，双跑 diff 采集）

**缺口**：diff 事件发生后，缺乏「在时间窗口内回溯因果链」的归因管道——双跑出现 diff 时无法自动定位事件链。

## 2. 决策

### 决策 1：归因基座复用 VectorClock，不重造偏序算法
因果判定（happens_before / concurrent / compare）直接使用既有 VectorClock（71 测试基线）。新增 `event-bus::attribution::CausalAttributionLedger` 承载**记录 + 窗口回溯**职责（组合而非重写）。

### 决策 2：归因语义 = 窗口内因果前驱 + 并发观察者
`attribute(diff_fingerprint, now_unix_ms)`：
1. 定位窗口（`[now-5s, now]`，`ATTRIBUTION_WINDOW_MS = 5_000`）内最后一次同指纹记录
2. 收集窗口内与其关系为 `Before`（因）或 `Concurrent`（并发观察者）的记录，按时差升序输出
3. `After`（果）与 `Equal`（自身）排除

### 决策 3：时钟注入 + 环形台账
- `now_unix_ms` 由调用方注入（生产真实时钟 / 测试与长跑加速注入，RK-P20 双轨纪律）
- 台账环形容量 4096（与 shadow_diff MAX_ENTRIES 同量级），长跑不膨胀

### 决策 4：归因失败降级人工（Ch12 原文兑现）
- diff 不在台账（`diff_found=false`）或链为空 → 返回显式结果，**调用方降级人工归因**，不 panic 不猜测
- 归因 P95 > 5s（窗口语义超时）→ 同样降级；T9 以合成链路实测验证（门禁：注入 diff 5s 窗口内归因）

### 决策 5：与 ShadowDiff 分工
| 组件 | 职责 | 落档 |
|---|---|---|
| `shadow_diff::ShadowDiffRecorder` | 双路径指纹序列比对，产出 diff | P4-T4 |
| `attribution::CausalAttributionLedger` | diff 指纹 → 5s 窗口因果链 | 本 ADR（P4-T6） |

两组件经 **事件指纹**（`shadow_diff::event_fingerprint`）解耦：归因输入 = 比对输出的 first_diff 期望指纹。

## 3. 后果

- 正面：diff 归因自动化闭环（比对 → 归因 → 人工兜底三级）；VectorClock 既有投资复用；测试注入时钟使 ≥7 天双跑可在单会话验证
- 代价：发布路径需挂载 `record(fingerprint, clock, at)` 一次调用（O(1) push，环形淘汰）；跨实例 payload 级比对仍需 meta 规范化（登记为局限，诚实数据）
- 否决方案：全量事件图（CausalGraph 全图驻留）——内存不可控且归因查询 O(n²)；窗口 + 时钟偏序已满足 5s 归因语义

## 4. 验收证据（W22）

- `attribution.rs` 9 测试：合成链 A→B→C 归因完整 / 并发入链 / 窗口外排除 / diff 缺失显式 / 果不入链 / 环形淘汰 / 同指纹最近优先 / 单调性 50 组 prop / delta 升序
- T9（W23）以本管道实测「任何 diff 可 5s 内归因」门禁
