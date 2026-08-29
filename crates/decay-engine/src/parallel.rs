//! P1-T14: 批量能力衰减的 ComputeBridge 并行注入
//!
//! 对应任务:P1-T14（WI-34 并行化第 2 批补全,v4.0 §7.5.1 L-a）
//! 对应架构层:L4 Security
//! 对应创新点:连续权限流体衰减模型（ADR-002）
//!
//! # 注入模式（WI-34 七条纪律 ④桥接唯一 / ⑤逐一回滚 / ⑥IO 不上 rayon）
//! - **识别热点**:[`crate::DecayEngine`] 的批量衰减路径 —— 对能力注册表内全部
//!   能力应用衰减（`decay_bench` bulk_decay_throughput / `decay_compute` bulk 实测）:
//!   逐条 `decay` = DashMap `get_mut`（分片锁）+ `Instant::now` + 浮点衰减公式
//!   （线性/指数 + clamp + 自动冻结判定）,纯 CPU 密集,批量下锁与逐次调用开销叠加。
//! - **快照分离,计算并行,串行提交**:衰减语义**逐能力独立**（无全局守恒/总量约束,
//!   ADR-002 连续流体模型,每个能力仅依赖自身 level/frozen/last_decay_at）;
//!   批量路径先主线程一次性快照 `(level, frozen, last_decay_at)` 为
//!   `Arc<Vec<(String, DecaySnapshot)>>`（DashMap 分片锁在快照窗口内短暂持有,
//!   不跨闭包边界）,rayon 闭包内仅做**纯计算**（衰减公式 + 自动冻结判定,
//!   无 IO / await / 持锁）;结果回主线程后**按输入序**串行提交写回 DashMap
//!   （提交阶段含调试/告警日志,不入 rayon）。
//! - **挂 ComputeBridge**:[`ComputeBridge::route`](nexus_core::compute::ComputeBridge::route)
//!   按 `TaskKind::Generic` 三态判定 → `Inline`（n 小于阈值,串行）或 `Rayon`
//!   （[`spawn_compute_batch`](nexus_core::compute::ComputeBridge::spawn_compute_batch)
//!   批量并行,结果序 = 输入序）。
//! - **保留回退**:`DecayEngine::parallel_batch` 配置开关 + `CHIMERA_NO_PARALLEL_DECAY`
//!   环境变量（启动期 OnceLock 读取一次,不在热路径）双重关闭 → 强制串行;
//!   并行计算失败（理论不可达:闭包纯计算）自动回退串行。
//! - **确定性**:并行与串行逐元素一致（同输入同输出,Ω₂）;批量语义 = **快照语义**
//!   （全部能力基于调用时刻统一 `now` 快照计算,非顺序依赖语义）,断言测试锁定
//!   （含顺序断言:结果序 = 输入序）。
//!
//! # TaskKind 选择
//! `TaskKind::Generic`（阈值 10,000,chunk 64）—— 任务约束"不新增 TaskKind 变体";
//! 批量能力衰减不在六类登记语义内（非 CLV 相似度/掩码/KNN/GSOE/CSC 压缩评分）,
//! 归入未登记任务的保守默认;≥ 10,000 能力的大批量衰减触发 Rayon
//! （decay 单次计算微秒级,低于阈值时串行更优——调度开销反超计算量）。
//!
//! # rayon 闭包契约
//! 闭包捕获 `Arc<Vec<(String, DecaySnapshot)>>` + `Arc<Vec<DecayEventParam>>`
//! （事件精简为 `(kind, severity)`,零 String clone——Freeze reason 仅日志用,
//!   留在提交阶段）+ 索引范围 + `Arc<DecayConfig>`（只读共享）,仅调
//!   [`apply_decay_pure`] 纯计算;闭包内禁 IO / await / 持锁（红线 §7.5.3 纪律⑥）。
//!
//! # 失败语义
//! 单能力衰减失败（`CapabilityLevel::new` 越界,理论不可达——clamp 保证 ∈ [0,1]）
//! 经 Result 逐项传播,不阻断同批其他能力（与串行逐项一致）。池内 panic 被
//! catch_unwind 隔离,防御性回退串行（理论不可达:闭包纯计算）。

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use nexus_core::compute::{bridge, DispatchPlan, TaskKind};

use crate::error::DecayError;
use crate::types::{CapabilityLevel, DecayConfig, DecayEvent};

