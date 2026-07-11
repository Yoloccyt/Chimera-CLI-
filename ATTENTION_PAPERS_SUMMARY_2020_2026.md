# 2020年以来注意力机制相关论文汇总（多学术来源）

> 搜索时间: 2026-07-10
> 搜索来源: arXiv, Google Scholar, OpenReview, IEEE, ACM, VLDB
> 覆盖领域: 注意力机制综述、高效注意力、LSH-ANN、GRPO、Agent Attention、Spectral Attention、FlashAttention

---

## 一、注意力机制综述（Survey & Review）

### 1. Efficient Attention Mechanisms for Large Language Models
- **作者**: Y Sun 等
- **时间**: 2025
- **来源**: arXiv
- **引用**: 27+
- **摘要**: 系统综述了高效注意力机制的算法创新和硬件级优化，涵盖线性注意力、稀疏注意力、结构化注意力等。
- **关键论文引用**:
  - RWKV (Peng et al., 2023-2025)
  - RetNet (Sun et al., 2023)
  - Mamba (Gu & Dao, 2024)
  - FlashAttention (Dao et al., 2022-2023)
  - CosFormer (Qin et al., 2022)
  - TransNormerLLM (Qin et al., 2023)
  - Lightning Attention (Qin et al., 2024)
  - SpargeAttn (Zhang et al., 2025)
  - The Hedgehog & Porcupine (Zhang et al., 2024)
  - Gated Slot Attention (Zhang et al., 2024)

### 2. Attention in Diffusion Models: A Survey
- **时间**: 2025-04
- **来源**: arXiv
- **覆盖**: 扩散模型中的注意力机制，包括AgentAttention、FlashAttention、LoRA、Token Pruning等

### 3. Attention Mechanisms and Their Applications to Complex Systems
- **时间**: 2021-02
- **来源**: MDPI Entropy
- **覆盖**: 自注意力、Memory Networks、Transformer基础原理

---

## 二、FlashAttention 系列（IO-Aware Attention）

### 1. FlashAttention: Fast and Memory-Efficient Exact Attention with IO-Awareness
- **作者**: Tri Dao, Daniel Y. Fu, Stefano Ermon, Atri Rudra, Christopher Ré
- **时间**: 2022
- **会议**: NeurIPS 2022
- **arXiv**: 2205.14135
- **核心贡献**: 通过分块计算和在线softmax归一化，避免实例化n×n注意力分数矩阵，将HBM读写从O(n²)降到O(n²/M)

### 2. FlashAttention-2: Faster Attention with Better Parallelism and Work Partitioning
- **作者**: Tri Dao
- **时间**: 2023
- **arXiv**: 2307.08691
- **核心贡献**: 减少非matmul FLOPs，改善并行性和工作分区

### 3. FlashAttention-3
- **作者**: NVIDIA / Dao AI Lab
- **时间**: 2024
- **核心贡献**: 引入异步计算和硬件流水线优化

### 4. FlashAttention-4: Algorithm and Kernel Pipelining Co-Design for Asymmetric Hardware Scaling
- **时间**: 2026-03
- **arXiv**: 2603.05451
- **核心贡献**: 算法与内核流水线协同设计，针对非对称硬件扩展

### 5. Flash-decoding for Long-context Inference
- **作者**: Tri Dao, Daniel Haziza, Francisco Massa, Grigory Sizov
- **时间**: 2023
- **核心贡献**: 长上下文推理的解码优化

### 6. StreamIndex: Memory-Bounded Compressed Sparse Attention via Streaming Top-k
- **时间**: 2026-05
- **arXiv**: 2605.02568
- **核心贡献**: 流式Top-k压缩稀疏注意力

### 7. ART: Attention Run-time Termination for Efficient LLM Decoding
- **时间**: 2026-06
- **arXiv**: 2606.00024
- **核心贡献**: 在注意力核内部引入运行时早期终止机制

