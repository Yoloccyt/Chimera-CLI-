//! 规格与行为契约组 — Harness-as-Spec、六维控制面、行为契约、蓝图、变体、
//! 形式化属性、平台接地、ToolPlan DSL、调度契约（L0 契约）。
//!
//! 成员经 crate 根 re-export，旧路径 `crate::<leaf>` / `nexus_contracts::<leaf>` 不变。

pub mod behavior_contract;
pub mod blueprint;
pub mod formal_props;
pub mod harness_dimensions;
pub mod harness_spec;
pub mod platform_grounding;
pub mod scheduler_contract;
pub mod tool_plan;
pub mod variant;
