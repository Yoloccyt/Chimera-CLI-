//! nmc-encoder 多模态融合面属性测试 — 嵌入不变量的"任意输入"验证
//!
//! 对应架构层: L2 Memory(nmc-encoder,NMC 多模态编码)
//!
//! ## 覆盖的不变量(权威源:`lib.rs` / `fusion.rs` 文档断言"输出维度严格 512")
//! 1. **维度闭合**:任意元素集合(含空、含超长 embedding、含 NaN/Inf 位模式)× 三种融合策略
//!    ⇒ `fuse` 恒 `Ok`,且 `dimension() == 512 == as_slice().len()`。
//! 2. **无 NaN 传染**:输入全部有限 ⇒ 输出不含 NaN
//!    (f32 求和可溢出为 ±Inf,但 Inf 仍可比较;**NaN 会静默毒化整条 CLV**)。
//! 3. **截断/填充语义**:单元素 ⇒ 前 `len` 维保真、其余补 0。
//! 4. **空输入零向量**:无元素时输出恒为 512 维全零(三种策略同口径)。
//! 5. **确定性(Ω₂)**:同输入两次融合逐位(`to_bits`)相同。
//!
//! ## 语法约束(§4.1 规范)
//! proptest 1.11+ 用 block-named 语法:`fn name(arg in strategy) { body }`
//!
//! ## 策略空间的取舍
//! `MultimodalFusionEngine::new` 不调用 `NmcConfig::validate()`(后者才校验 `clv_dim`),
//! 故属性 1–5 一律使用**已校验的默认 `clv_dim = 512`**;`clv_dim != 512` 的行为分叉
//! 由 [`misconfigured_clv_dim_is_rejected_only_on_non_empty_input`] 单独见证。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use nmc_encoder::error::NmcError;
use nmc_encoder::types::{ClvOutput, CognitiveElement, Modality};
use nmc_encoder::{FusionStrategy, MultimodalFusionEngine, NmcConfig};
use proptest::prelude::*;

/// CLV 维度(与 `nexus_core::CLV::DIMENSION` 对齐)
const CLV_DIM: usize = 512;

// ============================================================
// 辅助策略(Strategy)
// ============================================================

/// 生成任意模态 —— 覆盖全部 5 个变体(Weighted 策略按模态权重分支)
fn arb_modality() -> impl Strategy<Value = Modality> {
    prop_oneof![
        Just(Modality::Text),
        Just(Modality::Image),
        Just(Modality::Video),
        Just(Modality::Audio),
        Just(Modality::Desktop),
    ]
}

/// 生成"温和"有限标量 —— 用于需要保真度比对的属性
fn arb_moderate_scalar() -> impl Strategy<Value = f32> {
    (-1_000_000i32..1_000_000i32).prop_map(|v| v as f32)
}

/// 生成**有限** f32 标量 —— 含极大值,使求和溢出(±Inf)可达,但不含 NaN/Inf
fn arb_finite_scalar() -> impl Strategy<Value = f32> {
    prop_oneof![
        arb_moderate_scalar(),
        Just(0.0f32),
        Just(-0.0f32),
        Just(f32::MIN_POSITIVE),
        Just(f32::EPSILON),
        Just(1e30f32),
        Just(-1e30f32),
        Just(f32::MAX),
        Just(f32::MIN),
    ]
}

/// 生成任意位模式标量(含 NaN/Inf)—— 属性 1 的输入域不设有限性前提
fn arb_any_scalar() -> impl Strategy<Value = f32> {
    prop_oneof![
        arb_finite_scalar(),
        Just(f32::NAN),
        Just(f32::INFINITY),
        Just(f32::NEG_INFINITY),
    ]
}

/// 生成任意 embedding —— 长度覆盖 0(空感知器输出)与跨过 512(触发截断)
fn arb_embedding(scalars: impl Strategy<Value = f32>) -> impl Strategy<Value = Vec<f32>> {
    prop::collection::vec(scalars, 0..600)
}

/// 生成任意认知元素(有限值)
fn arb_element() -> impl Strategy<Value = CognitiveElement> {
    (
        arb_modality(),
        "[a-f0-9]{0,12}",
        arb_embedding(arb_finite_scalar()),
    )
        .prop_map(|(modality, content_hash, embedding)| {
            CognitiveElement::new(modality, content_hash, embedding)
        })
}

