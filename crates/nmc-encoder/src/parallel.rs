//! P1-T14: 批量感知编码的 ComputeBridge 并行注入
//!
//! 对应任务:P1-T14（WI-34 并行化首批注入,Phase 1 地基波次,v4.0 §7.5.1 L-a）
//! 对应架构层:L2 Memory
//! 对应创新点:NMC(Native Multimodal Context,原生多模态上下文编码)
//!
//! # 注入模式（WI-34 七条纪律 ④桥接唯一 / ⑤逐一回滚 / ⑥IO 不上 rayon）
//! - **识别热点**:[`NmcEncoder::perceive`] 的批量路径 —— 感知器哈希/嵌入
//!   (Text/Desktop 为 SHA256 + 字节频率;Image/Video/Audio 为 tract-onnx 推理)
//!   + 融合为 512 维 CLV,纯 CPU 密集(v4.0 §7.5.1 预估 L-a 2-4×【待验证】)。
//! - **快照分离**:计算核心 [`NmcEncoder::perceive_core`](crate::fusion::NmcEncoder::perceive_core)
//!   为无副作用纯计算(无 IO/await/持锁);事件发布(`publish_encoded_event`,
//!   tokio broadcast,IO 侧)留主线程按序执行,禁入 rayon 闭包。
//! - **挂 ComputeBridge**:[`ComputeBridge::route`](nexus_core::compute::ComputeBridge::route)
//!   按 `TaskKind::ClvSimilarity` 三态判定 → `Inline`(n 小于阈值,串行)或 `Rayon`
//!   ([`spawn_compute_batch`](nexus_core::compute::ComputeBridge::spawn_compute_batch)
//!   批量并行,结果序 = 输入序)。
//! - **保留回退**:`NmcConfig::parallel_encode` 配置开关 + `CHIMERA_NO_PARALLEL_NMC`
//!   环境变量(启动期 OnceLock 读取一次,不在热路径)双重关闭 → 强制串行。
//! - **确定性**:并行与串行逐元素一致(感知为确定性纯函数,断言测试锁定)。
//!
//! # TaskKind 选择
//! `TaskKind::ClvSimilarity`(阈值 1000,chunk 64)—— 任务约束"不新增 TaskKind 变体";
//! 批量编码的产出正是 **CLV**(512 维潜在向量),与 ClvSimilarity("CLV 相似度配对")
//! 语义最接近;批量 > 1000 输入触发 Rayon,与批量编码场景吻合
//! (Generic 阈值 10000 过高,批量编码通常 < 10K 输入/波次)。
//!
//! # 并行粒度（CHUNK 分组）
//! 单条编码约微秒级(SHA256 + 256 桶字节频率 + 融合),逐条 spawn 微任务的调度
//! 开销将反超计算量;按 `CHUNK = 64` 分组(与阈值表 chunk 对齐),每个 rayon
//! 闭包处理一个 chunk 内的全部编码,显著降低调度开销(osa/faae 同模式)。
//!
//! # rayon 闭包契约
//! 闭包捕获 `Arc<NmcEncoder>`(共享感知器,零克隆)+ `Arc<Vec<PerceptionInput>>`
//! (共享输入,零复制)+ 索引范围,仅调 [`perceive_core`](crate::fusion::NmcEncoder::perceive_core)
//! 纯计算;禁 IO / await / 持锁(红线 §7.5.3 纪律⑥)。事件发布在结果回主线程后按序执行。
//!
//! # tract-onnx 线程安全边界
//! 各感知器持有的 `TractPlan`(tract `SimplePlan`)为 immutable 共享结构:
//! `Op` trait 约束 `Send + Sync + 'static`(tract-core 0.22.3 源码),
//! `run(&self)` 每次调用创建独立执行状态 → **推理线程安全**,`Arc<NmcEncoder>`
//! 可跨线程共享,无需每线程克隆或锁(编译期断言 `test_nmc_encoder_send_sync` 锁定)。

use std::sync::{Arc, OnceLock};

use nexus_core::compute::{bridge, DispatchPlan, TaskKind};

use crate::error::NmcError;
use crate::fusion::NmcEncoder;
use crate::types::{ClvOutput, PerceptionInput};

/// 环境变量关闭开关名(纪律⑤;仅测试/运维使用)
const ENV_NO_PARALLEL: &str = "CHIMERA_NO_PARALLEL_NMC";

