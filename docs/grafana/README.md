# NEXUS-OMEGA Efficiency Monitor — Grafana 仪表盘部署指南

> 对应 Task 5(W7-Carryover-5)的 Grafana 仪表盘配置。
> 本指南说明如何部署 `dashboard.json`,将 efficiency-monitor 的 `/metrics` 端点接入 Prometheus + Grafana 监控栈。

---

## 前置条件

| 组件 | 版本要求 | 说明 |
|------|---------|------|
| Grafana | 10.x+ | 仪表盘 JSON 基于 `schemaVersion: 38`,需 Grafana 10+ 兼容 |
| Prometheus | 2.x+ | 数据采集与存储 |
| efficiency-monitor crate | 已编译 | `/metrics` 端点可用(默认端口 9090) |

环境变量(Windows PowerShell,如需在当前会话生效):

```powershell
$env:CARGO_HOME = 'D:\Chimera CLI\.toolchain\cargo'
$env:RUSTUP_HOME = 'D:\Chimera CLI\.toolchain\rustup'
$env:PATH = "D:\Chimera CLI\.toolchain\cargo\bin;D:\msys64\mingw64\bin;$env:PATH"
```

---

## 步骤 1:启动 efficiency-monitor

在项目根目录执行:

```bash
cargo run -p efficiency-monitor -- --metrics-port 9090
```

启动后 `/metrics` 端点默认监听在 `http://localhost:9090/metrics`,输出 Prometheus 文本格式指标。

**验证端点可用**(PowerShell):

```powershell
Invoke-WebRequest http://localhost:9090/metrics -UseBasicParsing | Select-Object -ExpandProperty Content
```

预期输出包含:

```text
# HELP nexus_event_total Total NexusEvent published by type
# TYPE nexus_event_total counter
nexus_event_total{type="CacheHit"} 5
# HELP nexus_critical_event_total Total Critical NexusEvent published by type
# TYPE nexus_critical_event_total counter
# HELP nexus_alert_triggered_total Total alerts triggered
# TYPE nexus_alert_triggered_total counter
nexus_alert_triggered_total{severity="critical"} 2
```

---

## 步骤 2:配置 Prometheus 数据源

在 Prometheus 配置文件 `prometheus.yml` 中添加 scrape 配置:

```yaml
scrape_configs:
  - job_name: 'nexus-omega'
    scrape_interval: 5s
    metrics_path: /metrics
    static_configs:
      - targets: ['localhost:9090']
```

**重启 Prometheus** 使配置生效:

- systemd:`sudo systemctl restart prometheus`
- Windows:重启 prometheus.exe 进程

**验证 scrape 状态**:访问 `http://localhost:9091/targets`(或 Prometheus 实际地址),确认 `nexus-omega` job 状态为 `UP`。

---

## 步骤 3:在 Grafana 中添加 Prometheus 数据源

1. 打开 Grafana Web UI(默认 `http://localhost:3000`,默认账号 admin/admin)
2. 左侧菜单 → **Configuration** → **Data Sources**
3. 点击 **Add data source** → 选择 **Prometheus**
4. 填写配置:
   - **URL**: `http://localhost:9090`(Prometheus 地址,不是 efficiency-monitor)
   - **Access**: Server(default)
   - 其他保持默认
5. 点击 **Save & Test**,出现绿色 "Data source is working" 提示即成功

---

## 步骤 4:导入仪表盘

1. 左侧菜单 → **Dashboards**
2. 点击 **New** → **Import**
3. 选择 **Upload JSON file**,上传本目录下的 `dashboard.json`
4. 在导入页面:
   - **Dashboard name**: `NEXUS-OMEGA Efficiency Monitor`(自动填充)
   - **Prometheus 数据源**:下拉选择步骤 3 创建的数据源(对应模板变量 `DS_PROMETHEUS`)
5. 点击 **Import**

导入后,仪表盘会自动加载,5 秒刷新一次,默认时间范围 `now-1h` 到 `now`。

---

## 面板说明

仪表盘共 5 个面板,布局如下:

| ID | 面板名称 | 类型 | 查询 | 阈值 / 说明 |
|----|---------|------|------|------------|
| 1 | Nexus Event Total | Time series | `sum by (type) (rate(nexus_event_total[5m]))` | 按事件类型分组(UserIntentEncoded / QuestCreated / ConsensusReached / SkepticVeto / RedTeamAudit / CacheHit / BudgetExceeded 等) |
| 2 | Nexus Alert Triggered Total | Time series | `sum by (severity) (rate(nexus_alert_triggered_total[5m]))` | critical / warning / info 三色;阈值线:critical > 0 时标红 |
| 3 | Nexus Critical Event Total | Stat | `sum(nexus_critical_event_total)` | 绿色(0,健康)/ 红色(>0,存在 Critical 事件);包括 SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded |
| 4 | MCP Mesh Transaction Latency | Time series | `histogram_quantile(0.95, sum by (le) (rate(mcp_mesh_transaction_latency_bucket[5m])))` + p50 | 阈值线 100ms(p95 目标);超过需排查 Mesh 节点负载 |
| 5 | SESA Sparsity Ratio | Gauge | `sesa_sparsity_ratio` | 范围 0-1;绿色(<0.4,达标)/ 黄色(0.4-0.6,警戒)/ 红色(>0.6,稀疏化失效) |

**布局**:

