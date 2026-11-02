# ADR-095~134 落档登记（Phase 1 治理奠基，P1-T2）

> 日期：2026-08-23 ｜ 分支：feat/phase1-w1-w8 ｜ 依据：九源手册 §6.2（ADR 总登记）+ v4.0 总案 WI 映射 + Phase 1 交叉验证记录（research/cross_verification.md）
> 状态说明：ADR-095~134 原为规划态（无文件支撑，D-15 裁决），本文件按项目 ADR 流程正式落档。落档口径：与 v4.0 WI 冲突处按 Phase 1 裁决修正后登记（D-4/D-12 等）。
> **后续新决策自 ADR-135 起编号**（用户指令 + 手册 Ch18 收尾声明）。

## 1. ADR-095~134 落档总表（40 条）

| 编号 | 决策 | 状态 | 来源 | Phase 1 落地映射 |
|---|---|---|---|---|
| ADR-095 | 双运行时分工（tokio=IO / rayon=CPU） | ✅ 已决-已落地 | S9 | T8 ComputeBridge（rayon 1.10 入 nexus-core） |
| ADR-096 | ComputeBridge 全局单例 + oneshot 返回 | ✅ 已决-已落地 | S9 | T8 bridge() OnceLock + spawn_compute |
| ADR-097 | rayon 池 = num_cpus−2 | ✅ 已决-已落地 | S9 | T8 available_parallelism()-2 下限 2 |
| ADR-098 | 批大小 = 线程数倍数 | ✅ 已决-已落地 | S9 | T8 spawn_compute_batch（rayon scope 分块） |
| ADR-099 | IO-on-rayon 禁令（sqlite→spawn_blocking） | ✅ 已决-已落地 | S9 | T8 契约纪律 + T13 lint 待固化 |
| ADR-100 | 双构建矩阵（x86-64-v3 + native） | ✅ 已决-已落地 | S9 | T10 dual-build job（release.yml） |
| ADR-101 | 禁止手写 SIMD | ✅ 已决-已落地 | S9 | T10 全程零 SIMD（E8-1 复核） |
| ADR-102 | 跨构建确定性：双模式归约 + 1e-6 容忍 | ✅ 已决-已落地（**口径修正**） | S9 | T10 DetReduce；审计开销 30% 门禁修正为趋势监控 + CI 位模式 diff（W1 测定校准） |
| ADR-103 | HTS-CPU 阈值三重来源法 | ✅ 已决-已落地 | S9 | T9（离线测定 + SPRT 序贯检验 + cgroup 校正） |
| ADR-104 | ShardedBus 背压协议 = 分片丢弃 + mpsc 阻塞 | ✅ 已决-已落地（修正：shed=回退 broadcast 非丢弃） | S9 | T12（事件永不丢弃，漏发率恒 0） |
| ADR-105 | Session 会话键分片（FNV-1a + 启动快照） | ✅ 已决-已落地 | S9 | T12 fnv1a(type_name)%64（kind() 缺失替代，语义等价裁决） |
| ADR-106 | 双模式归约挂载 DetReduce 管线 | ✅ 已决-已落地 | S9 | T10 ComputeBridge::reduce 统一入口 |
| ADR-107 | ComputeKernel trait 预留（GPU 钩子不实现） | ✅ 已决-未实施（储备） | S9 | 无 Phase 1 动作（C14 GPU 否决维持） |
| ADR-108 | CBMR 微批读取（读/写分区放大） | ✅ 已决-未实施（Phase 2） | S9 | Phase 2 W9 session-store（D-7 归属） |
| ADR-109 | k-way 归并时间线回放 | ✅ 已决-未实施（Phase 2） | S9 | Phase 2 W10 |
| ADR-110 | VerifierTournament 五阶段验证管线 | ✅ 已决-未实施（Phase 2+） | S9 | v4.0 WI-26 增强（TTS 第七源 ADR-095 吸收） |
| ADR-111 | SER 两阶段检索 + HNSW 阈值门控 | ✅ 已决-未实施（Phase 2） | S9 | v4.0 WI-15 阶段一（PatternIndex） |
| ADR-112 | TSR×MoE 任务-子代理路由（top-k 6~8） | ✅ 已决-未实施（Phase 2） | S9 | v4.0 WI-09；挂载点裁决见 ADR-137 |
| ADR-113 | AERA 自适应错误恢复分配 | ✅ 已决-未实施（Phase 2） | S9 | v4.0 WI-19 |
| ADR-114 | CBF 信用流背压 | ✅ 已决-已落地 | S9 | T11 credit_flow.rs（256 初始/Notify 高优窗口） |
| ADR-115 | RSB 三缓冲模拟状态 | ✅ 已决-未实施（Phase 2） | S9 | v4.0 WI-20；挂载点裁决见 ADR-137 |
| ADR-116 | CausalGraph 5s 归因窗口 | ✅ 已决-未实施（Phase 2+） | S9 | v4.0 §2.5 ADR-093 部分吸收 |
| ADR-117 | ProcedureRegistry 三级检索 | ✅ 已决-未实施（Phase 2+） | S9 | v4.0 WI-10 增强（ADR-089 吸收） |
| ADR-118 | PTC 并行工具协调 DAG | ✅ 已决-未实施（Phase 2） | S9 | v4.0 WI-16（Ⅲ期） |
| ADR-119 | CSC 四级压缩阈值 ×1.3/×1.15/×1.0 | ✅ 已决-未实施（Phase 2） | S9 | v4.0 WI-12；挂载点裁决见 ADR-137 |
| ADR-120 | L-f 任务粒度自适应调度 | ✅ 已决-已落地 | S9 | T8/T9（route + DispatchPlan + HTS 动态表） |
| ADR-121 | NexusEventV2 降级为查询视图 | ✅ 已决-维持 | S9 | D-6 裁决（双轨制，WI-21 Phase 3） |
| ADR-122 | 降级路径 = 子命令 + 配置（禁 feature 标志） | ✅ 已决-已落地 | S9 | 全 Phase 1（env 开关 + 配置项） |
| ADR-123 | 否决 GPU 卸载 | ✅ 已决-维持 | S9 | 无动作 |
| ADR-124 | 否决 crossbeam 作为第三运行时 | ✅ 已决-已落地 | S9 | T12 crossbeam-queue 仅内部实现（ArrayQueue） |
| ADR-125 | PerCpuPadded 遥测计数器（批提交） | ✅ 已决-已落地（部分） | S9 | T11 release_many 批归还语义 |
| ADR-126 | S8 Triple-Runtime 否决登记 | ✅ 已决-已落地 | 手册 C8 | 双运行时唯一（tokio+rayon） |
| ADR-127 | RuntimeSwitcher 合并入 ComputeBridge L-f route() | ✅ 已决-已落地 | 手册 C9 | T8 DispatchPlan 三态 |
| ADR-128 | DashMap/arc-swap 替代 ShardedMap-SegQueue | ✅ 已决-已落地 | 手册 C10 | T9 ArcSwap<HtsTable>（RCU 读无锁） |
| ADR-129 | 全系统禁自旋：统一 Notify 等待 | ✅ 已决-已落地 | 手册 C11 | T11/T12 Notify 唤醒（无 spin 验证） |
| ADR-130 | ParallelEventBus 并入 ShardedEventBus | ✅ 已决-已落地 | 手册 C12 | T12 唯一分片总线（无并行双总线） |
| ADR-131 | 半层否决与 Lx.5 重映射 | ✅ 已决-已落地 | 手册 | D-5 裁决（深度打磨计划半层写法整体否决） |
| ADR-132 | CausalGraph × W5 反馈回路联动设计 | ✅ 已决-未实施（Phase 2+） | 手册缝合 | v4.0 §18 追溯 |
| ADR-133 | T-08 外部 API 守卫（LLM 供应商漂移检测） | ✅ 已决-未实施（Phase 2） | 手册缝合 | v4.0 WI-06 相关 |
| ADR-134 | 22 周基线 + 4 周缓冲的双轨计划 | ✅ 已决-已落地 | 手册 | Phase 1 = W1-W8（波次 0+1），缓冲机制生效中 |