/// 进程级 env 缓存 — 启动期读取一次,不在热路径(任务约束)
static NO_PARALLEL_ENV: OnceLock<bool> = OnceLock::new();

/// 并行分块大小 — 与阈值表 chunk(64)对齐,避免微任务调度开销反超计算量
const CHUNK: usize = 64;

/// 解析环境变量值 — 纯函数("1"/"true"/"on" 视为关闭,大小写不敏感)
#[must_use]
pub(crate) fn parse_no_parallel_env(value: Option<&str>) -> bool {
    value.is_some_and(|v| matches!(v, "1" | "true" | "TRUE" | "on" | "ON"))
}

/// env 关闭开关 — OnceLock 惰性读取(启动期一次,非热路径)
#[must_use]
pub(crate) fn env_no_parallel() -> bool {
    *NO_PARALLEL_ENV
        .get_or_init(|| parse_no_parallel_env(std::env::var(ENV_NO_PARALLEL).ok().as_deref()))
}

/// 并行开关最终判定 — 配置开关 AND 非 env 关闭(任一关闭 → 串行回退)
#[must_use]
pub(crate) fn should_parallel(config_flag: bool) -> bool {
    config_flag && !env_no_parallel()
}

/// 批量感知编码入口（ComputeBridge 路由判定）
///
/// 路由判定:
/// ① `should_parallel(config_flag)` 为 false(配置/env 关闭)→ 串行;
/// ② `bridge().route(TaskKind::ClvSimilarity, n)` 为 `Inline` → 串行;
/// ③ 否则 → `spawn_compute_batch` 并行(CHUNK 分组)。
///
/// `inputs` 以 `&Arc<Vec<PerceptionInput>>` 传入:并行闭包捕获同一 Arc + 索引范围,
/// **零输入级 clone**(faae 注入实测证明 chunk.to_vec() 全量复制是 0.76× 回归根因,
/// 已规避);`encoder` 以 `&Arc<NmcEncoder>` 传入,闭包 `Arc::clone` 共享感知器
/// (tract plan 跨线程共享,见模块 doc)。
///
/// 返回 `Vec<Result<ClvOutput, NmcError>>`,结果序 = 输入序,逐输入独立结果
/// (单个输入感知失败不阻断同批其他输入,与串行 `perceive` 语义一致)。
/// 并行与串行路径**逐元素一致**(确定性断言测试锁定)。
///
/// 事件发布:并行路径的 `publish_encoded_event` 全部在**主线程**按输入序执行
/// (IO 侧不上 rayon 红线);失败输入不发布事件(与 `perceive` 一致)。
///
/// pub:批量热点核心入口,供 bench(`parallel_encoding.rs`)与集成方直测对照。
#[must_use]
pub fn perceive_batch(
    encoder: &Arc<NmcEncoder>,
    inputs: &Arc<Vec<PerceptionInput>>,
    parallel_enabled: bool,
) -> Vec<Result<ClvOutput, NmcError>> {
    let n = inputs.len();
    if !should_parallel(parallel_enabled)
        || bridge().route(TaskKind::ClvSimilarity, n) == DispatchPlan::Inline
    {
        perceive_serial(encoder, inputs)
    } else {
        perceive_parallel(encoder, inputs)
    }
}

/// 串行路径 — 顺序感知编码(回退 + Inline 分支,与注入前行为逐位一致)
fn perceive_serial(
    encoder: &NmcEncoder,
    inputs: &[PerceptionInput],
) -> Vec<Result<ClvOutput, NmcError>> {
    inputs
        .iter()
        .map(|input| {
            let modality = input.modality();
            match encoder.perceive_core(input) {
                Ok((clv_output, content_hash)) => {
                    encoder.publish_encoded_event(modality, content_hash, clv_output.dimension());
                    Ok(clv_output)
                }
                Err(e) => Err(e),
            }
        })
        .collect()
}

