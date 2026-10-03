//! Image OCR chain when vision is off.
//!
//! Order: RapidOCR (PP-OCRv4 mobile, `chinese_cht` + `chinese_cht_dict`,
//! no simplified-Chinese conversion), then optional Tesseract, then `ocrs`.
//! Upstream `ocrs` is Latin-only and stays last. Weights are not in git and
//! are not downloaded by this crate or by unit tests.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

const DET_FILE: &str = "ch_PP-OCRv4_det_mobile.onnx";
const REC_FILE: &str = "chinese_cht_PP-OCRv3_rec_mobile.onnx";
const DICT_FILE: &str = "chinese_cht_dict.txt";
const OCR_VERSION: &str = "PP-OCRv4";
const REC_LANG: &str = "chinese_cht";
const MODEL_TYPE: &str = "mobile";

const RAPID_TIMEOUT: Duration = Duration::from_secs(20);
const TESSERACT_TIMEOUT: Duration = Duration::from_secs(15);

const RAPIDOCR_RUNNER: &str = r#"
import json, sys
from rapidocr import RapidOCR
params = json.loads(sys.argv[1])
image = sys.argv[2]
engine = RapidOCR(params=params)
result = engine(image)
lines = []
txts = getattr(result, "txts", None) if result is not None else None
if txts:
    lines.extend(t for t in txts if t)
elif isinstance(result, (list, tuple)) and result and isinstance(result[0], (list, tuple)):
    for row in result[0]:
        if isinstance(row, (list, tuple)) and len(row) > 1 and isinstance(row[1], str) and row[1]:
            lines.append(row[1])
sys.stdout.write("\n".join(lines))
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ImageRead {
    Vision,
    OcrChain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OcrStage {
    RapidOcr,
    Tesseract,
    Ocrs,
}

/// Vision, when the model supports it, is the whole image. OCR does not run.
pub(super) fn image_read(supports_vision: bool) -> ImageRead {
    if supports_vision {
        ImageRead::Vision
    } else {
        ImageRead::OcrChain
    }
}

/// RapidOCR, then optional Tesseract, then `ocrs`. Never the reverse.
pub(super) fn ocr_stage_order() -> [OcrStage; 3] {
    [OcrStage::RapidOcr, OcrStage::Tesseract, OcrStage::Ocrs]
}

/// Identity. Do not map traditional characters to simplified Chinese.
pub(super) fn keep_traditional(text: &str) -> String {
    text.to_string()
}

pub(super) fn accepted_traditional(text: &str) -> Option<String> {
    let kept = keep_traditional(text);
    if kept.trim().is_empty() {
        None
    } else {
        Some(kept)
    }
}

pub(super) fn rapidocr_model_dir(model_dir: &Path) -> PathBuf {
    model_dir.join("rapidocr")
}

pub(super) fn rapidocr_skip_reason(model_dir: &Path) -> Option<&'static str> {
    let dir = rapidocr_model_dir(model_dir);
    let ready = [DET_FILE, REC_FILE, DICT_FILE]
        .iter()
        .all(|name| dir.join(name).is_file());
    if ready {
        None
    } else {
        Some("pinned PP-OCRv4 chinese_cht files are not on disk; RapidOCR is not fetched")
    }
}

/// Params passed to RapidOCR so it cannot fall back to its PP-OCRv6 default.
pub(super) fn rapidocr_engine_params(
    det: &Path,
    rec: &Path,
    dict: &Path,
) -> Vec<(&'static str, String)> {
    vec![
        ("Det.engine_type", "onnxruntime".to_string()),
        ("Det.lang_type", "ch".to_string()),
        ("Det.model_type", MODEL_TYPE.to_string()),
        ("Det.ocr_version", OCR_VERSION.to_string()),
        ("Det.model_path", det.display().to_string()),
        ("Rec.engine_type", "onnxruntime".to_string()),
        ("Rec.lang_type", REC_LANG.to_string()),
        ("Rec.model_type", MODEL_TYPE.to_string()),
        ("Rec.ocr_version", OCR_VERSION.to_string()),
        ("Rec.model_path", rec.display().to_string()),
        ("Rec.rec_keys_path", dict.display().to_string()),
    ]
}

