//! Meta-Agent 适配器 — 外部 Harness 描述规范化(polish-v2.7 closure Stage B-8)
//!
//! 对应架构层: L5 Knowledge(gsoe-evolution spec_loader 扩展)
//! 对应 ADR: ADR-049 决策 1(meta-agent-adapter 降级档:gsoe-evolution spec_loader 扩展)
//! 对应设计源: `chimera_ultimate_polish_v2.7.md` §3.2(CMU meta-agent-adapter:自动适配 Harness)
//!
//! # 降级映射(ADR-049)
//!
//! | 方案原设计(CMU) | 骨架降级实现 |
//! |---|---|
//! | Meta-Agent 自动分析外部 Harness 并适配 | 规则式规范化:外部描述结构 → HarnessSpec TOML |
//! | LLM 驱动的能力映射 | 显式字段映射(name/steps/constraints 一一对应) |
//! | 自动接线执行 | **不接线**——产出经 SpecLoader 全量校验的 HarnessSpec,登记走 SpecRegistry 既有通路 |
//!
//! # 设计决策(WHY)
//!
//! - **委托 SpecLoader 而非直接构造 HarnessSpec**: 适配产物必须经过与手写 spec
//!   完全相同的校验管线(L0 validate + ImmutableSurface 守护 + 4 强制门严格检查),
//!   委托 `SpecLoader::load_from_str` 天然继承全部防线,适配器自身零校验逻辑重复
//! - **强制门自动注入**: 外部来源不了解本项目的 4 个强制 acceptance_gates
//!   (tests_pass/bench_no_regression/invariants_clean/redline_scan_clean),
//!   适配器无条件注入——外部 Harness 进入本系统必须接受本系统的验收门治理
//! - **TOML 转义经 `toml` crate 序列化**: 外部字符串字段(name/property 等)可能
//!   含引号/换行,手工拼接有注入风险,统一用 `toml::Value` 序列化保证转义正确
//!
//! # R2 冻结声明(ADR-042)
//!
//! 本适配器为纯规则式文本规范化,无任何学习/训练路径;
//! 标识符规避 5 个 R2 扫描关键词(gsoe-evolution 属 CI 扫描目录)。
//!
//! # 使用示例
//!
//! ```
//! use gsoe_evolution::meta_adapter::{ExternalHarnessDescriptor, ExternalStep, MetaAgentAdapter};
//!
//! let descriptor = ExternalHarnessDescriptor {
//!     source: "cmu-agent-x".to_string(),
//!     name: "external-fuzz-harness".to_string(),
//!     version: 1,
//!     steps: vec![ExternalStep {
//!         name: "gen_input".to_string(),
//!         actors: vec!["Architect.propose".to_string()],
//!         guard: Some("no_panic".to_string()),
//!     }],
//!     invariants: vec![("no_panic".to_string(), "must_not_panic".to_string())],
//! };
//!
//! let spec = MetaAgentAdapter::adapt(&descriptor).unwrap();
//! assert_eq!(spec.meta.name, "external-fuzz-harness");
//! assert_eq!(spec.hops.len(), 1);
//! ```

use crate::spec_loader::{SpecLoader, SpecLoaderError};
use nexus_contracts::{HarnessSpec, REQUIRED_ACCEPTANCE_GATES};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ============================================================
// 外部 Harness 描述结构
// ============================================================

/// 外部执行步骤 — 异构 Harness 的最小步骤抽象
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalStep {
    /// 步骤名称(映射为 HopSpec.name)
    pub name: String,
    /// 执行者序列(映射为 HopSpec.order,如 "Architect.propose")
    pub actors: Vec<String>,
    /// 关联的不变量守卫(映射为 HopSpec.contracts 引用;None = 无守卫)
    pub guard: Option<String>,
}

/// 外部 Harness 描述 — 适配器的输入
///
/// 这是异构外部 Harness(CMU meta-agent 语境下的"待适配 Harness")的
/// 最小公共结构:调用方负责将外部格式(JSON/YAML/API 响应)解析到此结构,
/// 适配器只负责规范化到本项目 HarnessSpec DSL。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalHarnessDescriptor {
    /// 来源标识(如 "cmu-agent-x",记入 auxiliary 供审计追溯)
    pub source: String,
    /// Harness 名称(映射为 HarnessMeta.name)
    pub name: String,
    /// 版本号(映射为 HarnessMeta.version,必须 ≥1)
    pub version: u32,
    /// 执行步骤(映射为 hops)
    pub steps: Vec<ExternalStep>,
    /// 不变量列表(name, property)(映射为 contracts)
    pub invariants: Vec<(String, String)>,
}

// ============================================================
// 适配器错误
// ============================================================

/// Meta-Agent 适配器错误
///
/// WHY 独立 enum: 适配失败分"输入不合格"(适配器自身拒绝)与
/// "规范化产物未过校验"(SpecLoader 拒绝)两类,调用方处置路径不同
/// (前者修外部描述,后者通常是外部 Harness 触碰了不可进化面)。
#[derive(Debug, Error)]
pub enum MetaAdapterError {
    /// 外部描述缺少必要字段(空名称/零版本/无步骤)
    #[error("外部 Harness 描述不合格: {reason}")]
    InvalidDescriptor {
        /// 不合格原因
        reason: String,
    },

