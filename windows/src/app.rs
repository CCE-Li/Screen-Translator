//! Application wiring: overlay window UI thread, edit/work modes, the
//! capture → OCR → translate pipeline thread and the translation worker.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{anyhow, Result};
use crossbeam_channel::{Receiver, Sender};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, MOD_ALT, MOD_CONTROL};

use screen_translator_core::config::{AppConfig, RegionConfig, TranslationConfig};
use screen_translator_core::session::{RegionSession, TranslationTask};
use screen_translator_core::translation::openai::OpenAiProvider;
use screen_translator_core::translation::local::LocalProvider;
use screen_translator_core::translation::{TranslateRequest, TranslationProvider};
use screen_translator_core::types::{OverlayText, Rect, Translation};

use crate::capture::DesktopCapture;
use crate::ocr_winrt::WindowsOcr;
use crate::overlay::{
    apply_resize, drag_to_rect, hit_test_resize, DragRect, OverlayWindow, ResizeHandle,
    StatsSnapshot,
};

const HOTKEY_TOGGLE_MODE: i32 = 1;
const HOTKEY_EXIT: i32 = 2;
const TIMER_OVERLAY: usize = 1;
const TIMER_INTERVAL_MS: u32 = 16;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Work,
    Edit,
}

enum OverlayMsg {
    Draw(Vec<OverlayText>),
    Stats(StatsSnapshot),
    Clear,
}

enum PipelineCmd {
    Shutdown,
}

struct TranslateBatch {
    task: TranslationTask,
}

enum DoneMsg {
    Ok {
        task: TranslationTask,
        results: Vec<Translation>,
        latency_ms: u64,
    },
    Failed(TranslationTask),
}

/// State owned by the UI thread. WndProc receives a raw pointer to this
/// through GWLP_USERDATA; it is guaranteed to outlive the window because the
/// Box is only dropped after the message loop ends.
pub struct UiState {
    overlay: Option<OverlayWindow>,
    overlay_rx: Receiver<OverlayMsg>,
    overlay_tx: Sender<OverlayMsg>,
    config: AppConfig,
    config_path: PathBuf,
    mode: Mode,
    drag: Option<DragRect>,
    moving_index: Option<usize>,
    resize: Option<ResizeState>,
    /// Channels owned by the currently running pipeline (recreated per run).
    channels: Option<PipelineChannels>,
    pipeline: Option<JoinHandle<()>>,
}

/// Channels + shutdown flag for one pipeline run. Fresh instances are created
/// on every `start_pipeline` so a stopped pipeline can never interfere with a
/// restarted one.
struct PipelineChannels {
    cmd_tx: Sender<PipelineCmd>,
    cmd_rx: Receiver<PipelineCmd>,
    translate_tx: Sender<TranslateBatch>,
    translate_rx: Receiver<TranslateBatch>,
    done_tx: Sender<DoneMsg>,
    done_rx: Receiver<DoneMsg>,
    shutdown: Arc<AtomicBool>,
}

/// An in-progress region resize (original rect anchored; handle tells which
/// side the cursor drags).
struct ResizeState {
    index: usize,
    handle: ResizeHandle,
    orig: Rect,
}

