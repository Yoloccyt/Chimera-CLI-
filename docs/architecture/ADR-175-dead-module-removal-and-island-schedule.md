# ADR-175: 死模块删除收编 与 冻结孤岛偿还批次推进计划

- **状态**: Accepted（2026-09-03，方向 D 死模块与孤岛处置终裁;执行已完成部分含代码删除）
- **决策者**: 架构批次 E01 提案；用户批准批次 spec 后终裁
- **关联**:
  - ADR-160（可达性棘轮;freeze.txt 只减不增）
  - ADR-171（治理登记;T11-b1 批次登记）
  - `scripts/crate_reachability_freeze.txt`
  - `scripts/bench_inventory_freeze.txt`
  - `docs/reports/arch-direction-D-dead-modules-verify-2026-09-03.md`（复核证据）

> **编号说明**: 承接 ADR-174 顺延为 **ADR-175**。内含两节:4 死模块删除收编(已完成,代码改动)+ 孤岛偿还推进计划(登记,不实施接线)。

## 节 1:模块级死模块删除(执行完成)

对 freeze.txt 模块级登记的 4 项死模块(R3 F-R3-02 证据)执行删除,遵循"先验证零生产消费者→删除→回归":

| 死模块 | 预处理动作 | 配套删除 | 验证 |
|--------|-----------|---------|------|
| osa-coordinator::cheap_index | 删 src/cheap_index.rs(488 行) | lib.rs `pub mod`+`pub use` 声明;nexus-contracts util.rs 例外注释改处理(crate 批注) | cargo check/test 绿 |
| hcw-window::CompressionPipeline | 删 src/pipeline.rs(1267 行) | lib.rs `pub mod pipeline`;tests/double_run.rs;benches/csc_bench.rs + sparse_compare.rs;Cargo.toml `[[bench]]` 两段;bench_inventory_freeze.txt 两行 | cargo check/test 绿 |
| hcw-window::compressor::trim_to_budget | 删函数(约 131 行)+ doc | use xts_top_k_by 导入随删(compute_importance_score 保留,parallel/selector_learner 共享) | cargo check/test 绿 |
| osa-coordinator::skill_plan | 删 src/skill_plan.rs(359 行) | lib.rs 声明;tests/l6_skill_loading_test.rs;repo-wiki 注释引用更新 | cargo check/test 绿 |

freeze.txt 模块级登记段按"只收缩"规则改写为 RETIRED 说明;bench_inventory_freeze.txt 同步删两行。**棘轮行数净减,`check_crate_reachability.sh` 应保持 EXIT=0(无新增报警)。**

## 节 2:冻结孤岛偿还推进计划(登记,不接线)

针对 ADR-171 T11-b1 WIP 集合(此前"no scheduled date"悬置),本条给**下一步动作 + 责任批次**:

| 孤岛 | 下一步动作 | 责任/时机 |
|------|-----------|----------|
| mas-sched(ADR-145) | 接入前置:审查 L0 契约占用面;若 chimera-mas 拆分面未稳,出延期 ADR | 批次 D-2(方向 A 装配面理解后) |
| nexus-hook(ADR-146) | 同上;生命周期钩子为空配置即现状,延期 ADR 候选 | 批次 D-2 |
| mtpe-executor | 消费方定位(chimera-mas 或 gqep);接线评估 | 批次 D-2 |
| csn-substitutor | 能力降级链接线评估 | 批次 D-2 |
| mca-gateway | **M4 独立 PR**(ADR-065 决策 6;组合根 optional dep + "mca" feature 链 + 组合根调用) | 批次 D-3(独立 PR,见 ADR-171 T8) |
| 其余(acb/decb/lsct/chtc/gea/sesa/ssra/scc/auto-dpo/model-router) | 与方向 F 厚度决策联动复核(adb 合并评估可能改变处置) | 批次 D-4(在 ADR-176 F 决策后) |

本 ADR 只登记计划,不实施接线;任何接线在另行 ADR 或独立 PR 中落地,落地后按棘轮规则删 freeze.txt 对应行。

## 否决项

- **否决在节 1 执行前补"失败测试"**:4 死模块零生产消费者(R3/R7+2026-09-03 复核),补失败测试无行为可断言;以 grep 零残留 + cargo check/test 回归作为等价验证(协议豁免,证据见 arch-direction-D 报告)。
- **否决节 1 之外的任何孤岛除名**:孤岛偿还=接线方生效,登记不等于除名。

## 后果

- **正向**: 4 个 dead-module 债务清偿(约 2500 行删除);棘轮净化;bench 清单同步。
- **正向**: 孤岛 WIP 从"no scheduled date"升级为带责任批次的计划。
- **无负向**: 相关 crate 全绿(osa-coordinator/hcw-window check+test EXIT=0)。

## 验证

- [x] `arch-direction-D-dead-modules-verify-2026-09-03.md` 已落地
- [x] cargo check -p osa-coordinator -p hcw-window -p nexus-contracts --all-targets EXIT=0
- [x] cargo test -p osa-coordinator -p hcw-window EXIT=0(0 failed)
- [x] 全仓 grep(排除 target)生产残留核查:仅 mca-gateway(reackage 独立同名函数,无 hcm 耦合)与已更新注释,无死模块符号残留
- [x] freeze.txt/bench_inventory_freeze.txt 已按只收缩同步