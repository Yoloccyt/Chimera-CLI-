//! FaaE 语义路由属性测试 — 验证路由结果数与熵值不变量
//!
//! 对应 SubTask 29.6:为 faae-router 补充 proptest
//!
//! # 验证的不变量
//! 1. 路由结果数 ≤ top_k:result.candidates.len() <= top_k
//! 2. 熵值 ∈ [0.0, 1.0]:compute_entropy 返回值始终在单位区间
//! 3. 路由置信度 ∈ [0.0, 1.0]:非负向量的余弦相似度 × priority ∈ [0, 1]
//! 4. usage_count 非递减:每次路由后 usage_count 递增
//!
//! # 策略
//! - 生成合法的 top_k ∈ [1, 10]
//! - 生成合法的 n_tools ∈ [1, 20]
//! - 生成 64 维 [0, 1] 浮点向量(非负,确保 cosine similarity ∈ [0, 1])
//! - 使用 tokio::runtime::Runtime 在 proptest 中执行 async 代码

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;

use event_bus::EventBus;
use faae_router::{EdsbBalancer, ExpertProfile, FaaeConfig, FaaeRouter, ToolId};
use proptest::prelude::*;
use tokio::sync::RwLock;

/// 将任意可显示错误转换为 TestCaseError(避免 unwrap,用 ? 传播)
fn fail<E: std::fmt::Display>(e: E) -> TestCaseError {
    TestCaseError::fail(format!("{e}"))
}

/// 生成 [0.0, 1.0] 范围的 f32 策略
fn prop_unit_f32() -> impl Strategy<Value = f32> {
    any::<f32>().prop_map(|v| {
        if v.is_nan() || v.is_infinite() {
            0.5
        } else {
            v.abs().rem_euclid(1.0)
        }
    })
}

/// 构造测试用专家画像
fn make_profile(name: &str, vector: Vec<f32>, priority: f32) -> ExpertProfile {
    ExpertProfile::new(name, vector, vec!["test".into()], priority)
}