    /// 步骤引用了未声明的不变量守卫
    #[error("步骤 '{step}' 引用未声明的守卫 '{guard}'")]
    UndeclaredGuard {
        /// 引用方步骤名
        step: String,
        /// 未声明的守卫名
        guard: String,
    },

    /// TOML 序列化失败(字段值无法表示为 TOML)
    #[error("外部描述 TOML 序列化失败: {0}")]
    TomlSerialize(#[from] toml::ser::Error),

    /// 规范化产物未通过 SpecLoader 校验(通常是触碰不可进化面)
    #[error("规范化产物校验失败: {0}")]
    SpecRejected(#[from] SpecLoaderError),
}

// ============================================================
// MetaAgentAdapter — 规则式规范化
// ============================================================

/// Meta-Agent 适配器 — 外部 Harness 描述 → 合法 HarnessSpec
///
/// 无状态关联函数集合(与 SpecLoader 同款零状态模式):
/// 规范化 = 字段映射 + 强制门注入 + TOML 生成 + SpecLoader 全量校验。
pub struct MetaAgentAdapter;

impl MetaAgentAdapter {
    /// 将外部 Harness 描述规范化为经过全量校验的 HarnessSpec
    ///
    /// # 流程
    /// 1. 输入预检(名称/版本/步骤守卫引用完整性)
    /// 2. 生成 HarnessSpec DSL TOML(强制门自动注入 + 来源审计字段)
    /// 3. 委托 `SpecLoader::load_from_str` 执行 L0 校验 + ImmutableSurface
    ///    守护 + acceptance_gates 严格检查
    ///
    /// # 错误
    /// - `InvalidDescriptor`: 空名称 / 零版本 / 无步骤
    /// - `UndeclaredGuard`: 步骤引用了 invariants 中不存在的守卫
    /// - `SpecRejected`: 产物未过 SpecLoader 校验(如外部名称撞不可进化面)
    pub fn adapt(descriptor: &ExternalHarnessDescriptor) -> Result<HarnessSpec, MetaAdapterError> {
        Self::precheck(descriptor)?;
        let toml_text = Self::to_spec_toml(descriptor)?;
        Ok(SpecLoader::load_from_str(&toml_text)?)
    }

    /// 输入预检 — 系统边界校验(外部输入不可信)
    fn precheck(descriptor: &ExternalHarnessDescriptor) -> Result<(), MetaAdapterError> {
        if descriptor.name.trim().is_empty() {
            return Err(MetaAdapterError::InvalidDescriptor {
                reason: "name 为空".to_string(),
            });
        }
        if descriptor.version == 0 {
            return Err(MetaAdapterError::InvalidDescriptor {
                reason: "version 必须 ≥1".to_string(),
            });
        }
        if descriptor.steps.is_empty() {
            return Err(MetaAdapterError::InvalidDescriptor {
                reason: "steps 为空(无步骤的 Harness 无适配意义)".to_string(),
            });
        }
        // 守卫引用完整性:步骤引用的守卫必须在 invariants 中声明
        for step in &descriptor.steps {
            if let Some(guard) = &step.guard {
                let declared = descriptor.invariants.iter().any(|(name, _)| name == guard);
                if !declared {
                    return Err(MetaAdapterError::UndeclaredGuard {
                        step: step.name.clone(),
                        guard: guard.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    /// 生成 HarnessSpec DSL TOML 文本
    ///
    /// 字符串值统一经 `toml::Value` 序列化(防注入,见模块头设计决策)。
    fn to_spec_toml(descriptor: &ExternalHarnessDescriptor) -> Result<String, MetaAdapterError> {
        // 字符串 → 合法 TOML 字面量(含引号与转义)
        let quote = |s: &str| toml::Value::String(s.to_string()).to_string();

        let mut out = String::new();
        // [meta] — 外部来源恒为可进化(immutable=false):不可进化面只能由本系统内部声明
        out.push_str("[meta]\n");
        out.push_str(&format!("name = {}\n", quote(&descriptor.name)));
        out.push_str(&format!("version = {}\n", descriptor.version));
        out.push_str("immutable = false\n\n");

        // [[contracts]] — 不变量映射
        for (name, property) in &descriptor.invariants {
            out.push_str("[[contracts]]\n");
            out.push_str(&format!("name = {}\n", quote(name)));
            out.push_str(&format!("property = {}\n\n", quote(property)));
        }

        // [[hops]] — 步骤映射
        for step in &descriptor.steps {
            out.push_str("[[hops]]\n");
            out.push_str(&format!("name = {}\n", quote(&step.name)));
            let order_items: Vec<String> = step.actors.iter().map(|a| quote(a)).collect();
            out.push_str(&format!("order = [{}]\n", order_items.join(", ")));
            let contracts_items: Vec<String> = step.guard.iter().map(|g| quote(g)).collect();
            out.push_str(&format!("contracts = [{}]\n\n", contracts_items.join(", ")));
        }

        // [auxiliary] — 强制门无条件注入 + 来源审计字段
        let gates: Vec<String> = REQUIRED_ACCEPTANCE_GATES.iter().map(|g| quote(g)).collect();
        out.push_str("[auxiliary]\n");
        out.push_str(&format!("acceptance_gates = [{}]\n", gates.join(", ")));
        out.push_str(&format!("adapted_from = {}\n", quote(&descriptor.source)));
        Ok(out)
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_descriptor() -> ExternalHarnessDescriptor {
        ExternalHarnessDescriptor {
            source: "cmu-agent-x".to_string(),
            name: "external-fuzz-harness".to_string(),
            version: 1,
            steps: vec![ExternalStep {
                name: "gen_input".to_string(),
                actors: vec!["Architect.propose".to_string()],
                guard: Some("no_panic".to_string()),
            }],
            invariants: vec![("no_panic".to_string(), "must_not_panic".to_string())],
        }
    }

    #[test]
    fn test_adapt_valid_descriptor() {
        let spec = MetaAgentAdapter::adapt(&valid_descriptor()).unwrap();
        assert_eq!(spec.meta.name, "external-fuzz-harness");
        assert_eq!(spec.meta.version, 1);
        // 外部来源恒为可进化
        assert!(!spec.meta.immutable);
        assert_eq!(spec.contracts.len(), 1);
        assert_eq!(spec.hops.len(), 1);
    }

    #[test]
    fn test_adapt_injects_required_gates() {
        let spec = MetaAgentAdapter::adapt(&valid_descriptor()).unwrap();
        let auxiliary = spec.auxiliary.expect("auxiliary 必须存在");
        for gate in REQUIRED_ACCEPTANCE_GATES {
            assert!(auxiliary.contains(gate), "强制门 {gate} 未注入");
        }
        // 来源审计字段
        assert!(auxiliary.contains("cmu-agent-x"));
    }

    #[test]
    fn test_adapt_rejects_empty_name() {
        let mut d = valid_descriptor();
        d.name = "  ".to_string();
        assert!(matches!(
            MetaAgentAdapter::adapt(&d),
            Err(MetaAdapterError::InvalidDescriptor { .. })
        ));
    }

    #[test]
    fn test_adapt_rejects_zero_version() {
        let mut d = valid_descriptor();
        d.version = 0;
        assert!(matches!(
            MetaAgentAdapter::adapt(&d),
            Err(MetaAdapterError::InvalidDescriptor { .. })
        ));
    }

    #[test]
    fn test_adapt_rejects_empty_steps() {
        let mut d = valid_descriptor();
        d.steps.clear();
        assert!(matches!(
            MetaAgentAdapter::adapt(&d),
            Err(MetaAdapterError::InvalidDescriptor { .. })
        ));
    }

    #[test]
    fn test_adapt_rejects_undeclared_guard() {
        let mut d = valid_descriptor();
        d.steps[0].guard = Some("ghost_guard".to_string());
        match MetaAgentAdapter::adapt(&d) {
            Err(MetaAdapterError::UndeclaredGuard { step, guard }) => {
                assert_eq!(step, "gen_input");
                assert_eq!(guard, "ghost_guard");
            }
            other => panic!("期望 UndeclaredGuard,实际: {other:?}"),
        }
    }

    #[test]
    fn test_adapt_rejects_immutable_surface_collision() {
        // 外部描述的守卫名撞不可进化面标识 → SpecLoader 校验层拒绝
        // (适配器自身不重复实现该防线,委托 SpecLoader,见模块头设计决策)
        let mut d = valid_descriptor();
        d.invariants = vec![("g1".to_string(), "p1".to_string())];
        d.steps[0].guard = Some("g1".to_string());
        // order 中注入不可进化面标识
        d.steps[0].actors = vec!["critical-budget-exceeded".to_string()];
        assert!(matches!(
            MetaAgentAdapter::adapt(&d),
            Err(MetaAdapterError::SpecRejected(_))
        ));
    }

    #[test]
    fn test_adapt_escapes_special_characters() {
        // 名称含引号:必须正确转义而非注入 TOML 结构
        let mut d = valid_descriptor();
        d.name = "ext\"harness".to_string();
        let spec = MetaAgentAdapter::adapt(&d).unwrap();
        assert_eq!(spec.meta.name, "ext\"harness");
    }

    #[test]
    fn test_adapt_step_without_guard() {
        let mut d = valid_descriptor();
        d.steps.push(ExternalStep {
            name: "no_guard_step".to_string(),
            actors: vec!["Executor.run".to_string()],
            guard: None,
        });
        let spec = MetaAgentAdapter::adapt(&d).unwrap();
        assert_eq!(spec.hops.len(), 2);
        // 无守卫步骤的 contracts 为空
        assert!(spec.hops[1].contracts.is_empty());
    }
}