impl PipelineChannels {
    fn new() -> Self {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<PipelineCmd>();
        let (translate_tx, translate_rx) = crossbeam_channel::unbounded::<TranslateBatch>();
        let (done_tx, done_rx) = crossbeam_channel::unbounded::<DoneMsg>();
        Self {
            cmd_tx,
            cmd_rx,
            translate_tx,
            translate_rx,
            done_tx,
            done_rx,
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl UiState {
    pub fn new(config: AppConfig, config_path: PathBuf) -> Self {
        let (overlay_tx, overlay_rx) = crossbeam_channel::unbounded::<OverlayMsg>();

        let mode = if has_workable_config(&config) {
            Mode::Work
        } else {
            Mode::Edit
        };

        Self {
            overlay: None,
            overlay_rx,
            overlay_tx,
            config,
            config_path,
            mode,
            drag: None,
            moving_index: None,
            resize: None,
            channels: None,
            pipeline: None,
        }
    }

    /// Create the overlay window. Must be called after `register_overlay_class`.
    pub fn create_overlay(&mut self) -> Result<()> {
        let instance = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }
            .map_err(|e| anyhow!("GetModuleHandleW: {e}"))?;
        let class_name = windows::core::w!("ScreenTranslatorOverlay");
        let ex_style = windows::Win32::UI::WindowsAndMessaging::WS_EX_TOPMOST
            | windows::Win32::UI::WindowsAndMessaging::WS_EX_LAYERED
            | windows::Win32::UI::WindowsAndMessaging::WS_EX_TOOLWINDOW
            | windows::Win32::UI::WindowsAndMessaging::WS_EX_NOACTIVATE;
        let hwnd = unsafe {
            windows::Win32::UI::WindowsAndMessaging::CreateWindowExW(
                ex_style,
                class_name,
                windows::core::w!("Screen Translator"),
                windows::Win32::UI::WindowsAndMessaging::WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )
        }
        .map_err(|e| anyhow!("CreateWindowExW: {e}"))?;

        // Pass a pointer to ourselves so WndProc can find us on WM_NCCREATE.
        let self_ptr = self as *mut UiState;
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
                hwnd,
                windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
                self_ptr as isize,
            );
        }

        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetTimer(
                Some(hwnd),
                TIMER_OVERLAY,
                TIMER_INTERVAL_MS,
                None,
            );
            let (toggle_vk, toggle_mods) = register_hotkey_with_fallback(
                hwnd,
                HOTKEY_TOGGLE_MODE,
                b"TYR",
            );
            let (exit_vk, exit_mods) = register_hotkey_with_fallback(
                hwnd,
                HOTKEY_EXIT,
                b"XQ",
            );
            log::info!(
                "hotkeys: toggle {} (mods {:?}), exit {} (mods {:?})",
                vk_to_str(toggle_vk),
                toggle_mods,
                vk_to_str(exit_vk),
                exit_mods
            );
        }