/// 构造测试用专家注册表(用于 EDSB 测试)
///
/// WHY 接受 Vec<(String, u64)>:避免 Box::leak 内存泄漏,
/// proptest 中无法使用非 'static 的 &str 引用
async fn make_profiles(counts: Vec<(String, u64)>) -> HashMap<ToolId, Arc<RwLock<ExpertProfile>>> {
    let mut map = HashMap::new();
    for (name, count) in counts {
        let profile =
            ExpertProfile::with_usage_count(name.clone(), vec![0.5; 64], vec![], 0.5, count);
        map.insert(ToolId::new(name), Arc::new(RwLock::new(profile)));
    }
    map
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 不变量 1:路由结果数 ≤ top_k
    ///
    /// 任意候选集大小,route 返回的 candidates.len() <= top_k
    /// WHY Top-K 限制:FaaE 作为精筛层,Top-K 限制确保下游不被过多候选淹没
    #[test]
    fn test_route_candidates_never_exceed_top_k(
        n_tools in 1usize..=20,
        top_k in 1usize..=10,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let config = FaaeConfig::default().with_top_k(top_k).with_balance_enabled(false);
            let router = FaaeRouter::with_config(EventBus::new(), config);

            // 注册 n_tools 个专家(正交向量,确保不同相似度)
            for i in 0..n_tools {
                let mut v = vec![0.0; 64];
                v[i % 64] = 1.0;
                router.register_expert(make_profile(&format!("t{i}"), v, 1.0)).await;
            }

            // 构造候选列表(所有已注册工具)
            let candidates: Vec<ToolId> = (0..n_tools)
                .map(|i| ToolId::new(format!("t{i}")))
                .collect();

            let clv = vec![1.0; 64];
            let result = router.route(&clv, &candidates).await.map_err(fail)?;

            // 核心不变量:candidates.len() <= top_k
            prop_assert!(
                result.candidates.len() <= top_k,
                "候选数 {} 超过 top_k={}(n_tools={})",
                result.candidates.len(),
                top_k,
                n_tools
            );
            Ok::<(), TestCaseError>(())
        })?;
    }

    /// 不变量 2:熵值 ∈ [0.0, 1.0]
    ///
    /// 任意使用计数分布,compute_entropy 返回值 ∈ [0, 1]
    /// WHY 熵值区间:熵值用于均衡决策,超出 [0, 1] 会导致阈值判断失效
    #[test]
    fn test_entropy_always_in_unit_interval(
        n_tools in 2usize..=10,
        max_count in 0u64..=1000,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let balancer = EdsbBalancer::new(FaaeConfig::default(), EventBus::new());

            // 构造随机使用计数的 profiles
            let mut counts: Vec<(String, u64)> = Vec::new();
            for i in 0..n_tools {
                // 使用确定性计数(基于 i 与 max_count)
                counts.push((format!("t{i}"), (i as u64 + 1) * max_count / n_tools as u64));
            }
            let profiles = make_profiles(counts).await;

            let entropy = balancer.compute_entropy(&profiles).await.map_err(fail)?;

            prop_assert!(
                entropy.is_finite(),
                "熵值必须为有限值,实际: {}",
                entropy
            );
            prop_assert!(
                (0.0..=1.0).contains(&entropy),
                "熵值 {} 超出 [0, 1] 区间 (n_tools={}, max_count={})",
                entropy, n_tools, max_count
            );
            Ok::<(), TestCaseError>(())
        })?;
    }

    /// 不变量 3:路由置信度 ∈ [0.0, 1.0](非负向量)
    ///
    /// WHY 置信度区间:非负向量的余弦相似度 ∈ [0, 1],
    /// 乘以 priority ∈ [0, 1] 后仍 ∈ [0, 1]
    #[test]
    fn test_route_confidence_in_unit_interval(
        n_tools in 1usize..=15,
        priority in prop_unit_f32(),
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let config = FaaeConfig::default().with_balance_enabled(false);
            let router = FaaeRouter::with_config(EventBus::new(), config);

            // 注册 n_tools 个专家(非负向量)
            for i in 0..n_tools {
                let mut v = vec![0.0; 64];
                v[i % 64] = 1.0;
                router.register_expert(make_profile(&format!("t{i}"), v, priority)).await;
            }

            let candidates: Vec<ToolId> = (0..n_tools)
                .map(|i| ToolId::new(format!("t{i}")))
                .collect();

            // 非负 CLV
            let clv = vec![1.0; 64];
            let result = router.route(&clv, &candidates).await.map_err(fail)?;

            prop_assert!(
                result.confidence.is_finite(),
                "置信度必须为有限值,实际: {}",
                result.confidence
            );
            prop_assert!(
                (0.0..=1.0).contains(&result.confidence),
                "置信度 {} 超出 [0, 1] 区间 (priority={})",
                result.confidence,
                priority
            );
            Ok::<(), TestCaseError>(())
        })?;
    }

    /// 不变量 4:usage_count 非递减
    ///
    /// 每次路由后,被路由工具的 usage_count 应递增
    /// WHY 非递减:usage_count 用于熵计算与负载均衡,
    /// 递减会导致熵计算错误
    #[test]
    fn test_usage_count_non_decreasing_after_route(
        n_routes in 1u32..=10,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let config = FaaeConfig::default().with_balance_enabled(false);
            let router = FaaeRouter::with_config(EventBus::new(), config);

            // 注册 1 个专家
            router.register_expert(make_profile("t1", vec![1.0; 64], 1.0)).await;

            let candidates = vec![ToolId::new("t1")];
            let clv = vec![1.0; 64];

            // 路由前获取 usage_count
            let registry = router.registry();
            let initial_count = {
                let reg = registry.read().await;
                // WHY 绑定到局部变量:RwLockReadGuard 临时值在块尾 drop,
                // 需在 reg 存活期间提取值,避免借用生命周期错误
                let guard = reg.get(&ToolId::new("t1")).unwrap().read().await;
                guard.get_usage_count()
            };

            // 路由 n_routes 次
            for _ in 0..n_routes {
                router.route(&clv, &candidates).await.map_err(fail)?;
            }

            // 路由后获取 usage_count
            let final_count = {
                let reg = registry.read().await;
                let guard = reg.get(&ToolId::new("t1")).unwrap().read().await;
                guard.get_usage_count()
            };

            prop_assert!(
                final_count >= initial_count + n_routes as u64,
                "usage_count 应增加至少 {},初始 {} 最终 {}",
                n_routes,
                initial_count,
                final_count
            );
            Ok::<(), TestCaseError>(())
        })?;
    }

    /// 不变量 5:候选按相似度降序排列
    ///
    /// WHY 降序排列:Top-K 候选按相似度降序,确保最相关的工具排在前面
    #[test]
    fn test_candidates_sorted_by_score_desc(
        n_tools in 2usize..=15,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let config = FaaeConfig::default().with_balance_enabled(false);
            let router = FaaeRouter::with_config(EventBus::new(), config);

            // 注册正交专家
            for i in 0..n_tools {
                let mut v = vec![0.0; 64];
                v[i % 64] = 1.0;
                router.register_expert(make_profile(&format!("t{i}"), v, 1.0)).await;
            }

            let candidates: Vec<ToolId> = (0..n_tools)
                .map(|i| ToolId::new(format!("t{i}")))
                .collect();

            // CLV 匹配 t0(第 0 维为 1)
            let mut clv = vec![0.0; 64];
            clv[0] = 1.0;
            let result = router.route(&clv, &candidates).await.map_err(fail)?;

            // 验证降序排列
            for i in 1..result.candidates.len() {
                prop_assert!(
                    result.candidates[i - 1].1 >= result.candidates[i].1,
                    "候选未按分数降序:位置 {} 分数 {} < 位置 {} 分数 {}",
                    i - 1, result.candidates[i - 1].1, i, result.candidates[i].1
                );
            }
            Ok::<(), TestCaseError>(())
        })?;
    }

    /// 不变量 7:均匀分布熵 = 1.0
    ///
    /// WHY 均匀分布:所有工具使用次数相同时,熵应为 1.0(完全均匀)
    #[test]
    fn test_uniform_distribution_entropy_is_one(
        n_tools in 2usize..=10,
        count in 1u64..=1000,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let balancer = EdsbBalancer::new(FaaeConfig::default(), EventBus::new());

            let mut counts: Vec<(String, u64)> = Vec::new();
            for i in 0..n_tools {
                counts.push((format!("t{i}"), count));
            }
            let profiles = make_profiles(counts).await;

            let entropy = balancer.compute_entropy(&profiles).await.map_err(fail)?;

            prop_assert!(
                (entropy - 1.0).abs() < 1e-5,
                "均匀分布熵应为 1.0,实际: {} (n_tools={}, count={})",
                entropy, n_tools, count
            );
            Ok::<(), TestCaseError>(())
        })?;
    }
}

// WHY 空参数测试放在 proptest! 宏外:proptest! 宏要求至少 1 个 `parm in strategy` 参数,
// 零参数函数无法匹配宏模式,因此作为普通 #[test] 编写
#[tokio::test]
async fn test_empty_candidates_returns_routing_failed() {
    let router = FaaeRouter::new(EventBus::new());
    let clv = vec![0.5; 64];

    let result = router.route(&clv, &[]).await;

    assert!(result.is_err(), "空候选集应返回错误");
}