/// 环境变量关闭开关名（纪律⑤;仅测试/运维使用）
const ENV_NO_PARALLEL: &str = "CHIMERA_NO_PARALLEL_DECAY";

/// 进程级 env 缓存 — 启动期读取一次,不在热路径（任务约束）
static NO_PARALLEL_ENV: OnceLock<bool> = OnceLock::new();

/// 并行分块大小 — 任务粒度（轻计算场景放大摊薄调度/槽位锁开销;阈值表
/// Generic chunk 为 64,decay 单次计算微秒级,64 个/任务 → 任务数与调度开销
/// 反超计算收益（本机实测 20K 能力 313 任务 ≈ 无加速）,放大到 1024 后
/// 任务数 20,单任务 ~32μs 计算,调度占比可忽略）
const CHUNK: usize = 1024;

/// 解析环境变量值 — 纯函数（"1"/"true"/"on" 视为关闭,大小写不敏感）
#[must_use]
pub(crate) fn parse_no_parallel_env(value: Option<&str>) -> bool {
    value.is_some_and(|v| matches!(v, "1" | "true" | "TRUE" | "on" | "ON"))
}

/// env 关闭开关 — OnceLock 惰性读取（启动期一次,非热路径）
#[must_use]
pub(crate) fn env_no_parallel() -> bool {
    *NO_PARALLEL_ENV.get_or_init(|| {
        parse_no_parallel_env(std::env::var(ENV_NO_PARALLEL).ok().as_deref())
    })
}

/// 并行开关最终判定 — 配置开关 AND 非 env 关闭（任一关闭 → 串行回退）
#[must_use]
pub(crate) fn should_parallel(config_flag: bool) -> bool {
    config_flag && !env_no_parallel()
}

/// 衰减输入快照 — 批量路径的输入单元（主线程快照,闭包零复制捕获）
///
/// `last_decay_at` 为 `Instant`（Copy）,闭包捕获后 `elapsed` 统一以快照时刻
/// `now` 为基准计算 → 并行与串行逐元素一致的确定性前提。
///
/// 不含 id:`items` 已持有 id,提交阶段直接从输入取（快照零 String clone,
/// 20K 规模下省 ~0.8ms 主线程成本,本机实测）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct DecaySnapshot {
    /// 当前权限流体等级 [0, 1]
    pub(crate) level: f32,
    /// 是否冻结（冻结能力跳过衰减/惩罚/恢复）
    pub(crate) frozen: bool,
    /// 上次衰减时间戳（计算时间驱动衰减的 elapsed 基准）
    pub(crate) last_decay_at: Instant,
}

/// 事件精简参数 — 闭包捕获的零 String 事件表示
///
/// WHY 精简:`DecayEvent` 的 `capability_id` / `reason` 字符串不参与计算
/// （仅日志/广播用,留在提交阶段主线程按输入序输出）;闭包内零 String clone
/// （osa 注入实测教训:profile 级 clone 成本反超计算收益）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct DecayEventParam {
    /// 事件种类:0=TimeDecay, 1=ViolationPenalty, 2=Freeze, 3=Restore
    kind: u8,
    /// ViolationPenalty 的严重程度（其余事件忽略,置 0）
    severity: f32,
}

impl DecayEventParam {
    /// `DecayEvent` → 精简参数（纯映射,零分配）
    #[must_use]
    pub(crate) fn from_event(event: &DecayEvent) -> Self {
        match event {
            DecayEvent::TimeDecay => Self { kind: 0, severity: 0.0 },
            DecayEvent::ViolationPenalty { severity, .. } => {
                Self { kind: 1, severity: *severity }
            }
            DecayEvent::Freeze { .. } => Self { kind: 2, severity: 0.0 },
            DecayEvent::Restore { .. } => Self { kind: 3, severity: 0.0 },
        }
    }
}

/// 衰减计算结果 — 待主线程串行提交的纯值（Copy,无引用）
#[derive(Debug, Clone, Copy)]
pub(crate) struct DecayOutcome {
    /// 计算后的权限流体等级 [0, 1]
    pub(crate) level: f32,
    /// 计算后的冻结状态（自动冻结判定已并入）
    pub(crate) frozen: bool,
}

