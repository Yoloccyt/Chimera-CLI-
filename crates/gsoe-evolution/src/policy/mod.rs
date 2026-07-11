//! 策略模块 — GRPO 采样、变异与适应度评估
//!
//! 对应架构层:L5 Knowledge
//!
//! # 子模块
//! - `grpo`:GRPO 组采样与组内相对优势计算
//! - `mutation`:策略参数变异(Gaussian/Uniform/Elite)
//! - `fitness`:基于规则的适应度评估(Week 7 接入真实模型)

pub mod fitness;
pub mod grpo;
pub mod mutation;
pub mod trainer;