        self.overlay = Some(OverlayWindow::from_hwnd(hwnd, self.config.overlay_style.clone())?);
        self.set_mode(self.mode)?;
        Ok(())
    }

    fn set_mode(&mut self, mode: Mode) -> Result<()> {
        self.mode = mode;
        let work = matches!(mode, Mode::Work);
        let active = self.active_rect();
        if let Some(ov) = self.overlay.as_mut() {
            ov.set_mode(work)?;
            if work {
                ov.render_work(&[]);
            } else {
                ov.render_edit(&self.config.regions, active);
            }
        }
        Ok(())
    }

    /// Start the capture pipeline thread. No-op when already running.
    pub fn start_pipeline(&mut self) -> Result<()> {
        if self.pipeline.is_some() {
            return Ok(());
        }
        let regions: Vec<RegionConfig> = self
            .config
            .regions
            .iter()
            .filter(|r| r.enabled)
            .cloned()
            .collect();
        if regions.is_empty() {
            log::warn!("no enabled regions; staying in edit mode");
            return Ok(());
        }

        let ch = PipelineChannels::new();
        let overlay_tx = self.overlay_tx.clone();
        let cmd_rx = ch.cmd_rx.clone();
        let translate_tx = ch.translate_tx.clone();
        let translate_rx = ch.translate_rx.clone();
        let done_tx = ch.done_tx.clone();
        let done_rx = ch.done_rx.clone();
        let config = self.config.clone();
        let shutdown = ch.shutdown.clone();

        let handle = std::thread::Builder::new()
            .name("pipeline".into())
            .spawn(move || {
                run_pipeline(
                    regions,
                    config,
                    overlay_tx,
                    cmd_rx,
                    translate_tx,
                    translate_rx,
                    done_tx,
                    done_rx,
                    shutdown,
                )
            })
            .map_err(|e| anyhow!("spawn pipeline thread: {e}"))?;
        self.pipeline = Some(handle);
        self.channels = Some(ch);
        Ok(())
    }

    fn stop_pipeline(&mut self) {
        if let Some(handle) = self.pipeline.take() {
            if let Some(ch) = self.channels.take() {
                ch.shutdown.store(true, Ordering::SeqCst);
                let _ = ch.cmd_tx.send(PipelineCmd::Shutdown);
            }
            // The pipeline thread exits promptly (≤ one capture interval).
            let _ = handle.join();
        }
    }

    fn switch_mode(&mut self, work: bool) -> Result<()> {
        log::info!("switch mode -> {}", if work { "work" } else { "edit" });
        if work {
            self.start_pipeline()?;
        } else {
            self.stop_pipeline();
        }
        self.set_mode(if work { Mode::Work } else { Mode::Edit })
    }

    fn save_config(&self) {
        if let Err(e) = self.config.save(&self.config_path) {
            log::warn!("failed to save config: {e}");
        }
    }

    fn cursor_screen_pos() -> (i32, i32) {
        let mut p = POINT { x: 0, y: 0 };
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p);
        }
        (p.x, p.y)
    }

    fn on_timer(&mut self) {
        while let Ok(msg) = self.overlay_rx.try_recv() {
            let Some(ov) = self.overlay.as_mut() else {
                continue;
            };
            match msg {
                OverlayMsg::Draw(list) => {
                    if matches!(self.mode, Mode::Work) {
                        ov.render_work(&list);
                    }
                }
                OverlayMsg::Stats(s) => {
                    if matches!(self.mode, Mode::Work) {
                        ov.set_stats(s);
                    }
                }
                OverlayMsg::Clear => {
                    ov.clear();
                }
            }
        }
    }

    /// Rectangle being actively created/moved/resized (for overlay feedback).
    fn active_rect(&self) -> Option<Rect> {
        if let Some(d) = self.drag {
            return Some(drag_to_rect(d));
        }
        if let Some(rs) = &self.resize {
            let (x, y) = Self::cursor_screen_pos();
            return Some(apply_resize(rs.orig, rs.handle, x as f32, y as f32));
        }
        None
    }

    fn on_mouse_down(&mut self) {
        if matches!(self.mode, Mode::Work) {
            return;
        }
        let (x, y) = Self::cursor_screen_pos();
        let fx = x as f32;
        let fy = y as f32;
        self.drag = None;
        self.moving_index = None;

        // Resize handle takes priority over move/create.
        for (idx, r) in self.config.regions.iter().enumerate() {
            if let Some(handle) = hit_test_resize(r.rect, fx, fy) {
                log::debug!(
                    "resize start region {idx} handle {handle:?} at ({fx:.0},{fy:.0}) orig {:?}",
                    r.rect
                );
                self.resize = Some(ResizeState {
                    index: idx,
                    handle,
                    orig: r.rect,
                });
                let active = self.active_rect();
                if let Some(ov) = self.overlay.as_mut() {
                    ov.render_edit(&self.config.regions, active);
                }
                return;
            }
        }
        self.resize = None;

        if let Some(idx) = self.config.regions.iter().position(|r| r.rect.contains(fx, fy)) {
            self.moving_index = Some(idx);
        }
        self.drag = Some(DragRect {
            start: (x, y),
            current: (x, y),
        });
        let active = self.active_rect();
        if let Some(ov) = self.overlay.as_mut() {
            ov.render_edit(&self.config.regions, active);
        }
    }

    fn on_mouse_move(&mut self) {
        if matches!(self.mode, Mode::Work) {
            return;
        }
        if self.drag.is_none() && self.resize.is_none() {
            return;
        }
        let (x, y) = Self::cursor_screen_pos();
        if let Some(drag) = self.drag.as_mut() {
            drag.current = (x, y);
        }
        let active = self.active_rect();
        if let Some(ov) = self.overlay.as_mut() {
            ov.render_edit(&self.config.regions, active);
        }
    }

    fn on_mouse_up(&mut self) {
        if matches!(self.mode, Mode::Work) {
            return;
        }
        if let Some(rs) = self.resize.take() {
            let (x, y) = Self::cursor_screen_pos();
            let rect = apply_resize(rs.orig, rs.handle, x as f32, y as f32);
            log::debug!(
                "resize end handle {:?} cursor ({x},{y}) -> {:?}",
                rs.handle,
                rect
            );
            if let Some(r) = self.config.regions.get_mut(rs.index) {
                r.rect = rect;
            }
            self.save_config();
            if let Some(ov) = self.overlay.as_mut() {
                ov.render_edit(&self.config.regions, None);
            }
            return;
        }
        let Some(drag) = self.drag.take() else {
            return;
        };
        let rect = drag_to_rect(drag);
        let valid = rect.width >= 8.0 && rect.height >= 8.0;
        if valid {
            if let Some(idx) = self.moving_index {
                if let Some(r) = self.config.regions.get_mut(idx) {
                    r.rect = rect;
                }
            } else {
                let default_lang = self
                    .config
                    .regions
                    .first()
                    .map(|r| r.target_lang.clone())
                    .unwrap_or_else(|| "zh-CN".to_string());
                self.config.regions.push(RegionConfig {
                    id: format!("r{}", self.config.regions.len() + 1),
                    rect,
                    source_lang: self
                        .config
                        .regions
                        .first()
                        .and_then(|r| r.source_lang.clone()),
                    target_lang: default_lang,
                    enabled: true,
                });
            }
        }
        self.moving_index = None;
        self.save_config();
        if let Some(ov) = self.overlay.as_mut() {
            ov.render_edit(&self.config.regions, None);
        }
    }

    fn on_right_click(&mut self) {
        if matches!(self.mode, Mode::Work) {
            return;
        }
        let (x, y) = Self::cursor_screen_pos();
        let fx = x as f32;
        let fy = y as f32;
        self.config.regions.retain(|r| !r.rect.contains(fx, fy));
        self.save_config();
        if let Some(ov) = self.overlay.as_mut() {
            ov.render_edit(&self.config.regions, None);
        }
    }
}

