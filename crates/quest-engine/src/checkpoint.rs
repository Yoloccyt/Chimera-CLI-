//! 检查点管理器 — LHQP(Long-Horizon Quest Persistence)实现
//!
//! 对应架构层:L9 Quest
//! 对应创新点:LHQP(长周期任务持久化,进程崩溃后可从最近检查点恢复)
//!
//! # 设计决策(WHY)
//! - **文件布局**:`~/.aether/checkpoints/<quest_id>/<checkpoint_id>.bin`
//!   按 quest_id 分目录,避免单目录文件爆炸;支持多 Quest 并发持久化
//! - **序列化格式**:MessagePack(ADR-004,与 Event Bus 一致)
//!   跨进程兼容、紧凑(比 JSON 小 30-50%)、支持二进制数据
//! - **完整性校验**:SHA-256 哈希比对 `serialized_state`
//!   防止磁盘位翻转或人为篡改导致状态漂移
//! - **保留策略**:最近 N 个检查点(默认 5),超出删除最旧
//!   避免磁盘膨胀,同时保留足够回滚点供恢复
//! - **异步 I/O**:所有磁盘操作通过 `tokio::task::spawn_blocking` 封装,
//!   避免阻塞 tokio worker 线程(A1 优化:save/load 可能涉及多次文件 I/O +
//!   MessagePack 序列化 + SHA-256 校验,累计可达数十毫秒)
//!
//! # 架构红线
//! - 单函数 ≤ 200 行
//! - 禁止 unwrap()/expect() 在非测试代码
//! - 所有 IO 错误包装为 QuestError,不泄漏底层 io::Error

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nexus_core::{Checkpoint, Quest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::QuestError;

/// 检查点卫星态 — 与 Quest 本体检查点关联持久化的组件状态快照（卫星态接线工作包）
///
/// WHY 独立类型 + 旁路文件（sidecar）而非 L0 `Checkpoint` 加字段：
/// ① `Checkpoint` 是 L0 领域契约（`nexus-contracts-core`），加字段属跨层契约
///    变更（涉 ADR-213 F3 schema_version 演进语义）；卫星态是 L9 检查点管理器
///    的**关联存储**，既有文档契约明文"序列化后由调用方与 Checkpoint 关联存储，
///    **不修改 L0 Checkpoint**"（search_tree.rs / long_task_map.rs to_bytes 文档）。
/// ② sidecar 与主检查点同 `checkpoint_id` 配对、同生命周期（prune 一并清理），
///    语义等价于"独立序列化段"但零 L0 侵入；旧 checkpoint（无 sidecar）天然兼容。
///
/// 双层序列化设计：外层 `SatelliteState` 以具名 map 编码（ADR-213 R7 惯例），
/// 内层 `tree`/`map` 各为组件自持的 MessagePack bytes（`SearchTreeManager::to_bytes`
/// / `LongTaskMap::to_bytes` 输出）——组件内部格式演进不破坏外层壳格式。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SatelliteState {
    /// 搜索树组件态 bytes（`SearchTreeManager::to_bytes` 输出）
    pub tree: Vec<u8>,
    /// 长任务地图组件态 bytes（`LongTaskMap::to_bytes` 输出）
    pub map: Vec<u8>,
}

/// 检查点管理器 — 负责 Quest 状态的持久化与恢复
///
/// 文件结构:`~/.aether/checkpoints/<quest_id>/<checkpoint_id>.bin`
/// 序列化格式:MessagePack(ADR-004,与 Event Bus 一致)
/// 保留策略:最近 N 个检查点,超出删除最旧(避免磁盘膨胀)
#[derive(Clone)]
pub struct CheckpointManager {
    /// 检查点根目录(所有 Quest 的检查点按子目录组织)
    ///
    /// WHY derive Clone(ADR-257 D1):全部字段为路径/数目的值快照,Clone = 同一
    /// 检查点存储的两个句柄(磁盘为共享真源)——QuestEngine 浅 Clone 语义一致。
    checkpoint_dir: PathBuf,
    /// 每个 Quest 保留的最大检查点数(超出删除最旧)
    max_keep: usize,
}

impl CheckpointManager {
    /// 创建检查点管理器,默认保留 5 个检查点
    pub fn new(checkpoint_dir: PathBuf) -> Self {
        Self {
            checkpoint_dir,
            max_keep: 5,
        }
    }

    /// 创建检查点管理器,自定义保留数量
    pub fn with_max_keep(checkpoint_dir: PathBuf, max_keep: usize) -> Self {
        Self {
            checkpoint_dir,
            max_keep,
        }
    }

    /// 保存检查点 — 序列化 Quest 为 MessagePack,写入磁盘(异步)
    ///
    /// 无卫星态形态:与历史行为逐字节等价(不写 sidecar),委托
    /// [`Self::save_with_satellite`] 传 `None`。引擎自动检查点等无需卫星态的
    /// 调用方签名不变。
    pub async fn save(&self, quest: Arc<Quest>) -> Result<Checkpoint, QuestError> {
        self.save_with_satellite(quest, None).await
    }

    /// 保存检查点 + 卫星态旁路文件(异步)
    ///
    /// Quest 本体 `.bin` 之外,`satellite` 以 MessagePack 写入旁路文件
    /// `<checkpoint_id>.sat` — 与主检查点**同刻捕获、同 checkpoint_id 配对**,
    /// 保证恢复时 Quest 快照与组件态(任务地图/搜索树)时间对齐,不出现
    /// "卫星态新于 Quest 快照"的配对错位。
    ///
    /// 内部通过 `spawn_blocking` 将 Quest 编码 + SHA-256 校验 + 磁盘 I/O +
    /// prune_old 全部放到阻塞线程池执行,避免阻塞 tokio worker 线程。
    ///
    /// WHY 参数为 `Arc<Quest>`(WP-05):Quest 编码是 O(payload) 的同步 CPU,
    /// 与磁盘 I/O 一并收编进阻塞闭包,async worker 上不再有任何序列化 CPU 点;
    /// `Arc` 传参使闭包捕获只付引用计数克隆(rmp 序列化走 `&T`,无需解引用拷贝),
    /// 调用方持 Quest 所有权时 `Arc::new(quest)` 即零拷贝移交。
    ///
    /// 流程(步骤 2-6 在阻塞线程池内执行):
    /// 1. 生成 checkpoint_id(UUIDv7,时间有序,便于排序与因果追踪)
    /// 2. 序列化 Quest 为 MessagePack
    /// 3. 计算 SHA-256 哈希(用于恢复时完整性校验)
    /// 4. 创建 Checkpoint 实例并整体序列化为 MessagePack 写入磁盘(.bin)
    /// 5. 若带卫星态,序列化写入旁路文件(.sat)
    /// 6. 调用 prune_old 清理超出 max_keep 的旧检查点(连带清理其 sidecar)
    pub async fn save_with_satellite(
        &self,
        quest: Arc<Quest>,
        satellite: Option<&SatelliteState>,
    ) -> Result<Checkpoint, QuestError> {
        // UUIDv7 生成是 CPU-only 微开销操作,在 async 上下文中完成
        let checkpoint_id = format!("cp-{}", Uuid::now_v7());

        // 阻塞操作:Quest 编码 + SHA + 创建目录 + 写文件 + prune_old,全部收编进闭包
        let dir = self.checkpoint_dir.clone();
        let max_keep = self.max_keep;
        // WHY clone 为 owned:spawn_blocking 闭包 'static,借用无法跨越;
        // 卫星态 bytes 为 KB 量级组件快照,克隆开销可忽略
        let satellite_owned = satellite.cloned();

        tokio::task::spawn_blocking(move || {
            Self::encode_and_save(&quest, checkpoint_id, &dir, max_keep, satellite_owned.as_ref())
        })
        .await
        .map_err(|e| QuestError::CheckpointSaveFailed(format!("spawn_blocking join: {e}")))?
    }

