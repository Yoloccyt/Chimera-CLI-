//! SpecRegistry — HarnessSpec 版本化注册表（P4-W15.2.1）
//!
//! 对应架构层: **L5 Knowledge**（gsoe-evolution）
//! 对应 ADR: **ADR-031**（Harness-as-Spec + omega-learner 边界）
//! 对应任务: **P4-W15.2.1**（复用 `EvolutionResult` 谱系机制作为 lineage 存储）
//!
//! # 核心职责
//!
//! 1. **版本化存储**: 以 `(name, version)` 为 key 存储多个版本的 HarnessSpec
//! 2. **谱系追踪**: 复用 `HarnessMeta.version` / `HarnessMeta.parent` 字段建立父子链
//! 3. **A/B 测试**: 维持 `active`（生产版本）与 `candidate`（候选版本）双轨
//! 4. **一键回滚**: 通过 parent 链回滚到前一版本
//! 5. **不可进化面守护**: register 时调用 `HarnessSpec::validate()` 拒绝任何不可进化面修改
//!
//! # 设计决策（WHY）
//!
//! - **复用 HarnessMeta 谱系字段**：HarnessSpec 的 `meta.version`（单调递增）与
//!   `meta.parent`（父版本号）已在 P4-W15.1.1 实现，SpecRegistry 直接使用这两个字段
//!   建立 lineage 链，无需重新发明谱系机制。这与设计文档 §7.2
//!   `[harness.meta] parent = 46` 一致
//!
//! - **双轨 active/candidate 而非覆盖**：A/B 测试要求同时保留两个版本以支持
//!   指标对比与一键回滚。`promote_candidate()` 提升 candidate 为 active，
//!   原 active 保留在 `specs` 表中可随时回滚
//!
//! - **immutable spec 禁止覆盖**：若已注册 spec 的 `meta.immutable = true`，
//!   禁止注册同名新版本。这是不可进化面的运行时守护（compile-time 守护由
//!   `HarnessSpec::validate()` 的 `ImmutableSurfaceViolation` 提供）
//!
//! - **SpecRegistryError 独立于 SpecLoaderError**：注册表错误语义不同
//!   （版本冲突/父版本缺失/无候选/无父版本可回滚），需要独立错误分类。
//!   仍实现 `std::error::Error` 供 anyhow 链式调用
//!
//! - **零状态初始化**：`SpecRegistry::new()` 创建空表，register 时填充。
//!   无需持久化文件 IO（持久化由上层 GsoeEvolutionEngine 负责）
//!
//! # 流程图
//!
//! ```text
//! HarnessSpec (version=5, parent=4)
//!     │
//!     ▼
//! SpecRegistry::register(spec)
//!     │
//!     ├──> spec.validate() ──> 不可进化面检查
//!     │                         ├── Ok ──> 继续
//!     │                         └── Err ──> 返回 ValidationFailed
//!     │
//!     ├──> 检查 parent 链存在性 ──> parent_version 必须已注册
//!     │
//!     ├──> 检查 immutable 覆盖 ──> 同名旧版本 immutable=true 禁止覆盖
//!     │
//!     ├──> 插入 specs[name][5] = spec
//!     │
//!     └──> 若 parent=None（初始版本）──> 设置 active[name] = 5
//! ```
//!
//! # 防注入保证（P4-W15.2.3）
//!
//! - register 仅接受已 validate 的 HarnessSpec，不解析 TOML
//! - 所有查询方法均 `&self`（不可变借用），无法修改注册表
//! - rollback / promote_candidate / set_candidate 为 `&mut self`，仅修改
//!   active/candidate 指针，不修改 spec 内容本身

use crate::error::GsoeError;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_contracts::{HarnessSpec, HarnessSpecError, ImmutableSurface};
use std::collections::HashMap;
use thiserror::Error;

// ============================================================
// SpecRegistryError — 注册表错误类型（P4-W15.2.1）
// ============================================================

/// SpecRegistry 错误 — 版本冲突/父版本缺失/无激活版本等注册表操作失败
///
/// # 错误分类
///
/// | 错误变体 | 含义 | 触发场景 |
/// |---------|------|---------|
/// | `VersionConflict` | 版本冲突 | register 时 (name, version) 已存在 |
/// | `SpecNotFound` | spec 不存在 | get/get_active 时 name 或 version 不存在 |
/// | `ParentMissing` | 父版本缺失 | register 时 parent 版本未注册 |
/// | `NoActiveVersion` | 无激活版本 | get_active 时 name 未注册任何 active |
/// | `NoCandidate` | 无候选版本 | promote_candidate 时未设置 candidate |
/// | `NoParentToRollback` | 无父版本可回滚 | rollback 时 active 版本无 parent |
/// | `ImmutableSpecOverwrite` | 不可进化面覆盖 | register 时同名旧版本 immutable=true |
/// | `ValidationFailed` | spec 校验失败 | register 时 validate() 返回错误 |
#[derive(Debug, Error)]
pub enum SpecRegistryError {
    /// 版本冲突 — (name, version) 已存在
    #[error("spec 版本冲突: name={name} version={version} 已存在")]
    VersionConflict {
        /// 冲突的 spec 名称
        name: String,
        /// 冲突的版本号
        version: u32,
    },

    /// spec 不存在 — 查询的 name/version 未注册
    #[error("spec 不存在: name={name} version={version}")]
    SpecNotFound {
        /// 查询的 spec 名称
        name: String,
        /// 查询的版本号
        version: u32,
    },

    /// 父版本缺失 — register 时 parent 指向未注册的版本
    ///
    /// WHY 单独错误: 谱系完整性要求 parent 必须已注册，
    /// 否则 lineage 链断裂，rollback 无法工作
    #[error("父版本缺失: name={name} parent={parent}（需先注册 parent 版本）")]
    ParentMissing {
        /// spec 名称
        name: String,
        /// 缺失的父版本号
        parent: u32,
    },

    /// 无激活版本 — get_active 时 name 未注册任何 active
    #[error("无激活版本: name={name}（需先 register 初始版本或 promote_candidate）")]
    NoActiveVersion {
        /// 查询的 spec 名称
        name: String,
    },

    /// 无候选版本 — promote_candidate 时未设置 candidate
    #[error("无候选版本: name={name}（需先 set_candidate 设置候选）")]
    NoCandidate {
        /// 查询的 spec 名称
        name: String,
    },

    /// 无父版本可回滚 — rollback 时 active 版本无 parent
    ///
    /// WHY: 初始版本（version=1, parent=None）无父版本可回滚
    #[error("无父版本可回滚: name={name} current={current}（已是初始版本）")]
    NoParentToRollback {
        /// spec 名称
        name: String,
        /// 当前 active 版本号
        current: u32,
    },

    /// 不可进化面覆盖 — 同名旧版本 immutable=true，禁止注册新版本
    ///
    /// WHY 设计文档 §7.2 不可进化面守护: immutable spec 代表硬编码规则
    /// （如 13 条红线 + Critical 清单），不允许被新版本覆盖
    #[error("不可进化面 spec 不可覆盖: name={name}（旧版本标记 immutable=true）")]
    ImmutableSpecOverwrite {
        /// 不可进化的 spec 名称
        name: String,
    },

