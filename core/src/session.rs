//! Per-region live pipeline session. Owns the OCR provider, the translation
//! provider and their caches, and implements the throttling / change-detection
//! policy:
//!
//!   Frame → diff vs last → OCR (throttled) → text set change check →
//!   cache lookup → queue missing translations → overlay draw list.
//!
//! The session is single-threaded (must be owned by one thread). Translations
//! are returned to the caller as a drainable queue and results are applied
//! back through `accept_translations`, which the caller invokes from whatever
//! thread performed the network call.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::cache::LruCache;
use crate::config::RegionConfig;
use crate::diff::{changed_fraction, FrameSignature};
use crate::layout::layout_overlay;
use crate::ocr::{OcrError, OcrProvider};
use crate::translation::cache::TranslationCache;
use crate::translation::{
    normalize_text, TranslationProvider,
};
use crate::types::{
    Frame, OverlayText, TextRegion, TextAlign, Translation,
};

/// A batch of source texts that still need translation.
#[derive(Debug, Clone)]
pub struct TranslationTask {
    /// Id of the region this task belongs to (used to route results back).
    pub region_id: String,
    pub source_lang: Option<String>,
    pub target_lang: String,
    pub texts: Vec<String>,
}

#[derive(Debug)]
pub enum UpdateResult {
    /// Nothing changed; the overlay should keep its current content.
    NoChange,
    /// Re-render the overlay with the given draw list (possibly empty = clear).
    Redraw { overlays: Vec<OverlayText> },
}

#[derive(Debug, Default)]
pub struct SessionStats {
    pub ocr_runs: u64,
    pub frames_seen: u64,
    pub ocr_ms: u64,
    pub ocr_cache_hits: u64,
    pub translation_cache_hits: u64,
    pub translation_cache_misses: u64,
    pub pending: usize,
}

pub struct RegionSession {
    pub id: String,
    ocr: Box<dyn OcrProvider>,
    translator: Box<dyn TranslationProvider>,
    translation_cache: TranslationCache,
    /// Cache of OCR text regions keyed by normalized text, so identical text
    /// does not require re-OCR layout work.
    text_region_cache: LruCache<String, TextRegion>,
    last_signature: Option<FrameSignature>,
    last_regions: Vec<TextRegion>,
    last_ocr_time: Instant,
    last_failed: Option<Instant>,
    pending: HashSet<String>,
    queued: Vec<TranslationTask>,
    pub stats: SessionStats,
}

impl RegionSession {
    pub fn new(
        id: String,
        ocr: Box<dyn OcrProvider>,
        translator: Box<dyn TranslationProvider>,
        cache_capacity: usize,
    ) -> Self {
        Self {
            id,
            ocr,
            translator,
            translation_cache: TranslationCache::new(cache_capacity),
            text_region_cache: LruCache::new(512),
            last_signature: None,
            last_regions: Vec::new(),
            last_ocr_time: Instant::now() - Duration::from_secs(3600),
            last_failed: None,
            pending: HashSet::new(),
            queued: Vec::new(),
            stats: SessionStats::default(),
        }
    }