fn rapidocr_params_json(det: &Path, rec: &Path, dict: &Path) -> String {
    let mut map = serde_json::Map::new();
    for (key, value) in rapidocr_engine_params(det, rec, dict) {
        map.insert(key.to_string(), serde_json::Value::String(value));
    }
    serde_json::Value::Object(map).to_string()
}

pub(super) async fn try_rapidocr(path: &Path, model_dir: &Path) -> Option<String> {
    if let Some(reason) = rapidocr_skip_reason(model_dir) {
        tracing::debug!("{reason}");
        return None;
    }
    let dir = rapidocr_model_dir(model_dir);
    let params = rapidocr_params_json(
        &dir.join(DET_FILE),
        &dir.join(REC_FILE),
        &dir.join(DICT_FILE),
    );
    let mut cmd = tokio::process::Command::new("python3");
    cmd.arg("-c")
        .arg(RAPIDOCR_RUNNER)
        .arg(params)
        .arg(path)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = cmd.spawn().ok()?;
    let output = match tokio::time::timeout(RAPID_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            tracing::warn!("RapidOCR failed to run: {e}");
            return None;
        }
        Err(_) => {
            tracing::warn!("RapidOCR timed out; falling through");
            return None;
        }
    };
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        tracing::warn!("RapidOCR exited {}: {}", output.status, err.trim());
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

pub(super) async fn try_tesseract(path: &Path) -> Option<String> {
    let mut cmd = tokio::process::Command::new("tesseract");
    cmd.arg(path)
        .arg("stdout")
        .arg("-l")
        .arg("chi_tra+eng")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(_) => return None,
    };
    let output = match tokio::time::timeout(TESSERACT_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        _ => return None,
    };
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

