//! Windows built-in OCR provider (Windows.Media.Ocr). Zero model download:
//! uses the OCR language packs already installed on the system.

use windows::core::HSTRING;
use windows::Foundation::Rect as WinRect;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;

use screen_translator_core::ocr::{OcrError, OcrProvider};
use screen_translator_core::types::{Frame, Rect, TextRegion};

/// Max image dimension the OCR engine accepts. Larger inputs must be scaled.
const MAX_IMAGE_DIM: u32 = 2600;

pub struct WindowsOcr {
    engine: OcrEngine,
    language_tag: String,
}

impl WindowsOcr {
    /// `desired_lang`: BCP-47 tag, or empty to use user profile languages.
    pub fn create(desired_lang: &str) -> Result<Self, OcrError> {
        // WinRT requires the thread to be COM-initialized. Called at the start
        // of the OCR thread. In-place so the thread stays initialized.
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }

        let engine = if desired_lang.is_empty() {
            match OcrEngine::TryCreateFromUserProfileLanguages() {
                Ok(e) => e,
                Err(e) => {
                    return Err(OcrError::UnsupportedLanguage(format!(
                        "no OCR engine for user profile languages: {e}"
                    )))
                }
            }
        } else {
            let lang = Language::CreateLanguage(&HSTRING::from(desired_lang))
                .map_err(|e| OcrError::UnsupportedLanguage(format!("invalid language tag: {e}")))?;
            match OcrEngine::TryCreateFromLanguage(&lang) {
                Ok(e) => e,
                Err(e) => {
                    return Err(OcrError::UnsupportedLanguage(format!(
                        "{desired_lang}: {e}. Installed OCR languages: {}",
                        available_languages().join(", ")
                    )))
                }
            }
        };

        let language_tag = engine
            .RecognizerLanguage()
            .map(|l| l.LanguageTag().map(|t| t.to_string()).unwrap_or_default())
            .unwrap_or_default();

        Ok(Self {
            engine,
            language_tag,
        })
    }
}

fn available_languages() -> Vec<String> {
    OcrEngine::AvailableRecognizerLanguages()
        .map(|langs| {
            let mut out = Vec::new();
            for l in langs {
                if let Ok(tag) = l.LanguageTag() {
                    out.push(tag.to_string());
                }
            }
            out
        })
        .unwrap_or_default()
}

impl OcrProvider for WindowsOcr {
    fn name(&self) -> &'static str {
        "windows-ocr"
    }

    fn available_languages(&self) -> Vec<String> {
        available_languages()
    }

    fn recognize(&self, frame: &Frame) -> Result<Vec<TextRegion>, OcrError> {
        if frame.width == 0 || frame.height == 0 {
            return Err(OcrError::EmptyFrame);
        }

        // Downscale if the region exceeds the engine's max image dimension.
        let scale = {
            let s = frame.width.max(frame.height) as f32 / MAX_IMAGE_DIM as f32;
            if s > 1.0 {
                1.0 / s
            } else {
                1.0
            }
        };
        let (w, h) = if scale < 1.0 {
            (
                (frame.width as f32 * scale).max(1.0) as u32,
                (frame.height as f32 * scale).max(1.0) as u32,
            )
        } else {
            (frame.width, frame.height)
        };

        let scaled_pixels = if scale < 1.0 {
            downscale(&frame.pixels, frame.width, frame.height, w, h)
        } else {
            frame.pixels.clone()
        };

        let result = recognize_buffer(&self.engine, &scaled_pixels, w, h)?;
        Ok(result
            .into_iter()
            .map(|mut r| {
                if scale < 1.0 {
                    r.bounding_box = Rect::new(
                        r.bounding_box.x / scale,
                        r.bounding_box.y / scale,
                        r.bounding_box.width / scale,
                        r.bounding_box.height / scale,
                    );
                    r.font_size = (r.font_size / scale).max(4.0);
                }
                r.language = Some(self.language_tag.clone());
                r
            })
            .collect())
    }
}

