//! Offline provider. For now it echoes the source text (no real local model),
//! which is useful to verify the whole pipeline (capture → OCR → overlay)
//! without network, and as the foundation for a real local engine later.

use super::{normalize_text, TranslateRequest, TranslationProvider};
use crate::types::Translation;

#[derive(Default)]
pub struct LocalProvider {
    pub prefix: &'static str,
}

impl TranslationProvider for LocalProvider {
    fn name(&self) -> &'static str {
        "local"
    }

    fn translate(
        &self,
        request: &TranslateRequest,
    ) -> Result<Vec<Translation>, crate::translation::TranslateError> {
        Ok(request
            .texts
            .iter()
            .map(|t| Translation {
                translated_text: format!("{}{}", self.prefix, normalize_text(t)),
                detected_source_lang: request.source_lang.clone(),
                target_lang: request.target_lang.clone(),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echoes_text() {
        let p = LocalProvider::default();
        let r = TranslateRequest {
            texts: vec!["Iron Sword".into(), "Attack +30".into()],
            source_lang: Some("ja".into()),
            target_lang: "zh-CN".into(),
        };
        let out = p.translate(&r).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].translated_text, "Iron Sword");
        assert_eq!(out[1].translated_text, "Attack +30");
    }
}
