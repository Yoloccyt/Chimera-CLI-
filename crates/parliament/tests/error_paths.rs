//! Parliament 错误路径测试(SubTask 37.7)
//!
//! 验证 ParliamentError 5 个变体与 AHIRT 5 个错误路径的触发与处理。
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:AHIRT(Anti-Hack Intelligent Red Team,反黑客红队)

#![forbid(unsafe_code)]

use parliament::{
    AhirtRedTeam, Consensus, Opinion, ParliamentConfig, ProbePayload, ProbePayloadLibrary,
    ProbeType, Proposal, Role, RoleId, RoleProfile, VoteCounter,
};

// ============================================================
// Parliament 错误路径测试(5 个)
// ============================================================

/// 错误路径:RoleNotFound — 查询不存在的角色
///
/// WHY:RoleRegistry::get 对未注册的 RoleId 返回 None,
/// 上层应将其转换为 ParliamentError::RoleNotFound。
/// 此测试验证错误构造与 Display 输出包含 role_id 上下文。
#[test]
fn test_error_role_not_found() {
    let registry = parliament::RoleRegistry::new(&ParliamentConfig::default());

    // 查询不存在的角色 ID
    let missing_id = RoleId::new("role-nonexistent");
    let result = registry.get(&missing_id);

    // 验证返回 None(上层应转换为 RoleNotFound 错误)
    assert!(result.is_none(), "查询不存在的角色应返回 None");

    // 构造 RoleNotFound 错误,验证 Display 包含 role_id
    let err = parliament::ParliamentError::RoleNotFound {
        role_id: "role-nonexistent".into(),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("role not found"),
        "错误消息应包含 'role not found',实际: {msg}"
    );
    assert!(
        msg.contains("role-nonexistent"),
        "错误消息应包含 role_id,实际: {msg}"
    );
}

/// 错误路径:DebateTimeout — 辩论超时
///
/// WHY:5 角色未在 debate_timeout_ms 内全部完成时触发,
/// 携带超时阈值便于上层诊断。此测试验证错误构造与 Display。
#[test]
fn test_error_debate_timeout() {
    let err = parliament::ParliamentError::DebateTimeout { timeout_ms: 5000 };
    let msg = err.to_string();

    assert!(
        msg.contains("debate timed out"),
        "错误消息应包含 'debate timed out',实际: {msg}"
    );
    assert!(
        msg.contains("5000"),
        "错误消息应包含超时阈值 5000,实际: {msg}"
    );

    // 验证超时配置可影响错误:不同 timeout_ms 产生不同消息
    let err_short = parliament::ParliamentError::DebateTimeout { timeout_ms: 100 };
    assert!(err_short.to_string().contains("100"));
}

/// 错误路径:QuorumNotMet — 法定人数不足
///
/// WHY:参与率 < quorum_threshold 时,VoteCounter 返回 Consensus::Rejected,
/// 上层可据此构造 QuorumNotMet 错误。此测试通过低参与率触发该路径。
#[test]
fn test_error_quorum_not_met() {
    let config = ParliamentConfig::default();
    let counter = VoteCounter::new(&config);
    let proposal = Proposal::new("p-quorum", "q-quorum", "法定人数测试", 0.3);

    // 仅 1 个角色参与(总 5 角色),参与率 = 0.2 < quorum_threshold(0.6)
    let opinions = vec![Opinion::new(Role::Architect, 1.0, 0.9, "赞成")];
    let result = counter.count_votes(&opinions, 5, &proposal);

    // 验证参与率不足导致 Rejected
    assert!(
        result.participation_rate < config.quorum_threshold,
        "参与率 {} 应 < quorum_threshold {}",
        result.participation_rate,
        config.quorum_threshold
    );
    match &result.consensus {
        Consensus::Rejected { reason } => {
            assert!(
                reason.contains("quorum"),
                "拒绝原因应包含 'quorum',实际: {reason}"
            );
        }
        other => panic!("法定人数不足应 Rejected,实际: {other:?}"),
    }

    // 构造 QuorumNotMet 错误,验证 Display
    let err = parliament::ParliamentError::QuorumNotMet {
        participation: 0.2,
        required: 0.6,
    };
    let msg = err.to_string();
    assert!(msg.contains("0.2"), "错误消息应包含参与率 0.2,实际: {msg}");
    assert!(msg.contains("0.6"), "错误消息应包含要求 0.6,实际: {msg}");
}