/// 纯计算 — 对单个能力快照应用衰减事件（无锁无 IO,与
/// [`crate::engine::DecayEngine::decay_with_config`] 逐分支一致）
///
/// 语义对齐（行为锁定,断言测试覆盖）:
/// - `TimeDecay`:level -= elapsed × rate（线性,默认）或 level × exp(-elapsed/τ)
///   （指数,P1-4 共享 `nexus_core::decay` 公式）;冻结跳过;触发自动冻结检查;
/// - `ViolationPenalty`:level -= penalty × severity;冻结跳过;触发自动冻结检查;
/// - `Freeze`:level = 0 且 frozen = true（Skeptic 否决,幂等由调用方守卫）;
/// - `Restore`:level += elapsed × restore_rate（clamp [0,1]）;冻结跳过;
/// - 自动冻结:衰减类事件后 level ≤ freeze_threshold 且未冻结 → 置 0 + 冻结
///   （与 `decay_with_config` 的 check_auto_freeze 分支一致,权限不残留）;
/// - `last_decay_at` 更新在**提交阶段**统一进行（批量路径以统一 `now` 为基准,
///   与串行单次 `decay` 每次调用各自取 now 的语义不同——批量 = 快照语义）。
///
/// 失败面:level 越界经 `CapabilityLevel::new` 返回 [`DecayError::InvalidLevel`]
/// （理论不可达:全部输出经 clamp 保证 ∈ [0,1],签名保留错误面与现有代码风格一致）。
pub(crate) fn apply_decay_pure(
    level: f32,
    frozen: bool,
    last_decay_at: Instant,
    now: Instant,
    event: DecayEventParam,
    config: &DecayConfig,
) -> Result<DecayOutcome, DecayError> {
    let elapsed = now.duration_since(last_decay_at).as_secs_f32();

    // 事件种类 0/1/3 冻结跳过（与 decay_with_config 一致,不更新 last_decay_at）;
    // 自动冻结检查标志:仅在衰减类事件（0/1）后触发,Restore 不触发（恢复不应导致冻结）;
    // new_frozen:Freeze(2) 置 true,其余保留原状
    let (mut new_level, check_auto_freeze, mut new_frozen) = match event.kind {
        0 => {
            if frozen {
                return Ok(DecayOutcome { level, frozen });
            }
            let v = if config.use_exponential_decay {
                // 指数衰减:level = level × exp(-elapsed / tau)（nexus_core::decay 公式）
                let decay_factor = nexus_core::decay::exponential_decay_factor(
                    elapsed as f64,
                    config.decay_tau_seconds as f64,
                ) as f32;
                let lower = config.min_level.max(0.0);
                (level * decay_factor).clamp(lower, 1.0)
            } else {
                // 线性衰减（默认,向后兼容）:level -= elapsed × time_decay_rate
                let decay_amount = elapsed * config.time_decay_rate;
                let lower = config.min_level.max(0.0);
                (level - decay_amount).clamp(lower, 1.0)
            };
            (v, true, frozen)
        }
        1 => {
            if frozen {
                return Ok(DecayOutcome { level, frozen });
            }
            let penalty = config.event_decay_penalty * event.severity;
            let lower = config.min_level.max(0.0);
            ((level - penalty).clamp(lower, 1.0), true, frozen)
        }
        2 => {
            // Freeze:立即清零并冻结（Skeptic 否决）
            (0.0, false, true)
        }
        3 => {
            if frozen {
                return Ok(DecayOutcome { level, frozen });
            }
            let restore_amount = elapsed * config.restore_rate;
            ((level + restore_amount).clamp(0.0, 1.0), false, frozen)
        }
        _ => {
            return Err(DecayError::ConfigError(format!(
                "未知衰减事件种类: {}",
                event.kind
            )));
        }
    };


    // 自动冻结:低于阈值且未冻结则冻结（防止权限过低仍可操作,权限不应残留）
    if check_auto_freeze && !new_frozen && new_level <= config.freeze_threshold {
        new_frozen = true;
        new_level = 0.0;
    }

    // clamp 保证 ∈ [0,1],new 不失败;错误面保留以对齐现有调用风格
    let _ = CapabilityLevel::new(new_level)?;
    Ok(DecayOutcome {
        level: new_level,
        frozen: new_frozen,
    })
}