/// 生成任意认知元素(任意位模式)
fn arb_element_any_bits() -> impl Strategy<Value = CognitiveElement> {
    (
        arb_modality(),
        "[a-f0-9]{0,12}",
        arb_embedding(arb_any_scalar()),
    )
        .prop_map(|(modality, content_hash, embedding)| {
            CognitiveElement::new(modality, content_hash, embedding)
        })
}

/// 生成任意融合策略 —— 覆盖 Concat/Mean/Weighted 三条分支
fn arb_strategy() -> impl Strategy<Value = FusionStrategy> {
    prop_oneof![
        Just(FusionStrategy::Concat),
        Just(FusionStrategy::Mean),
        Just(FusionStrategy::Weighted),
    ]
}

/// 以指定策略构造融合引擎(配置经 `validate()` 自检,默认 `clv_dim = 512`)
fn engine(strategy: FusionStrategy) -> MultimodalFusionEngine {
    let cfg = NmcConfig::default().with_fusion_strategy(strategy);
    assert!(cfg.validate().is_ok(), "测试前提:配置必须合法");
    MultimodalFusionEngine::new(cfg)
}

/// 融合并断言成功(有效配置下 `fuse` 不应报错——属性 1 正是这一点的通用化)
fn fuse_ok(strategy: FusionStrategy, elements: Vec<CognitiveElement>) -> ClvOutput {
    engine(strategy)
        .fuse(elements)
        .expect("有效配置(clv_dim=512)下 fuse 必须成功")
}

proptest! {
    // ============================================================
    // 属性 1:维度闭合(与输入位模式无关)
    // ============================================================

    /// 任意元素集合 × 任意策略 ⇒ 输出维度恒为 512
    ///
    /// WHY 输入域含 NaN/Inf:维度不变量是最硬的契约,不该依赖"输入恰好正常"。
    /// 任何一条融合分支在极端 embedding 下少产出一维,`ClvOutput` 就会构造失败,
    /// 该属性立刻抓出来 —— 而不是等到下游余弦计算越界。
    #[test]
    fn fuse_always_yields_512_dims(
        strategy in arb_strategy(),
        elements in proptest::collection::vec(arb_element_any_bits(), 0..6),
    ) {
        let out = fuse_ok(strategy, elements);
        prop_assert_eq!(out.dimension(), CLV_DIM);
        prop_assert_eq!(out.as_slice().len(), CLV_DIM);
    }

    // ============================================================
    // 属性 2:有限输入不传染 NaN
    // ============================================================

    /// 输入全部有限 ⇒ 输出无 NaN
    ///
    /// WHY 只断言"无 NaN"而不断言"有限":Mean/Weighted 对 embedding 求和,
    /// `f32::MAX` 级输入相加会溢出为 ±Inf —— 这是既有实现的事实(未做饱和处理)。
    /// 两者风险等级不同:Inf 仍可比较/排序,NaN 会让 `cosine_similarity`
    /// 一路返回 NaN 且**无任何报错**,即"幽灵向量"的成因。
    #[test]
    fn finite_inputs_never_produce_nan(
        strategy in arb_strategy(),
        elements in proptest::collection::vec(arb_element(), 0..6),
    ) {
        prop_assume!(elements
            .iter()
            .all(|e| e.embedding.iter().all(|v| v.is_finite())));

        let out = fuse_ok(strategy, elements);
        let nan: Vec<usize> = out
            .as_slice()
            .iter()
            .enumerate()
            .filter(|(_, v)| v.is_nan())
            .map(|(i, _)| i)
            .collect();
        prop_assert!(
            nan.is_empty(),
            "有限输入产生了 NaN,前 5 个位置: {:?}",
            nan.iter().take(5).collect::<Vec<_>>()
        );
    }

    // ============================================================
    // 属性 3:单元素的截断/填充语义
    // ============================================================

    /// 单元素融合 ⇒ 前 `len` 维保真、其余补零
    ///
    /// 保真精度按策略分档:Concat 是纯拷贝(逐位相等),Mean 除以 1(相等),
    /// Weighted 为 `(v*w)/w`(f32 两次舍入,取相对误差界)。
    #[test]
    fn single_element_preserves_prefix_and_zero_pads(
        strategy in arb_strategy(),
        modality in arb_modality(),
        embedding in arb_embedding(arb_moderate_scalar()),
    ) {
        let elem = CognitiveElement::new(modality, "h".to_string(), embedding.clone());
        let out = fuse_ok(strategy, vec![elem]);
        let slice = out.as_slice();
        prop_assert_eq!(slice.len(), CLV_DIM);

        let keep = embedding.len().min(CLV_DIM);
        for (i, &want) in embedding[..keep].iter().enumerate() {
            let got = slice[i];
            match strategy {
                FusionStrategy::Concat | FusionStrategy::Mean => prop_assert_eq!(
                    got.to_bits(),
                    want.to_bits(),
                    "策略 {} 在第 {} 维丢失保真度",
                    strategy.as_str(),
                    i
                ),
                FusionStrategy::Weighted => {
                    let tol = want.abs() * 1e-6 + f32::EPSILON;
                    prop_assert!(
                        (got - want).abs() <= tol,
                        "Weighted 第 {i} 维偏差超界:{got} vs {want}"
                    );
                }
            }
        }
        prop_assert!(
            slice[keep..].iter().all(|v| *v == 0.0),
            "策略 {} 在 keep={keep} 之后未填充零",
            strategy.as_str()
        );
    }

    // ============================================================
    // 属性 4:空输入恒零向量(三种策略同口径)
    // ============================================================

    /// 无元素 ⇒ 512 维全零(文档:"若 elements 为空,返回零向量 CLV")
    #[test]
    fn empty_input_yields_zero_vector(strategy in arb_strategy()) {
        let out = fuse_ok(strategy, Vec::new());
        prop_assert_eq!(out.dimension(), CLV_DIM);
        prop_assert!(
            out.as_slice().iter().all(|v| *v == 0.0),
            "空输入必须得到零向量"
        );
    }

    // ============================================================
    // 属性 5:确定性(Ω₂)
    // ============================================================

    /// 同一引擎、同一输入两次融合结果逐位相同
    #[test]
    fn fusion_is_bitwise_deterministic(
        strategy in arb_strategy(),
        elements in proptest::collection::vec(arb_element(), 0..5),
    ) {
        let eng = engine(strategy);
        let bits = |v: &ClvOutput| v.as_slice().iter().map(|x| x.to_bits()).collect::<Vec<u32>>();
        let first = eng
            .fuse(elements.clone())
            .expect("有效配置下 fuse 必须成功");
        let second = eng.fuse(elements).expect("有效配置下 fuse 必须成功");
        prop_assert_eq!(bits(&first), bits(&second));
    }
}

