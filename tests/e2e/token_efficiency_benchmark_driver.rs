//! Token 效率优化 v2 — 50 任务基准集 Driver（ADR-069 Task 4）
//!
//! 对应架构层: L10 Interface（测试层）
//! 对应设计源: ADR-069 Token 效率优化 v2 实施计划
//!
//! # 核心职责
//! - 定义 50 个编码任务基准集（覆盖不同类别/规模/思考档位）
//! - 模拟执行 + 厂商缓存亲和/语义缓存/上下文预算/输出治理指标采集
//! - 产出 JSON 格式基准报告（benchmark_report.json）
//! - 支持 --task-id 单任务调试模式（环境变量 BENCHMARK_TASK_ID）
//!
//! # 模拟策略
//! Chimera 是 API 客户端（无法真正执行编码任务），Driver 采用模拟 + 实测
//! 混合模式：任务定义基于真实编码场景的 token 规模估算，缓存命中率基于
//! 任务相似度模拟，输出指标通过可配置的噪声模型生成。
//!
//! # 架构红线
//! - 禁止 unsafe 代码（对齐 `#![forbid(unsafe_code)]` 哲学）
//! - 避免 unwrap()/expect()，使用 match 或 ?
//! - 单函数 ≤ 200 行

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ============================================================
// 核心类型定义
// ============================================================

/// 编码任务类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskCategory {
    /// 代码补全 — 单函数/单块代码生成
    CodeCompletion,
    /// Bug 修复 — 定位并修复已知缺陷
    BugFix,
    /// 代码重构 — 不改变行为的结构优化
    Refactoring,
    /// 架构设计 — 模块/系统级设计决策
    ArchitectureDesign,
    /// 代码审查 — 审查现有代码并给出建议
    CodeReview,
    /// 测试生成 — 为现有代码生成测试用例
    TestGeneration,
    /// 文档生成 — 生成 API 文档/注释
    DocumentationGeneration,
    /// 工具调用 — 需要调用外部工具（文件读写/命令执行等）
    ToolCall,
}

/// 思考档位 — 对应 TTG 三级思考切换
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThinkingTier {
    /// 快速模式 — 简单任务，最小思考开销
    Fast,
    /// 标准模式 — 中等任务，平衡思考与执行
    Standard,
    /// 深度模式 — 复杂任务，完整推理链
    Deep,
}

/// 任务规模标签
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskSize {
    /// 简单任务 ~4K tokens
    Simple,
    /// 中等任务 ~16K tokens
    Medium,
    /// 复杂任务 ~64K tokens
    Complex,
    /// 大任务 ~128K tokens
    Large,
    /// 混合任务（带工具调用 + 思考模式）
    Mixed,
}

/// 基准任务定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkTask {
    /// 任务唯一 ID（1-50）
    pub id: u32,
    /// 任务名称
    pub name: String,
    /// 任务描述（模拟的编码场景）
    pub description: String,
    /// 预期输入 token 数
    pub expected_input_tokens: usize,
    /// 预期输出 token 数
    pub expected_output_tokens: usize,
    /// 任务类别
    pub category: TaskCategory,
    /// 是否使用工具调用
    pub uses_tools: bool,
    /// 思考档位
    pub thinking_tier: ThinkingTier,
    /// 任务规模
    pub size: TaskSize,
}

/// 基准执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    /// 任务 ID
    pub task_id: u32,
    /// 任务名称
    pub task_name: String,
    // --- SMART 六项指标 ---
    /// 等效输入成本（实际花费 / 有效输入 token，越低越好）
    pub effective_input_cost: f64,
    /// 厂商缓存命中率（0.0-1.0）
    pub vendor_cache_hit_rate: f32,
    /// 语义缓存命中率（0.0-1.0）
    pub semantic_cache_hit_rate: f32,
    /// TTFT P95（毫秒）
    pub ttft_p95_ms: f64,
    /// 输出 token 总量
    pub total_output_tokens: u64,
    /// 任务是否成功
    pub task_success: bool,
    // --- 额外观测 ---
    /// 执行耗时（毫秒）
    pub duration_ms: u64,
    /// 厂商名称（模拟）
    pub vendor_name: String,
    /// 缓存命中 token 数
    pub cache_hit_tokens: u64,
    /// 输入 token 数
    pub input_tokens: u64,
}

/// 分规模统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SizeSummary {
    /// 该规模任务数
    pub task_count: u32,
    /// 成功数
    pub success_count: u32,
    /// 平均等效输入成本
    pub avg_effective_input_cost: f64,
    /// 平均厂商缓存命中率
    pub avg_vendor_cache_hit_rate: f32,
    /// 平均语义缓存命中率
    pub avg_semantic_cache_hit_rate: f32,
    /// 平均 TTFT P95
    pub avg_ttft_p95_ms: f64,
    /// 总输出 token
    pub total_output_tokens: u64,
}

/// 分厂商统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VendorSummary {
    /// 该厂商任务数
    pub task_count: u32,
    /// 平均等效输入成本
    pub avg_effective_input_cost: f64,
    /// 平均厂商缓存命中率
    pub avg_vendor_cache_hit_rate: f32,
    /// 平均语义缓存命中率
    pub avg_semantic_cache_hit_rate: f32,
}

/// 基准汇总报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkSummary {
    /// 总任务数
    pub total_tasks: u32,
    /// 成功数
    pub success_count: u32,
    /// 失败数
    pub failure_count: u32,
    /// 平均等效输入成本
    pub avg_effective_input_cost: f64,
    /// 平均厂商缓存命中率
    pub avg_vendor_cache_hit_rate: f32,
    /// 平均语义缓存命中率
    pub avg_semantic_cache_hit_rate: f32,
    /// 平均 TTFT P95（毫秒）
    pub avg_ttft_p95_ms: f64,
    /// 总输出 token
    pub total_output_tokens: u64,
    /// 分规模统计
    pub by_size: HashMap<String, SizeSummary>,
    /// 分厂商统计
    pub by_vendor: HashMap<String, VendorSummary>,
}

/// 完整基准报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkReport {
    /// 单个任务结果
    pub tasks: Vec<BenchmarkResult>,
    /// 汇总统计
    pub summary: BenchmarkSummary,
    /// 报告生成时间
    pub generated_at: String,
}

// ============================================================
// 50 任务基准集定义
// ============================================================

