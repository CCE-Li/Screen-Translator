//! Application configuration. Serialized to JSON so the same file can be
//! shared between the editor and the runtime pipeline.

use std::path::Path;

pub mod defaults {
    pub const OCR_COOLDOWN_MS: u64 = 700;
    pub const OCR_RETRIGGER_MS: u64 = 3000;
    pub const DIFF_THRESHOLD: f32 = 0.01;
    pub const OPACITY: f32 = 0.9;
    pub const TEXT_COLOR: &str = "#FFFFFF";
    pub const BG_COLOR: &str = "#B0000000";
    pub const BG_OPACITY: f32 = 1.0;
    pub const CORNER_RADIUS: f32 = 6.0;
    pub const SHADOW: bool = true;
    pub const FONT_FAMILY: &str = "Microsoft YaHei UI";
    pub const SIZE_SCALE: f32 = 1.0;
    pub const MAX_FONT: f32 = 48.0;
}

use serde::{Deserialize, Serialize};

use crate::translation::openai::OpenAiConfig;
use crate::types::Rect;

/// Selects the translation backend at runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "lowercase")]
pub enum TranslationConfig {
    /// Offline echo provider (no API). Verify the pipeline without network.
    Local,
    /// Any OpenAI-compatible HTTP endpoint.
    OpenAi(OpenAiConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionConfig {
    pub id: String,
    /// Absolute screen coordinates.
    pub rect: Rect,
    /// BCP-47 tag; `None` = auto-detect source language.
    #[serde(default)]
    pub source_lang: Option<String>,
    pub target_lang: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayStyle {
    /// Overall overlay opacity 0.0..=1.0.
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    /// Text color, #RRGGBB or #AARRGGBB.
    #[serde(default = "default_text_color")]
    pub text_color: String,
    /// Background behind translation text, #AARRGGBB.
    #[serde(default = "default_bg_color")]
    pub background_color: String,
    /// Extra opacity multiplier (0.0..=1.0) for the background box. Set close
    /// to 1.0 with a fully-opaque color to cover the original text underneath.
    #[serde(default = "default_bg_opacity")]
    pub background_opacity: f32,
    #[serde(default = "default_radius")]
    pub corner_radius: f32,
    #[serde(default = "default_shadow")]
    pub shadow: bool,
    #[serde(default = "default_font_family")]
    pub font_family: String,
    /// Multiplier applied to the OCR-measured font size.
    #[serde(default = "default_size_scale")]
    pub font_size_scale: f32,
    #[serde(default = "default_max_font")]
    pub max_font_size: f32,
}

fn default_opacity() -> f32 {
    0.9
}
fn default_text_color() -> String {
    "#FFFFFF".into()
}
fn default_bg_color() -> String {
    "#B0000000".into()
}
fn default_bg_opacity() -> f32 {
    1.0
}
fn default_radius() -> f32 {
    6.0
}
fn default_shadow() -> bool {
    true
}
fn default_font_family() -> String {
    "Microsoft YaHei UI".into()
}
fn default_size_scale() -> f32 {
    1.0
}
fn default_max_font() -> f32 {
    48.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureConfig {
    /// Target capture rate (frames per second).
    #[serde(default = "default_capture_fps")]
    pub fps: f32,
    /// Minimum interval between OCR runs, in milliseconds.
    #[serde(default = "default_ocr_cooldown")]
    pub ocr_cooldown_ms: u64,
    /// Force re-OCR at least this often even when the frame is static, in ms.
    #[serde(default = "default_ocr_retrigger")]
    pub ocr_retrigger_ms: u64,
    /// Fraction of sampled cells that must change to trigger OCR (0.0..1.0).
    #[serde(default = "default_diff_threshold")]
    pub diff_threshold: f32,
}

fn default_capture_fps() -> f32 {
    5.0
}
fn default_ocr_cooldown() -> u64 {
    700
}
fn default_ocr_retrigger() -> u64 {
    3000
}
fn default_diff_threshold() -> f32 {
    0.01
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub regions: Vec<RegionConfig>,
    #[serde(default = "default_capture")]
    pub capture: CaptureConfig,
    /// BCP-47 tag for the OCR engine; empty = user profile languages.
    #[serde(default)]
    pub ocr_language: String,
    #[serde(default = "default_translation")]
    pub translation: TranslationConfig,
    #[serde(default = "default_overlay")]
    pub overlay_style: OverlayStyle,
    /// Draw a live performance HUD in the overlay corner.
    #[serde(default = "default_show_stats")]
    pub show_stats: bool,
}

fn default_show_stats() -> bool {
    false
}

fn default_translation() -> TranslationConfig {
    TranslationConfig::Local
}

fn default_capture() -> CaptureConfig {
    CaptureConfig {
        fps: default_capture_fps(),
        ocr_cooldown_ms: default_ocr_cooldown(),
        ocr_retrigger_ms: default_ocr_retrigger(),
        diff_threshold: default_diff_threshold(),
    }
}

fn default_overlay() -> OverlayStyle {
    OverlayStyle {
        opacity: default_opacity(),
        text_color: default_text_color(),
        background_color: default_bg_color(),
        background_opacity: default_bg_opacity(),
        corner_radius: default_radius(),
        shadow: default_shadow(),
        font_family: default_font_family(),
        font_size_scale: default_size_scale(),
        max_font_size: default_max_font(),
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            regions: Vec::new(),
            capture: default_capture(),
            ocr_language: String::new(),
            translation: default_translation(),
            overlay_style: default_overlay(),
            show_stats: default_show_stats(),
        }
    }
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<AppConfig, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let cfg: AppConfig =
            serde_json::from_str(&text).map_err(|e| format!("invalid config JSON: {e}"))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn load_or_default(path: &Path) -> AppConfig {
        match Self::load(path) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("config load failed, using defaults: {e}");
                AppConfig::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| format!("serialize config: {e}"))?;
        std::fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))
    }

    fn validate(&self) -> Result<(), String> {
        for r in &self.regions {
            if r.rect.width <= 0.0 || r.rect.height <= 0.0 {
                return Err(format!("region {} has invalid size", r.id));
            }
        }
        if self.capture.fps <= 0.0 {
            return Err("capture.fps must be > 0".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip() {
        let mut cfg = AppConfig::default();
        cfg.regions.push(RegionConfig {
            id: "r1".into(),
            rect: Rect::new(100.0, 200.0, 400.0, 100.0),
            source_lang: Some("ja".into()),
            target_lang: "zh-CN".into(),
            enabled: true,
        });
        cfg.translation = TranslationConfig::OpenAi(OpenAiConfig {
            base_url: "https://example.com/v1".into(),
            api_key: "k".into(),
            model: "m".into(),
            timeout_secs: 10,
            max_retries: 1,
            batch_size: 4,
        });
        let json = serde_json::to_string(&cfg).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.regions.len(), 1);
        assert_eq!(back.regions[0].source_lang.as_deref(), Some("ja"));
        match back.translation {
            TranslationConfig::OpenAi(c) => assert_eq!(c.model, "m"),
            _ => panic!("expected openai provider"),
        }
        assert_eq!(back.overlay_style.opacity, default_opacity());
    }

    #[test]
    fn defaults_fill_missing_fields() {
        let json = r#"{"regions":[]}"#;
        let cfg: AppConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.regions.is_empty());
        assert_eq!(cfg.capture.fps, default_capture_fps());
    }
}