    pub fn ocr_name(&self) -> &'static str {
        self.ocr.name()
    }

    pub fn translator_name(&self) -> &'static str {
        self.translator.name()
    }

    /// Feed a freshly captured frame. Returns what the overlay should do.
    pub fn process_frame(
        &mut self,
        frame: &Frame,
        cfg: &RegionConfig,
    ) -> UpdateResult {
        self.stats.frames_seen += 1;

        let sig = FrameSignature::compute(frame);
        let changed = match &self.last_signature {
            Some(prev) => changed_fraction(prev, &sig),
            None => 1.0,
        };
        self.last_signature = Some(sig);

        let now = Instant::now();
        let cooldown_ok =
            now.duration_since(self.last_ocr_time) >= Duration::from_millis(cfg_ocr_cooldown(cfg));
        let force = now.duration_since(self.last_ocr_time) >= Duration::from_millis(cfg_ocr_retrigger(cfg));
        let changed_enough = changed > cfg_ocr_threshold(cfg);

        if !(cooldown_ok && (changed_enough || force)) {
            return UpdateResult::NoChange;
        }

        self.last_ocr_time = now;
        let t0 = Instant::now();
        let regions = match self.ocr.recognize(frame) {
            Ok(r) => r,
            Err(OcrError::EmptyFrame) => Vec::new(),
            Err(e) => {
                log::warn!("[{}] OCR failed: {e}", self.id);
                Vec::new()
            }
        };
        self.stats.ocr_ms += t0.elapsed().as_millis() as u64;
        self.stats.ocr_runs += 1;

        let texts: Vec<String> = regions
            .iter()
            .map(|r| normalize_text(&r.text))
            .filter(|t| !t.is_empty())
            .collect();
        let texts_changed = texts != self.texts_of(&self.last_regions);
        self.last_regions = regions.clone();

        // Queue translations for any text we don't already have cached or in flight.
        let mut new_texts = Vec::new();
        for r in &regions {
            let key = normalize_text(&r.text);
            if key.is_empty() {
                continue;
            }
            let src = cfg.source_lang.as_deref().unwrap_or("");
            if self.translation_cache.get(src, &cfg.target_lang, &key).is_some() {
                self.stats.translation_cache_hits += 1;
                continue;
            }
            self.stats.translation_cache_misses += 1;
            if self.pending.contains(&key) {
                continue;
            }
            self.pending.insert(key);
            new_texts.push(r.text.clone());
        }
        if !new_texts.is_empty() {
            self.queued.push(TranslationTask {
                region_id: self.id.clone(),
                source_lang: cfg.source_lang.clone(),
                target_lang: cfg.target_lang.clone(),
                texts: new_texts,
            });
        }
        self.stats.pending = self.pending.len();

        // Cache text regions for later layout without re-OCR.
        for r in &regions {
            let key = normalize_text(&r.text);
            if !key.is_empty() {
                self.text_region_cache.insert(key, r.clone());
            }
        }

        if !texts_changed {
            // Same recognized text: if everything is already drawn, nothing to do.
            return UpdateResult::NoChange;
        }
        UpdateResult::Redraw {
            overlays: self.build_overlays(cfg),
        }
    }

    /// Take pending translation work. The caller chunks and runs it.
    pub fn drain_requests(&mut self) -> Vec<TranslationTask> {
        std::mem::take(&mut self.queued)
    }

    /// Apply translation results produced for `task` (caller must preserve
    /// order). Updates the cache and releases pending markers.
    pub fn accept_translations(&mut self, task: &TranslationTask, results: Vec<Translation>) {
        for (text, t) in task.texts.iter().zip(results) {
            let key = normalize_text(text);
            self.translation_cache.insert(
                task.source_lang.as_deref().unwrap_or(""),
                &task.target_lang,
                &key,
                t.translated_text,
            );
            self.pending.remove(&key);
        }
        self.stats.pending = self.pending.len();
    }

    /// Rebuild the current overlay draw list from cached translations.
    pub fn refresh_overlays(&mut self, cfg: &RegionConfig) -> Vec<OverlayText> {
        self.build_overlays(cfg)
    }

    /// Called when a translation request permanently failed. Releases the
    /// pending markers so a later frame can retry, rate-limited to avoid
    /// hammering the upstream when it is down.
    pub fn note_failure(&mut self, task: &TranslationTask) {
        let now = Instant::now();
        let cooldown_ok = self
            .last_failed
            .map(|t| now.duration_since(t) >= Duration::from_secs(5))
            .unwrap_or(true);
        if cooldown_ok {
            for t in &task.texts {
                self.pending.remove(&normalize_text(t));
            }
            self.last_failed = Some(now);
            self.stats.pending = self.pending.len();
        }
    }

    pub fn translation_cache_len(&self) -> usize {
        self.translation_cache.len()
    }

    pub fn translation_cache_hit_rate(&self) -> f32 {
        self.translation_cache.hit_rate()
    }

    /// Take the counters accumulated since the last call and reset them, so
    /// callers can measure rates over an interval (e.g. OCR runs/sec).
    pub fn take_stats_delta(&mut self) -> SessionStats {
        let d = SessionStats {
            ocr_runs: self.stats.ocr_runs,
            frames_seen: self.stats.frames_seen,
            ocr_ms: self.stats.ocr_ms,
            ocr_cache_hits: self.stats.ocr_cache_hits,
            translation_cache_hits: self.stats.translation_cache_hits,
            translation_cache_misses: self.stats.translation_cache_misses,
            pending: self.stats.pending,
        };
        self.stats.ocr_runs = 0;
        self.stats.frames_seen = 0;
        self.stats.ocr_ms = 0;
        self.stats.ocr_cache_hits = 0;
        self.stats.translation_cache_hits = 0;
        self.stats.translation_cache_misses = 0;
        d
    }

    fn build_overlays(&mut self, cfg: &RegionConfig) -> Vec<OverlayText> {
        let mut out = Vec::new();
        for region in &self.last_regions {
            let key = normalize_text(&region.text);
            if key.is_empty() {
                continue;
            }
            if let Some(translated) = self
                .translation_cache
                .get(cfg.source_lang.as_deref().unwrap_or(""), &cfg.target_lang, &key)
            {
                // Use the region cached for this text to keep layout stable.
                let src_region = match self.text_region_cache.get(&key) {
                    Some(r) => {
                        self.stats.ocr_cache_hits += 1;
                        r.clone()
                    }
                    None => region.clone(),
                };
                out.push(layout_overlay(
                    &src_region,
                    &translated,
                    &cfg_overlay_style(cfg),
                    TextAlign::Center,
                ));
            }
        }
        out
    }

    fn texts_of(&self, regions: &[TextRegion]) -> Vec<String> {
        regions
            .iter()
            .map(|r| normalize_text(&r.text))
            .filter(|t| !t.is_empty())
            .collect()
    }
}