/// 错误路径:VetoFailed — Skeptic 否决失败
///
/// WHY:Skeptic 角色不存在或无否决权时触发。此测试验证错误构造与 Display。
/// 实际场景:动态注销 Skeptic 后尝试否决,或配置错误导致 can_veto=false。
#[test]
fn test_error_veto_failed() {
    // 构造 VetoFailed 错误,模拟 Skeptic 不存在场景
    let err = parliament::ParliamentError::VetoFailed {
        reason: "skeptic role not registered".into(),
    };
    let msg = err.to_string();

    assert!(
        msg.contains("veto failed"),
        "错误消息应包含 'veto failed',实际: {msg}"
    );
    assert!(
        msg.contains("skeptic role not registered"),
        "错误消息应包含失败原因,实际: {msg}"
    );

    // 验证 RoleRegistry 中 Skeptic 默认拥有否决权(对照测试)
    let registry = parliament::RoleRegistry::new(&ParliamentConfig::default());
    let skeptic = registry
        .get_by_role(Role::Skeptic)
        .expect("Skeptic 角色应默认注册");
    assert!(
        skeptic.can_veto,
        "Skeptic 默认应拥有否决权,否则会触发 VetoFailed"
    );
}

/// 错误路径:ConfigError — 无效配置
///
/// WHY:ParliamentConfig::validate 检测到非法配置时返回 ConfigError,
/// 此测试通过多种非法配置触发该错误路径。
#[test]
fn test_error_config_invalid() {
    // 场景 1:权重和不为 1.0
    let bad_config = ParliamentConfig {
        architect_weight: 0.5, // 总和将超过 1.0
        ..Default::default()
    };
    let err = bad_config.validate().expect_err("权重和不为 1.0 应报错");
    let msg = err.to_string();
    assert!(
        msg.contains("weights sum"),
        "错误消息应包含 'weights sum',实际: {msg}"
    );

    // 场景 2:负权重
    let neg_config = ParliamentConfig {
        architect_weight: -0.1,
        ..Default::default()
    };
    let err = neg_config.validate().expect_err("负权重应报错");
    assert!(
        err.to_string().contains("non-negative"),
        "错误消息应包含 'non-negative'"
    );

    // 场景 3:共识阈值越界
    let threshold_config = ParliamentConfig {
        consensus_threshold: 1.5,
        ..Default::default()
    };
    let err = threshold_config.validate().expect_err("共识阈值越界应报错");
    assert!(
        err.to_string().contains("consensus_threshold"),
        "错误消息应包含 'consensus_threshold'"
    );

    // 场景 4:超时为 0
    let zero_timeout_config = ParliamentConfig {
        debate_timeout_ms: 0,
        ..Default::default()
    };
    let err = zero_timeout_config.validate().expect_err("超时为 0 应报错");
    assert!(
        err.to_string().contains("debate_timeout_ms"),
        "错误消息应包含 'debate_timeout_ms'"
    );

    // 场景 5:RoleRegistry::register 拒绝负权重角色
    let registry = parliament::RoleRegistry::new(&ParliamentConfig::default());
    let bad_profile = RoleProfile::new(
        "role-bad",
        Role::Architect,
        "非法角色",
        "model",
        -0.5, // 负权重
        false,
    );
    let err = registry
        .register(bad_profile)
        .expect_err("负权重角色应被拒绝");
    assert!(
        err.to_string().contains("invalid voting_weight"),
        "注册负权重角色应返回 ConfigError,实际: {err}"
    );
}

// ============================================================
// AHIRT 错误路径测试(5 个)
// ============================================================

/// 错误路径:AHIRT 探测类型无效
///
/// WHY:ProbeType 是固定 4 类的 enum,无无效变体。此测试验证:
/// 1. ProbeType::all() 返回恰好 4 类(无多余/缺失)
/// 2. as_str() 对所有变体返回非空字符串
/// 3. 任意 ProbeType 都可正确序列化/反序列化
#[test]
fn test_error_ahirt_probe_type_invalid() {
    // 验证 ProbeType::all() 返回恰好 4 类
    let all_types = ProbeType::all();
    assert_eq!(
        all_types.len(),
        4,
        "ProbeType 应有恰好 4 个变体(无无效类型)"
    );

    // 验证所有变体的 as_str() 返回非空字符串
    for probe_type in &all_types {
        let s = probe_type.as_str();
        assert!(
            !s.is_empty(),
            "ProbeType::{probe_type:?} 的 as_str() 不应为空"
        );
    }

    // 验证枚举不可构造无效变体(编译时保证,此处验证已知变体)
    let types: Vec<ProbeType> = vec![
        ProbeType::PromptInjection,
        ProbeType::CommandInjection,
        ProbeType::PrivilegeEscalation,
        ProbeType::SandboxEscape,
    ];
    assert_eq!(types.len(), 4, "仅 4 类合法 ProbeType");

    // 验证 Display 实现
    assert_eq!(ProbeType::PromptInjection.to_string(), "prompt_injection");
    assert_eq!(ProbeType::CommandInjection.to_string(), "command_injection");
    assert_eq!(
        ProbeType::PrivilegeEscalation.to_string(),
        "privilege_escalation"
    );
    assert_eq!(ProbeType::SandboxEscape.to_string(), "sandbox_escape");
}