/// 批量衰减计算核心（ComputeBridge 路由判定入口,供 engine 调用）
///
/// `snapshots` / `events` 以 `&Arc<Vec<_>>` 传入:并行闭包 `Arc::clone` 共享
/// 容器 + 索引范围,**零快照级复制**;`config` 以 `&Arc<DecayConfig>` 传入,
/// 闭包只读共享（零克隆）。`now` 为调用方统一时刻（快照语义,确定性前提）。
///
/// 路由判定:
/// ① `should_parallel(parallel_enabled)` 为 false（配置/env 关闭）→ 串行;
/// ② `bridge().route(TaskKind::Generic, n)` 为 `Inline` → 串行;
/// ③ 否则 → `spawn_compute_batch` 并行（CHUNK 分组,段内保序 + 段间按序拼接）。
///
/// 返回 `Vec<Result<DecayOutcome, DecayError>>`,结果序 = 输入序,逐输入独立
/// （单个失败不阻断同批其他能力,与串行逐项一致）。**提交由调用方负责**
/// （engine 按输入序写回 DashMap——写回是持锁操作,禁入 rayon 闭包）。
pub(crate) fn decay_batch_core(
    snapshots: &Arc<Vec<DecaySnapshot>>,
    events: &Arc<Vec<DecayEventParam>>,
    config: &Arc<DecayConfig>,
    now: Instant,
    parallel_enabled: bool,
) -> Vec<Result<DecayOutcome, DecayError>> {
    let n = snapshots.len();
    if !should_parallel(parallel_enabled)
        || bridge().route(TaskKind::Generic, n) == DispatchPlan::Inline
    {
        decay_batch_serial(snapshots, events, config, now)
    } else {
        match decay_batch_parallel(snapshots, events, config, now) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "并行批量衰减失败,回退串行");
                decay_batch_serial(snapshots, events, config, now)
            }
        }
    }
}

/// 串行路径 — 顺序应用衰减（回退 + Inline 分支;与并行路径共用 [`apply_decay_pure`]
/// 纯函数与统一 `now`,保证逐元素一致）
fn decay_batch_serial(
    snapshots: &Arc<Vec<DecaySnapshot>>,
    events: &Arc<Vec<DecayEventParam>>,
    config: &Arc<DecayConfig>,
    now: Instant,
) -> Vec<Result<DecayOutcome, DecayError>> {
    snapshots
        .iter()
        .zip(events.iter())
        .map(|(s, ev)| apply_decay_pure(s.level, s.frozen, s.last_decay_at, now, *ev, config))
        .collect()
}