fn vk_to_str(vk: u32) -> String {
    char::from_u32(vk).map(|c| c.to_string()).unwrap_or_else(|| format!("VK_{vk}"))
}

/// Try to register Ctrl+Alt+<candidate> for `id`; returns the first combo that
/// registers, or `(0, 0)` if all failed (some other app owns them).
fn register_hotkey_with_fallback(hwnd: HWND, id: i32, candidates: &[u8]) -> (u32, u32) {
    unsafe {
        for &vk in candidates {
            if RegisterHotKey(Some(hwnd), id, MOD_CONTROL | MOD_ALT, vk as u32).is_ok() {
                return (vk as u32, 0);
            }
        }
    }
    (0, 0)
}

fn has_workable_config(cfg: &AppConfig) -> bool {
    cfg.regions.iter().any(|r| r.enabled)
}

fn build_translator(cfg: &AppConfig) -> Box<dyn TranslationProvider> {
    match &cfg.translation {
        TranslationConfig::Local => Box::new(LocalProvider::default()),
        TranslationConfig::OpenAi(c) => Box::new(OpenAiProvider::new(c.clone())),
    }
}

/// One region being actively translated: its session plus the overlay items
/// currently on screen for it (already offset to screen coordinates).
struct RegionRunner {
    cfg: RegionConfig,
    session: RegionSession,
    overlays: Vec<OverlayText>,
}

/// Aggregate performance counters, logged periodically.
#[derive(Default)]
struct Metrics {
    frames: u64,
    capture_ms_total: u64,
    translate_calls: u64,
    translate_ms_total: u64,
    // process-level
    cpu_pct: f32,
    working_set_mb: f32,
}