/// 错误路径:AHIRT 载荷库为空
///
/// WHY:空载荷库时 get_by_type 返回空切片,probe 返回空 Vec,
/// 上层应处理此边界(避免除零或空迭代)。
#[test]
fn test_error_ahirt_payload_library_empty() {
    // 从空配置构造载荷库
    let empty_library = ProbePayloadLibrary::from_config(vec![]);

    // 验证载荷总数为 0
    assert_eq!(empty_library.count(), 0, "空载荷库的 count() 应为 0");

    // 验证 get_by_type 返回空切片
    for probe_type in &ProbeType::all() {
        let payloads = empty_library.get_by_type(*probe_type);
        assert!(
            payloads.is_empty(),
            "空载荷库的 get_by_type({probe_type:?}) 应返回空切片"
        );
    }

    // 验证 all() 返回空切片
    assert!(empty_library.all().is_empty(), "空载荷库的 all() 应为空");

    // 验证 AHIRT 红队使用空载荷库时 probe 返回空 Vec
    let red_team = AhirtRedTeam::new(empty_library);
    let results = red_team.probe(ProbeType::CommandInjection);
    assert!(
        results.is_empty(),
        "空载荷库的 probe 应返回空 Vec(无探测可执行)"
    );
}

/// 错误路径:AHIRT 探测率计算失败
///
/// WHY:空探测结果列表时,compute_detection_rate 返回 0.0(非 NaN/除零),
/// 此测试验证该边界处理正确。
#[test]
fn test_error_ahirt_detection_rate_calc_failed() {
    let red_team = AhirtRedTeam::default();

    // 场景 1:空列表 → 返回 0.0(非 NaN)
    let empty_rate = red_team.compute_detection_rate(&[]);
    assert!(
        empty_rate.abs() < 1e-6,
        "空列表探测率应为 0.0,实际: {empty_rate}"
    );
    assert!(!empty_rate.is_nan(), "空列表探测率不应为 NaN(除零保护)");
    assert!(!empty_rate.is_infinite(), "空列表探测率不应为 Infinite");

    // 场景 2:全失败列表 → 返回 0.0
    let all_failed: Vec<parliament::ProbeResult> = (0..5)
        .map(|i| parliament::ProbeResult {
            probe_type: ProbeType::CommandInjection,
            payload: format!("payload-{i}"),
            passed: false,
            actual_result: "allowed".to_string(),
            expected_result: "blocked".to_string(),
        })
        .collect();
    let rate = red_team.compute_detection_rate(&all_failed);
    assert!(rate.abs() < 1e-6, "全失败列表探测率应为 0.0,实际: {rate}");

    // 场景 3:验证探测率 ∈ [0, 1](边界保护)
    let mixed: Vec<parliament::ProbeResult> = vec![
        parliament::ProbeResult {
            probe_type: ProbeType::PromptInjection,
            payload: "p1".into(),
            passed: true,
            actual_result: "blocked".into(),
            expected_result: "blocked".into(),
        },
        parliament::ProbeResult {
            probe_type: ProbeType::PromptInjection,
            payload: "p2".into(),
            passed: false,
            actual_result: "allowed".into(),
            expected_result: "blocked".into(),
        },
    ];
    let rate = red_team.compute_detection_rate(&mixed);
    assert!(
        (0.0..=1.0).contains(&rate),
        "混合列表探测率应 ∈ [0,1],实际: {rate}"
    );
    assert!(
        (rate - 0.5).abs() < 1e-6,
        "1 通过 1 失败的探测率应为 0.5,实际: {rate}"
    );
}