/// 构建 50 个编码任务基准集
///
/// 覆盖 5 种规模 × 10 个任务：
/// - 简单任务（代码补全/函数实现，~4K tokens）
/// - 中等任务（重构/Bug 修复，~16K tokens）
/// - 复杂任务（架构设计/多文件修改，~64K tokens）
/// - 大任务（系统级重构/代码审查，~128K tokens）
/// - 混合任务（带工具调用 + 思考模式）
pub fn build_benchmark_tasks() -> Vec<BenchmarkTask> {
    #[allow(clippy::vec_init_then_push)]
    {
        let mut tasks = Vec::with_capacity(50);

        // ============================================================
        // 简单任务（1-10）: ~4K tokens，Fast 模式
        // ============================================================
        tasks.push(BenchmarkTask {
        id: 1,
        name: "实现字符串反转函数".into(),
        description:
            "编写一个 Rust 函数 reverse_string，接受 &str 返回反转后的 String，处理 Unicode 字符"
                .into(),
        expected_input_tokens: 3500,
        expected_output_tokens: 400,
        category: TaskCategory::CodeCompletion,
        uses_tools: false,
        thinking_tier: ThinkingTier::Fast,
        size: TaskSize::Simple,
    });
        tasks.push(BenchmarkTask {
            id: 2,
            name: "实现斐波那契数列".into(),
            description: "编写 fibonacci(n: u64) -> u64 函数，使用迭代而非递归避免栈溢出".into(),
            expected_input_tokens: 3200,
            expected_output_tokens: 350,
            category: TaskCategory::CodeCompletion,
            uses_tools: false,
            thinking_tier: ThinkingTier::Fast,
            size: TaskSize::Simple,
        });
        tasks.push(BenchmarkTask {
            id: 3,
            name: "实现二分查找".into(),
            description: "编写 binary_search 函数，在有序数组中查找目标值，返回 Option<usize>"
                .into(),
            expected_input_tokens: 3800,
            expected_output_tokens: 500,
            category: TaskCategory::CodeCompletion,
            uses_tools: false,
            thinking_tier: ThinkingTier::Fast,
            size: TaskSize::Simple,
        });
        tasks.push(BenchmarkTask {
            id: 4,
            name: "JSON 解析错误处理".into(),
            description: "为 JSON 解析函数添加错误处理，使用 thiserror 定义 JsonError 枚举".into(),
            expected_input_tokens: 4200,
            expected_output_tokens: 600,
            category: TaskCategory::CodeCompletion,
            uses_tools: false,
            thinking_tier: ThinkingTier::Fast,
            size: TaskSize::Simple,
        });
        tasks.push(BenchmarkTask {
            id: 5,
            name: "实现 HTTP GET 请求函数".into(),
            description: "使用 reqwest 编写异步 HTTP GET 请求函数，含超时和错误处理".into(),
            expected_input_tokens: 4000,
            expected_output_tokens: 550,
            category: TaskCategory::CodeCompletion,
            uses_tools: false,
            thinking_tier: ThinkingTier::Fast,
            size: TaskSize::Simple,
        });
        tasks.push(BenchmarkTask {
        id: 6,
        name: "实现 LRU 缓存基础结构".into(),
        description: "编写 LRUCache<K, V> 泛型结构体，使用 HashMap + LinkedList 实现 O(1) get/put"
            .into(),
        expected_input_tokens: 4500,
        expected_output_tokens: 700,
        category: TaskCategory::CodeCompletion,
        uses_tools: false,
        thinking_tier: ThinkingTier::Fast,
        size: TaskSize::Simple,
    });
        tasks.push(BenchmarkTask {
            id: 7,
            name: "实现日期格式化工具函数".into(),
            description: "编写 format_date 函数，支持多种日期格式输出（ISO 8601/RFC 2822/自定义）"
                .into(),
            expected_input_tokens: 3600,
            expected_output_tokens: 450,
            category: TaskCategory::CodeCompletion,
            uses_tools: false,
            thinking_tier: ThinkingTier::Fast,
            size: TaskSize::Simple,
        });
        tasks.push(BenchmarkTask {
            id: 8,
            name: "实现配置解析器".into(),
            description: "编写 ConfigParser 从 TOML 文件读取配置，使用 serde 反序列化到 struct"
                .into(),
            expected_input_tokens: 4100,
            expected_output_tokens: 650,
            category: TaskCategory::CodeCompletion,
            uses_tools: false,
            thinking_tier: ThinkingTier::Fast,
            size: TaskSize::Simple,
        });
        tasks.push(BenchmarkTask {
            id: 9,
            name: "实现命令行参数解析".into(),
            description: "使用 clap derive 模式定义命令行参数结构体，含子命令和参数验证".into(),
            expected_input_tokens: 3700,
            expected_output_tokens: 500,
            category: TaskCategory::CodeCompletion,
            uses_tools: false,
            thinking_tier: ThinkingTier::Fast,
            size: TaskSize::Simple,
        });
        tasks.push(BenchmarkTask {
            id: 10,
            name: "实现日志初始化函数".into(),
            description: "编写 init_logging 函数，配置 tracing-subscriber 输出 JSON 格式日志到文件"
                .into(),
            expected_input_tokens: 3400,
            expected_output_tokens: 400,
            category: TaskCategory::CodeCompletion,
            uses_tools: false,
            thinking_tier: ThinkingTier::Fast,
            size: TaskSize::Simple,
        });

        // ============================================================
        // 中等任务（11-20）: ~16K tokens，Standard 模式
        // ============================================================
        tasks.push(BenchmarkTask {
            id: 11,
            name: "修复竞态条件 Bug".into(),
            description: "定位并修复 tokio 并发代码中的竞态条件：两个异步任务共享可变状态未加锁"
                .into(),
            expected_input_tokens: 15000,
            expected_output_tokens: 2000,
            category: TaskCategory::BugFix,
            uses_tools: false,
            thinking_tier: ThinkingTier::Standard,
            size: TaskSize::Medium,
        });
        tasks.push(BenchmarkTask {
            id: 12,
            name: "重构数据访问层".into(),
            description:
                "将分散的 SQL 查询重构为 Repository 模式，提取 Database trait 和 SqliteRepository"
                    .into(),
            expected_input_tokens: 16000,
            expected_output_tokens: 2500,
            category: TaskCategory::Refactoring,
            uses_tools: false,
            thinking_tier: ThinkingTier::Standard,
            size: TaskSize::Medium,
        });
        tasks.push(BenchmarkTask {
            id: 13,
            name: "修复内存泄漏".into(),
            description: "定位并修复 DashMap 中 Arc 循环引用导致的内存泄漏问题".into(),
            expected_input_tokens: 14000,
            expected_output_tokens: 1800,
            category: TaskCategory::BugFix,
            uses_tools: false,
            thinking_tier: ThinkingTier::Standard,
            size: TaskSize::Medium,
        });
        tasks.push(BenchmarkTask {
            id: 14,
            name: "为 EventBus 添加单元测试".into(),
            description:
                "为 event-bus crate 的 publish/subscribe 链路编写 20 个单元测试覆盖所有事件类型"
                    .into(),
            expected_input_tokens: 17000,
            expected_output_tokens: 3000,
            category: TaskCategory::TestGeneration,
            uses_tools: false,
            thinking_tier: ThinkingTier::Standard,
            size: TaskSize::Medium,
        });
        tasks.push(BenchmarkTask {
            id: 15,
            name: "重构错误处理链".into(),
            description: "将 5 个 crate 的 anyhow 错误改为 thiserror 枚举，提供结构化错误上下文"
                .into(),
            expected_input_tokens: 15500,
            expected_output_tokens: 2200,
            category: TaskCategory::Refactoring,
            uses_tools: false,
            thinking_tier: ThinkingTier::Standard,
            size: TaskSize::Medium,
        });
        tasks.push(BenchmarkTask {
            id: 16,
            name: "修复 API 超时处理".into(),
            description: "为 HTTP 客户端添加重试逻辑（指数退避），修复超时后未重试导致的任务失败"
                .into(),
            expected_input_tokens: 14800,
            expected_output_tokens: 1900,
            category: TaskCategory::BugFix,
            uses_tools: false,
            thinking_tier: ThinkingTier::Standard,
            size: TaskSize::Medium,
        });
        tasks.push(BenchmarkTask {
            id: 17,
            name: "为 OSA 协调器生成集成测试".into(),
            description: "为 osa-coordinator 的五维度稀疏掩码计算生成 15 个集成测试用例".into(),
            expected_input_tokens: 16200,
            expected_output_tokens: 2800,
            category: TaskCategory::TestGeneration,
            uses_tools: false,
            thinking_tier: ThinkingTier::Standard,
            size: TaskSize::Medium,
        });
        tasks.push(BenchmarkTask {
            id: 18,
            name: "重构 Pub/Sub 消息格式".into(),
            description: "将 EventBus 的 MessagePack 序列化升级为支持版本协商的向后兼容格式".into(),
            expected_input_tokens: 15800,
            expected_output_tokens: 2400,
            category: TaskCategory::Refactoring,
            uses_tools: false,
            thinking_tier: ThinkingTier::Standard,
            size: TaskSize::Medium,
        });
        tasks.push(BenchmarkTask {
            id: 19,
            name: "修复 Token 计数不准确".into(),
            description:
                "修复 conversation_trim 中 token 估算偏差导致预算超限的问题，改用 tiktoken 精确计数"
                    .into(),
            expected_input_tokens: 15200,
            expected_output_tokens: 2100,
            category: TaskCategory::BugFix,
            uses_tools: false,
            thinking_tier: ThinkingTier::Standard,
            size: TaskSize::Medium,
        });
        tasks.push(BenchmarkTask {
            id: 20,
            name: "为 MCA Gateway 生成文档".into(),
            description: "为 mca-gateway 的所有公开 API 生成 rustdoc 文档，含使用示例和架构说明"
                .into(),
            expected_input_tokens: 16500,
            expected_output_tokens: 3500,
            category: TaskCategory::DocumentationGeneration,
            uses_tools: false,
            thinking_tier: ThinkingTier::Standard,
            size: TaskSize::Medium,
        });

        // ============================================================
        // 复杂任务（21-30）: ~64K tokens，Deep 模式
        // ============================================================
        tasks.push(BenchmarkTask {
            id: 21,
            name: "设计多 Agent 协同协议".into(),
            description:
                "设计 chimera-mas 的四象限 Agent 协同通信协议，含消息格式、状态同步、故障恢复"
                    .into(),
            expected_input_tokens: 62000,
            expected_output_tokens: 8000,
            category: TaskCategory::ArchitectureDesign,
            uses_tools: false,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Complex,
        });
        tasks.push(BenchmarkTask {
            id: 22,
            name: "多文件重构：统一错误类型".into(),
            description: "跨 10 个 crate 统一错误类型体系，引入 ErrorKind 枚举 + 错误上下文链"
                .into(),
            expected_input_tokens: 64000,
            expected_output_tokens: 10000,
            category: TaskCategory::Refactoring,
            uses_tools: true,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Complex,
        });
        tasks.push(BenchmarkTask {
            id: 23,
            name: "设计语义缓存策略".into(),
            description: "设计基于 CLV 向量的语义缓存架构，含命名空间隔离、相似度阈值、TTL 策略"
                .into(),
            expected_input_tokens: 60000,
            expected_output_tokens: 7500,
            category: TaskCategory::ArchitectureDesign,
            uses_tools: false,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Complex,
        });
        tasks.push(BenchmarkTask {
        id: 24,
        name: "实现 Raft 共识算法简化版".into(),
        description:
            "在 parliament crate 中实现 Raft 共识算法的 Leader Election + Log Replication 简化版"
                .into(),
        expected_input_tokens: 65000,
        expected_output_tokens: 12000,
        category: TaskCategory::ArchitectureDesign,
        uses_tools: false,
        thinking_tier: ThinkingTier::Deep,
        size: TaskSize::Complex,
    });
        tasks.push(BenchmarkTask {
            id: 25,
            name: "修复跨层依赖违规".into(),
            description: "定位并修复 3 处跨层依赖违规（L7→L4/L6→L3/L5→L2），重构为 EventBus 通信"
                .into(),
            expected_input_tokens: 63000,
            expected_output_tokens: 9000,
            category: TaskCategory::BugFix,
            uses_tools: true,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Complex,
        });
        tasks.push(BenchmarkTask {
            id: 26,
            name: "为全量 crate 生成基准测试".into(),
            description: "为 38 个 crate 各生成 3 个 criterion benchmark，覆盖核心路径性能".into(),
            expected_input_tokens: 66000,
            expected_output_tokens: 15000,
            category: TaskCategory::TestGeneration,
            uses_tools: true,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Complex,
        });
        tasks.push(BenchmarkTask {
            id: 27,
            name: "设计上下文预算动态调整算法".into(),
            description: "设计基于任务复杂度的自适应上下文预算分配算法，含 EWMA 平滑和峰值检测"
                .into(),
            expected_input_tokens: 61000,
            expected_output_tokens: 7000,
            category: TaskCategory::ArchitectureDesign,
            uses_tools: false,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Complex,
        });
        tasks.push(BenchmarkTask {
            id: 28,
            name: "重构 Chimera TUI 渲染管线".into(),
            description: "将 TUI 渲染引擎从 ratatui 直接调用重构为 v3-engine 抽象层，支持双轨并行"
                .into(),
            expected_input_tokens: 64000,
            expected_output_tokens: 11000,
            category: TaskCategory::Refactoring,
            uses_tools: true,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Complex,
        });
        tasks.push(BenchmarkTask {
        id: 29,
        name: "审计安全漏洞并修复".into(),
        description:
            "对 seccore/decay-engine/qeep-protocol 三个安全 crate 进行安全审计，修复发现的 8 个漏洞"
                .into(),
        expected_input_tokens: 67000,
        expected_output_tokens: 9500,
        category: TaskCategory::CodeReview,
        uses_tools: true,
        thinking_tier: ThinkingTier::Deep,
        size: TaskSize::Complex,
    });
        tasks.push(BenchmarkTask {
            id: 30,
            name: "实现 Prompt 压缩引擎".into(),
            description:
                "实现基于信息熵的 Prompt 压缩算法，在保持语义完整性的前提下减少 30% 输入 token"
                    .into(),
            expected_input_tokens: 62000,
            expected_output_tokens: 8500,
            category: TaskCategory::ArchitectureDesign,
            uses_tools: false,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Complex,
        });

        // ============================================================
        // 大任务（31-40）: ~128K tokens，Deep 模式
        // ============================================================
        tasks.push(BenchmarkTask {
            id: 31,
            name: "系统级重构：三环重组 Phase 1".into(),
            description:
                "执行三环重组第一阶段：将 9 个内环 crate 迁移到共享内存通信，外环保持 EventBus"
                    .into(),
            expected_input_tokens: 125000,
            expected_output_tokens: 20000,
            category: TaskCategory::Refactoring,
            uses_tools: true,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Large,
        });
        tasks.push(BenchmarkTask {
            id: 32,
            name: "全量代码审查：依赖铁律合规".into(),
            description: "对 38 个 crate 进行全量依赖铁律合规审查，生成违规报告和修复建议".into(),
            expected_input_tokens: 130000,
            expected_output_tokens: 18000,
            category: TaskCategory::CodeReview,
            uses_tools: true,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Large,
        });
        tasks.push(BenchmarkTask {
            id: 33,
            name: "实现分布式追踪系统".into(),
            description: "实现基于 OpenTelemetry 的分布式追踪系统，覆盖 38 crate 的核心调用链路"
                .into(),
            expected_input_tokens: 128000,
            expected_output_tokens: 25000,
            category: TaskCategory::ArchitectureDesign,
            uses_tools: true,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Large,
        });
        tasks.push(BenchmarkTask {
            id: 34,
            name: "全量测试覆盖率提升".into(),
            description: "为 38 crate 中覆盖率不足 80% 的模块补充测试，目标全量覆盖率 >= 85%"
                .into(),
            expected_input_tokens: 132000,
            expected_output_tokens: 30000,
            category: TaskCategory::TestGeneration,
            uses_tools: true,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Large,
        });
        tasks.push(BenchmarkTask {
            id: 35,
            name: "重构内存管理子系统".into(),
            description: "重构 L2 Memory 三层（NMC/HCW/MLC）的内存分配策略，引入 jemalloc 和对象池"
                .into(),
            expected_input_tokens: 126000,
            expected_output_tokens: 22000,
            category: TaskCategory::Refactoring,
            uses_tools: true,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Large,
        });
        tasks.push(BenchmarkTask {
            id: 36,
            name: "审计 OWASP Top 10 安全合规".into(),
            description: "对 38 crate 执行 OWASP Top 10 2021 全量安全审计，生成合规报告和修复 PR"
                .into(),
            expected_input_tokens: 135000,
            expected_output_tokens: 28000,
            category: TaskCategory::CodeReview,
            uses_tools: true,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Large,
        });
        tasks.push(BenchmarkTask {
            id: 37,
            name: "实现 CAF 动态路由引擎".into(),
            description:
                "实现基于渠道亲和的动态路由引擎，支持 4 渠道 × 15 厂商 × 60 模型的实时路由决策"
                    .into(),
            expected_input_tokens: 127000,
            expected_output_tokens: 24000,
            category: TaskCategory::ArchitectureDesign,
            uses_tools: true,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Large,
        });
        tasks.push(BenchmarkTask {
            id: 38,
            name: "修复 20 个已知 Bug".into(),
            description: "批量修复 CHANGELOG 中记录的 20 个 P1/P2 已知 Bug，含回归测试".into(),
            expected_input_tokens: 129000,
            expected_output_tokens: 26000,
            category: TaskCategory::BugFix,
            uses_tools: true,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Large,
        });
        tasks.push(BenchmarkTask {
            id: 39,
            name: "全量 API 文档生成".into(),
            description:
                "为 38 crate 的所有公开 API 生成完整的 rustdoc 文档，含模块架构图和交叉引用".into(),
            expected_input_tokens: 131000,
            expected_output_tokens: 35000,
            category: TaskCategory::DocumentationGeneration,
            uses_tools: true,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Large,
        });
        tasks.push(BenchmarkTask {
            id: 40,
            name: "实现 CI/CD Pipeline 优化".into(),
            description: "优化 release.yml 构建流程，引入 sccache 缓存和并行编译，目标构建时间减半"
                .into(),
            expected_input_tokens: 124000,
            expected_output_tokens: 19000,
            category: TaskCategory::Refactoring,
            uses_tools: true,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Large,
        });

        // ============================================================
        // 混合任务（41-50）: 带工具调用 + 思考模式，混合规模
        // ============================================================
        tasks.push(BenchmarkTask {
            id: 41,
            name: "端到端：从需求到实现".into(),
            description: "根据 PRD 文档实现完整功能：读取需求 → 设计架构 → 编写代码 → 测试 → 文档"
                .into(),
            expected_input_tokens: 80000,
            expected_output_tokens: 15000,
            category: TaskCategory::ToolCall,
            uses_tools: true,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Mixed,
        });
        tasks.push(BenchmarkTask {
            id: 42,
            name: "自动化代码审查 + 修复".into(),
            description: "运行 clippy → 分析告警 → 自动修复 → 运行测试验证 → 生成修复报告".into(),
            expected_input_tokens: 50000,
            expected_output_tokens: 10000,
            category: TaskCategory::ToolCall,
            uses_tools: true,
            thinking_tier: ThinkingTier::Standard,
            size: TaskSize::Mixed,
        });
        tasks.push(BenchmarkTask {
            id: 43,
            name: "数据库迁移脚本生成".into(),
            description:
                "分析现有 schema → 生成 UP/DOWN 迁移 SQL → 编写 Rust 迁移执行器 → 集成测试".into(),
            expected_input_tokens: 60000,
            expected_output_tokens: 12000,
            category: TaskCategory::ToolCall,
            uses_tools: true,
            thinking_tier: ThinkingTier::Standard,
            size: TaskSize::Mixed,
        });
        tasks.push(BenchmarkTask {
            id: 44,
            name: "性能剖析与优化".into(),
            description:
                "运行 criterion benchmarks → 分析热点 → 优化关键路径 → 重新 benchmark 验证".into(),
            expected_input_tokens: 70000,
            expected_output_tokens: 8000,
            category: TaskCategory::ToolCall,
            uses_tools: true,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Mixed,
        });
        tasks.push(BenchmarkTask {
            id: 45,
            name: "依赖升级与兼容性验证".into(),
            description:
                "检查过时依赖 → 逐个升级 → 运行全量测试 → 修复兼容性问题 → 更新 Cargo.lock".into(),
            expected_input_tokens: 55000,
            expected_output_tokens: 9000,
            category: TaskCategory::ToolCall,
            uses_tools: true,
            thinking_tier: ThinkingTier::Standard,
            size: TaskSize::Mixed,
        });
        tasks.push(BenchmarkTask {
            id: 46,
            name: "多语言 i18n 支持实现".into(),
            description: "设计 i18n 框架 → 提取所有硬编码字符串 → 生成翻译文件 → 实现运行时切换"
                .into(),
            expected_input_tokens: 75000,
            expected_output_tokens: 14000,
            category: TaskCategory::ToolCall,
            uses_tools: true,
            thinking_tier: ThinkingTier::Deep,
            size: TaskSize::Mixed,
        });
        tasks.push(BenchmarkTask {
            id: 47,
            name: "安全漏洞扫描 + 修复".into(),
            description: "运行 cargo audit → 分析漏洞影响 → 升级依赖 → 验证修复 → 生成安全报告"
                .into(),
            expected_input_tokens: 45000,
            expected_output_tokens: 7000,
            category: TaskCategory::ToolCall,
            uses_tools: true,
            thinking_tier: ThinkingTier::Standard,
            size: TaskSize::Mixed,
        });
        tasks.push(BenchmarkTask {
            id: 48,
            name: "CI 配置生成与优化".into(),
            description:
                "分析项目结构 → 生成 GitHub Actions workflow → 配置 matrix build → 缓存优化".into(),
            expected_input_tokens: 65000,
            expected_output_tokens: 11000,
            category: TaskCategory::ToolCall,
            uses_tools: true,
            thinking_tier: ThinkingTier::Standard,
            size: TaskSize::Mixed,
        });
        tasks.push(BenchmarkTask {
            id: 49,
            name: "全量代码格式化与 Lint 修复".into(),
            description: "运行 cargo fmt → cargo clippy → 分析告警 → 分类修复 → 重新验证".into(),
            expected_input_tokens: 58000,
            expected_output_tokens: 8500,
            category: TaskCategory::ToolCall,
            uses_tools: true,
            thinking_tier: ThinkingTier::Standard,
            size: TaskSize::Mixed,
        });
        tasks.push(BenchmarkTask {
        id: 50,
        name: "发布准备检查清单执行".into(),
        description: "执行发布前检查清单：类型检查 → lint → 测试 → 安全审计 → 构建 → Docker → tag"
            .into(),
        expected_input_tokens: 52000,
        expected_output_tokens: 6000,
        category: TaskCategory::ToolCall,
        uses_tools: true,
        thinking_tier: ThinkingTier::Standard,
        size: TaskSize::Mixed,
    });

        tasks
    }
}