/// Sample this process's CPU% (since the previous sample) and working set.
/// Returns (cpu_pct, working_set_mb).
fn sample_process_stats(prev: Option<(u64, std::time::Instant)>) -> (f32, f32, Option<(u64, std::time::Instant)>) {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    unsafe {
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let now = std::time::Instant::now();
        let handle = GetCurrentProcess();
        let ticks = if GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user).is_ok() {
            let to_u64 = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
            to_u64(kernel) + to_u64(user)
        } else {
            0
        };

        let mut mem = 0.0f32;
        let mut counters = windows::Win32::System::ProcessStatus::PROCESS_MEMORY_COUNTERS::default();
        let cb = std::mem::size_of::<windows::Win32::System::ProcessStatus::PROCESS_MEMORY_COUNTERS>() as u32;
        if windows::Win32::System::ProcessStatus::GetProcessMemoryInfo(handle, &mut counters, cb).is_ok() {
            mem = counters.WorkingSetSize as f32 / (1024.0 * 1024.0);
        }

        let cpu_pct = match prev {
            Some((prev_ticks, prev_time)) => {
                let dt_ns = now.duration_since(prev_time).as_nanos() as f64;
                if dt_ns > 0.0 {
                    ((ticks.saturating_sub(prev_ticks)) as f64 * 100.0 / dt_ns * 1000.0) as f32
                } else {
                    0.0
                }
            }
            None => 0.0,
        };
        (cpu_pct.max(0.0), mem, Some((ticks, now)))
    }
}