    /// spec 校验失败 — register 时 validate() 返回错误
    ///
    /// WHY 包装而非转换: 保留原始 HarnessSpecError 供调用方匹配具体变体
    /// （如 ImmutableSurfaceViolation 用于安全审计）
    #[error("spec 校验失败: {0}")]
    ValidationFailed(#[from] HarnessSpecError),

    /// 内部不变式违反 — 注册表内部数据结构不一致
    ///
    /// WHY 独立变体: 这些场景理论上不可能发生（由 register 的不变式保证），
    /// 但为避免生产代码 panic，使用 Result 传播而非 .expect()。
    /// 若此错误触发，说明存在 bug（如 specs 表与 active 指针不一致）。
    #[error("内部不变式违反: {invariant}（name={name}, version={version}）")]
    InternalInvariant {
        /// 被违反的不变式描述
        invariant: String,
        /// 相关 spec 名称
        name: String,
        /// 相关版本号
        version: u32,
    },
}

// ============================================================
// SpecRegistry — 注册表主类型（P4-W15.2.1）
// ============================================================

/// HarnessSpec 版本化注册表 — 维护 spec 的版本谱系与 A/B 测试状态
///
/// # 设计
///
/// - **三级索引**:
///   - `specs: HashMap<name, HashMap<version, HarnessSpec>>` — 按 (name, version) 存储
///   - `active: HashMap<name, version>` — 当前生产版本
///   - `candidates: HashMap<name, version>` — A/B 测试候选版本
///
/// - **谱系复用**: 直接使用 `HarnessMeta.version` + `HarnessMeta.parent`
///   字段作为谱系链，SpecRegistry 不重新发明版本号机制
///
/// - **append-only 谱系**: 一旦 spec 注册，`specs` 表中条目不可修改
///   （仅 active/candidate 指针可变），保证审计可追溯
///
/// # 示例
///
/// ## 注册初始 spec 并查询
///
/// ```
/// use gsoe_evolution::spec_registry::{SpecRegistry, SpecRegistryError};
/// use nexus_contracts::HarnessSpec;
/// # use nexus_contracts::{HarnessMeta, RetryPolicy};
/// #
/// # fn make_spec(name: &str, version: u32, parent: Option<u32>) -> HarnessSpec {
/// #     HarnessSpec {
/// #         meta: HarnessMeta {
/// #             name: name.to_string(),
/// #             version,
/// #             immutable: false,
/// #             parent,
/// #             task_type: None,
/// #         },
/// #         contracts: vec![],
/// #         hops: vec![],
/// #         retry: RetryPolicy::default(),
/// #         auxiliary: Some(
/// #             "acceptance_gates = [\"tests_pass\", \"bench_no_regression\", \"invariants_clean\", \"redline_scan_clean\"]".to_string(),
/// #         ),
/// #     }
/// # }
///
/// let mut registry = SpecRegistry::new();
///
/// // 注册初始版本
/// let v1 = make_spec("quest-parse", 1, None);
/// registry.register(v1)?;
///
/// // 查询激活版本
/// let active = registry.get_active("quest-parse").expect("应有激活版本");
/// assert_eq!(active.meta.version, 1);
/// # Ok::<(), SpecRegistryError>(())
/// ```
pub struct SpecRegistry {
    /// 按 (name, version) 索引的 spec 存储（append-only）
    specs: HashMap<String, HashMap<u32, HarnessSpec>>,
    /// 按 name 索引的当前激活版本号
    active: HashMap<String, u32>,
    /// 按 name 索引的候选版本号（A/B 测试用）
    candidates: HashMap<String, u32>,
    /// P5.2.3: 可选的 EventBus 连接,用于在注册成功后发布 SpecRegistered 事件
    ///
    /// WHY Option<EventBus>:保持向后兼容(P4-W15.2.1 既有测试用 new() 创建
    /// 无 bus 的注册表)。需要发布事件的调用方使用 with_event_bus(bus)。
    event_bus: Option<EventBus>,
}

impl SpecRegistry {
    /// 创建空注册表（无 EventBus 连接）
    pub fn new() -> Self {
        Self {
            specs: HashMap::new(),
            active: HashMap::new(),
            candidates: HashMap::new(),
            event_bus: None,
        }
    }

    /// P5.2.3: 创建带 EventBus 连接的 SpecRegistry
    ///
    /// 注册成功后会通过 `publish_blocking` 发布 `SpecRegistered` 事件,
    /// 通知下游订阅者(parliament / efficiency-monitor / repo-wiki)。
    ///
    /// WHY 与 GsoeEvolutionEngine::with_event_bus 模式一致:
    /// 构造器 consume bus by value(EventBus 内部 Arc,克隆廉价),
    /// 调用方在 `with_event_bus` 之前无需 subscribe(因 register 是 sync 方法,
    /// publish_blocking 立即返回,订阅者可异步消费)。
    ///
    /// # 参数
    /// - `bus`: EventBus 连接(将被持有,用于后续 register 时发布事件)
    pub fn with_event_bus(bus: EventBus) -> Self {
        let mut registry = Self::new();
        registry.event_bus = Some(bus);
        registry
    }

    /// P5.2.3: 返回 EventBus 连接的引用(供测试与审计)
    pub fn event_bus(&self) -> Option<&EventBus> {
        self.event_bus.as_ref()
    }

    /// 注册新 spec 版本(默认 source="spec-registry")
    ///
    /// P5.2.3:委托给 `register_with_source(spec, "spec-registry")`,
    /// 保持 P4-W15.2.1 既有调用方向后兼容(无需指定 source)。
    ///
    /// # 参数
    /// - `spec`: 待注册的 HarnessSpec(必须通过 validate)
    ///
    /// # 返回
    /// - `Ok(u32)`: 注册成功,返回版本号
    /// - `Err(SpecRegistryError)`: 注册失败
    pub fn register(&mut self, spec: HarnessSpec) -> Result<u32, SpecRegistryError> {
        self.register_with_source(spec, "spec-registry")
    }

