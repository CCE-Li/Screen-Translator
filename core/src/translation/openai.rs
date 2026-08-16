//! OpenAI-compatible HTTP translation provider.
//!
//! POST `{base_url}/chat/completions` with a numbered-marker prompt so a
//! whole batch round-trips in a single HTTP call. The response parser also
//! accepts a plain JSON array of strings.

use std::time::Duration;

use serde_json::{json, Value};

use super::{normalize_text, TranslateError, TranslateRequest, TranslationProvider};
use crate::types::Translation;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OpenAiConfig {
    /// e.g. `https://api.openai.com/v1`
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_retries")]
    pub max_retries: u32,
    /// Maximum texts per HTTP batch.
    #[serde(default = "default_batch")]
    pub batch_size: usize,
}

fn default_timeout() -> u64 {
    30
}
fn default_retries() -> u32 {
    2
}
fn default_batch() -> usize {
    8
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "gpt-4o-mini".into(),
            timeout_secs: default_timeout(),
            max_retries: default_retries(),
            batch_size: default_batch(),
        }
    }
}

pub struct OpenAiProvider {
    config: OpenAiConfig,
}

impl OpenAiProvider {
    pub fn new(config: OpenAiConfig) -> Self {
        Self { config }
    }
}

impl TranslationProvider for OpenAiProvider {
    fn name(&self) -> &'static str {
        "openai-compatible"
    }

    fn translate(
        &self,
        request: &TranslateRequest,
    ) -> Result<Vec<Translation>, TranslateError> {
        if request.texts.is_empty() {
            return Ok(Vec::new());
        }
        if request.texts.len() > self.config.batch_size {
            return Err(TranslateError::BatchTooLarge(format!(
                "{} texts requested but batch_size is {}",
                request.texts.len(),
                self.config.batch_size
            )));
        }

        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );

        let payload = build_payload(request, &self.config.model);
        let mut last_err = None;
        for attempt in 0..=self.config.max_retries {
            match self.post_once(&url, &payload) {
                Ok(content) => {
                    return parse_translation_response(&content, request);
                }
                Err(TranslateError::Transient(e)) => {
                    log::warn!("translation attempt {}/{} transient error: {e}", attempt + 1, self.config.max_retries + 1);
                    last_err = Some(e);
                    if attempt < self.config.max_retries {
                        std::thread::sleep(Duration::from_millis(300 * (1 << attempt)));
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Err(TranslateError::Transient(last_err.unwrap_or_else(|| "unknown".into())))
    }
}

impl OpenAiProvider {
    fn post_once(&self, url: &str, payload: &Value) -> Result<String, TranslateError> {
        let mut req = ureq::post(url)
            .set("Content-Type", "application/json")
            .timeout(Duration::from_secs(self.config.timeout_secs.max(1)));
        if !self.config.api_key.is_empty() {
            req = req.set("Authorization", &format!("Bearer {}", self.config.api_key));
        }
        let resp = req
            .send_json(payload)
            .map_err(|e| classify_http_err(url, e))?;

        let value: Value = resp
            .into_json()
            .map_err(|e| TranslateError::Parse(format!("invalid JSON response: {e}")))?;

        let content = value
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| TranslateError::Parse("response missing choices[0].message.content".into()))?;

        Ok(content.to_string())
    }
}

fn classify_http_err(url: &str, e: ureq::Error) -> TranslateError {
    match e {
        ureq::Error::Status(code, _) => {
            if code == 429 || (500..=599).contains(&code) {
                TranslateError::Transient(format!("HTTP {code} from {url}"))
            } else {
                TranslateError::Config(format!("HTTP {code} from {url}"))
            }
        }
        ureq::Error::Transport(t) => {
            TranslateError::Transient(format!("transport error: {t}"))
        }
    }
}

fn build_payload(request: &TranslateRequest, model: &str) -> Value {
    let src = request
        .source_lang
        .as_deref()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "auto-detect".to_string());
    let system = format!(
        "You are a translation engine. Translate the user's text from {src} to {}. \
         Return ONLY the translation(s), no explanations. If the user sent multiple items \
         marked as [[index]]text, reply with each translation on its own line prefixed \
         with the same [[index]] marker, preserving order.",
        request.target_lang
    );
    let mut user = String::new();
    for (i, t) in request.texts.iter().enumerate() {
        if i > 0 {
            user.push('\n');
        }
        user.push_str(&format!("[[{i}]]{t}"));
    }
    json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ],
        "temperature": 0.1,
        "stream": false
    })
}