/// 并行路径 — `spawn_compute_batch` 段间衰减计算,结果序 = 输入序
///
/// CHUNK 分组:每个闭包处理一个 chunk 的衰减计算（捕获 `Arc` 容器 + 索引范围,
/// 零快照复制）,返回 chunk 段内 `Vec<Result<DecayOutcome, DecayError>>`
/// （段内保序）;各 chunk 结果按段序拼接（段间保序）→ 与串行逐元素一致。
///
/// 池内 panic 被 catch_unwind 隔离:理论不可达（闭包纯计算）,防御性映射为
/// Err 触发调用方回退串行。
fn decay_batch_parallel(
    snapshots: &Arc<Vec<DecaySnapshot>>,
    events: &Arc<Vec<DecayEventParam>>,
    config: &Arc<DecayConfig>,
    now: Instant,
) -> Result<Vec<Result<DecayOutcome, DecayError>>, String> {
    let n = snapshots.len();
    let n_chunks = n.div_ceil(CHUNK);
    type ChunkTask = Box<dyn FnOnce() -> Vec<Result<DecayOutcome, DecayError>> + Send>;
    let tasks: Vec<ChunkTask> =
        (0..n_chunks)
            .map(|ci| {
                let snaps = Arc::clone(snapshots);
                let evs = Arc::clone(events);
                let cfg = Arc::clone(config);
                let start = ci * CHUNK;
                let end = (start + CHUNK).min(n);
                Box::new(move || {
                    snaps[start..end]
                        .iter()
                        .zip(evs[start..end].iter())
                        .map(|(s, ev)| {
                            apply_decay_pure(s.level, s.frozen, s.last_decay_at, now, *ev, &cfg)
                        })
                        .collect()
                }) as Box<dyn FnOnce() -> Vec<Result<DecayOutcome, DecayError>> + Send>
            })
            .collect();

    let results = bridge().spawn_compute_batch(TaskKind::Generic, tasks);

    // 按段序拼接（段间保序）,总长度 = 输入数
    let mut out = Vec::with_capacity(n);
    for r in results {
        match r {
            Ok(chunk_out) => out.extend(chunk_out),
            Err(e) => return Err(format!("并行批量衰减 chunk 计算异常: {e}")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造批量输入（快照 + 事件）,独立于引擎（快照语义:输入即初始状态）
    ///
    /// 事件按 i 轮转覆盖四种种类（含 ViolationPenalty 带 severity）,
    /// 覆盖 apply_decay_pure 全部分支。level 固定 [0.30, 0.89]（部分接近冻结阈值）,
    /// last_decay_at 固定为 10s 前（以统一 now 计算确定性 elapsed）。
    /// 返回 `(Vec<DecaySnapshot>, Vec<DecayEventParam>)` —— 快照不含 id
    /// （提交阶段从输入 items 取,快照零 String clone）。
    fn make_items(n: usize) -> (Vec<DecaySnapshot>, Vec<DecayEventParam>) {
        let last_decay_at = Instant::now() - std::time::Duration::from_secs(10);
        let mut snapshots = Vec::with_capacity(n);
        let mut events = Vec::with_capacity(n);
        for i in 0..n {
            let id = format!("cap-{i}");
            let level = 0.3 + ((i % 60) as f32) / 100.0; // [0.30, 0.89]
            snapshots.push(DecaySnapshot { level, frozen: false, last_decay_at });
            let ev = match i % 4 {
                0 => DecayEvent::TimeDecay,
                1 => DecayEvent::ViolationPenalty {
                    capability_id: id,
                    severity: 1.0 + (i % 3) as f32,
                },
                2 => DecayEvent::Freeze {
                    capability_id: id,
                    reason: "test-freeze".into(),
                },
                _ => DecayEvent::Restore { capability_id: id },
            };
            events.push(DecayEventParam::from_event(&ev));
        }
        (snapshots, events)
    }

    /// 测试用激进配置（衰减/恢复速率放大,便于快速观测语义差异）
    fn test_config() -> DecayConfig {
        DecayConfig {
            time_decay_rate: 0.5,
            event_decay_penalty: 0.2,
            min_level: 0.0,
            freeze_threshold: 0.05,
            restore_rate: 0.5,
            ..DecayConfig::default()
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
        // 配置开启 + env 未关 → 并行（进程级 OnceLock,测试环境默认未设置 → false）
        if !env_no_parallel() {
            assert!(should_parallel(true));
        }
        // 配置关闭 → 强制串行（无论 env）
        assert!(!should_parallel(false));
    }

    // ============================================================
    // 纯函数行为锁定（与 decay_with_config 逐分支语义一致）
    // ============================================================

    #[test]
    fn test_apply_time_decay_linear() {
        let config = DecayConfig::default();
        let last = Instant::now() - std::time::Duration::from_secs(10);
        let out = apply_decay_pure(
            0.8,
            false,
            last,
            Instant::now(),
            DecayEventParam { kind: 0, severity: 0.0 },
            &config,
        )
        .expect("线性衰减应成功");
        // 0.8 - 10 × 0.001 = 0.79
        assert!((out.level - 0.79).abs() < 1e-6);
        assert!(!out.frozen);
    }

    #[test]
    fn test_apply_time_decay_frozen_skipped() {
        let config = DecayConfig::default();
        let out = apply_decay_pure(
            0.5,
            true,
            Instant::now() - std::time::Duration::from_secs(100),
            Instant::now(),
            DecayEventParam { kind: 0, severity: 0.0 },
            &config,
        )
        .expect("冻结能力应原样返回");
        assert_eq!(out.level, 0.5);
        assert!(out.frozen, "冻结状态必须保留");
    }

    #[test]
    fn test_apply_violation_penalty() {
        let config = DecayConfig::default();
        let out = apply_decay_pure(
            0.8,
            false,
            Instant::now(),
            Instant::now(),
            DecayEventParam { kind: 1, severity: 2.0 },
            &config,
        )
        .expect("违规惩罚应成功");
        // 0.8 - 0.1 × 2.0 = 0.6
        assert!((out.level - 0.6).abs() < 1e-6);
    }

    #[test]
    fn test_apply_freeze_zeroes() {
        let config = DecayConfig::default();
        let out = apply_decay_pure(
            0.9,
            false,
            Instant::now(),
            Instant::now(),
            DecayEventParam { kind: 2, severity: 0.0 },
            &config,
        )
        .expect("冻结应成功");
        assert_eq!(out.level, 0.0);
        assert!(out.frozen);
    }

    #[test]
    fn test_apply_restore() {
        let config = DecayConfig::default();
        let out = apply_decay_pure(
            0.3,
            false,
            Instant::now() - std::time::Duration::from_secs(10),
            Instant::now(),
            DecayEventParam { kind: 3, severity: 0.0 },
            &config,
        )
        .expect("恢复应成功");
        // 0.3 + 10 × 0.01 = 0.4
        assert!((out.level - 0.4).abs() < 1e-6);
    }

    #[test]
    fn test_apply_auto_freeze_below_threshold() {
        let config = DecayConfig::default();
        // 0.04 的惩罚级衰减:0.06 - 0.1 = -0.04 → clamp 到 0.0 → 触发自动冻结
        let out = apply_decay_pure(
            0.06,
            false,
            Instant::now(),
            Instant::now(),
            DecayEventParam { kind: 1, severity: 1.0 },
            &config,
        )
        .expect("衰减应成功");
        assert_eq!(out.level, 0.0, "低于冻结阈值后权限应清零");
        assert!(out.frozen, "低于冻结阈值后应自动冻结");
    }

    #[test]
    fn test_apply_restore_never_auto_freeze() {
        let config = DecayConfig::default();
        // 恢复事件即使结果低于阈值也不应冻结（恢复是提升操作）
        let out = apply_decay_pure(
            0.04,
            false,
            Instant::now() - std::time::Duration::from_secs(1),
            Instant::now(),
            DecayEventParam { kind: 3, severity: 0.0 },
            &config,
        )
        .expect("恢复应成功");
        assert!(!out.frozen, "Restore 不触发自动冻结");
    }

    #[test]
    fn test_apply_unknown_kind_errors() {
        let config = DecayConfig::default();
        let res = apply_decay_pure(
            0.5,
            false,
            Instant::now(),
            Instant::now(),
            DecayEventParam { kind: 99, severity: 0.0 },
            &config,
        );
        assert!(matches!(res, Err(DecayError::ConfigError(_))));
    }

    // ============================================================
    // 并行 vs 串行一致性（快照语义,统一 now,逐元素 + 顺序断言）
    // ============================================================

    /// 装配:构造 N 个能力的快照/事件,分别走串行与并行核心,逐元素断言
    fn assert_serial_matches_parallel(n: usize) {
        let (snapshots, events) = make_items(n);
        let snaps = Arc::new(snapshots);
        let evs = Arc::new(events);
        let cfg = Arc::new(test_config());
        let now = Instant::now();
        let serial = decay_batch_core(&snaps, &evs, &cfg, now, false);
        let parallel = decay_batch_core(&snaps, &evs, &cfg, now, true);
        assert_eq!(serial.len(), n, "串行结果数必须等于输入数");
        assert_eq!(parallel.len(), n, "并行结果数必须等于输入数");
        for (i, (s, p)) in serial.iter().zip(parallel.iter()).enumerate() {
            match (s, p) {
                (Ok(s), Ok(p)) => {
                    assert_eq!(
                        s.level.to_bits(),
                        p.level.to_bits(),
                        "item[{i}] level 必须与串行逐位一致(含顺序)"
                    );
                    assert_eq!(s.frozen, p.frozen, "item[{i}] frozen 必须与串行一致");
                }
                (Err(se), Err(pe)) => {
                    assert_eq!(se.to_string(), pe.to_string(), "item[{i}] 错误必须一致");
                }
                _ => panic!("item[{i}] 成功/失败路径不一致"),
            }
        }
    }

    #[test]
    fn test_parallel_matches_serial_large_batch() {
        // 12_000 ≥ Generic 阈值 10_000 → Rayon 分支
        assert_serial_matches_parallel(12_000);
    }

    #[test]
    fn test_parallel_matches_serial_small_batch() {
        // 90 < 阈值 → Inline 串行,结果一致
        assert_serial_matches_parallel(90);
    }

    // ============================================================
    // 边界:空输入 / 单元素 / 非整 chunk
    // ============================================================

    #[test]
    fn test_empty_items() {
        let snaps = Arc::new(Vec::new());
        let evs = Arc::new(Vec::new());
        let cfg = Arc::new(DecayConfig::default());
        let out = decay_batch_core(&snaps, &evs, &cfg, Instant::now(), true);
        assert!(out.is_empty());
    }

    #[test]
    fn test_single_item() {
        let (snapshots, events) = make_items(1);
        let snaps = Arc::new(snapshots);
        let evs = Arc::new(events);
        let cfg = Arc::new(DecayConfig::default());
        let out = decay_batch_core(&snaps, &evs, &cfg, Instant::now(), true);
        assert_eq!(out.len(), 1);
        assert!(out[0].is_ok());
    }

    #[test]
    fn test_odd_chunk_boundary() {
        // 65 = 1 个整 chunk(64) + 1 个残块 → 跨块拼接顺序正确
        let (snapshots, events) = make_items(65);
        let snaps = Arc::new(snapshots);
        let evs = Arc::new(events);
        let cfg = Arc::new(DecayConfig::default());
        let now = Instant::now();
        let serial = decay_batch_core(&snaps, &evs, &cfg, now, false);
        let parallel = decay_batch_core(&snaps, &evs, &cfg, now, true);
        assert_eq!(serial.len(), 65);
        for (i, (s, p)) in serial.iter().zip(parallel.iter()).enumerate() {
            match (s, p) {
                (Ok(s), Ok(p)) => {
                    assert_eq!(s.level.to_bits(), p.level.to_bits(), "item[{i}] 必须与串行逐位一致");
                }
                (Err(se), Err(pe)) => {
                    assert_eq!(se.to_string(), pe.to_string(), "item[{i}] 错误必须一致");
                }
                _ => panic!("item[{i}] 成功/失败路径不一致"),
            }
        }
    }

    // ============================================================
    // 回退开关（配置关闭 → 强制串行,结果与串行一致）
    // ============================================================

    #[test]
    fn test_config_disable_falls_back_to_serial() {
        let (snapshots, events) = make_items(12_000);
        let snaps = Arc::new(snapshots);
        let evs = Arc::new(events);
        let cfg = Arc::new(DecayConfig::default());
        let now = Instant::now();
        let disabled = decay_batch_core(&snaps, &evs, &cfg, now, false);
        let direct_serial = decay_batch_serial(&snaps, &evs, &cfg, now);
        assert_eq!(disabled.len(), direct_serial.len());
        for (i, (a, b)) in disabled.iter().zip(direct_serial.iter()).enumerate() {
            match (a, b) {
                (Ok(a), Ok(b)) => assert_eq!(a.level.to_bits(), b.level.to_bits(), "item[{i}]"),
                (Err(ae), Err(be)) => assert_eq!(ae.to_string(), be.to_string(), "item[{i}]"),
                _ => panic!("item[{i}] 成功/失败路径不一致"),
            }
        }
    }

    /// env 关闭开关 → 走串行（集成验证:进程级 env 首次读取即生效）
    ///
    /// WHY OnceLock:本测试设置 env 后,`env_no_parallel()` 首次读取即缓存关闭态;
    /// 若其他测试先调用导致已缓存 false,本测试跳过断言（不污染其他测试）。
    #[test]
    fn test_env_disable_falls_back_to_serial() {
        if env_no_parallel() {
            // 已有其他来源关闭 —— 缓存生效,直接验证串行一致性
            let (snapshots, events) = make_items(12_000);
            let snaps = Arc::new(snapshots);
            let evs = Arc::new(events);
            let cfg = Arc::new(DecayConfig::default());
            let now = Instant::now();
            let out = decay_batch_core(&snaps, &evs, &cfg, now, true);
            let serial = decay_batch_serial(&snaps, &evs, &cfg, now);
            assert_eq!(out.len(), serial.len());
            return;
        }
        // env 尚未缓存:设置后首次读取 → 关闭并行（OnceLock 一次性语义）
        std::env::set_var(ENV_NO_PARALLEL, "1");
        let (snapshots, events) = make_items(12_000);
        let snaps = Arc::new(snapshots);
        let evs = Arc::new(events);
        let cfg = Arc::new(DecayConfig::default());
        let now = Instant::now();
        let out = decay_batch_core(&snaps, &evs, &cfg, now, true);
        let serial = decay_batch_serial(&snaps, &evs, &cfg, now);
        assert_eq!(out.len(), serial.len());
        // 恢复 env,避免影响同进程其他测试（OnceLock 已缓存,恢复仅对子进程有意义）
        std::env::remove_var(ENV_NO_PARALLEL);
    }
}