/// Run OCR on a BGRA8 buffer and return text regions with image-local coords.
fn recognize_buffer(
    engine: &OcrEngine,
    pixels: &[u8],
    width: u32,
    height: u32,
) -> Result<Vec<TextRegion>, OcrError> {
    let writer = DataWriter::new()
        .map_err(|e| OcrError::Provider(format!("DataWriter::new: {e}")))?;
    writer
        .WriteBytes(pixels)
        .map_err(|e| OcrError::Provider(format!("DataWriter::WriteBytes: {e}")))?;
    let buffer = writer
        .DetachBuffer()
        .map_err(|e| OcrError::Provider(format!("DataWriter::DetachBuffer: {e}")))?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        width as i32,
        height as i32,
    )
    .map_err(|e| OcrError::Provider(format!("SoftwareBitmap::CreateCopyFromBuffer: {e}")))?;

    let op = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| OcrError::Provider(format!("RecognizeAsync: {e}")))?;
    let result = op
        .get()
        .map_err(|e| OcrError::Provider(format!("RecognizeAsync.get: {e}")))?;

    let mut regions = Vec::new();
    let lines = result
        .Lines()
        .map_err(|e| OcrError::Provider(format!("OcrResult.Lines: {e}")))?;
    for line in lines {
        let words = line
            .Words()
            .map_err(|e| OcrError::Provider(format!("OcrLine.Words: {e}")))?;
        let mut line_text = String::new();
        let mut bounds: Option<WinRect> = None;
        for word in words {
            let text = word
                .Text()
                .map_err(|e| OcrError::Provider(format!("OcrWord.Text: {e}")))?
                .to_string();
            let rect = word
                .BoundingRect()
                .map_err(|e| OcrError::Provider(format!("OcrWord.BoundingRect: {e}")))?;
            if !line_text.is_empty() {
                line_text.push(' ');
            }
            line_text.push_str(&text);
            bounds = Some(match bounds {
                Some(b) => WinRect {
                    X: b.X.min(rect.X),
                    Y: b.Y.min(rect.Y),
                    Width: (b.X + b.Width).max(rect.X + rect.Width) - b.X.min(rect.X),
                    Height: (b.Y + b.Height).max(rect.Y + rect.Height) - b.Y.min(rect.Y),
                },
                None => rect,
            });
        }
        if let Some(b) = bounds {
            let text = line_text.trim().to_string();
            if !text.is_empty() {
                regions.push(TextRegion {
                    text,
                    confidence: 1.0, // WinRT OCR does not expose confidence
                    bounding_box: Rect::new(b.X, b.Y, b.Width, b.Height),
                    language: None,
                    font_size: b.Height.max(8.0),
                });
            }
        }
    }
    if !regions.is_empty() {
        let sample: Vec<String> = regions.iter().take(5).map(|r| r.text.clone()).collect();
        log::debug!("windows-ocr recognized {} line(s): {sample:?}", regions.len());
    }
    Ok(regions)
}

/// Simple box-filter downscale of a BGRA8 image.
fn downscale(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    if dw >= sw && dh >= sh {
        return src.to_vec();
    }
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    for dy in 0..dh {
        let sy0 = (dy as u64 * sh as u64 / dh as u64) as u32;
        let sy1 = (((dy + 1) as u64 * sh as u64 / dh as u64) as u32).min(sh);
        for dx in 0..dw {
            let sx0 = (dx as u64 * sw as u64 / dw as u64) as u32;
            let sx1 = (((dx + 1) as u64 * sw as u64 / dw as u64) as u32).min(sw);
            let mut r = 0u32;
            let mut g = 0u32;
            let mut b = 0u32;
            let mut n = 0u32;
            for sy in sy0..sy1 {
                let row = &src[(sy as usize) * (sw as usize) * 4..];
                for sx in sx0..sx1 {
                    let i = sx as usize * 4;
                    b += row[i] as u32;
                    g += row[i + 1] as u32;
                    r += row[i + 2] as u32;
                    n += 1;
                }
            }
            let o = (dy as usize * dw as usize + dx as usize) * 4;
            out[o] = (b / n.max(1)) as u8;
            out[o + 1] = (g / n.max(1)) as u8;
            out[o + 2] = (r / n.max(1)) as u8;
            out[o + 3] = 255;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downscale_preserves_dimensions() {
        let src = vec![0u8; (400 * 300 * 4) as usize];
        let out = downscale(&src, 400, 300, 100, 75);
        assert_eq!(out.len(), 100 * 75 * 4);
        // Alpha is set to 255; BGR must stay 0.
        for (i, &v) in out.iter().enumerate() {
            if i % 4 != 3 {
                assert_eq!(v, 0, "byte {i}");
            } else {
                assert_eq!(v, 255, "alpha at {i}");
            }
        }
    }

    #[test]
    fn downscale_is_identity_when_smaller() {
        let src = vec![7u8; (40 * 30 * 4) as usize];
        let out = downscale(&src, 40, 30, 40, 30);
        assert_eq!(out, src);
    }
}
