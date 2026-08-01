//! 厂商集中度免疫探针 — 供应商锁定的系统性免疫(MCA N7,ADR-067 决策 2)
//!
//! 对应架构层:L8 Parliament
//! 对应设计源:`Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.5 / §8.3 否决清单
//!
//! # 供应商锁定是系统级病理(N7)
//! "单一首选厂商默认配置"与 ImmuneSystem 的免疫哲学冲突:把"供应商绑定风险"
//! 定义为系统级病理并自动纠偏。单厂商流量占比 EWMA > 70% → 发布
//! `ProviderConcentrationWarning` → acb-governor 自动拉低该厂商路由权重。
//!
//! # 与 ImmuneSystem 三探针的关系
//! 本探针是**独立模块**,不并入 `ImmuneSystem` 的固定 `[ParadoxReport; 3]`
//! 数组(该数组与 ProbeType enum、级联风险评估深度耦合,扩容属侵入式变更)。
//! 第四悖论探针(厂商绑定)与三大悖论(记忆/推理/进化)正交,独立落地更清晰。
//!
//! # 数据流
//! 消费 `StreamSessionCompleted.route_key`(经 event-bus)累计厂商流量;
//! EWMA(α=0.1)平滑;均衡分布(7 厂商各 ~14%)不触发。
//!
//! # 红线
//! `ProviderConcentrationWarning` 为 **Normal 级**(不扩容 Critical 旁路,
//! 对齐 ADR-063"Normal 不扩容 Critical"的回归面最小化原则)。

use std::collections::HashMap;
use std::sync::Mutex;

/// EWMA 平滑系数(对齐 ADR-037 α=0.1)
const EWMA_ALPHA: f64 = 0.1;

/// 厂商集中度告警阈值(单厂商流量占比 EWMA > 70%)
pub const CONCENTRATION_THRESHOLD: f64 = 0.70;

/// 告警前的最小总样本数(预热阶段不告警)
///
/// WHY 预热阶段不告警: 启动瞬态中首个记录的厂商会
/// 瞬时占比 100%(其他厂商尚无样本),这是假阳性而非真实
/// 供应商锁定。仅总样本数超过阈值后(各厂商均有机会累计)
/// 才计量稳态集中度。
const WARMUP_MIN_SAMPLES: u64 = 20;

/// 厂商流量统计(EWMA 计数)
#[derive(Debug, Clone, Default)]
struct ProviderTraffic {
    /// 该厂商流量计数的 EWMA(平滑后的"加权次数")
    count_ewma: f64,
    /// 样本数
    samples: u64,
}

/// 厂商集中度探针 — 单厂商流量占比 EWMA 监控
///
/// # 线程安全
/// `Mutex<HashMap>` 同步聚合,锁内完成不跨 await;record 是微秒级同步操作。
///
/// # 计数模型(WHY 计数 EWMA 而非占比 EWMA)
/// 每厂商维护流量计数的 EWMA,占比 = 该厂商计数EWMA / 全部计数EWMA 之和。
/// 计数模型避免"占比 EWMA 在新厂商插入时瞬时膨胀"的缺陷:
/// 新厂商计数从 0 起渐进,不会瞬间占主。
#[derive(Debug, Default)]
pub struct ProviderConcentrationProbe {
    providers: Mutex<HashMap<String, ProviderTraffic>>,
}

impl ProviderConcentrationProbe {
    /// 创建空探针
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一次会话的厂商流量(消费 `StreamSessionCompleted.route_key`)
    ///
    /// route_key 形如 `provider/model`;提取 provider 段累计。
    /// 返回 (provider, share) 若该厂商占比 EWMA 超阈值(供发布告警)。
    pub fn record(&self, route_key: &str) -> Option<(String, f64)> {
        let provider = route_key.split('/').next().unwrap_or(route_key).to_string();
        let mut map = self.providers.lock().ok()?;
        // 命中厂商计数 EWMA 递增(首样本直取 1.0)
        let entry = map.entry(provider.clone()).or_default();
        entry.count_ewma = if entry.samples == 0 {
            1.0
        } else {
            EWMA_ALPHA * 1.0 + (1.0 - EWMA_ALPHA) * entry.count_ewma
        };
        entry.samples += 1;
        // 其余厂商计数 EWMA 衰减(不新增样本,仅平滑衰减)
        for (key, t) in map.iter_mut() {
            if *key != provider {
                t.count_ewma = EWMA_ALPHA * 0.0 + (1.0 - EWMA_ALPHA) * t.count_ewma;
            }
        }
        let share = Self::share_of_locked(&map, &provider);
        // 预热阶段不告警(总样本数未达阈值,避免启动瞬态假阳性)
        let total_samples: u64 = map.values().map(|t| t.samples).sum();
        if total_samples >= WARMUP_MIN_SAMPLES && share > CONCENTRATION_THRESHOLD {
            Some((provider, share))
        } else {
            None
        }
    }

