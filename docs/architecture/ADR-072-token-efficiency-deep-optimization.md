# ADR-072: Token 效率深度优化（客户端域 7 项 + 服务端 Deployment Profile）

> **状态**: Accepted（2026-08-07，实施中）
> **架构层**: L0 nexus-contracts / L3 scc-cache / L10 mca-gateway / L1 event-bus
> **关联**: ADR-069（Token 效率优化五项，v2.19.1-omega 落地）、ADR-034（装配期注入，非运行时 flag）
> **对应计划**: `.superpowers/sdd/` ultra-plan（mca-token-efficiency）

## 背景

ADR-069 已落地五项客户端优化（稳定前缀 4 层 / 六厂商缓存亲和矩阵 / 512 维语义缓存 / 上下文预算裁剪 / TTG 三档输出治理 + early stop），50 任务基准集达到 SMART 目标（等效输入成本↓63.5%、厂商缓存命中率 60%、语义缓存命中率 20%、输出 token↓20%）。

从分布式系统视角复审发现 4 类系统性缺陷，本 ADR 记录其修复决策：

1. **测量误差**: 估算口径隐式为字节/4（对中文恰好≈0.75 token/字符），但缺少每通道系统偏差修正
2. **缓存失效粒度**: Context Ledger 全量 SHA-256，任一历史消息变化失效全部语义缓存条目
3. **并发去重缺失**: in-flight 期间相同请求重复计费（多 Agent 并行/重试/竞速）
4. **路由反馈缺失**: 粘性权重与预估命中率为静态常量，未消费 CacheHitTracker 真实数据

## 决策（8 项）

### ① 显式化 Token 估算器 + EWMA 动态校准（A）

- 字节宽分类权重显式化（1B:0.25 / 2B:0.5 / 3B:0.75 / 4B:1.0，与既有字节/4 等价，消除隐式巧合）
- 新增 `TokenEstimator`：按 (provider, model) 存储 EWMA 校准系数（α=0.3，钳制 [0.5,1.5]），数据源 = StreamSessionCompleted.input_tokens 与发送内容估算之比
- 落点：`mca-gateway/src/token_estimate.rs`（纯函数 + 校准表分离）；conversation_trim/early_stop/cost 三处切换到统一口径
- 理由：字节宽是 BPE 密度的近似代理，各通道存在词表/分词系统偏差；校准是客户端对"字符统计→BPE"系统误差的最优线性近似

### ② 采样参数入缓存键（E）

- `TokenCacheKey` 新增 `sampling_bucket: u8`（temperature 四桶 × top_p 两桶，`#[serde(default)]` 向后兼容）
- `AffinityRequest` 新增 `sampling: SamplingParams` 与 `output_format: OutputFormat`（`#[serde(default)]`）
- 理由：temperature 不同却命中同一语义缓存会导致创意任务多样性丢失或确定性任务拿到随机响应；采样桶间互不命中

### ③ 观测闭环字段扩展（Phase 0）

- `StreamSessionCompleted` 新增五字段（均为 Option 或 bool）：`trimmed_before_tokens` / `trimmed_after_tokens` / `compressed_ratio` / `early_stop_reason` / `coalesced`
- 决策依据：字段级扩展优先于新增事件变体（35 crate 穷尽 match 波及面：全仓库仅 6 处构造点，消费方全部 `..` 兼容）
- 理由：裁剪/压缩/早停/合并收益必须可观测，否则 SMART 目标不可验证

### ④ in-flight 请求合并（C，Phase 3）

- `mca-gateway/src/coalescing.rs`：DashMap<CoalesceKey, watch::Sender>，语义缓存 miss 后合并并发相同请求为一次厂商调用
- 范围约束：仅非流式 invoke()；失败/超时传播给全部等待者；S9 未授权不合并
- 理由：语义缓存是完成态缓存，in-flight 期间重复请求必然重复计费；单飞合并是分布式系统标准去重模式

### ⑤ 分段 Context Ledger（B，Phase 2）

- 上下文哈希改为"响应决定段"校验：S1=System 提示 + S3=最近 4 条消息 + S4=最近工具结果链；S5=更早历史豁免
- `verify_context_ledger` 签名保留（[u8;32]），内部改为段拼接哈希
- 理由：全量哈希失效粒度太粗，多轮工具调用会话中语义缓存命中率趋近 0；早期历史对响应影响弱，豁免不产生实质正确性风险

### ⑥ 完成度感知早停（F，Phase 4）

- `early_stop.rs` 新增 CompletionDetector：仅当请求显式声明 `output_format`（Json/CodeFence/Markdown）时启用语义完成检测（JSON 括号平衡 / 围栏闭合 / 结构完成 + 4-gram 重复抑制）
- 新增 `StopReason::SemanticComplete`；FreeText 不启用（正确性优先）
- 理由：early stop 上限只省 TTFT 与下游处理，不省已生成 token；语义完成即停是唯一能"真省输出 token"的流式手段

### ⑦ 真实命中率反馈路由（G，Phase 3）

- `CacheAffinityIntegration::estimated_hit_rate` 静态估值由 CacheHitTracker 真实命中率（EWMA 平滑）替代
- 路由决策采用期望成本最小化：E[cost] = (1-hit_rate)×input_price + output_price（整数微元运算）
- 理由：静态估值自认"M0 阶段保守估值"，真实数据已采集却未回流

### ⑧ 服务端不可控域 Deployment Profile（J，Phase 5）

- `profiles/deployment/*.toml`：`[server_params]`（KV 量化/PagedAttention/chunked prefill 等，仅部署附注，Chimera **零解析零执行**）+ `[client_relevant]`（max_model_len→context_window 覆盖、prefix_caching→CacheSupport 覆盖，spec 卡片引用后消费）
- 理由：自部署通道（vLLM 等）的服务端能力必须以下发 Profile 表达，禁止写入客户端代码

## 影响

| 变更 | 波及 |
|------|------|
| TokenCacheKey 六维 | 构造点 3 处 + 测试/基准（全仓库 <15 处，假设已核实） |
| AffinityRequest 两字段 | 构造点 15 处（mca-gateway + nexus-contracts 测试） |
| StreamSessionCompleted 五字段 | 构造点 6 处（生产 2 + 测试 4），消费方 `..` 兼容 |
| token_estimate.rs 新模块 | mca-gateway lib + 3 处估算切换（行为等价） |
| EventBus/能力协商 | 无签名变更 |

## 替代方案（否决）

- **新增语义缓存事件变体**（否决）：35 crate 穷尽 match 波及，字段扩展已覆盖观测需求
- **采样参数并入 system_prompt_hash 盐**（否决，作为假设 2 的降级预案）：键空间污染 L1+L2 哈希语义
- **CJK 字符级加权 0.5**（否决）：与既有字节/4 行为不兼容且 emoji 低估
- **客户端解析 server_params**（否决）：违反"服务端不可控域零代码"边界

## 验证

- 全量回归：cargo test --workspace + clippy -D warnings + fmt --check
- 50 任务基准集对比：docs/mca/benchmark-baseline-2026-08-07.md 为基线，Phase 6 出验收报告
- 每项优化有针对性测试（估算收敛 / 桶互不命中 / 段哈希会话演进 / 50 并发合并计数 / 完成度检测）
