# ADR-169: SLO 观察期切换条件、责任人与期限（slo-daily 由 continue-on-error 转阻塞）

* 状态：**Proposed（待安全 + 性能联签）** — 日期 2026-08-31（Phase R R1-T10）
* **编号说明**：初稿编号 ADR-166，落盘时该编号已被 `ADR-166-doc-consistency-gate-execution-plane.md`（同日 R1-T11 产物）占用，故顺延至 **169**（168 已给 ComputeBridge SSOT）。编号权威以磁盘文件名为准，理由见 ADR-168 的"编号说明"。
* 关联：ADR-166（门禁"写了不跑"病灶族——本 ADR 的切移动作正是其镜像问题的解）、ADR-053 rev4 / ADR-054（R2 影子期判据范式）、`docs/reports/phaseR-wave1-closure.md` §5.2、RK-P30 / RK-P34
* 决策者：E08（性能）提案；E02（安全）会签；E01 仲裁
* **责任人**：E08（性能工程）；代理 E01

## 问题

R1-T10 建立了 `slo-daily`（每日 nightly，`CHIMERA_PERF_SCALE=4`，粗筛环境相关/机器依赖抖动）。按纪律"先观察 2 周再定是否升阻塞"，本波次它是 `continue-on-error: true`。

**但"观察两周"若没有可机械判定的口径，就会自动变成第三个 RK-P30 实例**——与 `check_perf_redlines.sh` 登记后仍零 CI 引用、`.sh` 层图滞留 38 导致可达性门禁连续 7 天空转同族：动作做完了，收口条件却悬空，于是永远停在"观察中"。

现在 slo.yml 只有散文承诺（"稳定后转正：删除 `continue-on-error` 即升为阻塞门禁"），而散文不是门禁。

## 决策（三个要素必须全部具体）

1. **判据（可机械复算）**：`slo-daily` 连续 **10 个工作日**满足
   * `flaky_ratio =（retries 后通过的测试数）/（slo-daily 选中测试总数）≤ 5%`，且
   * 非 scale 因素导致的红 **= 0**（"scale 因素"定义：失败断言消息含 `× scale` / `阈值(契约` 字样，即由 CHIMERA_PERF_SCALE 缩放后仍越界的项**不计入本判据**，另交 perf_thresholds.toml 的 [ratchet].calibration_pending 校准债流程（ADR-168 决策 2））。
     计数口径固定为 nextest JSON 的 `type = "test"` 事件中 `kind ∈ {success, output-failure, expired}`；`flaky` = 同一 test id 在 `attempt >= 2` 才 success。
2. **期限**：自 **2026-09-01**（首个 nightly 之后）起算，至迟 **2026-09-19** 做裁决。
   到期未满足判据时，**不得默认延长**，只能三选一：修复抖动源 / 把越界项降级为显式 dev-only（须同步 `ignored_test_inventory_freeze.txt` 棘轮）/ 明文记录"长期非阻塞"及理由。禁止第四种（继续观察）。
3. **动作**：判据满足 → 删除 `.github/workflows/slo.yml` 的 `slo-daily` job 上 `continue-on-error`，并在本 ADR 追加一行"Accepted + 切换日 + 连续 10 日的 flaky_ratio 实测表"。

## 后果

* 正向：观察期有了终点与红绿依据，不再靠人记。
* 正向：判据把"scale 相关失败"与"真抖动"分开——否则阈值未校准期（`calibration_pending` 当前 18 条，上限同为 18，**已零余量**）会把所有校准债误判成抖动，永远切不成阻塞。
* 负向：需要一个统计步骤（从 nextest JSON 算 flaky_ratio）。
  缓解：`slo.yml` 的 Artifact 摘要步已写 `slo_flaky.json`，切阻塞前补一步脚本聚合即可；脚本未写之前，判据不得声称"已可满足"。

## 验证

* [x] 三要素均已具体化（不写"稳定后"这类无法证伪的措辞）
* [x] 与阈值校准债（`[ratchet].max_calibration_pending = 18`，当前 18）显式耦合，避免两套判据互相掩盖
* [ ] flaky_ratio 聚合脚本（切阻塞前置条件，当前未写）
* [ ] 首个 nightly 跑通后回填基线日期