// ============================================================
// 配置口径见证(非属性):clv_dim 与 CLV::DIMENSION 不一致时的分叉
// ============================================================

/// `clv_dim != 512` 时,非空输入被拒但**空输入静默返回 512 维零向量**
///
/// WHY 钉住这条分叉:`NmcConfig::validate()` 会拒 `clv_dim != 512`,但
/// `MultimodalFusionEngine::new` 不调用它;而 `fuse` 的空输入分支直接返回
/// `ClvOutput::zero()`(固定 512),**绕过了 clv_dim 校验**。
/// 于是"配错维度的引擎"在非空输入上诚实报错、在空输入上假装成功。
/// 改法(构造期 `validate()`,或让空分支也走 `resize_to_dim`)属行为变更,
/// 交 crate owner 裁决;本测试只保证该事实被改动时会被发现。
#[test]
fn misconfigured_clv_dim_is_rejected_only_on_non_empty_input() {
    let cfg = NmcConfig::default().with_clv_dim(256);
    assert!(
        cfg.validate().is_err(),
        "前提:该配置本应被 validate() 判为非法"
    );
    let eng = MultimodalFusionEngine::new(cfg);
    let elem = CognitiveElement::new(Modality::Text, "h".to_string(), vec![0.1, 0.2, 0.3]);

    let non_empty = eng.fuse(vec![elem]);
    assert!(
        matches!(non_empty, Err(NmcError::DimensionMismatch { .. })),
        "非空输入应因维度不符而报错,got {non_empty:?}"
    );

    let empty = eng.fuse(Vec::new()).expect("当前实现:空输入不校验 clv_dim");
    assert_eq!(
        empty.dimension(),
        512,
        "空输入静默返回固定 512 维 —— 与 clv_dim=256 的配置不一致"
    );
}