    /// Quest 编码 + Checkpoint 构造 + 落盘 — save 的 spawn_blocking 闭包体
    ///
    /// WHY 独立静态函数:与 save_blocking 同理由 — spawn_blocking 要求闭包
    /// Send + 'static,显式传参避免捕获 `&self`;编码与落盘合入同一闭包,
    /// 序列化 CPU 与磁盘 I/O 同批离开 async worker。
    fn encode_and_save(
        quest: &Quest,
        checkpoint_id: String,
        checkpoint_dir: &Path,
        max_keep: usize,
        satellite: Option<&SatelliteState>,
    ) -> Result<Checkpoint, QuestError> {
        // 具名 map 编码(ADR-213 R7):数组形态把字段顺序钉成 wire 契约,同类型字段换序会
        // 静默改值;map 按 key 取值不受声明顺序影响。旧的位置式 blob 仍可被 from_slice
        // 读回(实测,见 nexus-contracts/tests/wire_format_compat.rs),故无需读侧垫片。
        let serialized_state = rmp_serde::to_vec_named(quest)
            .map_err(|e| QuestError::SerializationError(format!("msgpack encode: {e}")))?;
        let memory_snapshot_hash = compute_sha256_hex(&serialized_state);

        let checkpoint = Checkpoint::new(
            quest.quest_id.clone(),
            checkpoint_id,
            memory_snapshot_hash,
            serialized_state,
        );

        Self::save_blocking(checkpoint, checkpoint_dir, max_keep, satellite)
    }

