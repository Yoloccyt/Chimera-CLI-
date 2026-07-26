//! 臂定义 — LinUCB 的离散动作空间
//!
//! 对应任务: **P4-W13.1.3**（LinUCB 算法核心）
//! 对应 ADR: **ADR-031**（omega-learner 边界）
//!
//! # 设计动机
//!
//! v5.0 设计文档 §7.3 定义了六接缝,每条接缝的臂空间为有限离散集合
//! (如 S1 接缝 ρ∈{0.5, 2, 5, 10} 四档密度档位)。
//!
//! 本模块提供:
//! - `ArmId`: 臂的稳定标识(用于持久化与跨版本比较)
//! - `ArmIndex`: 臂在 LinUCB 内部数组中的位置索引(u32,适合 ndarray 索引)
//! - `ArmSet` trait: 臂空间抽象(可扩展为连续臂/无穷臂,本期只实现 DiscreteArmSet)
//! - `DiscreteArmSet`: 离散有限臂集(本期唯一实现,覆盖 S1-S6 全部接缝需求)

use serde::{Deserialize, Serialize};

/// 臂索引 — LinUCB 内部数组位置
///
/// WHY 用 u32 而非 usize:
/// - ndarray 索引接受 usize,但 u32 → usize 转换零成本(`as usize`)
/// - u32 范围(4G)远超任何接缝臂数需求(S1-S6 最大 ~10 臂)
/// - 4 字节固定大小,便于 FFI / 序列化 / 跨平台一致性
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArmIndex(u32);

impl ArmIndex {
    /// 创建臂索引
    ///
    /// # 参数
    /// - `idx`: 臂位置索引(必须为有限非负值)
    pub const fn new(idx: u32) -> Self {
        Self(idx)
    }

    /// 返回原始 u32 值
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    /// 转换为 usize(ndarray 索引使用)
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

impl From<u32> for ArmIndex {
    fn from(idx: u32) -> Self {
        Self::new(idx)
    }
}

impl From<usize> for ArmIndex {
    /// 从 usize 创建臂索引
    ///
    /// WHY 允许: 调用方常持 `Vec<ArmConfig>` 用 usize 索引,
    /// 直接转换避免每次 `as u32` 显式 cast。
    fn from(idx: usize) -> Self {
        // usize → u32 在 64 位平台上可能截断,但 LinUCB 臂数永远 < 4G
        // (单接缝最大 ~10 臂,六接缝共 ~60 臂)
        Self::new(idx as u32)
    }
}

/// 臂的稳定标识 — 跨版本持久化与比较
///
/// WHY 用 String 而非 enum:
/// - 不同接缝的臂语义不同(S1 密度档位 / S4 权重向量),枚举会膨胀
/// - 字符串便于 TOML/JSON 持久化与 spec 版本化
/// - 跨版本比较只需字符串相等(版本演进时新增臂不破坏旧 spec)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArmId(String);

impl ArmId {
    /// 创建臂标识
    pub fn new<S: Into<String>>(id: S) -> Self {
        Self(id.into())
    }

    /// 返回字符串引用
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 消费并返回内部 String
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Display for ArmId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for ArmId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ArmId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// 臂空间抽象 trait
///
/// 本期只实现 `DiscreteArmSet`(覆盖 S1-S6 全部接缝),
/// 预留 trait 扩展点支持未来连续臂空间(NeuralUCB 长期选项)。
pub trait ArmSet: std::fmt::Debug + Send + Sync {
    /// 返回臂空间大小(None 表示无穷/连续臂)
    fn size(&self) -> Option<usize>;

    /// 根据 ArmId 查询 ArmIndex(None 表示不存在)
    fn index_of(&self, id: &ArmId) -> Option<ArmIndex>;

