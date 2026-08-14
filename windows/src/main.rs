mod app;
mod capture;
mod ocr_winrt;
mod overlay;

use std::path::PathBuf;

use anyhow::Result;

fn config_path() -> PathBuf {
    let mut p = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("config.json"));
    p.set_file_name("config.json");
    p
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    // Make our coordinate space physical pixels, not DPI-scaled.
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }

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
    log::info!(
        "translation provider: {}",
        match config.translation {
            screen_translator_core::config::TranslationConfig::Local => "local (echo)",
            screen_translator_core::config::TranslationConfig::OpenAi(ref c) => {
                c.base_url.as_str()
            }
        }
    );
    log::info!(
        "regions: {} (mode: {})",
        config.regions.len(),
        if config.regions.iter().any(|r| r.enabled) { "work" } else { "edit" }
    );

    overlay::register_overlay_class()?;

    let mut ui = app::UiState::new(config, path);
    ui.create_overlay()?;
    ui.start_pipeline()?; // no-op when there are no enabled regions

    log::info!(
        "Screen Translator running. Toggle hotkey = Ctrl+Alt+key (see 'hotkeys:' line above), quit = Ctrl+Alt+key."
    );

    unsafe {
        let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
        while windows::Win32::UI::WindowsAndMessaging::GetMessageW(
            &mut msg,
            None,
            0,
            0,
        )
        .as_bool()
        {
            let _ = windows::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
            let _ = windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);        }
    }
    Ok(())
}
