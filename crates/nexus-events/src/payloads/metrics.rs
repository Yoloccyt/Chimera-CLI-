//! 面板结构化指标载荷
//!
//! WHY 独立成模块：本组类型只服务于 `面板结构化指标载荷` 一个领域，
//! 与其它域的载荷无耦合（同文件平铺时改任一领域都要翻找全部类型）。
//!
//! 对外路径不变：全部类型经 `payloads/mod.rs` 的 facade 转出，
//! `event_bus::payloads::X` 逐字不变（ADR-314 W6 / BC-03 域下沉）。
//!
//! 类型：`BudgetMetricsPayload` / `RouterStatsPayload` / `ClvSummary`。
//!
//! WHY `xts_top_k_by`：Top-8 取值走 L0 公共 Top-K 收敛工具
//! （O(n) 部分排序；架构红线要求禁止 sort_by O(n log n) 做 Top-K）。

use nexus_contracts::util::xts_top_k_by;
use serde::{Deserialize, Serialize};

/// 预算指标载荷 — TUI Budget 面板的结构化数据(P1.2 实时数据驱动面板)
///
/// WHY 定义在 event-bus:chimera-tui(L10)无法直接依赖 efficiency-monitor(L9),
/// 通过 event-bus(L1)传递结构化预算指标,避免面板侧从多个事件拼合。
/// 字段与 `chimera_tui::data::BudgetMetrics` 保持一致。
/// 注:此类型属于 P1.2 实时数据面板契约,非 M4 双向控制新增。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BudgetMetricsPayload {
    /// 总消耗量(单位由预算类型决定)
    pub total_consumption: f64,
    /// 剩余预算
    pub remaining_budget: f64,
    /// 利用率 [0.0, 1.0]
    pub utilization_rate: f32,
    /// 当前预算档位(如 "High"/"Medium"/"Low")
    pub current_tier: String,
    /// 档位系数,1.0 为基准
    pub coefficient: f32,
    /// 是否已触发预算超限
    pub is_exceeded: bool,
    /// 最新告警信息(无告警为 None)
    pub alert: Option<String>,
}

/// 路由器统计载荷 — 三路由器(KVBSR/SESA/FaaE)的统一统计格式
///
/// WHY 定义在 event-bus:chimera-tui(L10)无法直接依赖 L6 的 kvbsr-router/
/// sesa-router/faae-router,通过 event-bus(L1)传递结构化路由统计,
/// 避免面板侧从多个事件拼合,也避免 L10→L6 类型泄漏。
/// 由 L9 efficiency-monitor 聚合三路由器指标后统一发布。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouterStatsPayload {
    /// 命中率 [0.0, 1.0]
    pub hit_rate: f32,
    /// P50 延迟(微秒)
    pub p50_latency_us: u64,
    /// P95 延迟(微秒)
    pub p95_latency_us: u64,
    /// P99 延迟(微秒)
    pub p99_latency_us: u64,
    /// 热点能力列表(能力 ID,调用次数),按热度降序
    pub hot_capabilities: Vec<(String, u64)>,
}

/// CLV 摘要载荷 — TUI ClvVector 面板的结构化数据
///
/// WHY 定义在 event-bus:chimera-tui(L10)需要展示 CLV 摘要,
/// 但不能携带完整 512 维向量(性能负担),通过此摘要结构传递
/// 8 分块均值 + L2 范数 + Top-8 维度索引,足以可视化向量分布。
/// `from_clv_slice` 接受 f32 切片(调用方传 `clv.as_slice()`),
/// 保持 event-bus 对 nexus-core 零依赖(P1-2 解耦,L1 Core 深度优化)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClvSummary {
    /// 8 分块均值(每块 64 维,512/8=64)
    /// 索引 0 = 维度 [0..64),索引 7 = 维度 [448..512)
    pub block_means: Vec<f32>,
    /// L2 范数 = sqrt(sum(v_i^2))
    pub l2_norm: f32,
    /// Top-8 维度(维度索引, 值),按 |值| 降序排列
    /// 长度 ≤ 8(向量维度不足 8 时返回全部)
    pub top_dims: Vec<(usize, f32)>,
}