/// Runs capture + OCR + translation-queueing for the enabled regions.
#[allow(clippy::too_many_arguments)]
fn run_pipeline(
    regions: Vec<RegionConfig>,
    config: AppConfig,
    overlay_tx: Sender<OverlayMsg>,
    cmd_rx: Receiver<PipelineCmd>,
    translate_tx: Sender<TranslateBatch>,
    translate_rx: Receiver<TranslateBatch>,
    done_tx: Sender<DoneMsg>,
    done_rx: Receiver<DoneMsg>,
    shutdown: Arc<AtomicBool>,
) {
    log::info!(
        "pipeline thread started ({} region(s))",
        regions.len()
    );

    let mut capture = match DesktopCapture::new() {
        Ok(c) => c,
        Err(e) => {
            log::error!("capture init failed: {e:#}");
            return;
        }
    };

    // One OCR engine per region (each uses the region's source language).
    let mut runners: Vec<RegionRunner> = Vec::new();
    for cfg in regions {
        let ocr = match WindowsOcr::create(cfg.source_lang.as_deref().unwrap_or("")) {
            Ok(o) => o,
            Err(e) => {
                log::error!("OCR init failed for region {}: {e}", cfg.id);
                continue;
            }
        };
        let translator = build_translator(&config);
        let session = RegionSession::new(cfg.id.clone(), Box::new(ocr), translator, 2048);
        log::info!("region {} online (ocr={})", cfg.id, session.ocr_name());
        runners.push(RegionRunner {
            cfg,
            session,
            overlays: Vec::new(),
        });
    }
    if runners.is_empty() {
        log::error!("no regions could be started");
        return;
    }

    let batch_size = match &config.translation {
        TranslationConfig::OpenAi(c) => c.batch_size,
        _ => 8,
    };

    // Translation worker (network offloaded so OCR/capture stay fast).
    let worker_config = config.clone();
    let worker = std::thread::Builder::new()
        .name("translator".into())
        .spawn(move || translation_worker(worker_config, translate_rx, done_tx))
        .expect("spawn translator thread");

    let interval = Duration::from_secs_f64(1.0 / config.capture.fps.max(0.1) as f64);
    let mut metrics = Metrics::default();
    let mut last_stats_log = std::time::Instant::now();
    let mut proc_prev = None;

    loop {
        // Control + done messages.
        if let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                PipelineCmd::Shutdown => {
                    log::info!("pipeline shutdown requested");
                    break;
                }
            }
        }
        let mut dirty = false;
        while let Ok(done) = done_rx.try_recv() {
            match done {
                DoneMsg::Ok { task, results, latency_ms } => {
                    metrics.translate_calls += 1;
                    metrics.translate_ms_total += latency_ms;
                    if let Some(runner) = runners.iter_mut().find(|r| r.cfg.id == task.region_id) {
                        runner.session.accept_translations(&task, results);
                        let overlays = offset_overlays(
                            runner.session.refresh_overlays(&runner.cfg),
                            runner.cfg.rect.x,
                            runner.cfg.rect.y,
                        );
                        runner.overlays = overlays;
                        dirty = true;
                    } else {
                        log::warn!("translation result for unknown region {:?}", task.region_id);
                    }
                }
                DoneMsg::Failed(task) => {
                    log::warn!("translation failed for {} texts", task.texts.len());
                    if let Some(runner) = runners.iter_mut().find(|r| r.cfg.id == task.region_id) {
                        runner.session.note_failure(&task);
                    }
                }
            }
        }
        if dirty {
            send_combined_overlays(&runners, &overlay_tx);
        }

        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        // Capture and process each region in turn.
        let (ox, oy) = capture.origin();
        let frame_start = std::time::Instant::now();
        let mut dirty = false;
        for runner in runners.iter_mut() {
            let rx = (runner.cfg.rect.x as i64 - ox as i64).max(0) as u32;
            let ry = (runner.cfg.rect.y as i64 - oy as i64).max(0) as u32;
            match capture.capture_region(rx, ry, runner.cfg.rect.width as u32, runner.cfg.rect.height as u32) {
                Ok(Some(frame)) => {
                    use screen_translator_core::session::UpdateResult;
                    match runner.session.process_frame(&frame, &runner.cfg) {
                        UpdateResult::Redraw { overlays } => {
                            log::debug!(
                                "region {} redraw: {} overlay items",
                                runner.cfg.id,
                                overlays.len()
                            );
                            runner.overlays = offset_overlays(overlays, runner.cfg.rect.x, runner.cfg.rect.y);
                            dirty = true;
                        }
                        UpdateResult::NoChange => {}
                    }
                    // Queue new translation work (chunked to provider batch size).
                    for task in runner.session.drain_requests() {
                        for chunk in task.texts.chunks(batch_size) {
                            let t = TranslationTask {
                                region_id: task.region_id.clone(),
                                source_lang: task.source_lang.clone(),
                                target_lang: task.target_lang.clone(),
                                texts: chunk.to_vec(),
                            };
                            let _ = translate_tx.send(TranslateBatch { task: t });
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    log::warn!("capture error: {e:#}");
                }
            }
        }
        if dirty {
            send_combined_overlays(&runners, &overlay_tx);
        }
        metrics.capture_ms_total += frame_start.elapsed().as_millis() as u64;
        metrics.frames += 1;

        if last_stats_log.elapsed() >= Duration::from_secs(10) {
            let (cpu, mem, next) = sample_process_stats(proc_prev);
            proc_prev = next;
            metrics.cpu_pct = cpu;
            metrics.working_set_mb = mem;
            log_stats(&metrics, &mut runners, interval);
            last_stats_log = std::time::Instant::now();
            metrics = Metrics::default();
        } else if config.show_stats {
            // Refresh the on-screen HUD more frequently.
            let (cpu, mem, next) = sample_process_stats(proc_prev);
            proc_prev = next;
            let snapshot = build_stats_snapshot(&metrics, &runners, interval, cpu, mem);
            let _ = overlay_tx.send(OverlayMsg::Stats(snapshot));
        }

        std::thread::sleep(interval);
    }

    // Dropping `translate_tx` signals the worker to stop.
    drop(translate_tx);
    let _ = worker.join();
    let _ = overlay_tx.send(OverlayMsg::Clear);
}