pub(super) async fn ocr_with_ocrs(path: &Path, model_dir: &Path) -> Result<String> {
    super::ensure_ocr_models(model_dir).await?;

    let det_path = model_dir.join("text-detection.rten");
    let rec_path = model_dir.join("text-recognition.rten");
    let path_owned = path.to_path_buf();

    tokio::task::spawn_blocking(move || -> Result<String> {
        let detection_model =
            rten::Model::load_file(&det_path).context("Failed to load OCR detection model")?;
        let recognition_model =
            rten::Model::load_file(&rec_path).context("Failed to load OCR recognition model")?;

        let engine = ocrs::OcrEngine::new(ocrs::OcrEngineParams {
            detection_model: Some(detection_model),
            recognition_model: Some(recognition_model),
            ..Default::default()
        })?;

        let img = image::open(&path_owned)
            .context("Failed to open image for OCR")?
            .into_rgb8();
        let img_source = ocrs::ImageSource::from_bytes(img.as_raw(), img.dimensions())?;
        let ocr_input = engine.prepare_input(img_source)?;
        let text = engine.get_text(&ocr_input)?;
        Ok(text)
    })
    .await
    .context("OCR task panicked")?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// QA fixture. The fourth character is 喺 (U+55BA), not 咁, 嘉, or a second 嘅.
    const QA_FIXTURE: &str = "嘅係唔喺咗";
    const FETCH_SCRIPT: &str = include_str!("../../scripts/fetch-rapidocr-ppocrv4-chinese-cht.sh");

    enum StageText {
        Hit(String),
        Miss,
    }

    /// Same rule as `ocr_image`: first non-empty traditional text wins.
    fn first_traditional_hit(mut run: impl FnMut(OcrStage) -> StageText) -> Option<String> {
        for stage in ocr_stage_order() {
            if let StageText::Hit(text) = run(stage) {
                if let Some(kept) = accepted_traditional(&text) {
                    return Some(kept);
                }
            }
        }
        None
    }

    #[test]
    fn fixture_is_the_five_traditional_characters() {
        let chars: Vec<char> = QA_FIXTURE.chars().collect();
        assert_eq!(chars, ['嘅', '係', '唔', '喺', '咗']);
        assert_eq!(chars[3] as u32, 0x55BA);
        assert_ne!(chars[3], '咁');
        assert_ne!(chars[3], '嘉');
        assert_ne!(chars[0], chars[3]);
    }

    #[test]
    fn keep_traditional_does_not_simplify_the_fixture() {
        let kept = keep_traditional(QA_FIXTURE);
        assert_eq!(kept, QA_FIXTURE);
        for simplified in ['的', '是', '不', '在', '了'] {
            assert!(!kept.contains(simplified));
        }
        assert_eq!(
            accepted_traditional(QA_FIXTURE).as_deref(),
            Some(QA_FIXTURE)
        );
        assert!(accepted_traditional("  \n").is_none());
    }

    #[test]
    fn vision_short_circuits_before_the_ocr_chain() {
        assert_eq!(image_read(true), ImageRead::Vision);
        assert_eq!(image_read(false), ImageRead::OcrChain);
    }

    #[test]
    fn chain_order_is_rapidocr_then_tesseract_then_ocrs() {
        assert_eq!(
            ocr_stage_order(),
            [OcrStage::RapidOcr, OcrStage::Tesseract, OcrStage::Ocrs]
        );
    }

    #[test]
    fn rapidocr_hit_keeps_traditional_and_skips_later_stages() {
        let mut calls = Vec::new();
        let text = first_traditional_hit(|stage| {
            calls.push(stage);
            match stage {
                OcrStage::RapidOcr => StageText::Hit(QA_FIXTURE.to_string()),
                _ => panic!("later stage must not run"),
            }
        })
        .expect("fixture text");
        assert_eq!(calls, vec![OcrStage::RapidOcr]);
        assert_eq!(text, QA_FIXTURE);
    }

    #[test]
    fn empty_or_failed_stages_fall_through_and_ocrs_is_last() {
        let mut calls = Vec::new();
        let text = first_traditional_hit(|stage| {
            calls.push(stage);
            match stage {
                OcrStage::RapidOcr => StageText::Miss,
                OcrStage::Tesseract => StageText::Hit("   ".to_string()),
                OcrStage::Ocrs => StageText::Hit("latin only".to_string()),
            }
        })
        .expect("ocrs text");
        assert_eq!(
            calls,
            vec![OcrStage::RapidOcr, OcrStage::Tesseract, OcrStage::Ocrs]
        );
        assert_eq!(text, "latin only");
    }

    #[test]
    fn rapidocr_params_pin_v4_mobile_chinese_cht_and_not_v6() {
        let det = Path::new("/models").join(DET_FILE);
        let rec = Path::new("/models").join(REC_FILE);
        let dict = Path::new("/models").join(DICT_FILE);
        let params = rapidocr_engine_params(&det, &rec, &dict);
        let joined = params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Det.ocr_version=PP-OCRv4"));
        assert!(joined.contains("Rec.ocr_version=PP-OCRv4"));
        assert!(joined.contains("Rec.lang_type=chinese_cht"));
        assert!(joined.contains("Rec.model_type=mobile"));
        assert!(joined.contains("Det.model_type=mobile"));
        assert!(joined.contains(DET_FILE));
        assert!(joined.contains(REC_FILE));
        assert!(joined.contains(DICT_FILE));
        assert!(!joined.contains("PP-OCRv6"));
        assert!(!joined.contains("simplified"));
        assert!(!joined.contains("opencc"));
    }

    #[test]
    fn fetch_script_matches_the_pin_and_is_not_v6() {
        for needle in [
            DET_FILE,
            REC_FILE,
            DICT_FILE,
            "PP-OCRv4",
            "chinese_cht",
            "d2a7720d45a54257208b1e13e36a8479894cb74155a5efe29462512d42f49da9",
            "779656d044ce388045e02ea9244724616194e63928606436cdfc6dc3c9528cc6",
            "832551fee1f2fbc97508772d81ebdc8dba12c00de97a35c71c9ddf43ddac1a83",
        ] {
            assert!(FETCH_SCRIPT.contains(needle), "missing {needle}");
        }
        assert!(!FETCH_SCRIPT.contains("onnx/PP-OCRv6"));
        assert!(!FETCH_SCRIPT.contains("PP-OCRv6_"));
    }

    #[test]
    fn missing_pinned_files_skip_rapidocr_without_fetching() {
        let dir = std::env::temp_dir().join(format!("rustfox-ocr-absent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            rapidocr_skip_reason(&dir),
            Some("pinned PP-OCRv4 chinese_cht files are not on disk; RapidOCR is not fetched")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