// Small helpers to keep RegionConfig usable standalone (style/rate defaults
// when those fields aren't present in a struct we control). They read from the
// global default style/rates so core stays testable without full AppConfig.

fn cfg_ocr_cooldown(_cfg: &RegionConfig) -> u64 {
    crate::config::defaults::OCR_COOLDOWN_MS
}
fn cfg_ocr_retrigger(_cfg: &RegionConfig) -> u64 {
    crate::config::defaults::OCR_RETRIGGER_MS
}
fn cfg_ocr_threshold(_cfg: &RegionConfig) -> f32 {
    crate::config::defaults::DIFF_THRESHOLD
}
fn cfg_overlay_style(_cfg: &RegionConfig) -> crate::config::OverlayStyle {
    crate::config::OverlayStyle {
        opacity: crate::config::defaults::OPACITY,
        text_color: crate::config::defaults::TEXT_COLOR.to_string(),
        background_color: crate::config::defaults::BG_COLOR.to_string(),
        corner_radius: crate::config::defaults::CORNER_RADIUS,
        shadow: crate::config::defaults::SHADOW,
        font_family: crate::config::defaults::FONT_FAMILY.to_string(),
        font_size_scale: crate::config::defaults::SIZE_SCALE,
        max_font_size: crate::config::defaults::MAX_FONT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RegionConfig;
    use crate::translation::TranslateRequest;
    use crate::types::Rect;

    struct FakeOcr(Vec<String>);
    impl OcrProvider for FakeOcr {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn available_languages(&self) -> Vec<String> {
            vec!["en".into()]
        }
        fn recognize(&self, _f: &Frame) -> Result<Vec<TextRegion>, OcrError> {
            Ok(self
                .0
                .iter()
                .enumerate()
                .map(|(i, t)| TextRegion {
                    text: t.clone(),
                    confidence: 1.0,
                    bounding_box: Rect::new(10.0, 10.0 + i as f32 * 30.0, 200.0, 24.0),
                    language: Some("en".into()),
                    font_size: 18.0,
                })
                .collect())
        }
    }

    struct FakeTranslator;
    impl TranslationProvider for FakeTranslator {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn translate(
            &self,
            r: &TranslateRequest,
        ) -> Result<Vec<Translation>, crate::translation::TranslateError> {
            Ok(r.texts
                .iter()
                .map(|t| Translation {
                    translated_text: format!("{t}→译"),
                    detected_source_lang: r.source_lang.clone(),
                    target_lang: r.target_lang.clone(),
                })
                .collect())
        }
    }

    fn cfg() -> RegionConfig {
        RegionConfig {
            id: "r1".into(),
            rect: Rect::new(0.0, 0.0, 400.0, 300.0),
            source_lang: Some("ja".into()),
            target_lang: "zh-CN".into(),
            enabled: true,
        }
    }

    fn frame(w: u32, h: u32, v: u8) -> Frame {
        Frame {
            width: w,
            height: h,
            pixels: vec![v; (w * h * 4) as usize],
            timestamp: 0.0,
        }
    }

    fn frame_diff_trigger() -> Frame {
        frame(400, 300, 120)
    }

    #[test]
    fn first_frame_triggers_ocr_and_queues_translation() {
        let mut s = RegionSession::new(
            "r1".into(),
            Box::new(FakeOcr(vec!["Iron Sword".into()])),
            Box::new(FakeTranslator),
            100,
        );
        let c = cfg();
        let r = s.process_frame(&frame_diff_trigger(), &c);
        assert!(matches!(r, UpdateResult::Redraw { overlays } if overlays.is_empty()));
        let tasks = s.drain_requests();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].region_id, "r1");
        assert_eq!(tasks[0].texts, vec!["Iron Sword".to_string()]);
    }

    #[test]
    fn identical_static_frame_does_not_reocr() {
        let mut s = RegionSession::new(
            "r1".into(),
            Box::new(FakeOcr(vec!["Iron Sword".into()])),
            Box::new(FakeTranslator),
            100,
        );
        let c = cfg();
        let _ = s.process_frame(&frame_diff_trigger(), &c);
        // Static frame that is byte-identical: no new OCR.
        let r = s.process_frame(&frame_diff_trigger(), &c);
        assert!(matches!(r, UpdateResult::NoChange));
        assert_eq!(s.stats.ocr_runs, 1);
    }

    #[test]
    fn changed_text_after_translation_is_drawn() {
        let mut s = RegionSession::new(
            "r1".into(),
            Box::new(FakeOcr(vec!["Iron Sword".into()])),
            Box::new(FakeTranslator),
            100,
        );
        let c = cfg();
        let _ = s.process_frame(&frame_diff_trigger(), &c);
        let tasks = s.drain_requests();
        let results = s
            .translator
            .translate(&TranslateRequest {
                texts: tasks[0].texts.clone(),
                source_lang: tasks[0].source_lang.clone(),
                target_lang: tasks[0].target_lang.clone(),
            })
            .unwrap();
        s.accept_translations(&tasks[0], results);
        let overlays = s.refresh_overlays(&c);
        assert_eq!(overlays.len(), 1);
        assert_eq!(overlays[0].text, "Iron Sword→译");

        // Cache hit: second OCR of same text should not requeue.
        let r = s.process_frame(&frame_diff_trigger(), &c);
        assert!(matches!(r, UpdateResult::NoChange));
        assert!(s.drain_requests().is_empty());
    }

    #[test]
    fn overlapping_pending_requests_are_deduplicated() {
        let mut s = RegionSession::new(
            "r1".into(),
            Box::new(FakeOcr(vec!["a".into(), "b".into()])),
            Box::new(FakeTranslator),
            100,
        );
        let c = cfg();
        let _ = s.process_frame(&frame_diff_trigger(), &c);
        // Same texts again: nothing new queued.
        let _ = s.process_frame(&frame_diff_trigger(), &c);
        let tasks = s.drain_requests();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].texts.len(), 2);
    }
}