/// Build an on-screen HUD snapshot from interval metrics.
fn build_stats_snapshot(
    metrics: &Metrics,
    runners: &[RegionRunner],
    interval: Duration,
    cpu: f32,
    mem: f32,
) -> StatsSnapshot {
    let frames = metrics.frames.max(1);
    let mut ocr_ms = 0u64;
    let mut ocr_runs = 0u64;
    let mut pending = 0usize;
    let mut tcache_hits = 0u64;
    let mut tcache_misses = 0u64;
    for r in runners.iter() {
        let d = r.session.peek_stats();
        ocr_runs += d.ocr_runs;
        ocr_ms += d.ocr_ms;
        pending += d.pending;
        tcache_hits += d.translation_cache_hits;
        tcache_misses += d.translation_cache_misses;
    }
    let hit = if tcache_hits + tcache_misses > 0 {
        tcache_hits as f32 / (tcache_hits + tcache_misses) as f32
    } else {
        0.0
    };
    StatsSnapshot {
        fps: metrics.frames as f32 / 10.0,
        target_fps: (1.0 / interval.as_secs_f64()) as f32,
        capture_ms: metrics.capture_ms_total as f32 / frames as f32,
        ocr_ms: if ocr_runs > 0 {
            ocr_ms as f32 / ocr_runs as f32
        } else {
            0.0
        },
        translate_ms: if metrics.translate_calls > 0 {
            metrics.translate_ms_total as f32 / metrics.translate_calls as f32
        } else {
            0.0
        },
        ocr_runs,
        pending,
        cache_hit: hit,
        cpu,
        mem_mb: mem,
    }
}
fn send_combined_overlays(runners: &[RegionRunner], overlay_tx: &Sender<OverlayMsg>) {
    let mut all: Vec<OverlayText> = Vec::new();
    for r in runners {
        all.extend(r.overlays.iter().cloned());
    }
    if !all.is_empty() {
        let sample: Vec<String> = all
            .iter()
            .map(|o| format!("({:.0},{:.0} {}x{})", o.box_.x, o.box_.y, o.box_.width, o.box_.height))
            .collect();
        log::debug!("overlay draw: {} item(s): {sample:?}", all.len());
    }
    if overlay_tx.send(OverlayMsg::Draw(all)).is_err() {
        log::debug!("overlay channel closed");
    }
}

fn log_stats(metrics: &Metrics, runners: &mut [RegionRunner], interval: Duration) {
    // Consume interval counters from each session (deltas since last log).
    let mut ocr_runs = 0u64;
    let mut ocr_ms = 0u64;
    let mut tcache_hits = 0u64;
    let mut tcache_misses = 0u64;
    let mut ocr_hits = 0u64;
    let mut pending = 0usize;
    for r in runners.iter_mut() {
        let d = r.session.take_stats_delta();
        ocr_runs += d.ocr_runs;
        ocr_ms += d.ocr_ms;
        tcache_hits += d.translation_cache_hits;
        tcache_misses += d.translation_cache_misses;
        ocr_hits += d.ocr_cache_hits;
        pending += d.pending;
    }
    let fps = metrics.frames as f64 / 10.0;
    let avg_ocr = if ocr_runs > 0 { ocr_ms as f64 / ocr_runs as f64 } else { 0.0 };
    let avg_capture = if metrics.frames > 0 {
        metrics.capture_ms_total as f64 / metrics.frames as f64
    } else {
        0.0
    };
    let avg_translate = if metrics.translate_calls > 0 {
        metrics.translate_ms_total as f64 / metrics.translate_calls as f64
    } else {
        0.0
    };
    let hit_rate = if tcache_hits + tcache_misses > 0 {
        tcache_hits as f32 / (tcache_hits + tcache_misses) as f32
    } else {
        0.0
    };
    log::info!(
        "stats: fps={fps:.1} (target {target:.1}) capture_ms={avg_capture:.1} ocr_ms={avg_ocr:.1} \
         translate_ms={avg_translate:.1} ocr_runs={ocr_runs} ocr_cache_hits={ocr_hits} pending={pending} \
         tc_hit={hit_rate:.2} cpu={cpu:.0}% mem={mem:.0}MB",
        fps = fps,
        target = 1.0 / interval.as_secs_f64(),
        avg_capture = avg_capture,
        avg_ocr = avg_ocr,
        avg_translate = avg_translate,
        ocr_runs = ocr_runs,
        ocr_hits = ocr_hits,
        pending = pending,
        hit_rate = hit_rate,
        cpu = metrics.cpu_pct,
        mem = metrics.working_set_mb,
    );
}