    /// 检查点保存的阻塞部分 — 磁盘写入 + 卫星态旁路写入 + 旧检查点清理
    ///
    /// WHY 独立静态函数:spawn_blocking 要求闭包 Send + 'static,
    /// 将所需参数显式传入,避免捕获 `&self` 引发借用冲突。
    fn save_blocking(
        checkpoint: Checkpoint,
        checkpoint_dir: &Path,
        max_keep: usize,
        satellite: Option<&SatelliteState>,
    ) -> Result<Checkpoint, QuestError> {
        let quest_id = checkpoint.quest_id.clone();
        let checkpoint_id_str = checkpoint.checkpoint_id.clone();

        // 构造文件路径
        let file_path = checkpoint_dir
            .join(&quest_id)
            .join(format!("{checkpoint_id_str}.bin"));

        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                QuestError::CheckpointSaveFailed(format!("mkdir {}: {e}", parent.display()))
            })?;
        }

        // 序列化 Checkpoint 为 MessagePack 并写入磁盘(具名 map,理由见上;旧位置式文件仍可回读)
        let bytes = rmp_serde::to_vec_named(&checkpoint)
            .map_err(|e| QuestError::SerializationError(format!("msgpack encode cp: {e}")))?;

        // v2.3 §(b.5): 检查点落盘前拦截私钥/凭证/令牌类硬载荷(R3-1 补入的核心 Checkpoint 写点)。
        // Block-only:高熵/键名启发式设 Pass;benign 检查点(无硬标记)零影响;命中即拒绝保存。
        let scan_policy = nexus_contracts::sensitive::ScanPolicy {
            on_high_entropy: nexus_contracts::sensitive::SensitiveDisposition::Pass,
            on_secret_kv: nexus_contracts::sensitive::SensitiveDisposition::Pass,
            ..Default::default()
        };
        if nexus_contracts::sensitive::scan(&String::from_utf8_lossy(&bytes), &scan_policy)
            .is_blocked()
        {
            return Err(QuestError::CheckpointSaveFailed(format!(
                "checkpoint {checkpoint_id_str} 含敏感硬载荷(私钥/凭证/令牌),拒绝落盘"
            )));
        }

        fs::write(&file_path, bytes).map_err(|e| {
            QuestError::CheckpointSaveFailed(format!("write {}: {e}", file_path.display()))
        })?;

        // 卫星态旁路文件(sidecar):`<checkpoint_id>.sat`,与主 .bin 同刻捕获同 id 配对。
        // WHY 扩展名 .sat 而非 .sat.bin:list_checkpoints / load_latest_blocking /
        // prune_old_blocking 均按 extension=="bin" 过滤枚举,`.sat` 保证旁路文件
        // 不被误当作检查点主文件加载与排序(细节见 SatelliteState 文档)。
        if let Some(sat) = satellite {
            let sat_bytes = rmp_serde::to_vec_named(sat).map_err(|e| {
                QuestError::CheckpointSaveFailed(format!("msgpack encode satellite: {e}"))
            })?;
            // R3-1 同边界扩展:sidecar 是检查点写边界的新产物,与主 .bin 同受
            // 敏感硬载荷拦截(scan_policy 复用)—— 否则地图详情等自由文本字段
            // 会成为绕过主检查点扫描的旁路
            if nexus_contracts::sensitive::scan(&String::from_utf8_lossy(&sat_bytes), &scan_policy)
                .is_blocked()
            {
                return Err(QuestError::CheckpointSaveFailed(format!(
                    "checkpoint {checkpoint_id_str} 卫星态含敏感硬载荷(私钥/凭证/令牌),拒绝落盘"
                )));
            }
            let sat_path = file_path.with_extension("sat");
            fs::write(&sat_path, sat_bytes).map_err(|e| {
                // WHY 显式报错而非降级为 warn:主检查点已落盘仍可恢复(降级
                // quest-only),但卫星态持久化失败若静默吞掉,恢复链路会在
                // 无人察觉处无声断链——违背本工作包"跨会话记忆一致性"目标
                QuestError::CheckpointSaveFailed(format!(
                    "write satellite {}: {e}",
                    sat_path.display()
                ))
            })?;
        }

        // 清理超出 max_keep 的旧检查点(失败不阻断保存,仅记录)
        if let Err(e) = Self::prune_old_blocking(&quest_id, checkpoint_dir, max_keep) {
            tracing::warn!(
                quest_id = %quest_id,
                error = %e,
                "prune_old 失败,旧检查点未清理(不影响本次保存)"
            );
        }

        tracing::debug!(
            quest_id = %quest_id,
            checkpoint_id = %checkpoint_id_str,
            file = %file_path.display(),
            "检查点已保存"
        );
        Ok(checkpoint)
    }

    /// 加载指定检查点 — 读取磁盘并校验完整性(异步)
    ///
    /// 内部通过 `spawn_blocking` 将磁盘读取 + MessagePack 反序列化 + SHA-256 校验
    /// 放到阻塞线程池执行。
    pub async fn load(
        &self,
        quest_id: &str,
        checkpoint_id: &str,
    ) -> Result<Checkpoint, QuestError> {
        let file_path = self.checkpoint_path(quest_id, checkpoint_id);

        tokio::task::spawn_blocking(move || Self::load_blocking(&file_path))
            .await
            .map_err(|e| QuestError::CheckpointSaveFailed(format!("spawn_blocking join: {e}")))?
    }

    /// 检查点加载的阻塞部分 — 磁盘读取 + 反序列化 + 完整性校验
    fn load_blocking(file_path: &Path) -> Result<Checkpoint, QuestError> {
        let bytes = fs::read(file_path).map_err(|e| {
            QuestError::CheckpointNotFound(format!("read {}: {e}", file_path.display()))
        })?;
        let checkpoint: Checkpoint = rmp_serde::from_slice(&bytes)
            .map_err(|e| QuestError::SerializationError(format!("msgpack decode cp: {e}")))?;
        // 完整性校验(防磁盘位翻转或人为篡改)
        let actual_hash = compute_sha256_hex(&checkpoint.serialized_state);
        if actual_hash != checkpoint.memory_snapshot_hash {
            return Err(QuestError::CheckpointCorrupted);
        }
        Ok(checkpoint)
    }

    /// 加载最新检查点 + 卫星态并解码 Quest — 磁盘读取 + 完整性校验 + MessagePack 解码
    /// 合入单个 spawn_blocking 闭包(WP-05 恢复侧收编)
    ///
    /// WHY 单闭包合并:磁盘 IO(load_latest_blocking)、Quest 解码(from_slice)
    /// 与卫星态旁路读取同属阻塞面操作,合入同一闭包少两次阻塞池往返。
    /// WHY 返回双 Quest 副本:恢复路径有两个所有权消费方(注册表 insert 与公共
    /// API 返回值),二者所需的 O(payload) 克隆是同步 CPU — 与解码同在阻塞池内
    /// 执行,async worker 上零 payload 拷贝;`restore_from_checkpoint` 的公共
    /// 签名(返回 Quest)保持不变。
    /// 返回:(检查点(供 checkpoint_id 等元数据使用), 注册表副本, 返回值副本,
    /// 卫星态(旧 checkpoint 无 sidecar / sidecar 损坏时为 None))
    ///
    /// WHY 卫星态缺失降级为 None 而非报错(需求语义):旧 checkpoint(本工作包
    /// 之前保存)没有 sidecar 文件,恢复必须降级为"仅 Quest 本体"的现行为;
    /// sidecar 损坏同理 —— 主检查点已过 SHA-256 校验,卫星态丢失只损失组件态。
    pub(crate) async fn load_latest_full(
        &self,
        quest_id: &str,
    ) -> Result<Option<(Checkpoint, Quest, Quest, Option<SatelliteState>)>, QuestError> {
        let dir = self.checkpoint_dir.join(quest_id);
        let quest_id = quest_id.to_string();

        tokio::task::spawn_blocking(move || {
            let checkpoint = Self::load_latest_blocking(&dir, &quest_id)?;
            match checkpoint {
                Some(cp) => {
                    let quest: Quest =
                        rmp_serde::from_slice(&cp.serialized_state).map_err(|e| {
                            QuestError::SerializationError(format!("msgpack decode quest: {e}"))
                        })?;
                    // 双副本均在阻塞池内克隆(见方法 WHY),async 线程只做所有权移动
                    let registry_copy = quest.clone();
                    // 卫星态旁路读取:缺失/损坏 → None(降级语义,read_satellite_blocking 内部已记日志)
                    let satellite = Self::read_satellite_blocking(&dir, &cp.checkpoint_id);
                    Ok(Some((cp, registry_copy, quest, satellite)))
                }
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| QuestError::CheckpointSaveFailed(format!("spawn_blocking join: {e}")))?
    }

    /// 读取检查点的卫星态旁路文件 — 缺失/损坏返回 None(降级语义,不报错)
    ///
    /// WHY None 而非 Err:卫星态缺失(旧 checkpoint 无 `.sat` 文件)时恢复必须
    /// 降级为"仅 Quest 本体"的现行为;损坏同理。主检查点本体已通过 SHA-256
    /// 完整性校验,卫星态丢失只损失任务地图/搜索树组件态,不阻断 Quest 恢复。
    /// NotFound(常态路径)静默 None,其余 IO 错误记 warn 后同样降级。
    fn read_satellite_blocking(quest_dir: &Path, checkpoint_id: &str) -> Option<SatelliteState> {
        let sat_path = quest_dir.join(format!("{checkpoint_id}.sat"));
        let bytes = match fs::read(&sat_path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                tracing::warn!(
                    checkpoint_id = %checkpoint_id,
                    error = %e,
                    "卫星态旁路文件读取失败,降级为无卫星态恢复"
                );
                return None;
            }
        };
        match rmp_serde::from_slice::<SatelliteState>(&bytes) {
            Ok(sat) => Some(sat),
            Err(e) => {
                tracing::warn!(
                    checkpoint_id = %checkpoint_id,
                    error = %e,
                    "卫星态旁路文件解码失败(可能损坏),降级为无卫星态恢复"
                );
                None
            }
        }
    }

    /// 加载最新检查点(按 created_at 排序)— 无检查点返回 None(异步)
    ///
    /// WHY:崩溃恢复场景下,用户只需"最新可用检查点",无需知道具体 ID。
    /// 按 created_at 排序而非 checkpoint_id,因前者语义明确(时间),
    /// 后者虽 UUIDv7 时间有序但解析复杂。
    ///
    /// 内部通过 `spawn_blocking` 将多次磁盘读取 + 完整性校验放到阻塞线程池,
    /// 避免 load_latest 最差情况(max_keep 次文件读取)阻塞 tokio worker。
    pub async fn load_latest(&self, quest_id: &str) -> Result<Option<Checkpoint>, QuestError> {
        let dir = self.checkpoint_dir.join(quest_id);
        let quest_id = quest_id.to_string();

        tokio::task::spawn_blocking(move || Self::load_latest_blocking(&dir, &quest_id))
            .await
            .map_err(|e| QuestError::CheckpointSaveFailed(format!("spawn_blocking join: {e}")))?
    }

    /// 加载最新检查点的阻塞部分 — 列出 + 逐个加载 + 排序
    fn load_latest_blocking(
        quest_dir: &Path,
        quest_id: &str,
    ) -> Result<Option<Checkpoint>, QuestError> {
        // 列出所有 .bin 文件(等价于 list_checkpoints 的文件遍历部分)
        if !quest_dir.exists() {
            return Ok(None);
        }
        let entries = fs::read_dir(quest_dir).map_err(|e| {
            QuestError::CheckpointSaveFailed(format!("readdir {}: {e}", quest_dir.display()))
        })?;
        let mut ids = Vec::new();
        for entry in entries {
            let entry = entry
                .map_err(|e| QuestError::CheckpointSaveFailed(format!("readdir entry: {e}")))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("bin") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    ids.push(stem.to_string());
                }
            }
        }
        if ids.is_empty() {
            return Ok(None);
        }

        // 逐个加载并校验(失败的跳过,不阻断)
        let mut checkpoints: Vec<Checkpoint> = Vec::with_capacity(ids.len());
        for id in &ids {
            let file_path = quest_dir.join(format!("{id}.bin"));
            match Self::load_blocking(&file_path) {
                Ok(cp) => checkpoints.push(cp),
                Err(e) => {
                    tracing::warn!(
                        quest_id = quest_id,
                        checkpoint_id = %id,
                        error = %e,
                        "加载检查点失败,跳过(可能已损坏)"
                    );
                }
            }
        }
        // O(n) 替代 O(n log n): 仅需最大值,无需全排序
        Ok(checkpoints.into_iter().max_by_key(|cp| cp.created_at))
    }

    /// 校验检查点完整性 — 重新计算 SHA-256 与存储的 hash 比对
    ///
    /// 不匹配返回 `CheckpointCorrupted`,防止使用被篡改/损坏的状态恢复
    pub fn verify_integrity(&self, checkpoint: &Checkpoint) -> Result<(), QuestError> {
        let actual_hash = compute_sha256_hex(&checkpoint.serialized_state);
        if actual_hash != checkpoint.memory_snapshot_hash {
            return Err(QuestError::CheckpointCorrupted);
        }
        Ok(())
    }

    /// 保留最近 N 个检查点,删除其余(异步)
    ///
    /// 按文件名字典序升序排序(UUIDv7 时间序,见 prune_old_blocking 的 WHY 注释),
    /// 保留最新 N 个,删除其余文件;零解码,不读取 .bin 内容。
    /// WHY:避免磁盘膨胀,同时保留足够回滚点。
    pub async fn prune_old(&self, quest_id: &str, keep: usize) -> Result<(), QuestError> {
        let dir = self.checkpoint_dir.clone();
        let quest_id = quest_id.to_string();

        tokio::task::spawn_blocking(move || Self::prune_old_blocking(&quest_id, &dir, keep))
            .await
            .map_err(|e| QuestError::CheckpointSaveFailed(format!("spawn_blocking join: {e}")))?
    }

    /// 检查点清理的阻塞部分 — 列出 + 文件名排序 + 删除旧文件(零解码)
    ///
    /// WHY 独立静态函数:save_blocking 内部调用,需纯同步实现。
    fn prune_old_blocking(
        quest_id: &str,
        checkpoint_dir: &Path,
        keep: usize,
    ) -> Result<(), QuestError> {
        let quest_dir = checkpoint_dir.join(quest_id);
        if !quest_dir.exists() {
            return Ok(());
        }

        // 列出所有 .bin 文件
        let entries = fs::read_dir(&quest_dir).map_err(|e| {
            QuestError::CheckpointSaveFailed(format!("readdir {}: {e}", quest_dir.display()))
        })?;
        let mut ids = Vec::new();
        for entry in entries {
            let entry = entry
                .map_err(|e| QuestError::CheckpointSaveFailed(format!("readdir entry: {e}")))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("bin") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    ids.push(stem.to_string());
                }
            }
        }

        if ids.len() <= keep {
            return Ok(());
        }

        // WHY 文件名排序可替代 created_at:checkpoint_id = UUIDv7(前 48 bit 为毫秒
        // 时间戳),文件名前缀 `cp-` 相同,字典序 ≈ 时间序。同毫秒内多个 v7 的相对
        // 顺序虽不确定,但 save 为串行 await(每次落盘 ≈4ms 磁盘开销),同毫秒至多
        // 1 个检查点,无实际影响;用户可见的"取最新"路径 load_latest_blocking 仍按
        // created_at 排序,不受此处排序语义影响。
        // WHY 零解码:旧实现为取 created_at 对每个文件做完整反序列化 + SHA-256 校验
        // (O(N×payload));文件名排序仅 O(N log N) 字符串比较 + O(删量) unlink,
        // 不读取任何 .bin 内容。
        // 顺带修复:旧实现损坏文件(非法 msgpack)加载失败被 `if let Ok` 跳过,既不
        // 参与排序也永不清理;新实现按文件名正常纳入删除范围。
        ids.sort_unstable();
        // ids.len() > keep 已由上方提前返回保证,此处不会下溢
        for id in &ids[..ids.len() - keep] {
            let path = quest_dir.join(format!("{id}.bin"));
            if let Err(e) = fs::remove_file(&path) {
                tracing::warn!(
                    quest_id = quest_id,
                    checkpoint_id = id,
                    error = %e,
                    "删除旧检查点失败(继续清理其余)"
                );
            }
            // 卫星态旁路文件随主检查点一并删除(防孤儿 .sat 磁盘泄漏)。
            // WHY 容忍 NotFound:主文件删除失败而 sidecar 存在、或历史无卫星态
            // 检查点本无 sidecar,均为正常状态,与主文件删除容错语义一致。
            let sat_path = quest_dir.join(format!("{id}.sat"));
            if let Err(e) = fs::remove_file(&sat_path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(
                        quest_id = quest_id,
                        checkpoint_id = id,
                        error = %e,
                        "删除卫星态旁路文件失败(继续清理其余)"
                    );
                }
            }
        }
        Ok(())
    }

    /// 列出某 Quest 的全部检查点 ID(按文件名,不加载 .bin 内容)
    ///
    /// 返回顺序未定义,调用方需自行排序(如 load_latest 按 created_at 排序)。
    ///
    /// # 性能(L9 优化 2.5 数据关门,2026-07-31)
    ///
    /// 本方法仅 `read_dir` + 取文件名 stem,**不反序列化任何 .bin**,已是最优。
    /// criterion 实测 `checkpoint_bench`:save(100 任务)≈4.0ms(< 5ms 阈值),
    /// 且 save(10) 与 save(100) 几乎持平(瓶颈在磁盘 I/O 固定开销 ~3.7ms 而非
    /// 序列化量)。故 v4 报告 §9.1 IncrementalCheckpointSystem 与元数据边车
    /// 优化**明确不做**(数据关门):收益低于磁盘固定开销,且涉 ADR-004 格式
    /// 变更需另立 ADR。此结论以基准数据为据,非主观判断。
    pub fn list_checkpoints(&self, quest_id: &str) -> Result<Vec<String>, QuestError> {
        let dir = self.quest_dir(quest_id);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let entries = fs::read_dir(&dir).map_err(|e| {
            QuestError::CheckpointSaveFailed(format!("readdir {}: {e}", dir.display()))
        })?;
        let mut ids = Vec::new();
        for entry in entries {
            let entry = entry
                .map_err(|e| QuestError::CheckpointSaveFailed(format!("readdir entry: {e}")))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("bin") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    ids.push(stem.to_string());
                }
            }
        }
        Ok(ids)
    }

    /// 构造 Quest 检查点目录:`<checkpoint_dir>/<quest_id>/`
    fn quest_dir(&self, quest_id: &str) -> PathBuf {
        self.checkpoint_dir.join(quest_id)
    }

    /// 构造检查点文件路径:`<checkpoint_dir>/<quest_id>/<checkpoint_id>.bin`
    fn checkpoint_path(&self, quest_id: &str, checkpoint_id: &str) -> PathBuf {
        self.quest_dir(quest_id)
            .join(format!("{checkpoint_id}.bin"))
    }

    /// 检查点根目录(只读访问,供测试与诊断)
    pub fn checkpoint_dir(&self) -> &Path {
        &self.checkpoint_dir
    }

    /// 当前保留上限
    pub fn max_keep(&self) -> usize {
        self.max_keep
    }

    /// 从已加载的 Checkpoint 导出轨迹四元组(P4-W16.1.3 捕获点 2)
    ///
    /// 对应 spec.md §Scenario "model-router 轨迹捕获" 捕获点 2:
    /// quest-engine Checkpoint(MessagePack 快照)→ 轨迹导出器
    /// (状态/动作/奖励/上下文摘要四元组)
    ///
    /// # 设计决策
    /// - **同步方法**:导出是纯内存转换(MessagePack 解码 + 字段提取),
    ///   无 I/O,无需 spawn_blocking
    /// - **不可变借用**:仅观察 Checkpoint,不修改状态(与 RouteHook 契约一致)
    /// - **错误隔离**:解码失败返回 `TrajectoryExportFailed`,不阻断调用方
    ///
    /// # 参数
    /// - `checkpoint`:已通过 `load` / `load_latest` 加载的检查点
    ///
    /// # 返回
    /// `QuestTrajectory` 四元组(state/action/reward/context),供 P4-W16.2 经验回放池消费
    ///
    /// # 使用示例
    /// ```rust,ignore
    /// use quest_engine::CheckpointManager;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let cm = CheckpointManager::new("/tmp/cps".into());
    /// let checkpoint = cm.load_latest("quest-123").await?.expect("存在检查点");
    /// let trajectory = cm.export_trajectory(&checkpoint)?;
    /// println!("net_reward = {:.3}", trajectory.reward.net_reward);
    /// # Ok(())
    /// # }
    /// ```
    pub fn export_trajectory(
        &self,
        checkpoint: &Checkpoint,
    ) -> Result<crate::trajectory_exporter::QuestTrajectory, QuestError> {
        crate::trajectory_exporter::export_trajectory(checkpoint)
    }
}

