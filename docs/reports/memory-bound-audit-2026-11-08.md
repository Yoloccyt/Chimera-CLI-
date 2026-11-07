# 内存上界审计（R4-4，2026-11-08）

- **性质**：纯审计报告（零代码）——证实全部常驻缓存有显式上界、无无界增长点
- **方法**：逐缓存现取容量常量/配置字段（file:line 证据），配合 R4-1 分配预算棘轮（`crates/gqep-executor/tests/dispatch_alloc_budget.rs`）与 R1 零分配锁（`crates/seccore/tests/policy_zero_alloc.rs`）

## 1. 常驻缓存上界清单（全部 file:line 现取）

| 缓存 | 上界 | 证据 |
| --- | --- | --- |
| scc-cache 主缓存 | 256 条目（≈256KB 量级假设） | `crates/scc-cache/src/config.rs:18-22` |
| scc-cache semantic_cache | 256 条/命名空间 | `crates/scc-cache/src/semantic_cache.rs:43` |
| CMT Hot / Warm / Cold | 256 / 4096 / 65536（Ice 无限=对象存储语义,盘面非内存） | `crates/cmt-tiering/src/config.rs:22-37` |
| CMT experience card Hot | 256 | `crates/cmt-tiering/src/experience_card_storage.rs:40` |
| HCW 四级窗口 | L0 4096 / L1 32768 / L2 131072 / L3 1048576 token（L3 有效容量 ÷8 稀疏） | `crates/hcw-window/src/config.rs:177-180` |
| session-store 段 | 4096 行/段,微批 64 条/2ms | `crates/session-store/src/types.rs:231-248` |
| event-bus broadcast | 容量 1024（`DEFAULT_CAPACITY`） | `crates/event-bus/src/bus.rs:33` |
| event-bus Critical 旁路 | bounded mpsc（有界） | `bus.rs::is_critical_mpsc_event` 体系 |
| 派生策略缓存（沙箱门） | ≤256 条,键长 ≤128 | `crates/gqep-executor/src/tool_runtime/sandbox_gate.rs:40-43` |
| 批量派发 slots | N=批大小（调用方传入,非常驻）+ 在飞装箱 ≤ bound（R2-1 惰性装载） | `batch.rs` + ADR-261 |
| TUI sysinfo 刷新 | 默认 5000ms（采样非累积） | `crates/chimera-tui/src/panels/sysinfo.rs:47-50` |

**结论：零无界增长点**。全部内存驻留面 = f(配置常量)，LRU 驱逐在界内运转
（CMT Hot→Warm 驱逐 `coordinator.rs:219-241`；scc-cache 自研 LRU
`cache.rs:161-167` 带 Arc strong_count 保护）。

## 2. 分配预算棘轮（R4-1，防回归的结构哨兵）

| 预算点 | 登记上界 | 实测条件 |
| --- | --- | --- |
| 单次派发（allow，含叶子载荷） | 7 次 | R2 后代码态，2026-11-08 实测 |
| execute_batch（N=8 / 默认 limits） | 68 次 | 同上（≈8.5/请求） |

红绿机制自证：注入一次额外分配即红（断言 `used ≤ cap`，只降不升）。
教训登记：两个 `#[tokio::test]` 并行会互相污染全局分配计数（7→8 漂移），
分配测量必须在**单测试串行窗口**内进行。

## 3. 零分配面（R1 已锁定的更强保证）

判决热路径全零分配（deny-miss / allow-hit `*` 规则带参/无参），非规范输入
恰 1 次归一分配——见 `crates/seccore/tests/policy_zero_alloc.rs` 与 ADR-260 §3。

## 4. 分配器现状与评测（R4-2）

系统默认分配器（Windows Heap）+ mimalloc 评测 feature
（`allocator-mimalloc`，不默认）双态 A/B 结论见 ADR-263 §2。

## 5. 全局分配器观测的局限（诚实边界）

CountingAlloc 只计**分配次数/字节数**，不测 RSS；工作集水位需 OS 级采样
（R5 的 `energy_profile.ps1` 附带进程工作集采样，作为运行期参考证据）。
tokio 依赖面为 `features=["full"]`（未裁剪）——影响编译产物而非运行期内存，
不在本审计范围。