    /// 根据 ArmIndex 查询 ArmId(None 表示越界)
    fn id_of(&self, idx: ArmIndex) -> Option<&ArmId>;
}

/// 离散有限臂集 — LinUCB 本期唯一臂空间实现
///
/// # 示例
///
/// ```
/// use omega_learner::arm::{ArmId, ArmIndex, ArmSet, DiscreteArmSet};
///
/// let arm_set = DiscreteArmSet::new(vec![
///     ArmId::new("rho=0.5"),
///     ArmId::new("rho=2"),
///     ArmId::new("rho=5"),
///     ArmId::new("rho=10"),
/// ]);
/// assert_eq!(arm_set.size(), Some(4));
/// assert_eq!(arm_set.index_of(&ArmId::new("rho=2")), Some(ArmIndex::new(1)));
/// assert_eq!(arm_set.id_of(ArmIndex::new(3)), Some(&ArmId::new("rho=10")));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscreteArmSet {
    /// 臂标识列表(索引即 ArmIndex.as_usize)
    arms: Vec<ArmId>,
}

impl DiscreteArmSet {
    /// 创建离散臂集
    ///
    /// # 参数
    /// - `arms`: 臂标识列表(顺序决定 ArmIndex)
    ///
    /// # 校验
    /// - `arms` 不能为空(空臂集无意义,LinUCB 构造会校验)
    /// - `arms` 内元素必须唯一(重复臂 ID 索引歧义)
    pub fn new(arms: Vec<ArmId>) -> Self {
        // 调用方应保证唯一性;若不保证,这里不强制去重(避免 O(n^2) 校验),
        // 而是构造时保留原始顺序,index_of 返回首个匹配(可预测行为)
        Self { arms }
    }

    /// 返回臂数量
    pub fn len(&self) -> usize {
        self.arms.len()
    }

    /// 是否为空臂集
    pub fn is_empty(&self) -> bool {
        self.arms.is_empty()
    }

    /// 返回臂列表引用(用于遍历)
    pub fn arms(&self) -> &[ArmId] {
        &self.arms
    }

    /// 校验臂 ID 唯一性(测试与诊断用)
    ///
    /// WHY 不在 `new` 中校验: 避免构造时 O(n^2) 开销,
    /// 由调用方在需要时显式调用本方法(测试 / spec 加载时)。
    pub fn has_unique_ids(&self) -> bool {
        let mut seen = std::collections::HashSet::new();
        self.arms.iter().all(|id| seen.insert(id.as_str()))
    }
}

impl ArmSet for DiscreteArmSet {
    fn size(&self) -> Option<usize> {
        Some(self.arms.len())
    }

    fn index_of(&self, id: &ArmId) -> Option<ArmIndex> {
        self.arms.iter().position(|a| a == id).map(ArmIndex::from)
    }