    /// P5.2.3: 注册新 spec 版本(显式指定 source)
    ///
    /// source 字段用于在 `SpecRegistered` 事件中标注注册来源,下游订阅者
    /// 可据此区分不同注册路径:
    /// - `"spec-registry"`:默认(由 `register` 调用)
    /// - `"rhi-cg-channel-b"`:通道 B 否决通过后注册(本任务 P5.2.3 主路径)
    /// - `"manual"`:手动注册(如运维人员介入)
    /// - `"ab-test"`:A/B 测试场景
    ///
    /// # 流程
    /// 1. 调用 `spec.validate()` 确保不可进化面合规
    /// 2. 检查 (name, version) 是否已存在 → VersionConflict
    /// 3. 检查同名旧版本 immutable 标记 → ImmutableSpecOverwrite
    /// 4. 检查 parent 链存在性 → ParentMissing
    /// 5. 插入到 `specs[name][version]`
    /// 6. 若 parent=None(初始版本),设置 active[name] = version
    /// 7. **P5.2.3 新增**:若已连接 EventBus,发布 SpecRegistered 事件
    ///
    /// # 不可进化面守护(P4-W15.2.3 + P5.2.3 强化)
    ///
    /// 三层守护:
    /// 1. **compile-time**:`HarnessSpec::validate()` 的 `ImmutableSurfaceViolation`
    /// 2. **runtime-register**:同名旧版本 `immutable=true` 时 `ImmutableSpecOverwrite`
    /// 3. **runtime-channel-B**(P5.2.3 强化):`into_gsoe_error` 将上述错误映射为
    ///    `GsoeError::ImmutableSurfaceViolated`,供通道 B 否决路径统一处理
    ///
    /// # 参数
    /// - `spec`: 待注册的 HarnessSpec(必须通过 validate)
    /// - `source`: 注册来源标识(写入 SpecRegistered.source 字段)
    ///
    /// # 返回
    /// - `Ok(u32)`: 注册成功,返回版本号
    /// - `Err(SpecRegistryError)`: 注册失败(校验/冲突/parent 缺失/不可进化面覆盖)
    ///
    /// # 防注入保证
    /// - 仅接受已 validate 的 HarnessSpec,不解析 TOML
    /// - 不修改 spec 内容(append-only 插入)
    /// - source 字段长度受 String 限制,无注入风险
    pub fn register_with_source(
        &mut self,
        spec: HarnessSpec,
        source: &str,
    ) -> Result<u32, SpecRegistryError> {
        // 1. 调用 validate() 确保不可进化面合规(P4-W15.2.3 不可进化面硬编码)
        spec.validate()?;

        // 2. 提取字段信息(在 move 之前完成,供后续事件发布使用)
        let name = spec.meta.name.clone();
        let version = spec.meta.version;
        let parent = spec.meta.parent;

        // 3. 检查 (name, version) 是否已存在
        if self
            .specs
            .get(&name)
            .is_some_and(|v| v.contains_key(&version))
        {
            return Err(SpecRegistryError::VersionConflict { name, version });
        }

        // 4. 检查同名旧版本的 immutable 标记
        // WHY: 若任何已注册版本 immutable=true,禁止注册新版本
        //      这是不可进化面的运行时守护
        if let Some(versions) = self.specs.get(&name) {
            for existing in versions.values() {
                if existing.meta.immutable {
                    return Err(SpecRegistryError::ImmutableSpecOverwrite { name });
                }
            }
        }

        // 5. 检查 parent 链存在性(若 parent=Some(p))
        if let Some(parent_version) = parent {
            let parent_exists = self
                .specs
                .get(&name)
                .is_some_and(|v| v.contains_key(&parent_version));
            if !parent_exists {
                return Err(SpecRegistryError::ParentMissing {
                    name,
                    parent: parent_version,
                });
            }
        }

        // 6. 插入到 specs[name][version]
        self.specs
            .entry(name.clone())
            .or_default()
            .insert(version, spec);

        // 7. 若 parent=None(初始版本),设为 active
        if parent.is_none() {
            self.active.insert(name.clone(), version);
        }

        // 8. P5.2.3: 发布 SpecRegistered 事件(若已连接 EventBus)
        //
        // WHY publish_blocking 而非 publish: register_with_source 是 sync 方法,
        //     无法 await(§4.4 红线 8:sync 方法用 publish_blocking,async 用 publish().await)
        // WHY 失败仅 warn: 注册本身是 source of truth(已写入 specs 表),
        //     事件丢失仅导致下游未通知,可由下次注册或主动查询 SpecRegistry 补偿。
        //     与 GsoeEvolutionEngine::publish_evolution_event 模式一致。
        // WHY 在所有校验通过后才发布:确保事件语义 truthful(注册确实成功),
        //     避免发布"虚假成功"事件误导下游
        if let Some(bus) = &self.event_bus {
            // WHY name.clone():event 构造 move name,但失败日志仍需引用 name,
            //     故先 clone 一份供日志使用(避免 move 后借用编译错误)
            let name_for_log = name.clone();
            let event = NexusEvent::SpecRegistered {
                metadata: EventMetadata::new("gsoe-evolution"),
                spec_name: name,
                spec_version: version,
                parent_version: parent,
                source: source.to_string(),
            };
            if let Err(e) = bus.publish_blocking(event) {
                tracing::warn!(
                    error = %e,
                    spec_name = %name_for_log,
                    spec_version = version,
                    "发布 SpecRegistered 事件失败"
                );
            }
        }

        Ok(version)
    }

    /// 按 (name, version) 获取 spec
    ///
    /// # 返回
    /// - `Some(&HarnessSpec)`: 找到对应版本
    /// - `None`: name 或 version 不存在
    pub fn get(&self, name: &str, version: u32) -> Option<&HarnessSpec> {
        self.specs.get(name)?.get(&version)
    }

    /// 获取当前激活版本
    ///
    /// # 返回
    /// - `Some(&HarnessSpec)`: 找到 active 版本
    /// - `None`: name 未注册或无 active
    pub fn get_active(&self, name: &str) -> Option<&HarnessSpec> {
        let version = self.active.get(name)?;
        self.get(name, *version)
    }

    /// 获取候选版本（A/B 测试）
    ///
    /// # 返回
    /// - `Some(&HarnessSpec)`: 找到 candidate 版本
    /// - `None`: name 未设置 candidate
    pub fn get_candidate(&self, name: &str) -> Option<&HarnessSpec> {
        let version = self.candidates.get(name)?;
        self.get(name, *version)
    }

    /// 设置候选版本（A/B 测试入口）
    ///
    /// # 参数
    /// - `name`: spec 名称
    /// - `version`: 候选版本号（必须已注册）
    ///
    /// # 返回
    /// - `Ok(())`: 设置成功
    /// - `Err(SpecNotFound)`: name/version 未注册
    pub fn set_candidate(&mut self, name: &str, version: u32) -> Result<(), SpecRegistryError> {
        if self.get(name, version).is_none() {
            return Err(SpecRegistryError::SpecNotFound {
                name: name.to_string(),
                version,
            });
        }
        self.candidates.insert(name.to_string(), version);
        Ok(())
    }

    /// 将候选版本提升为激活版本
    ///
    /// # 流程
    /// 1. 检查 candidate 是否存在 → NoCandidate
    /// 2. 将 candidate 设为 active
    /// 3. 清除 candidate 指针
    ///
    /// # 返回
    /// - `Ok(u32)`: 新的 active 版本号
    /// - `Err(NoCandidate)`: name 未设置 candidate
    pub fn promote_candidate(&mut self, name: &str) -> Result<u32, SpecRegistryError> {
        let candidate_version =
            self.candidates
                .get(name)
                .copied()
                .ok_or_else(|| SpecRegistryError::NoCandidate {
                    name: name.to_string(),
                })?;

        // 提升为 active
        self.active.insert(name.to_string(), candidate_version);
        // 清除 candidate 指针
        self.candidates.remove(name);

        Ok(candidate_version)
    }

    /// 一键回滚到父版本
    ///
    /// # 流程
    /// 1. 获取当前 active 版本
    /// 2. 读取 active 版本的 meta.parent
    /// 3. 若 parent=None → NoParentToRollback
    /// 4. 将 active 设为 parent 版本
    ///
    /// # 返回
    /// - `Ok(u32)`: 回滚后的版本号（原 active 的 parent）
    /// - `Err(NoActiveVersion)`: name 无 active
    /// - `Err(NoParentToRollback)`: active 已是初始版本
    pub fn rollback(&mut self, name: &str) -> Result<u32, SpecRegistryError> {
        let active_version =
            self.active
                .get(name)
                .copied()
                .ok_or_else(|| SpecRegistryError::NoActiveVersion {
                    name: name.to_string(),
                })?;

        // WHY ok_or_else + ?: active_version 来自 self.active 表，而 register 保证
        // 写入 active 前已将 spec 插入 specs 表（不变式保证）。但为避免 panic，
        // 使用 InternalInvariant 传播错误，若触发则说明存在 bug。
        let active_spec =
            self.get(name, active_version)
                .ok_or_else(|| SpecRegistryError::InternalInvariant {
                    invariant: "active 版本必须存在于 specs 表".to_string(),
                    name: name.to_string(),
                    version: active_version,
                })?;

        let parent_version =
            active_spec
                .meta
                .parent
                .ok_or_else(|| SpecRegistryError::NoParentToRollback {
                    name: name.to_string(),
                    current: active_version,
                })?;

        // 设置 active 为 parent 版本
        self.active.insert(name.to_string(), parent_version);

        Ok(parent_version)
    }

    /// 列出 name 的所有已注册版本（升序）
    ///
    /// # 返回
    /// - `Vec<u32>`: 版本号列表（升序）；空 Vec 表示 name 未注册
    pub fn list_versions(&self, name: &str) -> Vec<u32> {
        match self.specs.get(name) {
            Some(versions) => {
                let mut v: Vec<u32> = versions.keys().copied().collect();
                v.sort_unstable();
                v
            }
            None => Vec::new(),
        }
    }

