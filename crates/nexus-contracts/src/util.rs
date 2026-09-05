//! L0 共享纯函数(零依赖 · 零堆分配 · 零全局状态)
//!
//! 本模块收敛**跨 crate 重复出现的微观算法**,避免各 crate 各自维护本地副本而分叉口径。
//! 现存四组能力:
//!
//! | 组 | 函数 | 出处 |
//! |---|---|---|
//! | Top-K(降序) | `xts_top_k` / `xts_top_k_by` | 红线 #8(WS-2 C1) |
//! | 激活函数 | `sigmoid` | 第三轮冗余审计 批 B |
//! | 分位数(已排序切片) | `percentile_sorted` | 第三轮冗余审计 批 C |
//! | 余弦相似度 | `cosine_similarity_slices` | 第四轮冗余收敛 实施-8(自 L1 `nexus-core` 下沉) |
//!
//! **落点约定**:新增此类无副作用微观算法时优先放入本模块,不要在新 crate 再写本地副本。
//! 本模块是 ADR-033"纯类型 + 零逻辑"约束的受控例外,因此成员必须保持零 I/O、零分配、
//! 零全局状态;性能证据见 `benches/util_micro.rs`(含零堆分配硬断言)。
//!
//! `xts_top_k` / `xts_top_k_by` 以 **O(n) 部分排序**
//! (`select_nth_unstable_by`) + **O(k log k) 局部排序**取代"全排后截断"的
//! `sort_by` 全排序 (O(n log n)),用于**降序**取 Top-K。
//!
//! # Top-K 设计
//!
//! - `k` 自动钳制到切片长度(`k.min(len)`),因此 `k >= len` 等价于全量降序排序。
//! - 分区(选第 `k-1` 大)后,`[0..k)` 为降序意义下的 Top-k 无序集合;
//!   再对 `[0..k)` 做 **稳定** `sort_by(降序)`,保证返回段严格降序且
//!   与原有"全排后截断"的相等元素相对顺序完全一致(WS-2 等价性要求)。
//! - 复杂度 O(n + k log k),满足红线 #8(select_nth_unstable_by 分区)。
//!
//! # 用法
//!
//! ```
//! use nexus_contracts::util::{xts_top_k, xts_top_k_by};
//!
//! let mut v = vec![3u32, 1, 4, 1, 5, 9, 2];
//! let top = xts_top_k(&mut v, 3);
//! assert_eq!(top, &[9, 5, 4]);
//!
//! // by 版本(自定义比较器,支持 f32 等非 Ord 类型)
//! let mut scored = vec![("a", 0.1f32), ("b", 0.9), ("c", 0.5)];
//! let top = xts_top_k_by(&mut scored, 2, |a, b| {
//!     b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
//! });
//! assert_eq!(top[0].0, "b");
//! assert_eq!(top[1].0, "c");
//! ```
//!
//! > ADR-033"纯类型 + 零逻辑"约束下的纯函数工具先例(与 `test_scale` /
//! > `archive_monotonicity` / `mcsm` 同级,无 IO 无状态变更)。

use std::cmp::Ordering;

/// 降序取前 `k` 个元素(需 `T: Ord`),返回前 `k` 段切片(严格降序)。
///
/// - `k` 钳制到 `items.len()`:若 `k >= len` 则等价于全量降序排序。
/// - `k == 0` 返回空切片。
/// - 复杂度:`select_nth_unstable_by` O(n) + 前 k 段 `sort_unstable_by` O(k log k)。
///
/// f32/f64 等非 `Ord` 类型请使用 [`xts_top_k_by`] 并提供 `partial_cmp` 比较器。
#[inline]
pub fn xts_top_k<T: Ord>(items: &mut [T], k: usize) -> &mut [T] {
    xts_top_k_by(items, k, |a, b| b.cmp(a))
}

