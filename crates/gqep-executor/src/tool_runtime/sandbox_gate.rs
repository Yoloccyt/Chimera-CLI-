//! 沙箱前置门 — 派发前必须通过的安全检查（T2-3）
//!
//! 本层只做同步校验，不持有运行时长任务、不跨 `.await`：
//! 请求在进入路由/传输之前先被策略层与沙箱层各自否决一次。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use seccore::{validate_command_verdict, validate_env, CommandPolicy, EnvPolicy};

use super::dispatch::{DispatchError, DispatchRequest};
use nexus_contracts::tool_dispatch::DispatchRequestRef;

/// 沙箱门 — 同步校验一次派发请求
pub trait SandboxGate: Send + Sync {
    /// 校验请求；`Err` 即 fail-closed，调用方不得继续派发
    fn check(&self, req: &DispatchRequest) -> Result<(), DispatchError>;

    /// 校验借用化请求 — 热路径零分配入口
    ///
    /// WHY 有这个方法：链内派发走 [`DispatchRequestRef`]（借用）以消除热路径的
    /// 2 次堆分配，但沙箱门的既有契约是 `&DispatchRequest`（拥有型）。
    ///
    /// 默认实现把借用视图**升级**为拥有型再调 `check` —— 语义完全等价，
    /// 但保留了那 2 次分配。实现者可覆写本方法走零分配路径
    /// （[`SeccoreSandboxGate`] 已覆写）。
    ///
    /// **判决必须等价**：对同一工具名与参数，`check` 与 `check_ref` 的结果
    /// 必须完全相同（含错误变体与文案）。这是链内借用化的硬约束 ——
    /// 若两者判决不同，借用路径就成了一条绕过安全校验的旁路。
    fn check_ref(&self, req: &DispatchRequestRef<'_>) -> Result<(), DispatchError> {
        self.check(&req.to_owned_request())
    }
}

/// 工具名派生策略缓存上界
///
/// WHY 有界：工具名来自调用方输入，无界缓存会变成内存放大面；超界后回退
/// 逐次 clone（判决不变，只损失缓存收益）。生产工具注册表的规模远小于此值。
const MAX_CACHED_TOOL_POLICIES: usize = 256;

/// 超长工具名不缓存（异常输入不得撑爆键空间）
const MAX_CACHED_TOOL_NAME_LEN: usize = 128;

/// seccore 支撑的沙箱门 — 命令白名单 + 危险模式 + 环境变量策略
#[derive(Debug)]
pub struct SeccoreSandboxGate {
    /// 命令策略（白名单 + 拦截模式）
    command_policy: CommandPolicy,
    /// 环境变量策略
    env_policy: EnvPolicy,
    /// 工具名 → 已插入该工具名的策略副本（只读派生记忆化）
    ///
    /// WHY：`check` 每次调用都要把整份白名单 `HashSet` clone 一次再插入工具名；
    /// 该派生是**只读且确定**的，按工具名缓存即可消除热路径的逐次分配。
    /// WHY `RwLock` 而非 `Mutex`：并行批量派发下读多写少，读锁可共享；
    /// 锁仅在同步 `check` 内短暂持有，**不跨 `.await`**（本模块无 async）。
    tool_policies: RwLock<HashMap<String, Arc<CommandPolicy>>>,
}

impl Clone for SeccoreSandboxGate {
    fn clone(&self) -> Self {
        Self {
            command_policy: self.command_policy.clone(),
            env_policy: self.env_policy.clone(),
            // WHY 空缓存而非复制条目：缓存是纯派生加速层，复制会让克隆成本
            // 退化为「逐工具 clone 策略」，与缓存目的相反。
            tool_policies: RwLock::new(HashMap::new()),
        }
    }
}

impl SeccoreSandboxGate {
    /// 以显式策略构造
    #[must_use]
    pub fn new(command_policy: CommandPolicy, env_policy: EnvPolicy) -> Self {
        Self {
            command_policy,
            env_policy,
            tool_policies: RwLock::new(HashMap::new()),
        }
    }