    /// 计算厂商占比(计数EWMA / 总计数EWMA;内部辅助)
    fn share_of_locked(map: &HashMap<String, ProviderTraffic>, provider: &str) -> f64 {
        let total: f64 = map.values().map(|t| t.count_ewma).sum();
        if total <= 0.0 {
            return 0.0;
        }
        map.get(provider)
            .map(|t| t.count_ewma / total)
            .unwrap_or(0.0)
    }

    /// 查询厂商当前占比 EWMA(计数模型)
    pub fn share_of(&self, provider: &str) -> Option<f64> {
        let map = self.providers.lock().ok()?;
        if !map.contains_key(provider) {
            return None;
        }
        Some(Self::share_of_locked(&map, provider))
    }

    /// 列出所有超阈值厂商(供周期性检查;含预热阶段判定)
    pub fn concentrated_providers(&self) -> Vec<(String, f64)> {
        let Ok(map) = self.providers.lock() else {
            return Vec::new();
        };
        let total_samples: u64 = map.values().map(|t| t.samples).sum();
        if total_samples < WARMUP_MIN_SAMPLES {
            return Vec::new();
        }
        map.keys()
            .filter_map(|k| {
                let share = Self::share_of_locked(&map, k);
                if share > CONCENTRATION_THRESHOLD {
                    Some((k.clone(), share))
                } else {
                    None
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_provider_dominates_triggers_warning() {
        let probe = ProviderConcentrationProbe::new();
        // 单厂商连续流量 → 占比趋近 100% > 70%
        let mut warned = false;
        for _ in 0..30 {
            if probe.record("deep_seek/deepseek-v4-flash").is_some() {
                warned = true;
            }
        }
        assert!(warned, "单厂商垄断流量必须触发告警");
        let share = probe.share_of("deep_seek").unwrap();
        assert!(share > CONCENTRATION_THRESHOLD, "占比 {share} 应 > 0.70");
    }

    #[test]
    fn balanced_traffic_no_warning() {
        let probe = ProviderConcentrationProbe::new();
        // 7 厂商均衡流量(各 ~14%)→ 无告警
        let providers = [
            "zhipu/glm-5.2",
            "deep_seek/deepseek-v4-flash",
            "moonshot/kimi-k3",
            "mini_max/MiniMax-M3",
            "volcano_ark/doubao-seed-2.1-pro",
            "alibaba_cloud/qwen-max",
            "step_fun/step-3.5-flash-2603",
        ];
        let mut warned = false;
        for _ in 0..20 {
            for p in providers {
                if probe.record(p).is_some() {
                    warned = true;
                }
            }
        }
        assert!(!warned, "均衡分布不应触发告警");
        assert!(probe.concentrated_providers().is_empty());
    }

    #[test]
    fn concentration_recovers_when_traffic_diversifies() {
        let probe = ProviderConcentrationProbe::new();
        // 先单厂商垄断
        for _ in 0..30 {
            probe.record("deep_seek/deepseek-v4-flash");
        }
        assert!(probe.share_of("deep_seek").unwrap() > CONCENTRATION_THRESHOLD);
        // 再引入大量其他厂商流量 → 占比回落
        for _ in 0..60 {
            probe.record("zhipu/glm-5.2");
            probe.record("moonshot/kimi-k3");
        }
        // 垄断缓解(占比应显著下降)
        let share = probe.share_of("deep_seek").unwrap();
        assert!(share < 0.9, "流量多元化后占比应回落,实际 {share}");
    }

    #[test]
    fn route_key_extracts_provider_segment() {
        let probe = ProviderConcentrationProbe::new();
        probe.record("zhipu/glm-5.2");
        assert!(probe.share_of("zhipu").is_some());
        // model 段不计入 provider
        assert!(probe.share_of("glm-5.2").is_none());
    }
}