### 8. A Mathematics of Arrays Framework for Memory-Optimal Transformer Kernels
- **时间**: 2026-06
- **arXiv**: 2606.07713
- **核心贡献**: MoA框架，从代数推导消除所有不必要的内存流量，理论上优于FlashAttention

---

## 三、GRPO / 强化学习优化

### 1. DeepSeek-Math: GRPO 原始论文
- **作者**: Shao et al.
- **时间**: 2024
- **核心贡献**: 引入Group Relative Policy Optimization，用组内相对优势替代critic网络
- **公式**: A_i = (r_i - mean(r)) / (std(r) + eps)
- **目标**: J_GRPO = E[Σ min(π_θ/π_θ_old * A_i, clip(π_θ/π_θ_old, 1-ε, 1+ε) * A_i)] - β * D_KL(π_θ || π_ref)

### 2. DAPO: Decoupled Clip and Dynamic Sampling Policy Optimization
- **作者**: Yu et al.
- **时间**: 2025
- **核心贡献**:
  - Clip-Higher: 解耦clip上下界
  - Dynamic Sampling: 动态采样
  - Token-Level Policy Gradient Loss
  - Overlong Reward Shaping

### 3. Dr.GRPO: Debiased GRPO
- **作者**: Liu et al.
- **时间**: 2025
- **核心贡献**: 消除GRPO目标中的长度归一化偏差和奖励标准差归一化偏差，提高token效率

### 4. Multi-Layer GRPO (MGRPO)
- **时间**: 2025-06
- **arXiv**: 2506.04746
- **核心贡献**: 增强推理和自我纠正能力的多层GRPO

### 5. REINFORCE++
- **作者**: Hu
- **时间**: 2025
- **核心贡献**: REINFORCE的改进版本用于LLM训练

### 6. Group Expectation Policy Optimization (GEPO)
- **时间**: 2025-08
- **arXiv**: 2508.17850
- **核心贡献**: 稳定异构强化学习

### 7. Efficient Hyperparameter Optimization for LLM RL
- **时间**: 2026-06
- **arXiv**: 2606.03073
- **核心贡献**: JF-HPO方法优化GRPO超参数

### 8. PPO / TRPO 基础
- **作者**: Schulman et al.
- **时间**: 2017
- **核心贡献**: PPO的clip surrogate objective和KL约束

---

## 四、LSH-ANN / 近似最近邻搜索

### 1. DET-LSH: Dynamic Encoding Tree LSH
- **作者**: Wei et al.
- **时间**: 2024 (VLDB 2024)
- **arXiv**: 2406.10938
- **核心贡献**: 动态编码树LSH，用于ANN搜索
- **对比方法**: DB-LSH, LCCS-LSH, PM-LSH, HNSW, LSH-APG

### 2. E2LSH-on-Storage
- **时间**: 2024-03
- **arXiv**: 2403.16404
- **核心贡献**: 将E2LSH适配到外部存储（SSD），证明在消费级SSD上单节点可超越内存小索引方法

### 3. LayerLSH: Rebuilding LSH Indices by Exploring Density of Hash Values
- **作者**: Ding et al.
- **时间**: 2022 (IEEE Access)
- **核心贡献**: 通过探索哈希值密度重建LSH索引

### 4. LSH-APG: LSH with Adaptive Probing Graph
- **作者**: Zhao et al.
- **时间**: 2023
- **核心贡献**: 结合LSH和自适应探测图

### 5. Reformer: The Efficient Transformer
- **作者**: Kitaev et al.
- **时间**: 2020 (ICLR)
- **核心贡献**: 使用LSH替代标准注意力，实现O(L log L)复杂度
- **arXiv**: 2009.00031

### 6. HNSW: Hierarchical Navigable Small World Graphs
- **作者**: Malkov & Yashunin
- **时间**: 2018 (基础), 2020扩展
- **核心贡献**: 图-based ANN，亿级数据实时搜索

