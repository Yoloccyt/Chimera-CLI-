//! WI-C2 — NMC 资产管线测试（ADR-196 D4 前置四件事之 (1)(2)(3) 的管线面）
//!
//! # 覆盖（三档）
//! - **档 0（无模型）**：恒 fallback 现行为锁定（CI 恒跑）——image/video/audio
//!   `EncodingFailed`;text 字节频率降级（256 维）;
//! - **档 1（单模型目录）**：model_dir 指向含三默认模型文件的目录 → `validate()` 通过、
//!   感知器构造不 panic、假内容加载失败后**优雅回退**（ADR-196 D4 第 2 条:"加载失败
//!   降级保持 backend=None,不得把无模型变成运行期错误"）;
//! - **档 2（批量档）**：四模型全配置（text 显式 opt-in + 目录）→ 管线全链无 panic、
//!   回退语义保持（真实推理档待资产定档,见 manifest 头注;owner 决策项）。
//!
//! # manifest ↔ ModelType 映射锁
//! `assets/nmc/manifest.toml` 每条的 (model_type, file) 必须与**代码权威**一致:
//! Image/Video/Audio = `NmcConfig::default()` 对应字段;Text = 加载器字面量
//! （`perceptors/onnx_backend.rs` get_model_filename 的 Text 臂）。任一侧改名即红。
//!
//! # 伴生制品
//! `scripts/fetch_nmc_assets.ps1` + `.sh`（parity 对;dry-run 默认;纯 ASCII——G-85/
//! PS5.1 纪律）;模型文件不入库（.gitignore: assets/nmc/models/）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code

use std::path::PathBuf;

use nmc_encoder::perceptors::{
    AudioPerceptor, ImagePerceptor, Perceptor, TextPerceptor, VideoPerceptor,
};
use nmc_encoder::{NmcConfig, NmcError, PerceptionInput};

/// 资产清单（include_str:编译期钉入,缺失即编译失败）
const MANIFEST: &str = include_str!("../../../assets/nmc/manifest.toml");

#[derive(Debug, Clone, Default, PartialEq)]
struct Row {
    model_type: String,
    file: String,
    url: String,
    sha256: String,
}