/// 错误路径:AHIRT 探测超时(空载荷库模拟)
///
/// WHY:AHIRT 探测本身是同步的(基于 seccore::validate_command),
/// 无显式超时。但空载荷库导致 probe 返回空结果,模拟"无探测完成"场景,
/// 上层应处理此边界(避免误报 100% 探测率)。
#[test]
fn test_error_ahirt_probe_timeout() {
    let empty_library = ProbePayloadLibrary::from_config(vec![]);
    let red_team = AhirtRedTeam::new(empty_library);

    // 模拟"探测超时":probe_all 返回空统计(无探测完成)
    let stats = red_team.probe_all();

    // 验证统计:total=0, passed=0, failed=0
    assert_eq!(stats.total, 0, "空载荷库的 probe_all total 应为 0");
    assert_eq!(stats.passed, 0, "空载荷库的 probe_all passed 应为 0");
    assert_eq!(stats.failed, 0, "空载荷库的 probe_all failed 应为 0");

    // 验证探测率为 0.0(非 NaN,非 1.0 误报)
    assert!(
        stats.detection_rate.abs() < 1e-6,
        "空载荷库的探测率应为 0.0(非误报 1.0),实际: {}",
        stats.detection_rate
    );
    assert!(!stats.detection_rate.is_nan(), "探测率不应为 NaN");

    // 验证 verify_security 在空载荷库时不报告漏洞
    let report = red_team.verify_security();
    assert!(
        report.vulnerable_types.is_empty(),
        "空载荷库不应报告漏洞类型"
    );
    assert!(
        report.remediation_suggestions.is_empty(),
        "空载荷库不应有修复建议"
    );
}

/// 错误路径:AHIRT 配置无效(载荷库与策略校验)
///
/// WHY:AHIRT 依赖 CommandPolicy 与 ProbePayloadLibrary,
/// 此测试验证无效配置(空载荷库 + 默认策略)下的边界行为。
#[test]
fn test_error_ahirt_config_invalid() {
    // 场景 1:空载荷库构造的 AHIRT 红队,probe_all 不应 panic
    let empty_library = ProbePayloadLibrary::from_config(vec![]);
    let red_team = AhirtRedTeam::new(empty_library);

    // 验证 probe_all 在空配置下不 panic(返回空统计)
    let stats = red_team.probe_all();
    assert_eq!(stats.total, 0, "空配置下 probe_all 应返回 total=0");

    // 场景 2:仅含 1 类载荷的库,其他类为空
    let single_type_payloads: Vec<ProbePayload> = vec![
        ProbePayload::new(ProbeType::CommandInjection, "$(cmd)"),
        ProbePayload::new(ProbeType::CommandInjection, "| malicious"),
    ];
    let single_type_library = ProbePayloadLibrary::from_config(single_type_payloads);
    let red_team_single = AhirtRedTeam::new(single_type_library);

    // 验证仅 CommandInjection 有载荷,其他类为空
    assert_eq!(
        red_team_single.probe(ProbeType::CommandInjection).len(),
        2,
        "CommandInjection 应有 2 个载荷"
    );
    assert!(
        red_team_single.probe(ProbeType::PromptInjection).is_empty(),
        "PromptInjection 应为空(未配置)"
    );
    assert!(
        red_team_single
            .probe(ProbeType::PrivilegeEscalation)
            .is_empty(),
        "PrivilegeEscalation 应为空(未配置)"
    );
    assert!(
        red_team_single.probe(ProbeType::SandboxEscape).is_empty(),
        "SandboxEscape 应为空(未配置)"
    );

    // 场景 3:重复 payload 不破坏库(去重由 from_config 处理)
    let duplicate_payloads: Vec<ProbePayload> = vec![
        ProbePayload::new(ProbeType::CommandInjection, "dup"),
        ProbePayload::new(ProbeType::CommandInjection, "dup"),
    ];
    let dup_library = ProbePayloadLibrary::from_config(duplicate_payloads);
    // 验证库不 panic(允许重复,排序后建立索引)
    assert_eq!(dup_library.count(), 2, "重复载荷库应保留所有载荷(不去重)");

    // 场景 4:验证默认 AHIRT 红队的载荷库有 100 个载荷(4 类 × 25)
    let default_red_team = AhirtRedTeam::default();
    let default_stats = default_red_team.probe_all();
    assert_eq!(
        default_stats.total, 100,
        "默认 AHIRT 红队应有 100 个载荷(4 类 × 25)"
    );
}