## 2. Phase 1 新增裁决点登记（ADR-135 起，提交架构委员会）

| 编号 | 提案 | 状态 | 建议 | 触发时机 |
|---|---|---|---|---|
| ADR-135 | rayon 入 nexus-core（L1）依赖面 | 🔶 待委员会批准 | 批准（v4.0 §7.5.2 口径；T8 已实施，dep 审计通过） | W2 契约评审（实施先行，批准补记） |
| ADR-136 | ShardedBus 首批接入 3 个 B 级 crate 名单 | 🔶 待委员会批准 | T12 已接入 mlc-engine/efficiency-monitor 2 个；第 3 个候选从 gqep-executor/repo-wiki 选定 | W6 前 |
| ADR-137 | nexus-compress/nexus-moe-router/nexus-residual：新建 vs 增强 | 🔶 待委员会裁决 | 倾向增强（hcw-window/faae-router/mlc-engine 联动，Ω₆ 复用） | Phase 2 前置（W8 收口时） |
| ADR-138 | ADR-095~134 落档口径（与 v4.0 WI 合并登记） | 🔶 待委员会确认 | 本文件即落档形态；独立文件化随 Phase 2 持续推进 | W1 治理周 |

## 3. 落档纪律

1. 本文件为 ADR-095~134 的**权威落档登记**；任何修订须经架构委员会并在此追加
2. 涉及"已落地"标记的 ADR 均有 Phase 1 任务交付物与测试证据支撑（见 §1 映射列）
3. "未实施（Phase 2/2+）"条目按 D-7 归属排期，不视为债务（计划内）
4. ADR-102 与门禁口径修正（审计开销 30%）的详细论证见 docs/reports/phase1-w1-baseline.md §5
