//! OCR provider abstraction. UI and pipeline code must never depend on a
//! concrete OCR implementation; they only talk to `OcrProvider`.

use crate::types::{Frame, TextRegion};

/// A provider capable of recognizing text in a captured frame.
pub trait OcrProvider: Send + Sync {
    /// Human readable name of the provider (for logs / settings).
    fn name(&self) -> &'static str;

    /// BCP-47 tags of languages this provider can recognize.
    fn available_languages(&self) -> Vec<String>;

    /// Recognize all text in the frame.
    ///
    /// Implementations are responsible for their own throttling and for
    /// returning text regions with region-local coordinates.
    fn recognize(&self, frame: &Frame) -> Result<Vec<TextRegion>, OcrError>;
}

#[derive(Debug)]
pub enum OcrError {
    UnsupportedLanguage(String),
    Provider(String),
    EmptyFrame,
}

impl std::fmt::Display for OcrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OcrError::UnsupportedLanguage(l) => {
                write!(f, "no OCR engine available for language ({l})")
            }
            OcrError::Provider(e) => write!(f, "OCR failure: {e}"),
            OcrError::EmptyFrame => write!(f, "frame is empty"),
        }
    }
}

impl std::error::Error for OcrError {}

/// A provider that recognizes nothing. Used as a placeholder when no
/// recognizer can be constructed (e.g. missing OCR language pack) so the
/// pipeline can still start and report the problem instead of crashing.
#[derive(Default)]
pub struct NullOcr;

impl OcrProvider for NullOcr {
    fn name(&self) -> &'static str {
        "null"
    }

    fn available_languages(&self) -> Vec<String> {
        Vec::new()
    }

    fn recognize(&self, _frame: &Frame) -> Result<Vec<TextRegion>, OcrError> {
        Ok(Vec::new())
    }
}
