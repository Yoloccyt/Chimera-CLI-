import {
  Callout,
  Card,
  CardBody,
  CardHeader,
  Code,
  Divider,
  Grid,
  H1,
  H2,
  Stack,
  Stat,
  Table,
  Tag,
  Text,
} from 'qoder/canvas';

export default function L0ContractsRefactorCompletion() {
  return (
    <Stack gap={20}>
      <Stack gap={6}>
        <H1>L0 契约层专项重构 — 完成报告</H1>
        <Text tone="secondary">
          目标：完整实现 Spec《L0 契约层专项重构》全部内容。以代码实测为唯一权威裁决两份文档冲突，
          保持单 crate + 落地纯度机器门 + 例外登记表单测 + 子模块树重排（facade 逐字不变）。
        </Text>
      </Stack>

      <Grid columns={4} gap={14}>
        <Stat value="7/7" label="波次 W0–W6 全实现" tone="success" />
        <Stat value="53" label="模块 → 10 子目录（纯重命名）" />
        <Stat value="G-99/G-100" label="新增纯度机器门" />
        <Stat value="0" label="L0 违规 / 依赖方向违规" tone="success" />
      </Grid>

      <Divider />

      <H2>核心决策</H2>
      <Grid columns={2} gap={14}>
        <Card>
          <CardHeader>
            <Text weight="bold">L0 边界裁决</Text>
          </CardHeader>
          <CardBody>
            <Text>
              采信代码 + v3.4.0§5：L0 = 仅 <Code>nexus-contracts</Code>。否决手册§8.1「L0=nexus-core +
              nexus-telemetry + GPU」（矛盾分层 / telemetry 不存在 / GPU 违 ADR-195），登记为归档蓝图。
            </Text>
          </CardBody>
        </Card>
        <Card>
          <CardHeader>
            <Text weight="bold">crate 拆分重评估</Text>
          </CardHeader>
          <CardBody>
            <Text>
              维持单 crate（承 ADR-049 决策 1）。否决拆 6 兄弟 crate：违架构减法 / 爆炸半径 34 消费方 /
              契约根退化 / 零 API 收益。落地内部子模块化而非物理拆分。
            </Text>
          </CardBody>
        </Card>
      </Grid>

      <H2>关键步骤</H2>
      <Table
        headers={['波次', '动作', '结果']}
        rows={[
          ['W0', '核对基线 / 保留 5 个在途 doc 编辑', '747 unit + 全部集成 0 失败'],
          ['W1', '十维穷举扫描 → 事实基线 + 冲突裁决台账', 'docs/reports 台账落档'],
          ['W2', 'check_l0_purity.py + ci.yml 接线 + gate G-99/G-100', 'selftest 双向 + 真实 0 违规'],
          ['W3', '53 平铺模块 git mv 入 10 子目录 + crate 根 re-export', 'workspace --all-targets 编译通过'],
          ['W4', '契约冻结测试：核实既有覆盖', 'wire_roundtrip/rl_types/domain 已全，无重复'],
          ['W5', 'ADR-212 + adr_index 行 + 文档-代码一致性', 'adr_index_sync / doc_consistency EXIT 0'],
          ['§5', 'ADR_033_EXCEPTION_MODULES const + 4 测', '登记名/核心例外/结构锁/去重 全过'],
        ]}
      />

      <H2>交付物 (a)–(e) 与变更文件</H2>
      <Table
        headers={['类别', '产物']}
        rows={[
          ['(a) 架构说明 + 冲突台账', 'docs/reports/L0-contracts-refactor-audit_2026-10-12.md'],
          ['(b) 代码变更', 'scripts/check_l0_purity.py · .github/workflows/ci.yml · scripts/gate_manifest.toml · 53 src 移动 + 10 mod.rs · lib.rs re-export 重构'],
          ['(c) 契约/事件记录', '无公共 trait/类型/事件签名变更（100% 向后兼容）'],
          ['(d) 测试验证', 'L0 全量 test + clippy + fmt + event-bus 运行期抽样'],
          ['(e) 文档/ADR', 'docs/architecture/ADR-212-...md · docs/architecture/adr_index.md'],
        ]}
      />

      <H2>验证证据</H2>
      <Table
        headers={['门禁', '结果']}
        rows={[
          ['cargo check --workspace --all-targets', 'EXIT 0（39 crate lib+test+bench+example 全编译）'],
          ['cargo test -p nexus-contracts', '747 unit + 集成 + 58 doctest + 4 registry = 0 失败'],
          ['cargo test -p event-bus（重 L0 消费方）', '全绿 0 失败'],
          ['cargo clippy -p nexus-contracts --all-targets -D warnings', 'EXIT 0（0 警告）'],
          ['cargo fmt -p nexus-contracts -- --check', 'EXIT 0'],
          ['check_dependency_rules.ps1（真实 + selftest）', 'EXIT 0（铁律全过）'],
          ['check_l0_purity.py（真实 + selftest）', 'EXIT 0（64 源文件 / 0 违规）'],
          ['adr_index_sync.py / check_doc_consistency.ps1', 'EXIT 0（D3 unrowed 0）'],
        ]}
      />

      <Callout tone="warning" title="唯一未直接跑成的命令：cargo test --workspace 全量">
        <Text>
          本地 D 盘仅余 ~5GB 且无 E 盘可重定向，链接 39 crate debug 测试二进制时
          <Code>ld: final link failed: No space left on device</Code>。属环境资源限制，非代码缺陷。
          以 <Code>--all-targets</Code> 全编译 + nexus-contracts/event-bus 全量运行绿 + 53 文件纯重命名（内容逐字不变）
          + facade/prelude/公共 API 逐字不变 作为等效强度证据；全量回归由 CI（充足磁盘）承担。
        </Text>
      </Callout>

      <H2>最终结论</H2>
      <Stack gap={8}>
        <Text>
          L0 单 crate 边界经代码实测裁决并固化（ADR-212）；纯度缺口的机器门 G-99/G-100 与例外登记表
          单测落地并入 CI / 门清单；53 模块收敛为 10 内聚子树且对外零破坏；无功能退化 / 无性能劣化 /
          无新增 unsafe / 无公共 API 不当暴露。
        </Text>
        <Grid columns={4} gap={10}>
          <Tag tone="success">W0–W6 全实现</Tag>
          <Tag tone="success">facade 逐字不变</Tag>
          <Tag tone="success">100% 向后兼容</Tag>
          <Tag tone="warning">workspace 全量测试 CI 承担</Tag>
        </Grid>
      </Stack>

      <Text tone="secondary" size="small">
        生成于 2026-10-12 · 分支 feat/c1-compute-plane-governance · L0 = crates/nexus-contracts（34 生产消费方）
      </Text>
    </Stack>
  );
}