    /// 返回从初始版本到当前 active 的完整版本链
    ///
    /// # 流程
    /// 1. 从 active 开始
    /// 2. 沿 parent 链向前追溯
    /// 3. 反转链表（初始→当前）
    ///
    /// # 返回
    /// - `Ok(Vec<u32>)`: 版本链（初始版本在前，active 在末尾）
    /// - `Err(NoActiveVersion)`: name 无 active
    pub fn lineage(&self, name: &str) -> Result<Vec<u32>, SpecRegistryError> {
        let mut chain: Vec<u32> = Vec::new();
        let mut current =
            self.active
                .get(name)
                .copied()
                .ok_or_else(|| SpecRegistryError::NoActiveVersion {
                    name: name.to_string(),
                })?;

        // 从 active 向前追溯 parent 链
        loop {
            chain.push(current);
            // WHY ok_or_else + ?: current 来自 specs 表中已注册的版本号
            // （初始来自 active，后续来自 parent 字段），register 的不变式保证
            // 这些版本一定存在于 specs 表。但为避免 panic，使用 InternalInvariant
            // 传播错误，若触发则说明数据结构不一致（bug）。
            let spec =
                self.get(name, current)
                    .ok_or_else(|| SpecRegistryError::InternalInvariant {
                        invariant: "lineage 追溯的版本必须存在于 specs 表".to_string(),
                        name: name.to_string(),
                        version: current,
                    })?;
            match spec.meta.parent {
                Some(parent) => current = parent,
                None => break, // 到达初始版本
            }
        }

        // 反转：初始在前，active 在末尾
        chain.reverse();
        Ok(chain)
    }

    /// 列出所有已注册的 spec 名称
    ///
    /// # 返回
    /// - `Vec<String>`: 名称列表（无序，可按需排序）
    pub fn list_names(&self) -> Vec<String> {
        self.specs.keys().cloned().collect()
    }

    /// 返回不可进化面清单（透传 L0 API，便于调用方安全审计）
    ///
    /// WHY 提供: 调用方（如 GsoeEvolutionEngine）可能需要枚举不可进化面
    /// 做安全审计。此方法透传 `HarnessSpec::immutable_surfaces()`
    pub fn immutable_surfaces() -> &'static [ImmutableSurface; 20] {
        HarnessSpec::immutable_surfaces()
    }

    /// 将 SpecRegistryError 转换为 GsoeError(P5.2.3 强化:不可进化面违反映射)
    ///
    /// P5.2.3 增强:三层不可进化面守护的第三层(runtime-channel-B),
    /// 将以下 SpecRegistryError 变体映射为 `GsoeError::ImmutableSurfaceViolated`,
    /// 供通道 B 否决路径统一处理:
    /// - `ImmutableSpecOverwrite`:同名旧版本 immutable=true
    /// - `ValidationFailed(ImmutableSurfaceViolation)`:spec 触碰不可进化面资源
    /// - `ValidationFailed(ImmutableMetaNotMarked)`:不可进化面 spec 未标记 immutable=true
    ///
    /// 其他 SpecRegistryError(VersionConflict / ParentMissing / SpecNotFound 等)
    /// 仍归类为 `GsoeError::ConfigError`,这些与不可进化面无关。
    pub fn into_gsoe_error(err: SpecRegistryError) -> GsoeError {
        match err {
            SpecRegistryError::ImmutableSpecOverwrite { name } => {
                GsoeError::ImmutableSurfaceViolated {
                    reason: format!("spec '{}' 同名旧版本 immutable=true,不允许注册新版本", name),
                }
            }
            SpecRegistryError::ValidationFailed(HarnessSpecError::ImmutableSurfaceViolation {
                location,
                surface,
            }) => GsoeError::ImmutableSurfaceViolated {
                reason: format!(
                    "spec 触碰不可进化面: location={}, surface={}",
                    location,
                    surface.as_str()
                ),
            },
            SpecRegistryError::ValidationFailed(HarnessSpecError::ImmutableMetaNotMarked {
                name,
            }) => GsoeError::ImmutableSurfaceViolated {
                reason: format!("spec '{}' 在不可进化面清单中但未标记 immutable=true", name),
            },
            other => GsoeError::ConfigError {
                reason: other.to_string(),
            },
        }
    }

    /// 返回 name 的已注册版本数
    ///
    /// # 返回
    /// - `usize`: 版本数；0 表示 name 未注册
    pub fn version_count(&self, name: &str) -> usize {
        self.specs.get(name).map(|v| v.len()).unwrap_or(0)
    }

    /// 返回注册表中的 spec 总数（所有 name 的所有 version 之和）
    pub fn total_specs(&self) -> usize {
        self.specs.values().map(|v| v.len()).sum()
    }
}