```
┌───────────────────────────┬───────────────────────────┐
│ Panel 1: Event Total      │ Panel 2: Alert Triggered  │  (y=0, h=8)
│           (12w)           │           (12w)           │
├──────────────┬───────────────────────────────────────┤
│ Panel 3:     │ Panel 4: MCP Mesh Latency             │  (y=8, h=8)
│ Critical     │ (16w, p50 + p95)                      │
│ Event (8w)   │                                       │
├───────────────────────────────────────────────────────┤
│ Panel 5: SESA Sparsity Ratio (24w, 全宽 Gauge)        │  (y=16, h=8)
└───────────────────────────────────────────────────────┘
```

### 指标可用性说明

| 指标 | 当前状态 | 来源 crate |
|------|---------|-----------|
| `nexus_event_total` | ✅ 已实现 | efficiency-monitor(`collectors.rs::EventMetricCollector`) |
| `nexus_critical_event_total` | ✅ 已实现 | efficiency-monitor(同上) |
| `nexus_alert_triggered_total` | ✅ 已实现 | efficiency-monitor(同上) |
| `mcp_mesh_transaction_latency_bucket` | ⏳ 待实现 | mcp-mesh(histogram,需 crate 自身暴露 `/metrics`) |
| `sesa_sparsity_ratio` | ⏳ 待实现 | sesa-router(gauge,需 crate 自身暴露 `/metrics`) |

**Panel 4 与 Panel 5 在对应 crate 暴露指标前会显示 "No data"**,属正常现象,不是配置错误。

---

## 故障排查

### 问题 1:所有面板无数据

**排查步骤**:

1. **检查 Prometheus targets 状态**
   - 访问 `http://localhost:9091/targets`(Prometheus 实际地址)
   - 确认 `nexus-omega` job 状态为 `UP`
   - 若 `DOWN`:检查 efficiency-monitor 是否运行、端口是否被占用

2. **检查 efficiency-monitor 进程**
   ```powershell
   Get-Process | Where-Object { $_.ProcessName -like '*efficiency*' }
   ```
   或直接测试端点:
   ```powershell
   Invoke-WebRequest http://localhost:9090/metrics -UseBasicParsing
   ```

3. **检查 Grafana 数据源**
   - Configuration → Data Sources → Prometheus → Save & Test
   - 若失败:检查 Prometheus URL 是否正确、Prometheus 是否运行

### 问题 2:`/metrics` 端点不可达

```powershell
# 测试端点
Invoke-WebRequest http://localhost:9090/metrics -UseBasicParsing

# 检查端口占用
netstat -ano | findstr :9090
```

可能原因:
- efficiency-monitor 未启动 → 重新执行 `cargo run -p efficiency-monitor -- --metrics-port 9090`
- 端口被其他进程占用 → 使用 `--metrics-port 9091` 指定其他端口,同步更新 `prometheus.yml`
- 防火墙拦截 → 放行 9090 端口

### 问题 3:指标名不匹配

若 Prometheus 采集到数据但 Grafana 面板仍无显示,可能是指标名与查询不匹配:

```powershell
# 拉取实际指标名列表
(Invoke-WebRequest http://localhost:9090/metrics -UseBasicParsing).Content -split "`n" |
    Where-Object { $_ -match '^# HELP' }
```

对照 `dashboard.json` 中的 `expr` 字段:
- Panel 1:`nexus_event_total`
- Panel 2:`nexus_alert_triggered_total`
- Panel 3:`nexus_critical_event_total`
- Panel 4:`mcp_mesh_transaction_latency_bucket`
- Panel 5:`sesa_sparsity_ratio`

若实际指标名不同,需同步修改 `dashboard.json` 中对应 `expr`。

### 问题 4:Panel 4/Panel 5 显示 "No data"

**这不是配置错误**。这两个面板对应的指标(`mcp_mesh_transaction_latency_bucket` / `sesa_sparsity_ratio`)尚未在 mcp-mesh / sesa-router crate 中实现。

待对应 crate 暴露 `/metrics` 端点后,需在 `prometheus.yml` 中添加额外 scrape 配置:

```yaml
scrape_configs:
  - job_name: 'nexus-omega'
    scrape_interval: 5s
    static_configs:
      - targets: ['localhost:9090']  # efficiency-monitor
  - job_name: 'mcp-mesh'
    scrape_interval: 5s
    static_configs:
      - targets: ['localhost:9091']  # mcp-mesh metrics 端点
  - job_name: 'sesa-router'
    scrape_interval: 5s
    static_configs:
      - targets: ['localhost:9092']  # sesa-router metrics 端点
```

---

## 相关文件

| 文件 | 说明 |
|------|------|
| `docs/grafana/dashboard.json` | Grafana 仪表盘配置(schemaVersion 38,5 个面板) |
| `docs/grafana/README.md` | 本文档 |
| `crates/efficiency-monitor/src/dashboard.rs` | Prometheus 文本格式渲染实现(顶部 doc comment 引用本目录) |
| `crates/efficiency-monitor/src/collectors.rs` | `EventMetricCollector` 实现,产出三个核心 counter 指标 |

---

## 维护说明

- **修改面板查询**:编辑 `dashboard.json` 中对应 panel 的 `targets[].expr` 字段
- **修改阈值**:编辑 `fieldConfig.defaults.thresholds.steps`
- **修改刷新频率**:修改顶层 `refresh` 字段(默认 `5s`)
- **修改时间范围**:修改顶层 `time.from` / `time.to`(默认 `now-1h` / `now`)
- **升级 schemaVersion**:Grafana 升级后,可在 UI 中打开仪表盘 → Settings → Save,自动迁移到新 schema,再导出覆盖 `dashboard.json`

每次修改 `dashboard.json` 后,建议在 Grafana UI 中重新 Import 验证,或用 `Get-Content dashboard.json | ConvertFrom-Json`(PowerShell)/ `python -m json.tool dashboard.json`(Python)校验 JSON 格式。
