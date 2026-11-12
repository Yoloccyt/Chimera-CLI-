# ADR-288: HCW 版本化锁外压缩——SubTask 12.7 P0 竞态的正式解法

- 状态: Accepted
- 日期: 2026-11-12
- 批次: R8 工具链优化（上下文/记忆层）
- 关联: ADR-192（mlc L2 分层检索）、SubTask 12.7（hcw-window window.rs 并发安全历史修复）、R8 执行报告 `docs/reports/toolchain-r8-context-memory-execution-2026-11-12.md`

## 背景

hcw-window 的降级/L3 溢出压缩（`select_window` / `compress_to_capacity`）需要执行 O(n×512) 全量评分 + Top-K + rayon 批调度。SubTask 12.7 之前的实现采用「读锁快照→锁外 compress→写锁覆盖 entries」模式，被判定为 **P0 竞态**：压缩基于旧快照，写锁内盲目覆盖会丢失压缩期间并发 insert 的新条目。当时的修复方案是**全程持有写锁**——正确但代价高昂：压缩期间所有读者（get/get_arc/snapshot）阻塞，且需要 block_in_place 双分支避免占死 worker。

## 决策

以 `HcwState.mutations` 版本计数重建「锁外压缩」，三阶段执行：

1. **读锁快照**：`state.entries.clone()`（`Vec<Arc<ContextEntry>>` 的 Arc 引用计数拷贝，O(n) 指针复制，非 content 深拷贝）+ 读取 `mutations` 版本号。
2. **锁外压缩**：`ContextCompressor::compress` 在无锁状态运行（保留 MultiThread `block_in_place` / current_thread 直调双分支，消除占死 worker 与 reader 饿死）。
3. **写锁合并**：比较 `state.mutations != version`——
   - 无变更：retained 即完整结果，直接 `replace_entries`；
   - 有变更：`merge_compress_with_snapshot` 差集合并——retained 过滤已删除条目（以 state 现状为准）∪ 并入快照后新增条目（12.7 丢数据路径被封死）；合并后**重验容量**，超容走既有「拒绝降级/WindowOverflow」分支。

### 关键设计点

- **bump 点完备性**：`push_entry` / `remove` / `retain_by_file_ids`(仅真删减时) / `replace_entries` 四个结构性变更点统一 bump。`replace_entries` 是新增封装（赋值 + rebuild_index + invalidate_repr_clv_cache + bump 四合一），防止调用方漂移造成假阴性。
- **假阳性无害原则**：版本校验宁假阳性不假阴性——多余的合并分支结果等价，漏检的合并丢数据。
- **serde(skip) 兼容**：`mutations` 为纯运行时计数器，反序列化从 0 起算；版本校验只依赖单调性，起点无关，旧 checkpoint 数据零迁移。
- **from tier 重读**：Switch/Compressed 事件的 `from` 字段在写锁执行时重读 `state.current_tier`（读锁与写锁之间 tier 可能被并发 select_window 改变），事件载荷反映真实前值。

## 后果

- 正向：压缩期间读者不再阻塞；写锁临界区从 O(n×512) 评分+Top-K+rayon 调度降为 O(n) Arc 快照/差集合并；`compress_to_capacity` 同构受益。
- 风险与缓解：并发双 select_window 语义为 last-writer-wins（与全程写锁时代的串行语义等价）；合并分支由 5 个新单测覆盖（无变更/并发新增/并发删除/版本 bump 语义/serde 兼容），既有 12.7 并发安全网测试（`tests/window.rs::test_select_window_concurrent_safety`、`tests/concurrent.rs`）全绿。
- 回滚：revert 本 ADR 对应提交即恢复全程持锁形态，`mutations` 字段 serde skip 不阻碍回滚。

## 验收

- `cargo test -p hcw-window` 全 target 0 failed（含并发安全网）；
- `cargo clippy -p hcw-window --all-targets -- -D warnings` 干净；
- `compress_100k_to_32k` bench 8.26µs（压缩本体无回归）；window_select ns 级无回归。
