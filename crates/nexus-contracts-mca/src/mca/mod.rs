//! MCA 亲和与消息协议组 — 模型亲和契约、上下文能力档案、统一价格向量、
//! OmniMessage 协议（L0 契约）。
//!
//! 成员经 crate 根 re-export，旧路径 `crate::<leaf>` / `nexus_contracts::<leaf>` 不变。

pub mod affinity;
pub mod context_capability;
pub mod omni_message;
pub mod price_vector;