/// 计算 SHA-256 哈希并返回十六进制字符串
///
/// WHY 单独函数:save 与 verify_integrity 共用,确保哈希算法一致
fn compute_sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let hash = hasher.finalize();
    hex::encode(hash)
}

/// 检查点元数据(用于内部排序与诊断,不包含完整序列化数据)
///
/// WHY:此类型当前未在公共 API 使用,预留供未来"轻量索引文件"优化 —
/// 当前直接从 .bin 文件加载完整 Checkpoint 获取 created_at,
/// 文件数少时性能可接受;后续若检查点数量增长,可单独持久化元数据索引
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)] // 预留供未来索引优化使用
struct CheckpointMetaInternal {
    checkpoint_id: String,
    quest_id: String,
    created_at: chrono::DateTime<chrono::Utc>,
    memory_snapshot_hash: String,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::long_task_map::{LongTaskMap, NodeStatus, StepResult};
    use crate::memory_sync_hook::NoopMemorySyncHook;
    use crate::search_tree::SearchTreeManager;
    use chrono::Utc;
    use nexus_contracts::experience_card::{
        AtomicOperator, CardMetadata, ExecutionStatus, ThreeFactorScore,
    };
    use nexus_contracts::ExperienceCard;
    use nexus_core::{MultimodalInput, Task, TaskStatus, ThinkingMode, UserIntent};
    use tempfile::tempdir;