    /// 取（或派生并缓存）该工具名的策略
    ///
    /// 缓存键是工具名；值是「基础策略 + 该工具名白名单项」的只读副本。
    /// 命中即零 clone；未命中才 clone 一次并插入（每工具名至多一次）。
    fn policy_for(&self, tool_name: &str) -> Arc<CommandPolicy> {
        if tool_name.len() > MAX_CACHED_TOOL_NAME_LEN {
            return Arc::new(self.derive_policy(tool_name));
        }

        let cached = {
            let cache = self
                .tool_policies
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cache.get(tool_name).map(Arc::clone)
        };
        if let Some(policy) = cached {
            return policy;
        }

        let derived = Arc::new(self.derive_policy(tool_name));
        {
            let mut cache = self
                .tool_policies
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if cache.len() < MAX_CACHED_TOOL_POLICIES {
                // 并发竞态下可能已被其他线程写入：保留先写入者即可（两者等价）
                cache
                    .entry(tool_name.to_string())
                    .or_insert_with(|| Arc::clone(&derived));
            }
        }
        derived
    }

    /// 派生「基础策略 + 工具名白名单项」
    fn derive_policy(&self, tool_name: &str) -> CommandPolicy {
        let mut policy = self.command_policy.clone();
        // 工具名是逻辑标识（如 `repo.search`），不来自 shell 程序白名单；
        // 若直接沿用 default_secure 白名单，所有非 `echo/ls/...` 工具都会被判 Abuse。
        // 拦截模式（注入/提权/沙箱逃逸/数据泄露/篡改）仍对 `tool_name + args_json` 完整生效。
        policy.allowed_commands.insert(tool_name.to_lowercase());
        policy
    }

    /// 默认安全策略（`CommandPolicy` / `EnvPolicy` 的 `default_secure`）
    #[must_use]
    pub fn default_secure() -> Self {
        Self::new(CommandPolicy::default_secure(), EnvPolicy::default_secure())
    }
}

impl Default for SeccoreSandboxGate {
    fn default() -> Self {
        Self::default_secure()
    }
}

impl SandboxGate for SeccoreSandboxGate {
    fn check(&self, req: &DispatchRequest) -> Result<(), DispatchError> {
        // 派生策略按工具名记忆化（解题见 `policy_for`）；命中即零 clone。
        let policy = self.policy_for(&req.tool_name);

        // 纯判决入口：不再构造被丢弃的 Command/CommandSpec（ADR-256）。
        // 判决体与 check_ref 同源 ⇒ 拥有/借用两条路不可能给出不同结论。
        validate_command_verdict(&req.tool_name, &[req.args_json.as_str()], policy.as_ref())
            .map_err(|e| DispatchError::Sandbox(format!("{e}")))?;

        // 空 env 必然 Ok；保留调用以对齐「环境变量也要过策略」的语义。
        let env: HashMap<String, String> = HashMap::new();
        validate_env(&env, &self.env_policy).map_err(|e| DispatchError::Sandbox(format!("{e}")))?;

        Ok(())
    }

    /// 借用化校验 — 与 [`Self::check`] 判决完全等价，但省去 2 次 clone。
    ///
    /// WHY 判决等价是硬约束：本方法与 `check` 走**同一套**拦截模式与
    /// 同一份派生策略缓存，只是入参来源不同（借用 vs 拥有）。
    /// 两者判决若不同，借用路径就成了绕过安全校验的旁路 ——
    /// 由 `tool_runtime_borrowed_equivalence.rs` 的 property test 锁定。
    fn check_ref(&self, req: &DispatchRequestRef<'_>) -> Result<(), DispatchError> {
        // 派生策略按工具名记忆化（与 `check` 同路径、同缓存）。
        let policy = self.policy_for(req.tool_name);

        // 与 check 同一入口、同一判决体。原先这里 to_string() 两次只为喂给
        // 拥有型签名 —— 即「借用化省下的分配在入站处又付回去」，ADR-241 D6
        // 记录的净收益 0 正是这个构造造成的；本行把它抹平。
        validate_command_verdict(req.tool_name, &[req.args_json], policy.as_ref())
            .map_err(|e| DispatchError::Sandbox(format!("{e}")))?;

        let env: HashMap<String, String> = HashMap::new();
        validate_env(&env, &self.env_policy).map_err(|e| DispatchError::Sandbox(format!("{e}")))?;

        Ok(())
    }
}

/// 显式放行的沙箱门 — **仅**用于纯内存、无副作用的 in-proc 工具
///
/// WHY 显式命名：避免「空实现」被误读为默认安全策略；任何落盘/进程/网络
/// 工具都必须使用 [`SeccoreSandboxGate`]。
#[derive(Debug, Clone, Copy, Default)]
pub struct AllowAllSandboxGate;

