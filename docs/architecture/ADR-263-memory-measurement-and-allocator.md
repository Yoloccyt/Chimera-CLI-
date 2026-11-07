# ADR-263: 内存实测基建 + mimalloc 评测裁决（R4）

- **状态**：Partially Accepted（2026-11-08，`feat/toolchain-parallel-optimization` R4 波次；R4-1/R4-4 采纳，R4-2 **评测未过门、保持非默认**，R4-3 暂缓）
- **上游**：已批准方案「工具链系统性深度优化重构方案」R4 节
- **关联**：ADR-260（零分配锁先例）/ ADR-261（批量惰性装载）/ ADR-241 D5（同轮对照裁决先例）

## 1. D1 · 分配预算棘轮（R4-1 采纳）

`crates/gqep-executor/tests/dispatch_alloc_budget.rs`（CountingAlloc 复用
util_micro.rs 先例；单测试串行窗口防并行污染）：派发热路径堆分配次数上界
**只降不升**——

| 预算点 | 登记上界 | 实测 |
| --- | --- | --- |
| 单次派发（allow，含叶子载荷） | 7 | 2026-11-08 @ R2 后代码态 |
| execute_batch（N=8 / 默认 limits） | 68 | 同上 |

红绿机制：任何改动使分配数上升即红；有意优化下调登记值须在 commit 说明。
配套审计报告 `docs/reports/memory-bound-audit-2026-11-08.md`（R4-4）：全部
常驻缓存有显式上界、**零无界增长点**（逐项 file:line 证据）。
**教训登记**：两个 `#[tokio::test]` 并行共享全局分配计数器会互相污染
（实测 7→8 漂移）——分配测量必须在单测试串行窗口内进行。

## 2. D2 · mimalloc 评测（R4-2：**未过采纳门，保持非默认**）

**脚手架**：workspace `mimalloc = { version = "0.1", default-features = false }`
（optional）+ `gqep-executor` feature `allocator-mimalloc`（cfg
global_allocator 仅 bench 二进制生效，**不默认**）+ `chimera-cli` 侧留待采纳
裁决后接线（本次未接线）。windows-gnu 构建通过（mimalloc-sys 4m06s）。

**同日同机 A/B 实测**（`dispatch_borrowed_chain` 6 测点，criterion
`--save-baseline r4base` / `--baseline r4base`，p<0.05 全部显著）：

| 测点 | change（median） | 达 ≥5% 门？ |
| --- | --- | --- |
| owned_c1 | −4.52% [−5.38, −3.63] | ✗ |
| borrowed_c1 | −4.25% [−5.11, −3.30] | ✗ |
| owned_c4 | −4.35% [−5.47, −3.34] | ✗ |
| **borrowed_c4** | **−7.01% [−8.40, −5.79]** | ✓ |
| owned_c8 | −4.75% [−5.79, −3.71] | ✗ |
| borrowed_c8 | −4.65% [−5.76, −3.35] | ✗ |

**裁决：不设默认**（采纳判据 = median ≥5% **且** 内存峰值不升；六点中仅
1/6 过 5%，内存峰值侧因墙钟门未过而未测）。方向证据真实（全线 −3~7%）
——feature 保留为评测基建，重评估触发：① R1/R2 后分配占比重新成为主导
（分派发预算棘轮读数显著上升）；② 出现高频大分配载荷（大批量/deep copy）
使 mimalloc 的大块分配优势兑现。日志 `tmp/r4_ab1.log`/`tmp/r4_ab2.log`。

**依赖面注记**：optional 依赖仍进 Cargo.lock（默认构建不编译）——mimalloc
族不在 G-86 排除名单（非 GPU 族）；若 `cargo audit` 未来对 mimalloc-sys
报 unmaintained/unsound，直接移除 workspace 条目即可（证据已留档，无回退成本）。

## 3. D3 · R4-3 mas 50agent 实测轨（暂缓）

既有 `50agent_mem_peak` 为公式估算（`mas_benchmark.rs:566` 自陈）。真实
构造轨需在 delegation_fanout bench 注入 CountingAlloc peak 追踪——模式已被
R4-1 验证可复制，但涉及 chimera-mas bench 装配改造；暂缓，触发条件 = MAS
多 Agent 常驻场景进入生产路径时随批实施（复用 R4-1 模式，预计 <1 小时工作量）。

## 4. 回滚

R4-1 测试与 R4-4 报告为纯增量（revert 无涟漪）；mimalloc workspace 条目 +
feature 移除即完全回退（默认构建从未编译它）。