    /// 构造卫星态测试用经验卡片(搜索树扩展节点)
    fn sat_card(node_id: &str, parent_id: Option<&str>, score: f32) -> ExperienceCard {
        ExperienceCard {
            card_id: Box::from(format!("card_{node_id}")),
            task_id: Box::from("q-sat"),
            node_id: Box::from(node_id),
            parent_id: parent_id.map(Box::from),
            created_at: Utc::now(),
            operator: AtomicOperator::Improve,
            score,
            delta_vs_parent: 0.1,
            method_family: Box::from("test"),
            error_signature: None,
            three_factor: ThreeFactorScore {
                quality: score,
                progress: 0.1,
                novelty: 0.5,
            },
            execution_status: ExecutionStatus::Success,
            token_evidence_ids: Vec::new(),
            segment_id: None,
            metadata: CardMetadata::default(),
        }
    }

    /// 构造卫星态测试用步骤(任务地图 record_step 输入)
    fn sat_step(state: &str, success: bool) -> StepResult {
        StepResult {
            state: state.to_string(),
            detail: format!("{state} 的详细内容"),
            next_action: "continue".to_string(),
            action: "analyze".to_string(),
            success,
        }
    }

    /// 构造测试用 Quest
    fn make_quest(id: &str, task_count: usize) -> Quest {
        let tasks = (0..task_count)
            .map(|i| Task {
                task_id: format!("task-{i}"),
                description: format!("任务 {i}"),
                status: TaskStatus::Pending,
                dependencies: if i == 0 {
                    vec![]
                } else {
                    vec![format!("task-{}", i - 1)]
                },
            })
            .collect();
        Quest {
            quest_id: id.into(),
            title: format!("测试 Quest {id}"),
            tasks,
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
            priority: 128,
        }
    }

