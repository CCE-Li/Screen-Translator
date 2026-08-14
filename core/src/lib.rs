//! Screen Translator core.
//!
//! Cross-platform, UI-independent pipeline logic: types, OCR & translation
//! provider abstractions, caches, layout and the per-region live session.

pub mod cache;
pub mod config;
pub mod diff;
pub mod layout;
pub mod ocr;
pub mod session;
pub mod translation;
pub mod types;

pub use session::{RegionSession, SessionStats, TranslationTask, UpdateResult};
pub use types::{
    Frame, OverlayText, Rect, TextAlign, TextRegion, Translation,
};