    fn id_of(&self, idx: ArmIndex) -> Option<&ArmId> {
        self.arms.get(idx.as_usize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_arm_set() -> DiscreteArmSet {
        DiscreteArmSet::new(vec![
            ArmId::new("rho=0.5"),
            ArmId::new("rho=2"),
            ArmId::new("rho=5"),
            ArmId::new("rho=10"),
        ])
    }

    // ============================================================
    // ArmIndex 测试
    // ============================================================

    #[test]
    fn test_arm_index_new() {
        let idx = ArmIndex::new(3);
        assert_eq!(idx.as_u32(), 3);
        assert_eq!(idx.as_usize(), 3);
    }

    #[test]
    fn test_arm_index_zero() {
        let idx = ArmIndex::new(0);
        assert_eq!(idx.as_u32(), 0);
        assert_eq!(idx.as_usize(), 0);
    }

    #[test]
    fn test_arm_index_from_u32() {
        let idx: ArmIndex = 42u32.into();
        assert_eq!(idx.as_u32(), 42);
    }

    #[test]
    fn test_arm_index_from_usize() {
        let idx: ArmIndex = 7usize.into();
        assert_eq!(idx.as_usize(), 7);
    }

    #[test]
    fn test_arm_index_equality() {
        assert_eq!(ArmIndex::new(1), ArmIndex::new(1));
        assert_ne!(ArmIndex::new(1), ArmIndex::new(2));
    }

    #[test]
    fn test_arm_index_copy() {
        let idx = ArmIndex::new(5);
        let copied = idx; // Copy
        assert_eq!(idx, copied);
    }

    #[test]
    fn test_arm_index_serialize_json() {
        let idx = ArmIndex::new(42);
        let json = serde_json::to_string(&idx).unwrap();
        let deserialized: ArmIndex = serde_json::from_str(&json).unwrap();
        assert_eq!(idx, deserialized);
    }

    // ============================================================
    // ArmId 测试
    // ============================================================

    #[test]
    fn test_arm_id_new() {
        let id = ArmId::new("rho=0.5");
        assert_eq!(id.as_str(), "rho=0.5");
    }

    #[test]
    fn test_arm_id_from_string() {
        let id = ArmId::from("density_low".to_string());
        assert_eq!(id.as_str(), "density_low");
    }

    #[test]
    fn test_arm_id_from_str() {
        let id = ArmId::from("density_high");
        assert_eq!(id.as_str(), "density_high");
    }

    #[test]
    fn test_arm_id_into_inner() {
        let id = ArmId::new("test");
        assert_eq!(id.into_inner(), "test");
    }

    #[test]
    fn test_arm_id_display() {
        let id = ArmId::new("rho=5");
        assert_eq!(format!("{}", id), "rho=5");
    }

    #[test]
    fn test_arm_id_equality() {
        assert_eq!(ArmId::new("a"), ArmId::new("a"));
        assert_ne!(ArmId::new("a"), ArmId::new("b"));
    }

    #[test]
    fn test_arm_id_serialize_json() {
        let id = ArmId::new("rho=2");
        let json = serde_json::to_string(&id).unwrap();
        let deserialized: ArmId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, deserialized);
    }

    // ============================================================
    // DiscreteArmSet 测试
    // ============================================================

    #[test]
    fn test_arm_set_new() {
        let arm_set = sample_arm_set();
        assert_eq!(arm_set.len(), 4);
        assert!(!arm_set.is_empty());
    }

    #[test]
    fn test_arm_set_empty() {
        let arm_set = DiscreteArmSet::new(vec![]);
        assert!(arm_set.is_empty());
        assert_eq!(arm_set.len(), 0);
    }

    #[test]
    fn test_arm_set_size() {
        let arm_set = sample_arm_set();
        assert_eq!(arm_set.size(), Some(4));
    }

    #[test]
    fn test_arm_set_index_of() {
        let arm_set = sample_arm_set();
        assert_eq!(
            arm_set.index_of(&ArmId::new("rho=0.5")),
            Some(ArmIndex::new(0))
        );
        assert_eq!(
            arm_set.index_of(&ArmId::new("rho=10")),
            Some(ArmIndex::new(3))
        );
        assert_eq!(arm_set.index_of(&ArmId::new("rho=99")), None);
    }

    #[test]
    fn test_arm_set_id_of() {
        let arm_set = sample_arm_set();
        assert_eq!(
            arm_set.id_of(ArmIndex::new(0)),
            Some(&ArmId::new("rho=0.5"))
        );
        assert_eq!(arm_set.id_of(ArmIndex::new(3)), Some(&ArmId::new("rho=10")));
        assert_eq!(arm_set.id_of(ArmIndex::new(4)), None);
    }

    #[test]
    fn test_arm_set_arms_slice() {
        let arm_set = sample_arm_set();
        let arms = arm_set.arms();
        assert_eq!(arms.len(), 4);
        assert_eq!(arms[0], ArmId::new("rho=0.5"));
        assert_eq!(arms[3], ArmId::new("rho=10"));
    }

    #[test]
    fn test_arm_set_has_unique_ids_true() {
        let arm_set = sample_arm_set();
        assert!(arm_set.has_unique_ids());
    }

    #[test]
    fn test_arm_set_has_unique_ids_false() {
        let arm_set = DiscreteArmSet::new(vec![
            ArmId::new("a"),
            ArmId::new("b"),
            ArmId::new("a"), // 重复
        ]);
        assert!(!arm_set.has_unique_ids());
    }

    #[test]
    fn test_arm_set_index_of_returns_first_match_on_duplicate() {
        // 重复 ID 时返回首个匹配(可预测行为)
        let arm_set = DiscreteArmSet::new(vec![ArmId::new("dup"), ArmId::new("dup")]);
        assert_eq!(arm_set.index_of(&ArmId::new("dup")), Some(ArmIndex::new(0)));
    }

    #[test]
    fn test_arm_set_serialize_json() {
        let arm_set = sample_arm_set();
        let json = serde_json::to_string(&arm_set).unwrap();
        let deserialized: DiscreteArmSet = serde_json::from_str(&json).unwrap();
        assert_eq!(arm_set.size(), deserialized.size());
        assert_eq!(
            deserialized.id_of(ArmIndex::new(0)),
            Some(&ArmId::new("rho=0.5"))
        );
    }
}
