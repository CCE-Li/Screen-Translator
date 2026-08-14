pub mod cache;
pub mod local;
pub mod openai;

use crate::types::Translation;

/// A batch translation request.
#[derive(Debug, Clone)]
pub struct TranslateRequest {
    /// Source texts. Order is preserved in the response.
    pub texts: Vec<String>,
    /// BCP-47 / ISO-639 tag; `None` means auto-detect.
    pub source_lang: Option<String>,
    pub target_lang: String,
}

/// Translates one or more texts. Providers must preserve input order in the
/// returned vector and must not mutate inputs.
pub trait TranslationProvider: Send + Sync {
    fn name(&self) -> &'static str;

    fn translate(
        &self,
        request: &TranslateRequest,
    ) -> Result<Vec<Translation>, TranslateError>;
}

#[derive(Debug)]
pub enum TranslateError {
    /// Non-transient config problem (bad URL, auth rejected, 4xx).
    Config(String),
    /// Network / upstream failure that can be retried.
    Transient(String),
    /// Response could not be parsed.
    Parse(String),
    /// The provider rejected the batch (e.g. too many items).
    BatchTooLarge(String),
}

impl std::fmt::Display for TranslateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TranslateError::Config(e) => write!(f, "translation config error: {e}"),
            TranslateError::Transient(e) => write!(f, "translation retryable error: {e}"),
            TranslateError::Parse(e) => write!(f, "translation parse error: {e}"),
            TranslateError::BatchTooLarge(e) => write!(f, "translation batch too large: {e}"),
        }
    }
}

impl std::error::Error for TranslateError {}

/// Normalize a source text for use as a cache key: trim and collapse runs of
/// whitespace, preserving case.
pub fn normalize_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_ws = false;
    for ch in text.trim().chars() {
        if ch.is_whitespace() {
            if !last_ws {
                out.push(' ');
                last_ws = true;
            }
        } else {
            out.push(ch);
            last_ws = false;
        }
    }
    out
}