fn translation_worker(
    config: AppConfig,
    translate_rx: Receiver<TranslateBatch>,
    done_tx: Sender<DoneMsg>,
) {
    let provider = build_translator(&config);
    while let Ok(batch) = translate_rx.recv() {
        let req = TranslateRequest {
            texts: batch.task.texts.clone(),
            source_lang: batch.task.source_lang.clone(),
            target_lang: batch.task.target_lang.clone(),
        };
        let t0 = std::time::Instant::now();
        match provider.translate(&req) {
            Ok(results) => {
                let latency_ms = t0.elapsed().as_millis() as u64;
                let _ = done_tx.send(DoneMsg::Ok {
                    task: batch.task,
                    results,
                    latency_ms,
                });
            }
            Err(e) => {
                log::warn!("translate error: {e}");
                let _ = done_tx.send(DoneMsg::Failed(batch.task));
            }
        }
    }
}

fn offset_overlays(mut overlays: Vec<OverlayText>, dx: f32, dy: f32) -> Vec<OverlayText> {
    for o in &mut overlays {
        o.box_.x += dx;
        o.box_.y += dy;
    }
    overlays
}

/// The overlay window's window proc. Registered as the class proc.
pub unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging as wm;
    let state_ptr = wm::GetWindowLongPtrW(hwnd, wm::GWLP_USERDATA) as *mut UiState;

    match msg {
        wm::WM_NCCREATE => {
            let create = lparam.0 as *const wm::CREATESTRUCTW;
            if !create.is_null() {
                let cs = &*create;
                wm::SetWindowLongPtrW(hwnd, wm::GWLP_USERDATA, cs.lpCreateParams as isize);
            }
            return LRESULT(1);
        }
        wm::WM_TIMER => {
            if let Some(state) = state_ptr.as_mut() {
                state.on_timer();
            }
            return LRESULT(0);
        }
        wm::WM_LBUTTONDOWN => {
            if let Some(state) = state_ptr.as_mut() {
                // Capture the mouse so we keep receiving move/up during a drag.
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(hwnd);
                state.on_mouse_down();
            }
            return LRESULT(0);
        }
        wm::WM_MOUSEMOVE => {
            if let Some(state) = state_ptr.as_mut() {
                state.on_mouse_move();
            }
            return LRESULT(0);
        }
        wm::WM_LBUTTONUP => {
            if let Some(state) = state_ptr.as_mut() {
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                state.on_mouse_up();
            }
            return LRESULT(0);
        }
        wm::WM_RBUTTONUP => {
            if let Some(state) = state_ptr.as_mut() {
                state.on_right_click();
            }
            return LRESULT(0);
        }
        wm::WM_KEYDOWN => {
            let vk = wparam.0 as u32;
            if let Some(state) = state_ptr.as_mut() {
                if vk == 0x1B {
                    // ESC cancels drag.
                    state.drag = None;
                    if let Some(ov) = state.overlay.as_mut() {
                        ov.render_edit(&state.config.regions, None);
                    }
                }
            }
            return LRESULT(0);
        }
        wm::WM_HOTKEY => {
            if let Some(state) = state_ptr.as_mut() {
                let id = wparam.0 as i32;
                match id {
                    HOTKEY_TOGGLE_MODE => {
                        log::debug!("hotkey toggle");
                        let go_work = state.mode == Mode::Edit;
                        let _ = state.switch_mode(go_work);
                    }
                    HOTKEY_EXIT => {
                        log::debug!("hotkey exit");
                        let _ = wm::DestroyWindow(hwnd);
                    }
                    _ => {}
                }
            }
            return LRESULT(0);
        }
        wm::WM_ERASEBKGND => return LRESULT(1),
        wm::WM_DESTROY => {
            wm::PostQuitMessage(0);
            return LRESULT(0);
        }
        _ => {}
    }
    wm::DefWindowProcW(hwnd, msg, wparam, lparam)
}







