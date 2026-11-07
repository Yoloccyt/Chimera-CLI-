# ADR-255 — MCP 配置诚实化收口（死配置/死字段清除 + 默认 servers 清空 + 宿主 GitHub MCP 认证接线）

- **状态**: Accepted（2026-11-07）
- **关联**: ADR-253（MCP 传输层语义补全）、ADR-245（`mcp call` 显式联网）、ADR-244（装配点收敛）
- **代码**: `crates/nexus-core/src/config.rs`、`crates/chimera-cli/src/config.rs`、`crates/chimera-cli/tests/{config_test.rs,cli.rs,mcp_config_defaults.rs}`、`examples/config.sample.yaml`（派生）
- **外部配置**: `~/.chimera/omega.yaml`、`~/.zcode/cli/config.json`

---

## 1. 背景：分布式配置深审的三个事实（2026-11-07）

对项目层（schema/模板/样例/测试夹具）与宿主层（ZCode `config.json` + 日志取证）的
全量配置审查发现：

1. **默认三条 MCP server 无一可真实调用**（历史默认，四处权威来源持续分发）：
   - `filesystem`（stdio npx）：注册端只读 `url`，stdio 型一律 warn 跳过；
   - `github`（`https://api.github.com/mcp`）：**实测 404**（GitHub 官方 MCP 端点实为
     `api.githubcopilot.com/mcp/`），且 `auth: oauth` 字段有 schema 零消费——注册
     "成功"是假阳性，真实调用被 InProcessClient mock 掩盖（ADR-245 §1 同源问题）；
   - `postgres`（`postgresql://localhost:5432/mcp`）：`localhost` 在 SSRF 域名黑名单，
     每次注册被拒；且 `postgresql://` scheme 与 mesh 的 TCP 帧协议不兼容。
2. **`mcp.mesh` 整段死配置**：`McpMeshConfig`（transports/entanglement）有 schema、
   被模板/样例/用户配置三处 promote，全仓零消费；`MeshConfig`（mcp-mesh）透传链
   不存在（三处硬编码 `MeshConfig::default()`），且 `transports: [stdio, http]` 与
   实现矛盾（HTTP 未启用，`CODE_WIKI.md` §3 mcp-mesh 自注）。
3. **宿主 ZCode 的 `mcp.servers.github` 三重不可用**（日志取证
   `~/.zcode/cli/log/zcode-2026-11-06.jsonl`）：
   - `qoder_url` 为非法键 → 每次配置加载告警 `config_mcp_server_invalid`（45+ 次）；
   - http 条目无任何认证 headers，宿主无 `GITHUB_*` 环境变量、gh CLI 未登录；
   - 实际报错为网络层 `fetch failed`（OAuth 授权与连接双双失败，22s），从未进入
     MCP startup 名单（`serverCount:12` 中无 github）。curl 实证：
     `api.github.com/mcp` → 404；`api.githubcopilot.com/mcp/` → 401（缺认证）。

## 2. 决策

### D1. 默认 `servers` 清空——配置不再撒谎

`default_mcp_servers()` 返回空 `Vec`。理由：分发"注定不可用"的默认条目比空列表
更有害（`mcp list` 的假阳性、"github 在列表里"的错觉）。需要 MCP 服务器的用户在
`~/.chimera/omega.yaml` 显式声明；模板提供注释化示例。

### D2. 死配置/死字段随 schema 一并移除（不留垫片）

- 删除 `McpMeshConfig` struct 与 `McpConfig::mesh` 字段；
- `McpServerConfig` 删除 `command`/`args`（stdio 模式，注册端从不读取）与 `auth`
  （零消费，mesh 无认证透传能力）；
- **读取兼容性**：容器级 `#[serde(default)]` 且无 `deny_unknown_fields`，旧配置
  文件中的 `mesh:`/`command:`/`args:`/`auth:` 键被静默忽略，不报错不炸。

### D3. 模板/样例/用户配置同步为诚实形态

- `omega_yaml_template()` 的 mcp 段：`servers: []` + 注释化 url 型示例 +
  **协议边界声明**（mcp-mesh 当前为私有 2PC TCP 帧协议〔ADR-253 §4〕，不实现
  MCP JSON-RPC、无认证透传，标准 MCP 服务器即便注册成功也无法真实调用）；
- `examples/config.sample.yaml` 由 parity 门重派生（G-68 自测 11 断言 PASS、
  G-69 真树 rc=0，两门均只对拍顶层段名，嵌套变更不破门）；
