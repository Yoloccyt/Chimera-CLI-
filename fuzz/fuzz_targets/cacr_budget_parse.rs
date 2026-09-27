//! Fuzz target: 成本/部署配置解析 + 成本熔断状态机模糊测试
//!
//! 对应架构层:L10 Interface(mca-gateway / CAF 渠道亲和成本面)
//!
//! # 后继关系(WHY 此 target 仍在盘上)
//! 本 target 原模糊"已退役的成本感知路由 crate"里的预算配置(`CacrConfig`)与
//! 守卫(`CacrGuard`)。该 crate 已随架构减法批次物理删除(唯一登记表
//! `docs/architecture/CODE_WIKI.md §3.12`),其"成本感知 + 预算守卫"语义由
//! `mca-gateway`(ADR-065)承接:
//! - 配置解析面 → `spec_loader::{parse_profile_toml, parse_spec_toml}`(TOML,用户可编辑)
//! - 预算守卫面 → `cost_guard::CostGuard`(`record()` 累计 + `check()` 熔断状态机)
//! 三个原目标逐条保留,不做覆盖缩水。
//!
//! # 模糊目标
//! 1. 两个 TOML 装载入口对**任意字节**必须全函数:畸形/截断/超大输入只产 Err,不 panic
//! 2. 装载成功后,派生访问器(`route_key`/`preferred_dialect`)不 panic
//! 3. `CostGuard` 在任意 (预算上限, 成本, 时钟) 三元组下不 panic,且:
//!    - 累计成本单调不减(禁止回绕 —— 回绕即熔断静默旁路)
//!    - `spent < limit` 时 `check()` 恒 Ok(未超限不得拒绝)
//!    - `check()` 不改变累计成本(观测面不得写状态)
//!
//! # 实战收获(本 target 的分析已抓到并修复两处真实缺陷)
//! `CostGuard` 原实现用裸 `+` 计算熔断重开时刻、用 `fetch_add` 累计成本:
//! 前者在 `now_secs` 接近 `i64::MAX` 时于 debug 下 overflow-panic、release 下回绕成
//! 负哨兵,使 `now < circuit_open_until` 恒假 → 每次 check 都走半开放行分支;
//! 后者在累加溢出时回绕,使 `spent < limit` 恒真 → 熔断永久旁路。
//! 两处均已改为饱和运算,回归测试见 `crates/mca-gateway/src/cost_guard.rs` 的
//! `max_clock_value_keeps_circuit_open` / `record_saturates_and_never_wraps`。
//!
//! # 命名说明(勿误判为待清理残骸)
//! `[[bin]]` 名保留 `cacr_budget_parse`:该名字是 `.github/workflows/fuzz.yml`
//! matrix 的字面项,改名须同步 CI matrix(属仓库工件变更,需单独授权)。
//! target 内容已完全迁移到后继面,由本注释与 `check_retired_crate_residue` 门佐证。
//!
//! # 运行方式(需 nightly)
//! ```bash
//! cargo +nightly fuzz run cacr_budget_parse
//! ```
//
// 注意:此文件不添加 #![forbid(unsafe_code)],因为 libfuzzer-sys 的
// fuzz_target! 宏内部展开为 FFI 调用(unsafe),与 forbid 冲突。
// fuzz crate 独立于主 workspace,不影响成员 crate 的 forbid 覆盖率。

// Windows-GNU 下使用 stub 宏(chimera_fuzz),非 Windows 使用 libfuzzer_sys
#[cfg(windows)]
use chimera_fuzz::fuzz_target;
#[cfg(not(windows))]
use libfuzzer_sys::fuzz_target;
use mca_gateway::cost_guard::CostGuard;
use mca_gateway::spec_loader::{parse_profile_toml, parse_spec_toml};
use nexus_contracts::affinity::ProtocolDialect;

/// 从输入第 `offset` 字节起取一个 u64;短输入循环复用自身字节,
/// 使取值域仍由 fuzz 输入决定(而非常量偏斜到小数域)。
fn head_u64(data: &[u8], offset: usize) -> u64 {
    let mut buf = [0u8; 8];
    for (i, slot) in buf.iter_mut().enumerate() {
        *slot = if data.is_empty() {
            0
        } else {
            data[(offset + i) % data.len()]
        };
    }
    u64::from_le_bytes(buf)
}

fuzz_target!(|data: &[u8]| {
    // 统一按 UTF-8 有损解码:TOML 装载入口收 &str,畸形字节必须走 Err 而非 panic
    let text = String::from_utf8_lossy(data);

    // === 目标1:部署 Profile TOML 解析全函数 ===
    if let Ok(profile) = parse_profile_toml(&text) {
        // server_params 的"零解析零执行"边界:解析成功即 schema 通过,
        // 客户端只消费 client_relevant 段
        let _window = profile.client_relevant.context_window_override;
        let _is_object = profile.server_params.is_object();
    }

    // === 目标2:affinity spec 装载 + 派生访问器 ===
    if let Ok(specs) = parse_spec_toml(&text) {
        for spec in &specs {
            let _key = spec.route_key();
            let _preferred = spec.preferred_dialect();
            let _dialects = [
                spec.supports_dialect(ProtocolDialect::OpenAiChat),
                spec.supports_dialect(ProtocolDialect::AnthropicMessages),
                spec.supports_dialect(ProtocolDialect::OpenAiResponses),
            ];
        }
    }

    // === 目标3:成本熔断状态机在任意三元组下不 panic、不回绕 ===
    // 三元组全部取自输入(含 i64::MAX 邻域),正是抓到裸 `+` 溢出的取值域
    let limit_micro = head_u64(data, 8);
    let cost_micro = head_u64(data, 16);
    let now_secs = head_u64(data, 24) as i64;

    // 上限取 None / Some 两档:不设限与设限两条分支都须全函数
    let guard = if limit_micro % 2 == 0 {
        CostGuard::new(None)
    } else {
        CostGuard::new(Some(limit_micro))
    };

    guard.record(cost_micro);
    let spent = guard.spent_micro();

    // 不变量 A:累计成本单调不减(record 不得把 spent 绕回低位)
    assert!(
        spent >= cost_micro,
        "record 回绕:cost={cost_micro} 后 spent={spent}"
    );

    if let Some(limit) = guard.budget_limit_micro() {
        // 不变量 B:未超限必须放行,与 now_secs 取值无关
        if spent < limit {
            assert!(
                guard.check(now_secs).is_ok(),
                "spent={spent} < limit={limit} 时 now_secs={now_secs} 被误拒"
            );
        }
    }

    // 不变量 C:极端时钟下反复 check 仍全函数(状态机可重入),且不写成本
    for probe in [now_secs, now_secs.wrapping_add(1), 0, i64::MIN, i64::MAX] {
        let _ = guard.check(probe);
    }
    assert_eq!(
        guard.spent_micro(),
        spent,
        "check() 属只读判定,不得改变累计成本"
    );
});