impl ClvSummary {
    /// 从 CLV 向量切片计算摘要
    ///
    /// 计算:
    /// 1. **8 分块均值**: 将向量分为 8 块(512 维时每块 64 维),计算每块算术均值
    /// 2. **L2 范数**: sqrt(sum(v_i^2))
    /// 3. **Top-8 维度**: 按 |v_i| 降序选取前 8 个(维度索引, 值)
    ///
    /// WHY 切片签名(P1-2 解耦): 原签名 `from_clv(&nexus_core::clv::CLV)` 是
    /// event-bus 对 nexus-core 唯一生产依赖边(Cargo.toml 注释已预设解除条件)。
    /// 改签为 `&[f32]` 后 event-bus 零 nexus-core 依赖,调用方传
    /// `clv.as_slice()` 即可(生产调用方为 0,直接改签无兼容负担)。
    ///
    /// # 算法选择
    /// - 分块均值: O(n) 遍历,每块累加后除以块长
    /// - L2 范数: O(n) 遍历,累加 v_i^2 后开方
    /// - Top-8: 使用 `select_nth_unstable_by` O(n) 算法(架构红线要求,
    ///   禁止 sort_by O(n log n) 做 Top-K);Top-k 内部排序(k ≤ 8,成本可忽略)
    ///
    /// # 边界处理
    /// - 空/过短切片(长度 < 8): block_size = 0,block_means 返回空 Vec(防除零/越界)
    /// - 零向量: l2_norm = 0.0 + 全 0 block_means + 空 top_dims(无显著维度)
    ///
    /// # 参数
    /// slice: CLV 向量的 f32 切片(nexus_core::CLV::as_slice(),512 维为标准形态)
    ///
    /// # 返回
    /// ClvSummary 实例
    pub fn from_clv_slice(slice: &[f32]) -> Self {
        // 1. 计算 8 分块均值(512 维时每块 64 维)
        // 边界:长度 < 8 时 block_size = 0,跳过分块(防 slice 越界与除零)
        let block_size = slice.len() / 8;
        let block_means: Vec<f32> = if block_size == 0 {
            Vec::new()
        } else {
            (0..8)
                .map(|i| {
                    let start = i * block_size;
                    let end = start + block_size;
                    let sum: f32 = slice[start..end].iter().sum();
                    sum / block_size as f32
                })
                .collect()
        };

        // 2. 计算 L2 范数 = sqrt(sum(v_i^2))
        let sum_sq: f32 = slice.iter().map(|&v| v * v).sum();
        let l2_norm = sum_sq.sqrt();

        // 3. 计算 Top-8 维度(按 |值| 降序)
        // 零向量边界:l2_norm == 0 表示全零,无显著维度,返回空
        let top_dims = if l2_norm == 0.0 {
            Vec::new()
        } else {
            // 构造 (维度索引, |值|, 值) 元组向量
            let mut indexed: Vec<(usize, f32, f32)> = slice
                .iter()
                .enumerate()
                .map(|(i, &v)| (i, v.abs(), v))
                .collect();

            let k = 8.min(indexed.len());
            if k == 0 {
                Vec::new()
            } else {
                // 公共 xts_top_k_by:O(n) 部分排序 + 前 k 段降序(红线 #8 ws2-c1)
                let top = xts_top_k_by(&mut indexed, k, |a, b| {
                    b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                });

                // 转换为 (维度索引, 值) 格式(k ≤ 8,take 8 为 no-op)
                top.iter().take(8).map(|&(i, _abs, v)| (i, v)).collect()
            }
        };

        ClvSummary {
            block_means,
            l2_norm,
            top_dims,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn budget_metrics_payload_field_names_are_frozen() {
        // 字段名是跨进程线格式契约:efficiency-monitor 聚合后经 event-bus
        // 交给 TUI 面板,改名会让消费端静默失配(默认值或反序列化错误)。
        // 此处硬编码完整期望串,JSON 片段即契约本体。
        let payload = BudgetMetricsPayload {
            total_consumption: 10.0,
            remaining_budget: 2.5,
            utilization_rate: 0.8,
            current_tier: "High".into(),
            coefficient: 1.0,
            is_exceeded: false,
            alert: None,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert_eq!(
            json,
            r#"{"total_consumption":10.0,"remaining_budget":2.5,"utilization_rate":0.8,"current_tier":"High","coefficient":1.0,"is_exceeded":false,"alert":null}"#
        );
        let back: BudgetMetricsPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(back, payload);
    }

    #[test]
    fn router_stats_nested_hot_capabilities_roundtrip() {
        let payload = RouterStatsPayload {
            hit_rate: 0.75,
            p50_latency_us: 10,
            p95_latency_us: 20,
            p99_latency_us: 30,
            hot_capabilities: vec![("grep".into(), 7), ("edit".into(), 3)],
        };
        let value = serde_json::to_value(&payload).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 5);
        // (String, u64) 元组在 JSON 里编码为二元数组,顺序即 (能力, 调用次数)
        assert_eq!(value["hot_capabilities"][0][0], serde_json::json!("grep"));
        assert_eq!(value["hot_capabilities"][0][1], serde_json::json!(7));
        let back: RouterStatsPayload =
            serde_json::from_str(&serde_json::to_string(&payload).unwrap()).unwrap();
        assert_eq!(back, payload);
    }

    #[test]
    fn clv_summary_block_means_norm_and_top_dims_roundtrip() {
        // 16 维切片:block_size = 2,块均值可手算片上验证
        let slice: Vec<f32> = (1..=16).map(|v| v as f32).collect();
        let summary = ClvSummary::from_clv_slice(&slice);
        assert_eq!(summary.block_means.len(), 8);
        assert_eq!(summary.block_means[0], 1.5);
        assert_eq!(summary.block_means[7], 15.5);
        assert!((summary.l2_norm - 1496.0_f32.sqrt()).abs() < 1e-3);
        assert_eq!(summary.top_dims.len(), 8);
        assert_eq!(summary.top_dims[0], (15, 16.0));

        // 嵌套结构线格式:top_dims 是 [[维度索引, 值], ...]
        let value = serde_json::to_value(&summary).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 3);
        assert!(value["block_means"].is_array());
        assert!(value["l2_norm"].is_number());
        assert_eq!(value["top_dims"][0][0], serde_json::json!(15));
        assert_eq!(value["top_dims"][0][1], serde_json::json!(16.0));

        let back: ClvSummary =
            serde_json::from_str(&serde_json::to_string(&summary).unwrap()).unwrap();
        assert_eq!(back, summary);

        // 边界:空切片不得除零/越界,且零向量无显著维度
        let empty = ClvSummary::from_clv_slice(&[]);
        assert!(empty.block_means.is_empty());
        assert_eq!(empty.l2_norm, 0.0);
        assert!(empty.top_dims.is_empty());
    }
}