- 用户 `~/.chimera/omega.yaml` mcp 段同步。

### D4. 宿主 GitHub MCP：修配置结构，认证留 env 占位

`~/.zcode/cli/config.json` 的 `github` 条目：

- 删除非法键 `qoder_url`（消除 config 层告警）；
- 按官方 schema（`type`/`url`/`headers`）补认证头：
  `"Authorization": "Bearer ${GITHUB_PERSONAL_ACCESS_TOKEN}"`。

**WHY env 占位而非直接写入 token**：宿主无任何 GitHub token、gh CLI 未登录，
凭据只能由用户自行提供（GitHub → Settings → Developer settings → PAT，
`gh repo`/`copilot` 相关 scope）；`${VAR}` 插值是宿主 documented 的敏感值形态，
token 不落配置文件。用户设置该环境变量后重开 ZCode 会话即生效。

### D5. 测试契约迁移

- `config_test.rs` / nexus-core 内嵌测试：`servers.is_empty()` 断言翻转；
- 旧 e2e `test_mcp_list_reports_configured_servers`（依赖默认三条）迁移为
  `tests/mcp_config_defaults.rs`：进程内直调 `commands::mcp::execute`，
  ①空默认契约（类型层）②注入 RFC 5737 url 型 server 驱动
  「配置 → 注册 → 列表」链路 ③loopback SSRF 拒绝路径 warn-不-panic 契约。
  WHY 进程内直调：不新增子进程 spawn 点位（Mimosa 门拦截），且与
  mcp-mesh 自身集成测试互补覆盖 cli 侧映射段。

## 3. 否决方案

| 方案 | 否决理由 |
|------|---------|
| A. 把 `github` 条目端点改为 `api.githubcopilot.com/mcp/` 并保留默认 | 端点真实了，但 mesh 无认证透传 + 私有 2PC 协议，**调用仍不可能**——只是更隐蔽的假阳性 |
| B. 保留 mesh 段并接通 `MeshConfig` 透传 | `McpMeshConfig` 与 `MeshConfig` 字段完全不相交，透传需先重新设计映射（独立工程）；在消费方存在前保留 schema 即继续撒谎 |
| C. 给 `McpServerConfig` 补 `deny_unknown_fields` 严格化 | 行为变更波及全部 14 段之外的嵌套键与既有用户配置，超出本批范围；登记为后续建议 |
| D. 恢复 `profiles/deployment/*.toml` 样例（mca-gateway） | 属 mca 渠道亲和域（非 MCP），文档已自我标注"待实现"，不捆绑 |
| E. 禁用宿主 ios-simulator / android-emulator / rust-analyzer-lsp 插件 | 平台/环境不可用属实，但属用户工具偏好（询问未获答复），只登记建议不代做 |

## 4. 后果

**正向**

- 默认配置、模板、样例、用户文件四处一致地诚实：空 servers + 协议边界声明；
- `chimera mcp list` 空默认输出友好提示而非三条幽灵；
- 宿主 GitHub MCP 配置合法（零告警），填入 token 即生效；
- 死字段删除后，"填了 command/auth 就该生效"的错误预期从 schema 层消失。

**代价 / 边界**

- `McpConfig`/`McpServerConfig` schema 变更（serde 忽略多余键，读取兼容，
  反序列化旧文件不炸）；
- 依赖默认三条 server 的测试契约迁移（3 处）；
- 项目侧 MCP 真实调用仍受 ADR-253 §4 协议边界限制（已知边界，非本批范围）。

## 5. 测试证据

```text
cargo test -p nexus-core            # 内嵌测试断言翻转后全绿
cargo test -p chimera-cli           # config_test/config_lazy/cli/mcp_config_defaults 全绿
python scripts/check_config_sample_parity.py --selftest   # G-68: 11 断言 PASS
python scripts/check_config_sample_parity.py              # G-69: 真树 rc=0
```

宿主取证：日志 `config_mcp_server_invalid` 告警根因（`qoder_url`）已随配置修复
消除；重开 ZCode 会话后 github server 应以合法配置进入 startup 名单
（token 未设置时仍将认证失败——这是凭据缺失，非配置缺陷）。

## 6. 回退

还原 `nexus-core/src/config.rs` 的 schema 与默认值 + 重跑 parity 门即可；
`~/.zcode/cli/config.json` 的 github 条目回退为含 `qoder_url` 的原样（或直接删除）。