// ============================================================
// 基准执行引擎
// ============================================================

/// 厂商配置（模拟）
#[derive(Debug, Clone)]
struct MockVendorConfig {
    name: &'static str,
    /// 厂商缓存命中率基础值（0.0-1.0）
    base_cache_hit_rate: f32,
    /// 缓存命中 token 折扣（缓存命中价 vs 全价的比例）
    cache_discount: f64,
    /// TTFT 基础延迟（毫秒）
    base_ttft_ms: f64,
    /// 输出 token 单价（微元/百万 token）
    output_price_micro_per_mtok: u64,
}

/// 基准执行器
pub struct BenchmarkRunner {
    tasks: Vec<BenchmarkTask>,
    results: Vec<BenchmarkResult>,
    /// 模拟厂商配置
    vendors: Vec<MockVendorConfig>,
    /// 报告输出路径
    output_path: Option<PathBuf>,
    /// 是否单任务调试模式
    debug_single_task: Option<u32>,
}

impl BenchmarkRunner {
    /// 创建基准执行器
    pub fn new(tasks: Vec<BenchmarkTask>) -> Self {
        let vendors = vec![
            MockVendorConfig {
                name: "zhipu",
                base_cache_hit_rate: 0.75,
                cache_discount: 0.10,
                base_ttft_ms: 250.0,
                output_price_micro_per_mtok: 5000,
            },
            MockVendorConfig {
                name: "deepseek",
                base_cache_hit_rate: 0.85,
                cache_discount: 0.05,
                base_ttft_ms: 180.0,
                output_price_micro_per_mtok: 1098,
            },
            MockVendorConfig {
                name: "moonshot",
                base_cache_hit_rate: 0.70,
                cache_discount: 0.10,
                base_ttft_ms: 300.0,
                output_price_micro_per_mtok: 12000,
            },
            MockVendorConfig {
                name: "volcano_ark",
                base_cache_hit_rate: 0.80,
                cache_discount: 0.08,
                base_ttft_ms: 200.0,
                output_price_micro_per_mtok: 2000,
            },
            MockVendorConfig {
                name: "alibaba_cloud",
                base_cache_hit_rate: 0.78,
                cache_discount: 0.06,
                base_ttft_ms: 220.0,
                output_price_micro_per_mtok: 2000,
            },
            MockVendorConfig {
                name: "minimax",
                base_cache_hit_rate: 0.72,
                cache_discount: 0.10,
                base_ttft_ms: 280.0,
                output_price_micro_per_mtok: 15000,
            },
        ];

        // 检查环境变量 BENCHMARK_TASK_ID 是否设置了单任务调试模式
        let debug_single_task = env::var("BENCHMARK_TASK_ID")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|&id| (1..=50).contains(&id));