/// 降序取前 `k` 个元素(自定义比较器),返回前 `k` 段切片(按 `cmp` 序)。
///
/// - `k` 钳制到 `items.len()`:若 `k >= len` 则等价于全量降序排序。
/// - `k == 0` 返回空切片。
/// - 复杂度:`select_nth_unstable_by` O(n) + 前 k 段 `sort_unstable_by` O(k log k)。
///
/// 比较器需表达"降序"(如 f32 评分:`|a, b| b.1.partial_cmp(&a.1)...`),
/// 与原有 `sort_by` 全排后截断的语义等价。
#[inline]
pub fn xts_top_k_by<T, F>(items: &mut [T], k: usize, mut cmp: F) -> &mut [T]
where
    F: FnMut(&T, &T) -> Ordering,
{
    let n = items.len();
    let k = k.min(n);

    if k == 0 {
        return &mut items[..0];
    }

    // 分区:[0..k) 为降序意义下最大的 k 个,内部无序
    items.select_nth_unstable_by(k - 1, &mut cmp);

    // 前 k 段稳定排序为严格降序(相等元素保持相对顺序,与原 sort_by 全排语义一致)
    items[..k].sort_by(cmp);

    &mut items[..k]
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // ============================================================
    // 单测
    // ============================================================

    #[test]
    fn k_zero_returns_empty() {
        let mut v = vec![5, 3, 8, 1];
        let top = xts_top_k(&mut v, 0);
        assert!(top.is_empty());
    }

    #[test]
    fn k_equals_len_sorts_all_desc() {
        let mut v = vec![5, 3, 8, 1, 3];
        let top = xts_top_k(&mut v, 5);
        assert_eq!(top, &[8, 5, 3, 3, 1]);
    }

    #[test]
    fn k_exceeds_len_clamps_to_len() {
        let mut v = vec![2, 9, 4];
        let top = xts_top_k(&mut v, 10);
        assert_eq!(top, &[9, 4, 2]);
    }

    #[test]
    fn single_element() {
        let mut v = vec![42];
        // k=1
        assert_eq!(xts_top_k(&mut v, 1), &[42]);
        // k=0
        assert!(xts_top_k(&mut v, 0).is_empty());
        // k>len
        assert_eq!(xts_top_k(&mut v, 3), &[42]);
    }

    #[test]
    fn empty_slice() {
        let mut v: Vec<i32> = Vec::new();
        assert!(xts_top_k(&mut v, 0).is_empty());
        assert!(xts_top_k(&mut v, 5).is_empty());
    }

    #[test]
    fn all_duplicates() {
        let mut v = vec![7, 7, 7, 7, 7];
        let top = xts_top_k(&mut v, 3);
        assert_eq!(top, &[7, 7, 7]);
        assert_eq!(top.len(), 3);
    }

    #[test]
    fn already_descending_input() {
        let mut v = vec![10, 9, 8, 7, 1];
        let top = xts_top_k(&mut v, 3);
        assert_eq!(top, &[10, 9, 8]);
    }

    #[test]
    fn already_ascending_input() {
        let mut v = vec![1, 2, 3, 4, 9];
        let top = xts_top_k(&mut v, 3);
        assert_eq!(top, &[9, 4, 3]);
    }

    #[test]
    fn by_version_f32_desc() {
        let mut scored = vec![("a", 0.1f32), ("b", 0.9), ("c", 0.5), ("d", 0.9)];
        let top = xts_top_k_by(&mut scored, 3, |a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        });
        // 评分降序:0.9, 0.9, 0.5
        assert_eq!(top[0].1, 0.9);
        assert_eq!(top[1].1, 0.9);
        assert_eq!(top[2].1, 0.5);
    }

    #[test]
    fn by_version_k_clamped() {
        let mut scored = vec![(1, 0.3f32), (2, 0.1)];
        let top = xts_top_k_by(&mut scored, 100, |a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        });
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].1, 0.3);
        assert!(top.iter().all(|w| w.1 == 0.3 || w.1 == 0.1));
    }

    // ============================================================
    // proptest 属性:(1) 前 k 降序 (2) 前 k 包含全库 top-k 集合 (3) i<k≤j → v[i]≥v[j]
    // ============================================================

    proptest::proptest! {
        #![proptest_config(
            proptest::prelude::ProptestConfig { max_shrink_iters: 512, ..Default::default() }
        )]

        #[test]
        fn prop_ordering_and_partition(v: Vec<i32>, k in 0usize..40) {
            let mut v = v;
            let n = v.len();
            let k = k.min(n);

            xts_top_k(&mut v, k);

            // 性质 1:前 k 段严格降序
            if k >= 2 {
                prop_assert!(v[..k].windows(2).all(|w| w[0] >= w[1]));
            }

            // 性质 3:跨边界支配 i<k ≤ j → v[i] >= v[j]
            if k > 0 && k < n {
                let head_min = *v[..k].iter().min().unwrap();
                let tail_max = *v[k..].iter().max().unwrap();
                prop_assert!(head_min >= tail_max);
            }

            // 性质 2:前 k 集合 == 全库 top-k 集合(与全排后截断的多重集合一致)
            let mut sorted = v.clone();
            sorted.sort_unstable_by(|a, b| b.cmp(a));
            let expected = &sorted[..k];
            let mut got: Vec<i32> = v[..k].to_vec();
            got.sort_unstable_by(|a, b| b.cmp(a));
            prop_assert_eq!(&got, expected);
        }
    }

    // 与 sort_by + truncate 语义等价(批量 1000,含 by 版本)
    #[test]
    fn equivalent_to_sort_by_truncate_ord() {
        let config = proptest::test_runner::Config {
            cases: 1000,
            ..Default::default()
        };
        proptest::proptest!(config, |(v in any::<Vec<i32>>(), k in 0usize..30usize)| {
            let n = v.len();
            let k = k.min(n);

            // 参考:全排(降序)后截断
            let mut expected = v.clone();
            expected.sort_unstable_by(|a, b| b.cmp(a));
            expected.truncate(k);

            // 被测:xts_top_k
            let mut got_v = v.clone();
            let got = xts_top_k(&mut got_v, k);

            // 归一化(两者均已降序),逐元素比对
            let mut got_sorted = got.to_vec();
            got_sorted.sort_unstable_by(|a, b| b.cmp(a));
            prop_assert_eq!(&got_sorted, &expected);
        });
    }

    // 与 sort_by + truncate 语义等价(by 版本,1000 批,自定义比较器)
    #[test]
    fn equivalent_to_sort_by_truncate_by() {
        let config = proptest::test_runner::Config {
            cases: 1000,
            ..Default::default()
        };
        proptest::proptest!(config, |(v in proptest::collection::vec(any::<i32>(), 0..40), k in 0usize..30usize)| {
            // 用 (id, score) 测试 by 版,score 为 -v 以制造区分度
            let data: Vec<(usize, i32)> = v.iter().copied().enumerate().collect();
            let n = data.len();
            let k = k.min(n);
            let cmp = |a: &(usize, i32), b: &(usize, i32)| {
                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
            };

            // 参考:全排(降序)后截断
            let mut expected = data.clone();
            expected.sort_unstable_by(&cmp);
            expected.truncate(k);

            // 被测:xts_top_k_by
            let mut got_data = data.clone();
            let got = xts_top_k_by(&mut got_data, k, cmp);

            let mut got_sorted = got.to_vec();
            got_sorted.sort_unstable_by(&cmp);
            prop_assert_eq!(&got_sorted, &expected);
        });
    }
}