/// 并行路径 — `spawn_compute_batch` 批量编码,结果序 = 输入序
///
/// CHUNK 分组:每个闭包处理一个 chunk 的编码(捕获 `Arc<NmcEncoder>` +
/// `Arc<Vec<PerceptionInput>>` + 索引范围,零输入复制),返回 chunk 内逐输入
/// `Result<(ClvOutput, String), NmcError>`(String 为 content_hash,供主线程发布事件)。
///
/// 池内 panic 被 catch_unwind 隔离:理论不可达(闭包纯计算),防御性映射为该
/// chunk 逐输入 `EncodingFailed`(不新增 NmcError 变体)。
fn perceive_parallel(
    encoder: &Arc<NmcEncoder>,
    inputs: &Arc<Vec<PerceptionInput>>,
) -> Vec<Result<ClvOutput, NmcError>> {
    let n_chunks = inputs.len().div_ceil(CHUNK);
    type ChunkTask = Box<dyn FnOnce() -> Vec<Result<(ClvOutput, String), NmcError>> + Send>;
    let tasks: Vec<ChunkTask> = (0..n_chunks)
        .map(|ci| {
            let enc = Arc::clone(encoder);
            let all = Arc::clone(inputs);
            let start = ci * CHUNK;
            let end = (start + CHUNK).min(all.len());
            Box::new(move || {
                all[start..end]
                    .iter()
                    .map(|input| enc.perceive_core(input))
                    .collect()
            }) as Box<dyn FnOnce() -> Vec<Result<(ClvOutput, String), NmcError>> + Send>
        })
        .collect();

    let results = bridge().spawn_compute_batch(TaskKind::ClvSimilarity, tasks);

    let mut out = Vec::with_capacity(inputs.len());
    let mut input_idx = 0usize;
    for r in results {
        match r {
            Ok(chunk_out) => {
                for item in chunk_out {
                    let modality = inputs[input_idx].modality();
                    match item {
                        Ok((clv_output, content_hash)) => {
                            // 事件发布留主线程按输入序(IO 侧,禁入 rayon)
                            encoder.publish_encoded_event(
                                modality,
                                content_hash,
                                clv_output.dimension(),
                            );
                            out.push(Ok(clv_output));
                        }
                        Err(e) => out.push(Err(e)),
                    }
                    input_idx += 1;
                }
            }
            // 防御分支:理论不可达(闭包纯计算,panic 源隔离)。
            // 映射为该 chunk 逐输入 EncodingFailed,保持结果数与输入数一致。
            Err(e) => {
                let chunk_end = (input_idx + CHUNK).min(inputs.len());
                // WHY allow(mut_range_bound):range 进入时已求值(input_idx..chunk_end 复制),
                // 循环内 input_idx 递增为外层游标延续,语义有意;重构为 while 反而模糊错误填充意图
                #[allow(clippy::mut_range_bound)]
                for idx in input_idx..chunk_end {
                    out.push(Err(NmcError::EncodingFailed {
                        modality: inputs[idx].modality().as_str().into(),
                        reason: format!("并行批量编码 chunk 计算异常: {e}"),
                    }));
                    input_idx += 1;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NmcConfig;

    /// 构造测试输入(固定确定性,长文本以放大纯计算成本)
    fn make_inputs(n: usize) -> Vec<PerceptionInput> {
        let content = "Parallel batch encoding test content with deterministic seed. ".repeat(64); // ~4KB,放大 SHA256 + 字节频率嵌入计算成本
        (0..n)
            .map(|i| PerceptionInput::Text(format!("input-{i:04}:{content}")))
            .collect()
    }

    /// 错误一致性断言辅助 — NmcError 无 PartialEq,用 Debug 字符串比对
    fn assert_err_equal(
        serial: &Result<ClvOutput, NmcError>,
        parallel: &Result<ClvOutput, NmcError>,
        idx: usize,
    ) {
        match (serial, parallel) {
            (Err(se), Err(pe)) => assert_eq!(
                format!("{se:?}"),
                format!("{pe:?}"),
                "input[{idx}] 错误必须与串行逐元素一致"
            ),
            (Ok(_), Err(pe)) => panic!("input[{idx}] 并行失败但串行成功: {pe:?}"),
            (Err(se), Ok(_)) => panic!("input[{idx}] 并行成功但串行失败: {se:?}"),
            (Ok(_), Ok(_)) => {}
        }
    }

    // ============================================================
    // env 开关 / 判定逻辑
    // ============================================================

    #[test]
    fn test_parse_no_parallel_env() {
        assert!(parse_no_parallel_env(Some("1")));
        assert!(parse_no_parallel_env(Some("true")));
        assert!(parse_no_parallel_env(Some("TRUE")));
        assert!(parse_no_parallel_env(Some("on")));
        assert!(!parse_no_parallel_env(Some("0")));
        assert!(!parse_no_parallel_env(Some("false")));
        assert!(!parse_no_parallel_env(Some("yes")));
        assert!(!parse_no_parallel_env(None));
    }

    #[test]
    fn test_should_parallel_gating() {
        // 配置开启 + env 未关 → 并行(进程级 OnceLock,测试环境默认未设置 → false)
        if !env_no_parallel() {
            assert!(should_parallel(true));
        }
        // 配置关闭 → 强制串行(无论 env)
        assert!(!should_parallel(false));
    }

    // ============================================================
    // 并行 vs 串行一致性(固定确定性输入,含混合模态)
    // ============================================================

    #[test]
    fn test_parallel_matches_serial_large_batch() {
        let encoder = Arc::new(NmcEncoder::new(NmcConfig::default()).expect("编码器构建"));
        let inputs = Arc::new(make_inputs(1_200)); // ≥ ClvSimilarity 阈值 1000 → Rayon 分支
        let serial = perceive_serial(&encoder, &inputs);
        let parallel = perceive_batch(&encoder, &inputs, true);
        assert_eq!(serial.len(), 1_200, "结果数必须等于输入数");
        assert_eq!(parallel.len(), 1_200, "结果数必须等于输入数");
        for (i, (s, p)) in serial.iter().zip(parallel.iter()).enumerate() {
            match (s, p) {
                (Ok(so), Ok(po)) => {
                    assert_eq!(so, po, "input[{i}] CLV 必须与串行逐元素一致");
                    assert_eq!(so.dimension(), 512);
                }
                _ => assert_err_equal(s, p, i),
            }
        }
    }

    #[test]
    fn test_parallel_matches_serial_small_batch() {
        // 小批量(< 阈值)→ Inline 串行,结果一致
        let encoder = Arc::new(NmcEncoder::new(NmcConfig::default()).expect("编码器构建"));
        let inputs = Arc::new(make_inputs(90));
        let serial = perceive_serial(&encoder, &inputs);
        let parallel = perceive_batch(&encoder, &inputs, true);
        assert_eq!(serial.len(), parallel.len());
        for (i, (s, p)) in serial.iter().zip(parallel.iter()).enumerate() {
            match (s, p) {
                (Ok(so), Ok(po)) => assert_eq!(so, po, "input[{i}] CLV 必须一致"),
                _ => assert_err_equal(s, p, i),
            }
        }
    }

    #[test]
    fn test_parallel_matches_serial_mixed_modalities() {
        // 混合模态:Text 成功 + Image/Video/Audio 失败(占位字节输入)→ 错误逐元素一致
        let encoder = Arc::new(NmcEncoder::new(NmcConfig::default()).expect("编码器构建"));
        let mut inputs: Vec<PerceptionInput> = make_inputs(1_100); // ≥ 阈值触发 Rayon
        for (i, slot) in inputs.iter_mut().step_by(7).enumerate() {
            match i % 3 {
                0 => *slot = PerceptionInput::Image(vec![0; 1024]),
                1 => *slot = PerceptionInput::Video(vec![0; 1024]),
                _ => *slot = PerceptionInput::Audio(vec![0; 512]),
            }
        }
        let inputs = Arc::new(inputs);
        let serial = perceive_serial(&encoder, &inputs);
        let parallel = perceive_batch(&encoder, &inputs, true);
        assert_eq!(serial.len(), parallel.len());
        for (i, (s, p)) in serial.iter().zip(parallel.iter()).enumerate() {
            match (s, p) {
                (Ok(so), Ok(po)) => assert_eq!(so, po, "input[{i}] CLV 必须一致"),
                _ => assert_err_equal(s, p, i),
            }
        }
    }

    // ============================================================
    // 边界:空输入 / 单元素 / 非整 chunk
    // ============================================================

    #[test]
    fn test_empty_inputs() {
        let encoder = Arc::new(NmcEncoder::new(NmcConfig::default()).expect("编码器构建"));
        let inputs = Arc::new(Vec::<PerceptionInput>::new());
        let out = perceive_batch(&encoder, &inputs, true);
        assert!(out.is_empty());
    }

    #[test]
    fn test_single_input() {
        let encoder = Arc::new(NmcEncoder::new(NmcConfig::default()).expect("编码器构建"));
        let inputs = Arc::new(make_inputs(1));
        let out = perceive_batch(&encoder, &inputs, true);
        assert_eq!(out.len(), 1);
        assert!(out[0].is_ok(), "单文本输入应编码成功");
    }

    #[test]
    fn test_odd_chunk_boundary() {
        // 65 个输入 = 1 个整 chunk(64) + 1 个残块 → 跨块结果拼接顺序正确
        let encoder = Arc::new(NmcEncoder::new(NmcConfig::default()).expect("编码器构建"));
        let inputs = Arc::new(make_inputs(65));
        let serial = perceive_serial(&encoder, &inputs);
        let parallel = perceive_batch(&encoder, &inputs, true);
        assert_eq!(serial.len(), parallel.len());
        for (i, (s, p)) in serial.iter().zip(parallel.iter()).enumerate() {
            match (s, p) {
                (Ok(so), Ok(po)) => assert_eq!(so, po, "input[{i}] CLV 必须一致"),
                _ => assert_err_equal(s, p, i),
            }
        }
    }

    // ============================================================
    // 回退开关(配置关闭 → 强制串行,结果与串行一致)
    // ============================================================

    #[test]
    fn test_config_disable_falls_back_to_serial() {
        // 大 N(≥ 阈值)下配置关闭 → 必须仍走串行且结果正确
        let encoder = Arc::new(NmcEncoder::new(NmcConfig::default()).expect("编码器构建"));
        let inputs = Arc::new(make_inputs(1_200));
        let disabled = perceive_batch(&encoder, &inputs, false);
        let serial = perceive_serial(&encoder, &inputs);
        assert_eq!(disabled.len(), serial.len());
        for (i, (d, s)) in disabled.iter().zip(serial.iter()).enumerate() {
            match (d, s) {
                (Ok(do_), Ok(so)) => assert_eq!(do_, so, "input[{i}] 配置关闭后必须与串行一致"),
                _ => assert_err_equal(d, s, i),
            }
        }
    }

    /// env 关闭开关 → 走串行(集成验证:进程级 env 首次读取即生效)
    ///
    /// WHY OnceLock:本测试设置 env 后,`env_no_parallel()` 首次读取即缓存关闭态;
    /// 若其他测试先调用导致已缓存 false,本测试跳过断言(不污染其他测试)。
    #[test]
    fn test_env_disable_falls_back_to_serial() {
        if env_no_parallel() {
            // 已有其他来源关闭 —— 缓存生效,直接验证串行一致性
            let encoder = Arc::new(NmcEncoder::new(NmcConfig::default()).expect("编码器构建"));
            let inputs = Arc::new(make_inputs(1_200));
            let out = perceive_batch(&encoder, &inputs, true);
            assert_eq!(out.len(), 1_200);
            return;
        }
        // env 尚未缓存:设置后首次读取 → 关闭并行(OnceLock 一次性语义)
        std::env::set_var(ENV_NO_PARALLEL, "1");
        let encoder = Arc::new(NmcEncoder::new(NmcConfig::default()).expect("编码器构建"));
        let inputs = Arc::new(make_inputs(1_200));
        let out = perceive_batch(&encoder, &inputs, true);
        let serial = perceive_serial(&encoder, &inputs);
        assert_eq!(out.len(), serial.len());
        for (i, (o, s)) in out.iter().zip(serial.iter()).enumerate() {
            match (o, s) {
                (Ok(oo), Ok(so)) => assert_eq!(oo, so, "input[{i}] env 关闭后必须与串行一致"),
                _ => assert_err_equal(o, s, i),
            }
        }
        // 恢复 env,避免影响同进程其他测试(OnceLock 已缓存,恢复仅对子进程有意义)
        std::env::remove_var(ENV_NO_PARALLEL);
    }

    // ============================================================
    // 配置开关与路由接线
    // ============================================================

    #[test]
    fn test_config_flag_wires_through() {
        let encoder = Arc::new(NmcEncoder::new(NmcConfig::default()).expect("编码器构建"));
        let inputs = Arc::new(make_inputs(1_200));
        let config = NmcConfig::default();
        // 配置默认 true → 路由判定走 should_parallel(config.parallel_encode)
        let out = perceive_batch(&encoder, &inputs, config.parallel_encode);
        assert_eq!(out.len(), 1_200);
        // 配置 false → 强制串行
        let disabled = NmcConfig::default().with_parallel_encode(false);
        assert!(!disabled.parallel_encode);
        let out = perceive_batch(&encoder, &inputs, disabled.parallel_encode);
        assert_eq!(out.len(), 1_200);
    }
}
