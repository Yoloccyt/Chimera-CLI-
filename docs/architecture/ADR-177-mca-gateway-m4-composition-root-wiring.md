# ADR-177: mca-gateway M4 组合根装配实施(特征门控)

- **状态**: Accepted（2026-09-03，架构重构批次2 Task 2 实施落地）
- **决策者**: 架构批次 E01 提案；用户批准批次 spec 后终裁
- **关联**:
  - ADR-065（MCA 总纲，决策 6：编译期 feature `mca` 双轨隔离 + M4 独立 PR 装配路径）
  - ADR-171 T8（mca-gateway 空特性装配口径登记，M4 列 planned independent PR）
  - ADR-160（可达性棘轮；freeze.txt 只收缩）
  - `Cargo.toml`（根 features/workspace deps）、`crates/chimera-cli/{Cargo.toml,src/main.rs}`、`scripts/crate_reachability_freeze.txt`

> **编号说明**: 承接 ADR-176(批次1)顺延为 **ADR-177**,与 ADR-178/179/180 构成批次2 治理连续编号。

## 背景

ADR-171 T8 将 mca-gateway 的装配路径定位为"M4 独立 PR":`chimera-cli` 声明 `optional` 依赖 + `"mca"` feature 链 + 组合根调用。批次2 探索确认所有前置齐备(根 feature 空档、workspace deps 已声明、`McaGateway::new` API 现成、ci.yml mca job 存在),实施完成。

## 决策(实际实施形态)

1. **Feature 传递设计**:CI(`ci.yml:266/288`)以 workspace 级 `cargo check --workspace --all-targets --features mca` 调用;Cargo feature resolver 语义会**天然激活每个定义同名 feature 的 member**。故:
   - 根 `[features] mca = []` **保持空**(仅注释扩大说明传递意图),不新增转发链;
   - `crates/chimera-cli/Cargo.toml`:`mca-gateway = { workspace = true, optional = true }` + `[features] mca = ["dep:mca-gateway"]`(L10 同层,与 mcp-mesh 先例同构)。CI 现有命令即可双轨验证,零 CI 改动。
2. **组合根装配**:`chimera-cli/src/main.rs` `run()` 在 config 加载后、`commands::dispatch` 前,`#[cfg(feature = "mca")]` 块内调用 `init_mca_gateway()`:`McaGateway::new(McaGatewayConfig::default())` 构造 + `spec_count()==0` 装配态自检 + `tracing::info!`;失败仅 `warn!` 不 panic。纯装配态:零网络请求、零连线、零 spec 注册。默认(无 feature)路径 cfg 剔除,零符号。
3. **测试**:main.rs 门控模块两点断言——`#[cfg(feature="mca")]` 可构造不 panic;`#[cfg(not(...))]` 断言 feature 门关。
4. **freeze.txt**:删除 mca-gateway 冻结行,原位留 REMOVED 注释(2026-09-03 T2);孤岛 15→14。
5. **语义说明**:因 optional 边不计入 BFS,`reachable` 计数保持 28,但 mca-gateway 由"冻结孤岛"重分类为 **GATED(特征门控装配面)**,不再触发棘轮告警。

## 否决项

- **否决根 feature 显式转发链**(`mca = ["chimera-cli/mca"]`):与 CI workspace 级调用的 cargo 语义重复,增加维护面,收益为零。
- **否决默认启用 mca-gateway**:保持 ADR-065 双轨隔离(默认 binary 不含 mca 面),避免体积/行为变化;真实请求通路(transport 连线)不在本 ADR 范围。

## 后果

- **正向**: mca-gateway 首次获得生产可装配路径(特征门控);mca CI job 语义从"仅独立编译"升级为"装配态验证";后续真实通道激活(注册 spec/transport 连线)可直接在配置好的装配点展开。
- **无负向**: 默认 binary 行为/体积不变;freeze 只收缩;new_gaps=0。

## 验证(实测 2026-09-03)

- [x] `cargo check --workspace` EXIT=0(默认,mca 不引入)
- [x] `cargo check --workspace --all-targets --features mca` EXIT=0(5m15s,无警告)
- [x] `scripts/check_crate_reachability.py`:`[OK] reachable=28 frozen=14 new_gaps=0`,mca-gateway 移出 [FREEZE] 转 [GATED]
- [x] 默认测试全绿 EXIT=0(唯一 sandbox 受限用例为既有 DB 落盘测试写真实用户路径,非本批;--skip 后 108/0)
- [x] `--features mca` 门控测试 ok;clippy 默认+mca EXIT=0;fmt EXIT=0
- [x] Cargo.lock 仅 +mca-gateway 一条;用户 dirty 文件未触碰(证据:git status 复核)