//! Layout: how to place translated text into the original text's bounding box
//! while preserving the original layout as closely as possible.

use crate::config::OverlayStyle;
use crate::types::{OverlayText, Rect, TextAlign, TextRegion};

/// Estimate rendered width of `text` at `font_size` px.
///
/// Uses a heuristic: CJK glyphs are roughly square (width == font_size),
/// Latin/digits are about half width, spaces a third. Good enough to decide
/// shrink-to-fit without a real font measurement pass.
pub fn estimate_text_width(text: &str, font_size: f32) -> f32 {
    let mut w = 0.0f32;
    for c in text.chars() {
        if is_cjk(c) {
            w += font_size;
        } else if c == ' ' {
            w += font_size * 0.33;
        } else {
            w += font_size * 0.55;
        }
    }
    w
}

/// True for CJK unified ideographs, kana and hangul.
pub fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x2EFF | 0x3000..=0x303F | 0x3040..=0x30FF | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xAC00..=0xD7AF)
}

/// Estimate the source font size from an OCR region when the OCR provider did
/// not report one.
pub fn estimate_font_size(region: &TextRegion) -> f32 {
    if region.font_size > 0.0 {
        return region.font_size;
    }
    (region.bounding_box.height * 0.85).clamp(8.0, 120.0)
}

/// Produce the overlay instruction for one translated text region.
///
/// The font size is derived from the original text size, then scaled down so
/// the translation fits within the original bounding box width (single line),
/// keeping multi-line wrapping to the renderer.
pub fn layout_overlay(
    region: &TextRegion,
    translated_text: &str,
    style: &OverlayStyle,
    align: TextAlign,
) -> OverlayText {
    let box_ = region.bounding_box;
    let base = estimate_font_size(region);
    let mut font_size = (base * style.font_size_scale).clamp(6.0, style.max_font_size.max(6.0));

    let avail = box_.width.max(8.0);
    let mut needed = estimate_text_width(translated_text, font_size);
    let mut guard = 0;
    while needed > avail && font_size > 6.0 && guard < 24 {
        font_size *= 0.9;
        needed = estimate_text_width(translated_text, font_size);
        guard += 1;
    }

    OverlayText {
        text: translated_text.to_string(),
        box_,
        font_size,
        align,
    }
}

/// Ensure the overlay rect stays inside `bounds` (typically the screen).
pub fn clamp_rect_to_bounds(r: Rect, bounds: Rect) -> Rect {
    let x = r.x.clamp(bounds.x, bounds.x + bounds.width - r.width);
    let y = r.y.clamp(bounds.y, bounds.y + bounds.height - r.height);
    Rect::new(x, y, r.width, r.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> OverlayStyle {
        OverlayStyle {
            opacity: 1.0,
            text_color: "#FFFFFF".into(),
            background_color: "#B0000000".into(),
            corner_radius: 4.0,
            shadow: true,
            font_family: "Arial".into(),
            font_size_scale: 1.0,
            max_font_size: 48.0,
        }
    }

    #[test]
    fn cjk_width_is_square() {
        assert!(is_cjk('中'));
        assert!(is_cjk('鉄'));
        assert!(!is_cjk('a'));
        assert!((estimate_text_width("铁剑", 20.0) - 40.0).abs() < 0.01);
        assert!((estimate_text_width("ab", 20.0) - 22.0).abs() < 0.01);
    }

    #[test]
    fn shrinks_long_translation_to_fit() {
        let mut r = TextRegion {
            text: "abc".into(),
            confidence: 1.0,
            bounding_box: Rect::new(0.0, 0.0, 100.0, 24.0),
            language: Some("en".into()),
            font_size: 20.0,
        };
        let out = layout_overlay(&r, "ABCDEFGHIJKLMNOPQRSTUVWXYZ 你好世界", &style(), TextAlign::Center);
        assert!(out.font_size <= 12.0);
        let w = estimate_text_width(&out.text, out.font_size);
        assert!(w <= 110.0, "font {:.1} width {:.1}", out.font_size, w);
        r.bounding_box = Rect::new(0.0, 0.0, 100.0, 24.0);
        let out2 = layout_overlay(&r, "铁剑", &style(), TextAlign::Center);
        assert!(out2.font_size <= 48.0);
    }
}