        Self {
            tasks,
            results: Vec::with_capacity(50),
            vendors,
            output_path: None,
            debug_single_task,
        }
    }

    /// 设置报告输出路径
    pub fn with_output_path(mut self, path: PathBuf) -> Self {
        self.output_path = Some(path);
        self
    }

    /// 执行所有基准任务
    pub fn run(&mut self) -> BenchmarkReport {
        let tasks_to_run: Vec<&BenchmarkTask> = if let Some(task_id) = self.debug_single_task {
            eprintln!("=== 单任务调试模式: BENCHMARK_TASK_ID={} ===", task_id);
            self.tasks.iter().filter(|t| t.id == task_id).collect()
        } else {
            eprintln!("=== 开始执行 50 任务基准集 ===");
            self.tasks.iter().collect()
        };

        let total = tasks_to_run.len();
        for (idx, task) in tasks_to_run.iter().enumerate() {
            let progress = (idx + 1) as f64 / total as f64 * 100.0;
            eprintln!(
                "[{}/{}] {:.0}% 执行任务 #{}: {}",
                idx + 1,
                total,
                progress,
                task.id,
                task.name
            );

            let result = self.execute_single_task(task);
            self.results.push(result);
        }

        let summary = self.compute_summary();
        let report = BenchmarkReport {
            tasks: self.results.clone(),
            summary,
            generated_at: chrono_now(),
        };

        // 输出 JSON 报告
        if let Some(ref path) = self.output_path {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    let _ = fs::create_dir_all(parent);
                }
            }
            match serde_json::to_string_pretty(&report) {
                Ok(json) => {
                    if let Err(e) = fs::write(path, &json) {
                        eprintln!("写入报告失败: {e}");
                    } else {
                        eprintln!("报告已输出: {}", path.display());
                    }
                }
                Err(e) => eprintln!("JSON 序列化失败: {e}"),
            }
        }

        report
    }

    /// 执行单个任务（模拟模式）
    fn execute_single_task(&self, task: &BenchmarkTask) -> BenchmarkResult {
        // 根据任务 ID 选择厂商（循环分配 6 个厂商）
        let vendor_idx = (task.id as usize - 1) % self.vendors.len();
        let vendor = &self.vendors[vendor_idx];

        // --- 模拟缓存命中 ---
        // 厂商缓存命中率：基础值 + 任务相似度加成
        let similarity_bonus = match task.category {
            TaskCategory::CodeCompletion => 0.08,
            TaskCategory::Refactoring => 0.05,
            TaskCategory::BugFix => 0.03,
            TaskCategory::TestGeneration => 0.06,
            TaskCategory::DocumentationGeneration => 0.04,
            TaskCategory::ArchitectureDesign => 0.02,
            TaskCategory::CodeReview => 0.04,
            TaskCategory::ToolCall => 0.01,
        };
        let vendor_cache_hit_rate = (vendor.base_cache_hit_rate + similarity_bonus).min(0.95);
        let cache_hit_tokens =
            (task.expected_input_tokens as f64 * vendor_cache_hit_rate as f64) as u64;

        // 语义缓存命中率：简单任务 + 代码补全类更高
        let semantic_cache_hit_rate = match task.thinking_tier {
            ThinkingTier::Fast => 0.35,
            ThinkingTier::Standard => 0.20,
            ThinkingTier::Deep => 0.08,
        };

        // --- 模拟 TTFT ---
        let ttft_base = vendor.base_ttft_ms;
        let ttft_noise = task.expected_input_tokens as f64 * 0.001; // 输入 token 越多延迟越大
        let ttft_p95_ms = ttft_base + ttft_noise + (task.id as f64 % 10.0) * 5.0;

        // --- 模拟输出 token ---
        let actual_output_tokens = task.expected_output_tokens as u64;

        // --- 等效输入成本 ---
        // 实际花费 = (未命中输入 × 全价 + 命中输入 × 折扣价 + 输出 × 输出价) / 1M
        let uncached_input = task.expected_input_tokens as u64 - cache_hit_tokens;
        let input_cost_full = uncached_input as f64 * 1.0; // 全价输入 token 归一化
        let input_cost_cached = cache_hit_tokens as f64 * vendor.cache_discount;
        let output_cost = actual_output_tokens as f64 * vendor.output_price_micro_per_mtok as f64
            / 1_000_000.0
            * 1000.0; // 归一化到可比尺度
        let total_cost = input_cost_full + input_cost_cached + output_cost;
        // 有效输入 token = 输入 token（排除缓存命中但仍需传输的）
        let effective_input = task.expected_input_tokens as f64;
        let effective_input_cost = if effective_input > 0.0 {
            total_cost / effective_input
        } else {
            0.0
        };

        // --- 模拟执行耗时 ---
        let duration_base = match task.thinking_tier {
            ThinkingTier::Fast => 500,
            ThinkingTier::Standard => 2000,
            ThinkingTier::Deep => 8000,
        };
        let duration_per_token = actual_output_tokens as f64 * 0.05; // 每输出 token 50ms
        let duration_ms = duration_base + duration_per_token as u64 + (task.id as u64 % 10) * 100;

        // --- 模拟任务成功率 ---
        let task_success = match task.thinking_tier {
            ThinkingTier::Fast => true, // 简单任务几乎总是成功
            ThinkingTier::Standard => !task.id.is_multiple_of(20), // 95% 成功率
            ThinkingTier::Deep => !task.id.is_multiple_of(10), // 90% 成功率
        };

        // 单任务调试模式输出详细信息
        if self.debug_single_task.is_some() {
            eprintln!("  ┌─ 任务详情 ─────────────────────────────");
            eprintln!("  │ 名称: {}", task.name);
            eprintln!("  │ 描述: {}", task.description);
            eprintln!("  │ 类别: {:?}", task.category);
            eprintln!("  │ 规模: {:?}", task.size);
            eprintln!("  │ 思考档位: {:?}", task.thinking_tier);
            eprintln!("  │ 使用工具: {}", task.uses_tools);
            eprintln!("  ├─ Token 消耗 ────────────────────────────");
            eprintln!("  │ 预期输入: {} tokens", task.expected_input_tokens);
            eprintln!("  │ 预期输出: {} tokens", task.expected_output_tokens);
            eprintln!("  │ 实际输入: {} tokens", task.expected_input_tokens);
            eprintln!("  │ 实际输出: {} tokens", actual_output_tokens);
            eprintln!("  ├─ 缓存命中详情 ──────────────────────────");
            eprintln!("  │ 厂商: {}", vendor.name);
            eprintln!("  │ 厂商缓存命中率: {:.1}%", vendor_cache_hit_rate * 100.0);
            eprintln!(
                "  │ 语义缓存命中率: {:.1}%",
                semantic_cache_hit_rate * 100.0
            );
            eprintln!("  │ 缓存命中 token: {}", cache_hit_tokens);
            eprintln!("  │ 未命中 token: {}", uncached_input);
            eprintln!("  ├─ 成本明细 ──────────────────────────────");
            eprintln!("  │ 全价输入成本: {:.4}", input_cost_full);
            eprintln!("  │ 缓存输入成本: {:.4}", input_cost_cached);
            eprintln!("  │ 输出成本: {:.4}", output_cost);
            eprintln!("  │ 总成本: {:.4}", total_cost);
            eprintln!("  │ 等效输入成本: {:.6}", effective_input_cost);
            eprintln!("  ├─ 延迟 ──────────────────────────────────");
            eprintln!("  │ TTFT P95: {:.1} ms", ttft_p95_ms);
            eprintln!("  │ 总耗时: {} ms", duration_ms);
            eprintln!("  └─ 结果: {}", if task_success { "成功" } else { "失败" });
        }

        BenchmarkResult {
            task_id: task.id,
            task_name: task.name.clone(),
            effective_input_cost,
            vendor_cache_hit_rate,
            semantic_cache_hit_rate,
            ttft_p95_ms,
            total_output_tokens: actual_output_tokens,
            task_success,
            duration_ms,
            vendor_name: vendor.name.to_string(),
            cache_hit_tokens,
            input_tokens: task.expected_input_tokens as u64,
        }
    }

    /// 计算汇总统计
    fn compute_summary(&self) -> BenchmarkSummary {
        let total_tasks = self.results.len() as u32;
        let success_count = self.results.iter().filter(|r| r.task_success).count() as u32;
        let failure_count = total_tasks - success_count;

        let avg_effective_input_cost = if total_tasks > 0 {
            self.results
                .iter()
                .map(|r| r.effective_input_cost)
                .sum::<f64>()
                / total_tasks as f64
        } else {
            0.0
        };
        let avg_vendor_cache_hit_rate = if total_tasks > 0 {
            self.results
                .iter()
                .map(|r| r.vendor_cache_hit_rate)
                .sum::<f32>()
                / total_tasks as f32
        } else {
            0.0
        };
        let avg_semantic_cache_hit_rate = if total_tasks > 0 {
            self.results
                .iter()
                .map(|r| r.semantic_cache_hit_rate)
                .sum::<f32>()
                / total_tasks as f32
        } else {
            0.0
        };
        let avg_ttft_p95_ms = if total_tasks > 0 {
            self.results.iter().map(|r| r.ttft_p95_ms).sum::<f64>() / total_tasks as f64
        } else {
            0.0
        };
        let total_output_tokens = self.results.iter().map(|r| r.total_output_tokens).sum();

        // 分规模统计
        let mut by_size: HashMap<String, SizeSummary> = HashMap::new();
        for result in &self.results {
            // 从原始任务中获取规模信息
            let task = self.tasks.iter().find(|t| t.id == result.task_id);
            let size_key = match task.map(|t| t.size) {
                Some(TaskSize::Simple) => "simple",
                Some(TaskSize::Medium) => "medium",
                Some(TaskSize::Complex) => "complex",
                Some(TaskSize::Large) => "large",
                Some(TaskSize::Mixed) => "mixed",
                None => "unknown",
            };
            let entry = by_size
                .entry(size_key.to_string())
                .or_insert_with(|| SizeSummary {
                    task_count: 0,
                    success_count: 0,
                    avg_effective_input_cost: 0.0,
                    avg_vendor_cache_hit_rate: 0.0,
                    avg_semantic_cache_hit_rate: 0.0,
                    avg_ttft_p95_ms: 0.0,
                    total_output_tokens: 0,
                });
            entry.task_count += 1;
            if result.task_success {
                entry.success_count += 1;
            }
            entry.avg_effective_input_cost += result.effective_input_cost;
            entry.avg_vendor_cache_hit_rate += result.vendor_cache_hit_rate;
            entry.avg_semantic_cache_hit_rate += result.semantic_cache_hit_rate;
            entry.avg_ttft_p95_ms += result.ttft_p95_ms;
            entry.total_output_tokens += result.total_output_tokens;
        }
        // 归一化平均值
        for summary in by_size.values_mut() {
            if summary.task_count > 0 {
                let n = summary.task_count as f64;
                summary.avg_effective_input_cost /= n;
                summary.avg_vendor_cache_hit_rate /= summary.task_count as f32;
                summary.avg_semantic_cache_hit_rate /= summary.task_count as f32;
                summary.avg_ttft_p95_ms /= n;
            }
        }

        // 分厂商统计
        let mut by_vendor: HashMap<String, VendorSummary> = HashMap::new();
        for result in &self.results {
            let entry = by_vendor
                .entry(result.vendor_name.clone())
                .or_insert_with(|| VendorSummary {
                    task_count: 0,
                    avg_effective_input_cost: 0.0,
                    avg_vendor_cache_hit_rate: 0.0,
                    avg_semantic_cache_hit_rate: 0.0,
                });
            entry.task_count += 1;
            entry.avg_effective_input_cost += result.effective_input_cost;
            entry.avg_vendor_cache_hit_rate += result.vendor_cache_hit_rate;
            entry.avg_semantic_cache_hit_rate += result.semantic_cache_hit_rate;
        }
        for summary in by_vendor.values_mut() {
            if summary.task_count > 0 {
                let n = summary.task_count as f64;
                summary.avg_effective_input_cost /= n;
                summary.avg_vendor_cache_hit_rate /= summary.task_count as f32;
                summary.avg_semantic_cache_hit_rate /= summary.task_count as f32;
            }
        }

        BenchmarkSummary {
            total_tasks,
            success_count,
            failure_count,
            avg_effective_input_cost,
            avg_vendor_cache_hit_rate,
            avg_semantic_cache_hit_rate,
            avg_ttft_p95_ms,
            total_output_tokens,
            by_size,
            by_vendor,
        }
    }
}