/// 有序切片分位数取值:`idx = round((len - 1) * p)` 并 clamp 到 `[0, len - 1]`
///
/// 全仓统一口径,替代历史上散落在 bench/test 的 14 份近重复实现
/// (索引公式存在 `round((n-1)*p)` / `trunc(n*p)` / `ceil(p*n)-1` 三种微变体,
/// 两两差异不超过 1 个样本索引)。本函数采用多数派 `round((n-1)*p)`,
/// 与 5 个 criterion bench 及 hcw-window 召回系红线测试逐位一致。
///
/// 契约:
/// - `sorted` **必须已升序排列**;本函数不排序。WHY:排序是 O(n log n) 开销,
///   是否排序、按何种键排序属调用方策略,不应隐藏在"取分位"这一步里。
/// - 空集返回 `None`,由调用方决定安全默认值(`Duration::ZERO` / `0` 等)。
/// - `p` 越界(超出 `[0,1]`)时结果 clamp 到首/尾样本,不 panic。
///
/// WHY 泛型 `T: Copy`:同时服务 `Duration`(延迟分位)与 `u64`(计数/字节分位),
/// 避免为两种元素类型各写一份逻辑相同的实现。
/// WHY 不做 f64 线性插值:既有 14 处实现均为"取实际样本点"语义,插值会改变
/// 既有 p95 红线测得值;且项目红线 §6.2 #6 禁止 f32/f64 隐式混算。
#[inline]
pub fn percentile_sorted<T: Copy>(sorted: &[T], p: f64) -> Option<T> {
    let n = sorted.len();
    if n == 0 {
        return None;
    }
    // f64→usize 的 `as` 转换在 Rust 中为饱和转换:负值归 0、超界归 usize::MAX,
    // 再由 min(n-1) 收口,故 p 为负或 >1 时安全 clamp 而非 panic。
    let idx = ((n - 1) as f64 * p).round() as usize;
    Some(sorted[idx.min(n - 1)])
}

#[cfg(test)]
mod percentile_tests {
    use super::*;
    use proptest::prelude::*;
    use std::time::Duration;

    #[test]
    fn empty_slice_returns_none() {
        let empty: &[Duration] = &[];
        assert_eq!(percentile_sorted(empty, 0.95), None);
    }

    #[test]
    fn single_element_is_always_that_element() {
        // n=1 时 (n-1)*p 恒为 0,任意分位都落在唯一样本
        let one = [Duration::from_millis(42)];
        for p in [0.0, 0.5, 0.95, 1.0] {
            assert_eq!(percentile_sorted(&one, p), Some(Duration::from_millis(42)));
        }
    }

    #[test]
    fn matches_round_formula_on_known_sample() {
        // 100 个样本 0..99 ms,p=0.95 → round(99*0.95)=round(94.05)=94
        let data: Vec<u64> = (0..100).collect();
        assert_eq!(percentile_sorted(&data, 0.95), Some(94));
        // p=0.50 → round(49.5)=50(f64::round 半值远离零)
        assert_eq!(percentile_sorted(&data, 0.50), Some(50));
        // p=0.99 → round(98.01)=98
        assert_eq!(percentile_sorted(&data, 0.99), Some(98));
    }

    #[test]
    fn out_of_range_p_clamps_instead_of_panicking() {
        let data: Vec<u64> = (0..20).collect();
        assert_eq!(percentile_sorted(&data, 0.0), Some(0));
        assert_eq!(percentile_sorted(&data, 1.0), Some(19));
        // 越界输入 clamp 到端点(系统边界容错,不 panic)
        assert_eq!(percentile_sorted(&data, -1.0), Some(0));
        assert_eq!(percentile_sorted(&data, 2.0), Some(19));
    }

    /// 属性:结果必为切片中真实存在的元素,且索引单调不减
    ///
    /// WHY 单调性:同一数据集上 p 越大分位值不应越小,这是百分位定义的
    /// 核心不变量,可捕获索引公式被改错的问题。
    #[test]
    fn monotonic_and_member_of_slice() {
        let data: Vec<u64> = (0..50).map(|i| i * 3).collect();
        let mut prev = 0u64;
        let mut p = 0.0f64;
        while p <= 1.0 + f64::EPSILON {
            let got = percentile_sorted(&data, p).expect("非空切片必有分位值");
            assert!(data.contains(&got), "分位值必须是样本集中的真实元素");
            assert!(got >= prev, "p 增大时分位值不应减小 (p={p})");
            prev = got;
            p += 0.05;
        }
    }