impl AllowAllSandboxGate {
    /// 构造
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl SandboxGate for AllowAllSandboxGate {
    fn check(&self, _req: &DispatchRequest) -> Result<(), DispatchError> {
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn request(tool_name: &str, args_json: &str) -> DispatchRequest {
        DispatchRequest {
            tool_name: tool_name.to_string(),
            args_json: args_json.to_string(),
        }
    }

    /// 逻辑工具名无需白名单登记即可通过（白名单在 check 内按工具名补齐）
    #[test]
    fn allows_unlisted_tool_name() {
        let gate = SeccoreSandboxGate::default_secure();
        assert!(
            gate.check(&request("repo.search", "{\"q\":\"x\"}")).is_ok(),
            "逻辑工具名应通过白名单（拦截模式仍生效）"
        );
    }

    /// 注入形态参数仍被拦截（证明上一条不是「全放行」）
    #[test]
    fn blocks_injection_in_args() {
        let gate = SeccoreSandboxGate::default_secure();
        let err = gate
            .check(&request("repo.search", "$(rm -rf /)"))
            .expect_err("注入形态应被沙箱拦截");
        assert!(
            matches!(err, DispatchError::Sandbox(_)),
            "拦截应归因为 Sandbox，实际: {err:?}"
        );
    }

    /// AllowAll 门显式放行
    #[test]
    fn allow_all_permits_everything() {
        let gate = AllowAllSandboxGate::new();
        assert!(gate.check(&request("anything", "; rm -rf /")).is_ok());
    }

    // ----------------------------------------------------------
    // 派发策略缓存（性能增量 2026-11-06）
    // ----------------------------------------------------------

    /// 同一工具名重复检查只派生一次（缓存条目数不随调用次数增长）
    #[test]
    fn tool_policy_cache_reuses_derived_policy() {
        let gate = SeccoreSandboxGate::default_secure();
        assert!(gate.check(&request("repo.search", "{\"q\":\"a\"}")).is_ok());
        assert!(gate.check(&request("repo.search", "{\"q\":\"b\"}")).is_ok());
        let cache = gate
            .tool_policies
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(cache.len(), 1, "同一工具名应只缓存一条派生策略");
    }

    /// 缓存有界：超过上界后不再新增条目（判决不受影响）
    #[test]
    fn tool_policy_cache_is_bounded() {
        let gate = SeccoreSandboxGate::default_secure();
        for i in 0..(MAX_CACHED_TOOL_POLICIES + 44) {
            let tool = format!("tool-{i}");
            assert!(gate.check(&request(&tool, "{}")).is_ok(), "{tool} 应通过");
        }
        let cache = gate
            .tool_policies
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(
            cache.len(),
            MAX_CACHED_TOOL_POLICIES,
            "缓存条目数不得超过上界"
        );
    }

    /// 超长工具名走 fallback（不缓存），判决不变
    #[test]
    fn oversized_tool_name_skips_cache() {
        let gate = SeccoreSandboxGate::default_secure();
        let long = "x".repeat(MAX_CACHED_TOOL_NAME_LEN + 1);
        assert!(gate.check(&request(&long, "{}")).is_ok());
        let cache = gate
            .tool_policies
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(cache.is_empty(), "超长工具名不得进入缓存");
    }

    /// 缓存不改变判决：命中缓存后注入形态仍被拦截
    #[test]
    fn cache_does_not_change_verdicts() {
        let gate = SeccoreSandboxGate::default_secure();
        assert!(gate
            .check(&request("repo.search", "{\"q\":\"safe\"}"))
            .is_ok());
        let err = gate
            .check(&request("repo.search", "$(rm -rf /)"))
            .expect_err("注入形态应被拦截");
        assert!(
            matches!(err, DispatchError::Sandbox(_)),
            "拦截应归因为 Sandbox，实际: {err:?}"
        );
    }

    /// 克隆出的门是独立缓存实例（不复用旧条目）
    #[test]
    fn cloned_gate_starts_with_empty_cache() {
        let gate = SeccoreSandboxGate::default_secure();
        assert!(gate.check(&request("repo.search", "{}")).is_ok());
        let cloned = gate.clone();
        assert!(
            cloned
                .tool_policies
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty(),
            "克隆实例应从空缓存开始"
        );
        assert!(cloned.check(&request("repo.search", "{}")).is_ok());
    }
}
