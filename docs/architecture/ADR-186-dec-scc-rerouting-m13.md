# ADR-186: M13 decb/scc 重路由偿还批次（decb→efficiency-monitor 生产边 / scc→chimera-cli 组合根）

- **状态**: ✅ **Accepted（批次执行 2026-09-17；追认补录 2026-10-07）**——W4-0 T0-5 / GAP-2；原权威记录 = `CODE_WIKI.md` 顶部批注与 §3.11「M13 批次注记」（物理文件留待补录）；本文件为**编号落位补录**，不新增决策、不改写权威注记
- **编号沿革**: 186 原为 M13（decb/scc 重路由）与 Q3（内环界）双占；2026-09-24 Q3 定档 **ADR-187**（`ADR-187-inner-ring-check-a-retirement.md` 已落盘，G-38/G-39 `spec_ref` 已同步）⇒ **186 归 M13 保留**（本文件即其物理落位）
- **决策者**: E01（架构）；本批次为 M10「被依赖铁律驳回两岛」的 M13 重路由偿还
- **承袭**: ADR-160（可达性棘轮）/ ADR-161（孤岛偿还路径①：接组合根）/ ADR-175（批次机制）/ ADR-185（M12 三岛链先例）/ M11 wiring 决策报告（重路由候选方向）
- **不触碰**: RL 闸门 / R2 冻结（ADR-042/053）；本批次零新增事件变体（其时 145，现数以 `types.rs` 与 `check_doc_drift.sh` 自报为准）

---

## 1. 背景

M10 时两岛原计划接线被依赖铁律驳回（内环保守规则）：

- `decb-governor` 原计划经 **parliament** 转正 → parliament 属内环 9 crate 白名单，decb 不在白名单亦非 L0/L1 基座（Check A 违规；M7「同层允许」裁定遗漏内环保守规则）；
- `scc-cache` 原计划经 **hcw-window** 转正 → hcw 同属内环，且 L2→L3 为向上依赖（Check B 违规；「L2→L3 向下合法」系方向误判）。

M11 wiring 决策报告给出重路由候选方向（decb → efficiency-monitor；scc → chimera-cli 组合根，ADR-161 路径①）；**M13 完成消费者决策论证与接线**（2026-09-17）。

---

## 2. 决策（逐字承 `CODE_WIKI.md` §3.11 M13 批次注记，2026-10-07 核验）

- **D1 decb 消费者 = `efficiency-monitor::RuntimeAuditor`（第 6 维 `budget_discipline`）**：efficiency-monitor 新增 decb-governor 生产依赖（L9→L8 向下、非内环，Check A/B 双绿）。RuntimeAuditor 是 chimera-cli 生产域唯一已接线 efficiency-monitor 组件（experience_loop 组合根装配，60s 周期 `generate_report`），故为唯一真实调用路径；每次报告经 `DecbGovernor::get_stats()` 采样预算快照，维度分 = 1 − 利用率（预算头寸健康度），未注入时中性 0.5（证据纪律同口径）。`HarnessReportGenerated` / `AssessmentUpdated` 事件面分毫不动，该批未新增变体；chimera-cli experience_loop 构造共享 DecbGovernor 注入（`with_decb_governor`），DECB 零消耗不发布事件，装配本身零行为变化。
- **D2 scc 消费者 = chimera-cli 组合根 `AppContext.scc` + `doctor` 第 9 探针**：按 ADR-161 路径①组合根装配 `SccCache`（`SccConfig::default` 容量 256，零 IO 轻构造，与 bus 同生命周期）；doctor 新增 `scc_cache` 探针（索引 8，尾部追加）每次运行真实消费装配实例 `stats()` 只读快照（非探针内 ephemeral 自建——那不构成对装配面的真实消费）。**否决**备选 chat REPL 上下文缓存（缓存无人真实读 = 制造需求，CacheHit/Miss 事件侵入用户面）。探针数 8→9 为唯一对外可见变化（既有 8 探针内容/顺序/状态零变化；cli.rs/help.rs 陈旧「6 维度」文案顺带订正为 9）。
- **D3 棘轮同步**: freeze REMOVED 注记 + 可达性棘轮实测 `reachable=32 / frozen=9 / new_gaps=0`（M13 时点；现势口径以 `check_crate_reachability.sh` 自报为准）。

---

## 3. 后果与边界

- **正面**：冻结孤岛 11→9；生产可达 30→32；两岛获得**真实生产调用路径**（非名义接线：探针消费装配实例快照 / 第 6 维真实采样）。
- **边界**：本 ADR 不触 RL 闸门 / R2 冻结；`efficiency-monitor→decb-governor` 与 `chimera-cli→scc-cache` 两条依赖边的合法性以当时 Check A/B + 边冻结机制为准（ADR-187 定序后环分区为 NOT-ENFORCED 设计意图，但两岛均已按当时最严口径偿还）。
- **文档漂移更正（2026-10-07 核验）**：本批次事实的权威记录 = `CODE_WIKI.md:5`（顶部批注）+ `CODE_WIKI.md` §3.11（M13 批次注记，含 D1/D2 全文论证）；本文件为编号落位补录，凡与权威注记冲突处以注记为准。

---

## 4. 验收（回填）

- `reachable=32 / frozen=9 / new_gaps=0`（`check_crate_reachability.sh`，M13 时点实测）；
- doctor 探针 9（既有 8 探针内容/顺序/状态零变化）；
- RuntimeAuditor 第 6 维 `budget_discipline` 有真实数据路径（`get_stats` 生产消费）。

---

- **证据锚点**: `docs/architecture/CODE_WIKI.md:5`（顶部批注）· `docs/architecture/CODE_WIKI.md:742-754`（§3.11 M13 批次注记）· `scripts/crate_reachability_freeze.txt` · `docs/architecture/ADR-161-island-repayment-roadmap-and-batch-ratchet.md` · `docs/architecture/ADR-187-inner-ring-check-a-retirement.md`