    /// 属性:`p` 取任意值(含越界)都不 panic,且返回值为切片真实成员
    ///
    /// WHY 覆盖越界 `p`:本函数被 6 个 p95 红线测试复用,越界输入必须 saturate
    /// 到端点而非 panic,否则会把断言失败伪装成进程崩溃而丢失定位信息。
    /// WHY 闭包内 `prop_assert!` 裸写:`proptest!` 宏自行把闭包体包成返回
    /// `TestCaseResult` 的函数,故既不能加 `?` 也不能补 `Ok(())`。
    #[test]
    fn proptest_any_p_is_total_and_returns_member() {
        let config = proptest::test_runner::Config {
            cases: 1000,
            ..Default::default()
        };
        proptest::proptest!(config, |(v in proptest::collection::vec(any::<i64>(), 1..60), p in -5.0f64..5.0f64)| {
            let mut data = v;
            data.sort_unstable();
            let got = percentile_sorted(&data, p).expect("非空切片必有分位值");
            prop_assert!(data.contains(&got));
        });
    }

    /// 属性:空集恒返 `None`;非空时 `p=0.0` 取首元素、`p=1.0` 取尾元素
    #[test]
    fn proptest_empty_contract_and_endpoint_selection() {
        let config = proptest::test_runner::Config {
            cases: 500,
            ..Default::default()
        };
        proptest::proptest!(config, |(v in proptest::collection::vec(any::<i64>(), 0..40))| {
            let empty: &[i64] = &[];
            prop_assert_eq!(percentile_sorted(empty, 0.95), None);

            let mut data = v;
            data.sort_unstable();
            if data.is_empty() {
                prop_assert_eq!(percentile_sorted(&data, 0.0), None);
                prop_assert_eq!(percentile_sorted(&data, 1.0), None);
            } else {
                prop_assert_eq!(percentile_sorted(&data, 0.0), data.first().copied());
                prop_assert_eq!(percentile_sorted(&data, 1.0), data.last().copied());
            }
        });
    }
}

/// 标准逻辑斯蒂函数:`1.0 / (1.0 + exp(-x))`
///
/// WHY 全程 f32 运算:项目红线禁止 f32 隐式转 f64(§6.2 红线 #6)。
/// 精度足够用于置信度映射与门控概率计算。
#[inline]
pub fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

#[cfg(test)]
mod sigmoid_tests {
    use super::*;