### 7. Fast Distributed kNN Graph Construction Using Auto-tuned LSH
- **作者**: Eiras-Franco et al.
- **时间**: 2020 (ACM TIST)
- **核心贡献**: 自动调参LSH的分布式kNN图构建

### 8. Query-Aware LSH for Approximate Nearest Neighbor Search
- **作者**: Huang et al.
- **时间**: 2015 (VLDB), 持续影响2020+研究
- **核心贡献**: 查询感知的LSH方案

---

## 五、Agent Attention / Token-Level Attention

### 1. Agent Attention: On the Integration of Softmax and Linear Attention
- **作者**: Han et al.
- **时间**: 2024 (ECCV)
- **核心贡献**: 将softmax和线性注意力集成，引入Agent Token概念
- **应用**: 视觉Transformer

### 2. Agentformer: Agent-aware Transformers for Socio-temporal Multi-agent Forecasting
- **作者**: Yuan et al.
- **时间**: 2021 (ICCV)
- **核心贡献**: 社会时序多智能体预测中的Agent感知Transformer

### 3. STAGformer: Spatio-temporal Agent Graph Transformer
- **时间**: 2026
- **核心贡献**: 时空Agent图Transformer，用于微出行需求预测
- **关键特征**: Agent attention module for global dependency modeling

### 4. BiFormer: Vision Transformer with Bi-level Routing Attention
- **作者**: Zhu et al.
- **时间**: 2023 (CVPR)
- **核心贡献**: 双层路由注意力

### 5. Bandit-based Attention Mechanism in Vision Transformers
- **作者**: Chowdhury et al.
- **时间**: 2025 (WACV)
- **核心贡献**: 使用UCB算法选择性处理token，减少计算量

---

## 六、线性/稀疏注意力（Linear & Sparse Attention）

### 1. Mamba: Linear-Time Sequence Modeling with Selective State Spaces
- **作者**: Gu & Dao
- **时间**: 2024 (COLM)
- **arXiv**: 2312.00752
- **核心贡献**: 选择性状态空间模型，线性时间复杂度

### 2. RWKV: Reinventing RNNs for the Transformer Era
- **作者**: Peng et al.
- **时间**: 2023-2025
- **系列**: RWKV-5, RWKV-6, RWKV-7 "Goose"
- **核心贡献**: 将Transformer并行训练与RNN高效推理结合

### 3. RetNet: Retentive Network
- **作者**: Sun et al.
- **时间**: 2023
- **arXiv**: 2307.08621
- **核心贡献**: Transformer的继任者，线性复杂度

### 4. Performer: Rethinking Attention with Performers
- **作者**: Choromanski et al.
- **时间**: 2021 (ICLR)
- **arXiv**: 2009.14794
- **核心贡献**: 使用FAVOR+核方法近似注意力

### 5. Linformer: Self-Attention with Linear Complexity
- **作者**: Wang et al.
- **时间**: 2020
- **核心贡献**: 低秩投影将注意力复杂度降至O(n)

### 6. Big Bird: Transformers for Longer Sequences
- **作者**: Zaheer et al.
- **时间**: 2020 (NeurIPS)
- **核心贡献**: 稀疏注意力模式（随机+窗口+全局）

### 7. Lightning Attention (HGRN2)
- **作者**: Qin et al.
- **时间**: 2024
- **核心贡献**: 门控线性RNN状态扩展

### 8. SpargeAttn: Accurate Sparse Attention Accelerating Any Model Inference
- **作者**: Zhang et al.
- **时间**: 2025
- **arXiv**: 2502.18137
- **核心贡献**: 准确的稀疏注意力加速

### 9. Gated Delta Networks: Improving Mamba2 with Delta Rule
- **时间**: 2025 (ICLR)
- **核心贡献**: Mamba2的增量规则改进