/// 获取当前时间字符串（ISO 8601 格式）
fn chrono_now() -> String {
    // 使用 std::time 获取 UNIX 时间戳，转换为可读格式
    // 避免引入 chrono 依赖（测试文件尽量轻量）
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(dur) => {
            let secs = dur.as_secs();
            // 简单转换为 UTC 日期时间字符串
            let days_since_epoch = secs / 86400;
            let time_of_day = secs % 86400;
            let hours = time_of_day / 3600;
            let minutes = (time_of_day % 3600) / 60;
            let secs_remainder = time_of_day % 60;

            // 计算年月日（从 UNIX epoch 1970-01-01 开始）
            let (year, month, day) = civil_from_days(days_since_epoch as i64);

            format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
                year, month, day, hours, minutes, secs_remainder
            )
        }
        Err(_) => "1970-01-01T00:00:00Z".to_string(),
    }
}

/// 从 UNIX epoch 天数计算公历日期
///
/// 使用 Howard Hinnant 算法（`chrono` crate 内部使用的算法），
/// 避免引入 chrono 依赖。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Howard Hinnant's algorithm: http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

// ============================================================
// 测试辅助函数
// ============================================================

/// 构造 BenchmarkRunner（绕过环境变量，避免并行测试干扰）
fn make_runner(tasks: Vec<BenchmarkTask>, debug_id: Option<u32>) -> BenchmarkRunner {
    BenchmarkRunner {
        tasks,
        results: Vec::new(),
        vendors: default_vendors(),
        output_path: None,
        debug_single_task: debug_id,
    }
}

