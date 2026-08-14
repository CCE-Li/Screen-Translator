//! Core cross-platform types shared by the pipeline, OCR providers,
//! translation providers and the overlay renderer.

use serde::{Deserialize, Serialize};

/// A normalized text rectangle in pixels.
///
/// Coordinates are relative to a defined origin (either a capture region's
/// top-left corner, or the full screen) depending on context. All types use
/// `f32` so the same shapes work for pixel-space and DPI-scaled space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.width && py >= self.y && py < self.y + self.height
    }
}

/// One block of recognized text, with its screen/region bounding box.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRegion {
    pub text: String,
    /// 0.0 ..= 1.0 (1.0 when the provider does not expose confidence).
    pub confidence: f32,
    /// Position/size of the recognized text (region-local coordinates).
    pub bounding_box: Rect,
    /// BCP-47 tag of the language used to recognize this text, if known.
    pub language: Option<String>,
    /// Approximate font size (px) of the recognized text, when available.
    pub font_size: f32,
}

/// A captured frame of the region being translated.
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// BGRA8, tightly packed, top-left origin. Length == width*height*4.
    pub pixels: Vec<u8>,
    /// Timestamp (monotonic, seconds).
    pub timestamp: f64,
}

impl Frame {
    pub fn stride(&self) -> usize {
        self.width as usize * 4
    }
}

/// A translation result for a single source text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Translation {
    pub translated_text: String,
    /// ISO-639 language codes, when the provider reports them.
    pub detected_source_lang: Option<String>,
    pub target_lang: String,
}

/// Horizontal alignment used when laying out translated text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

/// A finalized overlay instruction produced by the layout engine.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlayText {
    pub text: String,
    /// Absolute screen-space rectangle the text should be drawn inside.
    pub box_: Rect,
    pub font_size: f32,
    pub align: TextAlign,
}