impl Default for SpecRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 单元测试（P4-W15.2.1）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::{EventBus, NexusEvent};
    use nexus_contracts::{ContractSpec, HarnessMeta, HopSpec, RetryPolicy};

    // ============================================================
    // 测试辅助函数
    // ============================================================

    /// 构造一个最小合法 spec（用于测试）
    ///
    /// WHY 手动构造而非 SpecLoader::load_from_str: 单元测试需精确控制
    /// meta.version / parent 字段以测试谱系逻辑，SpecLoader 需 TOML 字符串
    /// 较繁琐。直接构造 HarnessSpec 更清晰
    fn make_spec(name: &str, version: u32, parent: Option<u32>) -> HarnessSpec {
        HarnessSpec {
            meta: HarnessMeta {
                name: name.to_string(),
                version,
                immutable: false,
                parent,
                task_type: None,
            },
            contracts: vec![ContractSpec {
                name: "no_panic".to_string(),
                property: "fuzz_target_must_not_panic".to_string(),
                description: None,
                from: None,
                to: None,
                fields: vec![],
            }],
            hops: vec![HopSpec {
                name: "generate_input".to_string(),
                input_type: None,
                output_type: None,
                contracts: vec!["no_panic".to_string()],
                description: None,
                order: vec!["Architect.propose".to_string()],
                on_veto: None,
                fallback: None,
            }],
            retry: RetryPolicy::default(),
            auxiliary: Some(
                "acceptance_gates = [\"tests_pass\", \"bench_no_regression\", \"invariants_clean\", \"redline_scan_clean\"]"
                    .to_string(),
            ),
        }
    }

    /// 构造一个不可进化 spec（meta.immutable = true）
    fn make_immutable_spec(name: &str, version: u32) -> HarnessSpec {
        let mut spec = make_spec(name, version, None);
        spec.meta.immutable = true;
        spec
    }

    // ============================================================
    // register 成功路径测试
    // ============================================================

    #[test]
    fn test_register_initial_version_succeeds() {
        let mut registry = SpecRegistry::new();
        let spec = make_spec("quest-parse", 1, None);
        let version = registry.register(spec).expect("初始版本应注册成功");
        assert_eq!(version, 1);
    }

    #[test]
    fn test_register_initial_version_sets_active() {
        let mut registry = SpecRegistry::new();
        let spec = make_spec("quest-parse", 1, None);
        registry.register(spec).unwrap();

        // 初始版本应自动设为 active
        let active = registry.get_active("quest-parse");
        assert!(active.is_some());
        assert_eq!(active.unwrap().meta.version, 1);
    }

    #[test]
    fn test_register_child_version_does_not_change_active() {
        let mut registry = SpecRegistry::new();
        // 注册 v1（初始版本，设为 active）
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        // 注册 v2（parent=1，不应改变 active）
        registry
            .register(make_spec("quest-parse", 2, Some(1)))
            .unwrap();

        // active 仍应为 1
        let active = registry.get_active("quest-parse").unwrap();
        assert_eq!(active.meta.version, 1);
    }

    #[test]
    fn test_register_multiple_specs_independent() {
        let mut registry = SpecRegistry::new();
        registry.register(make_spec("spec-a", 1, None)).unwrap();
        registry.register(make_spec("spec-b", 1, None)).unwrap();

        assert_eq!(registry.total_specs(), 2);
        assert_eq!(registry.list_names().len(), 2);
    }

    // ============================================================
    // register 错误路径测试
    // ============================================================

    #[test]
    fn test_register_rejects_version_conflict() {
        let mut registry = SpecRegistry::new();
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();

        // 重复注册 (name, version) 应失败
        let result = registry.register(make_spec("quest-parse", 1, None));
        match result {
            Err(SpecRegistryError::VersionConflict { name, version }) => {
                assert_eq!(name, "quest-parse");
                assert_eq!(version, 1);
            }
            other => panic!("期望 VersionConflict，实际: {:?}", other),
        }
    }

    #[test]
    fn test_register_rejects_missing_parent() {
        let mut registry = SpecRegistry::new();
        // parent=5 但 v5 未注册
        let result = registry.register(make_spec("quest-parse", 6, Some(5)));
        match result {
            Err(SpecRegistryError::ParentMissing { name, parent }) => {
                assert_eq!(name, "quest-parse");
                assert_eq!(parent, 5);
            }
            other => panic!("期望 ParentMissing，实际: {:?}", other),
        }
    }

    #[test]
    fn test_register_rejects_immutable_overwrite() {
        let mut registry = SpecRegistry::new();
        // 注册不可进化 v1
        registry
            .register(make_immutable_spec("quest-parse", 1))
            .unwrap();

        // 尝试注册 v2 应被拒绝（旧版本 immutable=true）
        let result = registry.register(make_spec("quest-parse", 2, Some(1)));
        match result {
            Err(SpecRegistryError::ImmutableSpecOverwrite { name }) => {
                assert_eq!(name, "quest-parse");
            }
            other => panic!("期望 ImmutableSpecOverwrite，实际: {:?}", other),
        }
    }

    #[test]
    fn test_register_rejects_validation_failure() {
        let mut registry = SpecRegistry::new();
        // version=0 违反 validate() 的 InvalidVersion 规则
        let mut bad_spec = make_spec("quest-parse", 0, None);
        // 重新设置 meta.version
        bad_spec.meta.version = 0;

        let result = registry.register(bad_spec);
        match result {
            Err(SpecRegistryError::ValidationFailed(HarnessSpecError::InvalidVersion)) => {}
            other => panic!("期望 ValidationFailed(InvalidVersion)，实际: {:?}", other),
        }
    }

    #[test]
    fn test_register_rejects_empty_meta_name() {
        let mut registry = SpecRegistry::new();
        let mut bad_spec = make_spec("", 1, None);
        bad_spec.meta.name = String::new();

        let result = registry.register(bad_spec);
        match result {
            Err(SpecRegistryError::ValidationFailed(HarnessSpecError::EmptyMetaName)) => {}
            other => panic!("期望 ValidationFailed(EmptyMetaName)，实际: {:?}", other),
        }
    }

    // ============================================================
    // get / get_active 查询测试
    // ============================================================

    #[test]
    fn test_get_returns_spec_by_name_and_version() {
        let mut registry = SpecRegistry::new();
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        registry
            .register(make_spec("quest-parse", 2, Some(1)))
            .unwrap();

        let v1 = registry.get("quest-parse", 1).unwrap();
        let v2 = registry.get("quest-parse", 2).unwrap();
        assert_eq!(v1.meta.version, 1);
        assert_eq!(v2.meta.version, 2);
    }

    #[test]
    fn test_get_returns_none_for_unknown_name() {
        let registry = SpecRegistry::new();
        assert!(registry.get("unknown", 1).is_none());
    }

    #[test]
    fn test_get_returns_none_for_unknown_version() {
        let mut registry = SpecRegistry::new();
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        assert!(registry.get("quest-parse", 99).is_none());
    }

    #[test]
    fn test_get_active_returns_none_for_unregistered() {
        let registry = SpecRegistry::new();
        assert!(registry.get_active("unregistered").is_none());
    }

    // ============================================================
    // A/B 测试: set_candidate / promote_candidate
    // ============================================================

    #[test]
    fn test_set_candidate_succeeds_for_registered_spec() {
        let mut registry = SpecRegistry::new();
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        registry
            .register(make_spec("quest-parse", 2, Some(1)))
            .unwrap();

        let result = registry.set_candidate("quest-parse", 2);
        assert!(result.is_ok());
        assert_eq!(
            registry.get_candidate("quest-parse").unwrap().meta.version,
            2
        );
    }

    #[test]
    fn test_set_candidate_rejects_unregistered_version() {
        let mut registry = SpecRegistry::new();
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();

        let result = registry.set_candidate("quest-parse", 99);
        match result {
            Err(SpecRegistryError::SpecNotFound { name, version }) => {
                assert_eq!(name, "quest-parse");
                assert_eq!(version, 99);
            }
            other => panic!("期望 SpecNotFound，实际: {:?}", other),
        }
    }

    #[test]
    fn test_promote_candidate_succeeds() {
        let mut registry = SpecRegistry::new();
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        registry
            .register(make_spec("quest-parse", 2, Some(1)))
            .unwrap();
        registry.set_candidate("quest-parse", 2).unwrap();

        let new_active = registry.promote_candidate("quest-parse").unwrap();
        assert_eq!(new_active, 2);

        // active 应更新为 2
        assert_eq!(registry.get_active("quest-parse").unwrap().meta.version, 2);
        // candidate 应被清除
        assert!(registry.get_candidate("quest-parse").is_none());
    }

    #[test]
    fn test_promote_candidate_rejects_when_no_candidate() {
        let mut registry = SpecRegistry::new();
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();

        let result = registry.promote_candidate("quest-parse");
        match result {
            Err(SpecRegistryError::NoCandidate { name }) => {
                assert_eq!(name, "quest-parse");
            }
            other => panic!("期望 NoCandidate，实际: {:?}", other),
        }
    }

    #[test]
    fn test_ab_test_full_lifecycle() {
        let mut registry = SpecRegistry::new();
        // 1. 注册 v1（自动设为 active）
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        // 2. 注册 v2（parent=1）
        registry
            .register(make_spec("quest-parse", 2, Some(1)))
            .unwrap();
        // 3. 设置 v2 为 candidate
        registry.set_candidate("quest-parse", 2).unwrap();
        // 4. active=1, candidate=2
        assert_eq!(registry.get_active("quest-parse").unwrap().meta.version, 1);
        assert_eq!(
            registry.get_candidate("quest-parse").unwrap().meta.version,
            2
        );
        // 5. promote candidate → active=2
        registry.promote_candidate("quest-parse").unwrap();
        assert_eq!(registry.get_active("quest-parse").unwrap().meta.version, 2);
        // 6. candidate 已清除
        assert!(registry.get_candidate("quest-parse").is_none());
    }

    // ============================================================
    // rollback 测试
    // ============================================================

    #[test]
    fn test_rollback_succeeds_to_parent() {
        let mut registry = SpecRegistry::new();
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        registry
            .register(make_spec("quest-parse", 2, Some(1)))
            .unwrap();
        registry.set_candidate("quest-parse", 2).unwrap();
        registry.promote_candidate("quest-parse").unwrap();
        // active = 2

        let rolled_back = registry.rollback("quest-parse").unwrap();
        assert_eq!(rolled_back, 1);
        assert_eq!(registry.get_active("quest-parse").unwrap().meta.version, 1);
    }

    #[test]
    fn test_rollback_rejects_when_no_active() {
        let mut registry = SpecRegistry::new();
        let result = registry.rollback("unregistered");
        match result {
            Err(SpecRegistryError::NoActiveVersion { name }) => {
                assert_eq!(name, "unregistered");
            }
            other => panic!("期望 NoActiveVersion，实际: {:?}", other),
        }
    }

    #[test]
    fn test_rollback_rejects_initial_version() {
        let mut registry = SpecRegistry::new();
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        // active=1, parent=None → 无法回滚

        let result = registry.rollback("quest-parse");
        match result {
            Err(SpecRegistryError::NoParentToRollback { name, current }) => {
                assert_eq!(name, "quest-parse");
                assert_eq!(current, 1);
            }
            other => panic!("期望 NoParentToRollback，实际: {:?}", other),
        }
    }

    #[test]
    fn test_rollback_chain_three_versions() {
        let mut registry = SpecRegistry::new();
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        registry
            .register(make_spec("quest-parse", 2, Some(1)))
            .unwrap();
        registry
            .register(make_spec("quest-parse", 3, Some(2)))
            .unwrap();

        // active=1, 设置 v3 为 candidate
        registry.set_candidate("quest-parse", 3).unwrap();
        registry.promote_candidate("quest-parse").unwrap();
        assert_eq!(registry.get_active("quest-parse").unwrap().meta.version, 3);

        // 回滚一次 → active=2
        let r1 = registry.rollback("quest-parse").unwrap();
        assert_eq!(r1, 2);

        // 再次回滚 → active=1
        let r2 = registry.rollback("quest-parse").unwrap();
        assert_eq!(r2, 1);

        // 第三次回滚应失败（已是初始版本）
        let result = registry.rollback("quest-parse");
        assert!(matches!(
            result,
            Err(SpecRegistryError::NoParentToRollback { .. })
        ));
    }

    // ============================================================
    // list_versions / lineage / list_names 测试
    // ============================================================

    #[test]
    fn test_list_versions_returns_sorted() {
        let mut registry = SpecRegistry::new();
        registry
            .register(make_spec("quest-parse", 3, None))
            .unwrap();
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        // 注意：version=1 与 version=3 是独立的初始版本（无 parent 关系）

        let versions = registry.list_versions("quest-parse");
        assert_eq!(versions, vec![1, 3]); // 升序
    }

    #[test]
    fn test_list_versions_empty_for_unregistered() {
        let registry = SpecRegistry::new();
        assert!(registry.list_versions("unknown").is_empty());
    }

    #[test]
    fn test_lineage_returns_full_chain() {
        let mut registry = SpecRegistry::new();
        // v1 → v2 → v3 → v4
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        registry
            .register(make_spec("quest-parse", 2, Some(1)))
            .unwrap();
        registry
            .register(make_spec("quest-parse", 3, Some(2)))
            .unwrap();
        registry
            .register(make_spec("quest-parse", 4, Some(3)))
            .unwrap();

        // 设置 v4 为 active
        registry.set_candidate("quest-parse", 4).unwrap();
        registry.promote_candidate("quest-parse").unwrap();

        let lineage = registry.lineage("quest-parse").unwrap();
        assert_eq!(lineage, vec![1, 2, 3, 4]); // 从初始到当前
    }

    #[test]
    fn test_lineage_single_version() {
        let mut registry = SpecRegistry::new();
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();

        let lineage = registry.lineage("quest-parse").unwrap();
        assert_eq!(lineage, vec![1]);
    }

    #[test]
    fn test_lineage_rejects_unregistered() {
        let registry = SpecRegistry::new();
        let result = registry.lineage("unknown");
        match result {
            Err(SpecRegistryError::NoActiveVersion { name }) => {
                assert_eq!(name, "unknown");
            }
            other => panic!("期望 NoActiveVersion，实际: {:?}", other),
        }
    }

    #[test]
    fn test_list_names_returns_all_registered() {
        let mut registry = SpecRegistry::new();
        registry.register(make_spec("spec-a", 1, None)).unwrap();
        registry.register(make_spec("spec-b", 1, None)).unwrap();
        registry.register(make_spec("spec-c", 1, None)).unwrap();

        let mut names = registry.list_names();
        names.sort();
        assert_eq!(names, vec!["spec-a", "spec-b", "spec-c"]);
    }

    // ============================================================
    // version_count / total_specs 测试
    // ============================================================

    #[test]
    fn test_version_count_zero_for_unregistered() {
        let registry = SpecRegistry::new();
        assert_eq!(registry.version_count("unknown"), 0);
    }

    #[test]
    fn test_version_count_returns_count() {
        let mut registry = SpecRegistry::new();
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        registry
            .register(make_spec("quest-parse", 2, Some(1)))
            .unwrap();
        registry
            .register(make_spec("quest-parse", 3, Some(2)))
            .unwrap();

        assert_eq!(registry.version_count("quest-parse"), 3);
    }

    #[test]
    fn test_total_specs_aggregates_across_names() {
        let mut registry = SpecRegistry::new();
        registry.register(make_spec("spec-a", 1, None)).unwrap();
        registry.register(make_spec("spec-a", 2, Some(1))).unwrap();
        registry.register(make_spec("spec-b", 1, None)).unwrap();

        assert_eq!(registry.total_specs(), 3);
    }

    // ============================================================
    // immutable_surfaces 透传测试
    // ============================================================

    #[test]
    fn test_immutable_surfaces_returns_20_variants() {
        let surfaces = SpecRegistry::immutable_surfaces();
        assert_eq!(surfaces.len(), 20);
    }

    // ============================================================
    // into_gsoe_error 转换测试
    // ============================================================

    #[test]
    fn test_into_gsoe_error_preserves_message() {
        let err = SpecRegistryError::NoCandidate {
            name: "test".to_string(),
        };
        let gsoe_err = SpecRegistry::into_gsoe_error(err);
        let msg = gsoe_err.to_string();
        assert!(msg.contains("test"));
        assert!(msg.contains("候选"));
    }

    // ============================================================
    // Default trait 测试
    // ============================================================

    #[test]
    fn test_default_creates_empty_registry() {
        let registry = SpecRegistry::default();
        assert_eq!(registry.total_specs(), 0);
        assert!(registry.list_names().is_empty());
    }

    // ============================================================
    // 复杂场景测试
    // ============================================================

    #[test]
    fn test_complex_lineage_with_branches() {
        // 测试分叉谱系：v1 → v2 → v3a / v3b
        // SpecRegistry 允许同一 parent 有多个子版本（A/B 测试场景）
        let mut registry = SpecRegistry::new();
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        registry
            .register(make_spec("quest-parse", 2, Some(1)))
            .unwrap();
        // v3a 与 v3b 都以 v2 为 parent
        registry
            .register(make_spec("quest-parse", 3, Some(2)))
            .unwrap();
        // 第二个 v3 不可能（version=3 已用），改用 v4 同样以 v2 为 parent
        registry
            .register(make_spec("quest-parse", 4, Some(2)))
            .unwrap();

        // 设置 v4 为 candidate，promote 为 active
        registry.set_candidate("quest-parse", 4).unwrap();
        registry.promote_candidate("quest-parse").unwrap();

        // lineage 应为 1 → 2 → 4（不含 v3，因 v3 不在 active 链上）
        let lineage = registry.lineage("quest-parse").unwrap();
        assert_eq!(lineage, vec![1, 2, 4]);
    }

    #[test]
    fn test_register_independent_specs_with_same_version() {
        // 不同 name 可以有相同的 version 号（version 在 name 内唯一）
        let mut registry = SpecRegistry::new();
        registry.register(make_spec("spec-a", 1, None)).unwrap();
        registry.register(make_spec("spec-b", 1, None)).unwrap();
        registry.register(make_spec("spec-c", 1, None)).unwrap();

        assert_eq!(registry.total_specs(), 3);
        assert_eq!(registry.version_count("spec-a"), 1);
        assert_eq!(registry.version_count("spec-b"), 1);
    }

    #[test]
    fn test_promote_candidate_clears_candidate_pointer() {
        let mut registry = SpecRegistry::new();
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        registry
            .register(make_spec("quest-parse", 2, Some(1)))
            .unwrap();
        registry.set_candidate("quest-parse", 2).unwrap();
        assert!(registry.get_candidate("quest-parse").is_some());

        registry.promote_candidate("quest-parse").unwrap();
        // candidate 应被清除
        assert!(registry.get_candidate("quest-parse").is_none());
    }

    // ============================================================
    // 不可进化面守护测试（P4-W15.2.3）
    // ============================================================

    #[test]
    fn test_immutable_spec_blocks_all_future_versions() {
        let mut registry = SpecRegistry::new();
        // 注册不可进化 v1
        registry
            .register(make_immutable_spec("critical-rule", 1))
            .unwrap();

        // 任何后续版本都应被拒绝（即使 parent 链合法）
        let result = registry.register(make_spec("critical-rule", 2, Some(1)));
        assert!(matches!(
            result,
            Err(SpecRegistryError::ImmutableSpecOverwrite { .. })
        ));
    }

    #[test]
    fn test_non_immutable_allows_multiple_versions() {
        let mut registry = SpecRegistry::new();
        // 普通 spec 允许多版本
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        registry
            .register(make_spec("quest-parse", 2, Some(1)))
            .unwrap();
        registry
            .register(make_spec("quest-parse", 3, Some(2)))
            .unwrap();

        assert_eq!(registry.version_count("quest-parse"), 3);
    }

    #[test]
    fn test_immutable_spec_still_readable() {
        let mut registry = SpecRegistry::new();
        registry
            .register(make_immutable_spec("critical-rule", 1))
            .unwrap();

        // 不可进化 spec 仍可查询
        let spec = registry.get("critical-rule", 1).unwrap();
        assert!(spec.meta.immutable);
    }

    // ============================================================
    // 防注入保证测试（P4-W15.2.3）
    // ============================================================

    /// 验证 &self 不变性: 查询方法均为 &self
    #[test]
    fn test_query_methods_take_immutable_ref() {
        let mut registry = SpecRegistry::new();
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();

        // 取不可变引用
        let registry_ref: &SpecRegistry = &registry;

        // 在 &SpecRegistry 上调用所有查询方法（编译期保证 &self 签名）
        let _ = registry_ref.get("quest-parse", 1);
        let _ = registry_ref.get_active("quest-parse");
        let _ = registry_ref.get_candidate("quest-parse");
        let _ = registry_ref.list_versions("quest-parse");
        let _ = registry_ref.lineage("quest-parse");
        let _ = registry_ref.list_names();
        let _ = registry_ref.version_count("quest-parse");
        let _ = registry_ref.total_specs();
        let _ = SpecRegistry::immutable_surfaces();
    }

    /// 验证 register 不修改已存在的 spec（append-only）
    #[test]
    fn test_register_does_not_modify_existing_specs() {
        let mut registry = SpecRegistry::new();
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        registry
            .register(make_spec("quest-parse", 2, Some(1)))
            .unwrap();

        // v1 的内容不应被 v2 注册影响
        let v1 = registry.get("quest-parse", 1).unwrap();
        assert_eq!(v1.meta.version, 1);
        assert_eq!(v1.meta.parent, None);
    }

    /// 验证 rollback 不删除版本（仅修改 active 指针）
    #[test]
    fn test_rollback_does_not_delete_versions() {
        let mut registry = SpecRegistry::new();
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        registry
            .register(make_spec("quest-parse", 2, Some(1)))
            .unwrap();
        registry.set_candidate("quest-parse", 2).unwrap();
        registry.promote_candidate("quest-parse").unwrap();
        // active=2

        registry.rollback("quest-parse").unwrap();
        // active=1，但 v2 仍应存在
        assert!(registry.get("quest-parse", 2).is_some());
        assert_eq!(registry.version_count("quest-parse"), 2);
    }

    // ============================================================
    // P5.2.3: EventBus 集成 + SpecRegistered 事件测试
    // ============================================================

    #[test]
    fn test_new_creates_registry_without_event_bus() {
        // P5.2.3: 验证 new() 不连接 EventBus(向后兼容)
        let registry = SpecRegistry::new();
        assert!(registry.event_bus().is_none());
    }

    #[test]
    fn test_with_event_bus_creates_registry_with_bus() {
        // P5.2.3: 验证 with_event_bus() 连接 EventBus
        let bus = EventBus::new();
        let registry = SpecRegistry::with_event_bus(bus);
        assert!(registry.event_bus().is_some());
    }

    #[test]
    fn test_register_without_event_bus_still_works() {
        // P5.2.3: 向后兼容性验证 — 无 EventBus 时 register 仍正常工作
        let mut registry = SpecRegistry::new();
        let spec = make_spec("quest-parse", 1, None);
        let version = registry.register(spec).expect("无 EventBus 时应正常注册");
        assert_eq!(version, 1);
        assert_eq!(registry.total_specs(), 1);
    }

    #[test]
    fn test_register_with_event_bus_publishes_spec_registered_event() {
        // P5.2.3: 验证 register 在有 EventBus 时发布 SpecRegistered 事件
        //
        // §4.4 红线 3: subscribe 必须在 publish 之前同步调用,
        // 否则事件会被静默丢弃(broadcast 不缓存历史)
        let bus = EventBus::new();
        let mut rx = bus.subscribe(); // 先订阅,确保收到事件
        let mut registry = SpecRegistry::with_event_bus(bus);

        // 注册一个初始版本 spec
        let spec = make_spec("quest-parse", 1, None);
        let version = registry.register(spec).expect("注册应成功");
        assert_eq!(version, 1);

        // 验证 SpecRegistered 事件已发布
        let event = rx
            .try_recv()
            .expect("应有 SpecRegistered 事件")
            .expect("事件不应为 None");

        match event {
            NexusEvent::SpecRegistered {
                spec_name,
                spec_version,
                parent_version,
                source,
                ..
            } => {
                assert_eq!(spec_name, "quest-parse");
                assert_eq!(spec_version, 1);
                assert_eq!(parent_version, None); // 初始版本无 parent
                assert_eq!(source, "spec-registry"); // 默认 source
            }
            other => panic!("期望 SpecRegistered 事件,实际: {:?}", other),
        }
    }

    #[test]
    fn test_register_child_version_event_contains_parent() {
        // P5.2.3: 验证注册子版本时,事件的 parent_version 字段正确填充
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut registry = SpecRegistry::with_event_bus(bus);

        // 注册 v1(初始版本)
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        // 消费 v1 的事件
        let _v1_event = rx.try_recv().unwrap().unwrap();

        // 注册 v2(parent=1)
        registry
            .register(make_spec("quest-parse", 2, Some(1)))
            .unwrap();

        // 验证 v2 的事件
        let event = rx.try_recv().unwrap().unwrap();
        match event {
            NexusEvent::SpecRegistered {
                spec_name,
                spec_version,
                parent_version,
                source,
                ..
            } => {
                assert_eq!(spec_name, "quest-parse");
                assert_eq!(spec_version, 2);
                assert_eq!(parent_version, Some(1)); // 子版本的 parent 应为 1
                assert_eq!(source, "spec-registry");
            }
            other => panic!("期望 SpecRegistered 事件,实际: {:?}", other),
        }
    }

    #[test]
    fn test_register_with_source_channel_b() {
        // P5.2.3: 验证 register_with_source 可指定 source="rhi-cg-channel-b"
        //
        // 这是通道 B 否决通过后的主注册路径
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut registry = SpecRegistry::with_event_bus(bus);

        let spec = make_spec("quest-parse", 1, None);
        registry
            .register_with_source(spec, "rhi-cg-channel-b")
            .expect("注册应成功");

        let event = rx.try_recv().unwrap().unwrap();
        match event {
            NexusEvent::SpecRegistered { source, .. } => {
                assert_eq!(source, "rhi-cg-channel-b");
            }
            other => panic!("期望 SpecRegistered 事件,实际: {:?}", other),
        }
    }

    #[test]
    fn test_register_validation_failure_does_not_publish_event() {
        // P5.2.3: 验证 validate() 失败时不发布事件(避免虚假成功通知)
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut registry = SpecRegistry::with_event_bus(bus);

        // version=0 违反 validate() 的 InvalidVersion 规则
        let mut bad_spec = make_spec("quest-parse", 0, None);
        bad_spec.meta.version = 0;

        let result = registry.register(bad_spec);
        assert!(result.is_err());

        // 验证没有事件被发布
        let event = rx.try_recv().expect("try_recv 不应报错");
        assert!(
            event.is_none(),
            "validate 失败时不应发布 SpecRegistered 事件"
        );
    }

    #[test]
    fn test_register_version_conflict_does_not_publish_event() {
        // P5.2.3: 验证 VersionConflict 失败时不发布事件
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut registry = SpecRegistry::with_event_bus(bus);

        // 第一次注册成功(发布事件)
        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();
        let _first_event = rx.try_recv().unwrap().unwrap();

        // 第二次重复注册(版本冲突)
        let result = registry.register(make_spec("quest-parse", 1, None));
        assert!(matches!(
            result,
            Err(SpecRegistryError::VersionConflict { .. })
        ));

        // 验证没有新事件被发布
        let event = rx.try_recv().expect("try_recv 不应报错");
        assert!(
            event.is_none(),
            "VersionConflict 失败时不应发布 SpecRegistered 事件"
        );
    }

    #[test]
    fn test_register_immutable_overwrite_does_not_publish_event() {
        // P5.2.3: 验证 ImmutableSpecOverwrite 失败时不发布事件
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut registry = SpecRegistry::with_event_bus(bus);

        // 注册不可进化 v1(成功,发布事件)
        registry
            .register(make_immutable_spec("critical-rule", 1))
            .unwrap();
        let _first_event = rx.try_recv().unwrap().unwrap();

        // 尝试注册 v2(被不可进化面守护拒绝)
        let result = registry.register(make_spec("critical-rule", 2, Some(1)));
        assert!(matches!(
            result,
            Err(SpecRegistryError::ImmutableSpecOverwrite { .. })
        ));

        // 验证没有新事件被发布
        let event = rx.try_recv().expect("try_recv 不应报错");
        assert!(
            event.is_none(),
            "ImmutableSpecOverwrite 失败时不应发布 SpecRegistered 事件"
        );
    }

    #[test]
    fn test_register_parent_missing_does_not_publish_event() {
        // P5.2.3: 验证 ParentMissing 失败时不发布事件
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut registry = SpecRegistry::with_event_bus(bus);

        // parent=5 但 v5 未注册
        let result = registry.register(make_spec("quest-parse", 6, Some(5)));
        assert!(matches!(
            result,
            Err(SpecRegistryError::ParentMissing { .. })
        ));

        // 验证没有事件被发布
        let event = rx.try_recv().expect("try_recv 不应报错");
        assert!(
            event.is_none(),
            "ParentMissing 失败时不应发布 SpecRegistered 事件"
        );
    }

    #[test]
    fn test_multiple_registrations_publish_multiple_events() {
        // P5.2.3: 验证多次注册会发布多个事件(无事件合并/丢失)
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut registry = SpecRegistry::with_event_bus(bus);

        // 注册 3 个不同 spec
        registry.register(make_spec("spec-a", 1, None)).unwrap();
        registry.register(make_spec("spec-b", 1, None)).unwrap();
        registry.register(make_spec("spec-c", 1, None)).unwrap();

        // 应收到 3 个 SpecRegistered 事件
        for expected_name in ["spec-a", "spec-b", "spec-c"] {
            let event = rx.try_recv().unwrap().unwrap();
            match event {
                NexusEvent::SpecRegistered { spec_name, .. } => {
                    assert_eq!(spec_name, expected_name);
                }
                other => panic!("期望 SpecRegistered,实际: {:?}", other),
            }
        }

        // 验证无更多事件
        assert!(rx.try_recv().unwrap().is_none());
    }

    #[test]
    fn test_event_metadata_source_is_gsoe_evolution() {
        // P5.2.3: 验证事件 metadata.source 为 "gsoe-evolution"(发布者标识)
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut registry = SpecRegistry::with_event_bus(bus);

        registry
            .register(make_spec("quest-parse", 1, None))
            .unwrap();

        let event = rx.try_recv().unwrap().unwrap();
        match event {
            NexusEvent::SpecRegistered { metadata, .. } => {
                assert_eq!(metadata.source, "gsoe-evolution");
            }
            other => panic!("期望 SpecRegistered,实际: {:?}", other),
        }
    }

    // ============================================================
    // P5.2.3: into_gsoe_error 不可进化面违反映射测试
    // ============================================================

    #[test]
    fn test_into_gsoe_error_maps_immutable_overwrite() {
        // P5.2.3: ImmutableSpecOverwrite 应映射为 ImmutableSurfaceViolated
        let err = SpecRegistryError::ImmutableSpecOverwrite {
            name: "critical-rule".to_string(),
        };
        let gsoe_err = SpecRegistry::into_gsoe_error(err);
        match gsoe_err {
            GsoeError::ImmutableSurfaceViolated { reason } => {
                assert!(reason.contains("critical-rule"));
                assert!(reason.contains("immutable=true"));
            }
            other => panic!("期望 ImmutableSurfaceViolated,实际: {:?}", other),
        }
    }

    #[test]
    fn test_into_gsoe_error_maps_immutable_surface_violation() {
        // P5.2.3: ValidationFailed(ImmutableSurfaceViolation) 应映射为 ImmutableSurfaceViolated
        // 使用 nexus_contracts::ImmutableSurface 的第一个变体做测试
        let surface = ImmutableSurface::RedlineLockAcrossAwait;
        let err =
            SpecRegistryError::ValidationFailed(HarnessSpecError::ImmutableSurfaceViolation {
                location: "contracts[0].from".to_string(),
                surface,
            });
        let gsoe_err = SpecRegistry::into_gsoe_error(err);
        match gsoe_err {
            GsoeError::ImmutableSurfaceViolated { reason } => {
                assert!(reason.contains("contracts[0].from"));
                assert!(reason.contains("不可进化面"));
            }
            other => panic!("期望 ImmutableSurfaceViolated,实际: {:?}", other),
        }
    }

    #[test]
    fn test_into_gsoe_error_preserves_other_errors_as_config() {
        // P5.2.3: 非不可进化面错误仍归类为 ConfigError(向后兼容)
        let err = SpecRegistryError::NoCandidate {
            name: "test".to_string(),
        };
        let gsoe_err = SpecRegistry::into_gsoe_error(err);
        match gsoe_err {
            GsoeError::ConfigError { reason } => {
                assert!(reason.contains("test"));
                assert!(reason.contains("候选"));
            }
            other => panic!("期望 ConfigError,实际: {:?}", other),
        }

        // 验证 VersionConflict 也归类为 ConfigError
        let err = SpecRegistryError::VersionConflict {
            name: "spec-x".to_string(),
            version: 5,
        };
        let gsoe_err = SpecRegistry::into_gsoe_error(err);
        assert!(matches!(gsoe_err, GsoeError::ConfigError { .. }));
    }

    #[test]
    fn test_into_gsoe_error_maps_immutable_meta_not_marked() {
        // P5.2.3: ValidationFailed(ImmutableMetaNotMarked) 应映射为 ImmutableSurfaceViolated
        let err = SpecRegistryError::ValidationFailed(HarnessSpecError::ImmutableMetaNotMarked {
            name: "critical-redline".to_string(),
        });
        let gsoe_err = SpecRegistry::into_gsoe_error(err);
        match gsoe_err {
            GsoeError::ImmutableSurfaceViolated { reason } => {
                assert!(reason.contains("critical-redline"));
                assert!(reason.contains("immutable=true"));
            }
            other => panic!("期望 ImmutableSurfaceViolated,实际: {:?}", other),
        }
    }

    // ============================================================
    // P5.2.3: Default trait 测试(向后兼容)
    // ============================================================

    #[test]
    fn test_default_creates_registry_without_event_bus() {
        // P5.2.3: Default 实现应保持 new() 语义(无 EventBus)
        let registry = SpecRegistry::default();
        assert!(registry.event_bus().is_none());
        assert_eq!(registry.total_specs(), 0);
    }
}