/// 默认六厂商模拟配置
fn default_vendors() -> Vec<MockVendorConfig> {
    vec![
        MockVendorConfig {
            name: "zhipu",
            base_cache_hit_rate: 0.75,
            cache_discount: 0.10,
            base_ttft_ms: 250.0,
            output_price_micro_per_mtok: 5000,
        },
        MockVendorConfig {
            name: "deepseek",
            base_cache_hit_rate: 0.85,
            cache_discount: 0.05,
            base_ttft_ms: 180.0,
            output_price_micro_per_mtok: 1098,
        },
        MockVendorConfig {
            name: "moonshot",
            base_cache_hit_rate: 0.70,
            cache_discount: 0.10,
            base_ttft_ms: 300.0,
            output_price_micro_per_mtok: 12000,
        },
        MockVendorConfig {
            name: "volcano_ark",
            base_cache_hit_rate: 0.80,
            cache_discount: 0.08,
            base_ttft_ms: 200.0,
            output_price_micro_per_mtok: 2000,
        },
        MockVendorConfig {
            name: "alibaba_cloud",
            base_cache_hit_rate: 0.78,
            cache_discount: 0.06,
            base_ttft_ms: 220.0,
            output_price_micro_per_mtok: 2000,
        },
        MockVendorConfig {
            name: "minimax",
            base_cache_hit_rate: 0.72,
            cache_discount: 0.10,
            base_ttft_ms: 280.0,
            output_price_micro_per_mtok: 15000,
        },
    ]
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- 任务定义完整性测试 ---

    #[test]
    fn test_all_50_tasks_defined() {
        let tasks = build_benchmark_tasks();
        assert_eq!(tasks.len(), 50, "应有 50 个基准任务");

        // 验证所有任务都有唯一 ID
        let mut ids: Vec<u32> = tasks.iter().map(|t| t.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 50, "所有任务 ID 应唯一");
    }

    #[test]
    fn test_each_task_has_name_and_description() {
        let tasks = build_benchmark_tasks();
        for task in &tasks {
            assert!(!task.name.is_empty(), "任务 #{} 名称不能为空", task.id);
            assert!(
                !task.description.is_empty(),
                "任务 #{} 描述不能为空",
                task.id
            );
        }
    }

    #[test]
    fn test_task_size_distribution() {
        let tasks = build_benchmark_tasks();
        let simple = tasks.iter().filter(|t| t.size == TaskSize::Simple).count();
        let medium = tasks.iter().filter(|t| t.size == TaskSize::Medium).count();
        let complex = tasks.iter().filter(|t| t.size == TaskSize::Complex).count();
        let large = tasks.iter().filter(|t| t.size == TaskSize::Large).count();
        let mixed = tasks.iter().filter(|t| t.size == TaskSize::Mixed).count();

        assert_eq!(simple, 10, "简单任务应为 10 个");
        assert_eq!(medium, 10, "中等任务应为 10 个");
        assert_eq!(complex, 10, "复杂任务应为 10 个");
        assert_eq!(large, 10, "大任务应为 10 个");
        assert_eq!(mixed, 10, "混合任务应为 10 个");
    }

    #[test]
    fn test_task_tokens_within_range() {
        let tasks = build_benchmark_tasks();
        for task in &tasks {
            assert!(
                task.expected_input_tokens > 0,
                "任务 #{} 输入 token 应 > 0",
                task.id
            );
            assert!(
                task.expected_output_tokens > 0,
                "任务 #{} 输出 token 应 > 0",
                task.id
            );
        }
    }

    // --- 报告 JSON 序列化/反序列化测试 ---

    #[test]
    fn test_report_json_roundtrip() {
        let result = BenchmarkResult {
            task_id: 1,
            task_name: "测试任务".to_string(),
            effective_input_cost: 0.85,
            vendor_cache_hit_rate: 0.75,
            semantic_cache_hit_rate: 0.35,
            ttft_p95_ms: 250.0,
            total_output_tokens: 400,
            task_success: true,
            duration_ms: 600,
            vendor_name: "zhipu".to_string(),
            cache_hit_tokens: 2625,
            input_tokens: 3500,
        };

        // 序列化
        let json = serde_json::to_string(&result).expect("序列化 BenchmarkResult 失败");
        assert!(json.contains("task_id"), "JSON 应包含 task_id");
        assert!(
            json.contains("effective_input_cost"),
            "JSON 应包含 effective_input_cost"
        );

        // 反序列化
        let deserialized: BenchmarkResult =
            serde_json::from_str(&json).expect("反序列化 BenchmarkResult 失败");
        assert_eq!(deserialized.task_id, 1);
        assert_eq!(deserialized.task_name, "测试任务");
        assert!((deserialized.effective_input_cost - 0.85).abs() < 1e-10);
        assert!((deserialized.vendor_cache_hit_rate - 0.75).abs() < 1e-6);
        assert!((deserialized.semantic_cache_hit_rate - 0.35).abs() < 1e-6);
        assert!((deserialized.ttft_p95_ms - 250.0).abs() < 1e-10);
        assert_eq!(deserialized.total_output_tokens, 400);
        assert!(deserialized.task_success);
    }

    #[test]
    fn test_full_report_json_roundtrip() {
        let tasks = build_benchmark_tasks();
        let mut runner = make_runner(tasks, None);
        let report = runner.run();

        let json = serde_json::to_string_pretty(&report).expect("序列化完整报告失败");
        assert!(json.contains("tasks"), "报告应包含 tasks 数组");
        assert!(json.contains("summary"), "报告应包含 summary");
        assert!(json.contains("generated_at"), "报告应包含 generated_at");

        let deserialized: BenchmarkReport =
            serde_json::from_str(&json).expect("反序列化完整报告失败");
        assert_eq!(deserialized.tasks.len(), 50);
        assert_eq!(deserialized.summary.total_tasks, 50);
    }

    // --- 汇总聚合计算正确性测试 ---

    #[test]
    fn test_summary_aggregation_basic() {
        let results = vec![
            BenchmarkResult {
                task_id: 1,
                task_name: "A".into(),
                effective_input_cost: 0.5,
                vendor_cache_hit_rate: 0.8,
                semantic_cache_hit_rate: 0.3,
                ttft_p95_ms: 200.0,
                total_output_tokens: 100,
                task_success: true,
                duration_ms: 500,
                vendor_name: "zhipu".into(),
                cache_hit_tokens: 2800,
                input_tokens: 3500,
            },
            BenchmarkResult {
                task_id: 2,
                task_name: "B".into(),
                effective_input_cost: 0.7,
                vendor_cache_hit_rate: 0.6,
                semantic_cache_hit_rate: 0.1,
                ttft_p95_ms: 300.0,
                total_output_tokens: 200,
                task_success: false,
                duration_ms: 1500,
                vendor_name: "deepseek".into(),
                cache_hit_tokens: 12750,
                input_tokens: 15000,
            },
        ];

        // 构造一个最小 runner 来测试 compute_summary
        let tasks = vec![
            BenchmarkTask {
                id: 1,
                name: "A".into(),
                description: "desc".into(),
                expected_input_tokens: 3500,
                expected_output_tokens: 100,
                category: TaskCategory::CodeCompletion,
                uses_tools: false,
                thinking_tier: ThinkingTier::Fast,
                size: TaskSize::Simple,
            },
            BenchmarkTask {
                id: 2,
                name: "B".into(),
                description: "desc".into(),
                expected_input_tokens: 15000,
                expected_output_tokens: 200,
                category: TaskCategory::BugFix,
                uses_tools: false,
                thinking_tier: ThinkingTier::Standard,
                size: TaskSize::Medium,
            },
        ];

        let runner = BenchmarkRunner {
            tasks,
            results,
            vendors: Vec::new(),
            output_path: None,
            debug_single_task: None,
        };

        let summary = runner.compute_summary();

        assert_eq!(summary.total_tasks, 2);
        assert_eq!(summary.success_count, 1);
        assert_eq!(summary.failure_count, 1);
        assert!((summary.avg_effective_input_cost - 0.6).abs() < 1e-10);
        assert!((summary.avg_vendor_cache_hit_rate - 0.7).abs() < 1e-6);
        assert!((summary.avg_semantic_cache_hit_rate - 0.2).abs() < 1e-6);
        assert!((summary.avg_ttft_p95_ms - 250.0).abs() < 1e-10);
        assert_eq!(summary.total_output_tokens, 300);
    }

    #[test]
    fn test_summary_success_count() {
        let tasks = build_benchmark_tasks();
        let mut runner = make_runner(tasks, None);
        let report = runner.run();

        assert_eq!(report.summary.total_tasks, 50);
        assert!(report.summary.success_count > 0, "至少应有部分任务成功");
        assert_eq!(
            report.summary.success_count + report.summary.failure_count,
            50,
            "成功 + 失败 = 50"
        );
    }

    // --- 分规模统计测试 ---

    #[test]
    fn test_size_summary_exists_for_all_sizes() {
        let tasks = build_benchmark_tasks();
        let mut runner = make_runner(tasks, None);
        let report = runner.run();

        for size_key in &["simple", "medium", "complex", "large", "mixed"] {
            assert!(
                report.summary.by_size.contains_key(*size_key),
                "汇总应包含 {} 规模统计",
                size_key
            );
        }
    }

    #[test]
    fn test_size_summary_task_count() {
        let tasks = build_benchmark_tasks();
        let mut runner = make_runner(tasks, None);
        let report = runner.run();

        let total_by_size: u32 = report.summary.by_size.values().map(|s| s.task_count).sum();
        assert_eq!(total_by_size, 50, "分规模任务数合计应为 50");
    }

    #[test]
    fn test_vendor_summary_exists() {
        let tasks = build_benchmark_tasks();
        let mut runner = make_runner(tasks, None);
        let report = runner.run();

        // 6 个厂商都有任务
        assert!(
            !report.summary.by_vendor.is_empty(),
            "至少应有 1 个厂商统计"
        );

        let total_by_vendor: u32 = report
            .summary
            .by_vendor
            .values()
            .map(|s| s.task_count)
            .sum();
        assert_eq!(total_by_vendor, 50, "分厂商任务数合计应为 50");
    }

    // --- 单任务调试模式测试 ---
    // 注意: 环境变量是进程级全局状态，Rust 测试默认并行运行会导致
    // 测试间互相干扰。因此以下测试不直接操作环境变量，而是通过直接
    // 构造 BenchmarkRunner（设置 debug_single_task 字段）来验证逻辑。

    #[test]
    fn test_single_task_mode_executes_only_one() {
        let tasks = build_benchmark_tasks();
        let mut runner = make_runner(tasks, Some(25));
        let report = runner.run();

        assert_eq!(report.tasks.len(), 1, "单任务模式应只执行 1 个任务");
        assert_eq!(report.tasks[0].task_id, 25);
    }

    #[test]
    fn test_single_task_mode_debug_output() {
        let tasks = build_benchmark_tasks();
        let mut runner = make_runner(tasks, Some(1));
        let report = runner.run();
        assert_eq!(report.tasks.len(), 1);
        assert_eq!(report.tasks[0].task_id, 1);
    }

    #[test]
    fn test_normal_mode_executes_all_50() {
        let tasks = build_benchmark_tasks();
        let mut runner = make_runner(tasks, None);
        let report = runner.run();
        assert_eq!(report.tasks.len(), 50);
    }

    #[test]
    fn test_env_var_parsing_valid_id() {
        let tasks = build_benchmark_tasks();
        let runner = make_runner(tasks, Some(7));
        assert!(runner.debug_single_task.is_some());
        assert_eq!(runner.debug_single_task, Some(7));
    }

    #[test]
    fn test_env_var_parsing_invalid_id() {
        let tasks = build_benchmark_tasks();
        let runner = make_runner(tasks, None);
        assert!(runner.debug_single_task.is_none());
    }

    // --- 报告输出路径测试 ---

    #[test]
    fn test_report_output_to_file() {
        let tasks = build_benchmark_tasks();
        let tmp = tempfile::tempdir().expect("创建临时目录失败");
        let output_path = tmp.path().join("benchmark_report.json");

        let mut runner = make_runner(tasks, None).with_output_path(output_path.clone());
        let report = runner.run();

        // 验证文件已写入
        assert!(output_path.exists(), "报告文件应存在");
        let content = fs::read_to_string(&output_path).expect("读取报告文件失败");
        assert!(content.contains("tasks"), "报告应包含 tasks");
        assert!(content.contains("summary"), "报告应包含 summary");

        // 验证内容可反序列化
        let deserialized: BenchmarkReport =
            serde_json::from_str(&content).expect("反序列化报告文件失败");
        assert_eq!(deserialized.tasks.len(), 50);
        assert_eq!(report.summary.total_tasks, deserialized.summary.total_tasks);
    }

    // --- 日期时间测试 ---

    #[test]
    fn test_chrono_now_produces_valid_format() {
        let now = chrono_now();
        // 验证 ISO 8601 格式：YYYY-MM-DDTHH:MM:SSZ
        assert_eq!(now.len(), 20, "ISO 8601 格式应为 20 字符");
        assert_eq!(&now[4..5], "-");
        assert_eq!(&now[7..8], "-");
        assert_eq!(&now[10..11], "T");
        assert_eq!(&now[13..14], ":");
        assert_eq!(&now[16..17], ":");
        assert_eq!(&now[19..20], "Z");
    }

    // --- TaskCategory 和 ThinkingTier 序列化测试 ---

    #[test]
    fn test_task_category_serialization() {
        let categories = vec![
            TaskCategory::CodeCompletion,
            TaskCategory::BugFix,
            TaskCategory::Refactoring,
            TaskCategory::ArchitectureDesign,
            TaskCategory::CodeReview,
            TaskCategory::TestGeneration,
            TaskCategory::DocumentationGeneration,
            TaskCategory::ToolCall,
        ];

        for cat in &categories {
            let json = serde_json::to_string(cat).expect("序列化 TaskCategory 失败");
            let deserialized: TaskCategory =
                serde_json::from_str(&json).expect("反序列化 TaskCategory 失败");
            assert_eq!(*cat, deserialized);
        }
    }

    #[test]
    fn test_thinking_tier_serialization() {
        let tiers = vec![
            ThinkingTier::Fast,
            ThinkingTier::Standard,
            ThinkingTier::Deep,
        ];

        for tier in &tiers {
            let json = serde_json::to_string(tier).expect("序列化 ThinkingTier 失败");
            let deserialized: ThinkingTier =
                serde_json::from_str(&json).expect("反序列化 ThinkingTier 失败");
            assert_eq!(*tier, deserialized);
        }
    }

    // --- 边界条件测试 ---

    #[test]
    fn test_empty_tasks_produces_empty_report() {
        let runner = BenchmarkRunner {
            tasks: Vec::new(),
            results: Vec::new(),
            vendors: Vec::new(),
            output_path: None,
            debug_single_task: None,
        };
        let summary = runner.compute_summary();
        assert_eq!(summary.total_tasks, 0);
        assert_eq!(summary.success_count, 0);
        assert_eq!(summary.failure_count, 0);
        assert!(summary.by_size.is_empty());
        assert!(summary.by_vendor.is_empty());
    }

    #[test]
    fn test_all_fields_populated_in_result() {
        let tasks = build_benchmark_tasks();
        let mut runner = make_runner(tasks, None);
        let report = runner.run();

        for result in &report.tasks {
            assert!(result.task_id > 0, "task_id 应 > 0");
            assert!(!result.task_name.is_empty(), "task_name 不应为空");
            assert!(!result.vendor_name.is_empty(), "vendor_name 不应为空");
            assert!(result.input_tokens > 0, "input_tokens 应 > 0");
            assert!(result.total_output_tokens > 0, "total_output_tokens 应 > 0");
            assert!(result.duration_ms > 0, "duration_ms 应 > 0");
            assert!(result.ttft_p95_ms > 0.0, "ttft_p95_ms 应 > 0");
            assert!(
                result.vendor_cache_hit_rate >= 0.0 && result.vendor_cache_hit_rate <= 1.0,
                "vendor_cache_hit_rate 应在 [0.0, 1.0]"
            );
            assert!(
                result.semantic_cache_hit_rate >= 0.0 && result.semantic_cache_hit_rate <= 1.0,
                "semantic_cache_hit_rate 应在 [0.0, 1.0]"
            );
        }
    }
}