/// 极简 [[model]] 行解析（与 scripts/fetch_nmc_assets.* 同方言）
fn parse_manifest(text: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut cur: Option<Row> = None;
    for line in text.lines().map(str::trim) {
        if line == "[[model]]" {
            if let Some(r) = cur.take() {
                rows.push(r);
            }
            cur = Some(Row::default());
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"');
        if let Some(r) = cur.as_mut() {
            match k.trim() {
                "model_type" => r.model_type = v.into(),
                "file" => r.file = v.into(),
                "url" => r.url = v.into(),
                "sha256" => r.sha256 = v.into(),
                _ => {}
            }
        }
    }
    if let Some(r) = cur {
        rows.push(r);
    }
    rows
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// 建独占临时目录（重跑安全:先清再建）
fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nmc_assets_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 清单结构守卫 + (model_type, file) 映射锁（代码权威逐项对拍）
#[test]
fn manifest_matches_code_authority() {
    let rows = parse_manifest(MANIFEST);
    assert_eq!(rows.len(), 4, "清单必须恰有 4 条（Image/Video/Audio/Text）");

    let cfg = NmcConfig::default();
    let by_type = |t: &str| {
        rows.iter()
            .find(|r| r.model_type == t)
            .unwrap_or_else(|| panic!("缺少 model_type={t} 行"))
            .clone()
    };

    // 代码权威:Image/Video/Audio = NmcConfig 默认文件名;Text = 加载器字面量
    let authority = [
        ("Image", cfg.image_model.as_str()),
        ("Video", cfg.video_model.as_str()),
        ("Audio", cfg.audio_model.as_str()),
        (
            "Text",
            "all-MiniLM-L6-v2.onnx", // onnx_backend.rs get_model_filename Text 臂字面量
        ),
    ];
    for (t, expected_file) in authority {
        let row = by_type(t);
        assert_eq!(
            row.file, expected_file,
            "{t} 行 file 与代码权威失配——改名单侧即越权（manifest↔ModelType 锁）"
        );
        assert!(row.file.ends_with(".onnx"), "{t} file 必须 .onnx 结尾");
        // 定档一致性:url/sha256 同设同空
        assert_eq!(
            row.url.is_empty(),
            row.sha256.is_empty(),
            "{t} url/sha256 必须同设同空（半填充 = 畸形）"
        );
    }

    // 无重复 file/type
    let mut files: Vec<&str> = rows.iter().map(|r| r.file.as_str()).collect();
    files.sort();
    files.dedup();
    assert_eq!(files.len(), 4, "file 不得重复");
}

/// 档 0（无模型档,CI 恒跑）:恒 fallback 现行为锁定
#[test]
fn tier0_no_model_is_fallback_locked() {
    let cfg = NmcConfig::default();
    assert!(cfg.model_dir.is_empty(), "默认未启用 ONNX");

    let img = ImagePerceptor::new(cfg.clone());
    assert!(
        matches!(
            img.perceive(&PerceptionInput::Image(vec![0xFF; 64])),
            Err(NmcError::EncodingFailed { .. })
        ),
        "无模型:图像感知器必须 EncodingFailed（占位回退语义锁定）"
    );

    let vid = VideoPerceptor::new(cfg.clone());
    assert!(matches!(
        vid.perceive(&PerceptionInput::Video(vec![0; 64])),
        Err(NmcError::EncodingFailed { .. })
    ));

    let aud = AudioPerceptor::new(cfg.clone());
    assert!(matches!(
        aud.perceive(&PerceptionInput::Audio(vec![0; 64])),
        Err(NmcError::EncodingFailed { .. })
    ));

    // 文本路:无模型 → 字节频率降级（非错误,256 维）
    let txt = TextPerceptor::new(cfg);
    let elem = txt
        .perceive(&PerceptionInput::Text("fallback tier0".into()))
        .unwrap();
    assert_eq!(
        elem.embedding_dim(),
        256,
        "字节频率降级维度 = text_dim(256)"
    );
}

/// 档 1（单模型目录）:validate 通过 + 构造不 panic + 假内容优雅回退
#[test]
fn tier1_single_dir_load_attempt_degrades_gracefully() {
    let dir = scratch_dir("tier1");
    let cfg = NmcConfig::default().with_model_dir(dir.to_string_lossy().to_string());
    // 三默认模型文件（假内容:仅验证存在性与回退语义,真实推理待资产定档）
    for name in [&cfg.image_model, &cfg.video_model, &cfg.audio_model] {
        std::fs::write(dir.join(name), b"not-a-real-onnx").unwrap();
    }
    assert!(
        cfg.validate().is_ok(),
        "三默认模型文件在位时 validate 必须通过"
    );

    // 构造感知器（内部尝试 load → 假内容失败 → backend=None）不 panic
    let img = ImagePerceptor::new(cfg.clone());
    assert!(
        matches!(
            img.perceive(&PerceptionInput::Image(vec![0xFF; 64])),
            Err(NmcError::EncodingFailed { .. })
        ),
        "假内容加载失败后必须优雅回退（D4 第 2 条:无模型不得变运行期错误）"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 档 2（批量档）:四模型全配置管线全链无 panic、回退语义保持
#[test]
fn tier2_all_four_configured_pipeline_stays_safe() {
    let dir = scratch_dir("tier2");
    let cfg = NmcConfig::default()
        .with_model_dir(dir.to_string_lossy().to_string())
        .with_text_model("all-MiniLM-L6-v2.onnx");
    for name in [
        &cfg.image_model,
        &cfg.video_model,
        &cfg.audio_model,
        "all-MiniLM-L6-v2.onnx",
    ] {
        std::fs::write(dir.join(name), b"not-a-real-onnx").unwrap();
    }
    assert!(cfg.validate().is_ok(), "四模型在位时 validate 必须通过");

    // 四感知器全部构造 + 调用一次,任何路径不得 panic
    let _ = ImagePerceptor::new(cfg.clone());
    let _ = VideoPerceptor::new(cfg.clone());
    let _ = AudioPerceptor::new(cfg.clone());
    let txt = TextPerceptor::new(cfg.clone());
    let elem = txt
        .perceive(&PerceptionInput::Text("tier2 batch".into()))
        .unwrap();
    // tokenizer.json 缺失 → 加载失败 → 字节频率回退（优雅降级,非错误）
    assert_eq!(elem.embedding_dim(), 256);

    let _ = std::fs::remove_dir_all(&dir);
}

/// 伴生制品守卫:脚本对在位 + 纯 ASCII（G-85 免 BOM 前提）+ 模型目录不入库
#[test]
fn companion_artifacts_are_in_place() {
    let root = repo_root();
    for rel in [
        "scripts/fetch_nmc_assets.ps1",
        "scripts/fetch_nmc_assets.sh",
    ] {
        let path = root.join(rel);
        assert!(path.exists(), "伴生脚本缺失: {rel}");
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            bytes.iter().all(|b| *b < 0x80),
            "{rel} 必须纯 ASCII（G-85:非 ASCII 的 .ps1 需 UTF-8 BOM;parity 对双侧同纪律）"
        );
    }
    let gitignore = std::fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(
        gitignore.contains("assets/nmc/models/"),
        "模型文件不入库纪律: .gitignore 必须含 assets/nmc/models/"
    );
}
