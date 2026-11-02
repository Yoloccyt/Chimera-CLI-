//! `/bench` 一期：build.rs 静态计数 provider（chimera-cli 组合根，L10）。
//!
//! 对应任务：v3.0 §(b.6)/§5.2「/bench 面板——数据先行」；本批为 `BenchStatsProvider`
//! 的首个生产实现（此前面板只有 bench.rs 内 MockProvider，生产零接线）。
//!
//! # 诚实边界
//! - **有真实源的字段**：`total_benches`（全 workspace `[[bench]]` 数）与
//!   `slo_thresholds_total`（`scripts/perf_thresholds.toml` 段数）——由 `build.rs`
//!   构建期扫描清单生成，非硬编码（杜绝计数漂移）。
//! - **无运行时生产者的字段**：`slo_breaches`/`retrieval_p99_ms`/`agreement_rate`/
//!   `certifiable_ratio` 一律 `None`（面板诚实标 N/A，绝不回填 0 造假）；待各自生产者
//!   落地（如 mlc 检索延迟直方图、评测层一致率计数）后再接，属独立后续任务。
//!
//! # 依赖方向
//! chimera-cli(L10) → chimera-tui(L10 同层) + nexus-contracts 类型经面板 re-export，
//! 合规。provider 放 L10 而非 L10-UI 内，是为读构建期生成的常量并规避 chimera-tui
//! 反向依赖度量/评测 crate（D-1 依赖倒置）。

include!(concat!(env!("OUT_DIR"), "/bench_consts.rs"));

use chimera_tui::panels::{BenchStats, BenchStatsProvider};

/// 诚实的静态基准统计提供者（仅填 build.rs 可派生的真实静态计数）。
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct StaticBenchProvider;

impl BenchStatsProvider for StaticBenchProvider {
    fn bench_stats(&self) -> BenchStats {
        BenchStats {
            total_benches: TOTAL_BENCHES,
            slo_thresholds_total: SLO_THRESHOLD_TOTAL,
            // 无运行时生产者 → 诚实 None（面板渲染 N/A）。
            slo_breaches: None,
            retrieval_p99_ms: None,
            agreement_rate: None,
            certifiable_ratio: None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module idiom; E-5 targets production code
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// 独立重算 `crates/*/Cargo.toml` 的 `[[bench]]` 数（与 build.rs 双法对拍，防漂移）。
    fn recount_benches() -> u32 {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut n = 0u32;
        if let Ok(rd) = std::fs::read_dir(root.join("crates")) {
            for e in rd.flatten() {
                let cand = e.path().join("Cargo.toml");
                if cand.is_file() {
                    if let Ok(s) = std::fs::read_to_string(&cand) {
                        n += s
                            .lines()
                            .filter(|l| l.trim_start().starts_with("[[bench]]"))
                            .count() as u32;
                    }
                }
            }
        }
        n
    }

    /// 独立重算 `perf_thresholds.toml` 的 `[[` 段数。
    fn recount_thresholds() -> u32 {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/perf_thresholds.toml");
        std::fs::read_to_string(path)
            .map(|s| {
                s.lines()
                    .filter(|l| l.trim_start().starts_with("[["))
                    .count() as u32
            })
            .unwrap_or(0)
    }

    #[test]
    fn provider_static_counts_match_independent_recount() {
        // build.rs 生成值须与测试内独立重算逐字一致（同仓清单，两处不同实现互证）
        let s = StaticBenchProvider.bench_stats();
        assert_eq!(
            s.total_benches,
            recount_benches(),
            "TOTAL_BENCHES 须与独立重算 [[bench]] 数一致"
        );
        assert_eq!(
            s.slo_thresholds_total,
            recount_thresholds(),
            "SLO_THRESHOLD_TOTAL 须与独立重算 perf_thresholds 段数一致"
        );
        assert!(s.total_benches > 0, "workspace 必有已注册 criterion 基准");
    }

    #[test]
    fn sourceless_runtime_fields_are_none_not_zero() {
        // 诚实性核心断言：无运行时生产者的四项必须是 None，不得为 0（防造假）。
        let s = StaticBenchProvider.bench_stats();
        assert!(s.slo_breaches.is_none());
        assert!(s.retrieval_p99_ms.is_none());
        assert!(s.agreement_rate.is_none());
        assert!(s.certifiable_ratio.is_none());
    }
}