/// Parse a provider response into per-request translations, preserving order.
///
/// Accepts two shapes:
///  1. a plain JSON array of strings: `["铁剑", ...]`
///  2. marker lines: `[[0]]铁剑\n[[1]]攻击力 +30`
///
/// Falls back to filling missing items with the source text so a partially
/// formed response never crashes the pipeline.
pub fn parse_translation_response(
    content: &str,
    request: &TranslateRequest,
) -> Result<Vec<Translation>, TranslateError> {
    let n = request.texts.len();
    let mut out: Vec<Option<String>> = vec![None; n];

    let trimmed = content.trim();
    // Attempt JSON array first.
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        if let Some(arr) = v.as_array() {
            let ok = arr.len() == n && arr.iter().all(|x| x.is_string());
            if ok {
                for (i, item) in arr.iter().enumerate() {
                    out[i] = item.as_str().map(|s| s.to_string());
                }
                return Ok(finish(out, request));
            }
        }
    }

    // Marker parse: find "[[n]]" anywhere in the string.
    for (i, text) in request.texts.iter().enumerate() {
        let marker = format!("[[{i}]]");
        if let Some(pos) = trimmed.find(&marker) {
            let rest = &trimmed[pos + marker.len()..];
            let end = rest.find("[[").unwrap_or(rest.len());
            let seg = rest[..end].trim();
            if !seg.is_empty() {
                out[i] = Some(seg.to_string());
            } else {
                out[i] = Some(text.clone());
            }
        }
    }

    // If the model echoed no markers at all and produced one line per item,
    // split by lines as a last resort.
    if out.iter().all(Option::is_none) {
        let lines: Vec<&str> = trimmed.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
        if lines.len() == n {
            for (i, l) in lines.iter().enumerate() {
                out[i] = Some(l.to_string());
            }
        }
    }

    Ok(finish(out, request))
}

fn finish(
    mut out: Vec<Option<String>>,
    request: &TranslateRequest,
) -> Vec<Translation> {
    request
        .texts
        .iter()
        .enumerate()
        .map(|(i, src)| {
            let translated = out[i]
                .take()
                .filter(|t| !normalize_text(t).is_empty())
                .unwrap_or_else(|| src.clone());
            Translation {
                translated_text: translated,
                detected_source_lang: request.source_lang.clone(),
                target_lang: request.target_lang.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(texts: Vec<&str>) -> TranslateRequest {
        TranslateRequest {
            texts: texts.into_iter().map(|s| s.to_string()).collect(),
            source_lang: Some("ja".into()),
            target_lang: "zh-CN".into(),
        }
    }

    #[test]
    fn parses_json_array() {
        let r = req(vec!["Iron Sword", "Attack +30"]);
        let out = parse_translation_response(r#"["铁剑","攻击力 +30"]"#, &r).unwrap();
        assert_eq!(out[0].translated_text, "铁剑");
        assert_eq!(out[1].translated_text, "攻击力 +30");
    }

    #[test]
    fn parses_markers() {
        let r = req(vec!["Iron Sword", "Attack +30"]);
        let out = parse_translation_response("[[0]]铁剑\n[[1]]攻击力 +30", &r).unwrap();
        assert_eq!(out[0].translated_text, "铁剑");
        assert_eq!(out[1].translated_text, "攻击力 +30");
    }

    #[test]
    fn fills_missing_with_source() {
        let r = req(vec!["a", "b", "c"]);
        let out = parse_translation_response("[[0]]A", &r).unwrap();
        assert_eq!(out[0].translated_text, "A");
        assert_eq!(out[1].translated_text, "b");
        assert_eq!(out[2].translated_text, "c");
    }

    #[test]
    fn rejects_too_large_batch_early_in_provider_flow() {
        let cfg = OpenAiConfig {
            base_url: "http://localhost:1/v1".into(),
            api_key: "".into(),
            model: "m".into(),
            timeout_secs: 1,
            max_retries: 0,
            batch_size: 2,
        };
        let p = OpenAiProvider::new(cfg);
        let r = req(vec!["a", "b", "c"]);
        assert!(matches!(p.translate(&r), Err(TranslateError::BatchTooLarge(_))));
    }
}