    #[test]
    fn sigmoid_zero_returns_half() {
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn sigmoid_positive_saturates_to_one() {
        assert!((sigmoid(20.0) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn sigmoid_negative_saturates_to_zero() {
        assert!(sigmoid(-20.0) < 1e-4);
    }
}

/// CI 性能阈值统一缩放因子(环境变量 `CHIMERA_PERF_SCALE`,缺省 1.0)
///
/// # 职责
/// 把"性能断言的宽松程度"从各测试文件的硬编码常量里抽成一个统一旋钮:
/// - 发布阻塞门跑 `scale = 1.0`(即契约本身);
/// - 每日观测档跑 `scale = 4`(共享 runner 噪声下先积累基线,不立刻阻塞)。
///
/// # WHY 单一因子而非"每阈值一个 env"
/// 既有 6 个 `HCW_*` 覆盖点是逐阈值可改,但止抖动时仍要人手工调每一个数——
/// `kvbsr-router/tests/scale.rs` 的加速比阈值从 >5× 退到 >3× 再退到 >2×
/// (注释自陈原因为"workspace 整体测试时资源竞争"),就是"没有隔离档 → 只能放宽阈值
/// → 阈值单调劣化"的实例。集中成一个因子后,放宽是**显式且一次性**的,
/// 而契约值永久留在代码里。命名与 `CHIMERA_TEST_TIMEOUT_SCALE` 同族。
///
/// # 失败安全
/// 未设置 / 非数字 / NaN / 无穷 / 超出 `[0.01, 100]` 一律回退 **1.0**。
/// 即"旋钮写错时跑契约本身",绝不因配置失误静默放松红线。
///
/// # 注意
/// 读进程环境变量,仅适用于测试/CI 路径;生产代码不得依赖本函数做业务决策。
#[inline]
pub fn perf_scale() -> f64 {
    /// 缺省与失败安全值:跑契约本身。
    const DEFAULT: f64 = 1.0;
    /// 下界:再小会让所有性能断言恒真(假绿),等于拆掉门。
    const MIN: f64 = 0.01;
    /// 上界:再大是明显的配置失误(100 倍放松已无测量意义)。
    const MAX: f64 = 100.0;

    match std::env::var("CHIMERA_PERF_SCALE") {
        Ok(raw) => match raw.trim().parse::<f64>() {
            Ok(v) if v.is_finite() && (MIN..=MAX).contains(&v) => v,
            _ => DEFAULT,
        },
        Err(_) => DEFAULT,
    }
}

/// 按 [`perf_scale`] 缩放一个毫秒级阈值(整数入/整数出,调用点无需浮点样板)
///
/// 结果至少为 1ms,避免 `scale` 极小时阈值被舍成 0 而变成恒假断言。
#[inline]
pub fn perf_scale_ms(base_ms: u64) -> u64 {
    ((base_ms as f64) * perf_scale()).round().max(1.0) as u64
}

#[cfg(test)]
mod perf_scale_tests {
    use super::*;

    use std::sync::Mutex;

    /// 环境变量是**进程级**的,而 `cargo test` 默认多线程跑同一 bin 内的用例
    /// （nextest 则每用例独立进程）——不串行化时两个用例会互踩同一个旋钮
    /// （本模块首次实现即因此 FAILED,左 3 右 2）。锁在 nextest 下无害。
    static PERF_SCALE_ENV_LOCK: Mutex<()> = Mutex::new(());

    /// 在锁保护下设置旋钮(`None` = 删除)并求值,退出时恢复原状(含 panic 路径)。
    ///
    /// `UnwindSafe` 约束是 `catch_unwind` 的硬性要求;本模块的闭包只读 env
    /// 与常量、不捕获可变引用,满足该约束(调用方若捕获可变引用,编译器
    /// 会直接拦下 —— 比默许“panic 可能读到不一致 env”更安全)。
    fn with_scale<T, F>(value: Option<&str>, f: F) -> T
    where
        F: FnOnce() -> T + std::panic::UnwindSafe,
    {
        let _guard = PERF_SCALE_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let saved = std::env::var("CHIMERA_PERF_SCALE").ok();
        match value {
            Some(v) => std::env::set_var("CHIMERA_PERF_SCALE", v),
            None => std::env::remove_var("CHIMERA_PERF_SCALE"),
        }
        // panic 必须跨 catch 后原样向上抛,否则断言失败会被吞成通过。
        let result = std::panic::catch_unwind(f);
        match saved {
            Some(v) => std::env::set_var("CHIMERA_PERF_SCALE", v),
            None => std::env::remove_var("CHIMERA_PERF_SCALE"),
        }
        match result {
            Ok(v) => v,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    /// 缺省与失败安全:未设置、非数字、越界、NaN 全部回退 1.0。
    #[test]
    fn perf_scale_defaults_to_one_on_unset_and_invalid_input() {
        assert_eq!(with_scale(None, perf_scale), 1.0, "未设置时应跑契约本身");

        for bogus in [
            "", "  ", "abc", "1.0.2", "nan", "inf", "-1", "0", "0.001", "1000",
        ] {
            assert_eq!(
                with_scale(Some(bogus), perf_scale),
                1.0,
                "非法值 {bogus:?} 应回退 1.0"
            );
        }

        for ok in ["1", "4", "0.5", " 2.0 ", "0.01", "100"] {
            let parsed = ok.trim().parse::<f64>().unwrap();
            assert_eq!(
                with_scale(Some(ok), perf_scale),
                parsed,
                "合法值 {ok:?} 应生效"
            );
        }
    }

    /// 整数阈值缩放的边界:四舍五入、下限 1ms、与 base=0 的退化输入。
    #[test]
    fn perf_scale_ms_rounds_and_never_collapses_to_zero() {
        assert_eq!(with_scale(Some("1"), || perf_scale_ms(10)), 10);
        assert_eq!(with_scale(Some("4"), || perf_scale_ms(50)), 200);
        assert_eq!(with_scale(Some("4"), || perf_scale_ms(3)), 12);

        // 小数量化必须向上取整到 2(而非舍成 1 或 0)：0.5 × 3 = 1.5
        assert_eq!(with_scale(Some("0.5"), || perf_scale_ms(3)), 2);

        // 极小 scale 不得把阈值舍成 0(会变成恒假断言,比假绿更隐蔽)
        assert_eq!(with_scale(Some("0.01"), || perf_scale_ms(10)), 1);
        assert_eq!(with_scale(Some("0.01"), || perf_scale_ms(0)), 1);
    }
}

/// 计算两个 f32 切片的余弦相似度(**f32 切片域**的唯一权威实现)
///
/// 全仓余弦实现现状:
/// - 本函数 = 所有 `&[f32]` 相似度计算的唯一权威(12+ crate 经
///   `nexus_core::cosine_similarity_slices` 重导出或直接引用 L0 路径消费);
/// - **曾登记例外已取消**:`osa-coordinator::cheap_index::cosine_similarity`(f64 域变体)
///   已于 2026-09-03 架构批次死模块处置中删除,本函数恢复为全仓 `&[f32]` 相似度唯一权威。
///
/// 公式:dot(a, b) / (|a| * |b|)
///
/// # 返回值
/// 返回值 ∈ [-1.0, 1.0],通过 `clamp` 钳制浮点误差导致的微小越界。
///
/// # 溢出降级
/// 分量达 ~1e19 量级时平方和溢出为 inf,归一化结果为非有限值;
/// 此时**返回 0.0 而非 NaN**。WHY:`f32::clamp` 对 NaN 的两侧比较均为 false,
/// 会把 NaN **原样返回**并泄进下游 Top-K 排序(比较结果不确定 → 顺序不稳定);
/// 与零向量取同一 fail-safe 口径——方向信息已不可信的向量判"不相似"最安全。
/// 已知代价:`cos(v, v)` 在 v 溢出时返回 0.0 而非 1.0。可接受,因 CLV 由编码器
/// 产出且经归一化,分量逼近 f32::MAX 属异常上游而非正常语义。
///
/// # 零向量处理
/// 若任一向量为零向量(|a|==0 或 |b|==0),返回 0.0 而非 NaN。
/// WHY 统一零向量处理:避免不同 crate 返回 NaN 导致下游计算异常
/// (如路由评分 NaN 导致排序异常)。
///
/// # 不等长输入
/// 取两个切片的最小长度计算(兼容不等长输入,最安全)。
/// 调用方若需严格等长校验,应在调用前自行断言。
///
/// # 设计决策(WHY)
/// SubTask 21.4 — mlc-engine(types.rs)、kvbsr-router(blocks.rs)、
/// repo-wiki(vector.rs)三处重复实现余弦相似度,且零向量处理策略不一致。
/// 提取到 L1 Core 统一行为,消除约 80 行重复代码。
///
/// **落点演进(第四轮冗余收敛 实施-8)**:该函数被 L0 `nexus-contracts::vector`
/// 的契约测试需要,而 L0 禁止依赖 L1,历史上只能靠 dev-dependency 反向伸手拿 L1
/// 实现。现已把**定义**下沉到 L0 util(与本模块其余共享纯函数同级),
/// L1 `nexus_core::cosine_similarity_slices` 保留 `pub use` 重导出,
/// 全部既有调用路径(30+ 处)与 `nexus-core/tests/proptest.rs` 断言零改动。
///
/// # 示例
/// ```
/// use nexus_contracts::util::cosine_similarity_slices;
///
/// // 相同向量余弦相似度为 1.0
/// let v = vec![1.0, 2.0, 3.0];
/// let sim = cosine_similarity_slices(&v, &v);
/// assert!((sim - 1.0).abs() < 1e-5);
///
/// // 零向量返回 0.0(非 NaN)
/// let zero = vec![0.0, 0.0, 0.0];
/// assert_eq!(cosine_similarity_slices(&zero, &v), 0.0);
/// ```
#[inline]
pub fn cosine_similarity_slices(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    if len == 0 {
        return 0.0;
    }

    // ── P2-10: chunks_exact(4) + 4 路累加器优化 ──────────────────────
    //
    // WHY chunks_exact(4) 替代手动索引(P2-10 优化):
    //
    // 1. **消除冗余边界检查**: `chunks_exact(4)` 返回的每个 chunk 保证
    //    长度为 4,LLVM 通过 inter-procedural analysis 可消除 `ca[0..3]`
    //    的部分 bounds check。旧实现的手动索引 `a[base + 1]` 每次访问
    //    都生成独立边界检查指令(4 次/迭代 × 3 变量 = 12 次/迭代)。
    //
    // 2. **促进 LLVM auto-vectorization**: `chunks_exact + zip` 是 LLVM
    //    识别的标准 SIMD 友好模式,可将 4 路 FMA 编译为 SIMD 指令:
    //    - AVX2: `VFMADD213PS`(256-bit, 8 float/cycle)
    //    - SSE2: `MULPS + ADDPS`(128-bit, 4 float/cycle)
    //
    // 3. **单 pass 三计算**: dot + norm_a + norm_b 在同一循环内完成,
    //    优化 L1 缓存利用率(512-dim f32 = 2KB, 完全在 L1 内)。
    //
    // 4. **4 路累加器打破依赖链**: 单累加器 `acc += x` 形成循环依赖链
    //    (每次 FMA 依赖上次结果,延迟 ~4 cycles)。4 路独立累加器让 CPU
    //    可并行执行 4 条 FMA 指令,充分利用流水线。
    //
    // 约束: 纯 safe Rust,无 unsafe / intrinsics(forbid(unsafe_code) 合规)。
    // 实测(P2-10 criterion benchmark, 2026-07-28):
    //   512d: 117ns → 28ns(76% ↓, 4.2× 加速)
    //   1024d: 240ns → 53ns(78% ↓, 4.5× 加速)
    //   接近理论 4× 极限(SIMD 4-float/cycle),证明 LLVM 已 auto-vectorize
    let (mut dot0, mut dot1, mut dot2, mut dot3) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let (mut na0, mut na1, mut na2, mut na3) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let (mut nb0, mut nb1, mut nb2, mut nb3) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);

    // 主循环: chunks_exact(4) 保证每个 chunk 长度为 4,消除冗余边界检查
    // 注意: a[..len] 取最小长度子切片,确保两切片等长
    let a_slice = &a[..len];
    let b_slice = &b[..len];
    for (ca, cb) in a_slice.chunks_exact(4).zip(b_slice.chunks_exact(4)) {
        // ca, cb: &[f32] 长度保证为 4,LLVM 可优化边界检查
        dot0 += ca[0] * cb[0];
        dot1 += ca[1] * cb[1];
        dot2 += ca[2] * cb[2];
        dot3 += ca[3] * cb[3];

        na0 += ca[0] * ca[0];
        na1 += ca[1] * ca[1];
        na2 += ca[2] * ca[2];
        na3 += ca[3] * ca[3];

        nb0 += cb[0] * cb[0];
        nb1 += cb[1] * cb[1];
        nb2 += cb[2] * cb[2];
        nb3 += cb[3] * cb[3];
    }

    // 合并 4 路累加器
    let mut dot = dot0 + dot1 + dot2 + dot3;
    let mut norm_a = na0 + na1 + na2 + na3;
    let mut norm_b = nb0 + nb1 + nb2 + nb3;

    // 尾部处理: chunks_exact 的 remainder(len % 4 非 0 时逐元素补算)
    let processed = (len / 4) * 4;
    for i in processed..len {
        let (ai, bi) = (a[i], b[i]);
        dot += ai * bi;
        norm_a += ai * ai;
        norm_b += bi * bi;
    }

    let norm_a = norm_a.sqrt();
    let norm_b = norm_b.sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    let sim = dot / (norm_a * norm_b);
    // 溢出降级:分量达 ~1e19 量级时平方和溢出为 inf,sim 变成 inf/inf = NaN。
    // 而 f32::clamp 对 NaN 的两侧比较均为 false 会**原样返回 NaN**,
    // 直接违反本函数"返回值 ∈ [-1.0, 1.0]"契约,并把 NaN 泄进下游排序
    // (比较结果不确定 → Top-K 顺序不稳定)。取 0.0 与零向量同一 fail-safe 口径:
    // 溢出向量的方向信息已不可信,判为"不相似"比判出一个毒化排序的 NaN 更安全。
    if !sim.is_finite() {
        return 0.0;
    }
    sim.clamp(-1.0, 1.0)
}

#[cfg(test)]
mod cosine_tests {
    use super::cosine_similarity_slices;
    // proptest! 块内的 prop_assert! / prop_assert_eq! 依赖 prelude 导入
    // (与本文件 `mod tests` 同一模式)
    use proptest::prelude::*;

    // ── 正常路径 ──────────────────────────────────────────────

    #[test]
    fn identical_vectors_give_one() {
        let v = [1.0f32, 2.0, 3.0, 4.0];
        assert!((cosine_similarity_slices(&v, &v) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn orthogonal_vectors_give_zero() {
        let a = [1.0f32, 0.0, 0.0, 0.0];
        let b = [0.0f32, 1.0, 0.0, 0.0];
        assert_eq!(cosine_similarity_slices(&a, &b), 0.0);
    }

    #[test]
    fn opposite_vectors_give_minus_one() {
        let a = [1.0f32, 2.0, 3.0];
        let b = [-1.0f32, -2.0, -3.0];
        assert!((cosine_similarity_slices(&a, &b) + 1.0).abs() < 1e-5);
    }

    /// 手工核对已知值:dot=2*1+3*2=8, |a|=sqrt(13), |b|=sqrt(5)
    #[test]
    fn matches_hand_computed_value() {
        let a = [2.0f32, 3.0];
        let b = [1.0f32, 2.0];
        let expect = 8.0 / (13.0f32.sqrt() * 5.0f32.sqrt());
        assert!((cosine_similarity_slices(&a, &b) - expect).abs() < 1e-6);
    }

    // ── 边界路径 ──────────────────────────────────────────────

    #[test]
    fn empty_inputs_give_zero() {
        assert_eq!(cosine_similarity_slices(&[], &[]), 0.0);
        assert_eq!(cosine_similarity_slices(&[1.0], &[]), 0.0);
        assert_eq!(cosine_similarity_slices(&[], &[1.0]), 0.0);
    }

    #[test]
    fn single_element_vectors() {
        assert!((cosine_similarity_slices(&[3.0], &[5.0]) - 1.0).abs() < 1e-6);
        assert_eq!(cosine_similarity_slices(&[3.0], &[-5.0]), -1.0);
        assert_eq!(cosine_similarity_slices(&[0.0], &[5.0]), 0.0);
    }

    /// 长度 4 的整数倍与非整数倍都必须走通(chunks_exact(4) 主循环 + 尾部补算)
    #[test]
    fn tail_lengths_are_all_handled() {
        for len in 1..=17usize {
            let a: Vec<f32> = (0..len).map(|i| 1.0 + i as f32 * 0.25).collect();
            let same = cosine_similarity_slices(&a, &a);
            assert!(
                (same - 1.0).abs() < 1e-5,
                "len={len} 自相似度应 ~1.0,实际 {same}(尾部路径可能失通)"
            );
        }
    }

    #[test]
    fn unequal_lengths_use_shorter_prefix() {
        // 不等长语义:只用前 min(len_a, len_b) 维,后段一律忽略
        let base_a = [1.0f32, 2.0];
        let base_b = [3.0f32, 4.0];
        let expect = cosine_similarity_slices(&base_a, &base_b);

        // 各补两个"脏"分量:若被参与计算,结果必然改变
        let long_a = [1.0f32, 2.0, 99.0, -50.0];
        let long_b = [3.0f32, 4.0, -77.0, 12.0];
        assert!(
            (cosine_similarity_slices(&long_a, &base_b) - expect).abs() < 1e-6,
            "a 更长时应截断到 b 的长度"
        );
        assert!(
            (cosine_similarity_slices(&base_a, &long_b) - expect).abs() < 1e-6,
            "b 更长时应截断到 a 的长度"
        );
        // 注:两侧等长时全部维度都参与计算(不属"截断"语义),
        // 该性质由 proptest `prop_truncation_equivalence` 覆盖。
    }

    // ── 异常/防御路径 ─────────────────────────────────────────

    #[test]
    fn zero_vector_yields_zero_not_nan() {
        let zero = [0.0f32, 0.0, 0.0, 0.0];
        let v = [1.0f32, 2.0, 3.0, 4.0];
        assert_eq!(cosine_similarity_slices(&zero, &v), 0.0);
        assert_eq!(cosine_similarity_slices(&v, &zero), 0.0);
        assert_eq!(cosine_similarity_slices(&zero, &zero), 0.0);
    }

    /// 极小量级向量不应因下溢被判成零向量
    #[test]
    fn subnormal_scale_still_reports_one() {
        let a = [1e-20f32, 2e-20, 3e-20];
        let got = cosine_similarity_slices(&a, &a);
        assert!(
            (got - 1.0).abs() < 1e-5,
            "1e-20 量级自相似度应 ~1.0,实际 {got}"
        );
    }

    #[test]
    fn result_is_never_nan() {
        let pathological: &[&[f32]] = &[
            &[],
            &[0.0],
            &[f32::MAX, f32::MAX],
            &[f32::MIN_POSITIVE, f32::MIN_POSITIVE],
            &[1e30, -1e30, 1e30],
        ];
        for a in pathological {
            for b in pathological {
                let got = cosine_similarity_slices(a, b);
                assert!(!got.is_nan(), "cos({a:?}, {b:?}) 产生 NaN");
            }
        }
    }

    /// 超大分量会让平方和溢出为 inf → 归一化后得 NaN 语义。
    /// 契约:溢出时 clamp 而非返回 NaN(下游路由评分依赖非 NaN)。
    #[test]
    fn overflow_input_is_clamped_not_nan() {
        let a = [f32::MAX, f32::MAX];
        let got = cosine_similarity_slices(&a, &a);
        assert!(!got.is_nan(), "f32::MAX 自相似产生 NaN");
        assert!((-1.0..=1.0).contains(&got), "结果越界: {got}");
    }

    // ── proptest 属性 ─────────────────────────────────────────

    proptest::proptest! {
        /// 值域恒定落在 [-1, 1] 且永不为 NaN —— 任意实数向量(含全零/不等长)
        #[test]
        fn prop_bounded_and_finite(
            a in proptest::collection::vec(-1e6f32..1e6f32, 0..40),
            b in proptest::collection::vec(-1e6f32..1e6f32, 0..40),
        ) {
            let got = cosine_similarity_slices(&a, &b);
            prop_assert!(!got.is_nan());
            prop_assert!((-1.0..=1.0).contains(&got));
        }

        /// 对称性:cos(a,b) == cos(b,a) 逐位相等(点积与范数均对称)
        #[test]
        fn prop_symmetric(
            a in proptest::collection::vec(-1e3f32..1e3f32, 1..32),
            b in proptest::collection::vec(-1e3f32..1e3f32, 1..32),
        ) {
            prop_assert_eq!(
                cosine_similarity_slices(&a, &b),
                cosine_similarity_slices(&b, &a)
            );
        }

        /// 非零向量自相似度恒为 1.0(归一化不变量)
        #[test]
        fn prop_self_similarity_is_one_for_nonzero(
            v in proptest::collection::vec(0.1f32..1e3f32, 1..64),
        ) {
            let got = cosine_similarity_slices(&v, &v);
            prop_assert!((got - 1.0).abs() < 1e-5, "自相似度 {got} 偏离 1.0");
        }

        /// 不等长语义:cos(a, b) == cos(a[..min], b[..min]) 逐位相等
        #[test]
        fn prop_truncation_equivalence(
            a in proptest::collection::vec(-1e3f32..1e3f32, 1..48),
            b in proptest::collection::vec(-1e3f32..1e3f32, 1..48),
        ) {
            let min = a.len().min(b.len());
            prop_assert_eq!(
                cosine_similarity_slices(&a, &b),
                cosine_similarity_slices(&a[..min], &b[..min])
            );
        }

        /// 零向量任一侧恒 0.0(除零防御不变量)
        #[test]
        fn prop_zero_vector_is_zero(
            v in proptest::collection::vec(-1e3f32..1e3f32, 0..48),
        ) {
            let zero = vec![0.0f32; v.len()];
            prop_assert_eq!(cosine_similarity_slices(&zero, &v), 0.0);
            prop_assert_eq!(cosine_similarity_slices(&v, &zero), 0.0);
        }

        /// 正交性保持:a 与 b 支撑集不相交时相似度必为 0
        #[test]
        fn prop_disjoint_support_is_orthogonal(k in 1usize..24) {
            let mut a = vec![0.0f32; 2 * k];
            let mut b = vec![0.0f32; 2 * k];
            for i in 0..k {
                a[i] = 1.0 + i as f32;
                b[k + i] = 2.0 + i as f32;
            }
            prop_assert_eq!(cosine_similarity_slices(&a, &b), 0.0);
        }
    }
}
