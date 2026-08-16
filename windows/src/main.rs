mod app;
mod capture;
mod debugui;
mod ocr_winrt;
mod overlay;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;

fn config_path() -> PathBuf {
    let mut p = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("config.json"));
    p.set_file_name("config.json");
    p
}

fn main() -> Result<()> {
    let open_debug = std::env::args().any(|a| a == "--debug");

    // Make our coordinate space physical pixels, not DPI-scaled. DXGI capture
    // and cursor positions must be in the same space as the overlay window.
    let dpi_ok = unsafe {
        windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
        .is_ok()
    };
    if !dpi_ok {
        // Fall back to system DPI awareness (still physical for a single scale).
        let _ = unsafe {
            windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
            )
        };
    }
    log::info!("DPI awareness: per-monitor v2 = {dpi_ok}");

    let path = config_path();
    let config = screen_translator_core::config::AppConfig::load_or_default(&path);
    if !path.exists() {
        // Write a starter config so the user can find and edit it.
        if let Err(e) = config.save(&path) {
            log::warn!("could not write starter config: {e}");
        } else {
            log::info!("wrote starter config to {}", path.display());
        }
    }

    let shared = Arc::new(Mutex::new(debugui::DebugShared::new(config.clone(), path.clone())));
    {
        let shared_log = debugui::SharedLog::new(shared.clone());
        log::set_boxed_logger(Box::new(shared_log))
            .map_err(|e| anyhow::anyhow!("set_logger: {e}"))?;
        log::set_max_level(log::LevelFilter::Info);
    }

    if open_debug {
        debugui::spawn_debug_panel(shared.clone());
    }

    let cfg = shared.lock().unwrap().config.clone();
    log::info!(
        "translation provider: {}",
        match cfg.translation {
            screen_translator_core::config::TranslationConfig::Local => "local (echo)".to_string(),
            screen_translator_core::config::TranslationConfig::OpenAi(c) => c.base_url,
        }
    );

    overlay::register_overlay_class()?;

    let mut ui = app::UiState::new(config, path, shared.clone());    ui.create_overlay()?;
    ui.start_pipeline()?; // no-op when there are no enabled regions

    log::info!(
        "Screen Translator running. Toggle hotkey = Ctrl+Alt+key (see 'hotkeys:' line above), quit = Ctrl+Alt+key.{}",
        if open_debug { " Debug panel open (--debug)." } else { "" }
    );

    unsafe {
        let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
        while windows::Win32::UI::WindowsAndMessaging::GetMessageW(&mut msg, None, 0, 0).as_bool()
        {
            let _ = windows::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
            let _ = windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
        }
    }
    Ok(())
}