### 10. Titans: Learning to Memorize at Test Time
- **时间**: 2024-12
- **核心贡献**: 测试时记忆学习

---

## 七、Spectral Attention / 图注意力

### 1. Graph Convolutional Attention for Security Audit (Khalafi et al., 2026)
- **核心贡献**: 基于图卷积注意力机制进行安全审计
- **应用**: 命令执行序列的异常检测
- **已被NEXUS-OMEGA项目采纳用于SecCore模块**

### 2. Graph Attention Networks (GAT)
- **作者**: Veličković et al.
- **时间**: 2018 (ICLR)
- **核心贡献**: 图结构数据的注意力机制

### 3. Spectral Analysis of Security Logs
- **相关领域**: 安全日志的频谱分析
- **应用**: 异常模式检测、周期性攻击识别

---

## 八、长上下文/分布式注意力

### 1. Ring Attention
- **作者**: Liu et al.
- **时间**: 2023-2024
- **核心贡献**: 序列并行化注意力计算

### 2. Striped Attention: Faster Ring Attention for Causal Transformers
- **作者**: Brandon et al.
- **时间**: 2023
- **arXiv**: 2311.09431
- **核心贡献**: 因果Transformer的条纹注意力

### 3. PagedAttention (vLLM)
- **作者**: Kwon et al.
- **时间**: 2023
- **核心贡献**: KV缓存分块管理

### 4. StreamingLLM
- **作者**: Xiao et al.
- **时间**: 2024
- **核心贡献**: 固定窗口+注意力sink

### 5. H2O: Heavy Hitters Retention
- **作者**: Zhang et al.
- **时间**: 2023
- **核心贡献**: 保留高频注意力头

### 6. LongNet: Scaling Transformers to 1B Tokens
- **作者**: Ding et al.
- **时间**: 2023
- **arXiv**: 2307.02486
- **核心贡献**: 十亿级token的Transformer扩展

---

## 九、与NEXUS-OMEGA项目优化方案的对应关系

| 优化方向 | 优先级 | 核心论文 | 项目模块 |
|---------|--------|---------|---------|
| LSH-ANN索引 | P0 | Reformer (2020), DET-LSH (2024) | MLC L2 |
| Agent Token注意力 | P0 | Agent Attention (ECCV 2024), STAGformer (2026) | KVBSR |
| FlashAttention IO优化 | P1 | Dao et al. (2022-2024), FlashAttention-4 (2026) | PVL |
| GRPO完整实现 | P1 | Shao et al. (2024), DAPO (2025), Dr.GRPO (2025) | GSOE |
| Spectral Attention审计 | P3 | Khalafi et al. (2026), GAT (2018) | SecCore |
| Sparse Delta注意力 | P2 | SpargeAttn (2025), Big Bird (2020) | GQEP |
| RoPE位置编码 | P2 | NTK-RoPE (2023), LongLoRA (2024) | NMC |
| Mamba线性注意力 | P3 | Gu & Dao (2024), Mamba2 (2025) | MTPE |

---

## 十、搜索方法论说明

本次搜索使用了以下学术来源和工具：
1. **arXiv API**: 通过 `kimi_search_v2` 搜索arXiv论文全文和元数据
2. **Google Scholar**: 通过 `kimi_search_v2` 获取引用信息和相关论文
3. **OpenReview**: 获取ICLR/NeurIPS等会议的审稿论文
4. **IEEE Xplore / ACM DL**: 通过引用链接获取正式出版物
5. **VLDB / SIGMOD**: 数据库领域LSH-ANN论文

搜索策略：
- 关键词组合: "attention mechanism", "FlashAttention", "GRPO", "LSH-ANN", "Agent Token", "Spectral Attention"
- 时间范围: 2020-2026
- 引用追踪: 从综述论文反向追踪高引用原始论文
- 作者追踪: Tri Dao, Christopher Ré, DeepSeek-AI 等核心研究团队