    #[tokio::test]
    async fn test_save_load_roundtrip() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let quest = make_quest("q-1", 3);

        let checkpoint = cm.save(Arc::new(quest.clone())).await.unwrap();
        assert_eq!(checkpoint.quest_id, "q-1");
        assert!(!checkpoint.serialized_state.is_empty());
        assert!(!checkpoint.memory_snapshot_hash.is_empty());

        let loaded = cm.load("q-1", &checkpoint.checkpoint_id).await.unwrap();
        assert_eq!(loaded.quest_id, checkpoint.quest_id);
        assert_eq!(loaded.checkpoint_id, checkpoint.checkpoint_id);
        assert_eq!(loaded.memory_snapshot_hash, checkpoint.memory_snapshot_hash);
        assert_eq!(loaded.serialized_state, checkpoint.serialized_state);

        // 反序列化 Quest 验证字段一致
        let restored_quest: Quest = rmp_serde::from_slice(&loaded.serialized_state).unwrap();
        assert_eq!(restored_quest.quest_id, quest.quest_id);
        assert_eq!(restored_quest.tasks.len(), quest.tasks.len());
        assert_eq!(restored_quest.thinking_mode, quest.thinking_mode);
    }

    #[tokio::test]
    async fn test_verify_integrity_corrupted() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let quest = make_quest("q-1", 2);

        let mut checkpoint = cm.save(Arc::new(quest)).await.unwrap();
        // 篡改 serialized_state,哈希应不匹配
        checkpoint.serialized_state[0] ^= 0xff;
        let result = cm.verify_integrity(&checkpoint);
        assert!(matches!(result, Err(QuestError::CheckpointCorrupted)));
    }

    #[tokio::test]
    async fn test_load_corrupted_file_returns_error() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let quest = make_quest("q-1", 2);

        let checkpoint = cm.save(Arc::new(quest)).await.unwrap();
        // 直接篡改磁盘文件
        let path = cm.checkpoint_path("q-1", &checkpoint.checkpoint_id);
        let mut bytes = std::fs::read(&path).unwrap();
        // 翻转最后一个字节(可能影响 serialized_state 或 hash 字段)
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&path, bytes).unwrap();

        let result = cm.load("q-1", &checkpoint.checkpoint_id).await;
        // 篡改可能破坏反序列化或哈希校验,任一错误均可接受
        assert!(result.is_err(), "篡改文件后 load 应失败");
    }

    #[tokio::test]
    async fn test_prune_old_keeps_latest_n() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::with_max_keep(tmp.path().to_path_buf(), 3);
        let quest = make_quest("q-1", 1);

        // 创建 5 个检查点
        let mut ids = Vec::new();
        for _ in 0..5 {
            // 微小延迟确保 created_at 不同(chrono::Utc::now 精度可能不足)
            std::thread::sleep(std::time::Duration::from_millis(5));
            let cp = cm.save(Arc::new(quest.clone())).await.unwrap();
            ids.push(cp.checkpoint_id);
        }

        let remaining = cm.list_checkpoints("q-1").unwrap();
        assert_eq!(remaining.len(), 3, "应保留最近 3 个检查点");
        // 最新的 3 个应保留(后创建的)
        assert!(remaining.contains(&ids[3]));
        assert!(remaining.contains(&ids[4]));
    }

    #[tokio::test]
    async fn test_load_latest_returns_none_when_empty() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let result = cm.load_latest("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_load_latest_returns_most_recent() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let quest = make_quest("q-1", 1);

        let mut newest_time = chrono::DateTime::<chrono::Utc>::MIN_UTC;
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            let cp = cm.save(Arc::new(quest.clone())).await.unwrap();
            newest_time = cp.created_at;
        }

        let latest = cm.load_latest("q-1").await.unwrap().unwrap();
        assert_eq!(latest.created_at, newest_time);
    }

    #[test]
    fn test_list_checkpoints_empty_quest() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let ids = cm.list_checkpoints("no-such-quest").unwrap();
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn test_load_nonexistent_returns_error() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let result = cm.load("q-1", "cp-nonexistent").await;
        assert!(matches!(result, Err(QuestError::CheckpointNotFound(_))));
    }

    #[test]
    fn test_compute_sha256_hex_deterministic() {
        let h1 = compute_sha256_hex(b"hello");
        let h2 = compute_sha256_hex(b"hello");
        let h3 = compute_sha256_hex(b"world");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        // SHA-256 hex 长度为 64
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_with_max_keep_custom() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::with_max_keep(tmp.path().to_path_buf(), 10);
        assert_eq!(cm.max_keep(), 10);
    }

    #[test]
    fn test_checkpoint_dir_accessor() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().to_path_buf();
        let cm = CheckpointManager::new(path.clone());
        assert_eq!(cm.checkpoint_dir(), &path);
    }

    #[tokio::test]
    async fn test_save_creates_nested_directory() {
        let tmp = tempdir().unwrap();
        // 嵌套不存在的目录,save 应自动创建
        let nested = tmp.path().join("a").join("b").join("c");
        let cm = CheckpointManager::new(nested.clone());
        let quest = make_quest("q-1", 1);
        let result = cm.save(Arc::new(quest)).await;
        assert!(result.is_ok(), "应自动创建嵌套目录");
        assert!(nested.exists());
    }

    #[tokio::test]
    async fn test_multiple_quests_isolated() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let q1 = make_quest("q-1", 2);
        let q2 = make_quest("q-2", 3);

        cm.save(Arc::new(q1)).await.unwrap();
        cm.save(Arc::new(q2)).await.unwrap();

        // 各 Quest 的检查点互不影响
        let ids1 = cm.list_checkpoints("q-1").unwrap();
        let ids2 = cm.list_checkpoints("q-2").unwrap();
        assert_eq!(ids1.len(), 1);
        assert_eq!(ids2.len(), 1);

        // 删除 q-1 的检查点不影响 q-2
        let path1 = cm.checkpoint_path("q-1", &ids1[0]);
        std::fs::remove_file(&path1).unwrap();
        assert_eq!(cm.list_checkpoints("q-2").unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_prune_when_under_limit() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::with_max_keep(tmp.path().to_path_buf(), 5);
        let quest = make_quest("q-1", 1);

        // 仅创建 2 个检查点(未超限),prune 不应删除任何
        cm.save(Arc::new(quest.clone())).await.unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        cm.save(Arc::new(quest.clone())).await.unwrap();

        let ids = cm.list_checkpoints("q-1").unwrap();
        assert_eq!(ids.len(), 2);
    }

    /// 验证 max_by_key 在 created_at 相同时的行为确定性:
    /// 两个相同 created_at 的 checkpoint,应返回其中一个(不 panic,且结果有效)
    #[tokio::test]
    async fn test_latest_checkpoint_tie_breaker() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let quest = make_quest("q-tie", 1);

        // 连续保存两个检查点(极短时间内 created_at 可能相同)
        let cp1 = cm.save(Arc::new(quest.clone())).await.unwrap();
        let cp2 = cm.save(Arc::new(quest.clone())).await.unwrap();

        let latest = cm.load_latest("q-tie").await.unwrap();
        assert!(latest.is_some(), "应返回某个检查点");
        let latest = latest.unwrap();
        // 结果必须是 cp1 或 cp2 之一(不会是凭空出现的)
        assert!(
            latest.checkpoint_id == cp1.checkpoint_id || latest.checkpoint_id == cp2.checkpoint_id,
            "tie 情况下应返回已存在的某个 checkpoint"
        );
        // 返回的 created_at 必须 >= 两者中较小的(即确实是最大值)
        assert!(latest.created_at >= cp1.created_at);
        assert!(latest.created_at >= cp2.created_at);
    }

    /// 验证 UserIntent 与 Quest 的 MessagePack 序列化兼容性
    /// (确保未来扩展字段不破坏旧检查点)
    #[test]
    fn test_msgpack_quest_serialization_stable() {
        let quest = make_quest("q-stable", 4);
        let bytes = rmp_serde::to_vec(&quest).unwrap();
        let de: Quest = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(de, quest);
    }

    /// 验证包含多模态输入的 Quest 也能正确序列化
    #[tokio::test]
    async fn test_save_quest_with_multimodal_intent_context() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        // 构造带多模态输入描述的 Quest(模拟实际场景)
        let intent = UserIntent {
            intent_id: "i-1".into(),
            raw_text: "分析图像。生成报告。".into(),
            multimodal_inputs: vec![MultimodalInput::Text("图像数据".into())],
            risk_level: 50,
        };
        // Quest 本身不存储 intent,但 description 携带文本
        let quest = Quest {
            quest_id: "q-mm".into(),
            title: intent.raw_text.clone(),
            tasks: vec![Task {
                task_id: "task-0".into(),
                description: intent.raw_text.clone(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            }],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
            priority: 128,
        };
        let cp = cm.save(Arc::new(quest.clone())).await.unwrap();
        let loaded = cm.load("q-mm", &cp.checkpoint_id).await.unwrap();
        let restored: Quest = rmp_serde::from_slice(&loaded.serialized_state).unwrap();
        assert_eq!(restored, quest);
    }

    /// 验证 prune 对损坏文件(非法 msgpack)也能正常清理:
    /// 旧实现逐个 load_blocking 取 created_at,损坏文件加载失败被 `if let Ok` 跳过,
    /// 既不参与排序也永不清理;零解码实现按文件名排序,损坏文件正常纳入删除范围。
    /// (行为改进锁定:旧实现下本测试必红——损坏文件被永久保留)
    #[tokio::test]
    async fn prune_works_on_corrupted_files() {
        let tmp = tempdir().unwrap();
        // max_keep=5:3 次保存不触发 save 内部自动 prune,损坏文件由测试手工注入
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let quest = make_quest("q-corrupt", 1);

        let mut ids = Vec::new();
        for _ in 0..3 {
            // 微小延迟确保 UUIDv7 毫秒时间戳不同,文件名字典序严格对应保存顺序
            std::thread::sleep(std::time::Duration::from_millis(5));
            let cp = cm.save(Arc::new(quest.clone())).await.unwrap();
            ids.push(cp.checkpoint_id);
        }

        // 手工写入损坏 .bin:UUIDv7 时间戳前缀全零(1970 年),字典序必然最旧
        let quest_dir = tmp.path().join("q-corrupt");
        let corrupted_name = "cp-00000000-0000-1000-8000-000000000000";
        std::fs::write(
            quest_dir.join(format!("{corrupted_name}.bin")),
            b"\x00garbage-not-msgpack\xff",
        )
        .unwrap();

        // 4 个文件,keep=2 → 删 2 个最旧:损坏文件 + 第 1 个正常检查点
        cm.prune_old("q-corrupt", 2).await.unwrap();

        let remaining = cm.list_checkpoints("q-corrupt").unwrap();
        assert_eq!(remaining.len(), 2, "应保留 2 个检查点");
        assert!(
            !remaining.contains(&corrupted_name.to_string()),
            "损坏文件应被 prune 删除(旧实现会永久保留)"
        );
        assert!(remaining.contains(&ids[1]), "第 2 新的正常检查点应保留");
        assert!(remaining.contains(&ids[2]), "最新的正常检查点应保留");
    }

    /// 验证 prune 按文件名字典序(UUIDv7 时间序)保留最新 keep 个:
    /// 零解码实现的保留集必须与旧实现按 created_at 排序的结果一致(语义不回退),
    /// 且与用户可见的"取最新"路径 load_latest(按 created_at 排序,未改动)一致。
    #[tokio::test]
    async fn prune_keeps_latest_by_filename_order() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let quest = make_quest("q-order", 1);

        let mut ids = Vec::new();
        for _ in 0..5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            let cp = cm.save(Arc::new(quest.clone())).await.unwrap();
            ids.push(cp.checkpoint_id);
        }

        cm.prune_old("q-order", 2).await.unwrap();

        let remaining = cm.list_checkpoints("q-order").unwrap();
        assert_eq!(remaining.len(), 2, "应保留 2 个检查点");
        assert!(remaining.contains(&ids[3]), "第 2 新的应保留");
        assert!(remaining.contains(&ids[4]), "最新的应保留");

        // load_latest 按 created_at 排序,应与 prune 保留的最新检查点一致
        let latest = cm.load_latest("q-order").await.unwrap().unwrap();
        assert_eq!(
            latest.checkpoint_id, ids[4],
            "load_latest 应返回最新保存的检查点"
        );
    }

    // ============================================================
    // 卫星态接线工作包:save→restore 往返 / 旧 checkpoint 兼容 / 降级语义
    // ============================================================

    /// 卫星态 save→restore 往返(需求 5):Quest + 卫星态逐字段还原。
    /// sidecar 与主检查点同 id 配对落盘;from_bytes_with 注入重建后
    /// 树(节点/深度/best/路径)与地图(节点/边/摘要/状态)逐字段一致。
    #[tokio::test]
    async fn test_save_with_satellite_roundtrip() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let quest = make_quest("q-sat", 3);

        // 源组件态:树(root + 2 层扩展)+ 地图(create_map + 2 步)
        let mut tree = SearchTreeManager::new(10);
        let root_id = tree.create_root("q-sat");
        let n1 = tree
            .expand_node(&root_id, sat_card("n1", Some(root_id.as_str()), 0.6))
            .unwrap();
        tree.expand_node(&n1, sat_card("n2", Some(n1.as_str()), 0.9))
            .unwrap();
        let mut map = LongTaskMap::default();
        let map_ref = map.create_map(&quest);
        map.record_step(&map_ref, &sat_step("步骤一", true));
        map.record_step(&map_ref, &sat_step("步骤二", false));

        let satellite = SatelliteState {
            tree: tree.to_bytes().unwrap(),
            map: map.to_bytes().unwrap(),
        };
        let checkpoint = cm
            .save_with_satellite(Arc::new(quest.clone()), Some(&satellite))
            .await
            .unwrap();

        // sidecar 与主检查点同 id 配对落盘(.sat 扩展名,不被 list 枚举)
        let sat_path = tmp
            .path()
            .join("q-sat")
            .join(format!("{}.sat", checkpoint.checkpoint_id));
        assert!(sat_path.exists(), "sidecar .sat 必须与主检查点同 id 落盘");
        assert!(
            !cm.list_checkpoints("q-sat").unwrap().iter().any(|id| id.contains(".sat")),
            "list_checkpoints 不得把 sidecar 当作检查点主文件枚举"
        );

        let (cp, _registry_copy, restored_quest, restored_satellite) =
            cm.load_latest_full("q-sat").await.unwrap().unwrap();
        assert_eq!(cp.checkpoint_id, checkpoint.checkpoint_id);
        // Quest 本体逐字段
        assert_eq!(restored_quest.quest_id, "q-sat");
        assert_eq!(restored_quest.tasks.len(), 3);
        assert_eq!(restored_quest.title, quest.title);
        assert_eq!(restored_quest.thinking_mode, quest.thinking_mode);

        // 卫星态逐字段还原(from_bytes_with 注入重建)
        let satellite = restored_satellite.expect("卫星态必须存在");
        let restored_tree = SearchTreeManager::from_bytes_with(
            &satellite.tree,
            Box::new(NoopMemorySyncHook),
        )
        .unwrap();
        let restored_map = LongTaskMap::from_bytes_with(
            &satellite.map,
            Box::new(crate::long_task_map::InMemoryExternalStorage::default()),
            Box::new(NoopMemorySyncHook),
        )
        .unwrap();
        // 树:节点数/深度/best/路径链
        assert_eq!(restored_tree.get_stats().total_nodes, 3);
        assert_eq!(restored_tree.get_stats().best_score, 0.9);
        assert_eq!(restored_tree.node_depth("n2"), Some(2));
        assert_eq!(restored_tree.get_best_path().len(), 3);
        // 地图:节点/边/摘要/状态
        assert_eq!(restored_map.node_count(), 3);
        assert_eq!(restored_map.edge_count(), 2);
        assert_eq!(
            restored_map.get_node("node_1").unwrap().state_summary,
            "步骤一"
        );
        assert_eq!(
            restored_map.get_node("node_2").unwrap().status,
            NodeStatus::Failed
        );
    }

    /// 旧 checkpoint(无卫星态)兼容(需求 5):load_latest_full 降级为
    /// satellite=None 不报错,Quest 本体照常解码(需求 3 语义)。
    #[tokio::test]
    async fn test_legacy_checkpoint_without_satellite_degrades() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let quest = make_quest("q-legacy", 2);

        // 旧路径 save(无卫星态):历史行为,不写 sidecar
        cm.save(Arc::new(quest)).await.unwrap();

        let (_cp, _registry_copy, restored_quest, satellite) =
            cm.load_latest_full("q-legacy").await.unwrap().unwrap();
        assert!(
            satellite.is_none(),
            "旧 checkpoint 无 sidecar,卫星态必须降级为 None"
        );
        assert_eq!(restored_quest.quest_id, "q-legacy");
        assert_eq!(restored_quest.tasks.len(), 2);
    }

    /// sidecar 损坏降级(需求 3 语义):主检查点完好(过 SHA-256 校验),
    /// 卫星态解码失败 → None,不阻断恢复。
    #[tokio::test]
    async fn test_satellite_corrupt_degrades_to_none() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let quest = make_quest("q-corrupt-sat", 2);

        let mut tree = SearchTreeManager::new(4);
        tree.create_root("q-corrupt-sat");
        let satellite = SatelliteState {
            tree: tree.to_bytes().unwrap(),
            map: LongTaskMap::default().to_bytes().unwrap(),
        };
        let checkpoint = cm
            .save_with_satellite(Arc::new(quest), Some(&satellite))
            .await
            .unwrap();

        // 手工写坏 sidecar(非法 msgpack)
        let sat_path = tmp
            .path()
            .join("q-corrupt-sat")
            .join(format!("{}.sat", checkpoint.checkpoint_id));
        std::fs::write(&sat_path, b"\x00garbage-not-msgpack\xff").unwrap();

        let (_cp, _registry_copy, restored_quest, satellite) =
            cm.load_latest_full("q-corrupt-sat")
                .await
                .unwrap()
                .unwrap();
        assert!(satellite.is_none(), "损坏 sidecar 必须降级为 None");
        assert_eq!(restored_quest.quest_id, "q-corrupt-sat");
    }

    /// prune 连带清理 sidecar(防孤儿 .sat 磁盘泄漏):被删检查点的
    /// sidecar 一并删除,保留检查点的 sidecar 完好。
    #[tokio::test]
    async fn test_prune_removes_sidecar_with_checkpoint() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::with_max_keep(tmp.path().to_path_buf(), 2);
        let quest = make_quest("q-prune", 1);

        let mut tree = SearchTreeManager::new(4);
        tree.create_root("q-prune");
        let satellite = SatelliteState {
            tree: tree.to_bytes().unwrap(),
            map: LongTaskMap::default().to_bytes().unwrap(),
        };

        let mut oldest_id = String::new();
        for i in 0..3 {
            // 微小延迟确保 UUIDv7 毫秒时间戳不同(文件名字典序对应保存顺序)
            std::thread::sleep(std::time::Duration::from_millis(5));
            let cp = cm
                .save_with_satellite(Arc::new(quest.clone()), Some(&satellite))
                .await
                .unwrap();
            if i == 0 {
                oldest_id = cp.checkpoint_id;
            }
        }

        let quest_dir = tmp.path().join("q-prune");
        // 最旧检查点及其 sidecar 一并删除
        assert!(!quest_dir.join(format!("{oldest_id}.bin")).exists());
        assert!(
            !quest_dir.join(format!("{oldest_id}.sat")).exists(),
            "被删检查点的 sidecar 必须连带清理(防孤儿文件)"
        );
        // 保留的 2 个检查点 sidecar 完好
        let remaining = cm.list_checkpoints("q-prune").unwrap();
        assert_eq!(remaining.len(), 2);
        for id in &remaining {
            assert!(quest_dir.join(format!("{id}.sat")).exists());
        }
    }
}
